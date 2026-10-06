# RFC hashing and adaptive plain reorders — 2026-10-06–07

## Scope and decision

Adopt A: private RFC maps/sets use foldhash 0.1.5 through a direct dependency. The native interner already uses foldhash, but this change expands its use to RFC optimization. It is a fast noncryptographic hash, not SipHash-equivalent collision resistance. No public option was added.

Adopt B: direct **plain** RFC generation (factorize/rationalize/tests all false) can replace low-benefit primitive permutations by position. It reuses existing interning and the exact LIS, before constructing native delta tuples. Native delta generation and `Delta::to_json_patch` keep their existing move strategy. Optimized and guarded modes retain their existing output policy.

Admission requires identical unique token sets, equal lengths, normalized primitive target values, default move detection without custom item/filter callbacks, and depth low enough for the existing native error boundary. Containers, duplicate values, unsupported numbers and deep inputs retain the existing path. For downstream `serde_json/preserve_order`, the existing unsorted-object fallback can also retain native output.

Replacement bytes use exact primitive serialization and escaped parent paths. Move bytes are **estimated using the widest array index**. Accept replacement cost at most 110% of that estimate, and stop checking as soon as the budget is exceeded. This is a byte/CPU tradeoff heuristic; it does not guarantee actual output grows by at most 10%, nor global minimum bytes. Rotations with few moves and long-value shuffles reject it.

## Method

Baseline: `ca8235a2c00a9212334e8695f086d62d7cc131ee`. A-only and final source snapshots and hashes are in `measurements.json`; production files are frozen before timing. Same compiler and dependency features for all variants, including `serde_json/arbitrary_precision` and `float_roundtrip`.

19 fixtures: the original 16, the prior random shuffle-2000, and reproducible 20k/long-value shuffles. Four RFC modes, three independent processes each, nine timed batches per process, alternating before/after order. Every fixture has a Rust json-patch 4.2.0 plain control; three large optimized cases also have A-only controls: **522 timing processes**. Validation and compilation are outside timing. Core includes owned output destruction; pipeline parses both raw inputs, generates and serializes. A separate allocation binary reports allocator requests, not RSS.

All RFC outputs are independently applied by Rust json-patch and reversed through Palim inversion. Go/JS were not re-timed here; the [preceding cross-language report](../ci-crosslang-20261006/REPORT.md) remains a separate dated run.

## Results

Milliseconds, median of three process medians. All raw nine-batch arrays, process medians, bytes and hashes are retained.

| Input / mode | Core before → after ms | Pipeline before → after ms | Patch bytes before → after |
|---|---:|---:|---:|
| shuffle-2000 / plain | 1.4723 → 0.2715 | 1.7880 → 0.5933 | 81,181 → 87,736 |
| shuffle-20000 / plain | 16.7728 → 3.1130 | 20.5926 → 6.4768 | 872,590 → 917,781 |
| shuffle-long-2000 / plain | 1.6121 → 1.6539 | 6.8689 → 6.8579 | 80,866 → 80,866 |
| rotate-1000000 / plain | 128.9923 → 131.2561 | 214.7178 → 209.4459 | 44 → 44 |
| scalar-guards-100000 / optimized | 169.5839 → 153.1753 | 227.9937 → 210.8743 | 1,978,940 → 1,978,940 |
| file_migrations_move_copy_5000 / optimized | 34.5915 → 29.9583 | 42.9172 → 38.1514 | 505,072 → 505,072 |
| disjoint-20000 / optimized | 23.5826 → 21.4349 | 25.5663 → 23.4026 | 340,038 → 340,038 |
| small-config-edit / plain | 0.0025 → 0.0024 | 0.0098 → 0.0098 | 53 → 53 |
| add-wide-100000 / plain | 31.1603 → 33.2895 | 83.7481 → 87.7784 | 4,288,932 → 4,288,932 |

The actual adaptive shuffle-2000 Core improves **5.42×**, not the forced-positional prototype's ~10×. Its 1,918 moves become 1,999 replacements; output grows **8.07%**. Shuffle-20000 improves **5.39×**, output grows **5.18%**. Rust json-patch remains faster in generation: 0.2056 / 2.1254 ms versus 0.2715 / 3.1130 ms. Pipeline is also faster for json-patch on these shuffles. The interning/LIS/admission costs remain.

A-only optimized Core: scalar-100k 169.5839 → 154.2311 ms (**9.1% less**), migration-5k 34.5915 → 29.7988 (**13.9% less**), disjoint-20k 23.5826 → 21.6768 (**8.1% less**). A changes hashing work, not the allocation counts or output bytes.

Million rotate still emits exactly the same one-move **44 B** patch. Core is 128.9923 → 131.2561 ms (**1.8% slower** in this run), while pipeline is 214.7178 → 209.4459 ms (**2.5% faster**). Current Rust json-patch Core/pipeline are 114.8378 / 259.1759 ms with a 58,888,891 B patch. Neither generation speed nor overall leadership is guaranteed. Long-value shuffle preserves bytes but Core is **2.6% slower**. The unmodified add-wide control is **6.8% slower** in this run; its depth/number checks remain. All measurements, including regressions, are kept. Individual scalar-opt processes vary up to 200.44 ms, so these single-machine medians are evidence for these fixtures, not universal guarantees.

### Allocation requests during generation

| Input / mode | Calls before → after | Peak extra live requested bytes before → after |
|---|---:|---:|
| shuffle-2000 / plain | 27,786 → 6,022 | 796,702 → 305,158 |
| shuffle-20000 / plain | 284,472 → 60,031 | 9,016,150 → 2,981,843 |
| million rotate / plain | 96 → 96 | 183,137,336 → 151,137,336 |
| scalar-100k / optimized | 2,131,738 → 2,131,738 | 38,518,850 → 38,518,850 |
| migration-5k / optimized | 433,662 → 433,662 | 42,677,201 → 42,677,201 |

All ten allocation processes return to zero additional live bytes after output destruction. Moving mapping-vector allocation after admission reduces rotate's peak lifetime overlap; cumulative requested bytes are unchanged there. Instrumented allocation processes are not used for timing.

## Frozen records and functional verification

- A-only and final each retain **all 3,305 legacy records byte-for-byte**, including errors/cost snapshots. This corpus does **not** exercise B's changed low-benefit plain output; it alone is insufficient evidence for B.
- Supplemental direct corpus: eight inputs × all eight RFC flag combinations × before/after = **128 records (64 pairs)**. Only plain short reverse and nested escaped-key reverse change bytes. Rotations, long values, duplicates, nonpermutations and every other flag combination remain identical. The small seeded shuffle is rejected by the byte heuristic and keeps moves.
- Of 76 paired fixture/mode outputs in the timing matrix, only plain shuffle-2000 and shuffle-20000 change bytes. All three process outputs per combination have one identical hash. Both 2k and million rotations preserve exact output in every mode.
- Debug/release: **202 tests each**, including depth/error comparisons, unchecked-number behavior, independent forward/inverse and 128 generated permutations.
- Rust 1.85 library check, formatting and Clippy all targets with warnings denied passed locally.
- JS interop: **1,073 required paths**, no failures. Default fuzzy reverses 562 well-formed JS self-inverse cases and rejects four malformed headers; strict succeeds on 471.
- Structured fuzz: **135,242 runs / 61 s**, ASan, max_len 2048, seed 20261006, no failure. It now generates cheap and long unique permutations alongside existing arbitrary operations and array protocol cases.
- Five new Criterion plain-RFC array entries passed a smoke run. Smoke checks are not timing claims.

The standalone replay also rebuilt all three variants and verified the 3,305-record oracle hashes. `cargo publish --dry-run --locked --allow-dirty` packaged and verified 47 files (757.7 KiB / 189.2 KiB compressed), then aborted upload as intended. Check receipts are in `measurements.json`.

This document records pre-commit local acceptance. Remote completion is determined by the six jobs attached to the eventual commit/tag in [GitHub Actions](https://github.com/majiayu000/palim/actions/workflows/ci.yml). Tagging follows successful remote checks; actual crate/GitHub publication awaits the user's requested final approval. At this snapshot crates.io still serves 0.1.4.

## Replay

No new large evidence archive is added. Existing fixture archive checksum, small generated-fixture recipes, probe source/locks, frozen source overrides and raw measurements are in this directory.

```sh
python3 results/adaptive-rfc-20261006/run.py \
  --workdir /tmp/palim-rfc-replay --output /tmp/palim-rfc-replay.json
```

`--phase frozen` replays the 3,305-record oracles only; `--phase prepare` builds frozen variants without timing; `--phase measure` uses prepared variants. The full default replay takes several minutes. Cargo dependencies must be cached for `--offline`. Timing numbers can vary; wire hashes and roundtrip assertions are deterministic for the recorded fixtures.
