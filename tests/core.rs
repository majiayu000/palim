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
    #[cfg(feature = "exact-numbers")]
    assert_eq!(decoded, delta);
    #[cfg(not(feature = "exact-numbers"))]
    {
        // Without float_roundtrip, serde_json may round a serialized f64 again.
        // Replay the wire delta against documents decoded by the same parser.
        let wire_left: Value = serde_json::from_str(&serde_json::to_string(left).unwrap()).unwrap();
        let wire_right: Value =
            serde_json::from_str(&serde_json::to_string(right).unwrap()).unwrap();
        assert_eq!(patch(&wire_left, &decoded).unwrap(), wire_right);
        assert_eq!(unpatch(&wire_right, &decoded).unwrap(), wire_left);
    }
    assert_eq!(
        apply_json_patch(left, &delta.to_json_patch(left).unwrap()).unwrap(),
        *right
    );
}

#[test]
fn by_key_tracks_moves_and_edits_with_typed_literal_field_ids() {
    let engine = DiffPatcher::by_key(String::from("user/id"));
    let before = json!([
        {"user/id": 1, "name": "Ada"},
        {"user/id": "1", "name": "Grace"},
        {"user/id": 2, "name": "Linus"}
    ]);
    let after = json!([
        {"user/id": 2, "name": "Linus updated"},
        {"user/id": 1, "name": "Ada"},
        {"user/id": "1", "name": "Grace updated"}
    ]);
    let delta = engine.diff(&before, &after).unwrap().unwrap();
    assert_eq!(delta.as_value()["_2"], json!(["", 0, 3]));
    assert_eq!(
        delta.as_value()["0"]["name"],
        json!(["Linus", "Linus updated"])
    );
    assert_eq!(
        delta.as_value()["2"]["name"],
        json!(["Grace", "Grace updated"])
    );
    check(&engine, &before, &after);
}

#[test]
fn by_key_accepts_missing_and_repeated_ids_and_primitive_items() {
    let engine = DiffPatcher::by_key("id");
    let pairs = [
        (
            json!([{"label": "unkeyed"}, {"id": "a", "done": false}, 7, null, [1]]),
            json!([[1], null, 7, {"id": "a", "done": true}, {"label": "unkeyed"}]),
        ),
        (
            json!([{"id": "same", "v": 1}, {"id": "same", "v": 2}]),
            json!([{"id": "same", "v": 2}, {"id": "same", "v": 3}]),
        ),
    ];
    for (before, after) in pairs {
        check(&engine, &before, &after);
    }
    assert!(
        engine
            .diff(
                &json!([{ "label": "unkeyed" }]),
                &json!([{ "label": "unkeyed" }])
            )
            .unwrap()
            .is_none()
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
fn atomic_replacements_keep_valid_number_and_mixed_payload_representations() {
    let dp = DiffPatcher::new(DiffOptions {
        text_diff_min_length: None,
        ..Default::default()
    });
    let mut values: Vec<Value> = [
        "null",
        "true",
        "1",
        "1.0",
        "-0.0",
        #[cfg(feature = "exact-numbers")]
        "18446744073709551617",
        #[cfg(feature = "exact-numbers")]
        "-18446744073709551617",
        #[cfg(feature = "exact-numbers")]
        "0.012345678900000001234567890000000123456789",
        #[cfg(feature = "exact-numbers")]
        "1e10000",
        #[cfg(feature = "exact-numbers")]
        "10e9999",
        #[cfg(feature = "exact-numbers")]
        "-1e-10000",
        r#""escaped \" text 🦀\n""#,
        #[cfg(feature = "exact-numbers")]
        r#"[null,true,1.0,{"n":18446744073709551617}]"#,
        #[cfg(feature = "exact-numbers")]
        r#"{"a/b":[-0.0,1e10000],"~key":{"n":0.01234567890000000123456789}}"#,
    ]
    .iter()
    .map(|input| serde_json::from_str(input).unwrap())
    .collect();
    values.extend(
        [
            0.1,
            398043487186.00244,
            1.0e40,
            f64::MAX,
            f64::MIN_POSITIVE,
            f64::from_bits(1),
        ]
        .into_iter()
        .map(|number| Value::Number(serde_json::Number::from_f64(number).unwrap())),
    );
    for left in values {
        let right = if left.is_boolean() {
            Value::Null
        } else {
            Value::Bool(false)
        };
        let delta = dp.diff(&left, &right).unwrap().unwrap();
        let previous = json!([&left, &right]);
        assert_eq!(delta.as_value(), &previous);
        assert_eq!(
            serde_json::to_vec(&delta).unwrap(),
            serde_json::to_vec(&previous).unwrap()
        );
        assert_eq!(delta.as_value()[0], left);
        check(&dp, &left, &right);
    }
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
