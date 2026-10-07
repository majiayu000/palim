# Palim API guide

Start with the [README](https://github.com/majiayu000/palim/blob/main/README.md)
for installation, a short example and help choosing an API. This guide covers
matching, patch formats, application contracts and interoperability. Individual
function signatures are in the [API documentation](https://docs.rs/palim).

Palim is an independently implemented JSON core, using LIS for unique array
identities, `imara-diff` for other sequence matching and `json-patch` for standard
patch application. JS Date/undefined/functions, browser HTML/CSS/animations,
a product CLI and language bindings are outside this JSON library.

## Choosing a change format

- **Native `Delta`:** keeps the old values needed for `unpatch` and `reverse`.
  Use it for history, undo/redo and jsondiffpatch interoperability. Apply it with
  `patch`, not `apply_json_patch`.
- **RFC 6902 `Patch`:** a standard sequence of operations for other clients.
  Generate it with `diff_json_patch`, or export an existing native delta with
  `Delta::to_json_patch`. Inversion requires the original baseline.
- **`CompareReport`:** describes differences, optionally ignoring order or small
  numeric changes. It is a report and cannot be applied as a patch.

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
- Opt-in arbitrary precision JSON numbers; mathematical numeric equality for standard tests
  and reports, without expanding huge exponents.
- Consuming application APIs, atomic in-place APIs and cumulative copy byte limits.
- Borrowed input preservation, errors with paths, no global configuration, shareable instances.

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
| `test_json_patch(&left, &tests)` | Read-only standard Test operations, without cloning the document |
| `diff_json_patch(&left, &right, &options)` / `engine.diff_json_patch(..)` | Optimized `Result<Patch, Error>` |
| `invert_json_patch(&left, &patch)` | Inverse standard operations, using the baseline |
| `invert_json_patch_guarded(&left, &patch)` | Inverse operations with tests of the values and containers being undone |
| `merge_patch(&left, &patch)` / `merge_patch_in_place(..)` | RFC 7396 application |
| `diff_merge_patch(&left, &right)` / `compose_merge_patches(&first, &second)` | Patch generation / composition; errors for unrepresentable cases |
| `compare(&left, &right, &options)` | `Result<CompareReport, Error>`; a report, not a patch |
| `delta.to_forward_only()` | `ForwardDelta`; apply with `.patch(&left)` |

`None` represents an unchanged document (or no changes after filtering). Serializing
`Option<Delta>` writes JSON `null` for None; a standalone Delta cannot be JSON null.
`DiffPatcher` also exposes patch/reverse/unpatch convenience methods; matching
options affect diff generation only.

### Matching options

For the common stable-ID case, `DiffPatcher::by_key("id")` configures `object_hash`
using the JSON representation of that property and keeps the default options.
The key is a literal field name, not a JSON Pointer; string and numeric IDs remain
distinct. Missing keys fall back to the complete item value; repeated IDs are allowed and
paired deterministically. It does not validate ID uniqueness. Use
`DiffPatcher::new(DiffOptions { .. })` for custom callbacks or other options.

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
Since 0.2.0, guard generation reuses already authenticated subtrees and invalidates
affected records when array indices shift. This avoids repeatedly emitting full
parent snapshots for the measured insertion and migration cases. Guard bytes and
operation counts can differ from earlier releases; the output remains RFC 6902.
With guards and moves, rationalization tries broader parents first to avoid
repeated container snapshots; other patches try deeper parents first. An accepted
parent replacement can discard more detailed edits. Neither order promises the
smallest possible patch.

With `factorize`, `rationalize` and `tests` all disabled, direct `diff_json_patch`
generation can use positional replacements for unique primitive permutations
when moves offer little estimated byte saving. It accepts exact replacement cost
up to 110% of estimated move cost, using the widest array index for the estimate;
this does not guarantee actual output grows by at most 10%. Custom item matchers,
filters and near-depth-limit inputs retain the existing path. Native `diff` and
`Delta::to_json_patch` retain their move strategy. The measured rotations and
long-value reorders keep byte-identical move output.

Private RFC maps and sets use `foldhash`, a noncryptographic hash with a different
collision-resistance contract from SipHash.

`invert_json_patch` consumes the original operations and their baseline values,
not a re-diff. Inverting an overwrite or ancestor move may require multiple
operations. `JsonPatchApplyOptions::max_copy_bytes` counts cumulative serialized
source bytes before each copy; moves do not count. Consuming APIs avoid the initial
document clone. Atomic in-place APIs compute privately and commit after success;
they are not zero-copy mutation APIs.

`test_json_patch` accepts only Test operations and checks them in order against
a borrowed document. It retains the standard depth limit and mathematical number
equality. It avoids a document clone, but still validates source depth and visits
the tested values. Mutating operations are rejected at their position.

Use `invert_json_patch_guarded` when a saved inverse might be applied to a changed
target. Its tests verify the values and containers involved in undoing each
operation, rather than authenticating the entire document. Restoring an absent key
or inserting into an array requires a parent snapshot; that snapshot can also
reject changes to unrelated siblings and can make the inverse larger.

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
decimal `0.2`. The comparison uses the decimal representation of the supplied
serde_json numbers; ordinary parsing may already have rounded those inputs.
With `exact-numbers`, inputs retain arbitrary precision, including huge symbolic
exponents, without conversion to f64 or expansion of exponent gaps. Decimal digit
work counts toward `max_comparisons` and `visited`.
`array_item_matcher` takes precedence over `object_hash` in reports.

Merge Patch treats object-member null as deletion. It cannot assign a new null
member, so `diff_merge_patch` rejects unrepresentable targets rather than changing
their meaning. Composition is independent of a baseline and likewise rejects
reset-then-object combinations that no single Merge Patch can represent for
every source. JSON null inside arrays and root null are supported normally.

### Wire format

For JavaScript interoperability, import the **forward** jsondiffpatch delta as
`Delta` and use Rust's `reverse` or `unpatch` for undo. The verification suite
checks this path in both languages. JavaScript's own inverse may retain rolling
hunk coordinates or contain an invalid text header; it is not interchangeable
with Rust's inverse in every case.

When an external inverse needs text relocation, explicitly use `patch_fuzzy`.
`TextPatchOptions { max_error_ratio: 0.0, ..Default::default() }` allows bounded
displacement while requiring exact text fragments. Default fuzzy options also
allow context edits, but approximate matching can fail or select a different
repeated fragment. Malformed headers remain errors in both APIs. See the
[compatibility results](https://github.com/majiayu000/palim/blob/main/BENCHMARK.md)
for the measured boundary.

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

Borrowed application APIs preserve their inputs. In-place APIs compute privately
and update the caller's document only after success. Consuming `_owned` APIs take
ownership of the document and do not return it on failure. Invalid delta structure,
bad indices, duplicate insertion destinations, incompatible types, missing/existing
properties and mismatched old replacement/deletion values return `Error`.
Default text application requires exact content at its expected position. Approximate application is explicit via the
fuzzy APIs; non-text old-value checks remain strict. Fuzzy displacement uses UTF-16
units, while its error ratio uses Unicode scalar edit counts. Complete exact
context takes priority within the displacement bound. With a nonzero error ratio,
hunks longer than 32 UTF-16 units without a complete exact match retain at most
four UTF-16 units of unchanged context per side; the error ratio applies to this
retained fragment. Zero-error matching retains complete context. Bounded anchors
and heuristic alignment can still reject a valid approximate match; this is not
a byte-for-byte clone of DMP's fuzzy matcher. Any failed hunk returns an error.
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
patches. Native diffs with unique array tokens use O(n log n) LIS and yield the
minimum number of single-item moves for pure permutations. Repeated tokens retain
heuristic Histogram matching above a bounded exact-LCS candidate limit. Within that limit the stable
subsequence is exact; this does not guarantee minimum edits for arbitrary custom
matchers or minimum serialized bytes. `Delta::to_json_patch` emits at most one
operation per native move; Fenwick ranks compute sequential move positions in
O(n + moves × log n) time and O(n + moves) space. Native deltas rebuild
arrays once, without repeated vector removals or a quadratic LCS matrix.

Documents and intermediate standard patch results are bounded at 128 containers;
deltas (including tuple wrappers) are bounded at **127** to round-trip through
serde_json's default reader. A near-limit source can therefore produce a depth
error if its delta exceeds the limit. Lower `max_depth` when needed;
values above 128 are rejected. Sequence lengths are checked against imara's i32 limit.

The opt-in `exact-numbers` Cargo feature enables serde_json's
`arbitrary_precision` and `float_roundtrip` features.
Native deltas retain numeric representation changes; standard `test` and reports
compare decimal numeric values (`1`, `1.0` and `1e0` are equal). Parse exact decimal
inputs from JSON text: converting a Rust f64 to JSON already chooses its shortest
decimal representation. Large numbers cannot be preserved by ordinary JS Number
consumers. For standard patches containing large integers, deserialize JSON text
with `serde_json::from_str::<Patch>`; the upstream tagged enum's `from_value` path
has a u128 buffering limitation. Generated operations use typed construction and
preserve those numbers. Strings must be valid Unicode.
In the default configuration, numeric precision and wire round trips have
serde_json's ordinary limits. RFC `test` and reports still compare mathematical
numeric values (`1` equals `1.0`). Cargo unifies dependency features: another
dependency can still enable exact numbers throughout the workspace.
Identity/filter callbacks are caller code; panic or allocation failure is not caught.

JS Date/undefined/functions, browser HTML/CSS/animations, a product CLI and language
bindings are outside this JSON library. See [DESIGN.md](https://github.com/majiayu000/palim/blob/main/DESIGN.md) for the design
and [BENCHMARK.md](https://github.com/majiayu000/palim/blob/main/BENCHMARK.md) for measured results and limitations.
