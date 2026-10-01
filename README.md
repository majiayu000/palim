# Palim

A Rust library for **reversible JSON structural diffs**, array identity matching
and moves, standard patches, Merge Patch and configurable comparison reports.
It reads and writes the JSON delta format used by
[jsondiffpatch](https://github.com/benjamine/jsondiffpatch), and exports RFC 6902
JSON Patch operations. This is an independently implemented JSON core, using
LIS for unique array identities, `imara-diff` for other sequence matching and
`json-patch` for standard patch application.

The package is implemented locally and has not been published. Minimum Rust: **1.85**.

## Example

```rust
use palim::{Delta, DiffOptions, DiffPatcher, apply_json_patch, patch, unpatch};
use serde_json::{Value, json};
use std::sync::Arc;

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let engine = DiffPatcher::new(DiffOptions {
    object_hash: Some(Arc::new(|item, _| item.get("id").map(Value::to_string))),
    ..Default::default()
});
let before = json!([{ "id": 1, "score": 10 }, { "id": 2, "score": 20 }]);
let after = json!([{ "id": 2, "score": 21 }, { "id": 1, "score": 10 }]);
let delta = engine.diff(&before, &after)?.ok_or("expected a change")?;

assert_eq!(patch(&before, &delta)?, after);
assert_eq!(unpatch(&after, &delta)?, before);

// Store the complete, reversible delta as ordinary JSON.
let wire = serde_json::to_string(&delta)?;
let restored: Delta = serde_json::from_str(&wire)?;
assert_eq!(restored, delta);

// Interoperate with clients that accept RFC 6902 instead.
let standard = delta.to_json_patch(&before)?;
assert_eq!(apply_json_patch(&before, &standard)?, after);
# Ok(())
# }
```

For local use, add a path dependency pointing at this directory, plus `serde_json`.

```toml
[dependencies]
palim = { path = "../palim" }
serde_json = "1.0"
```

## Features

- All JSON value types, nested objects and arrays, and root type changes.
- Array insertions, deletions, multiple moves, and edits inside moved items.
- Custom object identities, deterministic repeated-identity pairing, positional
  matching, custom one-to-one matchers, optional move detection and moved values.
- Object property filtering and node filters for roots, missing values and array positions.
- Complete deltas, serialization, strict application, inverse deltas and undo.
- Multi-hunk Unicode text diffs with diff-match-patch text encoding and UTF-16 coordinates.
- Opt-in fuzzy text application with displacement and error bounds.
- Explicit `ForwardDelta` for omitting old values when undo is unnecessary.
- RFC 6902 export with correctly escaped pointers; standard add/remove/replace/move/copy/test application.
- Optimized standard diff with cross-path move/copy, byte-based subtree replacement,
  old-value tests and baseline-dependent inversion of all six operations.
- RFC 7396 Merge Patch application, generation and representable composition.
- Comparison reports with path-dependent unordered multisets, custom equality,
  numeric tolerances, identity moves, similarity and work/output budgets.
- Arbitrary precision JSON numbers; mathematical numeric equality for standard tests
  and reports, without expanding huge exponents.
- Consuming application APIs, atomic in-place APIs and cumulative copy byte limits.
- Immutable inputs, errors with paths, no global configuration, shareable instances.

## API

| Operation | Result |
|---|---|
| `diff(&left, &right)` / `engine.diff(..)` | `Result<Option<Delta>, Error>` |
| `patch(&left, &delta)` | `Result<Value, Error>` |
| `patch_owned(left, &delta)` / `patch_in_place(&mut left, &delta)` | Consuming / atomic native application |
| `patch_fuzzy(&left, &delta, &options)` / `apply_text_patch(source, text, &options)` | Bounded approximate text application |
| `reverse(&delta)` | `Result<Delta, Error>`; uses the original delta |
| `unpatch(&right, &delta)` | `Result<Value, Error>` |
| `Delta::from_value(value)` | Validates protocol structure |
| `delta.as_value()` / `delta.into_value()` | Protocol JSON |
| `delta.to_json_patch(&left)` | Validated `Result<Patch, Error>` |
| `apply_json_patch(&left, &patch)` | Atomic `Result<Value, Error>` |
| `apply_json_patch_owned(left, &patch)` / `apply_json_patch_in_place(&mut left, &patch)` | Consuming / atomic standard application |
| `apply_json_patch_with_options(&left, &patch, &limits)` | Standard application with depth and copy budgets |
| `diff_json_patch(&left, &right, &options)` / `engine.diff_json_patch(..)` | Optimized `Result<Patch, Error>` |
| `invert_json_patch(&left, &patch)` | Inverse standard operations, using the baseline |
| `merge_patch(&left, &patch)` / `merge_patch_in_place(..)` | RFC 7396 application |
| `diff_merge_patch(&left, &right)` / `compose_merge_patches(&first, &second)` | Patch generation / composition; errors for unrepresentable cases |
| `compare(&left, &right, &options)` | `Result<CompareReport, Error>`; a report, not a patch |
| `delta.to_forward_only()` | `ForwardDelta`; apply with `.patch(&left)` |

`None` represents an unchanged document (or no changes after filtering). Serializing
`Option<Delta>` writes JSON `null` for None; a standalone Delta cannot be JSON null.
`DiffPatcher` also exposes patch/reverse/unpatch convenience methods; matching
options affect diff generation only.

### Matching options

`DiffOptions::default()` enables moves, uses empty move placeholders, matches
unkeyed objects/nested arrays by position, enables text diffs at **60 UTF-16 units**,
and limits document nesting to 128 containers.

`object_hash` is called once for every object/array item in each changed array.
A returned identity takes priority. When a callback returns None, the complete
JSON value is used. Without a callback, `match_by_position` controls positional
matching for objects and nested arrays; primitives always use exact values.
Repeated keys are permitted and paired deterministically. Identity ambiguity
can make a delta larger; choose stable unique identities when possible.

Set `detect_moves: false` to produce remove/add pairs, or
`include_value_on_move: true` to include moved values. Set
`text_diff_min_length: None` to replace strings as whole values.

The property filter receives `(name, left_parent, right_parent, parent_pointer)`.
Returning false excludes that property; patch retains the source value of excluded
properties, so its output can intentionally differ from the unfiltered target.

`node_filter` receives `(pointer, optional_left, optional_right)`. Root exclusion
returns no delta. For an array, the filter projects original positions: an excluded
position keeps its source value (or stays absent), then the projected array is
matched. Included additions of containers recursively filter their children;
container deletions retain excluded descendants. Replacements with a container
filter its new descendants; replacing a container with a scalar is atomic at the
parent, so descendant filters do not run. Nested array filters see original target
indices even when earlier positions are omitted; delta indices refer to the final
projected array. Excluded subtrees are retained without visiting their descendants.
Filters should be deterministic.

`array_item_matcher` receives `(array_pointer, left_item, right_item)`. It uses
maximum one-to-one pairing followed by LIS; ambiguity does not guarantee globally
minimum edits. `DiffOptions` rejects supplying both this matcher and `object_hash`.
Callbacks over every candidate pair can be expensive; stable IDs are faster.

### Standard patch optimization

`JsonPatchOptions` enables `factorize` and `rationalize` by default; `tests` is
off by default. Factorization uses move/copy only when it saves serialized bytes
and preserves the target. Rationalization considers replacing parent subtrees,
including the root, using actual UTF-8 patch bytes. This is a verified heuristic,
not a globally optimal compressor. Enable `tests` to guard old values; absent keys
and array insertions use parent snapshots because RFC 6902 has no absence test.

`invert_json_patch` consumes the original operations and their baseline values,
not a re-diff. Inverting an overwrite or ancestor move may require multiple
operations. `JsonPatchApplyOptions::max_copy_bytes` counts cumulative serialized
source bytes before each copy; moves do not count. Consuming APIs avoid the initial
document clone. Atomic in-place APIs compute privately and commit after success;
they are not zero-copy mutation APIs.

### Comparison reports and Merge Patch

`CompareOptions` supports `custom_equal`, node filters, per-path `unordered`,
`object_hash`, `array_item_matcher`, absolute/relative tolerance and two optional
budgets. Unordered arrays retain duplicate multiplicity and use maximum matching;
their order is not reported as a change. Ordered identity comparisons report
changed indices, not a minimal move script. The report distinguishes missing
values from JSON null in its serialized representation.

Similarity is the fraction of matching terminal/absent/type-change units. Empty
containers count as one unit; each reported move adds an unmatched unit; filtered
nodes contribute none. An empty or fully filtered comparison has similarity 1.
Exceeded budgets return `Error`, never a partial report. Similarity is this
library's metric, not a universal tree distance. Reports do not promise a delta
that reconstructs the original target after ignoring order or values.

Numeric tolerance checks the exact decimal inequality
`abs(a - b) <= max(absolute, relative * max(abs(a), abs(b)))`.
The finite f64 options denote their shortest JSON decimal values: `0.2` means
decimal `0.2`. Inputs stay arbitrary precision, including huge symbolic exponents;
no conversion of inputs to f64 or expansion of exponent gaps occurs. Decimal digit
work counts toward `max_comparisons` and `visited`.
`array_item_matcher` takes precedence over `object_hash` in reports.

Merge Patch treats object-member null as deletion. It cannot assign a new null
member, so `diff_merge_patch` rejects unrepresentable targets rather than changing
their meaning. Composition is independent of a baseline and likewise rejects
reset-then-object combinations that no single Merge Patch can represent for
every source. JSON null inside arrays and root null are supported normally.

### Wire format

| Change | JSON |
|---|---|
| Added | `[new]` |
| Replaced | `[old, new]` |
| Deleted | `[old, 0, 0]` |
| Object | `{ "property": child_delta }` |
| Array | `{ "_t": "a", "target_index": child_delta, "_source_index": deletion_or_move }` |
| Moved array item | `["", target_index, 3]`, under `_source_index` |
| Text | `["@@ ...\n", 0, 2]` |

Array indices must be canonical nonnegative decimal integers. Moves use original
source positions and final target positions; they are converted to sequential
positions when exporting JSON Patch. JSON Patch text edits become replace operations.

Keep complete old values for undo. `ForwardDelta` explicitly discards old
replacement/deletion values and has no reverse API. The wire protocol cannot
distinguish a real old value of 0 from an omitted-value placeholder: decode
forward-only wire data as `ForwardDelta`, never as `Delta`.

## Error and interoperability contract

Inputs are never modified. Invalid delta structure, bad indices, duplicate insertion
destinations, incompatible types, missing/existing properties and mismatched old
replacement/deletion values return `Error`. Default text application requires exact
content at its expected position. Approximate application is explicit via the
fuzzy APIs; non-text old-value checks remain strict. Fuzzy displacement uses UTF-16
units, while its error ratio uses Unicode scalar edit counts. Long hunks use bounded
anchors and heuristic alignment, which can reject a valid approximate match; this
is not a byte-for-byte clone of DMP's fuzzy matcher. Any failed hunk returns an error.
Unchanged properties and placeholder-only moved values do not authenticate the
entire source document. Included moved values are checked, except the wire format's
empty-string placeholder. Callers must supply the correct baseline.

JSON has no absent-root value. A root deletion returns an error rather than mapping
absence to null. Diffing two existing JSON documents always uses replacement for
root changes, so those generated deltas can be undone normally.

Text patches use the JavaScript UTF-16 coordinate convention. Inverse text hunks
are applied in reverse chronological order, including overlapping contexts. The
decoder accounts for the equal old/new context-length offset emitted after surrogate
repair by `@dmsnell/diff-match-patch` 1.1.0; inconsistent edit lengths remain errors.

Long-text generation first matches line groups, then refines small changed regions
into character edits. Each token diff is limited to 4096 combined input tokens.
Larger regions use exact replacements, so patches remain reversible but can be
coarser, particularly after line insertions change group alignment. This bound
applies to generation, not to the separate fuzzy alignment algorithm.

The implementation does not promise identical delta bytes or globally smallest
patches. Unique array tokens use O(n log n) LIS and yield the minimum number of
single-item moves for pure permutations. Repeated tokens retain heuristic Histogram
matching above a bounded exact-LCS candidate limit. Within that limit the stable
subsequence is exact; this does not guarantee minimum edits for arbitrary custom
matchers or minimum serialized bytes. Standard export emits at most one operation
per native move; Fenwick ranks compute sequential move positions in
O(n + moves × log n) time and O(n + moves) space. Native deltas rebuild
arrays once, without repeated vector removals or a quadratic LCS matrix.

Documents and intermediate standard patch results are bounded at 128 containers;
deltas (including tuple wrappers) are bounded at **127** to round-trip through
serde_json's default reader. A near-limit source can therefore produce a depth
error if its delta exceeds the limit. Lower `max_depth` when needed;
values above 128 are rejected. Sequence lengths are checked against imara's i32 limit.

Numbers enable serde_json's `arbitrary_precision` and `float_roundtrip` features.
Native deltas retain numeric representation changes; standard `test` and reports
compare decimal numeric values (`1`, `1.0` and `1e0` are equal). Parse exact decimal
inputs from JSON text: converting a Rust f64 to JSON already chooses its shortest
decimal representation. Large numbers cannot be preserved by ordinary JS Number
consumers. For standard patches containing large integers, deserialize JSON text
with `serde_json::from_str::<Patch>`; the upstream tagged enum's `from_value` path
has a u128 buffering limitation. Generated operations use typed construction and
preserve those numbers. Strings must be valid Unicode.
Identity/filter callbacks are caller code; panic or allocation failure is not caught.

JS Date/undefined/functions, browser HTML/CSS/animations, a product CLI and language
bindings are outside this JSON library. See [DESIGN.md](DESIGN.md) for the design
and [BENCHMARK.md](BENCHMARK.md) for measured results and limitations.

## Verification and benchmarks

```sh
cargo test --locked
cargo test --release --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --check
cargo +1.85.0 check --lib --locked
cargo bench --bench core --locked -- --noplot
```

Optional JavaScript interoperability and comparison (requires Node 20+, Python 3
and npm; Node 24.14.0 was tested):

```sh
npm ci --prefix tools --ignore-scripts --no-audit --no-fund
cargo build --release --example fixture_runner --locked
node tools/interop.mjs
python3 tools/benchmark.py
cargo build --release --example standard_bench --locked
python3 tools/standard-benchmark.py --task pipeline
python3 tools/standard-benchmark.py --task export
python3 tools/standard-benchmark.py --task apply
python3 tools/standard-benchmark.py --task inverse
```

Structured fuzzing uses a separate developer workspace and requires nightly Rust:

```sh
cargo install cargo-fuzz --version 0.13.2 --locked
cargo +nightly fuzz run core -- -max_total_time=115 -max_len=2048 -rss_limit_mb=1024 -seed=20261001
```

The CI workflow runs Rust tests on Linux, Windows and macOS, plus MSRV,
JavaScript interoperability and a 60-second fuzz smoke test. Local cross-target
library checks establish compilation only; see the verification record for checks
that actually ran.

The default Rust suite has no network or Node dependency. It includes deterministic
regressions, 373 saved document pairs, 92 active public JSON Patch cases, exhaustive
unique permutations through length seven, and fixed-seed property suites for trees,
real array deltas, matching, text, Merge Patch and standard inverses. JS verification saves every
delta and checks 1,073 cases in both directions, including complex repeated identities
and Unicode text. Upstream JS reverse failures are reported separately, while Rust's
own inverse must restore every source.

MIT for this implementation. Dependencies retain their respective licenses.
Upstream-derived test fixtures are Apache-2.0; see `tests/fixtures/NOTICE.md`.
