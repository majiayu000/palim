use palim::{Delta, DiffOptions, DiffPatcher, patch, unpatch};
use serde_json::json;

#[test]
fn imported_js_inverses_restore_repeated_unicode_and_overlapping_context() {
    use palim::{TextPatchOptions, patch_fuzzy};
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/js-text-inverse.json")).unwrap();
    let failures: Vec<_> = cases
        .iter()
        .filter_map(|case| {
            let inverse = Delta::from_value(case["inverse"].clone()).unwrap();
            let actual = patch_fuzzy(&case["right"], &inverse, &TextPatchOptions::default());
            (!actual.as_ref().is_ok_and(|v| v == &case["left"]))
                .then(|| format!("{}: {actual:?}", case["name"]))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn fuzzy_matching_preserves_complete_context_and_overlapping_exact_occurrences() {
    use palim::{TextPatchOptions, apply_text_patch};
    let prefix = "ABCDEFGHIJ".repeat(4);
    let suffix = "klmnopqrst".repeat(4);
    let original = format!("{prefix}old{suffix}");
    let decoy = format!("{}GHIJoldklmn{}", "x".repeat(36), "z".repeat(36));
    let source = format!("{decoy} / {original}");
    let text = format!("@@ -1,83 +1,83 @@\n {prefix}\n-old\n+NEW\n {suffix}\n");
    assert_eq!(
        apply_text_patch(&source, &text, &TextPatchOptions::default()).unwrap(),
        format!("{decoy} / {prefix}NEW{suffix}")
    );

    // Exact occurrences can overlap. The closest full match starts at 1,
    // rather than at 0 as a non-overlapping match iterator would report.
    let text = "@@ -3,4 +3,4 @@\n-aaaa\n+WXYZ\n";
    assert_eq!(
        apply_text_patch("aaaaab", text, &TextPatchOptions::default()).unwrap(),
        "aWXYZb"
    );
}

#[test]
fn normalized_text_lengths_cannot_overflow_coordinates() {
    for header in [
        format!("@@ -{},0 +0,0 @@\n-a\n+b\n", usize::MAX),
        format!("@@ -0,0 +{},0 @@\n-a\n+b\n", usize::MAX),
        format!("@@ -{},0 +0,0 @@\n-ab\n+cd\n", usize::MAX - 1),
    ] {
        let decoded = std::panic::catch_unwind(|| Delta::from_value(json!([header, 0, 2])));
        assert!(
            decoded.is_ok(),
            "text decoding must return Error, never panic"
        );
        assert!(
            decoded.unwrap().is_err(),
            "normalized coordinates must be checked"
        );
    }
}

#[test]
fn text_uri_decoding_preserves_javascript_reserved_escapes() {
    for escaped in [
        "%3B", "%2F", "%3F", "%3A", "%40", "%26", "%3D", "%2B", "%24", "%2C", "%23", "%2f",
    ] {
        let d = Delta::from_value(json!([format!("@@ -0,0 +1,3 @@\n+{escaped}\n"), 0, 2])).unwrap();
        assert_eq!(patch(&json!(""), &d).unwrap(), json!(escaped));
        assert_eq!(unpatch(&json!(escaped), &d).unwrap(), json!(""));
    }
    let d = Delta::from_value(json!(["@@ -0,0 +1,3 @@\n+%252F\n", 0, 2])).unwrap();
    assert_eq!(patch(&json!(""), &d).unwrap(), json!("%2F"));
}

#[test]
fn sparse_long_unicode_text_keeps_utf16_coordinates_and_context() {
    let prefix = "😀a𝄞é".repeat(20_000);
    let suffix = "🐱z中".repeat(20_000);
    let a = format!("{prefix}old{suffix}");
    let b = format!("{prefix}replacement{suffix}");
    let dp = DiffPatcher::new(DiffOptions {
        text_diff_min_length: Some(1),
        ..Default::default()
    });
    let d = dp.diff(&json!(a), &json!(b)).unwrap().unwrap();
    assert_eq!(patch(&json!(a), &d).unwrap(), json!(b));
    assert_eq!(unpatch(&json!(b), &d).unwrap(), json!(a));
    assert!(serde_json::to_string(&d).unwrap().len() < 600);
}

#[test]
fn distant_unicode_edits_keep_compact_hunks_and_roundtrip() {
    let a: String = (0..5000)
        .map(|i| format!("line {i:04} Unicode 🦀 中文 stable context\n"))
        .collect();
    let b = a
        .replacen("line 0000", "首行修改🚀", 1)
        .replacen("line 2500", "中间修改🚀", 1)
        .replacen("line 4999", "末行修改🚀", 1);
    let dp = DiffPatcher::new(DiffOptions {
        text_diff_min_length: Some(1),
        ..Default::default()
    });
    let left = json!(a);
    let right = json!(b);
    let d = dp.diff(&left, &right).unwrap().unwrap();
    let wire = serde_json::to_vec(&d).unwrap();
    assert!(wire.len() < 1000, "unchanged lines must remain context");
    assert_eq!(d.as_value()[0].as_str().unwrap().matches("@@ -").count(), 3);
    let decoded: Delta = serde_json::from_slice(&wire).unwrap();
    assert_eq!(patch(&left, &decoded).unwrap(), right);
    assert_eq!(unpatch(&right, &decoded).unwrap(), left);
    let mut independently_patched = left.clone();
    json_patch::patch(
        &mut independently_patched,
        &decoded.to_json_patch(&left).unwrap(),
    )
    .unwrap();
    assert_eq!(independently_patched, right);
}

#[test]
fn large_text_replacements_and_line_boundaries_roundtrip() {
    let dp = DiffPatcher::new(DiffOptions {
        text_diff_min_length: Some(1),
        ..Default::default()
    });
    let lines: String = (0..300)
        .map(|i| format!("{i:03}: 🦀 repeated 中文 context\r\n"))
        .collect();
    let changed_lines = lines
        .replacen("001: 🦀 repeated 中文 context\r\n", "", 1)
        .replacen("150:", "新增😀\n150:", 1)
        .replacen("299: 🦀", "终行🐱", 1);
    for (a, b) in [
        (lines, changed_lines),
        ("a中😀".repeat(5000), "β🦀界".repeat(4000)),
        (
            format!("OLD{}MID{}END", "🐱a".repeat(5000), "🦀z".repeat(5000)),
            format!("NEW{}CENTER{}FINAL", "🐱a".repeat(5000), "🦀z".repeat(5000)),
        ),
    ] {
        let left = json!(a);
        let right = json!(b);
        let d = dp.diff(&left, &right).unwrap().unwrap();
        assert_eq!(patch(&left, &d).unwrap(), right);
        assert_eq!(unpatch(&right, &d).unwrap(), left);
    }
}

#[test]
fn line_groups_preserve_empty_lines_unicode_and_terminal_newlines() {
    let dp = DiffPatcher::new(DiffOptions {
        text_diff_min_length: Some(1),
        ..Default::default()
    });
    for lines in [0, 1, 2047, 2048, 4095, 4096, 4097, 8192] {
        for ending in ["\n", "\r\n"] {
            let repeated = format!("🐱中{ending}{ending}").repeat(lines);
            let a = format!("OLD{ending}{repeated}末尾🦀{ending}");
            let b = format!("{ending}{repeated}{ending}新末尾🚀");
            let left = json!(a);
            let right = json!(b);
            let d = dp.diff(&left, &right).unwrap().unwrap();
            assert_eq!(patch(&left, &d).unwrap(), right);
            assert_eq!(unpatch(&right, &d).unwrap(), left);
        }
    }
}

#[test]
fn fuzzy_text_accepts_bounded_offsets_and_preserves_context_edits() {
    use palim::{TextPatchOptions, apply_text_patch};
    let text = "@@ -1,19 +1,17 @@\n The quick \n-brown\n+red\n  fox\n";
    let options = TextPatchOptions {
        max_distance: 20,
        max_error_ratio: 0.2,
    };
    for (source, expected) in [
        ("PREFIX The quick brown fox", "PREFIX The quick red fox"),
        ("The quack brown fox", "The quack red fox"),
        ("The quicker brown fox", "The quicker red fox"),
        ("The quck brown fox", "The quck red fox"),
        ("The quick browx fox", "The quick red fox"),
    ] {
        let saved = source.to_owned();
        assert_eq!(apply_text_patch(source, text, &options).unwrap(), expected);
        assert_eq!(source, saved);
    }
    let exact = TextPatchOptions {
        max_distance: 0,
        max_error_ratio: 0.0,
    };
    assert!(apply_text_patch("PREFIX The quick brown fox", text, &exact).is_err());
    assert!(apply_text_patch("The quack brown fox", text, &exact).is_err());
    assert_eq!(
        apply_text_patch("The quick brown fox", text, &exact).unwrap(),
        "The quick red fox"
    );
}

#[test]
fn fuzzy_text_handles_multiple_unicode_hunks_and_all_or_error() {
    use palim::{TextPatchOptions, apply_text_patch};
    let a = format!("🐱abc OLD {} xyz LAST🦀", "0123456789 ".repeat(8));
    let b = a.replace("OLD", "NEW-LONG").replace("LAST", "FINAL");
    let dp = DiffPatcher::new(DiffOptions {
        text_diff_min_length: Some(1),
        ..Default::default()
    });
    let d = dp.diff(&json!(a), &json!(b)).unwrap().unwrap();
    let text = d.as_value()[0].as_str().unwrap();
    assert!(text.matches("@@ -").count() >= 2);
    let source = format!("😀PREFIX {}", a.replace("abc", "axc"));
    let expected = format!("😀PREFIX {}", b.replace("abc", "axc"));
    let options = TextPatchOptions {
        max_distance: 30,
        max_error_ratio: 0.2,
    };
    assert_eq!(apply_text_patch(&source, text, &options).unwrap(), expected);
    // The first hunk still matches and applies; the second must reject the
    // changed deletion/context instead of returning a partially patched string.
    let failing = format!("😀PREFIX {}", a.replace("LAST🦀", "QQQQ🐱"));
    let saved = failing.clone();
    assert!(
        apply_text_patch(
            &failing,
            text,
            &TextPatchOptions {
                max_distance: 30,
                max_error_ratio: 0.0
            }
        )
        .is_err()
    );
    assert_eq!(failing, saved);
    for ratio in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
        assert!(
            apply_text_patch(
                &source,
                text,
                &TextPatchOptions {
                    max_distance: 30,
                    max_error_ratio: ratio
                }
            )
            .is_err()
        );
    }
}

#[test]
fn fuzzy_text_handles_long_deletions_without_losing_unicode() {
    use palim::{TextPatchOptions, apply_text_patch};
    let a = format!(
        "header 🐱 abc {} xyz 🦀 tail",
        "large block 中😀;".repeat(80)
    );
    let b = "header 🐱 abc replacement xyz 🦀 tail";
    let dp = DiffPatcher::new(DiffOptions {
        text_diff_min_length: Some(1),
        ..Default::default()
    });
    let d = dp.diff(&json!(a), &json!(b)).unwrap().unwrap();
    let source = format!("PREFIX {}", a.replace("abc", "axc"));
    let expected = format!("PREFIX {}", b.replace("abc", "axc"));
    assert_eq!(
        apply_text_patch(
            &source,
            d.as_value()[0].as_str().unwrap(),
            &TextPatchOptions {
                max_distance: 50,
                max_error_ratio: 0.1
            }
        )
        .unwrap(),
        expected
    );
}

#[test]
fn fuzzy_displacement_limit_counts_utf16_not_characters() {
    use palim::{TextPatchOptions, apply_text_patch};
    let text = "@@ -1,19 +1,17 @@\n The quick \n-brown\n+red\n  fox\n";
    let source = "😀The quick brown fox";
    assert!(
        apply_text_patch(
            source,
            text,
            &TextPatchOptions {
                max_distance: 1,
                max_error_ratio: 0.0
            }
        )
        .is_err()
    );
    assert_eq!(
        apply_text_patch(
            source,
            text,
            &TextPatchOptions {
                max_distance: 2,
                max_error_ratio: 0.0
            }
        )
        .unwrap(),
        "😀The quick red fox"
    );
}

#[test]
fn fuzzy_anchor_ties_replace_the_fragment_and_preserve_external_suffixes() {
    use palim::{TextPatchOptions, apply_text_patch};
    let text = "@@ -1,3 +1,3 @@\n-abc\n+XYZ\n";
    let options = TextPatchOptions {
        max_distance: 0,
        max_error_ratio: 0.34,
    };
    for (source, expected) in [
        ("abx", "XYZ"),
        ("ab", "XYZ"),
        ("abxSUFFIX", "XYZSUFFIX"),
        ("acSUFFIX", "XYZSUFFIX"),
        ("abcSUFFIX", "XYZSUFFIX"),
    ] {
        assert_eq!(apply_text_patch(source, text, &options).unwrap(), expected);
    }
}

#[test]
fn common_text_blocks_trim_only_at_utf8_boundaries() {
    let dp = DiffPatcher::new(DiffOptions {
        text_diff_min_length: Some(1),
        ..Default::default()
    });
    for padding in [0, 1, 7, 8, 31, 32, 255, 256, 257, 511, 512, 513] {
        for (old, new) in [("è", "é"), ("Ā", "ŀ"), ("😀", "😁")] {
            let prefix = format!("{}🐱", "a".repeat(padding));
            let suffix = format!("🦀{}", "z".repeat(padding));
            let a = format!("{prefix}{old}{suffix}");
            let b = format!("{prefix}{new}{suffix}");
            let d = dp.diff(&json!(a), &json!(b)).unwrap().unwrap();
            assert_eq!(patch(&json!(a), &d).unwrap(), json!(b));
            assert_eq!(unpatch(&json!(b), &d).unwrap(), json!(a));
        }
    }
}

#[test]
fn historical_js_inverse_coordinate_formats_remain_strict() {
    use palim::{TextPatchOptions, apply_text_patch, reverse};
    // JS jsondiffpatch's reverse regex does not swap this one-unit header;
    // declared +3 and actual -3 disagree. Fuzzy application must not accept it.
    let malformed = "@@ -1 +1,4 @@\n-text\n+~\n";
    assert!(Delta::from_value(json!([malformed, 0, 2])).is_err());
    assert!(apply_text_patch("text", malformed, &TextPatchOptions::default()).is_err());

    // Forward headers use rolling coordinates. Reversing operations while
    // retaining ascending hunk order needs relocation, as JS DMP provides.
    let forward = "@@ -1,9 +1,10 @@\n a\n+X\n bcdefghi\n@@ -15,7 +15,8 @@\n nop\n+Y\n qrst\n";
    let a = "abcdefghijklmnopqrst";
    let b = "aXbcdefghijklmnopYqrst";
    let d = Delta::from_value(json!([forward, 0, 2])).unwrap();
    assert_eq!(patch(&json!(a), &d).unwrap(), json!(b));
    assert_eq!(patch(&json!(b), &reverse(&d).unwrap()).unwrap(), json!(a));
    let js_inverse = "@@ -1,10 +1,9 @@\n a\n-X\n bcdefghi\n@@ -15,8 +15,7 @@\n nop\n-Y\n qrst\n";
    let d = Delta::from_value(json!([js_inverse, 0, 2])).unwrap();
    assert!(patch(&json!(b), &d).is_err());
    assert_eq!(
        apply_text_patch(b, js_inverse, &TextPatchOptions::default()).unwrap(),
        a
    );
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config {
        cases: 256,
        rng_seed: proptest::test_runner::RngSeed::Fixed(20261001),
        ..Default::default()
    })]
    #[test]
    fn fuzzy_unicode_offsets_preserve_surrounding_text(
        a in proptest::collection::vec(proptest::prelude::any::<char>(), 80..160),
        b in proptest::collection::vec(proptest::prelude::any::<char>(), 80..160),
        prefix in proptest::collection::vec(proptest::prelude::any::<char>(), 0..12),
        suffix in proptest::collection::vec(proptest::prelude::any::<char>(), 0..12),
    ) {
        use palim::{TextPatchOptions, apply_text_patch};
        let a: String = a.into_iter().collect();
        let b: String = b.into_iter().collect();
        let prefix: String = prefix.into_iter().collect();
        let suffix: String = suffix.into_iter().collect();
        let dp = DiffPatcher::new(DiffOptions { text_diff_min_length: Some(1), ..Default::default() });
        let d = dp.diff(&json!(a), &json!(b)).unwrap().unwrap();
        let source = format!("{prefix}{a}{suffix}");
        let expected = format!("{prefix}{b}{suffix}");
        proptest::prop_assert_eq!(
            apply_text_patch(&source, d.as_value()[0].as_str().unwrap(), &TextPatchOptions { max_distance: 32, max_error_ratio: 0.0 }).unwrap(),
            expected
        );
    }

    #[test]
    fn utf16_protocol_lengths_match_the_standard_library(
        content in proptest::collection::vec(proptest::prelude::any::<char>(), 0..512),
    ) {
        let content: String = content.into_iter().collect();
        let units = content.encode_utf16().count();
        let coordinates = if units == 0 { "0,0".to_owned() } else { format!("1,{units}") };
        let encoded = content.replace('%', "%25").replace('\n', "%0A");
        let d = Delta::from_value(json!([
            format!("@@ -0,0 +{coordinates} @@\n+{encoded}\n"), 0, 2
        ])).unwrap();
        proptest::prop_assert_eq!(patch(&json!(""), &d).unwrap(), json!(content));
    }
}

#[test]
fn borrowed_line_tokens_keep_character_hunks_and_utf16_headers() {
    let a: String = (0..4200)
        .map(|i| format!("{i:04}: 🦀 e\u{301} \0中文\r\n\n"))
        .collect();
    let b = a
        .replacen("0000", "开🦀", 1)
        .replacen("2100", "e\u{301}🚀", 1)
        .replacen("4199", "尾𐀀", 1);
    let left = json!(a);
    let right = json!(b);
    let delta = DiffPatcher::default().diff(&left, &right).unwrap().unwrap();
    // Frozen character-token output: mixed UTF-8 widths, CRLF, empty lines,
    // NUL and more than 4096 lines must retain the same hunk/header choices.
    let expected: serde_json::Value = serde_json::from_str(
        r#"["@@ -1,9 +1,8 @@\n-0000\n+%E5%BC%80%F0%9F%A6%80\n : %F0%9F%A6%80 \n@@ -37796,13 +37796,13 @@\n %E6%96%87%0D%0A%0A\n-2100\n+e%CC%81%F0%9F%9A%80\n : %F0%9F%A6%80 \n@@ -75578,13 +75578,12 @@\n %E6%96%87%0D%0A%0A\n-4199\n+%E5%B0%BE%F0%90%80%80\n : %F0%9F%A6%80 \n",0,2]"#,
    )
    .unwrap();
    assert_eq!(delta.as_value(), &expected);
    assert_eq!(patch(&left, &delta).unwrap(), right);
    assert_eq!(unpatch(&right, &delta).unwrap(), left);
}
