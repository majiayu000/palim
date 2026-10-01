use palim::{Delta, DiffOptions, DiffPatcher, PatchOperation, diff};
use proptest::prelude::*;
use serde_json::{Value, json};
use std::sync::Arc;

fn move_count(delta: &Value) -> usize {
    match delta {
        Value::Object(fields) => fields
            .iter()
            .map(|(key, value)| {
                if key.starts_with('_')
                    && value.as_array().is_some_and(|a| a.len() == 3 && a[2] == 3)
                {
                    1
                } else {
                    move_count(value)
                }
            })
            .sum(),
        _ => 0,
    }
}

fn check(left: &Value, right: &Value, delta: &Delta) {
    let patch = delta.to_json_patch(left).unwrap();
    let moves = patch
        .0
        .iter()
        .filter(|op| matches!(op, PatchOperation::Move(_)))
        .count();
    assert!(
        moves <= move_count(delta.as_value()),
        "{delta:?}: {patch:?}"
    );
    // Use the independent RFC application implementation directly. Its in-place
    // Vec edits are deliberately outside the exporter's rank computation.
    let mut actual = left.clone();
    json_patch::patch(&mut actual, &patch).unwrap();
    assert_eq!(&actual, right, "{delta:?}: {patch:?}");
}

#[test]
fn exhaustive_small_permutations_and_explicit_all_item_moves() {
    fn visit(left: &Value, values: &mut [usize], at: usize) {
        if at == values.len() {
            let right = json!(values);
            if let Some(delta) = diff(left, &right).unwrap() {
                check(left, &right, &delta);
            }
            // An imported producer may label every item as moved, including
            // items already at their destination. Avoid unnecessary RFC moves.
            let mut fields = serde_json::Map::new();
            fields.insert("_t".into(), json!("a"));
            for (new, old) in values.iter().enumerate() {
                fields.insert(format!("_{old}"), json!(["", new, 3]));
            }
            check(
                left,
                &right,
                &Delta::from_value(Value::Object(fields)).unwrap(),
            );
            return;
        }
        for next in at..values.len() {
            values.swap(at, next);
            visit(left, values, at + 1);
            values.swap(at, next);
        }
    }
    for len in 0..=7 {
        let mut values = (0..len).collect::<Vec<_>>();
        visit(&json!(values), &mut values, 0);
    }
}

#[test]
fn large_half_swaps_and_reversals_keep_one_operation_per_move() {
    for len in [500, 2_000, 20_000] {
        let left = json!((0..len).collect::<Vec<_>>());
        for (right, expected_moves) in [
            (
                json!((len / 2..len).chain(0..len / 2).collect::<Vec<_>>()),
                len / 2,
            ),
            (json!((0..len).rev().collect::<Vec<_>>()), len - 1),
        ] {
            let delta = diff(&left, &right).unwrap().unwrap();
            let patch = delta.to_json_patch(&left).unwrap();
            assert_eq!(move_count(delta.as_value()), expected_moves);
            assert_eq!(patch.0.len(), expected_moves);
            assert!(
                patch
                    .0
                    .iter()
                    .all(|op| matches!(op, PatchOperation::Move(_)))
            );
            check(&left, &right, &delta);
        }
    }
}

#[test]
fn typed_payloads_text_and_baseline_validation() {
    let big: Value =
        serde_json::from_str("12345678901234567890123456789012345678901234567890").unwrap();
    let left = json!({"a/b~": "a".repeat(100), "old": 1});
    let right = json!({"a/b~": "b".repeat(100), "big": big});
    let delta = diff(&left, &right).unwrap().unwrap();
    check(&left, &right, &delta);
    let bad_baseline = json!({"a/b~": "a".repeat(100), "old": 999});
    assert!(delta.to_json_patch(&bad_baseline).is_err());
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, rng_seed: proptest::test_runner::RngSeed::Fixed(20261001), .. ProptestConfig::default() })]
    #[test]
    fn duplicate_ids_add_remove_and_nested_edits(
        before in proptest::collection::vec((0u8..8, any::<i16>()), 0..40),
        after in proptest::collection::vec((0u8..8, any::<i16>()), 0..40),
        include in any::<bool>(),
    ) {
        let document = |items: &[(u8, i16)]| json!({"a/b~":items.iter().map(|(id, value)|
            json!({"id":id,"nested":[value, id],"text":format!("{}-{value}","x".repeat(70))})
        ).collect::<Vec<_>>()});
        let left = document(&before);
        let right = document(&after);
        let dp = DiffPatcher::new(DiffOptions {
            object_hash: Some(Arc::new(|v, _| v.get("id").map(Value::to_string))),
            include_value_on_move: include,
            ..Default::default()
        });
        if let Some(delta) = dp.diff(&left, &right).unwrap() {
            check(&left, &right, &delta);
        } else {
            prop_assert_eq!(left, right);
        }
    }
}
