use palim::{
    Patch, apply_json_patch, invert_json_patch, invert_json_patch_guarded, test_json_patch,
};
use serde_json::{Value, json};

fn parse(value: Value) -> Patch {
    serde_json::from_str(&value.to_string()).unwrap()
}

fn nested(depth: usize) -> Value {
    (0..depth).fold(json!(0), |value, _| json!([value]))
}

#[test]
fn read_only_tests_keep_recursive_number_equality_and_escaped_paths() {
    let doc = json!({"a/b":{"~key":[1,2]},"null":null});
    let tests = parse(json!([
        {"op":"test","path":"/a~1b/~0key","value":[1.0,2.0]},
        {"op":"test","path":"/null","value":null},
        {"op":"test","path":"","value":doc}
    ]));
    test_json_patch(&doc, &tests).unwrap();
    assert_eq!(apply_json_patch(&doc, &tests).unwrap(), doc);
    test_json_patch(&doc, &Patch::default()).unwrap();
    let huge: Value = serde_json::from_str("{\"n\":1e10000}").unwrap();
    let tests: Patch =
        serde_json::from_str("[{\"op\":\"test\",\"path\":\"/n\",\"value\":10e9999}]").unwrap();
    test_json_patch(&huge, &tests).unwrap();
}

#[test]
fn read_only_failures_keep_standard_order_and_error_details() {
    let doc = json!({"x":1});
    for operations in [
        json!([
            {"op":"test","path":"/x","value":1},
            {"op":"test","path":"/x","value":2}
        ]),
        json!([{"op":"test","path":"/missing","value":1}]),
        json!([
            {"op":"test","path":"/x","value":2},
            {"op":"replace","path":"/x","value":3}
        ]),
    ] {
        let operations = parse(operations);
        assert_eq!(
            test_json_patch(&doc, &operations).unwrap_err(),
            apply_json_patch(&doc, &operations).unwrap_err()
        );
    }
    let deep_payload = Patch(vec![palim::PatchOperation::Test(
        json_patch::TestOperation {
            path: json_patch::jsonptr::PointerBuf::parse("/missing").unwrap(),
            value: nested(129),
        },
    )]);
    assert_eq!(
        test_json_patch(&doc, &deep_payload).unwrap_err(),
        apply_json_patch(&doc, &deep_payload).unwrap_err()
    );
    test_json_patch(&nested(128), &Patch::default()).unwrap();
    let too_deep = nested(129);
    assert_eq!(
        test_json_patch(&too_deep, &Patch::default()).unwrap_err(),
        apply_json_patch(&too_deep, &Patch::default()).unwrap_err()
    );
    assert_eq!(doc, json!({"x":1}));
}

#[test]
fn read_only_tests_reject_every_mutating_operation_at_its_position() {
    let doc = json!({"x":1});
    for operation in [
        json!({"op":"add","path":"/x","value":2}),
        json!({"op":"remove","path":"/x"}),
        json!({"op":"replace","path":"/x","value":2}),
        json!({"op":"move","from":"/x","path":"/y"}),
        json!({"op":"copy","from":"/x","path":"/y"}),
    ] {
        let operations = parse(json!([
            {"op":"test","path":"/x","value":1},
            operation
        ]));
        let error = test_json_patch(&doc, &operations).unwrap_err();
        assert!(error.message.contains("'/1'"));
        assert!(error.message.contains("only test operations"));
        assert_eq!(error.path, operation["path"]);
    }
    assert_eq!(doc, json!({"x":1}));
}

#[test]
fn guarded_inverse_restores_overwrites_arrays_roots_and_all_six_operations() {
    for (before, operations) in [
        (
            json!({"a":{"n":1},"b":2,"arr":[0,1,2]}),
            json!([
                {"op":"test","path":"/b","value":2},
                {"op":"add","path":"/new","value":{"nested":true}},
                {"op":"replace","path":"/b","value":3},
                {"op":"copy","from":"/a","path":"/new"},
                {"op":"move","from":"/arr/0","path":"/arr/-"},
                {"op":"remove","path":"/a"}
            ]),
        ),
        (
            json!({"a":{"n":1},"b":2}),
            json!([{"op":"move","from":"/a/n","path":"/a"}]),
        ),
        (
            json!({"arr":[5,{"x":10},{"x":20}]}),
            json!([{"op":"move","from":"/arr/0","path":"/arr/1/x"}]),
        ),
        (
            json!({"a":{"n":1},"b":2}),
            json!([{"op":"copy","from":"/a","path":""}]),
        ),
        (
            json!({"a/b":[0,{"x/y":[8]},{"x/y":[1,2]}]}),
            json!([{"op":"move","from":"/a~1b/0","path":"/a~1b/1/x~1y/-"}]),
        ),
    ] {
        let operations = parse(operations);
        let after = apply_json_patch(&before, &operations).unwrap();
        let inverse = invert_json_patch_guarded(&before, &operations).unwrap();
        assert_eq!(apply_json_patch(&after, &inverse).unwrap(), before);
        let mut independent = after;
        json_patch::patch(&mut independent, &inverse).unwrap();
        assert_eq!(independent, before);
        assert!(
            inverse
                .0
                .iter()
                .any(|op| matches!(op, palim::PatchOperation::Test(_)))
        );
    }
}

#[test]
fn guarded_inverse_rejects_the_reported_wrong_array_baseline() {
    let before = json!([{"code":"1"},{"code":"2"}]);
    let operations = parse(json!([{"op":"add","path":"/0","value":{"code":"3"}}]));
    let after = apply_json_patch(&before, &operations).unwrap();
    let inverse = invert_json_patch_guarded(&before, &operations).unwrap();
    assert_eq!(apply_json_patch(&after, &inverse).unwrap(), before);
    let drifted = json!([{"code":"1"},{"code":"2"},{"code":"3"}]);
    assert!(apply_json_patch(&drifted, &inverse).is_err());
    let unguarded = invert_json_patch(&before, &operations).unwrap();
    assert_eq!(
        apply_json_patch(&drifted, &unguarded).unwrap(),
        json!([{"code":"2"},{"code":"3"}])
    );
}

#[test]
fn inverse_guards_distinguish_touched_values_from_parent_snapshots() {
    let before = json!({"x":1,"other":0});
    let replace = parse(json!([{"op":"replace","path":"/x","value":2}]));
    let inverse = invert_json_patch_guarded(&before, &replace).unwrap();
    assert_eq!(
        apply_json_patch(&json!({"x":2,"other":9}), &inverse).unwrap(),
        json!({"x":1,"other":9})
    );
    assert!(apply_json_patch(&json!({"x":3,"other":0}), &inverse).is_err());
    let remove = parse(json!([{"op":"remove","path":"/x"}]));
    let inverse = invert_json_patch_guarded(&before, &remove).unwrap();
    assert_eq!(
        apply_json_patch(&json!({"other":0}), &inverse).unwrap(),
        before
    );
    assert!(apply_json_patch(&json!({"other":9}), &inverse).is_err());
}

#[test]
fn guarded_inverse_keeps_original_errors_and_test_only_noops() {
    let before = json!({"x":1});
    let invalid = parse(json!([
        {"op":"replace","path":"/x","value":2},
        {"op":"remove","path":"/missing"}
    ]));
    assert_eq!(
        invert_json_patch_guarded(&before, &invalid).unwrap_err(),
        apply_json_patch(&before, &invalid).unwrap_err()
    );
    let tests = parse(json!([{"op":"test","path":"/x","value":1.0}]));
    assert_eq!(
        invert_json_patch_guarded(&before, &tests).unwrap(),
        Patch::default()
    );
    assert_eq!(before, json!({"x":1}));
}

#[test]
fn standard_text_generation_preserves_thresholds_filters_and_callback_order() {
    use palim::{DiffOptions, DiffPatcher, JsonPatchOptions};
    use std::sync::{Arc, Mutex};
    let text = "long Unicode 🦀 text\n".repeat(100);
    let changed = text.replacen("text", "edited", 1);
    for (before, after) in [
        (json!(text), json!(changed)),
        (
            json!({"visible":text,"hidden":"keep"}),
            json!({"visible":changed,"hidden":"excluded change"}),
        ),
        (
            json!({"items":[{"id":1,"text":text},{"id":2,"text":"same"}]}),
            json!({"items":[{"id":2,"text":"same"},{"id":1,"text":changed}]}),
        ),
    ] {
        for threshold in [Some(0), Some(60), None, Some(usize::MAX)] {
            let calls = Arc::new(Mutex::new(Vec::new()));
            let nodes = calls.clone();
            let properties = calls.clone();
            let engine = DiffPatcher::new(DiffOptions {
                text_diff_min_length: threshold,
                object_hash: Some(Arc::new(|value, _| value.get("id").map(Value::to_string))),
                node_filter: Some(Arc::new(move |path, _, _| {
                    nodes.lock().unwrap().push(format!("node:{path}"));
                    path != "/hidden"
                })),
                property_filter: Some(Arc::new(move |key, _, _, path| {
                    properties
                        .lock()
                        .unwrap()
                        .push(format!("property:{path}:{key}"));
                    true
                })),
                ..Default::default()
            });
            let native = engine.diff(&before, &after).unwrap().unwrap();
            let target = engine.patch(&before, &native).unwrap();
            let native_calls = calls.lock().unwrap().clone();
            for factorize in [false, true] {
                for rationalize in [false, true] {
                    for tests in [false, true] {
                        calls.lock().unwrap().clear();
                        let standard = engine
                            .diff_json_patch(
                                &before,
                                &after,
                                &JsonPatchOptions {
                                    factorize,
                                    rationalize,
                                    tests,
                                },
                            )
                            .unwrap();
                        assert_eq!(*calls.lock().unwrap(), native_calls);
                        let mut independent = before.clone();
                        json_patch::patch(&mut independent, &standard).unwrap();
                        assert_eq!(independent, target);
                        let inverse = invert_json_patch(&before, &standard).unwrap();
                        assert_eq!(apply_json_patch(&target, &inverse).unwrap(), before);
                    }
                }
            }
        }
    }
}
