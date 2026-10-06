# Changelog

## 0.2.0 — Unreleased

### Changed

- Guarded RFC 6902 generation reuses authenticated subtrees instead of repeatedly copying parent containers into `test` operations. Array index shifts invalidate affected authentication records. Guarded output bytes and operation counts can change; patches remain standard RFC 6902.
- Disjoint primitive arrays in plain guarded mode can use positional replacements with one baseline array test. This removes the previous quadratic output growth for these inputs.

### Performance

- Avoid reparsing already normalized arbitrary-precision numbers when copying additions. Noncanonical and malformed unchecked numbers retain the existing parser behavior.
- Bound input-depth traversal by nesting depth rather than container width.
- Resolve RFC optimization lookups with parsed JSON Pointer tokens and skip unchanged array-item export work.

### Fixed

- Increase the bounded-depth regression test's thread stack to 1 MiB so Windows debug builds can run its unchanged depth assertions.

### Verification

- Three operating systems, Rust 1.85 library checks, JS interoperability and structured fuzzing passed remotely on `b733c19`.
- Current-source cross-language measurements and pending optimization prototypes are recorded in [the comparison report](results/ci-crosslang-20261006/REPORT.md). Prototype output and dependency changes are not part of this release preparation.
