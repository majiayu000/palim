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
