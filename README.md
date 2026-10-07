# Palim: reversible JSON changes for Rust

[![crates.io](https://img.shields.io/crates/v/palim.svg)](https://crates.io/crates/palim)
[![API documentation](https://docs.rs/palim/badge.svg)](https://docs.rs/palim)
[![CI](https://github.com/majiayu000/palim/actions/workflows/ci.yml/badge.svg)](https://github.com/majiayu000/palim/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/majiayu000/palim/blob/main/LICENSE)

Palim tracks **list moves and edits by stable IDs** and saves reversible JSON
changes for undo/redo or replay. It also generates RFC 6902 JSON Patch for other
clients, reads and writes jsondiffpatch deltas, and compares configurations.

## Installation

Requires Rust **1.85** or newer:

```sh
cargo add palim serde_json
```

In `Cargo.toml`, use `palim = "0.3"` and `serde_json = "1.0"`.
Numbers use serde_json's ordinary behavior by default. For arbitrary-precision
integers/decimals and precise float wire round trips, opt in:

```toml
palim = { version = "0.3", features = ["exact-numbers"] }
```

This enables serde_json's `arbitrary_precision` and `float_roundtrip` features
for the shared dependency, affecting `Value` equality and float parsing elsewhere
in your project. Enable it to retain Palim 0.2's numeric capabilities. Without it,
ordinary parsing can round or reject numbers before Palim sees them.

## Match a list by ID, then undo the change

```rust
use palim::{DiffPatcher, patch, unpatch};
use serde_json::json;
let engine = DiffPatcher::by_key("id");
let before = json!([{ "id": 1, "score": 10 }, { "id": 2, "score": 20 }]);
let after = json!([{ "id": 2, "score": 21 }, { "id": 1, "score": 10 }]);
let delta = engine.diff(&before, &after)?.ok_or("expected a change")?;
assert_eq!(patch(&before, &delta)?, after);
assert_eq!(unpatch(&after, &delta)?, before);
# Ok::<(), Box<dyn std::error::Error>>(())
```

`diff` returns `None` when nothing changes. Serialize a complete `Delta` with
serde_json to save it for later replay. Apply it to the correct baseline;
Palim does not resolve concurrent edits. Missing IDs fall back to the complete
item value, and repeated IDs are paired deterministically.

## Choose an API

| I want to… | Use | What it produces |
|---|---|---|
| Save history or implement undo/redo | `diff` or `DiffPatcher::by_key("id").diff`, then `patch` / `unpatch` | Native `Delta`, including old values |
| Send a standard patch to a frontend or third party | `diff_json_patch` or `Delta::to_json_patch`, then `apply_json_patch` | RFC 6902 `Patch` operations |
| Undo a standard patch | `invert_json_patch`, with the original baseline | An inverse RFC 6902 `Patch` |
| See which paths changed | `compare` | `CompareReport`; a report, not a patch |
| Use JSON Merge Patch | `diff_merge_patch` / `merge_patch` | RFC 7396 JSON; object-member null means deletion |
| Read a JS jsondiffpatch change | Deserialize a forward delta as `Delta`, then `patch` / `unpatch` | Native interoperable delta |

**`Delta` and `Patch` are different formats.** Native deltas retain values for
undo; standard patches interoperate with RFC 6902 clients and need the baseline
for inversion. For a full API inventory and details on filters, fuzzy text,
limits and atomic application, read the
[API guide](https://github.com/majiayu000/palim/blob/main/API_GUIDE.md) or
[function documentation](https://docs.rs/palim).

## When to choose Palim

| Your need | A useful starting point |
|---|---|
| Undo/redo, replay, or jsondiffpatch-compatible deltas | Palim's reversible `Delta` |
| Large arrays with stable IDs, reorders and edits inside moved items | Palim's identity matching and move output |
| Standard patches with move/copy optimization, guards or inversion | Palim's RFC APIs |
| Ordinary positional RFC patches without those capabilities | `json-patch` offers a simpler API |
| Small configurations in an existing JavaScript application | Compare the JS libraries in your own workload |

Performance depends on the workload and output format. Dated measurements show
Palim's advantages on large reorders and reversible deltas, with slower cases on
random shuffles and wide additions. These historical runs do not measure the
ordinary-number default introduced in 0.3. See the
[benchmarks and measurement boundaries](https://github.com/majiayu000/palim/blob/main/BENCHMARK.md)
and [library comparison](https://github.com/majiayu000/palim/blob/main/COMPARISON.md).

## Runnable examples

| Example | Demonstrates |
|---|---|
| [id_list_history](https://github.com/majiayu000/palim/blob/main/examples/id_list_history.rs) | ID-based list moves, edits, saved deltas, undo and redo |
| [rfc_patch_transport](https://github.com/majiayu000/palim/blob/main/examples/rfc_patch_transport.rs) | Serialize RFC 6902 operations, apply them and generate an inverse |
| [config_report](https://github.com/majiayu000/palim/blob/main/examples/config_report.rs) | Compare configuration paths and print a report |
| [jsondiffpatch_interop](https://github.com/majiayu000/palim/blob/main/examples/jsondiffpatch_interop.rs) | Import a forward JS delta and use Rust to apply and undo it |

```sh
cargo run --example id_list_history --locked
cargo run --example rfc_patch_transport --locked
cargo run --example config_report --locked
cargo run --example jsondiffpatch_interop --locked
```

## More information

- [API guide](https://github.com/majiayu000/palim/blob/main/API_GUIDE.md): matching, patch formats, numeric semantics and error contracts.
- [Benchmarks and verification](https://github.com/majiayu000/palim/blob/main/BENCHMARK.md): dated results, interoperability boundaries and commands for contributors.
- [Design](https://github.com/majiayu000/palim/blob/main/DESIGN.md), [changelog](https://github.com/majiayu000/palim/blob/main/CHANGELOG.md) and [releases](https://github.com/majiayu000/palim/releases).

MIT for this implementation. Dependencies retain their respective licenses.
Upstream-derived test fixtures are Apache-2.0; see
[tests/fixtures/NOTICE.md](https://github.com/majiayu000/palim/blob/main/tests/fixtures/NOTICE.md).
