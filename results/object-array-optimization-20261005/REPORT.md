# Object RFC generation and array guard-cost optimization — 2026-10-05

## What changed

- Ordinary object/scalar RFC generation now emits owned operations directly. It merges sorted member iterators, reuses one JSON Pointer buffer, and copies only required target payloads. Native delta generation remains unchanged. Changed shared arrays and configured projection filters retain the existing path. The eligibility pass rejects changed arrays before allocating a disposable object prefix.
- Consecutive additions to the same array reuse the exact serialized parent-test cost and add only the new value length plus an optional comma. Any other operation invalidates the cache. Root `add` replaces the whole document and is explicitly excluded. Actual shadow replay still validates each operation; errors propagate.
- Existing operation ordering in the measured default Map build, arbitrary-precision addition normalization, delta wire depth limits, filters, array identities and inverse behavior are covered by regression/differential checks. No public API, dependency, version, or unsafe production code was added.

## Causes established from source and measurements

The former object path built reversible tuples containing old/new values and nested delta Maps, then walked that allocation graph again to export RFC operations. The new iterator walk removes that intermediate graph and repeated member lookups. The allocator probe below quantifies the resulting reduction.

The former guarded estimator serialized the whole array before every consecutive insertion. For equal-sized inserted values, serialized prefix lengths sum as 0 + 1 + ... + (n-1), yielding quadratic serialization work. A cached exact cost makes that component proportional to the inserted payload. Other matching, replay, and rationalization work still occurs.

## Before / after

Warm median of three process medians; nine batches per process. Core includes output destruction. Pipeline includes parsing both inputs, generation, compact UTF-8 serialization and destruction.

| Case / mode | Baseline core ms | Candidate core ms | Core speedup | Baseline pipeline ms | Candidate pipeline ms |
|---|---:|---:|---:|---:|---:|
| scalar-guards-2000 / plain | 1.201245 | 0.222512 | 5.40× | 2.063328 | 1.093646 |
| scalar-guards-10000 / plain | 6.361709 | 1.111195 | 5.73× | 10.934459 | 5.762479 |
| scalar-guards-100000 / plain | 78.054333 | 12.913084 | 6.04× | 134.281458 | 70.696417 |
| scalar-guards-100000 / plain-guarded | 157.607583 | 89.966208 | 1.75× | 219.928375 | 154.146875 |
| scalar-guards-100000 / optimized | 217.218375 | 150.378708 | 1.44× | 268.840958 | 205.477667 |
| file_migrations_move_copy_5000 / plain | 10.250500 | 3.221167 | 3.18× | 20.651125 | 13.330500 |
| file_migrations_move_copy_5000 / optimized | 39.816541 | 32.131916 | 1.24× | 47.469333 | 39.784208 |
| file_migrations_move_copy_5000 / guarded | 70.439500 | 62.640000 | 1.12× | 82.216209 | 74.644208 |
| disjoint-2000 / guarded | 20.180250 | 4.516437 | 4.47× | 20.421750 | 4.732052 |
| disjoint-10000 / guarded | 413.673458 | 23.019416 | 17.97× | 414.255292 | 24.131041 |
| disjoint-20000 / guarded | 1608.960167 | 46.339709 | 34.72× | 1619.690000 | 48.407041 |
| disjoint-20000 / optimized | 23.460459 | 23.204833 | 1.01× | 25.211583 | 24.968625 |
| rotate-2000 / plain | 0.223016 | 0.217922 | 1.02× | 0.373971 | 0.372990 |
| rotate-1000000 / plain | 162.389584 | 165.588084 | 0.98× | 240.435375 | 236.035000 |
| unchanged-100000 / plain-guarded | 0.592691 | 0.574882 | 1.03× | 10.736667 | 10.576916 |
| mixed-array-last-100000 / plain | 77.764125 | 80.019000 | 0.97× | 135.034834 | 137.827667 |
| small-config-edit / plain | 0.002167 | 0.002748 | 0.79× | 0.009430 | 0.010076 |

## Fresh Rust json-patch comparison

Same machine, serde_json precision features, input Values and exact serialized output on these two object workloads. json-patch 4.2.0 uses its direct diff walker. Both libraries own and drop their outputs within core timing.

| Plain object workload | Palim core ms | json-patch core ms | Palim pipeline ms | json-patch pipeline ms |
|---|---:|---:|---:|---:|
| scalar-guards-100000 | 12.913084 | 29.426083 | 70.696417 | 84.668750 |
| file_migrations_move_copy_5000 | 3.221167 | 2.411760 | 13.330500 | 12.677667 |

## Allocation evidence

Counters run in separate binaries, outside timing. Input Values are already allocated. Peak means extra requested live allocator bytes during generation; it is not RSS. All ten probes returned to their initial live allocation count after output destruction.

| Case / mode | Version | Allocation/reallocation calls | Requested bytes | Peak extra live bytes |
|---|---|---:|---:|---:|
| scalar-guards-100000 / plain | baseline | 1,116,707 | 50,169,976 | 29,997,354 |
| scalar-guards-100000 / plain | candidate | 300,022 | 20,245,955 | 10,767,550 |
| scalar-guards-100000 / plain-guarded | baseline | 2,831,678 | 109,836,210 | 62,748,839 |
| scalar-guards-100000 / plain-guarded | candidate | 2,014,993 | 79,912,189 | 43,519,024 |
| file_migrations_move_copy_5000 / plain | baseline | 241,750 | 29,102,515 | 26,082,897 |
| file_migrations_move_copy_5000 / plain | candidate | 85,058 | 11,050,028 | 8,953,511 |
| disjoint-20000 / guarded | baseline | 916,673 | 56,443,616 | 19,771,135 |
| disjoint-20000 / guarded | candidate | 916,673 | 56,443,616 | 19,771,135 |
| mixed-array-last-100000 / plain | baseline | 1,120,783 | 50,751,446 | 30,098,883 |
| mixed-array-last-100000 / plain | candidate | 1,120,783 | 50,751,446 | 30,098,883 |

## Verification completed

- `cargo fmt --check`.
- `cargo test --locked` and `cargo test --release --locked`: 183 / 183 tests passed.
- `cargo clippy --all-targets --locked -- -D warnings`.
- `cargo +1.85.0 check --lib --locked`.
- Release `fixture_runner` build and `node tools/interop.mjs`: 1073 cases, no failures.
- Nightly structured fuzz: 242974 runs, 61 seconds, no failure (60-second configured budget).
- 3,305 frozen differential records: identical public patches/inverses, private decisions, serialized costs and errors.
- 38 measured configurations, 114 sequential timing processes, ten allocator probes. All timed patch variants independently passed forward application and inverse restoration; every paired output hash matched.
- Evidence ZIP extracted into a separate directory: all hashes verified, both probes rebuilt, small fixture forward/inverse results and wire hashes replayed successfully.
- New permanent regressions exercise escaped/empty keys, unchecked valid number representations, native wire-depth errors, unchanged/changed shared arrays, prefix-resumed costs, empty/nonempty arrays, append/index-zero insertion, alternating containers, mixed writes, root replacement/type changes and invalid operation errors.

## Limits and remaining work

- Palim is faster on the measured large scalar object case; Rust json-patch remains faster on this member migration plain-patch workload. This is not an all-workloads lead.
- Mixed objects containing a changed array intentionally retain native matching. The eligibility scan adds some work; see the measured mixed-array case instead of assuming a gain.
- Small configurations can show overhead from eligibility/iterator setup at the microsecond scale.
- Array parent snapshots that are actually emitted by guarded inversion can still produce quadratic-size output. This change removes repeated cost estimation for consecutive inserts; it does not weaken those guards or compress their wire protocol.
- Interleaved changes can invalidate the single-parent cost cache. That is a further profiling target, as are factorization/rationalization after direct object generation.
- Array rotation uses the unchanged native matcher; timing differences there are not evidence of a new matching algorithm.
- Timing is warm, single-machine, and sequential. It does not establish tail latency, concurrency scaling, RSS, cold performance, or production service throughput. The baseline includes previous uncommitted guard improvements. These changes have not been committed or published.

## Evidence

Machine/source/fixture fingerprints, every batch and process median, wire hashes and allocation counters are in [measurements.json](measurements.json). Standalone source/probes/fixtures/oracles/logs and a rebuild script are in [evidence.zip](evidence.zip).
Archive: 19,253,784 bytes; 137 payload files; SHA-256 `83e0deb91a394a0ef5a6d26551cce8ce299fcb9ed5c63594fc8a826f4c43d5df`. All payload hashes and ZIP CRCs were checked.
