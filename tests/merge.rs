use palim::{compose_merge_patches, diff_merge_patch, merge_patch, merge_patch_in_place};
use proptest::prelude::*;
use serde_json::{Value, json};

#[test]
fn rfc7396_appendix_a_application_and_diff() {
    // Public examples from https://www.rfc-editor.org/rfc/rfc7396#appendix-A.
    let cases = [
        (json!({"a":"b"}), json!({"a":"c"}), json!({"a":"c"})),
        (json!({"a":"b"}), json!({"b":"c"}), json!({"a":"b","b":"c"})),
        (json!({"a":"b"}), json!({"a":null}), json!({})),
        (
            json!({"a":"b","b":"c"}),
            json!({"a":null}),
            json!({"b":"c"}),
        ),
        (json!({"a":["b"]}), json!({"a":"c"}), json!({"a":"c"})),
        (json!({"a":"c"}), json!({"a":["b"]}), json!({"a":["b"]})),
        (
            json!({"a":{"b":"c"}}),
            json!({"a":{"b":"d","c":null}}),
            json!({"a":{"b":"d"}}),
        ),
        (json!({"a":[{"b":"c"}]}), json!({"a":[1]}), json!({"a":[1]})),
        (json!(["a", "b"]), json!(["c", "d"]), json!(["c", "d"])),
        (json!({"a":"b"}), json!(["c"]), json!(["c"])),
        (json!({"a":"foo"}), json!(null), json!(null)),
        (json!({"a":"foo"}), json!("bar"), json!("bar")),
        (json!({"e":null}), json!({"a":1}), json!({"e":null,"a":1})),
        (json!([1, 2]), json!({"a":"b","c":null}), json!({"a":"b"})),
        (
            json!({}),
            json!({"a":{"bb":{"ccc":null}}}),
            json!({"a":{"bb":{}}}),
        ),
    ];
    for (source, patch, expected) in cases {
        assert_eq!(merge_patch(&source, &patch).unwrap(), expected);
        let mut mutable = source.clone();
        merge_patch_in_place(&mut mutable, &patch).unwrap();
        assert_eq!(mutable, expected);
        let generated = diff_merge_patch(&source, &expected).unwrap();
        assert_eq!(merge_patch(&source, &generated).unwrap(), expected);
    }
}

#[test]
fn replacement_and_empty_object_semantics() {
    for source in [json!(null), json!(false), json!(1), json!("x"), json!([1])] {
        assert_eq!(merge_patch(&source, &json!({})).unwrap(), json!({}));
        let unchanged = diff_merge_patch(&source, &source).unwrap();
        assert_eq!(merge_patch(&source, &unchanged).unwrap(), source);
        let materialize = diff_merge_patch(&source, &json!({})).unwrap();
        assert_eq!(merge_patch(&source, &materialize).unwrap(), json!({}));
    }
    assert_eq!(
        diff_merge_patch(&json!({}), &json!({"created":{}})).unwrap(),
        json!({"created":{}})
    );
    let source = json!({"n":null,"unchanged":{"n":null},"gone":9});
    let target = json!({"n":null,"unchanged":{"n":null},"created":{}});
    let patch = diff_merge_patch(&source, &target).unwrap();
    assert_eq!(patch, json!({"gone":null,"created":{}}));
    assert_eq!(merge_patch(&source, &patch).unwrap(), target);
}

#[test]
fn null_assignment_is_rejected_only_when_unrepresentable() {
    for (source, target, pointer) in [
        (json!({}), json!({"n":null}), "/n"),
        (json!({"n":1}), json!({"n":null}), "/n"),
        (json!(null), json!({"n":null}), "/n"),
        (json!({"a/b":{}}), json!({"a/b":{"~n":null}}), "/a~1b/~0n"),
        (
            json!({"n":{"kept":null}}),
            json!({"n":{"kept":null,"new":null}}),
            "/n/new",
        ),
    ] {
        let error = diff_merge_patch(&source, &target).unwrap_err();
        assert_eq!(error.path, pointer);
        assert!(error.message.contains("null"));
    }
    for (source, target) in [
        (json!({"n":null}), json!({"n":null})),
        (json!({}), json!([{"n":null}])),
        (json!({}), json!(null)),
    ] {
        let patch = diff_merge_patch(&source, &target).unwrap();
        assert_eq!(merge_patch(&source, &patch).unwrap(), target);
    }
}

#[test]
fn composition_preserves_deletion_instructions_and_nested_updates() {
    let first = json!({"a":null,"b":1,"nested":{"remove":null,"x":1},"keep":2});
    let second = json!({"b":null,"nested":{"y":2},"new":3});
    let combined = compose_merge_patches(&first, &second).unwrap();
    assert_eq!(
        combined,
        json!({"a":null,"b":null,"nested":{"remove":null,"x":1,"y":2},"keep":2,"new":3})
    );
    for baseline in [
        json!(null),
        json!([1]),
        json!({}),
        json!({"a":3,"b":4,"nested":{"remove":9,"unknown":8},"unknown":7}),
    ] {
        let once = merge_patch(&baseline, &combined).unwrap();
        let twice = merge_patch(&merge_patch(&baseline, &first).unwrap(), &second).unwrap();
        assert_eq!(once, twice);
    }
    assert_eq!(first["a"], json!(null));
    assert_eq!(second["b"], json!(null));
    for second in [json!(null), json!(4), json!([{"n":null}]), json!("x")] {
        assert_eq!(compose_merge_patches(&first, &second).unwrap(), second);
    }
}

#[test]
fn composition_rejects_loss_of_unknown_baseline_fields() {
    for (first, second, pointer) in [
        (json!(null), json!({}), ""),
        (json!(1), json!({"x":1}), ""),
        (json!([]), json!({"x":null}), ""),
        (json!({"a":null}), json!({"a":{}}), "/a"),
        (json!({"a":1}), json!({"a":{"x":2}}), "/a"),
        (
            json!({"a/b":{"~n":[]}}),
            json!({"a/b":{"~n":{"x":null}}}),
            "/a~1b/~0n",
        ),
    ] {
        assert_eq!(
            compose_merge_patches(&first, &second).unwrap_err().path,
            pointer
        );
    }
    // Applying second to first as ordinary JSON is not valid composition:
    // it wrongly preserves unknown baseline fields that first had removed.
    let first = json!(1);
    let second = json!({"x":2});
    let naive = merge_patch(&first, &second).unwrap();
    let baseline = json!({"unknown":3});
    assert_ne!(
        merge_patch(&baseline, &naive).unwrap(),
        merge_patch(&merge_patch(&baseline, &first).unwrap(), &second).unwrap()
    );
}

fn nested(depth: usize) -> Value {
    (0..depth).fold(json!(1), |value, _| json!({"next":value}))
}

#[test]
fn depth_errors_are_atomic() {
    let at_limit = nested(128);
    assert_eq!(merge_patch(&json!(null), &at_limit).unwrap(), at_limit);
    let too_deep = nested(129);
    let mut source = json!({"unchanged":true});
    let saved = source.clone();
    assert!(merge_patch_in_place(&mut source, &too_deep).is_err());
    assert_eq!(source, saved);
    assert!(merge_patch(&too_deep, &json!({})).is_err());
    assert!(diff_merge_patch(&saved, &too_deep).is_err());
    assert!(diff_merge_patch(&too_deep, &saved).is_err());
    assert!(compose_merge_patches(&too_deep, &json!({})).is_err());
    assert!(compose_merge_patches(&json!({}), &too_deep).is_err());
}

fn value_strategy() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| json!(n)),
        any::<u64>().prop_map(|n| json!(n)),
        (-1e12f64..1e12).prop_map(|n| json!(n)),
        "[a-z~/]{0,12}".prop_map(Value::String),
    ];
    leaf.prop_recursive(5, 80, 6, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            proptest::collection::btree_map("[a-z~/]{0,5}", inner, 0..6)
                .prop_map(|entries| Value::Object(entries.into_iter().collect())),
        ]
    })
}

fn reference_merge(source: &Value, patch: &Value) -> Value {
    // Use the existing independent RFC merge implementation as the oracle.
    let mut output = source.clone();
    json_patch::merge(&mut output, patch);
    output
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 2048,
        rng_seed: proptest::test_runner::RngSeed::Fixed(20261001),
        ..ProptestConfig::default()
    })]

    #[test]
    fn generated_patch_round_trips_every_representable_target(
        source in value_strategy(), arbitrary_patch in value_strategy()
    ) {
        let target = reference_merge(&source, &arbitrary_patch);
        prop_assert_eq!(merge_patch(&source, &arbitrary_patch).unwrap(), target.clone());
        let generated = diff_merge_patch(&source, &target).unwrap();
        prop_assert_eq!(reference_merge(&source, &generated), target);
    }

    #[test]
    fn accepted_compositions_equal_sequential_application(
        baseline in value_strategy(), first in value_strategy(), second in value_strategy()
    ) {
        if let Ok(combined) = compose_merge_patches(&first, &second) {
            let sequential = reference_merge(&reference_merge(&baseline, &first), &second);
            prop_assert_eq!(reference_merge(&baseline, &combined), sequential);
        }
    }
}
