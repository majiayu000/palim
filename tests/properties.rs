use palim::{Delta, DiffOptions, DiffPatcher, apply_json_patch, patch, reverse, unpatch};
use proptest::prelude::*;
use serde_json::{Value, json};
use std::sync::Arc;

fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| json!(n)),
        any::<u64>().prop_map(|n| json!(n)),
        (-1e12f64..1e12).prop_map(|n| json!(n)),
        proptest::collection::vec(any::<char>(), 0..35)
            .prop_map(|v| json!(v.into_iter().collect::<String>()))
    ];
    leaf.prop_recursive(5, 128, 8, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
            proptest::collection::btree_map("[a-z~/]{0,6}", inner, 0..8)
                .prop_map(|v| Value::Object(v.into_iter().collect()))
        ]
    })
}
fn round_trip(dp: &DiffPatcher, a: &Value, b: &Value) {
    if let Some(d) = dp.diff(a, b).unwrap() {
        assert_eq!(patch(a, &d).unwrap(), *b);
        assert_eq!(unpatch(b, &d).unwrap(), *a);
        assert_eq!(reverse(&reverse(&d).unwrap()).unwrap(), d);
        let imported: Delta = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        #[cfg(feature = "exact-numbers")]
        assert_eq!(imported, d);
        #[cfg(not(feature = "exact-numbers"))]
        {
            // serde_json without float_roundtrip may round serialized f64 values.
            let wire_a: Value = serde_json::from_str(&serde_json::to_string(a).unwrap()).unwrap();
            let wire_b: Value = serde_json::from_str(&serde_json::to_string(b).unwrap()).unwrap();
            assert_eq!(patch(&wire_a, &imported).unwrap(), wire_b);
            assert_eq!(unpatch(&wire_b, &imported).unwrap(), wire_a);
        }
        assert_eq!(
            apply_json_patch(a, &d.to_json_patch(a).unwrap()).unwrap(),
            *b
        );
        let forward = d.to_forward_only();
        assert_eq!(forward.patch(a).unwrap(), *b);
        let wire = serde_json::to_string(&forward).unwrap();
        let imported: palim::ForwardDelta = serde_json::from_str(&wire).unwrap();
        #[cfg(feature = "exact-numbers")]
        assert_eq!(imported.patch(a).unwrap(), *b);
        #[cfg(not(feature = "exact-numbers"))]
        {
            let wire_a: Value = serde_json::from_str(&serde_json::to_string(a).unwrap()).unwrap();
            let wire_b: Value = serde_json::from_str(&serde_json::to_string(b).unwrap()).unwrap();
            assert_eq!(imported.patch(&wire_a).unwrap(), wire_b);
        }
    } else {
        assert_eq!(a, b);
    }
}
proptest! {
    #![proptest_config(ProptestConfig { cases: 2048, rng_seed:proptest::test_runner::RngSeed::Fixed(20260930), .. ProptestConfig::default() })]
    #[test]
    fn arbitrary_json_round_trip(a in json_value(), b in json_value()) {
        round_trip(&DiffPatcher::default(),&a,&b);
    }
    #[test]
    fn duplicate_identity_and_mixed_array_edits(
        a in proptest::collection::vec((0u8..12,any::<i16>()),0..60),
        b in proptest::collection::vec((0u8..12,any::<i16>()),0..60),
        moves in any::<bool>(), include in any::<bool>()
    ) {
        let dp = DiffPatcher::new(DiffOptions { object_hash:Some(Arc::new(|v,_|v.get("id").map(Value::to_string))),
            detect_moves:moves,include_value_on_move:include,..Default::default() });
        let a = json!(a.iter().map(|(id,v)|json!({"id":id,"value":v})).collect::<Vec<_>>());
        let b = json!(b.iter().map(|(id,v)|json!({"id":id,"value":v})).collect::<Vec<_>>());
        round_trip(&dp,&a,&b);
    }
    #[test]
    fn arbitrary_unicode_text(a in proptest::collection::vec(any::<char>(),0..130), b in proptest::collection::vec(any::<char>(),0..130)) {
        let dp = DiffPatcher::new(DiffOptions { text_diff_min_length:Some(0),..Default::default() });
        round_trip(&dp,&json!(a.into_iter().collect::<String>()),&json!(b.into_iter().collect::<String>()));
    }
    #[test]
    fn untrusted_delta_never_panics(value in json_value(), baseline in json_value()) {
        if let Ok(d) = Delta::from_value(value) { let _ = patch(&baseline,&d); let _ = reverse(&d); let _ = d.to_json_patch(&baseline); }
    }
}
