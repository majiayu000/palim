use palim::{CompareOptions, compare};
use serde_json::{Value, json};
use std::sync::Arc;

#[test]
fn exact_json_report_and_mathematical_numbers() {
    let before = json!({"a/b": [1, {"~key": false}], "gone": null});
    let after = json!({"a/b": [1.0, {"~key": true}], "new": 2});
    let report = compare(&before, &after, &CompareOptions::default()).unwrap();
    assert_eq!(report.differences.len(), 3);
    assert_eq!(report.differences[0].path, "/a~1b/1/~0key");
    assert_eq!(report.differences[1].left, Some(Value::Null));
    assert_eq!(report.differences[1].right, None);
    assert_eq!(report.differences[2].left, None);
    let restored = serde_json::from_value(serde_json::to_value(&report).unwrap()).unwrap();
    assert_eq!(
        report, restored,
        "absence and JSON null survive report serialization"
    );
    assert!(report.similarity > 0.0 && report.similarity < 1.0);
    assert!(report.visited >= 7);
    assert!(
        compare(&json!(1), &json!(1.0), &CompareOptions::default())
            .unwrap()
            .differences
            .is_empty()
    );
    assert_eq!(
        compare(
            &json!(9007199254740993_u64),
            &json!(9007199254740992.0),
            &CompareOptions::default()
        )
        .unwrap()
        .differences
        .len(),
        1
    );
    assert_eq!(
        compare(
            &json!(u64::MAX),
            &json!(18446744073709551616.0),
            &CompareOptions::default()
        )
        .unwrap()
        .differences
        .len(),
        1
    );
}

#[cfg(feature = "exact-numbers")]
#[test]
fn arbitrary_precision_number_matching_and_exact_decimal_tolerance() {
    let left: Value = serde_json::from_str("[1e10000,1000000000000000000000000000001]").unwrap();
    let right: Value = serde_json::from_str("[1000000000000000000000000000001.0,10e9999]").unwrap();
    let options = CompareOptions {
        unordered: Some(Arc::new(|_| true)),
        ..Default::default()
    };
    let report = compare(&left, &right, &options).unwrap();
    assert!(report.differences.is_empty());
    assert_eq!(report.similarity, 1.0);
    let changed: Value = serde_json::from_str("[2e10000,1000000000000000000000000000001]").unwrap();
    assert_eq!(
        compare(&left, &changed, &options)
            .unwrap()
            .differences
            .len(),
        2
    );
    let tolerant = CompareOptions {
        absolute_tolerance: 1.0,
        ..Default::default()
    };
    let huge: Value = serde_json::from_str("1e10000").unwrap();
    let same: Value = serde_json::from_str("10e9999").unwrap();
    let other: Value = serde_json::from_str("2e10000").unwrap();
    assert!(
        compare(&huge, &same, &tolerant)
            .unwrap()
            .differences
            .is_empty()
    );
    assert_eq!(
        compare(&huge, &other, &tolerant).unwrap().differences.len(),
        1
    );
    let rounded: Value = serde_json::from_str("1000000000000000000000000000001").unwrap();
    let nearby: Value = serde_json::from_str("1e30").unwrap();
    assert!(
        compare(&rounded, &nearby, &tolerant)
            .unwrap()
            .differences
            .is_empty()
    );
}

#[cfg(feature = "exact-numbers")]
#[test]
fn numeric_work_budget_precedes_exact_and_unordered_canonicalization() {
    let exponent = format!("1{}", "0".repeat(4096));
    let preceding = "9".repeat(4096);
    let parse = |text: &str| serde_json::from_str::<Value>(text).unwrap();
    let huge = parse(&format!("1e{exponent}"));
    let equivalent = parse(&format!("10e{preceding}"));
    let options = CompareOptions {
        max_comparisons: Some(10),
        ..Default::default()
    };
    let error = compare(&json!({"n":huge}), &json!({"n":equivalent}), &options).unwrap_err();
    assert_eq!(error.path, "/n");
    assert!(error.message.contains("max_comparisons"));
    let unordered = CompareOptions {
        unordered: Some(Arc::new(|_| true)),
        ..options
    };
    let error = compare(&json!([huge]), &json!([equivalent]), &unordered).unwrap_err();
    assert_eq!(error.path, "/0");
    let error = compare(&json!([{"n":huge}]), &json!([{"n":equivalent}]), &unordered).unwrap_err();
    assert_eq!(error.path, "/0/n");
    // Identical representation takes the ordinary equality shortcut; no
    // canonical exponent parser or decimal arithmetic runs in this case.
    let one_visit = CompareOptions {
        max_comparisons: Some(1),
        ..Default::default()
    };
    assert!(
        compare(&huge, &huge, &one_visit)
            .unwrap()
            .differences
            .is_empty()
    );
}

#[test]
fn filters_include_root_array_and_missing_nodes() {
    let options = CompareOptions {
        node_filter: Some(Arc::new(|path, left, right| {
            path != "/0" && !matches!((left, right), (None, Some(_)))
        })),
        ..Default::default()
    };
    let report = compare(&json!([1]), &json!([2, 3]), &options).unwrap();
    assert!(report.differences.is_empty());
    assert_eq!(report.similarity, 1.0);
    let root_filter = CompareOptions {
        node_filter: Some(Arc::new(|path, _, _| !path.is_empty())),
        ..Default::default()
    };
    assert!(
        compare(&json!(1), &json!(2), &root_filter)
            .unwrap()
            .differences
            .is_empty()
    );
}

#[test]
fn custom_comparison_has_a_fallback_and_can_override_equality() {
    let options = CompareOptions {
        custom_equal: Some(Arc::new(|path, left, right| match path {
            "/name" => Some(
                left.as_str()
                    .unwrap()
                    .eq_ignore_ascii_case(right.as_str().unwrap()),
            ),
            "/force" => Some(false),
            _ => None,
        })),
        ..Default::default()
    };
    let report = compare(
        &json!({"name": "ALICE", "age": 1, "force": 0}),
        &json!({"name": "alice", "age": 2, "force": 0}),
        &options,
    )
    .unwrap();
    assert_eq!(
        report
            .differences
            .iter()
            .map(|d| d.path.as_str())
            .collect::<Vec<_>>(),
        ["/age", "/force"]
    );
}

#[test]
fn tolerance_uses_absolute_relative_and_exact_large_integer_distance() {
    let options = CompareOptions {
        absolute_tolerance: 0.01,
        relative_tolerance: 0.001,
        ..Default::default()
    };
    assert!(
        compare(
            &json!([0.001, 1000, -1000]),
            &json!([0.009, 1001, -1001]),
            &options
        )
        .unwrap()
        .differences
        .is_empty()
    );
    assert_eq!(
        compare(&json!(0), &json!(0.02), &options)
            .unwrap()
            .differences
            .len(),
        1
    );
    let exact_distance = CompareOptions {
        absolute_tolerance: 1.0,
        ..Default::default()
    };
    assert_eq!(
        compare(
            &json!(9007199254740992_u64),
            &json!(9007199254740994_u64),
            &exact_distance
        )
        .unwrap()
        .differences
        .len(),
        1
    );
    #[cfg(feature = "exact-numbers")]
    assert!(
        compare(
            &json!(u64::MAX),
            &serde_json::from_str("18446744073709551616.0").unwrap(),
            &exact_distance
        )
        .unwrap()
        .differences
        .is_empty()
    );
    // The shortest JSON representation of this f64 is 1.8446744073709552e19,
    // whose exact decimal value is 385 above u64::MAX, rather than one above it.
    assert_eq!(
        compare(
            &json!(u64::MAX),
            &json!(18446744073709551616.0),
            &exact_distance
        )
        .unwrap()
        .differences
        .len(),
        1
    );
    let overflow = CompareOptions {
        relative_tolerance: 1.9,
        ..Default::default()
    };
    assert_eq!(
        compare(&json!(f64::MAX), &json!(-f64::MAX), &overflow)
            .unwrap()
            .differences
            .len(),
        1
    );
}

#[test]
fn unordered_arrays_are_multisets_selected_by_path() {
    let options = CompareOptions {
        unordered: Some(Arc::new(|path| path == "/set")),
        ..Default::default()
    };
    let left = json!({"set": [1, 1, 2], "list": [1, 2]});
    let right = json!({"set": [2, 1, 1], "list": [2, 1]});
    let report = compare(&left, &right, &options).unwrap();
    assert_eq!(report.differences.len(), 2);
    assert!(report.moves.is_empty());
    let report = compare(
        &json!({"set": [1, 1, 2]}),
        &json!({"set": [1, 2, 2]}),
        &options,
    )
    .unwrap();
    assert_eq!(
        report
            .differences
            .iter()
            .filter(|d| d.left.is_some())
            .count(),
        1
    );
    assert_eq!(
        report
            .differences
            .iter()
            .filter(|d| d.right.is_some())
            .count(),
        1
    );
}

#[test]
fn unordered_matching_finds_maximum_pairs_instead_of_greedy_pairs() {
    let options = CompareOptions {
        unordered: Some(Arc::new(|_| true)),
        absolute_tolerance: 1.0,
        ..Default::default()
    };
    // 0 can match either 1 or -1, but 1 can match only 1.
    let report = compare(&json!([0, 1]), &json!([1, -1]), &options).unwrap();
    assert!(report.differences.is_empty());
    assert_eq!(report.similarity, 1.0);
}

#[test]
fn nested_unordered_containers_and_keyed_changes() {
    let options = CompareOptions {
        unordered: Some(Arc::new(|_| true)),
        ..Default::default()
    };
    let report = compare(
        &json!([{ "id": 1, "tags": [1, 2] }, { "id": 2 }]),
        &json!([{ "id": 2 }, { "id": 1.0, "tags": [2, 1] }]),
        &options,
    )
    .unwrap();
    assert!(report.differences.is_empty());
    assert_eq!(report.similarity, 1.0);
    let options = CompareOptions {
        unordered: Some(Arc::new(|_| true)),
        object_hash: Some(Arc::new(|value, _| value.get("id").map(Value::to_string))),
        ..Default::default()
    };
    let report = compare(
        &json!([{ "id": 1, "value": 1 }, { "id": 2 }]),
        &json!([{ "id": 2 }, { "id": 1, "value": 2 }]),
        &options,
    )
    .unwrap();
    assert!(report.moves.is_empty());
    assert_eq!(report.differences.len(), 1);
    assert_eq!(report.differences[0].path, "/1/value");
}

#[test]
fn structural_fingerprints_reduce_candidates_and_still_check_nested_order() {
    let items: Vec<_> = (0..256)
        .map(|id| json!({ "id": id, "value": id }))
        .collect();
    let mut reordered = items.clone();
    reordered.reverse();
    // Keep the original 5,000 node/matching allowance and include the exact
    // input digit work for two numeric fields in each of the two documents.
    // This remains far below exhaustive 256-by-256 candidate matching.
    let digit_work = 4 * (0..256).map(|id| id.to_string().len()).sum::<usize>();
    let work_limit = 5_000 + digit_work;
    let options = CompareOptions {
        unordered: Some(Arc::new(|path| path.is_empty())),
        max_comparisons: Some(work_limit),
        ..Default::default()
    };
    let report = compare(&json!(items), &json!(reordered), &options).unwrap();
    assert!(report.differences.is_empty());
    assert!(report.visited < work_limit);
    // Both nested arrays have the same orderless fingerprint, but this path is ordered.
    let report = compare(&json!([[1, 2]]), &json!([[2, 1]]), &options).unwrap();
    assert_eq!(report.differences.len(), 2);
}

#[test]
fn excluded_identity_items_do_not_report_moves_and_budget_counts_matching_edges() {
    let options = CompareOptions {
        object_hash: Some(Arc::new(|value, _| value.get("id").map(Value::to_string))),
        node_filter: Some(Arc::new(|path, _, _| path.is_empty())),
        ..Default::default()
    };
    let report = compare(
        &json!([{ "id": 1 }, { "id": 2 }]),
        &json!([{ "id": 2 }, { "id": 1 }]),
        &options,
    )
    .unwrap();
    assert!(report.moves.is_empty());
    assert!(report.differences.is_empty());
    assert_eq!(report.similarity, 1.0);
    let bounded = CompareOptions {
        unordered: Some(Arc::new(|_| true)),
        absolute_tolerance: 1.0,
        max_comparisons: Some(5),
        ..Default::default()
    };
    let error = compare(&json!([0, 1]), &json!([1, -1]), &bounded).unwrap_err();
    assert!(error.message.contains("max_comparisons"));
}

#[test]
fn identities_report_moves_and_nested_changes_without_counting_missing_as_null() {
    let options = CompareOptions {
        object_hash: Some(Arc::new(|value, _| value.get("id").map(Value::to_string))),
        ..Default::default()
    };
    let report = compare(
        &json!([{ "id": 1, "v": 10 }, { "id": 2, "v": 20 }]),
        &json!([{ "id": 2, "v": 21 }, { "id": 1, "v": 10 }]),
        &options,
    )
    .unwrap();
    assert_eq!(report.differences.len(), 1);
    assert_eq!(report.differences[0].path, "/0/v");
    assert_eq!(report.moves.len(), 2);
    assert!(
        report
            .moves
            .iter()
            .any(|m| m.from == "/1" && m.path == "/0")
    );
    assert!(report.similarity < 1.0);
}

#[test]
fn custom_matcher_and_duplicate_identities_are_deterministic() {
    let options = CompareOptions {
        array_item_matcher: Some(Arc::new(|_, left, right| left.get("id") == right.get("id"))),
        ..Default::default()
    };
    let left = json!([{ "id": 1, "v": "a" }, { "id": 1, "v": "b" }, { "id": 2 }]);
    let right = json!([{ "id": 2 }, { "id": 1, "v": "a" }, { "id": 1, "v": "b" }]);
    let a = compare(&left, &right, &options).unwrap();
    let b = compare(&left, &right, &options).unwrap();
    assert_eq!(a, b);
    assert!(a.differences.is_empty());
    assert_eq!(a.moves.len(), 3);
}

#[test]
fn budgets_and_invalid_options_return_errors_instead_of_partial_reports() {
    let limited = CompareOptions {
        max_differences: Some(1),
        ..Default::default()
    };
    let error = compare(&json!([1, 2]), &json!([3, 4]), &limited).unwrap_err();
    assert_eq!(error.path, "/1");
    assert!(error.message.contains("max_differences"));
    let limited = CompareOptions {
        max_comparisons: Some(1),
        ..Default::default()
    };
    assert!(
        compare(&json!([1]), &json!([1]), &limited)
            .unwrap_err()
            .message
            .contains("max_comparisons")
    );
    for tolerance in [-1.0, f64::NAN, f64::INFINITY] {
        let bad = CompareOptions {
            absolute_tolerance: tolerance,
            ..Default::default()
        };
        assert!(compare(&json!(1), &json!(1), &bad).is_err());
    }
    let depth = CompareOptions {
        max_depth: 0,
        ..Default::default()
    };
    assert!(compare(&json!([1]), &json!([1]), &depth).is_err());
}

#[test]
fn empty_equal_documents_and_report_serialization() {
    for value in [json!({}), json!([]), json!(null)] {
        let report = compare(&value, &value, &CompareOptions::default()).unwrap();
        assert_eq!(report.similarity, 1.0);
        let restored = serde_json::from_value(serde_json::to_value(&report).unwrap()).unwrap();
        assert_eq!(report, restored);
    }
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config {
        cases: 256,
        rng_seed: proptest::test_runner::RngSeed::Fixed(20260930),
        .. proptest::test_runner::Config::default()
    })]
    #[test]
    fn unordered_primitive_permutation_preserves_repeated_counts(
        values in proptest::collection::vec(-10_i64..10, 0..35)
    ) {
        let left = json!(values);
        let mut reversed = values.clone();
        reversed.reverse();
        let options = CompareOptions { unordered: Some(Arc::new(|_| true)), ..Default::default() };
        let report = compare(&left, &json!(reversed), &options).unwrap();
        proptest::prop_assert!(report.differences.is_empty());
        proptest::prop_assert_eq!(report.similarity, 1.0);
    }

    #[test]
    fn tolerance_pairing_agrees_with_exhaustive_matching(
        left in proptest::collection::vec(-3_i64..4, 0..7),
        right in proptest::collection::vec(-3_i64..4, 0..7)
    ) {
        fn maximum(left: &[i64], right: &[i64], used: u64) -> usize {
            let Some((&first, rest)) = left.split_first() else { return 0; };
            let mut result = maximum(rest, right, used);
            for (j, value) in right.iter().enumerate() {
                if used & (1 << j) == 0 && (first - value).abs() <= 1 {
                    result = result.max(1 + maximum(rest, right, used | (1 << j)));
                }
            }
            result
        }
        let pairs = maximum(&left, &right, 0);
        let options = CompareOptions {
            unordered: Some(Arc::new(|_| true)), absolute_tolerance: 1.0,
            ..Default::default()
        };
        let report = compare(&json!(left), &json!(right), &options).unwrap();
        proptest::prop_assert_eq!(report.differences.len(), left.len() + right.len() - 2 * pairs);
    }
}
