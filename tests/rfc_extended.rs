use palim::{
    CompareOptions, DiffOptions, DiffPatcher, JsonPatchOptions, Patch, PatchOperation,
    apply_json_patch, compare, diff_json_patch, invert_json_patch,
};
use proptest::prelude::*;
use serde_json::{Value, json};
use std::sync::Arc;

fn options(factorize: bool, rationalize: bool, tests: bool) -> JsonPatchOptions {
    JsonPatchOptions {
        factorize,
        rationalize,
        tests,
    }
}

fn parse(value: Value) -> Patch {
    serde_json::from_value(value).unwrap()
}

fn assert_roundtrip(before: &Value, after: &Value, patch: &Patch) {
    assert_eq!(apply_json_patch(before, patch).unwrap(), *after);
    let inverse = invert_json_patch(before, patch).unwrap();
    assert_eq!(apply_json_patch(after, &inverse).unwrap(), *before);
}

#[test]
fn plain_reorders_trade_low_byte_savings_for_positional_output() {
    for escaped_strings in [false, true] {
        let source: Vec<Value> = (0..256)
            .map(|i| {
                if escaped_strings {
                    json!(format!("{i}\"\\\n🦀"))
                } else {
                    json!(i)
                }
            })
            .collect();
        let target: Vec<_> = source.iter().rev().cloned().collect();
        let key = "a/~\"\\🦀";
        let before = json!({key: source});
        let after = json!({key: target});
        let positional = diff_json_patch(&before, &after, &options(false, false, false)).unwrap();
        assert_eq!(positional.0.len(), 256);
        assert!(
            positional
                .0
                .iter()
                .all(|op| matches!(op, PatchOperation::Replace(_)))
        );
        assert_roundtrip(&before, &after, &positional);
        let mut independent = before.clone();
        json_patch::patch(&mut independent, &positional).unwrap();
        assert_eq!(independent, after);

        // Native delta and its exporter still use the minimum-move script.
        let native = palim::diff(&before, &after).unwrap().unwrap();
        let exported = native.to_json_patch(&before).unwrap();
        assert_eq!(exported.0.len(), 255);
        assert!(
            exported
                .0
                .iter()
                .all(|op| matches!(op, PatchOperation::Move(_)))
        );
        for opts in [
            options(false, false, true),
            options(true, false, false),
            options(false, true, false),
        ] {
            let patch = diff_json_patch(&before, &after, &opts).unwrap();
            if !opts.rationalize {
                assert!(
                    patch
                        .0
                        .iter()
                        .any(|op| matches!(op, PatchOperation::Move(_)))
                );
            } else {
                // Rationalization can replace the whole changed container.
                assert!(patch.0.len() < positional.0.len());
            }
            assert_roundtrip(&before, &after, &patch);
        }
    }
}

#[test]
fn plain_keeps_rotations_and_large_value_moves_byte_identical_to_native() {
    for payload in [0, 256] {
        let source: Vec<Value> = (0..256)
            .map(|i| {
                if payload == 0 {
                    json!(i)
                } else {
                    json!(format!("{i}:{}", "🦀\"\\\n".repeat(payload)))
                }
            })
            .collect();
        for shift in [1, 128, 255] {
            let mut target = source.clone();
            target.rotate_left(shift);
            let before = json!(source);
            let after = json!(target);
            let expected = palim::diff(&before, &after)
                .unwrap()
                .unwrap()
                .to_json_patch(&before)
                .unwrap();
            let actual = diff_json_patch(&before, &after, &options(false, false, false)).unwrap();
            assert_eq!(
                serde_json::to_vec(&actual).unwrap(),
                serde_json::to_vec(&expected).unwrap()
            );
            assert_roundtrip(&before, &after, &actual);
        }
        if payload != 0 {
            let before = json!(source);
            let after = json!(source.iter().rev().cloned().collect::<Vec<_>>());
            let expected = palim::diff(&before, &after)
                .unwrap()
                .unwrap()
                .to_json_patch(&before)
                .unwrap();
            let actual = diff_json_patch(&before, &after, &options(false, false, false)).unwrap();
            assert_eq!(actual, expected);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases:128, rng_seed:proptest::test_runner::RngSeed::Fixed(20261006), ..ProptestConfig::default() })]
    #[test]
    fn adaptive_plain_permutations_roundtrip_independently(
        priorities in proptest::collection::vec(any::<u32>(), 2..128),
        nested in any::<bool>(),
    ) {
        // Sort by arbitrary priorities to generate permutations independently
        // of production token interning/LIS and include fixed positions.
        let source: Vec<_> = (0..priorities.len()).collect();
        let mut target = source.clone();
        target.sort_by_key(|&i| (priorities[i], i));
        let mut before = json!(source);
        let mut after = json!(target);
        if nested {
            before = json!({"a/~\"\\🦀":before});
            after = json!({"a/~\"\\🦀":after});
        }
        let patch = diff_json_patch(&before, &after, &options(false, false, false))?;
        let mut independent = before.clone();
        json_patch::patch(&mut independent, &patch)?;
        prop_assert_eq!(&independent, &after);
        let inverse = invert_json_patch(&before, &patch)?;
        json_patch::patch(&mut independent, &inverse)?;
        prop_assert_eq!(independent, before);
    }
}

#[test]
fn factorizes_cross_property_and_cross_array_moves() {
    let payload = "a large payload ".repeat(100);
    let before = json!({"a/b": payload, "keep": 1});
    let after = json!({"renamed": payload, "keep": 1});
    let patch = diff_json_patch(&before, &after, &options(true, false, false)).unwrap();
    assert!(
        patch
            .0
            .iter()
            .any(|op| matches!(op, PatchOperation::Move(op)
        if op.from.as_str() == "/a~1b" && op.path.as_str() == "/renamed"))
    );
    assert_roundtrip(&before, &after, &patch);

    let before = json!({"left": [payload, "survivor"], "right": ["existing"]});
    let after = json!({"left": ["survivor"], "right": [payload, "existing"]});
    let patch = diff_json_patch(&before, &after, &options(true, false, false)).unwrap();
    assert!(
        patch
            .0
            .iter()
            .any(|op| matches!(op, PatchOperation::Move(op)
        if op.from.as_str() == "/left/0" && op.path.as_str() == "/right/0"))
    );
    assert_roundtrip(&before, &after, &patch);
}

#[test]
fn factorizes_copies_using_the_current_document() {
    let template = json!({"body": "template ".repeat(120)});
    let before = json!({"template": template, "copies": []});
    let after = json!({"template": template, "copies": [template]});
    let patch = diff_json_patch(&before, &after, &options(true, false, false)).unwrap();
    assert!(
        patch
            .0
            .iter()
            .any(|op| matches!(op, PatchOperation::Copy(op)
        if op.from.as_str() == "/template" && op.path.as_str() == "/copies/0"))
    );
    assert_roundtrip(&before, &after, &patch);

    let payload = "same array value ".repeat(100);
    let before = json!([payload, "other"]);
    let after = json!([payload, "other", payload]);
    let patch = diff_json_patch(&before, &after, &options(true, false, false)).unwrap();
    assert!(
        patch
            .0
            .iter()
            .any(|op| matches!(op, PatchOperation::Copy(_)))
    );
    assert_roundtrip(&before, &after, &patch);
}

#[test]
fn copy_sources_must_fit_the_serialized_path_budget() {
    // The shorter pointer needs two JSON escapes, making it too expensive.
    // The slightly longer ASCII pointer produces a genuinely smaller copy.
    // With 1234 both sources fit, but the ASCII source still saves more bytes.
    for value in [123, 1234] {
        let before = json!({"\"\n": value, "abc": value, "target": 0});
        let after = json!({"\"\n": value, "abc": value, "target": value});
        let plain = diff_json_patch(&before, &after, &options(false, false, false)).unwrap();
        let optimized = diff_json_patch(&before, &after, &options(true, false, false)).unwrap();
        assert!(
            optimized
                .0
                .iter()
                .any(|op| matches!(op, PatchOperation::Copy(op)
        if op.from.as_str() == "/abc" && op.path.as_str() == "/target"))
        );
        assert!(
            serde_json::to_vec(&optimized).unwrap().len()
                < serde_json::to_vec(&plain).unwrap().len()
        );
        assert_roundtrip(&before, &after, &optimized);
    }
}

#[test]
fn copy_cost_bounds_allow_shortened_moves_copies_and_array_indices() {
    for tests in [false, true] {
        let before = json!({"aa": {"deep": 1234}, "x": 0});
        let after = json!({"aa": {}, "b": 1234, "x": 1234});
        let patch = diff_json_patch(&before, &after, &options(true, false, tests)).unwrap();
        assert!(
            patch
                .0
                .iter()
                .any(|op| matches!(op, PatchOperation::Move(op)
            if op.from.as_str() == "/aa/deep" && op.path.as_str() == "/b"))
        );
        assert!(
            patch
                .0
                .iter()
                .any(|op| matches!(op, PatchOperation::Copy(op)
            if op.from.as_str() == "/b" && op.path.as_str() == "/x"))
        );
        assert_roundtrip(&before, &after, &patch);

        let payload = json!({"v": 1234, "padding": "a large payload ".repeat(20)});
        // Load the numeric cache before the container copy shortens its path.
        let before = json!({"a": 0, "long_name": payload, "target": 0});
        let after = json!({"a": 2345, "b": payload, "long_name": payload, "target": 1234});
        let patch = diff_json_patch(&before, &after, &options(true, false, tests)).unwrap();
        assert!(
            patch
                .0
                .iter()
                .any(|op| matches!(op, PatchOperation::Copy(op)
            if op.from.as_str() == "/long_name" && op.path.as_str() == "/b"))
        );
        assert!(
            patch
                .0
                .iter()
                .any(|op| matches!(op, PatchOperation::Copy(op)
            if op.from.as_str() == "/b/v" && op.path.as_str() == "/target"))
        );
        assert_roundtrip(&before, &after, &patch);

        let before = json!([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 12345, "A", "B"]);
        let after = json!([12345, "A", 12345, "B"]);
        let patch = diff_json_patch(&before, &after, &options(true, false, tests)).unwrap();
        assert!(
            patch
                .0
                .iter()
                .any(|op| matches!(op, PatchOperation::Copy(op)
            if op.from.as_str() == "/0" && op.path.as_str() == "/2"))
        );
        assert_roundtrip(&before, &after, &patch);

        let before = json!({"1999": 1234, "target": 0});
        let after = json!({"0": 1234, "target": 1234});
        let patch = diff_json_patch(&before, &after, &options(true, false, tests)).unwrap();
        assert!(
            patch
                .0
                .iter()
                .any(|op| matches!(op, PatchOperation::Copy(op)
            if op.from.as_str() == "/0" && op.path.as_str() == "/target"))
        );
        assert_roundtrip(&before, &after, &patch);
    }
}

#[test]
fn copies_strings_added_or_replaced_by_an_earlier_operation() {
    let payload = "a new string payload ".repeat(100);
    let after = json!({"a": payload, "b": payload});
    for before in [json!({}), json!({"a": 0, "b": 0})] {
        for tests in [false, true] {
            let patch = diff_json_patch(&before, &after, &options(true, false, tests)).unwrap();
            assert!(
                patch
                    .0
                    .iter()
                    .any(|op| matches!(op, PatchOperation::Copy(op)
                if op.from.as_str() == "/a" && op.path.as_str() == "/b"))
            );
            assert_roundtrip(&before, &after, &patch);
        }
    }
}

#[test]
fn copies_strings_inside_an_earlier_added_or_replaced_payload() {
    let payload = "a new nested string ".repeat(100);
    let after = json!({"a": {"nested": [{"text": payload}]}, "b": payload});
    for before in [json!({}), json!({"a": 0, "b": 0})] {
        for tests in [false, true] {
            let patch = diff_json_patch(&before, &after, &options(true, false, tests)).unwrap();
            assert!(
                patch
                    .0
                    .iter()
                    .any(|op| matches!(op, PatchOperation::Copy(op)
                if op.from.as_str() == "/a/nested/0/text" && op.path.as_str() == "/b"))
            );
            assert_roundtrip(&before, &after, &patch);
        }
    }
}

#[test]
fn copies_numbers_added_or_replaced_by_an_earlier_operation() {
    let payload: Value =
        serde_json::from_str("340282366920938463463374607431768211456000000000000000001").unwrap();
    let after = json!({"a": payload, "b": payload});
    for before in [json!({}), json!({"a": 0, "b": 0})] {
        for tests in [false, true] {
            let patch = diff_json_patch(&before, &after, &options(true, false, tests)).unwrap();
            assert!(
                patch
                    .0
                    .iter()
                    .any(|op| matches!(op, PatchOperation::Copy(op)
                if op.from.as_str() == "/a" && op.path.as_str() == "/b"))
            );
            assert_roundtrip(&before, &after, &patch);
        }
    }
}

#[test]
fn copy_candidates_use_mathematical_number_equality() {
    for (source, wanted) in [
        ("1", "1.000000000000000000000000000000000000000000"),
        ("0", "-0.000000000000000000000000000000000000000000"),
        (
            "18446744073709551616",
            "1.8446744073709551616000000000000000000000e19",
        ),
        ("1e10000", "10.000000000000000000000000000000000000e9999"),
    ] {
        let source: Value = serde_json::from_str(source).unwrap();
        let wanted: Value = serde_json::from_str(wanted).unwrap();
        let before = json!({"a": source});
        let after = json!({"a": source, "b": wanted});
        for tests in [false, true] {
            let patch = diff_json_patch(&before, &after, &options(true, false, tests)).unwrap();
            assert!(
                patch
                    .0
                    .iter()
                    .any(|op| matches!(op, PatchOperation::Copy(op)
                if op.from.as_str() == "/a" && op.path.as_str() == "/b"))
            );
            let result = apply_json_patch(&before, &patch).unwrap();
            assert!(
                compare(&result, &after, &CompareOptions::default())
                    .unwrap()
                    .differences
                    .is_empty()
            );
            let inverse = invert_json_patch(&before, &patch).unwrap();
            assert_eq!(apply_json_patch(&result, &inverse).unwrap(), before);
        }
    }
}

#[test]
fn factorization_handles_interleaved_array_indices_and_duplicates() {
    let value = "duplicate value ".repeat(30);
    let cases = [
        (
            json!({"a": [value, "x", value, "y"], "b": ["z"]}),
            json!({"a": ["y", value], "b": [value, "x", "z"]}),
        ),
        (
            json!([value, "x", "y", value]),
            json!(["y", value, value, "x"]),
        ),
        (
            json!({"a": {"inside": value}, "b": value}),
            json!({"a": value, "b": {"inside": value}}),
        ),
    ];
    for (before, after) in cases {
        for tests in [false, true] {
            let patch = diff_json_patch(&before, &after, &options(true, false, tests)).unwrap();
            assert_roundtrip(&before, &after, &patch);
            let mut independent = before.clone();
            json_patch::patch(&mut independent, &patch).unwrap();
            assert_eq!(independent, after);
        }
    }
}

#[test]
fn rationalizes_nested_parents_by_serialized_size() {
    let before = json!({"unchanged": "keep ".repeat(2000), "section": {
        "a": 0, "b": 0, "c": 0, "d": 0, "e": 0, "f": 0
    }});
    let after = json!({"unchanged": "keep ".repeat(2000), "section": {
        "a": 1, "b": 2, "c": 3, "d": 4, "e": 5, "f": 6
    }});
    let raw = diff_json_patch(&before, &after, &options(false, false, false)).unwrap();
    let patch = diff_json_patch(&before, &after, &options(false, true, false)).unwrap();
    assert!(
        patch
            .0
            .iter()
            .any(|op| matches!(op, PatchOperation::Replace(op)
        if op.path.as_str() == "/section"))
    );
    assert!(
        !patch
            .0
            .iter()
            .any(|op| matches!(op, PatchOperation::Replace(op)
        if op.path.as_str().is_empty()))
    );
    assert!(serde_json::to_vec(&patch).unwrap().len() < serde_json::to_vec(&raw).unwrap().len());
    assert_roundtrip(&before, &after, &patch);
}

#[test]
fn rationalization_accounts_for_tests_and_escaped_utf8_bytes() {
    let before = json!({"unchanged": "large ".repeat(1000), "a/b": {
        "长字段一": "old", "长字段二": "old", "长字段三": "old", "quote\"": "old"
    }});
    let after = json!({"unchanged": "large ".repeat(1000), "a/b": {
        "长字段一": "new", "长字段二": "new", "长字段三": "new", "quote\"": "new"
    }});
    for tests in [false, true] {
        let raw = diff_json_patch(&before, &after, &options(false, false, tests)).unwrap();
        let patch = diff_json_patch(&before, &after, &options(false, true, tests)).unwrap();
        assert!(
            serde_json::to_vec(&patch).unwrap().len() <= serde_json::to_vec(&raw).unwrap().len()
        );
        assert_roundtrip(&before, &after, &patch);
    }
}

#[test]
fn guarded_rationalization_keeps_move_copy_prefixes_and_nested_replacements() {
    let engine = DiffPatcher::new(DiffOptions {
        object_hash: Some(Arc::new(|value, _| value.get("id").map(Value::to_string))),
        ..Default::default()
    });
    let old = json!({"a/b":0,"b~c":0,"中文🦀":0,"quote\"":0,"fifth":0,"sixth":0});
    let new = json!({"a/b":1,"b~c":2,"中文🦀":3,"quote\"":4,"fifth":5,"sixth":6});
    let payload = "copy payload 🦀 ".repeat(60);
    let moved_payload = "move material 🦀 ".repeat(80);
    let before = json!({
        "a\"copy":"old", "a~/move":[
            {"id":0,"body":moved_payload},{"id":1,"body":moved_payload},{"id":2,"body":moved_payload}],
        "section/🦀~":{"one":old,"two":old}, "source":payload,
        "untouched":"large unchanged payload ".repeat(1000)
    });
    let after = json!({
        "a\"copy":payload, "a~/move":[
            {"id":2,"body":moved_payload},{"id":0,"body":moved_payload},{"id":1,"body":moved_payload}],
        "section/🦀~":{"one":new,"two":new}, "source":payload,
        "untouched":before["untouched"]
    });
    let original = engine
        .diff_json_patch(&before, &after, &options(true, false, true))
        .unwrap();
    let optimized = engine
        .diff_json_patch(&before, &after, &options(true, true, true))
        .unwrap();
    let section = "/section~1🦀~0";
    let prefix = original
        .0
        .iter()
        .position(|op| match op {
            PatchOperation::Test(op) => op.path.as_str().starts_with(section),
            _ => false,
        })
        .unwrap();
    assert_eq!(&optimized.0[..prefix], &original.0[..prefix]);
    assert!(
        original.0[..prefix]
            .iter()
            .any(|op| matches!(op, PatchOperation::Move(_)))
    );
    assert!(
        original.0[..prefix]
            .iter()
            .any(|op| matches!(op, PatchOperation::Copy(_)))
    );
    for (index, op) in original.0[..prefix].iter().enumerate() {
        if matches!(op, PatchOperation::Move(_) | PatchOperation::Copy(_)) {
            assert!(matches!(original.0[index - 2], PatchOperation::Test(_)));
            assert!(matches!(original.0[index - 1], PatchOperation::Test(_)));
        }
    }
    assert!(
        optimized
            .0
            .iter()
            .any(|op| matches!(op, PatchOperation::Replace(op)
        if op.path.as_str() == section))
    );
    assert!(
        serde_json::to_vec(&optimized).unwrap().len()
            < serde_json::to_vec(&original).unwrap().len()
    );
    assert_roundtrip(&before, &after, &optimized);
    let mut independent = before.clone();
    json_patch::patch(&mut independent, &optimized).unwrap();
    assert_eq!(independent, after);
    let mut bad_source = before.clone();
    bad_source["source"] = json!("wrong");
    assert!(apply_json_patch(&bad_source, &optimized).is_err());
    let mut bad_order = before.clone();
    bad_order["a~/move"][0]["id"] = json!(99);
    assert!(apply_json_patch(&bad_order, &optimized).is_err());
}

#[test]
fn guarded_repeated_id_reordering_authenticates_the_replaced_container() {
    use palim::{DiffOptions, DiffPatcher};
    use std::sync::Arc;

    let items: Vec<_> = (0..256).map(|i| json!({"id": i % 8, "value": i})).collect();
    let mut reordered = items.clone();
    reordered.rotate_left(127);
    reordered[128]["value"] = json!(-1);
    // A large unchanged sibling makes a whole-document replacement wasteful.
    let before = json!({"items": items, "keep": "unchanged ".repeat(10_000)});
    let after = json!({"items": reordered, "keep": before["keep"]});
    let engine = DiffPatcher::new(DiffOptions {
        object_hash: Some(Arc::new(|v, _| v.get("id").map(Value::to_string))),
        ..Default::default()
    });
    let raw = engine
        .diff_json_patch(&before, &after, &options(false, false, true))
        .unwrap();
    let patch = engine
        .diff_json_patch(&before, &after, &options(true, true, true))
        .unwrap();
    assert_roundtrip(&before, &after, &patch);
    let mut independent = before.clone();
    json_patch::patch(&mut independent, &patch).unwrap();
    assert_eq!(independent, after);
    assert!(serde_json::to_vec(&patch).unwrap().len() < serde_json::to_vec(&raw).unwrap().len());
    assert!(
        patch
            .0
            .iter()
            .any(|op| matches!(op, PatchOperation::Test(op)
        if op.path.as_str() == "/items" && op.value == before["items"]))
    );
    assert!(
        !patch
            .0
            .iter()
            .any(|op| matches!(op, PatchOperation::Replace(op)
        if op.path.as_str().is_empty()))
    );
    let mut wrong = before.clone();
    wrong["items"][0]["value"] = json!(-99);
    let original = wrong.clone();
    assert!(palim::apply_json_patch_in_place(&mut wrong, &patch).is_err());
    assert_eq!(wrong, original);
}

#[test]
fn guards_array_insertions_and_missing_object_properties() {
    let before = json!({"items": [1], "name": "old"});
    let after = json!({"items": [1, 2], "name": "new", "added": true});
    let patch = diff_json_patch(&before, &after, &options(false, false, true)).unwrap();
    assert!(
        patch
            .0
            .iter()
            .any(|op| matches!(op, PatchOperation::Test(_)))
    );
    assert!(
        !patch
            .0
            .iter()
            .any(|op| matches!(op, PatchOperation::Test(op)
        if op.path.as_str() == "/items/1"))
    );
    assert_roundtrip(&before, &after, &patch);
    assert!(apply_json_patch(&json!({"items": [9], "name": "old"}), &patch).is_err());
    assert!(apply_json_patch(&json!({"items": [1], "name": "old", "added": 123}), &patch).is_err());

    let unguarded = diff_json_patch(&before, &after, &options(false, false, false)).unwrap();
    assert!(
        !unguarded
            .0
            .iter()
            .any(|op| matches!(op, PatchOperation::Test(_)))
    );
}

#[test]
fn guards_move_sources_and_copy_sources() {
    let payload = "payload ".repeat(100);
    let before = json!({"source": payload, "keep": true});
    let after = json!({"target": payload, "keep": true});
    let patch = diff_json_patch(&before, &after, &options(true, false, true)).unwrap();
    assert!(
        patch
            .0
            .iter()
            .any(|op| matches!(op, PatchOperation::Test(op)
        if op.path.as_str() == "/source" && op.value == before["source"]))
    );
    assert_roundtrip(&before, &after, &patch);
    assert!(apply_json_patch(&json!({"source": "wrong", "keep": true}), &patch).is_err());
}

#[test]
fn inverts_all_six_operations_with_overwrites_and_append() {
    let before = json!({"a": {"n": 1}, "b": {"n": 9}, "arr": [10, 20],
        "copy": {"old": true}, "keep": 0});
    let patch = parse(json!([
        {"op": "test", "path": "/a/n", "value": 1},
        {"op": "copy", "from": "/a", "path": "/copy"},
        {"op": "add", "path": "/arr/-", "value": 30},
        {"op": "move", "from": "/a", "path": "/b"},
        {"op": "replace", "path": "/keep", "value": 10},
        {"op": "remove", "path": "/arr/0"}
    ]));
    let after = apply_json_patch(&before, &patch).unwrap();
    assert_roundtrip(&before, &after, &patch);
}

#[test]
fn inverts_moves_into_ancestors_and_shifted_array_objects() {
    let cases = [
        (
            json!({"outer": {"inner": 1, "keep": 2}}),
            json!([{"op": "move", "from": "/outer/inner", "path": "/outer"}]),
        ),
        (
            json!({"a": {"n": 1}, "b": 2}),
            json!([{"op": "move", "from": "/a", "path": ""}]),
        ),
        (
            json!({"arr": [5, {"x": 10}, {"x": 20}]}),
            json!([{"op": "move", "from": "/arr/0", "path": "/arr/1/x"}]),
        ),
        (
            json!({"arr": [{"x": 0}, {"x": 10}, {"x": 20}]}),
            json!([{"op": "move", "from": "/arr/2/x", "path": "/arr/0"}]),
        ),
        (
            json!({"arr": [0, 1, 2]}),
            json!([{"op": "move", "from": "/arr/0", "path": "/arr/-"}]),
        ),
        (
            json!({"arr": [{"x": 0}, {"x": 10}, {"x": 20}]}),
            json!([{"op": "move", "from": "/arr/2/x", "path": "/arr/2"}]),
        ),
        (
            json!({"a/b": [0, {"x/y": [8]}, {"x/y": [1, 2]}]}),
            json!([{"op": "move", "from": "/a~1b/0", "path": "/a~1b/1/x~1y/-"}]),
        ),
        (
            json!({"a": 1}),
            json!([{"op": "move", "from": "/a", "path": "/a"}]),
        ),
    ];
    for (before, value) in cases {
        let patch = parse(value);
        let after = apply_json_patch(&before, &patch).unwrap();
        assert_roundtrip(&before, &after, &patch);
    }
}

#[test]
fn inverts_root_add_copy_replace_and_test_only_patches() {
    let before = json!({"a": {"n": 1}, "b": 2});
    for value in [
        json!([{"op": "add", "path": "", "value": [1, 2]}]),
        json!([{"op": "copy", "from": "/a", "path": ""}]),
        json!([{"op": "replace", "path": "", "value": null}]),
        json!([{"op": "test", "path": "/b", "value": 2}]),
    ] {
        let patch = parse(value);
        let after = apply_json_patch(&before, &patch).unwrap();
        assert_roundtrip(&before, &after, &patch);
    }
}

#[test]
fn inversion_preserves_original_error_and_input() {
    let before = json!({"a": 1});
    let patch = parse(json!([
        {"op": "replace", "path": "/a", "value": 2},
        {"op": "remove", "path": "/missing"}
    ]));
    assert_eq!(
        invert_json_patch(&before, &patch).unwrap_err(),
        apply_json_patch(&before, &patch).unwrap_err()
    );
    assert_eq!(before, json!({"a": 1}));
}

#[test]
fn arbitrary_precision_numbers_survive_all_standard_diff_options_and_wire() {
    for (before_number, after_number) in [
        ("18446744073709551617", "18446744073709551618"),
        (
            "340282366920938463463374607431768211456000000000000000001",
            "340282366920938463463374607431768211456000000000000000002",
        ),
        (
            "-340282366920938463463374607431768211456000000000000000001",
            "-340282366920938463463374607431768211456000000000000000002",
        ),
        (
            "0.12345678901234567890123456789012345678901",
            "0.12345678901234567890123456789012345678902",
        ),
        ("1e+10000", "2e+10000"),
    ] {
        let old: Value = serde_json::from_str(before_number).unwrap();
        let new: Value = serde_json::from_str(after_number).unwrap();
        let before = json!({"keep": old, "changed": old});
        let after = json!({"keep": old, "changed": new, "added": [new]});
        for factorize in [false, true] {
            for rationalize in [false, true] {
                for tests in [false, true] {
                    let patch =
                        diff_json_patch(&before, &after, &options(factorize, rationalize, tests))
                            .unwrap();
                    let wire = serde_json::to_string(&patch).unwrap();
                    let decoded: Patch = serde_json::from_str(&wire).unwrap();
                    assert_eq!(decoded, patch);
                    assert_roundtrip(&before, &after, &decoded);
                    let inverse = invert_json_patch(&before, &decoded).unwrap();
                    let inverse_wire = serde_json::to_string(&inverse).unwrap();
                    let inverse_decoded: Patch = serde_json::from_str(&inverse_wire).unwrap();
                    assert_eq!(apply_json_patch(&after, &inverse_decoded).unwrap(), before);
                    if tests {
                        let wrong = json!({"keep": old, "changed": new});
                        assert!(apply_json_patch(&wrong, &decoded).is_err());
                    }
                }
            }
        }
    }
}

#[test]
fn arbitrary_precision_payloads_survive_move_copy_guards_and_inverse() {
    let large: Value = serde_json::from_str("18446744073709551617").unwrap();
    let payload = json!({"n": large, "padding": "payload ".repeat(100)});
    for (before, after, is_move) in [
        (json!({"source": payload}), json!({"target": payload}), true),
        (
            json!({"source": payload, "copies": []}),
            json!({"source": payload, "copies": [payload]}),
            false,
        ),
    ] {
        let patch = diff_json_patch(&before, &after, &options(true, false, true)).unwrap();
        assert!(patch.0.iter().any(|op| if is_move {
            matches!(op, PatchOperation::Move(_))
        } else {
            matches!(op, PatchOperation::Copy(_))
        }));
        let decoded: Patch = serde_json::from_str(&serde_json::to_string(&patch).unwrap()).unwrap();
        assert_roundtrip(&before, &after, &decoded);
    }

    let before: Value =
        serde_json::from_str("{\"n\":18446744073709551617,\"copy\":18446744073709551617}").unwrap();
    let patch: Patch = serde_json::from_str(
        r#"[
        {"op":"test","path":"/n","value":18446744073709551617},
        {"op":"replace","path":"/n","value":18446744073709551618},
        {"op":"copy","from":"/n","path":"/copy"},
        {"op":"add","path":"/added","value":18446744073709551617},
        {"op":"move","from":"/copy","path":"/moved"},
        {"op":"remove","path":"/added"}
    ]"#,
    )
    .unwrap();
    let after = apply_json_patch(&before, &patch).unwrap();
    assert_roundtrip(&before, &after, &patch);
}

fn arb_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        (-20i64..20).prop_map(|v| json!(v)),
        "[a-z~/]{0,16}".prop_map(Value::String),
    ];
    leaf.prop_recursive(3, 40, 8, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
            prop::collection::btree_map("[a-c~/]{1,4}", inner, 0..6)
                .prop_map(|m| Value::Object(m.into_iter().collect()))
        ]
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256,
        rng_seed: proptest::test_runner::RngSeed::Fixed(20260930),
        ..ProptestConfig::default() })]
    #[test]
    fn optimized_and_guarded_patches_restore_random_documents(before in arb_json(), after in arb_json()) {
        for factorize in [false, true] {
            for rationalize in [false, true] {
                for tests in [false, true] {
                    let patch = diff_json_patch(&before, &after, &options(factorize, rationalize, tests))?;
                    let result = apply_json_patch(&before, &patch)?;
                    prop_assert_eq!(&result, &after);
                    let inverse = invert_json_patch(&before, &patch)?;
                    prop_assert_eq!(apply_json_patch(&after, &inverse)?, before.clone());
                    let mut independent = before.clone();
                    json_patch::patch(&mut independent, &patch)?;
                    prop_assert_eq!(independent, after.clone());
                }
            }
        }
    }

    #[test]
    fn inverses_restore_random_standard_operation_sequences(
        steps in prop::collection::vec((0u8..9, 0usize..100, -100i64..100), 0..60)
    ) {
        let before = json!({"a": [0, 1, 2], "b": [10, 11], "values": {"x": 1, "y": 2}});
        let mut current = before.clone();
        let mut operations = Vec::new();
        for (action, index, value) in steps {
            let a_len = current["a"].as_array().unwrap().len();
            let b_len = current["b"].as_array().unwrap().len();
            let op = match action {
                0 => json!({"op": "add", "path": format!("/a/{}", index % (a_len + 1)), "value": value}),
                1 if a_len > 0 => json!({"op": "remove", "path": format!("/a/{}", index % a_len)}),
                2 if a_len > 0 => json!({"op": "replace", "path": format!("/a/{}", index % a_len), "value": value}),
                3 if a_len > 0 => json!({"op": "move", "from": format!("/a/{}", index % a_len),
                    "path": format!("/b/{}", index % (b_len + 1))}),
                4 if a_len > 0 => json!({"op": "move", "from": format!("/a/{}", index % a_len),
                    "path": format!("/a/{}", (index + 3) % a_len)}),
                5 if a_len > 0 => json!({"op": "copy", "from": format!("/a/{}", index % a_len),
                    "path": "/b/-"}),
                6 if current["values"].get("x").is_some() =>
                    json!({"op": "copy", "from": "/values/x", "path": "/values/y"}),
                7 if current["values"].get("x").is_some() =>
                    json!({"op": "move", "from": "/values/x", "path": "/values/y"}),
                8 if current["values"].get("x").is_some() =>
                    json!({"op": "test", "path": "/values/x", "value": current["values"]["x"]}),
                _ => json!({"op": "add", "path": "/values/x", "value": value}),
            };
            let single = parse(json!([op]));
            current = apply_json_patch(&current, &single)?;
            operations.extend(single.0);
        }
        let patch = Patch(operations);
        let mut independent = before.clone();
        json_patch::patch(&mut independent, &patch)?;
        prop_assert_eq!(&independent, &current);
        let inverse = invert_json_patch(&before, &patch)?;
        prop_assert_eq!(apply_json_patch(&current, &inverse)?, before.clone());
        json_patch::patch(&mut independent, &inverse)?;
        prop_assert_eq!(independent, before);
    }
}
