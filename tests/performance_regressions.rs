use palim::{
    DiffOptions, DiffPatcher, JsonPatchOptions, Patch, PatchOperation, apply_json_patch,
    invert_json_patch,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

fn plain(tests: bool) -> JsonPatchOptions {
    JsonPatchOptions {
        factorize: false,
        rationalize: false,
        tests,
    }
}

fn roundtrip(left: &Value, right: &Value, patch: &Patch) {
    let decoded: Patch = serde_json::from_slice(&serde_json::to_vec(patch).unwrap()).unwrap();
    let mut independent = left.clone();
    json_patch::patch(&mut independent, &decoded).unwrap();
    assert_eq!(&independent, right);
    assert_eq!(apply_json_patch(left, &decoded).unwrap(), *right);
    let inverse = invert_json_patch(left, &decoded).unwrap();
    json_patch::patch(&mut independent, &inverse).unwrap();
    assert_eq!(&independent, left);
}

#[test]
fn disjoint_arrays_replace_overlap_and_handle_both_tail_directions() {
    let dp = DiffPatcher::default();
    for old_len in 0..9 {
        for new_len in 0..9 {
            let old = json!((0..old_len).collect::<Vec<_>>());
            let new = json!((100..100 + new_len).collect::<Vec<_>>());
            for (left, right) in [
                (old.clone(), new.clone()),
                (json!({"a/b~": old}), json!({"a/b~": new})),
            ] {
                let patch = dp.diff_json_patch(&left, &right, &plain(false)).unwrap();
                assert_eq!(patch.0.len(), old_len.max(new_len));
                assert_eq!(
                    patch
                        .0
                        .iter()
                        .filter(|op| matches!(op, PatchOperation::Replace(_)))
                        .count(),
                    old_len.min(new_len)
                );
                roundtrip(&left, &right, &patch);
                if let Some(delta) = dp.diff(&left, &right).unwrap() {
                    let exported = delta.to_json_patch(&left).unwrap();
                    assert_eq!(exported, patch);
                    roundtrip(&left, &right, &exported);
                }
            }
        }
    }
}

#[test]
fn plain_guarded_disjoint_arrays_use_one_structural_test() {
    let dp = DiffPatcher::default();
    for (old_len, new_len) in [(0, 0), (0, 512), (512, 0), (512, 512), (512, 768)] {
        let left = json!((0..old_len).collect::<Vec<_>>());
        let right = json!((10_000..10_000 + new_len).collect::<Vec<_>>());
        let guarded = dp.diff_json_patch(&left, &right, &plain(true)).unwrap();
        if old_len == 0 && new_len == 0 {
            assert!(guarded.0.is_empty());
            continue;
        }
        assert_eq!(guarded.0.len(), old_len.max(new_len) + 1);
        assert!(matches!(&guarded.0[0], PatchOperation::Test(test)
            if test.path.as_str().is_empty() && test.value == left));
        assert_eq!(
            guarded
                .0
                .iter()
                .filter(|op| matches!(op, PatchOperation::Replace(_)))
                .count(),
            old_len.min(new_len)
        );
        assert!(serde_json::to_vec(&guarded).unwrap().len() < old_len.max(new_len) * 100 + 100);
        roundtrip(&left, &right, &guarded);
        let mut drift = left.clone();
        drift.as_array_mut().unwrap().push(json!("unexpected"));
        assert!(apply_json_patch(&drift, &guarded).is_err());
    }
}

#[test]
fn repeated_disjoint_payloads_still_factorize_and_guard_container_drift() {
    let left = json!(["old-a", "old-b"]);
    let right = json!(["全新/文本~".repeat(100), "全新/文本~".repeat(100)]);
    let dp = DiffPatcher::default();
    for tests in [false, true] {
        let options = JsonPatchOptions {
            factorize: true,
            rationalize: false,
            tests,
        };
        let patch = dp.diff_json_patch(&left, &right, &options).unwrap();
        assert!(
            patch
                .0
                .iter()
                .any(|op| matches!(op, PatchOperation::Copy(_)))
        );
        // Frozen from be76218: later repeated additions copy the first newly
        // inserted value, rather than an unavailable value from the baseline.
        let mutations: Vec<_> = patch
            .0
            .iter()
            .filter(|operation| !matches!(operation, PatchOperation::Test(_)))
            .collect();
        assert_eq!(
            serde_json::to_value(mutations).unwrap(),
            json!([
                {"op": "remove", "path": "/1"},
                {"op": "remove", "path": "/0"},
                {"op": "add", "path": "/0", "value": right[0]},
                {"op": "copy", "from": "/0", "path": "/1"},
            ])
        );
        roundtrip(&left, &right, &patch);
    }
    let guarded = dp.diff_json_patch(&left, &right, &plain(true)).unwrap();
    roundtrip(&left, &right, &guarded);
    assert!(apply_json_patch(&json!(["old-a", "old-b", "unexpected"]), &guarded).is_err());
}

#[test]
fn scalar_shortcut_preserves_property_and_node_filter_callbacks() {
    let nodes = Arc::new(Mutex::new(Vec::new()));
    let properties = Arc::new(Mutex::new(Vec::new()));
    let recorded_nodes = nodes.clone();
    let recorded_properties = properties.clone();
    let left = json!({"bool": true, "null": null, "num": 42, "skip": 1, "text": "same", "z": 0});
    let right = json!({"bool": true, "null": null, "num": 42, "skip": 2, "text": "same", "z": 1});
    let dp = DiffPatcher::new(DiffOptions {
        property_filter: Some(Arc::new(move |name, _, _, path| {
            recorded_properties
                .lock()
                .unwrap()
                .push((path.to_owned(), name.to_owned()));
            name != "skip"
        })),
        node_filter: Some(Arc::new(move |path, _, _| {
            recorded_nodes.lock().unwrap().push(path.to_owned());
            true
        })),
        ..Default::default()
    });
    let patch = dp.diff_json_patch(&left, &right, &plain(false)).unwrap();
    roundtrip(
        &left,
        &json!({"bool": true, "null": null, "num": 42, "skip": 1, "text": "same", "z": 1}),
        &patch,
    );
    assert_eq!(
        *nodes.lock().unwrap(),
        ["", "/bool", "/null", "/num", "/text", "/z"]
    );
    assert_eq!(
        properties
            .lock()
            .unwrap()
            .iter()
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>(),
        ["bool", "null", "num", "skip", "text", "z"]
    );
    // With no node filter, unchanged scalar fields take the fast path, but
    // their property predicates must still run before they are skipped.
    properties.lock().unwrap().clear();
    let recorded = properties.clone();
    let dp = DiffPatcher::new(DiffOptions {
        property_filter: Some(Arc::new(move |name, _, _, _| {
            recorded
                .lock()
                .unwrap()
                .push((String::new(), name.to_owned()));
            name != "skip" && name != "z"
        })),
        ..Default::default()
    });
    assert!(dp.diff(&left, &right).unwrap().is_none());
    assert_eq!(
        properties
            .lock()
            .unwrap()
            .iter()
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>(),
        ["bool", "null", "num", "skip", "text", "z"]
    );
}

#[test]
fn disjoint_array_shortcut_keeps_identity_and_projection_semantics() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let recorded = calls.clone();
    let dp = DiffPatcher::new(DiffOptions {
        object_hash: Some(Arc::new(move |value, index| {
            recorded.lock().unwrap().push((value["id"].clone(), index));
            value.get("id").map(Value::to_string)
        })),
        ..Default::default()
    });
    let left = json!([{"id": 0}, {"id": 1}]);
    let right = json!([{"id": 10}, {"id": 11}, {"id": 12}]);
    let patch = dp.diff_json_patch(&left, &right, &plain(false)).unwrap();
    assert_eq!(patch.0.len(), 3);
    assert_eq!(calls.lock().unwrap().len(), 5);
    roundtrip(&left, &right, &patch);

    let filtered = DiffPatcher::new(DiffOptions {
        node_filter: Some(Arc::new(|path, _, _| path != "/0")),
        ..Default::default()
    });
    let left = json!([0, 1]);
    let right = json!([10, 11, 12]);
    let patch = filtered
        .diff_json_patch(&left, &right, &plain(false))
        .unwrap();
    roundtrip(&left, &json!([0, 11, 12]), &patch);
}

#[test]
fn depth_checks_keep_scalar_and_container_boundaries_before_filtering() {
    let dp = DiffPatcher::new(DiffOptions {
        max_depth: 0,
        node_filter: Some(Arc::new(|_, _, _| false)),
        ..Default::default()
    });
    assert!(dp.diff(&json!(1), &json!(2)).unwrap().is_none());
    let error = dp.diff(&json!([]), &json!([])).unwrap_err();
    assert_eq!(error.path, "");
    assert_eq!(error.message, "JSON nesting exceeds max_depth");

    let dp = DiffPatcher::new(DiffOptions {
        max_depth: 1,
        ..Default::default()
    });
    let flat = json!((0..10_000).collect::<Vec<_>>());
    assert!(dp.diff(&flat, &flat).unwrap().is_none());
    for nested in [json!([{}]), json!({"a": []})] {
        let error = dp.diff(&nested, &nested).unwrap_err();
        assert_eq!(error.path, "");
        assert_eq!(error.message, "JSON nesting exceeds max_depth");
    }
}

#[test]
fn root_array_fast_path_keeps_custom_pairing_and_option_errors() {
    let calls = Arc::new(Mutex::new(0));
    let recorded = calls.clone();
    let dp = DiffPatcher::new(DiffOptions {
        array_item_matcher: Some(Arc::new(move |_, old, new| {
            *recorded.lock().unwrap() += 1;
            old.as_i64().unwrap() + 100 == new.as_i64().unwrap()
        })),
        ..Default::default()
    });
    let left = json!([0, 1]);
    let right = json!([101, 100]);
    let patch = dp.diff_json_patch(&left, &right, &plain(false)).unwrap();
    assert_eq!(*calls.lock().unwrap(), 4);
    assert!(
        patch
            .0
            .iter()
            .any(|op| matches!(op, PatchOperation::Move(_)))
    );
    roundtrip(&left, &right, &patch);

    for options in [
        DiffOptions {
            max_depth: 129,
            ..Default::default()
        },
        DiffOptions {
            max_depth: 0,
            ..Default::default()
        },
        DiffOptions {
            max_depth: 129,
            object_hash: Some(Arc::new(|_, _| None)),
            array_item_matcher: Some(Arc::new(|_, _, _| false)),
            ..Default::default()
        },
    ] {
        let dp = DiffPatcher::new(options);
        let native = dp.diff(&left, &right).unwrap_err();
        assert_eq!(
            dp.diff_json_patch(&left, &right, &plain(false))
                .unwrap_err(),
            native
        );
    }
}

#[test]
fn dense_periodic_arrays_keep_a_single_move_and_roundtrip() {
    for (size, period) in [(20_000, 8), (20_001, 32)] {
        let left = json!((0..size).map(|i| i % period).collect::<Vec<_>>());
        let mut right = left.clone();
        right.as_array_mut().unwrap().rotate_left(1);
        let dp = DiffPatcher::default();
        let delta = dp.diff(&left, &right).unwrap().unwrap();
        assert_eq!(dp.patch(&left, &delta).unwrap(), right);
        assert_eq!(dp.unpatch(&right, &delta).unwrap(), left);
        assert_eq!(
            dp.patch(&right, &dp.reverse(&delta).unwrap()).unwrap(),
            left
        );
        let patch = dp.diff_json_patch(&left, &right, &plain(false)).unwrap();
        assert_eq!(patch.0.len(), 1);
        assert!(matches!(&patch.0[0], PatchOperation::Move(_)));
        roundtrip(&left, &right, &patch);
    }
}

#[test]
fn custom_matching_reuses_scratch_without_changing_callbacks_or_pairing() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let recorded = calls.clone();
    let dp = DiffPatcher::new(DiffOptions {
        array_item_matcher: Some(Arc::new(move |path, old, new| {
            recorded
                .lock()
                .unwrap()
                .push((path.to_owned(), old.clone(), new.clone()));
            matches!(
                (old.as_i64(), new.as_i64()),
                (Some(0..=2), Some(10)) | (Some(1..=2), Some(11)) | (Some(0), Some(12))
            )
        })),
        ..Default::default()
    });
    let left = json!([0, 1, 2]);
    let right = json!([10, 11, 12]);
    let delta = dp.diff(&left, &right).unwrap().unwrap();
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .map(|(_, old, new)| (old.as_i64().unwrap(), new.as_i64().unwrap()))
            .collect::<Vec<_>>(),
        [
            (0, 10),
            (1, 10),
            (2, 10),
            (0, 11),
            (1, 11),
            (2, 11),
            (0, 12),
            (1, 12),
            (2, 12)
        ]
    );
    assert_eq!(
        delta.as_value(),
        &json!({
            "_t":"a", "_1":["",1,3], "_2":["",0,3],
            "0":[2,10], "1":[1,11], "2":[0,12],
        })
    );
    assert_eq!(dp.patch(&left, &delta).unwrap(), right);
    assert_eq!(dp.unpatch(&right, &delta).unwrap(), left);

    calls.lock().unwrap().clear();
    let left = json!([]);
    let right = json!((0..8_000).collect::<Vec<_>>());
    let delta = dp.diff(&left, &right).unwrap().unwrap();
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(dp.patch(&left, &delta).unwrap(), right);
    assert_eq!(dp.unpatch(&right, &delta).unwrap(), left);
}

#[test]
fn single_root_copy_remains_profitable_and_guards_the_baseline() {
    let payload = "shared payload 🦀 ".repeat(100);
    let left = json!({"payload":payload,"keep":0});
    let right = json!(payload);
    for tests in [false, true] {
        let patch = DiffPatcher::default()
            .diff_json_patch(
                &left,
                &right,
                &JsonPatchOptions {
                    tests,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(patch.0.iter().any(|op| matches!(op,PatchOperation::Copy(op)
            if op.from.as_str()=="/payload" && op.path.as_str().is_empty())));
        roundtrip(&left, &right, &patch);
        if tests {
            assert!(apply_json_patch(&json!({"payload":"drift","keep":0}), &patch).is_err());
        }
    }
}

#[test]
fn default_disjoint_shortcut_preserves_configured_identity_callbacks() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let recorded = calls.clone();
    let dp = DiffPatcher::new(DiffOptions {
        object_hash: Some(Arc::new(move |value, index| {
            recorded.lock().unwrap().push((value.clone(), index));
            value.get("id").map(Value::to_string)
        })),
        ..Default::default()
    });
    for (left, right, expected_calls) in [
        (json!([]), json!([true, 12, "new"]), 0),
        (json!([false, 11, "old"]), json!([]), 0),
        (json!([false, 11, "old"]), json!([true, 12, "new"]), 0),
        (json!([{"id": 1}]), json!([{"id": 2}]), 2),
        (json!([0, {"id": 1}]), json!([10, {"id": 2}]), 2),
    ] {
        calls.lock().unwrap().clear();
        let delta = dp.diff(&left, &right).unwrap().unwrap();
        let native_calls = calls.lock().unwrap().clone();
        assert_eq!(native_calls.len(), expected_calls);
        assert_eq!(dp.patch(&left, &delta).unwrap(), right);
        calls.lock().unwrap().clear();
        let patch = dp
            .diff_json_patch(&left, &right, &JsonPatchOptions::default())
            .unwrap();
        assert_eq!(*calls.lock().unwrap(), native_calls);
        roundtrip(&left, &right, &patch);
    }
}

#[test]
fn rationalization_keeps_multiple_escaped_parent_replacements() {
    let left = json!({
        "a/b~": {"a_long_property_name": 1, "b_long_property_name": 2},
        "b~c": {"a_long_property_name": 3, "b_long_property_name": 4},
        "b~c/child": {"a_long_property_name": 5, "b_long_property_name": 6},
        "keep": "unchanged 🦀 ".repeat(1000),
    });
    let right = json!({
        "a/b~": {"a_long_property_name": 10, "b_long_property_name": 20},
        "b~c": {"a_long_property_name": 30, "b_long_property_name": 40},
        "b~c/child": {"a_long_property_name": 50, "b_long_property_name": 60},
        "keep": "unchanged 🦀 ".repeat(1000),
    });
    // Frozen be76218 output. Accepting the first replacement shortens the
    // operation list; both later parents must still be compressed correctly.
    // The slash in the third key is literal, not a descendant of the second.
    let expected = json!([
        {"op": "replace", "path": "/a~1b~0", "value": right["a/b~"]},
        {"op": "replace", "path": "/b~0c", "value": right["b~c"]},
        {"op": "replace", "path": "/b~0c~1child", "value": right["b~c/child"]},
    ]);
    for tests in [false, true] {
        let patch = DiffPatcher::default()
            .diff_json_patch(
                &left,
                &right,
                &JsonPatchOptions {
                    tests,
                    ..Default::default()
                },
            )
            .unwrap();
        let mutations: Vec<_> = patch
            .0
            .iter()
            .filter(|operation| !matches!(operation, PatchOperation::Test(_)))
            .collect();
        assert_eq!(serde_json::to_value(mutations).unwrap(), expected);
        assert_eq!(patch.0.len(), if tests { 6 } else { 3 });
        roundtrip(&left, &right, &patch);
        if tests {
            let mut drifted = left.clone();
            drifted["b~c/child"]["a_long_property_name"] = json!(99);
            assert!(apply_json_patch(&drifted, &patch).is_err());
        }
    }
}

#[test]
fn rationalization_keeps_escaped_move_boundaries_after_other_parent_replacements() {
    let payload = "move payload 🦀 ".repeat(50);
    let left = json!({
        "a/b~": {"a_long_property_name": 1, "b_long_property_name": 2, "leaving": payload},
        "b~c": {"a_long_property_name": 3, "b_long_property_name": 4},
        "c~d": {"a_long_property_name": 5, "b_long_property_name": 6},
        "destination": {},
        "keep": "unchanged 🦀 ".repeat(1000),
    });
    let right = json!({
        "a/b~": {"a_long_property_name": 10, "b_long_property_name": 20},
        "b~c": {"a_long_property_name": 30, "b_long_property_name": 40},
        "c~d": {"a_long_property_name": 50, "b_long_property_name": 60},
        "destination": {"leaving": payload},
        "keep": "unchanged 🦀 ".repeat(1000),
    });
    // Frozen be76218 output: replacing the move's source parent would delete
    // its source before the move, while the two independent parents compress.
    let expected = json!([
        {"op": "replace", "path": "/a~1b~0/a_long_property_name", "value": 10},
        {"op": "replace", "path": "/a~1b~0/b_long_property_name", "value": 20},
        {"op": "replace", "path": "/b~0c", "value": right["b~c"]},
        {"op": "replace", "path": "/c~0d", "value": right["c~d"]},
        {"op": "move", "from": "/a~1b~0/leaving", "path": "/destination/leaving"},
    ]);
    for tests in [false, true] {
        let patch = DiffPatcher::default()
            .diff_json_patch(
                &left,
                &right,
                &JsonPatchOptions {
                    tests,
                    ..Default::default()
                },
            )
            .unwrap();
        let mutations: Vec<_> = patch
            .0
            .iter()
            .filter(|operation| !matches!(operation, PatchOperation::Test(_)))
            .collect();
        assert_eq!(serde_json::to_value(mutations).unwrap(), expected);
        roundtrip(&left, &right, &patch);
        if tests {
            let mut drifted = left.clone();
            drifted["a/b~"]["leaving"] = json!("changed source");
            assert!(apply_json_patch(&drifted, &patch).is_err());
        }
    }
}

#[test]
fn independently_compressible_array_objects_keep_the_same_guarded_root_patch() {
    let left = json!(
        (0..24)
            .map(|id| json!({
                "id": id,
                "a_long_property_name": id,
                "b_long_property_name": id,
                "c_long_property_name": id,
            }))
            .collect::<Vec<_>>()
    );
    let right = json!(
        (0..24)
            .map(|id| json!({
                "id": id,
                "a_long_property_name": id + 100_000,
                "b_long_property_name": id + 200_000,
                "c_long_property_name": id + 300_000,
            }))
            .collect::<Vec<_>>()
    );
    for tests in [false, true] {
        let patch = DiffPatcher::default()
            .diff_json_patch(
                &left,
                &right,
                &JsonPatchOptions {
                    tests,
                    ..Default::default()
                },
            )
            .unwrap();
        let expected = if tests {
            json!([
                {"op": "test", "path": "", "value": left},
                {"op": "replace", "path": "", "value": right},
            ])
        } else {
            json!([{ "op": "replace", "path": "", "value": right }])
        };
        assert_eq!(serde_json::to_value(&patch).unwrap(), expected);
        roundtrip(&left, &right, &patch);
        if tests {
            let mut drifted = left.clone();
            drifted[12]["a_long_property_name"] = json!(-1);
            assert!(apply_json_patch(&drifted, &patch).is_err());
        }
    }
}
