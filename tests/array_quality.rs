use palim::{Delta, DiffOptions, DiffPatcher, PatchOperation, diff, patch, reverse};
use proptest::prelude::*;
use serde_json::{Value, json};
use std::sync::Arc;

fn native_moves(value: &Value) -> usize {
    let Some(map) = value.as_object() else {
        return 0;
    };
    map.iter()
        .map(|(key, child)| {
            usize::from(
                map.get("_t") == Some(&json!("a"))
                    && key.starts_with('_')
                    && key != "_t"
                    && child.get(2) == Some(&json!(3)),
            ) + native_moves(child)
        })
        .sum()
}

fn assert_round_trip(left: &Value, right: &Value, delta: &Delta) -> usize {
    assert_eq!(patch(left, delta).unwrap(), *right);
    let inverse = reverse(delta).unwrap();
    assert_eq!(patch(right, &inverse).unwrap(), *left);
    assert_eq!(reverse(&inverse).unwrap(), *delta);
    let standard = delta.to_json_patch(left).unwrap();
    let mut independent = left.clone();
    json_patch::patch(&mut independent, &standard).unwrap();
    assert_eq!(independent, *right, "RFC patch: {standard:?}");
    let standard_inverse = inverse.to_json_patch(right).unwrap();
    json_patch::patch(&mut independent, &standard_inverse).unwrap();
    assert_eq!(independent, *left);
    standard
        .0
        .iter()
        .filter(|op| matches!(op, PatchOperation::Move(_)))
        .count()
}

// An independent quadratic LCS oracle, deliberately different from production LIS.
fn minimum_moves(target: &[usize]) -> usize {
    target.len() - lcs_length(&(0..target.len()).collect::<Vec<_>>(), target)
}

fn lcs_length(source: &[usize], target: &[usize]) -> usize {
    let mut previous = vec![0; target.len() + 1];
    for &old in source {
        let mut current = vec![0; target.len() + 1];
        for (index, &new) in target.iter().enumerate() {
            current[index + 1] = if old == new {
                previous[index] + 1
            } else {
                previous[index + 1].max(current[index])
            };
        }
        previous = current;
    }
    previous[target.len()]
}

fn permutations(values: &mut [usize], start: usize, visit: &mut impl FnMut(&[usize])) {
    if start == values.len() {
        visit(values);
        return;
    }
    for index in start..values.len() {
        values.swap(start, index);
        permutations(values, start + 1, visit);
        values.swap(start, index);
    }
}

#[test]
fn unique_permutations_use_the_minimum_native_and_standard_moves() {
    let mut checked = 0;
    for len in 2..=7 {
        let source: Vec<_> = (0..len).collect();
        let left = json!(source);
        permutations(&mut source.clone(), 0, &mut |target| {
            if target == source {
                return;
            }
            let right = json!(target);
            let delta = diff(&left, &right).unwrap().unwrap();
            let expected = minimum_moves(target);
            assert_eq!(native_moves(delta.as_value()), expected, "{target:?}");
            assert_eq!(
                assert_round_trip(&left, &right, &delta),
                expected,
                "{target:?}"
            );
            checked += 1;
        });
    }
    assert_eq!(checked, 5906);
}

#[test]
fn small_repeated_sequences_use_the_minimum_stable_subsequence() {
    let sequences: Vec<Vec<usize>> = (0..64)
        .map(|bits| (0..6).map(|shift| (bits >> shift) & 1).collect())
        .collect();
    let mut checked = 0;
    for source in &sequences {
        for target in &sequences {
            if source == target {
                continue;
            }
            let common: usize = (0..2)
                .map(|token| {
                    source
                        .iter()
                        .filter(|&&x| x == token)
                        .count()
                        .min(target.iter().filter(|&&x| x == token).count())
                })
                .sum();
            let expected = common - lcs_length(source, target);
            let (left, right) = (json!(source), json!(target));
            let delta = diff(&left, &right).unwrap().unwrap();
            assert_eq!(
                native_moves(delta.as_value()),
                expected,
                "{source:?} → {target:?}"
            );
            assert_eq!(assert_round_trip(&left, &right, &delta), expected);
            let repeated = diff(&left, &right).unwrap().unwrap();
            assert_eq!(delta, repeated);
            checked += 1;
        }
    }
    assert_eq!(checked, 4032);
}

#[test]
fn large_dense_repetition_and_duplicate_identities_round_trip() {
    let source: Vec<_> = (0..2048).map(|index| index % 2).collect();
    let target: Vec<_> = source
        .iter()
        .skip(1)
        .chain(source.iter().take(1))
        .copied()
        .collect();
    let (left, right) = (json!(source), json!(target));
    let delta = diff(&left, &right).unwrap().unwrap();
    assert_round_trip(&left, &right, &delta);
    let engine = DiffPatcher::new(DiffOptions {
        object_hash: Some(Arc::new(|item, _| item.get("id").map(Value::to_string))),
        ..Default::default()
    });
    let left = json!([{"id":0,"v":0},{"id":0,"v":1},{"id":1,"v":2}]);
    let right = json!([{"id":1,"v":3},{"id":0,"v":4},{"id":0,"v":5}]);
    let delta = engine.diff(&left, &right).unwrap().unwrap();
    assert_eq!(native_moves(delta.as_value()), 1);
    assert_round_trip(&left, &right, &delta);
}

#[test]
fn standard_export_does_not_repeat_a_native_move() {
    let left = json!([0, 1, 2, 3]);
    let right = json!([1, 3, 0, 2]);
    let delta = diff(&left, &right).unwrap().unwrap();
    assert_eq!(native_moves(delta.as_value()), 2);
    assert_eq!(assert_round_trip(&left, &right, &delta), 2);
}

#[test]
fn half_rotation_of_500_items_needs_250_moves() {
    let left = json!((0..500).collect::<Vec<_>>());
    let right = json!((250..500).chain(0..250).collect::<Vec<_>>());
    let delta = diff(&left, &right).unwrap().unwrap();
    assert_eq!(native_moves(delta.as_value()), 250);
    assert_eq!(assert_round_trip(&left, &right, &delta), 250);
}

#[test]
fn unique_identity_moves_preserve_nested_edits_and_baseline_checks() {
    let engine = DiffPatcher::new(DiffOptions {
        object_hash: Some(Arc::new(|item, _| item.get("id").map(Value::to_string))),
        ..Default::default()
    });
    let left = json!({"items/~":[
        {"id":0,"nested":{"x/y":0}}, {"id":1,"nested":{"x/y":1}},
        {"id":2,"nested":{"x/y":2}}, {"id":3,"nested":{"x/y":3}}
    ]});
    let right = json!({"items/~":[
        {"id":3,"nested":{"x/y":30}}, {"id":0,"nested":{"x/y":0}},
        {"id":2,"nested":{"x/y":2}}, {"id":1,"nested":{"x/y":10}}
    ]});
    let delta = engine.diff(&left, &right).unwrap().unwrap();
    assert_eq!(native_moves(delta.as_value()), 2);
    assert_eq!(assert_round_trip(&left, &right, &delta), 2);
    let mut wrong = left.clone();
    wrong["items/~"][3]["nested"]["x/y"] = json!(999);
    let saved = wrong.clone();
    let error = delta.to_json_patch(&wrong).unwrap_err();
    assert_eq!(error.path, "/items~1~0/0/nested/x~1y");
    assert_eq!(wrong, saved);
}

fn ordered_items(entries: std::collections::BTreeMap<u8, (u16, i16)>) -> Value {
    let mut entries: Vec<_> = entries.into_iter().collect();
    entries.sort_unstable_by_key(|(id, (order, _))| (*order, *id));
    json!({"items/~": entries.into_iter().map(|(id, (_, value))|
        json!({"id":id,"nested":{"x/y":value,"list":[value,0]}})
    ).collect::<Vec<_>>()})
}

#[test]
fn node_filter_projects_original_positions_and_object_additions_and_deletions() {
    let engine = DiffPatcher::new(DiffOptions {
        node_filter: Some(Arc::new(|path, _, _| {
            !matches!(
                path,
                "/items/0" | "/items/2" | "/ignored" | "/left_only" | "/right_only"
            )
        })),
        ..Default::default()
    });
    let left = json!({"items":[0,1,2,3],"ignored":1,"left_only":2,"kept":3});
    let raw_right = json!({"items":[30,20],"ignored":9,"right_only":4,"kept":4});
    let projected = json!({"items":[0,20,2],"ignored":1,"left_only":2,"kept":4});
    let delta = engine.diff(&left, &raw_right).unwrap().unwrap();
    assert_round_trip(&left, &projected, &delta);
    let root_filtered = DiffPatcher::new(DiffOptions {
        node_filter: Some(Arc::new(|path, _, _| !path.is_empty())),
        ..Default::default()
    });
    assert!(root_filtered.diff(&left, &raw_right).unwrap().is_none());
    let add_filtered = DiffPatcher::new(DiffOptions {
        node_filter: Some(Arc::new(|path, left, right| {
            path != "/1" || left.is_some() || right.is_none()
        })),
        ..Default::default()
    });
    assert!(
        add_filtered
            .diff(&json!([0]), &json!([0, 9]))
            .unwrap()
            .is_none()
    );
}

#[test]
fn projected_array_items_are_filtered_once_and_nested_fields_still_filter() {
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = calls.clone();
    let engine = DiffPatcher::new(DiffOptions {
        object_hash: Some(Arc::new(|item, _| item.get("id").map(Value::to_string))),
        node_filter: Some(Arc::new(move |path, _, _| {
            recorded.lock().unwrap().push(path.to_owned());
            !path.ends_with("/ignored")
        })),
        ..Default::default()
    });
    let left = json!([{"id":0,"ignored":1,"kept":2},{"id":1,"ignored":3,"kept":4}]);
    let raw_right = json!([{"id":1,"ignored":30,"kept":40},{"id":0,"ignored":10,"kept":20}]);
    let projected = json!([{"id":1,"ignored":3,"kept":40},{"id":0,"ignored":1,"kept":20}]);
    let delta = engine.diff(&left, &raw_right).unwrap().unwrap();
    assert_round_trip(&left, &projected, &delta);
    let calls = calls.lock().unwrap();
    assert_eq!(calls.iter().filter(|path| *path == "/0").count(), 1);
    assert_eq!(calls.iter().filter(|path| *path == "/1").count(), 1);
    assert!(calls.iter().any(|path| path.ends_with("/ignored")));
}

#[test]
fn subtree_filters_project_additions_type_changes_and_partial_deletions() {
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = calls.clone();
    let engine = DiffPatcher::new(DiffOptions {
        node_filter: Some(Arc::new(move |path, left, right| {
            recorded
                .lock()
                .unwrap()
                .push((path.to_owned(), left.is_some(), right.is_some()));
            !path.ends_with("/secret") && path != "/added_array/1"
        })),
        ..Default::default()
    });
    let left = json!({
        "changed":0,
        "removed":{"secret":"keep","drop":1,"nested":{"secret":"also keep","drop":2}},
        "removed_array":[9,{"secret":"retain","drop":3},null]
    });
    let right = json!({
        "changed":{"visible":3,"secret":"omit"},
        "added":{"visible":1,"secret":"omit","nested":{"visible":2,"secret":"omit"}},
        "added_array":[{"visible":4,"secret":"omit"},"omit",{"visible":5,"secret":"omit"}]
    });
    let expected = json!({
        "changed":{"visible":3},
        "removed":{"secret":"keep","nested":{"secret":"also keep"}},
        "removed_array":[{"secret":"retain"}],
        "added":{"visible":1,"nested":{"visible":2}},
        "added_array":[{"visible":4},{"visible":5}]
    });
    let delta = engine.diff(&left, &right).unwrap().unwrap();
    assert_round_trip(&left, &expected, &delta);
    let calls = calls.lock().unwrap();
    assert!(calls.contains(&("/changed/secret".to_owned(), false, true)));
    assert!(calls.contains(&("/removed/secret".to_owned(), true, false)));
    let paths: std::collections::HashSet<_> = calls.iter().map(|(path, _, _)| path).collect();
    assert_eq!(
        paths.len(),
        calls.len(),
        "node filters must run once per original path"
    );
}

#[test]
fn subtree_filters_do_not_turn_excluded_missing_values_into_null() {
    let engine = DiffPatcher::new(DiffOptions {
        node_filter: Some(Arc::new(|path, _, _| {
            !path.ends_with("/secret") && path != "/array/0"
        })),
        ..Default::default()
    });
    for (left, right, expected) in [
        (
            json!({}),
            json!({"object":{"secret":null}}),
            json!({"object":{}}),
        ),
        (json!({}), json!({"array":[null,1]}), json!({"array":[1]})),
        (
            json!({"object":false}),
            json!({"object":{"secret":null,"value":1}}),
            json!({"object":{"value":1}}),
        ),
        (
            json!({"array":false}),
            json!({"array":[null,1]}),
            json!({"array":[1]}),
        ),
    ] {
        let delta = engine.diff(&left, &right).unwrap().unwrap();
        assert_round_trip(&left, &expected, &delta);
    }
}

#[test]
fn subtree_filters_allow_atomic_container_to_scalar_replacement() {
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = calls.clone();
    let engine = DiffPatcher::new(DiffOptions {
        node_filter: Some(Arc::new(move |path, _, _| {
            recorded.lock().unwrap().push(path.to_owned());
            !path.ends_with("/secret")
        })),
        ..Default::default()
    });
    let left = json!({"object":{"secret":"old"},"array":[{"secret":"old"}]});
    let right = json!({"object":null,"array":false});
    let delta = engine.diff(&left, &right).unwrap().unwrap();
    assert_round_trip(&left, &right, &delta);
    assert_eq!(calls.lock().unwrap().len(), 3);
}

#[test]
fn subtree_filters_keep_original_paths_after_array_compaction() {
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = calls.clone();
    let engine = DiffPatcher::new(DiffOptions {
        node_filter: Some(Arc::new(move |path, _, _| {
            recorded.lock().unwrap().push(path.to_owned());
            path != "/0" && path != "/1/secret"
        })),
        ..Default::default()
    });
    let left = json!([]);
    let right = json!([0,{"secret":1,"keep":2}]);
    let expected = json!([{"keep":2}]);
    let delta = engine.diff(&left, &right).unwrap().unwrap();
    assert_round_trip(&left, &expected, &delta);
    let calls = calls.lock().unwrap();
    assert!(calls.iter().any(|path| path == "/1/secret"));
    assert!(!calls.iter().any(|path| path == "/0/secret"));
    let unique: std::collections::HashSet<_> = calls.iter().collect();
    assert_eq!(unique.len(), calls.len());
}

#[test]
fn custom_array_matching_handles_changed_primitives_before_value_matching() {
    let engine = DiffPatcher::new(DiffOptions {
        array_item_matcher: Some(Arc::new(|path, left, right| {
            assert_eq!(path, "/items~1~0");
            left.as_i64()
                .zip(right.as_i64())
                .is_some_and(|(a, b)| a % 10 == b % 10)
        })),
        ..Default::default()
    });
    let left = json!({"items/~":[1,2,3]});
    let right = json!({"items/~":[13,11,12]});
    let delta = engine.diff(&left, &right).unwrap().unwrap();
    assert_eq!(native_moves(delta.as_value()), 1);
    assert_eq!(assert_round_trip(&left, &right, &delta), 1);
}

#[test]
fn custom_array_matching_resolves_ambiguous_pairs_without_remove_add() {
    let matcher = Arc::new(|_: &str, left: &Value, right: &Value| {
        matches!(
            (left.as_i64(), right.as_i64()),
            (Some(1), Some(3 | 4)) | (Some(2), Some(3))
        )
    });
    let left = json!([1, 2]);
    let right = json!([3, 4]);
    for moves in [true, false] {
        let engine = DiffPatcher::new(DiffOptions {
            array_item_matcher: Some(matcher.clone()),
            detect_moves: moves,
            ..Default::default()
        });
        let delta = engine.diff(&left, &right).unwrap().unwrap();
        assert_round_trip(&left, &right, &delta);
        if moves {
            let delta = delta.as_value().as_object().unwrap();
            assert!(
                delta
                    .values()
                    .filter_map(Value::as_array)
                    .all(|leaf| { leaf.len() == 2 || leaf.get(2) == Some(&json!(3)) })
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1024,
        rng_seed:proptest::test_runner::RngSeed::Fixed(20261001),
        .. ProptestConfig::default() })]
    #[test]
    fn mixed_add_remove_reorder_and_nested_changes(
        left in proptest::collection::btree_map(0u8..50,(any::<u16>(),any::<i16>()),0..30),
        right in proptest::collection::btree_map(0u8..50,(any::<u16>(),any::<i16>()),0..30),
        include in any::<bool>(), moves in any::<bool>()
    ) {
        let engine = DiffPatcher::new(DiffOptions {
            object_hash:Some(Arc::new(|item,_|item.get("id").map(Value::to_string))),
            include_value_on_move:include, detect_moves:moves, ..Default::default()
        });
        let (left,right) = (ordered_items(left),ordered_items(right));
        if let Some(delta) = engine.diff(&left,&right).unwrap() {
            let standard_moves = assert_round_trip(&left,&right,&delta);
            prop_assert!(standard_moves <= native_moves(delta.as_value()));
        } else {
            prop_assert_eq!(left,right);
        }
    }
    #[test]
    fn arbitrary_custom_matching_round_trips(
        left in proptest::collection::vec(-40i16..40,0..30),
        right in proptest::collection::vec(-40i16..40,0..30),
        moves in any::<bool>(), include in any::<bool>()
    ) {
        let engine = DiffPatcher::new(DiffOptions {
            array_item_matcher:Some(Arc::new(|_,left,right|
                left.as_i64().zip(right.as_i64()).is_some_and(|(a,b)|a % 7 == b % 7)
            )), detect_moves:moves, include_value_on_move:include,
            ..Default::default()
        });
        let (left,right) = (json!(left),json!(right));
        if let Some(delta) = engine.diff(&left,&right).unwrap() {
            prop_assert!(assert_round_trip(&left,&right,&delta) <= native_moves(delta.as_value()));
        } else {
            prop_assert_eq!(left,right);
        }
    }
    #[test]
    fn arbitrary_position_filters_match_the_explicit_projection(
        left in proptest::collection::vec(any::<i16>(),0..50),
        right in proptest::collection::vec(any::<i16>(),0..50),
        selected in proptest::collection::vec(any::<bool>(),0..60)
    ) {
        let projected:Vec<_> = (0..left.len().max(right.len())).filter_map(|i|
            if selected.get(i).copied().unwrap_or(true) { right.get(i) } else { left.get(i) }
        ).copied().collect();
        let engine = DiffPatcher::new(DiffOptions {
            node_filter:Some(Arc::new(move |path,_,_| {
                path.strip_prefix('/').and_then(|index|index.parse::<usize>().ok())
                    .is_none_or(|index|selected.get(index).copied().unwrap_or(true))
            })), ..Default::default()
        });
        let (left,right,projected) = (json!(left),json!(right),json!(projected));
        if let Some(delta) = engine.diff(&left,&right).unwrap() {
            assert_round_trip(&left,&projected,&delta);
        } else {
            prop_assert_eq!(left,projected);
        }
    }
}
