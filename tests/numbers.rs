use palim::{CompareOptions, Delta, Patch, apply_json_patch, compare, diff, patch, reverse};
use serde_json::{Value, json};
use std::sync::Arc;

fn value(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

fn standard_test(left: &Value, expected: &Value) -> Result<Value, palim::Error> {
    let wire = serde_json::to_string(&json!([
        {"op": "test", "path": "", "value": expected}
    ]))
    .unwrap();
    let operations: Patch = serde_json::from_str(&wire).unwrap();
    apply_json_patch(left, &operations)
}

#[test]
fn arbitrary_precision_values_round_trip_through_native_delta() {
    for (before, after) in [
        ("18446744073709551617", "18446744073709551618"),
        (
            "184467440737095516160000000000000000000000000000001",
            "184467440737095516160000000000000000000000000000002",
        ),
        (
            "-340282366920938463463374607431768211456",
            "-340282366920938463463374607431768211457",
        ),
        (
            "0.12345678901234567890123456789012345678901",
            "0.12345678901234567890123456789012345678902",
        ),
        ("1e+10000", "2e+10000"),
    ] {
        let before_value = value(before);
        let after_value = value(after);
        assert_eq!(serde_json::to_string(&before_value).unwrap(), before);
        assert_eq!(serde_json::to_string(&after_value).unwrap(), after);
        assert_ne!(before_value, after_value, "distinct values must not round");
        let delta = diff(&before_value, &after_value).unwrap().unwrap();
        let decoded: Delta = serde_json::from_str(&serde_json::to_string(&delta).unwrap()).unwrap();
        assert_eq!(patch(&before_value, &decoded).unwrap(), after_value);
        let standard = decoded.to_json_patch(&before_value).unwrap();
        assert_eq!(
            apply_json_patch(&before_value, &standard).unwrap(),
            after_value
        );
        assert_eq!(
            patch(&after_value, &reverse(&decoded).unwrap()).unwrap(),
            before_value
        );
    }
}

#[test]
fn standard_tests_and_reports_compare_exact_decimal_values() {
    for (left, right) in [
        ("1e10000", "10e9999"),
        ("1E+10000", "1000e9997"),
        ("-1.2300e-10000", "-123e-10002"),
        ("184467440737095516160000", "1.84467440737095516160000e23"),
        ("0", "-0.00000e999999999999999999999999999999999999999999"),
        ("0.0012300", "123e-5"),
    ] {
        let left = value(left);
        let right = value(right);
        assert_eq!(standard_test(&left, &right).unwrap(), left);
        assert!(
            compare(&left, &right, &CompareOptions::default())
                .unwrap()
                .differences
                .is_empty()
        );
    }
    for (left, right) in [
        (
            "184467440737095516160000000000001",
            "184467440737095516160000000000002",
        ),
        (
            "0.1234567890123456789012345678901",
            "0.1234567890123456789012345678902",
        ),
        ("1e10000", "1e10001"),
        ("1e-10000", "2e-10000"),
        ("-1e10000", "1e10000"),
    ] {
        let left = value(left);
        let right = value(right);
        assert!(standard_test(&left, &right).is_err());
        assert_eq!(
            compare(&left, &right, &CompareOptions::default())
                .unwrap()
                .differences
                .len(),
            1
        );
    }
}

#[test]
fn enormous_exponents_are_compared_without_expanding_the_number() {
    let exponent = format!("1{}", "0".repeat(4096));
    let preceding = "9".repeat(4096);
    let left = value(&format!("1e{exponent}"));
    let right = value(&format!("10e{preceding}"));
    assert_eq!(standard_test(&left, &right).unwrap(), left);
    assert!(
        compare(&left, &right, &CompareOptions::default())
            .unwrap()
            .differences
            .is_empty()
    );
    let negative = value(&format!("1e-{exponent}"));
    let negative_equivalent = value(&format!("0.1e-{preceding}"));
    assert_eq!(
        standard_test(&negative, &negative_equivalent).unwrap(),
        negative
    );
}

#[test]
fn unordered_reports_use_the_same_exact_number_equality_as_standard_test() {
    let options = CompareOptions {
        unordered: Some(Arc::new(|_| true)),
        ..Default::default()
    };
    let left = value("[1e10000,1e-10000,184467440737095516160000000000001]");
    let right = value("[184467440737095516160000000000001,10e9999,0.1e-9999]");
    assert!(
        compare(&left, &right, &options)
            .unwrap()
            .differences
            .is_empty()
    );
    let different = value("[184467440737095516160000000000002,10e9999,0.1e-9999]");
    assert!(
        !compare(&left, &different, &options)
            .unwrap()
            .differences
            .is_empty()
    );
    let objects_left = value("[{\"n\":1e10000},{\"n\":1e-10000}]");
    let objects_right = value("[{\"n\":0.1e-9999},{\"n\":10e9999}]");
    assert!(
        compare(&objects_left, &objects_right, &options)
            .unwrap()
            .differences
            .is_empty()
    );
}

#[test]
fn tolerance_compares_arbitrary_precision_decimal_values() {
    let options = CompareOptions {
        absolute_tolerance: 0.1,
        ..Default::default()
    };
    for (left, right, expected_equal) in [
        ("1e10000", "2e10000", false),
        ("1e-10000", "2e-10000", true),
        (
            "1000000000000000000000000000001",
            "1000000000000000000000000000002",
            false,
        ),
    ] {
        let left = value(left);
        let right = value(right);
        assert_eq!(
            compare(&left, &right, &options)
                .unwrap()
                .differences
                .is_empty(),
            expected_equal
        );
    }
    assert!(
        compare(&value("1e10000"), &value("10e9999"), &options)
            .unwrap()
            .differences
            .is_empty()
    );
}

fn assert_tolerance(left: &str, right: &str, absolute: f64, relative: f64, expected: bool) {
    let options = CompareOptions {
        absolute_tolerance: absolute,
        relative_tolerance: relative,
        ..Default::default()
    };
    assert_eq!(
        compare(&value(left), &value(right), &options)
            .unwrap()
            .differences
            .is_empty(),
        expected,
        "{left} vs {right}, absolute={absolute}, relative={relative}",
    );
}

#[test]
fn absolute_tolerance_is_exact_at_decimal_boundaries_and_cancellation() {
    for (left, right, tolerance, equal) in [
        (
            "1000000000000000000000000000001",
            "1000000000000000000000000000002",
            1.0,
            true,
        ),
        (
            "1000000000000000000000000000001",
            "1000000000000000000000000000003",
            1.0,
            false,
        ),
        ("1.0000000000000002e30", "1e30", 2.1e14, true),
        ("1.0000000000000002e30", "1e30", 1.5e14, false),
        ("0.1", "0.3", 0.2, true),
        ("0.1", "0.300000000000000000000000000001", 0.2, false),
        ("-0.1", "0.1", 0.2, true),
        ("-0.1", "0.100000000000000000000000000001", 0.2, false),
        ("0", "1e-1000000000", 0.1, true),
        ("1", "1e-1000000000", 1.0, true),
        ("1", "-1e-1000000000", 1.0, false),
        (
            "-0.00000000000000000000000000000000000000000000000001",
            "0",
            1e-50,
            true,
        ),
    ] {
        assert_tolerance(left, right, tolerance, 0.0, equal);
        assert_tolerance(right, left, tolerance, 0.0, equal);
    }
}

#[test]
fn relative_tolerance_handles_huge_small_and_signed_numbers_exactly() {
    for (left, right, tolerance, equal) in [
        ("1e10000", "2e10000", 0.5, true),
        ("1e10000", "2e10000", 0.49999999999999994, false),
        ("1e-10000", "2e-10000", 0.5, true),
        ("-1e10000", "2e10000", 1.5, true),
        ("-1e10000", "2e10000", 1.4999999999999998, false),
        ("0", "1e1000000000", 1.0, true),
        ("0", "1e1000000000", 0.9999999999999999, false),
        ("1", "1e-1000000000", 1.0, true),
        ("1", "-1e-1000000000", 1.0, false),
        ("1.0000000000000002e30", "1e30", 2e-16, true),
        ("1.0000000000000002e30", "1e30", 1.5e-16, false),
    ] {
        assert_tolerance(left, right, 0.0, tolerance, equal);
        assert_tolerance(right, left, 0.0, tolerance, equal);
    }
    assert_tolerance("100", "101", 1.0, 0.001, true);
    assert_tolerance("100", "101", 0.01, 0.01, true);
}

#[test]
fn huge_exponent_tolerance_is_sparse_and_obeys_existing_work_budget() {
    let exponent = format!("1{}", "0".repeat(4096));
    let left = format!("1e{exponent}");
    let right = format!("2e{exponent}");
    assert_tolerance(&left, &right, 0.0, 0.5, true);
    assert_tolerance(&left, &right, 0.0, 0.49, false);
    assert_tolerance(&format!("1e-{exponent}"), "0", 0.1, 0.0, true);
    assert_tolerance("1", &format!("-1e-{exponent}"), 1.0, 0.0, false);
    let options = CompareOptions {
        relative_tolerance: 0.5,
        max_comparisons: Some(10),
        ..Default::default()
    };
    let left = value(&format!("{{\"amount\":{left}}}"));
    let right = value(&format!("{{\"amount\":{right}}}"));
    let error = compare(&left, &right, &options).unwrap_err();
    assert_eq!(error.path, "/amount");
    assert!(error.message.contains("max_comparisons"));
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config {
        cases: 512,
        rng_seed: proptest::test_runner::RngSeed::Fixed(20261001),
        ..Default::default()
    })]
    #[test]
    fn shifting_decimal_places_keeps_mathematical_value(
        coefficient in -1_000_000_000_000_i64..1_000_000_000_000,
        exponent in -10000_i32..10000,
        places in 0_u8..24,
    ) {
        let left = value(&format!("{coefficient}e{exponent}"));
        let right = if coefficient == 0 {
            value(&format!("0e{}", exponent - i32::from(places)))
        } else {
            value(&format!("{coefficient}{}e{}", "0".repeat(usize::from(places)), exponent - i32::from(places)))
        };
        proptest::prop_assert_eq!(standard_test(&left, &right).unwrap(), left.clone());
        proptest::prop_assert!(compare(&left, &right, &CompareOptions::default()).unwrap().differences.is_empty());
    }
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config {
        cases: 2048,
        rng_seed: proptest::test_runner::RngSeed::Fixed(20261002),
        ..Default::default()
    })]
    #[test]
    fn sparse_decimal_tolerance_agrees_with_scaled_integer_arithmetic(
        left_coefficient in -1_000_000_i64..1_000_000,
        right_coefficient in -1_000_000_i64..1_000_000,
        left_exponent in -6_i32..7,
        right_exponent in -6_i32..7,
        absolute_milli in 0_u16..2000,
        relative_milli in 0_u16..2000,
    ) {
        let left = value(&format!("{left_coefficient}e{left_exponent}"));
        let right = value(&format!("{right_coefficient}e{right_exponent}"));
        let tolerance = |milli: u16| format!("{}.{:03}", milli / 1000, milli % 1000).parse::<f64>().unwrap();
        let options = CompareOptions {
            absolute_tolerance: tolerance(absolute_milli),
            relative_tolerance: tolerance(relative_milli),
            ..Default::default()
        };
        // Both document values are scaled by 10^6, then all tolerance terms by
        // 1000. Every operation stays within i128 and is independent of the
        // implementation's canonicalization, multiplication and sparse carry.
        let scaled_left = i128::from(left_coefficient) * 10_i128.pow((left_exponent + 6) as u32);
        let scaled_right = i128::from(right_coefficient) * 10_i128.pow((right_exponent + 6) as u32);
        let distance = (scaled_left - scaled_right).abs() * 1000;
        let absolute = i128::from(absolute_milli) * 1_000_000;
        let relative = i128::from(relative_milli) * scaled_left.abs().max(scaled_right.abs());
        let expected = distance <= absolute.max(relative);
        let actual = compare(&left, &right, &options).unwrap().differences.is_empty();
        proptest::prop_assert_eq!(actual, expected);
    }
}
