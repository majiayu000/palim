# Changelog

## 0.3.0

- **Breaking:** stop enabling serde_json's `arbitrary_precision` and `float_roundtrip`
  features by default. Adding `palim = "0.3"` preserves ordinary serde_json numeric
  representation and parsing rather than changing downstream Value equality and
  float parsing throughout the dependency graph.
- Opt in with `palim = { version = "0.3", features = ["exact-numbers"] }` to retain
  the arbitrary-precision numbers and float wire round trips provided by 0.2.
  This explicitly enables those serde_json features for the shared dependency.
- RFC `test` and comparison reports still use mathematical numeric equality in
  both configurations, including `1` and `1.0`. Ordinary parsing can round or
  reject numbers beyond serde_json's normal range before Palim sees them.
- Test both feature configurations in CI and at the minimum Rust version.

## 0.2.1

### Documentation

- Publish the refreshed README with the `0.2` installation example, guarded subtree reuse, adaptive plain reorders and the scope of native move guarantees.
- Include the measured performance tradeoffs, verification results and links to the complete reports.
- Update the current release link to 0.2.1.

## 0.2.0

### Changed

- Guarded RFC 6902 generation reuses authenticated subtrees instead of repeatedly copying parent containers into `test` operations. Array index shifts invalidate affected authentication records. Guarded output bytes and operation counts can change; patches remain standard RFC 6902.
- Disjoint primitive arrays in plain guarded mode can use positional replacements with one baseline array test. This removes the previous quadratic output growth for these inputs.
- Direct plain RFC generation can use positional replacements for low-benefit unique primitive permutations, based on estimated move bytes. Plain output shape/bytes can change. Native delta and its exporter, optimized/guarded modes, custom filters/matchers and deep/error paths keep their existing strategy.

### Performance

- Use foldhash 0.1.5 directly for private RFC maps/sets. It is a noncryptographic hash; its collision-resistance contract differs from SipHash.
- Reuse interning/LIS and skip native tuple construction when positional replacements are accepted. Large-value moves and tested rotations retain byte-identical output.
- Avoid reparsing already normalized arbitrary-precision numbers when copying additions. Noncanonical and malformed unchecked numbers retain the existing parser behavior.
- Bound input-depth traversal by nesting depth rather than container width.
- Resolve RFC optimization lookups with parsed JSON Pointer tokens and skip unchanged array-item export work.

### Fixed

- Increase the bounded-depth regression test's thread stack to 1 MiB so Windows debug builds can run its unchanged depth assertions.

### Verification

- Three operating systems, Rust 1.85 library checks, JS interoperability and structured fuzzing passed remotely on `b733c19`.
- Current-source cross-language measurements are recorded in [the cross-language report](results/ci-crosslang-20261006/REPORT.md).
- Final hashing/adaptive A/B: 522 timing processes, 3,305 unchanged legacy records plus 128 direct records, 202 debug/release tests each, 1,073 required JS interoperability paths and 135,242 local fuzz runs. Actual tradeoffs and regressions are preserved in [the final report](results/adaptive-rfc-20261006/REPORT.md).
