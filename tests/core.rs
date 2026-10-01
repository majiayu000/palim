use palim::{Delta, DiffOptions, DiffPatcher, apply_json_patch, diff, patch, reverse, unpatch};
use serde_json::{Value, json};
use std::sync::Arc;

fn check(patcher: &DiffPatcher, left: &Value, right: &Value) {
    let Some(delta) = patcher.diff(left, right).unwrap() else {
        assert_eq!(left, right);
        return;
    };
    assert_eq!(patch(left, &delta).unwrap(), *right, "forward: {delta:?}");
    assert_eq!(unpatch(right, &delta).unwrap(), *left, "undo: {delta:?}");
    let reversed = reverse(&delta).unwrap();
    assert_eq!(patch(right, &reversed).unwrap(), *left);
    assert_eq!(reverse(&reversed).unwrap(), delta);
    let wire = serde_json::to_string(&delta).unwrap();
    let decoded: Delta = serde_json::from_str(&wire).unwrap();
    assert_eq!(decoded, delta);
    assert_eq!(
        apply_json_patch(left, &delta.to_json_patch(left).unwrap()).unwrap(),
        *right
    );
}

#[test]
fn structural_round_trips() {
    let dp = DiffPatcher::default();
    let pairs = [
        (json!(null), json!([1, 2])),
        (json!(false), json!(true)),
        (
            json!({"a/b":1,"~key":{"x":true}, "gone": null}),
            json!({"a/b":2,"~key":{"x":false}, "new":[]}),
        ),
        (json!([0, 1, 2, 3, 4]), json!([4, 0, 1, 2, 3])),
        (json!([0, 1, 2, 3, 4]), json!([3, 1, 9, 4, 0])),
        (json!([1, 1, 2, 3, 2]), json!([2, 1, 3, 1, 2, 2])),
        (json!([]), json!([null, {}])),
        (json!([1, 2]), json!([])),
        (json!({"_t":"a", "":1}), json!({"_t":"b", "":2})),
        (json!(9007199254740993_u64), json!(18446744073709551615_u64)),
        (json!(1.25), json!(2.5)),
        (json!([[398043487186.00244]]), json!(null)),
    ];
    for (a, b) in pairs {
        check(&dp, &a, &b);
    }
    assert!(diff(&json!({}), &json!({})).unwrap().is_none());
}

#[test]
fn identity_moves_and_multiple_instances() {
    let opts = DiffOptions {
        object_hash: Some(Arc::new(|v, _| v.get("id").map(Value::to_string))),
        ..Default::default()
    };
    let a = json!([{ "id":1,"v":10 },{"id":2,"v":20},{"id":3,"v":30},{"id":4,"v":40}]);
    let b = json!([{ "id":4,"v":41 },{"id":2,"v":22},{"id":1,"v":11},{"id":9,"v":90}]);
    for include in [false, true] {
        for moves in [false, true] {
            let dp = DiffPatcher::new(DiffOptions {
                include_value_on_move: include,
                detect_moves: moves,
                ..opts.clone()
            });
            check(&dp, &a, &b);
        }
    }
    check(
        &DiffPatcher::new(opts),
        &json!([{ "id":1,"v":1 },{"id":1,"v":2}]),
        &json!([{ "id":1,"v":3 },{"id":1,"v":1}]),
    );
}

#[test]
fn unicode_long_text_and_multi_hunks() {
    let dp = DiffPatcher::new(DiffOptions {
        text_diff_min_length: Some(1),
        ..Default::default()
    });
    for (a, b) in [
        (
            "你好🦀 abc % \n end".to_owned(),
            "你好🦀 XYZ % \n end".to_owned(),
        ),
        (
            "🐱".repeat(80),
            format!("{}🦀{}", "🐱".repeat(20), "🐱".repeat(60)),
        ),
        (
            format!("start {} end", "middle ".repeat(100)),
            format!("START {} END", "middle ".repeat(100)),
        ),
        ("a".into(), "😀".into()),
    ] {
        check(&dp, &json!(a), &json!(b));
    }
}

#[test]
fn bad_baseline_is_an_error_and_input_is_unchanged() {
    let delta = Delta::from_value(json!({"x":[1,2]})).unwrap();
    let input = json!({"x":9});
    let saved = input.clone();
    assert!(patch(&input, &delta).is_err());
    assert_eq!(input, saved);
    for invalid in [
        json!(null),
        json!([]),
        json!([1, 0, 7]),
        json!({"_t":"a","_0":[1,2]}),
        json!({"_t":"a","01":[1]}),
    ] {
        assert!(
            Delta::from_value(invalid.clone()).is_err(),
            "accepted {invalid}"
        );
    }
}

#[test]
fn filters_and_depth_limits() {
    let dp = DiffPatcher::new(DiffOptions {
        property_filter: Some(Arc::new(|name, _, _, path| {
            name != "ignored" || path != "/nested"
        })),
        ..Default::default()
    });
    let a = json!({"nested":{"ignored":1,"kept":2},"ignored":1});
    let b = json!({"nested":{"ignored":9,"kept":3},"ignored":2});
    let d = dp.diff(&a, &b).unwrap().unwrap();
    let expected = json!({"nested":{"ignored":1,"kept":3},"ignored":2});
    assert_eq!(patch(&a, &d).unwrap(), expected);
    assert_eq!(unpatch(&expected, &d).unwrap(), a);
    let dp = DiffPatcher::new(DiffOptions {
        max_depth: 2,
        ..Default::default()
    });
    assert!(dp.diff(&json!({"a":{"b":[]}}), &json!(null)).is_err());
    assert!(dp.diff(&json!({"a":[]}), &json!(null)).is_ok());
    assert!(Delta::from_value(json!(["@@ -1,3 +1,2 @@\n-abc\n+X\n", 0, 2])).is_err());
}

#[test]
fn invalid_array_and_text_patches() {
    for wire in [
        json!({"_t":"a","_0":["",0,3],"0":[1]}),
        json!({"_t":"a","_0":[1,0,0],"_1":[2,0,0],"0":[1,2]}),
        json!(["@@ -1,1 +1,1 @@\n-%zz\n+x\n", 0, 2]),
        json!({"_t":"a","_0":["",-1,3]}),
    ] {
        if let Ok(d) = Delta::from_value(wire) {
            assert!(patch(&json!([]), &d).is_err());
        }
    }
    for wire in [
        json!({"_t":"a","_9":[1,0,0]}),
        json!({"_t":"a","5":[1]}),
        json!({"_t":"a","_0":["",99,3]}),
    ] {
        let d = Delta::from_value(wire).unwrap();
        assert!(patch(&json!([1]), &d).is_err());
    }
    let d = Delta::from_value(json!(["@@ -1,3 +1,3 @@\n-abc\n+xyz\n", 0, 2])).unwrap();
    assert!(patch(&json!("wrong"), &d).is_err());
    assert!(patch(&json!(123), &d).is_err());
}

#[test]
fn compact_moves_in_both_directions() {
    let a = json!((0..2000).collect::<Vec<_>>());
    for right in [
        json!((1..2000).chain([0]).collect::<Vec<_>>()),
        json!([1999].into_iter().chain(0..1999).collect::<Vec<_>>()),
    ] {
        let d = diff(&a, &right).unwrap().unwrap();
        let p = d.to_json_patch(&a).unwrap();
        assert_eq!(p.0.len(), 1);
        assert!(matches!(p.0[0], palim::PatchOperation::Move(_)));
        assert_eq!(apply_json_patch(&a, &p).unwrap(), right);
    }
}

#[test]
fn standard_patch_atomicity_error_path_and_intermediate_depth() {
    let input = json!({"x":1});
    let p: palim::Patch = serde_json::from_value(json!([
        {"op":"replace","path":"/x","value":2},
        {"op":"remove","path":"/missing"}
    ]))
    .unwrap();
    let error = apply_json_patch(&input, &p).unwrap_err();
    assert_eq!(error.path, "/missing");
    assert!(error.message.contains("'/1'"));
    assert_eq!(input, json!({"x":1}));
    let operations = json!(
        (0..140)
            .map(|_| json!({"op":"copy","from":"","path":"/x"}))
            .collect::<Vec<_>>()
    );
    let p = serde_json::from_value(operations).unwrap();
    assert!(
        apply_json_patch(&input, &p)
            .unwrap_err()
            .message
            .contains("nesting")
    );
    assert_eq!(input, json!({"x":1}));
}

#[test]
fn near_depth_limit_deltas_round_trip_or_return_depth_error() {
    let mut nested = json!(0);
    for _ in 0..130 {
        nested = Value::Array(vec![nested]);
        match diff(&json!(null), &nested) {
            Ok(Some(delta)) => {
                let decoded: Delta =
                    serde_json::from_str(&serde_json::to_string(&delta).unwrap()).unwrap();
                assert_eq!(patch(&json!(null), &decoded).unwrap(), nested);
            }
            Err(e) => assert!(e.message.contains("nesting")),
            Ok(None) => panic!("different documents must have a delta"),
        }
    }
}
