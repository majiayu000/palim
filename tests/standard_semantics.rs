use palim::{Delta, Patch, apply_json_patch, patch, reverse};
use serde_json::{Value, json};

fn standard(doc: &Value, operations: Value) -> Result<Value, palim::Error> {
    let operations: Patch = serde_json::from_value(operations).unwrap();
    apply_json_patch(doc, &operations)
}

#[test]
fn standard_test_uses_mathematical_recursive_numeric_equality() {
    for (doc, expected) in [
        (json!(1), json!(1.0)),
        (json!(-1), json!(-1.0)),
        (json!(0), json!(-0.0)),
        (json!({"x":[1,2]}), json!({"x":[1.0,2.0]})),
        (
            json!(9223372036854775808_u64),
            serde_json::from_str("9223372036854775808.0").unwrap(),
        ),
    ] {
        assert_eq!(
            standard(&doc, json!([{"op":"test","path":"","value":expected}])).unwrap(),
            doc
        );
    }
    for (doc, expected) in [
        (json!(true), json!(1)),
        (json!(9007199254740993_u64), json!(9007199254740992_f64)),
        (
            json!(18446744073709551615_u64),
            json!(18446744073709551616_f64),
        ),
        (
            json!(9223372036854775808_u64),
            json!(9223372036854775808_f64),
        ),
    ] {
        assert!(standard(&doc, json!([{"op":"test","path":"","value":expected}])).is_err());
    }
}

#[test]
fn root_move_to_itself_is_a_noop_and_missing_move_still_fails() {
    let doc = json!({"x":1});
    assert_eq!(
        standard(&doc, json!([{"op":"move","from":"","path":""}])).unwrap(),
        doc
    );
    assert!(
        standard(
            &doc,
            json!([{"op":"move","from":"/missing","path":"/missing"}])
        )
        .is_err()
    );
}

#[test]
fn standard_moves_and_copies_use_the_final_destination_container() {
    let doc = json!({"a/b":{"~key":{"leaf":1}},"items":[{"x":1},{}]});
    assert_eq!(
        standard(
            &doc,
            json!([
                {"op":"move","from":"/items/0","path":"/items/-"},
                {"op":"copy","from":"/a~1b/~0key","path":"/items/0/copied"},
                {"op":"move","from":"/a~1b/~0key","path":"/a~1b"}
            ])
        )
        .unwrap(),
        json!({"a/b":{"leaf":1},"items":[{"copied":{"leaf":1}},{"x":1}]})
    );
    assert_eq!(
        standard(&doc, json!([{"op":"move","from":"/a~1b/~0key","path":""}])).unwrap(),
        json!({"leaf":1})
    );
    assert_eq!(
        standard(&doc, json!([{"op":"copy","from":"","path":""}])).unwrap(),
        doc
    );
}

#[test]
fn instance_standard_diff_applies_only_the_filtered_projection() {
    use palim::{DiffOptions, DiffPatcher, JsonPatchOptions};
    use std::sync::Arc;
    let before = json!({"items":[1,2],"visible":{"x":1},"secret":{"x":1}});
    let after = json!({"items":[2,1],"visible":{"x":2},"secret":{"x":99},"hidden":3});
    let expected = json!({"items":[2,1],"visible":{"x":2},"secret":{"x":1}});
    for diff_options in [
        DiffOptions {
            property_filter: Some(Arc::new(|name, _, _, _| {
                name != "secret" && name != "hidden"
            })),
            ..Default::default()
        },
        DiffOptions {
            node_filter: Some(Arc::new(|path, _, _| {
                path != "/secret" && path != "/hidden"
            })),
            ..Default::default()
        },
    ] {
        let generator = DiffPatcher::new(diff_options);
        let operations = generator
            .diff_json_patch(&before, &after, &JsonPatchOptions::default())
            .unwrap();
        assert_eq!(apply_json_patch(&before, &operations).unwrap(), expected);
    }
}

#[test]
fn standard_test_error_keeps_operation_and_path() {
    let doc = json!({"x":1});
    let error = standard(
        &doc,
        json!([
            {"op":"replace","path":"/x","value":2},
            {"op":"test","path":"/x","value":3}
        ]),
    )
    .unwrap_err();
    assert_eq!(error.path, "/x");
    assert!(error.message.contains("'/1'"));
    assert_eq!(doc, json!({"x":1}));
}

#[test]
fn included_move_values_authenticate_and_remain_reversible_after_editing() {
    let invalid = Delta::from_value(json!({"_t":"a","_0":[999,1,3]})).unwrap();
    assert!(patch(&json!([1, 2]), &invalid).is_err());
    let before = json!([{ "id":1,"v":10 },{"id":2,"v":20}]);
    let after = json!([{ "id":2,"v":20 },{"id":1,"v":11}]);
    let delta = Delta::from_value(json!({
        "_t":"a", "_0":[{"id":1,"v":10},1,3], "1":{"v":[10,11]}
    }))
    .unwrap();
    assert_eq!(patch(&before, &delta).unwrap(), after);
    let inverse = reverse(&delta).unwrap();
    assert_eq!(patch(&after, &inverse).unwrap(), before);
    assert_eq!(reverse(&inverse).unwrap(), delta);
}
