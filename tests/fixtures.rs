use palim::{Delta, DiffOptions, DiffPatcher, Patch, apply_json_patch, patch, reverse};
use serde_json::Value;
use std::sync::Arc;

#[test]
fn saved_373_document_pairs() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/correctness.json")).unwrap();
    assert_eq!(cases.len(), 373);
    let dp = DiffPatcher::new(DiffOptions {
        object_hash: Some(Arc::new(|v, _| v.get("id").map(Value::to_string))),
        ..Default::default()
    });
    for case in cases {
        let (a, b) = (&case["left"], &case["right"]);
        if let Some(d) = dp.diff(a, b).unwrap() {
            let decoded: Delta = serde_json::from_value(d.as_value().clone()).unwrap();
            assert_eq!(patch(a, &decoded).unwrap(), *b, "{}", case["name"]);
            assert_eq!(
                patch(b, &reverse(&decoded).unwrap()).unwrap(),
                *a,
                "{}",
                case["name"]
            );
            assert_eq!(
                apply_json_patch(a, &d.to_json_patch(a).unwrap()).unwrap(),
                *b,
                "{}",
                case["name"]
            );
        } else {
            assert_eq!(a, b);
        }
    }
}

#[test]
fn rfc_6902_public_success_and_error_cases() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/json-patch-tests.json")).unwrap();
    let mut count = 0;
    for case in cases {
        if case["disabled"] == true || case.get("doc").is_none() || case.get("patch").is_none() {
            continue;
        }
        let input = &case["doc"];
        let decoded = serde_json::from_value::<Patch>(case["patch"].clone());
        let result = decoded
            .map_err(|e| e.to_string())
            .and_then(|p| apply_json_patch(input, &p).map_err(|e| e.to_string()));
        if case.get("error").is_some() {
            assert!(result.is_err(), "{}", case["comment"]);
        } else if let Some(expected) = case.get("expected") {
            assert_eq!(result.unwrap(), *expected, "{}", case["comment"]);
        } else {
            assert!(result.is_ok(), "{}", case["comment"]);
        }
        count += 1;
    }
    assert_eq!(count, 92);
}
