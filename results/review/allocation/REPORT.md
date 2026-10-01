# Low-allocation exploration — 2026-10-01

## Scope and method

Only a temporary developer tool and source snapshot were written under `/tmp/jsondiffpatch-allocation-20261001`. Production source/API/docs were not modified by this exploration. The AFTER binary depends on the consistent `frozen-crate` snapshot. Two warmups and five measured samples per phase, 113 phases, all output checks completed; requested allocation counts were identical across samples. A replay of the retained BEFORE binary reproduced every allocation count with the same fixture hashes. These checks validate the measurement outputs, not the production crate’s full test gates.

The allocator forwards to `System` with explicit SAFETY comments; counting uses atomics and allocates nothing itself. `allocation_bytes` sums requested successful alloc/alloc_zeroed sizes. `realloc_requested_bytes` sums full requested NEW sizes, not growth increments. Their sum, called gross requested bytes here, is neither resident memory nor live/unique allocated memory. Deallocation is not charged. Instrumented CPU timings are concurrent exploratory observations and must not be used as final benchmark results.

Owned-transfer prepares `left.clone()` before the counted scope to simulate a caller already owning an expendable baseline. Owned-copy-inside charges that clone and performs exactly the same work as borrowed application. Serialization and validation checks outside the counted scope do not contribute to the reported phase costs.

## Before/after standard-pipeline allocation requests

Exact byte counts; all five cases included (the first four are the mandatory representative set). No percentage implies a CPU speedup.

| Case | RFC no optimization, gross B before → after | RFC default, gross B before → after | parse → default RFC → serialize, gross B before → after |
|---|---:|---:|---:|
| 2,000 ID objects, rotate + edit | 6,484,200 → 828,908 | 22,740,151 → 4,653,270 | 28,336,782 → 10,249,901 |
| 1 MiB stable subtree, bool edit | 2,102,100 → 2,028 | 14,694,229 → 2,728 | 16,794,117 → 2,102,616 |
| ASCII long text | 2,511,010 → 176,720 | 3,910,453 → 701,998 | 5,898,662 → 2,690,207 |
| Unicode long text | 2,819,354 → 171,720 | 4,193,797 → 681,998 | 6,139,240 → 2,627,441 |
| 1 MiB root → null | 3,148,109 → 1,049,647 | 5,246,296 → 1,049,915 | 6,295,639 → 2,099,258 |

Allocation-call counts for default RFC changed respectively 222,373 → 84,173; 144 → 40; 68 → 37; 68 → 37; 22 → 9. Native diff, native application and native parse→diff→serialize allocation counts were unchanged in every case. The paired observations establish the total gain of the concurrently implemented RFC changes; they do not isolate each individual change.

AFTER source reuses the known target in `DiffPatcher::diff_json_patch` (`frozen-crate/src/lib.rs:226`, `src/export.rs:15`), creates copy-search shadow/index only when needed (`src/rfc.rs:270`), and uses a counting `Write` sink for byte-size comparisons (`src/rfc.rs:152`) rather than serializing into disposable buffers. Public `Delta::to_json_patch(left)` still reconstructs and validates its target: the 1 MiB subtree case retains 1,050,342 gross B there. A filtered diff still needs its projected target, preserving filtered semantics and baseline validation contracts.

The AFTER 2,000-ID default path still requests 4,653,270 B versus 828,908 B without optimizers. Copy factorization accounts for most of that remaining difference in this input. It is not a zero-allocation pipeline. The 1 MiB parse-inclusive path remains 2,102,616 B because both input Values are parsed and own their strings.

## Native ownership and delta value clones

Allocation bytes alone below (realloc requests are separate in raw data). The five cases have 74 / 36 / 56 / 56 / 1,048,597 B native wire deltas, respectively.

| Case | Native diff alloc B | Delta Value clone alloc B | Borrowed patch alloc B | Owned-transfer patch alloc B | Owned-copy-inside patch alloc B |
|---|---:|---:|---:|---:|---:|
| 2,000 ID objects, rotate + edit | 388,904 | 2,723 | 2,732,544 | 65,237 | 2,732,544 |
| 1 MiB stable subtree, bool edit | 1,498 | 1,342 | 1,049,956 | 92 | 1,049,956 |
| ASCII long text | 951 | 142 | 467,073 | 292,073 | 467,073 |
| Unicode long text | 951 | 142 | 403,739 | 233,739 | 403,739 |
| 1 MiB root → null | 1,049,327 | 1,049,279 | 1,049,231 | 16 | 1,049,231 |

Owned-transfer is materially useful for an already-owned baseline: 2,000-ID borrowed native patch has 20,045 allocation calls versus 42 for owned transfer, and 1 MiB subtree patch 21 versus 13. Charging a clone inside the owned call gives exactly the borrowed result. Consuming application saves the baseline copy; it does not make borrowed callers or newly parsed DOMs free. Native long-text transfer still allocates reconstructed output and text processing buffers.

The 1 MiB stable subtree is already avoided in native diff: only 1,498 allocated B. By contrast root replacement stores the old whole value for reversibility, so native diff and cloning that delta each request about 1.05 MB. This is the strongest measured use case for eventually borrowing changed payloads, not a reason to rewrite all diff paths. serde_json Value owns String, Vec and Map payloads; wrapping a borrowed Value in a new owned Value cannot supply structural sharing. [Value source](https://github.com/serde-rs/json/blob/v1.0.151/src/value/mod.rs).

## Serialization and bounded borrowing prototypes

`serde_json::to_writer` into a caller-supplied preallocated Vec requested zero allocations/reallocations for all measured native deltas. `to_vec` requested 128 B for small deltas; the 1 MiB delta requested 128 alloc B plus 3,145,767 realloc B as its output grew. With exact capacity allocated inside scope, the latter costs one 1,048,597 B allocation; with a buffer already owned by the caller the serializer phase costs zero. Existing Serialize support already enables this without another production writer API. [to_writer](https://docs.rs/serde_json/1.0.151/serde_json/fn.to_writer.html).

For ONLY the whole-root replacement, serializing `[&left, &right]` produced the exact native wire bytes and avoided constructing an owned delta payload. Parsing compact inputs as `&RawValue` and serializing the same two-element leaf also produced those bytes; neither prototype implements general structural diff, array matching, text deltas, reverse, filters, limits or error validation. Their output Vec still allocated roughly 3.15 MB of requested capacity growth. Borrowing is tied to live input Values/input bytes; converting a borrowed representation to owned restores the payload clone. [Serde lifetime contract](https://serde.rs/lifetimes.html).

RawValue parsing of the 2,000-object and 1 MiB-subtree roots requested just one 8 B allocation, versus 2,798,251 and 1,049,880 gross B for full Value parsing. This validates opaque JSON borrowing only: RawValue still validates/scans bytes and preserves original formatting. Raw bytes cannot correctly substitute for structural equality across whitespace, reordered object keys or equivalent numeric representations. A general structural diff still needs a DOM or a new token/offset index. [RawValue source](https://github.com/serde-rs/json/blob/v1.0.151/src/raw.rs).

## Isolated canonical-number cache experiment

2,000 parsed numbers × four canonical-key hashing passes. Temporary code retains the original canonical algorithm in `src/canonical_baseline.rs`; it is not a benchmark of the evolving precision/tolerance pipeline. Cache is an operation-local HashMap keyed by the exact borrowed source spelling; creation and values are counted.

| Input | Uncached gross B | Local cache gross B | Interpretation |
|---|---:|---:|---|
| 100 decimal spellings repeated | 376,000 | 25,160 | Fewer repeated mantissa allocations |
| 2,000 unique decimals | 376,000 | 757,316 | Cache capacity/storage increases requests about 2× |
| 100 ordinary integers repeated | 0 | 20,460 | Cache makes a cheap path worse |

For decimals the original key builds owned concatenated mantissa digits, which produces an allocation and a realloc in these inputs. Integer digit slices borrow from Number; small/zero BigInt exponents did not allocate in this experiment. An owned ScalarKey additionally owns digit strings; large scalar strings copied into copy-search indexes can dominate numbers. Do not blanket-cache canonical keys or claim BigInt always heap-allocates. Production numbers.rs changed concurrently, so these rows isolate an algorithm/cache tradeoff only.

## Adopt/adapt/build/reject for current requirements

| Option and primary precedent | Decision | Evidence / scope |
|---|---|---|
| Reuse known target, lazily create optimizer shadow/index, count wire bytes with a sink | BUILD, selected and already present in measured AFTER snapshot | Largest mandatory-case gains; private implementation changes preserve the public owned Value / Delta model. Parent owns implementation and production validation. |
| Caller consumes existing Value for application | ADOPT existing API | Owned/borrowed comparison above proves the transfer benefit and clone-inclusive boundary. |
| Reuse serialization buffer with serde_json::to_writer | ADOPT existing capability | Zero allocations in serialization phase with preallocated caller buffer; no new API necessary. |
| Borrowed delta payloads, fionn-diff PatchOperationRef<Cow<Value>> | ADAPT later only if large replacement payloads dominate real users | fionn-diff 0.2.0 borrows Add/Replace/Test payloads, owns computed paths, and into_owned clones borrowed payloads. Our singleton leaf proves about 1 MiB delta-payload copying can be avoided, not a general native implementation. Input lifetimes, text-generated values and container metadata still cost memory. [Source](https://docs.rs/crate/fionn-diff/0.2.0/source/src/diff_zerocopy.rs). |
| RawValue-backed general structural diff | REJECT for this round | Excellent opaque forwarding; implementing structure-aware numeric/key/array/text semantics would create another parser/index/backend. Synthetic leaf prototype is insufficient for current full feature contract. |
| Directly stream a new patch while diffing | REJECT new public API this round | Existing Serialize gives buffer reuse; skipping Delta could save replacement cloning but reversibility/native interop, filters and standard optimizers need a retained plan. No complete general prototype or end-to-end gain yet. |
| Generic typed diff with derives, dipa | REJECT for current JSON boundary | dipa derives typed Rust Diffable/Patchable deltas and targets efficient binary state encoding. It is a different data contract; Serialize alone provides no standard field reflection for arbitrary structs. [Official repo](https://github.com/chinedufn/dipa). |
| Universal/global NumbersCanonical cache | REJECT | Decimal-unique and integer allocation regressions above; no measured whole-pipeline evidence justifies added lifetime/storage/concurrency policy. |

## Reproduction and evidence limits

- BEFORE executable: `baseline-binary`; `allocation-results-baseline-replay.json` reproduces `allocation-results.json` allocation counters exactly. Its SHA and retained harness SHA are in `manifest.json`. Run it while the three production fixture hashes match `inputs_sha256`.
- AFTER executable: `target/release/jsondiffpatch-allocation-exploration`; `cargo run --release` depends only on `frozen-crate` for the library and fixture paths. Raw output: `allocation-results-after.json`.
- `comparison.json` contains all 113 median before/after phase rows. `manifest.json` hashes raw artifacts, fixture inputs and binaries. `snapshot.json` hashes every copied source/input file; originals were identical before and after copying.
- Exact source hashes for the first build were not captured before compilation. `environment.json` observes post-run source, seven files already had newer mtimes. The retained BEFORE binary is reproducible, but that observation must not be called its exact compiled-source fingerprint.
- The AFTER snapshot is exact for this exploration and may differ from the final crate, especially numbers.rs. Numbers source identity is not evidence that later numeric changes or production checks passed.
- All requests reported here concern allocation. Neither CPU speedup, peak RSS nor real downstream adoption was measured. No full production test/benchmark gate was run by this agent.
