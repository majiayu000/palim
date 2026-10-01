use palim::{Delta, DiffOptions, DiffPatcher, apply_json_patch, patch, reverse};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use proptest::prelude::*;
use serde_json::{Map, Value, json};

fn unicode() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            any::<char>(),
            prop::sample::select(vec!['😀', '🦀', '%', '\n', '\0', '界'])
        ],
        0..24,
    )
    .prop_map(|chars| chars.into_iter().collect())
}

fn document() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| json!(n)),
        any::<u64>().prop_map(|n| json!(n)),
        unicode().prop_map(Value::String),
    ]
    .prop_recursive(3, 64, 6, |child| {
        prop_oneof![
            proptest::collection::vec(child.clone(), 0..6).prop_map(Value::Array),
            proptest::collection::btree_map("[a-z0-9_~/]{0,5}", child, 0..6)
                .prop_map(|entries| Value::Object(entries.into_iter().collect())),
        ]
    })
}

fn wire_round_trip(delta: &Delta) -> Delta {
    let wire = serde_json::to_string(delta).unwrap();
    let decoded: Delta = serde_json::from_str(&wire).unwrap();
    assert_eq!(&decoded, delta);
    decoded
}

// An accepted, baseline-independent delta may still be inapplicable to this
// document. Error is legitimate; a successful inverse must remain a valid Delta.
fn exercise_supplied_wire(wire: Value, baseline: &Value) {
    if let Ok(delta) = Delta::from_value(wire) {
        let delta = wire_round_trip(&delta);
        let inverse = reverse(&delta)
            .ok()
            .map(|inverse| wire_round_trip(&inverse));
        if let (Ok(target), Some(inverse)) = (patch(baseline, &delta), inverse) {
            assert_eq!(patch(&target, &inverse).unwrap(), *baseline);
        }
        let _ = delta.to_json_patch(baseline);
        let _ = patch(&json!({"wrong/type": true}), &delta);
    }
}

fn check_complete_delta(delta: &Delta, before: &Value, after: &Value) {
    let decoded = wire_round_trip(delta);
    assert_eq!(patch(before, &decoded).unwrap(), *after);
    let inverse = wire_round_trip(&reverse(&decoded).unwrap());
    assert_eq!(patch(after, &inverse).unwrap(), *before);
    assert_eq!(
        apply_json_patch(before, &decoded.to_json_patch(before).unwrap()).unwrap(),
        *after
    );
}

fn coordinate(start: usize, length: usize) -> String {
    match length {
        0 => format!("{start},0"),
        1 => (start + 1).to_string(),
        _ => format!("{},{length}", start + 1),
    }
}

fn encoded(text: &str) -> String {
    // diff-match-patch uses encodeURI (then renders spaces literally), not
    // encodeURIComponent: URI syntax characters must remain unescaped.
    const URI: &percent_encoding::AsciiSet = &NON_ALPHANUMERIC
        .remove(b';')
        .remove(b'/')
        .remove(b'?')
        .remove(b':')
        .remove(b'@')
        .remove(b'&')
        .remove(b'=')
        .remove(b'+')
        .remove(b'$')
        .remove(b',')
        .remove(b'#')
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'!')
        .remove(b'~')
        .remove(b'*')
        .remove(b'\'')
        .remove(b'(')
        .remove(b')')
        .remove(b' ');
    utf8_percent_encode(text, URI).to_string()
}

#[test]
fn accepted_text_coordinates_have_serializable_inverses() {
    let maximum = usize::MAX;
    let text = format!("@@ -{maximum},0 +{maximum},0 @@\n-a\n+b\n");
    if let Ok(inverse) = Delta::from_value(json!([text, 0, 2])).and_then(|delta| reverse(&delta)) {
        let wire = serde_json::to_string(&inverse).unwrap();
        serde_json::from_str::<Delta>(&wire).unwrap();
    }
}

#[test]
fn fuzzy_replacement_consumes_a_substituted_last_character() {
    use palim::{TextPatchOptions, apply_text_patch};
    let text = "@@ -1,3 +1,3 @@\n-abc\n+XYZ\n";
    let options = TextPatchOptions {
        max_distance: 0,
        max_error_ratio: 0.34,
    };
    assert_eq!(apply_text_patch("abx", text, &options).unwrap(), "XYZ");
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 2048,
        rng_seed: proptest::test_runner::RngSeed::Fixed(20261001),
        ..ProptestConfig::default()
    })]

    #[test]
    fn supplied_array_move_delete_add_and_edit(
        input in proptest::collection::vec(any::<i16>(), 2..30),
        raw_from in any::<usize>(), raw_move in any::<usize>(), raw_add in any::<usize>(),
        added in any::<i16>(), edited in any::<i16>(), include_move in any::<bool>(),
    ) {
        let from = raw_from % input.len();
        let removed = (from + 1) % input.len();
        let move_before_add = raw_move % (input.len() - 1);
        let addition = raw_add % input.len();
        let moved_target = move_before_add + usize::from(addition <= move_before_add);

        let mut expected = input.iter().enumerate()
            .filter(|(i, _)| *i != from && *i != removed)
            .map(|(_, value)| *value).collect::<Vec<_>>();
        expected.insert(move_before_add, input[from]);
        expected.insert(addition, added);
        expected[moved_target] = edited;

        let mut wire = Map::new();
        wire.insert("_t".into(), json!("a"));
        wire.insert(format!("_{from}"), json!([
            if include_move { json!(input[from]) } else { json!("") }, moved_target, 3
        ]));
        wire.insert(format!("_{removed}"), json!([input[removed], 0, 0]));
        wire.insert(addition.to_string(), json!([added]));
        wire.insert(moved_target.to_string(), json!([input[from], edited]));
        let delta = Delta::from_value(Value::Object(wire)).unwrap();
        check_complete_delta(&delta, &json!(input), &json!(expected));
        prop_assert!(patch(&json!({}), &delta).is_err(), "an array delta rejects an object baseline");
    }

    #[test]
    fn generated_documents_keep_complete_protocol_invariants(
        before in document(), after in document(),
        position in any::<bool>(), moves in any::<bool>(), include_move in any::<bool>(),
    ) {
        let engine = DiffPatcher::new(DiffOptions {
            match_by_position: position,
            detect_moves: moves,
            include_value_on_move: include_move,
            text_diff_min_length: Some(0),
            ..Default::default()
        });
        if let Some(delta) = engine.diff(&before, &after).unwrap() {
            check_complete_delta(&delta, &before, &after);
            prop_assert_eq!(delta.to_forward_only().patch(&before).unwrap(), after);
        } else {
            prop_assert_eq!(before, after);
        }
    }
}

fn extreme_index() -> impl Strategy<Value = usize> {
    prop_oneof![
        0usize..80,
        Just(usize::MAX),
        Just(usize::MAX - 1),
        Just(usize::MAX / 2),
        any::<usize>(),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 2048,
        rng_seed: proptest::test_runner::RngSeed::Fixed(20261002),
        ..ProptestConfig::default()
    })]

    #[test]
    fn array_protocol_mutations_never_escape_validated_type(
        baseline in proptest::collection::vec(any::<i16>(), 0..32),
        source in extreme_index(), target in extreme_index(), value in any::<i16>(),
        variant in 0u8..9,
    ) {
        let mut wire = Map::new();
        wire.insert("_t".into(), json!("a"));
        match variant {
            0 => { wire.insert(format!("_{source}"), json!(["", target, 3])); }
            1 => { wire.insert(format!("_{source}"), json!([value, 0, 0])); }
            2 => { wire.insert(target.to_string(), json!([value, value.wrapping_add(1)])); }
            3 => { wire.insert(target.to_string(), json!([value])); }
            4 => {
                wire.insert("_0".into(), json!(["", target, 3]));
                wire.insert("_1".into(), json!(["", target, 3]));
            }
            5 => {
                wire.insert(format!("_{source}"), json!(["", target, 3]));
                wire.insert(target.to_string(), json!([value]));
            }
            6 => { wire.insert(format!("0{target}"), json!([value])); }
            7 => { wire.insert(format!("_{source}"), json!({"nested": [value]})); }
            _ => { wire.insert(target.to_string(), json!([value, 0, 0])); }
        }
        let wire = Value::Object(wire);
        if variant >= 4 {
            prop_assert!(Delta::from_value(wire).is_err());
        } else {
            exercise_supplied_wire(wire, &json!(baseline));
        }
    }

    #[test]
    fn supplied_unicode_text_and_protocol_mutations(
        old in unicode(), new in unicode(), prefix in unicode(), suffix in unicode(),
        variant in 0u8..10, huge in extreme_index(),
    ) {
        let start = prefix.encode_utf16().count();
        let old_length = old.encode_utf16().count();
        let new_length = new.encode_utf16().count();
        let header = format!("@@ -{} +{} @@\n", coordinate(start, old_length), coordinate(start, new_length));
        let operations = format!("-{}\n+{}\n", encoded(&old), encoded(&new));
        let text = format!("{header}{operations}");
        let before = json!(format!("{prefix}{old}{suffix}"));
        let after = json!(format!("{prefix}{new}{suffix}"));
        let delta = Delta::from_value(json!([text, 0, 2])).unwrap();
        check_complete_delta(&delta, &before, &after);
        prop_assert!(patch(&json!(0), &delta).is_err());

        let mutated = match variant {
            0 => format!("@@ ?{}", &text[4..]),
            1 => format!("{header}-%\n+x\n"),
            2 => format!("{header}-%GG\n+x\n"),
            3 => format!("{header}-%ED%A0%80\n+x\n"),
            4 => "@@ -0,1 +0,1 @@\n-a\n+b\n".to_owned(),
            5 => format!("@@ -{} +{} @@\n{operations}",
                coordinate(start, old_length + 1), coordinate(start, new_length)),
            6 => format!("@@ -{huge},0 +{huge},0 @@\n-a\n+b\n"),
            7 => format!("{header}?unknown\n"),
            8 => format!("@@ -{} +{} @@\n{operations}",
                coordinate(start, old_length + 1), coordinate(start, new_length + 1)),
            _ => format!("@@ -{huge},2 +{huge},2 @@\n-aa\n+bb\n"),
        };
        let wire = json!([mutated, 0, 2]);
        if variant <= 5 || variant == 7 {
            prop_assert!(Delta::from_value(wire).is_err());
        } else {
            exercise_supplied_wire(wire, &before);
        }
    }
}
