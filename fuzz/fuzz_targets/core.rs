#![no_main]

use arbitrary::Unstructured;
use palim::{
    CompareOptions, Delta, DiffOptions, DiffPatcher, JsonPatchOptions, Patch, PatchOperation,
    TextPatchOptions, apply_json_patch, apply_text_patch, compare, diff_json_patch,
    invert_json_patch, patch, reverse, unpatch,
};
use libfuzzer_sys::fuzz_target;
use serde_json::{Map, Value, json};
use std::{fmt::Write, sync::Arc};

fn byte(input: &mut Unstructured<'_>) -> u8 {
    input.arbitrary().unwrap_or(0)
}

fn index(input: &mut Unstructured<'_>) -> usize {
    match byte(input) % 5 {
        0 => usize::MAX,
        1 => usize::MAX - 1,
        2 => usize::MAX / 2,
        _ => usize::from(byte(input)),
    }
}

fn text(input: &mut Unstructured<'_>) -> String {
    const CHARACTERS: &[char] = &[
        'a', 'b', 'c', ' ', '/', '~', '%', ':', '?', '+', '\n', '\r', '\0', '界', 'é', '😀', '🦀',
        '𝄞',
    ];
    let count = byte(input) % 25;
    (0..count)
        .map(|_| {
            let choice = byte(input);
            if choice % 3 == 0 {
                input.arbitrary::<char>().unwrap_or('a')
            } else {
                CHARACTERS[usize::from(choice) % CHARACTERS.len()]
            }
        })
        .collect()
}

fn number(input: &mut Unstructured<'_>) -> Value {
    const LITERALS: &[&str] = &[
        "1.0",
        "-0.0",
        "9007199254740993",
        "18446744073709551617",
        "340282366920938463463374607431768211455",
        "1e1000000000",
        "10e999999999",
        "1e999999999999999999999999",
        "10e999999999999999999999998",
        "1e-10000",
        "398043487186.00244",
        "-123.4500",
    ];
    match byte(input) % 3 {
        0 => json!(input.arbitrary::<i64>().unwrap_or(0)),
        1 => json!(input.arbitrary::<u64>().unwrap_or(0)),
        _ => serde_json::from_str(LITERALS[usize::from(byte(input)) % LITERALS.len()]).unwrap(),
    }
}

// Depth, fanout, and strings are bounded independently of libFuzzer's byte limit.
fn document(input: &mut Unstructured<'_>, depth: u8) -> Value {
    let choice = byte(input) % if depth == 0 { 5 } else { 7 };
    match choice {
        0 => Value::Null,
        1 => json!(byte(input) & 1 != 0),
        2 | 3 => number(input),
        4 => json!(text(input)),
        5 => Value::Array(
            (0..byte(input) % 7)
                .map(|_| document(input, depth - 1))
                .collect(),
        ),
        _ => {
            const KEYS: &[&str] = &["", "id", "value", "_t", "a/b", "~", "0", "ignored"];
            let mut values = Map::new();
            for _ in 0..byte(input) % 5 {
                let key = KEYS[usize::from(byte(input)) % KEYS.len()];
                values.insert(key.into(), document(input, depth - 1));
            }
            Value::Object(values)
        }
    }
}

fn decode_round_trip(delta: &Delta) -> Delta {
    let decoded: Delta = serde_json::from_slice(&serde_json::to_vec(delta).unwrap()).unwrap();
    assert_eq!(&decoded, delta);
    decoded
}

fn complete_delta(before: &Value, after: &Value, delta: &Delta) {
    let decoded = decode_round_trip(delta);
    assert_eq!(patch(before, &decoded).unwrap(), *after);
    assert_eq!(unpatch(after, &decoded).unwrap(), *before);
    let inverse = decode_round_trip(&reverse(&decoded).unwrap());
    assert_eq!(patch(after, &inverse).unwrap(), *before);
    assert_eq!(
        apply_json_patch(before, &decoded.to_json_patch(before).unwrap()).unwrap(),
        *after
    );
    assert_eq!(decoded.to_forward_only().patch(before).unwrap(), *after);
}

fn supplied_delta(wire: Value, before: &Value) {
    if let Ok(delta) = Delta::from_value(wire) {
        let decoded = decode_round_trip(&delta);
        if let Ok(inverse) = reverse(&decoded) {
            decode_round_trip(&inverse);
        }
        if let Ok(after) = patch(before, &decoded) {
            // A root insertion cannot retain an absent root in JSON; its
            // inverse is deliberately inapplicable to an existing document.
            if !decoded
                .as_value()
                .as_array()
                .is_some_and(|fields| fields.len() == 1)
            {
                complete_delta(before, &after, &decoded);
            }
        }
        let _ = decoded.to_json_patch(before);
        let _ = patch(&json!({"wrong/type": true}), &decoded);
    }
}

fn native(input: &mut Unstructured<'_>) {
    let before = document(input, 3);
    let after = document(input, 3);
    let flags = byte(input);
    let engine = DiffPatcher::new(DiffOptions {
        match_by_position: flags & 1 != 0,
        detect_moves: flags & 2 != 0,
        include_value_on_move: flags & 4 != 0,
        text_diff_min_length: Some(0),
        object_hash: (flags & 8 != 0).then(|| {
            Arc::new(|value: &Value, _: usize| value.get("id").map(Value::to_string)) as _
        }),
        ..Default::default()
    });
    if let Some(delta) = engine.diff(&before, &after).unwrap() {
        complete_delta(&before, &after, &delta);
    } else {
        assert_eq!(before, after);
    }
}

fn arrays(input: &mut Unstructured<'_>) {
    let count = usize::from(byte(input) % 12) + 2;
    let before: Vec<_> = (0..count).map(|_| document(input, 1)).collect();
    let from = usize::from(byte(input)) % count;
    let removed = (from + 1) % count;
    let move_before_add = usize::from(byte(input)) % (count - 1);
    let addition = usize::from(byte(input)) % count;
    let moved_target = move_before_add + usize::from(addition <= move_before_add);
    let added = document(input, 1);
    let edited = document(input, 1);
    let mut after: Vec<_> = before
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != from && *i != removed)
        .map(|(_, value)| value.clone())
        .collect();
    after.insert(move_before_add, before[from].clone());
    after.insert(addition, added.clone());
    after[moved_target] = edited.clone();
    let mut wire = Map::new();
    wire.insert("_t".into(), json!("a"));
    wire.insert(
        format!("_{from}"),
        json!([
            if byte(input) & 1 != 0 {
                before[from].clone()
            } else {
                json!("")
            },
            moved_target,
            3
        ]),
    );
    wire.insert(format!("_{removed}"), json!([before[removed], 0, 0]));
    wire.insert(addition.to_string(), json!([added]));
    wire.insert(moved_target.to_string(), json!([before[from], edited]));
    let delta = Delta::from_value(Value::Object(wire.clone())).unwrap();
    complete_delta(&json!(before), &json!(after), &delta);

    let source = index(input);
    let target = index(input);
    match byte(input) % 5 {
        0 => {
            wire.insert(format!("_{source}"), json!(["", target, 3]));
        }
        1 => {
            wire.insert(target.to_string(), json!([0, 1]));
        }
        2 => {
            wire.insert(target.to_string(), json!([0]));
        }
        3 => {
            wire.insert(format!("0{target}"), json!([0]));
        }
        _ => {
            wire.insert(format!("_{source}"), json!({"invalid": [0]}));
        }
    }
    supplied_delta(Value::Object(wire), &json!(before));
}

fn coordinate(start: usize, length: usize) -> String {
    match length {
        0 => format!("{start},0"),
        1 => (start + 1).to_string(),
        _ => format!("{},{length}", start + 1),
    }
}

fn encode(text: &str) -> String {
    let mut encoded = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'();,/?:@&=+$# ".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            write!(&mut encoded, "%{byte:02X}").unwrap();
        }
    }
    encoded
}

fn texts(input: &mut Unstructured<'_>) {
    let old = text(input);
    let new = text(input);
    let prefix = text(input);
    let suffix = text(input);
    let start = prefix.encode_utf16().count();
    let old_length = old.encode_utf16().count();
    let new_length = new.encode_utf16().count();
    let header = format!(
        "@@ -{} +{} @@\n",
        coordinate(start, old_length),
        coordinate(start, new_length)
    );
    let operations = format!("-{}\n+{}\n", encode(&old), encode(&new));
    let text = format!("{header}{operations}");
    let before = json!(format!("{prefix}{old}{suffix}"));
    let after = json!(format!("{prefix}{new}{suffix}"));
    let delta = Delta::from_value(json!([text, 0, 2])).unwrap();
    complete_delta(&before, &after, &delta);
    assert_eq!(
        apply_text_patch(
            before.as_str().unwrap(),
            &text,
            &TextPatchOptions {
                max_distance: 0,
                max_error_ratio: 0.0
            }
        )
        .unwrap(),
        after.as_str().unwrap()
    );
    let maximum = index(input);
    let mutated = match byte(input) % 6 {
        0 => format!("{header}-%\n+x\n"),
        1 => format!("{header}-%ED%A0%80\n+x\n"),
        2 => format!("@@ -{maximum},0 +{maximum},0 @@\n-a\n+b\n"),
        3 => format!(
            "@@ -{} +{} @@\n{operations}",
            coordinate(start, old_length + 1),
            coordinate(start, new_length)
        ),
        4 => format!("{header}?unknown\n"),
        _ => format!("@@ -{maximum},2 +{maximum},2 @@\n-aa\n+bb\n"),
    };
    supplied_delta(json!([mutated, 0, 2]), &before);
}

fn standard(input: &mut Unstructured<'_>) {
    let before = json!({"items": [document(input, 1), document(input, 1)], "x": document(input, 2), "a/b": {"~": 1}});
    const PATHS: &[&str] = &[
        "", "/items", "/items/0", "/items/-", "/x", "/missing", "/a~1b/~0",
    ];
    const KINDS: &[&str] = &["add", "remove", "replace", "move", "copy", "test"];
    let mut operations = Vec::new();
    for _ in 0..byte(input) % 7 {
        let kind = KINDS[usize::from(byte(input)) % KINDS.len()];
        let path = PATHS[usize::from(byte(input)) % PATHS.len()];
        let from = PATHS[usize::from(byte(input)) % PATHS.len()];
        // Decode only a null placeholder, then assign the owned payload. This
        // avoids upstream serde's Content buffer's known u128 decoding limitation.
        let raw = json!({"op": kind, "path": path, "from": from, "value": null});
        let mut operation: PatchOperation = serde_json::from_value(raw).unwrap();
        let value = document(input, 2);
        match &mut operation {
            PatchOperation::Add(op) => op.value = value,
            PatchOperation::Replace(op) => op.value = value,
            PatchOperation::Test(op) => op.value = value,
            _ => {}
        }
        operations.push(operation);
    }
    let operations = Patch(operations);
    if let Ok(after) = apply_json_patch(&before, &operations) {
        let inverse = invert_json_patch(&before, &operations).unwrap();
        assert_eq!(apply_json_patch(&after, &inverse).unwrap(), before);
    }
    let after = document(input, 3);
    let flags = byte(input);
    let operations = diff_json_patch(
        &before,
        &after,
        &JsonPatchOptions {
            factorize: flags & 1 != 0,
            rationalize: flags & 2 != 0,
            tests: flags & 4 != 0,
        },
    )
    .unwrap();
    let target = apply_json_patch(&before, &operations).unwrap();
    // Standard optimization uses mathematical number equality. The inverse
    // restores the actual baseline, even if a numeric representation is retained.
    assert!(
        compare(&target, &after, &CompareOptions::default())
            .unwrap()
            .differences
            .is_empty()
    );
    assert_eq!(
        apply_json_patch(&target, &invert_json_patch(&before, &operations).unwrap()).unwrap(),
        before
    );
}

fn reports(input: &mut Unstructured<'_>) {
    let before = document(input, 3);
    let after = document(input, 3);
    let flags = byte(input);
    let mut options = CompareOptions {
        absolute_tolerance: if flags & 1 != 0 { 1.0 } else { 0.0 },
        relative_tolerance: if flags & 2 != 0 { 0.01 } else { 0.0 },
        unordered: (flags & 4 != 0).then(|| Arc::new(|_: &str| true) as _),
        max_comparisons: Some(4096),
        ..Default::default()
    };
    if let Ok(report) = compare(&before, &after, &options) {
        assert!(report.similarity.is_finite() && (0.0..=1.0).contains(&report.similarity));
        let _: palim::CompareReport =
            serde_json::from_slice(&serde_json::to_vec(&report).unwrap()).unwrap();
    }
    options.node_filter = Some(Arc::new(|_, _, _| false));
    let excluded = compare(&before, &after, &options).unwrap();
    assert!(excluded.differences.is_empty() && excluded.moves.is_empty());
    assert_eq!(excluded.similarity, 1.0);
}

// A one-byte seed selects each existing regression. The remaining input is
// still used by the generated modes, and libFuzzer mutates both selectors/data.
fn regressions(input: &mut Unstructured<'_>) {
    match byte(input) % 7 {
        0 => supplied_delta(
            json!([
                format!("@@ -{},0 +{},0 @@\n-a\n+b\n", usize::MAX, usize::MAX),
                0,
                2
            ]),
            &json!("a"),
        ),
        1 => {
            let delta = Delta::from_value(json!(["@@ -0,0 +1,3 @@\n+%2f\n", 0, 2])).unwrap();
            complete_delta(&json!(""), &json!("%2f"), &delta);
        }
        2 => {
            let before = json!([[398043487186.00244]]);
            complete_delta(
                &before,
                &Value::Null,
                &palim::diff(&before, &Value::Null)
                    .unwrap()
                    .unwrap(),
            );
        }
        3 => {
            let before = json!([{"id":1,"v":10}, {"id":2,"v":20}]);
            let after = json!([{"id":2,"v":20}, {"id":1,"v":11}]);
            let delta =
                Delta::from_value(json!({"_t":"a", "_0":[{"id":1,"v":10},1,3], "1":{"v":[10,11]}}))
                    .unwrap();
            complete_delta(&before, &after, &delta);
        }
        4 => supplied_delta(
            json!({"_t":"a", usize::MAX.to_string():[0,1], "_0":[0,0,0]}),
            &json!([0]),
        ),
        5 => {
            assert_eq!(
                apply_text_patch(
                    "abx",
                    "@@ -1,3 +1,3 @@\n-abc\n+XYZ\n",
                    &TextPatchOptions {
                        max_distance: 0,
                        max_error_ratio: 0.34
                    }
                )
                .unwrap(),
                "XYZ"
            );
        }
        _ => {
            let before: Value = serde_json::from_str("1e999999999999999999999999").unwrap();
            let after: Value = serde_json::from_str("10e999999999999999999999998").unwrap();
            assert!(
                compare(&before, &after, &CompareOptions::default())
                    .unwrap()
                    .differences
                    .is_empty()
            );
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let mut input = Unstructured::new(data);
    match byte(&mut input) % 6 {
        0 => native(&mut input),
        1 => arrays(&mut input),
        2 => texts(&mut input),
        3 => standard(&mut input),
        4 => reports(&mut input),
        _ => regressions(&mut input),
    }
});
