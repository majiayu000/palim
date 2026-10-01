use palim::{
    Delta, JsonPatchApplyOptions, Patch, apply_json_patch_in_place, apply_json_patch_owned,
    apply_json_patch_with_options, patch_in_place, patch_owned,
};
use serde_json::json;

#[test]
fn owned_and_atomic_in_place_application_restore_documents() {
    let before = json!({"items":[1,2,3],"keep":{"nested":true}});
    let delta = Delta::from_value(json!({"items":{"_t":"a","_2":["",0,3]},"new":[1]})).unwrap();
    let after = json!({"items":[3,1,2],"keep":{"nested":true},"new":1});
    assert_eq!(patch_owned(before.clone(), &delta).unwrap(), after);
    let mut doc = before.clone();
    patch_in_place(&mut doc, &delta).unwrap();
    assert_eq!(doc, after);
    let bad = Delta::from_value(json!({"new":[99,100]})).unwrap();
    assert!(patch_in_place(&mut doc, &bad).is_err());
    assert_eq!(doc, after);
    let operations: Patch = serde_json::from_value(json!([
        {"op":"test","path":"/new","value":1.0},
        {"op":"replace","path":"/new","value":2}
    ]))
    .unwrap();
    assert_eq!(
        apply_json_patch_owned(after.clone(), &operations).unwrap()["new"],
        json!(2)
    );
    apply_json_patch_in_place(&mut doc, &operations).unwrap();
    assert_eq!(doc["new"], json!(2));
    let bad: Patch = serde_json::from_value(json!([
        {"op":"replace","path":"/new","value":3},
        {"op":"remove","path":"/missing"}
    ]))
    .unwrap();
    assert!(apply_json_patch_in_place(&mut doc, &bad).is_err());
    assert_eq!(doc["new"], json!(2));
}

#[test]
fn copy_budget_counts_cumulative_serialized_source_bytes_before_copying() {
    let doc = json!({"x":"12345"});
    let operations: Patch = serde_json::from_value(json!([
        {"op":"copy","from":"/x","path":"/a"},
        {"op":"copy","from":"/x","path":"/b"}
    ]))
    .unwrap();
    let options = JsonPatchApplyOptions {
        max_copy_bytes: Some(14),
        ..Default::default()
    };
    assert_eq!(
        apply_json_patch_with_options(&doc, &operations, &options).unwrap()["b"],
        json!("12345")
    );
    let options = JsonPatchApplyOptions {
        max_copy_bytes: Some(13),
        ..Default::default()
    };
    let error = apply_json_patch_with_options(&doc, &operations, &options).unwrap_err();
    assert!(error.message.contains("copy"));
    assert!(error.message.contains("'/1'"));
    assert_eq!(doc, json!({"x":"12345"}));
}

#[test]
fn fuzzy_application_reaches_nested_strings_and_keeps_other_edits_strict() {
    use palim::{DiffOptions, DiffPatcher, TextPatchOptions, patch, patch_fuzzy};
    let before = json!({"items":[{"text":"The quick brown fox", "version":1}]});
    let after = json!({"items":[{"text":"The quick red fox", "version":2}]});
    let delta = DiffPatcher::new(DiffOptions {
        text_diff_min_length: Some(1),
        ..Default::default()
    })
    .diff(&before, &after)
    .unwrap()
    .unwrap();
    let displaced = json!({"items":[{"text":"PREFIX The quack brown fox", "version":1}]});
    let options = TextPatchOptions {
        max_distance: 20,
        max_error_ratio: 0.2,
    };
    assert!(patch(&displaced, &delta).is_err());
    assert_eq!(
        patch_fuzzy(&displaced, &delta, &options).unwrap(),
        json!({"items":[{"text":"PREFIX The quack red fox", "version":2}]})
    );
    let wrong_version = json!({"items":[{"text":"PREFIX The quack brown fox", "version":99}]});
    assert!(patch_fuzzy(&wrong_version, &delta, &options).is_err());
    assert_eq!(wrong_version["items"][0]["version"], json!(99));
    let scalar = Delta::from_value(json!([1, 2])).unwrap();
    assert!(
        patch_fuzzy(
            &json!(1),
            &scalar,
            &TextPatchOptions {
                max_error_ratio: f64::NAN,
                ..Default::default()
            }
        )
        .is_err()
    );
}

#[test]
fn standard_depth_limits_validate_the_baseline_and_intermediate_results() {
    let doc = json!({"x":{"y":1}});
    let options = JsonPatchApplyOptions {
        max_depth: 1,
        ..Default::default()
    };
    assert!(apply_json_patch_with_options(&doc, &Patch::default(), &options).is_err());
    let add: Patch = serde_json::from_value(json!([
        {"op":"add","path":"/x","value":{"y":1}}
    ]))
    .unwrap();
    let error = apply_json_patch_with_options(&json!({}), &add, &options).unwrap_err();
    assert!(error.message.contains("'/0'"));
    assert_eq!(error.path, "/x");
    assert!(
        apply_json_patch_with_options(
            &json!({}),
            &Patch::default(),
            &JsonPatchApplyOptions {
                max_depth: 129,
                ..Default::default()
            }
        )
        .is_err()
    );
}

#[test]
fn standard_insertions_reject_combined_depth_at_the_failing_operation() {
    let doc = json!({"source":{"leaf":1},"target":{},"items":[]});
    let options = JsonPatchApplyOptions {
        max_depth: 2,
        ..Default::default()
    };
    for operation in [
        json!({"op":"add","path":"/target/new","value":{}}),
        json!({"op":"replace","path":"/source/leaf","value":{}}),
        json!({"op":"move","from":"/source","path":"/target/new"}),
        json!({"op":"copy","from":"/source","path":"/target/new"}),
        json!({"op":"copy","from":"/source","path":"/items/-"}),
        json!({"op":"copy","from":"","path":"/copy"}),
    ] {
        let failing_path = operation["path"].as_str().unwrap();
        let operations: Patch = serde_json::from_value(json!([
            {"op":"replace","path":"/source/leaf","value":2},
            operation
        ]))
        .unwrap();
        let error = apply_json_patch_with_options(&doc, &operations, &options).unwrap_err();
        assert_eq!(error.path, failing_path);
        assert!(error.message.contains("'/1'"), "{error}");
        assert!(error.message.contains("max_depth"), "{error}");
        assert_eq!(doc, json!({"source":{"leaf":1},"target":{},"items":[]}));
    }
}

#[test]
fn standard_path_errors_precede_combined_depth_errors() {
    let doc = json!({"source":{"leaf":1},"target":{},"items":[]});
    let options = JsonPatchApplyOptions {
        max_depth: 2,
        ..Default::default()
    };
    for (operation, message) in [
        (
            json!({"op":"add","path":"/target/missing/new","value":{}}),
            "path is invalid",
        ),
        (
            json!({"op":"replace","path":"/target/missing","value":{}}),
            "path is invalid",
        ),
        (
            json!({"op":"move","from":"/source","path":"/target/missing/new"}),
            "path is invalid",
        ),
        (
            json!({"op":"copy","from":"/source","path":"/target/missing/new"}),
            "path is invalid",
        ),
        (
            json!({"op":"move","from":"/missing","path":"/target/new"}),
            "\"from\" path is invalid",
        ),
        (
            json!({"op":"copy","from":"/missing","path":"/target/new"}),
            "\"from\" path is invalid",
        ),
        (
            json!({"op":"move","from":"/source","path":"/source/new"}),
            "cannot move the value inside itself",
        ),
        (
            json!({"op":"move","from":"","path":"/target/new"}),
            "cannot move the value inside itself",
        ),
        (
            json!({"op":"add","path":"/items/1","value":{}}),
            "path is invalid",
        ),
    ] {
        let expected_path = operation["path"].as_str().unwrap();
        let operations: Patch = serde_json::from_value(json!([
            {"op":"test","path":"/source/leaf","value":1},
            operation
        ]))
        .unwrap();
        let error = apply_json_patch_with_options(&doc, &operations, &options).unwrap_err();
        assert_eq!(error.path, expected_path);
        assert!(error.message.contains("'/1'"), "{error}");
        assert!(error.message.contains(message), "{error}");
        assert!(!error.message.contains("max_depth"), "{error}");
    }
}

#[test]
fn standard_resource_error_priority_remains_stable() {
    let operations: Patch = serde_json::from_value(json!([
        {"op":"add","path":"/missing/new","value":{"nested":{}}}
    ]))
    .unwrap();
    let error = apply_json_patch_with_options(
        &json!({}),
        &operations,
        &JsonPatchApplyOptions {
            max_depth: 1,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(error.message.contains("max_depth"), "{error}");
    let operations: Patch = serde_json::from_value(json!([
        {"op":"copy","from":"/source","path":"/missing/new"}
    ]))
    .unwrap();
    let error = apply_json_patch_with_options(
        &json!({"source":"12345"}),
        &operations,
        &JsonPatchApplyOptions {
            max_copy_bytes: Some(0),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(error.message.contains("copy byte budget"), "{error}");
    assert_eq!(error.path, "/missing/new");
}
