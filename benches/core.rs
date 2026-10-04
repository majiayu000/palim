use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use palim::{
    CompareOptions, DiffOptions, DiffPatcher, JsonPatchOptions, PatchOperation, apply_json_patch,
    compare, diff_merge_patch, invert_json_patch, invert_json_patch_guarded, merge_patch, patch,
    patch_owned, reverse, test_json_patch,
};
use serde_json::{Value, json};
use std::{hint::black_box, sync::Arc, time::Duration};

fn fixtures() -> Vec<(String, Value, Value)> {
    let mut cases = vec![(
        "small".into(),
        json!({"settings":{"a":1,"b":false}}),
        json!({"settings":{"a":2,"b":false}}),
    )];
    for n in [2000, 20000] {
        let a = json!((0..n).collect::<Vec<_>>());
        let mut reverse = a.as_array().unwrap().clone();
        reverse.reverse();
        cases.push((format!("identical-{n}"), a.clone(), a.clone()));
        cases.push((
            format!("prepend-{n}"),
            a.clone(),
            json!(
                [-1_i64]
                    .into_iter()
                    .chain((0..n).map(|i| i as i64))
                    .collect::<Vec<_>>()
            ),
        ));
        cases.push((
            format!("rotate-{n}"),
            a.clone(),
            json!([n - 1].into_iter().chain(0..n - 1).collect::<Vec<_>>()),
        ));
        cases.push((format!("reverse-{n}"), a.clone(), json!(reverse)));
        cases.push((
            format!("disjoint-{n}"),
            a,
            json!((n..2 * n).collect::<Vec<_>>()),
        ));
        let a = json!(
            (0..n)
                .map(|id| json!({"id":id,"name":format!("entry-{id}"),"score":id}))
                .collect::<Vec<_>>()
        );
        let mut b = a.as_array().unwrap().clone();
        b.rotate_right(1);
        b[n / 2]["score"] = json!(-1);
        cases.push((format!("keyed-{n}"), a, json!(b)));
    }
    let text = "Unicode 🦀 中文 text\n".repeat(5000);
    cases.push((
        "text".into(),
        json!(text),
        json!(text.replacen("text", "edited", 1)),
    ));
    cases.push((
        "halfswap-500".into(),
        json!((0..500).collect::<Vec<_>>()),
        json!((250..500).chain(0..250).collect::<Vec<_>>()),
    ));
    cases
}
fn benchmarks(c: &mut Criterion) {
    let dp = DiffPatcher::new(DiffOptions {
        object_hash: Some(Arc::new(|v, _| v.get("id").map(Value::to_string))),
        ..Default::default()
    });
    let mut group = c.benchmark_group("core");
    for (name, a, b) in fixtures() {
        let delta = dp.diff(&a, &b).unwrap();
        if let Some(d) = &delta {
            assert_eq!(patch(&a, d).unwrap(), b);
            assert_eq!(patch(&b, &reverse(d).unwrap()).unwrap(), a);
        }
        group.bench_with_input(
            BenchmarkId::new("diff", &name),
            &(&a, &b),
            |bencher, (a, b)| {
                bencher.iter(|| dp.diff(black_box(a), black_box(b)).unwrap());
            },
        );
        if let Some(d) = &delta {
            group.bench_with_input(
                BenchmarkId::new("patch", &name),
                &(&a, d),
                |bencher, (a, d)| bencher.iter(|| patch(black_box(a), black_box(d)).unwrap()),
            );
            group.bench_with_input(BenchmarkId::new("reverse", &name), d, |bencher, d| {
                bencher.iter(|| reverse(black_box(d)).unwrap())
            });
        }
    }
    group.finish();
}

fn extended_apis(c: &mut Criterion) {
    let dp = DiffPatcher::new(DiffOptions {
        object_hash: Some(Arc::new(|v, _| v.get("id").map(Value::to_string))),
        ..Default::default()
    });
    let plain = JsonPatchOptions {
        factorize: false,
        rationalize: false,
        tests: false,
    };
    let optimized = JsonPatchOptions::default();
    let mut standard = c.benchmark_group("standard");
    for (name, a, b) in fixtures().into_iter().filter(|(name, _, _)| {
        [
            "small",
            "rotate-2000",
            "keyed-2000",
            "disjoint-2000",
            "halfswap-500",
        ]
        .contains(&name.as_str())
    }) {
        let delta = dp.diff(&a, &b).unwrap().unwrap();
        let exported = delta.to_json_patch(&a).unwrap();
        let mut applied = a.clone();
        json_patch::patch(&mut applied, &exported).unwrap();
        assert_eq!(applied, b);
        if name == "halfswap-500" {
            assert_eq!(
                exported
                    .0
                    .iter()
                    .filter(|op| matches!(op, PatchOperation::Move(_)))
                    .count(),
                250
            );
        }
        standard.bench_function(BenchmarkId::new("export-precomputed", &name), |bencher| {
            bencher.iter(|| delta.to_json_patch(black_box(&a)).unwrap());
        });
        for (mode, options) in [("plain", &plain), ("optimized", &optimized)] {
            let change = dp.diff_json_patch(&a, &b, options).unwrap();
            let mut applied = a.clone();
            json_patch::patch(&mut applied, &change).unwrap();
            assert_eq!(applied, b);
            standard.bench_function(BenchmarkId::new(mode, &name), |bencher| {
                bencher.iter(|| {
                    dp.diff_json_patch(black_box(&a), black_box(&b), options)
                        .unwrap()
                });
            });
        }
    }
    standard.finish();

    let mut comparison = c.benchmark_group("compare");
    let a = json!({"set": (0..1000).chain(0..1000).collect::<Vec<_>>()});
    let b = json!({"set": (0..1000).rev().chain((0..1000).rev()).collect::<Vec<_>>()});
    let unordered = CompareOptions {
        unordered: Some(Arc::new(|path| path == "/set")),
        ..Default::default()
    };
    let report = compare(&a, &b, &unordered).unwrap();
    assert!(report.differences.is_empty() && report.moves.is_empty());
    assert_eq!(report.similarity, 1.0);
    comparison.bench_function("unordered-multiset-2000", |bencher| {
        bencher.iter(|| compare(black_box(&a), black_box(&b), &unordered).unwrap());
    });
    let a = json!((0..2000).map(|i| i as f64 / 10.0).collect::<Vec<_>>());
    let b = json!(
        (0..2000)
            .map(|i| i as f64 / 10.0 + 0.001)
            .collect::<Vec<_>>()
    );
    let tolerance = CompareOptions {
        absolute_tolerance: 0.01,
        ..Default::default()
    };
    let report = compare(&a, &b, &tolerance).unwrap();
    assert!(report.differences.is_empty() && report.moves.is_empty());
    assert_eq!(report.similarity, 1.0);
    comparison.bench_function("absolute-tolerance-2000", |bencher| {
        bencher.iter(|| compare(black_box(&a), black_box(&b), &tolerance).unwrap());
    });
    comparison.finish();

    let a = json!({"settings": (0..1000)
        .map(|i| (format!("key-{i}"), json!({"enabled": true, "score": i})))
        .collect::<serde_json::Map<_, _>>()});
    let mut b = a.clone();
    for i in 0..20 {
        b["settings"][format!("key-{i}")]["score"] = json!(-1);
    }
    b["settings"].as_object_mut().unwrap().remove("key-999");
    b["settings"]["added"] = json!({"enabled": false});
    let change = diff_merge_patch(&a, &b).unwrap();
    assert_eq!(merge_patch(&a, &change).unwrap(), b);
    let mut merge = c.benchmark_group("merge");
    merge.bench_function("diff-settings-1000", |bencher| {
        bencher.iter(|| diff_merge_patch(black_box(&a), black_box(&b)).unwrap());
    });
    merge.bench_function("apply-settings-1000", |bencher| {
        bencher.iter(|| merge_patch(black_box(&a), black_box(&change)).unwrap());
    });
    merge.finish();

    let mut owned = c.benchmark_group("owned");
    for (name, a, b) in fixtures()
        .into_iter()
        .filter(|(name, _, _)| ["keyed-2000", "halfswap-500"].contains(&name.as_str()))
    {
        let delta = dp.diff(&a, &b).unwrap().unwrap();
        assert_eq!(patch_owned(a.clone(), &delta).unwrap(), b);
        // Input ownership is prepared outside the timed application. This is
        // the consuming API's cost, not clone-plus-patch end-to-end latency.
        owned.bench_function(BenchmarkId::new("patch-preowned", &name), |bencher| {
            bencher.iter_batched(
                || a.clone(),
                |baseline| patch_owned(black_box(baseline), black_box(&delta)).unwrap(),
                BatchSize::LargeInput,
            );
        });
    }
    owned.finish();
}

fn risk_cases(c: &mut Criterion) {
    let dp = DiffPatcher::new(DiffOptions {
        object_hash: Some(Arc::new(|v, _| v.get("id").map(Value::to_string))),
        ..Default::default()
    });
    let base: String = (0..5000)
        .map(|i| format!("line {i:04} Unicode 🦀 中文 stable context\n"))
        .collect();
    let texts = [
        ("unicode-first", base.replacen("line 0000", "首行修改🚀", 1)),
        (
            "unicode-middle",
            base.replacen("line 2500", "中间修改🚀", 1),
        ),
        ("unicode-last", base.replacen("line 4999", "末行修改🚀", 1)),
        (
            "unicode-three-edits",
            base.replacen("line 0000", "首行修改🚀", 1)
                .replacen("line 2500", "中间修改🚀", 1)
                .replacen("line 4999", "末行修改🚀", 1),
        ),
        ("unicode-disjoint", "αβγδεζηθ🚀".repeat(12_500)),
    ];
    let mut text = c.benchmark_group("risk/text");
    for (name, target) in texts {
        let (a, b) = (json!(base), json!(target));
        let delta = dp.diff(&a, &b).unwrap().unwrap();
        assert_eq!(patch(&a, &delta).unwrap(), b);
        assert_eq!(patch(&b, &reverse(&delta).unwrap()).unwrap(), a);
        text.bench_function(BenchmarkId::new("diff", name), |bencher| {
            bencher.iter(|| dp.diff(black_box(&a), black_box(&b)).unwrap());
        });
        text.bench_function(BenchmarkId::new("patch", name), |bencher| {
            bencher.iter(|| patch(black_box(&a), black_box(&delta)).unwrap());
        });
    }
    text.finish();

    let mut arrays = c.benchmark_group("risk/array");
    let mut cases = Vec::new();
    for (name, period) in [("duplicates-high-2000", 8), ("duplicates-low-2000", 1000)] {
        let source: Vec<_> = (0..2000).map(|i| i % period).collect();
        let mut target = source.clone();
        target.rotate_left(37);
        for i in (0..2000).step_by(100) {
            target[i] = 3000 + i;
        }
        cases.push((name, json!(source), json!(target)));
    }
    let source: Vec<_> = (0..2000)
        .map(|i| json!({"id": i % 32, "value": i}))
        .collect();
    let mut target = source.clone();
    target.rotate_left(997);
    target[1000]["value"] = json!(-1);
    cases.push(("ambiguous-ids-2000", json!(source), json!(target)));
    cases.push((
        "shuffle-2000",
        json!((0..2000).collect::<Vec<_>>()),
        json!((0..2000).map(|i| i * 997 % 2000).collect::<Vec<_>>()),
    ));
    cases.push((
        "reverse-2000",
        json!((0..2000).collect::<Vec<_>>()),
        json!((0..2000).rev().collect::<Vec<_>>()),
    ));
    for (name, a, b) in cases {
        let delta = dp.diff(&a, &b).unwrap().unwrap();
        assert_eq!(patch(&a, &delta).unwrap(), b);
        assert_eq!(patch(&b, &reverse(&delta).unwrap()).unwrap(), a);
        let standard = delta.to_json_patch(&a).unwrap();
        let mut independent = a.clone();
        json_patch::patch(&mut independent, &standard).unwrap();
        assert_eq!(independent, b);
        arrays.bench_function(BenchmarkId::new("diff", name), |bencher| {
            bencher.iter(|| dp.diff(black_box(&a), black_box(&b)).unwrap());
        });
        arrays.bench_function(BenchmarkId::new("export-precomputed", name), |bencher| {
            bencher.iter(|| delta.to_json_patch(black_box(&a)).unwrap());
        });
    }
    arrays.finish();

    let a = json!({"ignored": (0..20000).map(|i| json!({"id": i, "v": i})).collect::<Vec<_>>(),
        "kept": {"count": 1}});
    let b = json!({"ignored": (0..20000).map(|i| json!({"id": i, "v": -1})).collect::<Vec<_>>(),
        "kept": {"count": 2}});
    let filtered = DiffPatcher::new(DiffOptions {
        node_filter: Some(Arc::new(|path, _, _| path != "/ignored")),
        ..Default::default()
    });
    let delta = filtered.diff(&a, &b).unwrap().unwrap();
    let mut projected = a.clone();
    projected["kept"]["count"] = json!(2);
    assert_eq!(patch(&a, &delta).unwrap(), projected);
    let options = CompareOptions {
        node_filter: Some(Arc::new(|path, _, _| path != "/ignored")),
        ..Default::default()
    };
    let report = compare(&a, &b, &options).unwrap();
    assert_eq!(report.differences.len(), 1);
    assert_eq!(report.differences[0].path, "/kept/count");
    let mut filter = c.benchmark_group("risk/filter");
    filter.bench_function("diff-whole-subtree-20000", |bencher| {
        bencher.iter(|| filtered.diff(black_box(&a), black_box(&b)).unwrap());
    });
    filter.bench_function("compare-whole-subtree-20000", |bencher| {
        bencher.iter(|| compare(black_box(&a), black_box(&b), &options).unwrap());
    });
    filter.finish();

    let mut decimal = c.benchmark_group("risk/decimal");
    for (name, left, right, absolute, relative) in [
        (
            "beyond-f64-2000",
            "9007199254740993.0001",
            "9007199254740993.0002",
            0.01,
            0.0,
        ),
        (
            "huge-positive-exponent",
            "1e1000000000",
            "1.005e1000000000",
            0.0,
            0.01,
        ),
        (
            "huge-negative-exponent",
            "1e-1000000000",
            "2e-1000000000",
            0.01,
            0.0,
        ),
    ] {
        let left: Value = serde_json::from_str(left).unwrap();
        let right: Value = serde_json::from_str(right).unwrap();
        let (a, b) = if name == "beyond-f64-2000" {
            (json!(vec![left; 2000]), json!(vec![right; 2000]))
        } else {
            (left, right)
        };
        let options = CompareOptions {
            absolute_tolerance: absolute,
            relative_tolerance: relative,
            ..Default::default()
        };
        let report = compare(&a, &b, &options).unwrap();
        assert!(report.differences.is_empty() && report.moves.is_empty());
        assert_eq!(report.similarity, 1.0);
        decimal.bench_function(name, |bencher| {
            bencher.iter(|| compare(black_box(&a), black_box(&b), &options).unwrap());
        });
    }
    decimal.finish();

    let a = json!({"items": (0..2000).map(|i| json!({"nested": {"score": i, "keep": "unchanged"}}))
        .collect::<Vec<_>>(), "untouched": vec!["unchanged payload"; 5000]});
    let operations: Vec<_> = (0..2000)
        .map(|i| json!({"op": "replace", "path": format!("/items/{i}/nested/score"), "value": -1}))
        .collect();
    let change: json_patch::Patch = serde_json::from_value(json!(operations)).unwrap();
    let mut b = a.clone();
    json_patch::patch(&mut b, &change).unwrap();
    let guards: Vec<_> = (0..2000)
        .flat_map(|i| {
            [
                json!({"op": "test", "path": format!("/items/{i}/nested/score"), "value": i}),
                json!({"op": "replace", "path": format!("/items/{i}/nested/score"), "value": -1}),
            ]
        })
        .collect();
    let guards: json_patch::Patch = serde_json::from_value(json!(guards)).unwrap();
    let inverse = invert_json_patch(&a, &change).unwrap();
    assert_eq!(apply_json_patch(&a, &change).unwrap(), b);
    assert_eq!(apply_json_patch(&a, &guards).unwrap(), b);
    assert_eq!(apply_json_patch(&b, &inverse).unwrap(), a);
    let mut application = c.benchmark_group("risk/standard-apply");
    for (name, baseline, patch, expected) in [
        ("replace-2000", &a, &change, &b),
        ("guarded-4000", &a, &guards, &b),
        ("inverse-2000", &b, &inverse, &a),
    ] {
        let mut independent = baseline.clone();
        json_patch::patch(&mut independent, patch).unwrap();
        assert_eq!(&independent, expected);
        application.bench_function(BenchmarkId::new("ours", name), |bencher| {
            bencher.iter(|| apply_json_patch(black_box(baseline), black_box(patch)).unwrap());
        });
        application.bench_function(BenchmarkId::new("typed-json-patch", name), |bencher| {
            bencher.iter(|| {
                let mut result = black_box(baseline).clone();
                json_patch::patch(&mut result, black_box(patch)).unwrap();
                result
            });
        });
    }
    application.bench_function("invert-2000", |bencher| {
        bencher.iter(|| invert_json_patch(black_box(&a), black_box(&change)).unwrap());
    });
    let guarded_inverse = invert_json_patch_guarded(&a, &change).unwrap();
    assert_eq!(apply_json_patch(&b, &guarded_inverse).unwrap(), a);
    application.bench_function("invert-guarded-2000", |bencher| {
        bencher.iter(|| invert_json_patch_guarded(black_box(&a), black_box(&change)).unwrap());
    });
    application.finish();
}

fn read_only_tests(c: &mut Criterion) {
    let baseline =
        json!({"keep": "unchanged payload".repeat(65_536), "items": (0..1000).collect::<Vec<_>>()});
    let mut group = c.benchmark_group("standard/test-only");
    for count in [1, 1000] {
        let tests: json_patch::Patch = serde_json::from_value(json!(
            (0..count)
                .map(|i| json!({"op":"test", "path":format!("/items/{i}"), "value":i}))
                .collect::<Vec<_>>()
        ))
        .unwrap();
        test_json_patch(&baseline, &tests).unwrap();
        group.bench_function(BenchmarkId::new("borrowed-read-only", count), |bencher| {
            bencher.iter(|| test_json_patch(black_box(&baseline), black_box(&tests)).unwrap());
        });
        group.bench_function(
            BenchmarkId::new("borrowed-clone-return", count),
            |bencher| {
                bencher.iter(|| apply_json_patch(black_box(&baseline), black_box(&tests)).unwrap());
            },
        );
        group.bench_function(
            BenchmarkId::new("json-patch-borrowed-clone", count),
            |bencher| {
                bencher.iter(|| {
                    let mut document = black_box(&baseline).clone();
                    json_patch::patch(&mut document, black_box(&tests)).unwrap();
                    document
                });
            },
        );
        // An already mutable document needs no clone in json-patch. This has a
        // different ownership boundary and omits Palim's source-depth check.
        let mut mutable = baseline.clone();
        group.bench_function(
            BenchmarkId::new("json-patch-already-mutable", count),
            |bencher| {
                bencher.iter(|| {
                    json_patch::patch(black_box(&mut mutable), black_box(&tests)).unwrap()
                });
            },
        );
    }
    group.finish();
}
fn object_migrations(c: &mut Criterion) {
    let mut group = c.benchmark_group("standard/object-migrations");
    for count in [100, 800] {
        for duplicates in [false, true] {
            let files = |prefix: &str| {
                (0..count)
                    .map(|i| {
                        (
                            format!("{prefix}_{i:04}.toml"),
                            json!({"component":if duplicates {0} else {i},
                                "contents":"manifest Unicode 🦀 ".repeat(40)}),
                        )
                    })
                    .collect::<serde_json::Map<_, _>>()
            };
            let left = json!({"files":files("legacy")});
            let right = json!({"files":files("current")});
            for (mode, rationalize, tests) in [
                ("factorize", false, false),
                ("optimized", true, false),
                ("guarded", true, true),
            ] {
                let options = JsonPatchOptions {
                    factorize: true,
                    rationalize,
                    tests,
                };
                let change = palim::diff_json_patch(&left, &right, &options).unwrap();
                let mut applied = left.clone();
                json_patch::patch(&mut applied, &change).unwrap();
                assert_eq!(applied, right);
                let inverse = invert_json_patch(&left, &change).unwrap();
                json_patch::patch(&mut applied, &inverse).unwrap();
                assert_eq!(applied, left);
                group.bench_function(
                    BenchmarkId::new(mode, format!("{count}-duplicates-{duplicates}")),
                    |bencher| {
                        bencher.iter(|| {
                            palim::diff_json_patch(black_box(&left), black_box(&right), &options)
                                .unwrap()
                        });
                    },
                );
            }
        }
    }
    group.finish();
}

criterion_group! { name = benches; config = Criterion::default().sample_size(20).warm_up_time(Duration::from_millis(100)).measurement_time(Duration::from_millis(300)).nresamples(5000); targets=benchmarks, extended_apis, risk_cases, read_only_tests, object_migrations }
criterion_main!(benches);
