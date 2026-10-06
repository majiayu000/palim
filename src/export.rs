use crate::{Delta, Error, delta, patch};
use json_patch::{
    AddOperation, MoveOperation, Patch, PatchOperation, RemoveOperation, ReplaceOperation,
    jsonptr::{PointerBuf, Token},
};
use serde_json::{Value, json};
use std::collections::HashSet;

// Generate objects directly; only arrays and unsorted Map backends construct
// a local native delta. Projection filters use the whole native path instead.
pub(crate) fn direct_patch(
    left: &Value,
    right: &Value,
    matching: &crate::DiffOptions,
    options: &crate::JsonPatchOptions,
    input_depth: usize,
) -> Result<Patch, Error> {
    let mut direct = Direct {
        matching,
        replace_disjoint: !options.factorize && !options.tests,
        // A native leaf tuple adds at most one container to the input depth.
        bounded_delta: input_depth < delta::MAX_DELTA_DEPTH,
        operations: Vec::new(),
    };
    let depth_error = direct.walk(left, right, &mut PointerBuf::new(), 0)?;
    if let Some(error) = depth_error {
        return Err(error);
    }
    Ok(Patch(direct.operations))
}

// Reuse the existing Map structure instead of serde_json::to_value's generic
// serializer and per-entry insertion, then keep its numeric normalization.
#[inline(never)]
pub(crate) fn copy_addition(value: &Value) -> Value {
    let mut copied = value.clone();
    normalize_addition(&mut copied);
    copied
}

fn normalize_addition(value: &mut Value) {
    match value {
        Value::Array(values) => values.iter_mut().for_each(normalize_addition),
        Value::Object(values) => values.values_mut().for_each(normalize_addition),
        // Use the same parser and unwrap behavior as json!(value), including
        // malformed Numbers constructed through from_string_unchecked.
        Value::Number(number) if !normalized_number(number.as_str()) => {
            *number = number.as_str().parse().unwrap();
        }
        _ => {}
    }
}

// Under arbitrary_precision, serde_json keeps decimal digits but lowercases E,
// adds an exponent sign, and converts the integer -0 to 0. Only skip its parser
// for strings it would leave byte-for-byte unchanged; everything else still
// uses that parser, including invalid from_string_unchecked payloads.
fn normalized_number(raw: &str) -> bool {
    let mut bytes = raw.as_bytes();
    if let Some(rest) = bytes.strip_prefix(b"-") {
        bytes = rest;
    }
    match bytes.first() {
        Some(b'0') => bytes = &bytes[1..],
        Some(b'1'..=b'9') => {
            bytes = &bytes[1..];
            while bytes.first().is_some_and(u8::is_ascii_digit) {
                bytes = &bytes[1..];
            }
        }
        _ => return false,
    }
    if let Some(rest) = bytes.strip_prefix(b".") {
        bytes = rest;
        if !bytes.first().is_some_and(u8::is_ascii_digit) {
            return false;
        }
        while bytes.first().is_some_and(u8::is_ascii_digit) {
            bytes = &bytes[1..];
        }
    }
    if let Some(rest) = bytes.strip_prefix(b"e") {
        bytes = rest;
        match bytes.first() {
            Some(b'+' | b'-') => bytes = &bytes[1..],
            _ => return false,
        }
        if !bytes.first().is_some_and(u8::is_ascii_digit) {
            return false;
        }
        while bytes.first().is_some_and(u8::is_ascii_digit) {
            bytes = &bytes[1..];
        }
    }
    bytes.is_empty() && raw != "-0"
}

struct Direct<'a> {
    matching: &'a crate::DiffOptions,
    replace_disjoint: bool,
    bounded_delta: bool,
    operations: Vec<PatchOperation>,
}

fn check_leaf_depth(depth: usize, values: &[&Value]) -> Result<(), Error> {
    if depth >= delta::MAX_DELTA_DEPTH {
        return Err(Error::new("", "JSON nesting exceeds max_depth"));
    }
    for value in values {
        if value.is_array() || value.is_object() {
            delta::check_depth(value, delta::MAX_DELTA_DEPTH - depth - 1)?;
        }
    }
    Ok(())
}

impl Direct<'_> {
    fn native(
        &mut self,
        left: &Value,
        right: &Value,
        path: &PointerBuf,
        depth: usize,
    ) -> Result<Option<Error>, Error> {
        let Some(change) = crate::diff::node(left, right, self.matching, path.as_str())? else {
            return Ok(None);
        };
        let depth_error = if self.bounded_delta {
            None
        } else {
            delta::check_depth(&change, delta::MAX_DELTA_DEPTH.saturating_sub(depth)).err()
        };
        walk(
            left,
            right,
            &change,
            path,
            &mut self.operations,
            self.replace_disjoint,
        )?;
        Ok(depth_error)
    }

    fn walk(
        &mut self,
        left: &Value,
        right: &Value,
        path: &mut PointerBuf,
        depth: usize,
    ) -> Result<Option<Error>, Error> {
        if left == right {
            return Ok(None);
        }
        match (left, right) {
            (Value::Array(_), Value::Array(_)) => self.native(left, right, path, depth),
            (Value::Object(source), Value::Object(target)) => {
                // serde_json's downstream preserve_order feature can substitute
                // an insertion-ordered Map. Retain native ordering for that case.
                if !source.keys().is_sorted() || !target.keys().is_sorted() {
                    return self.native(left, right, path, depth);
                }
                let mut source = source.iter().peekable();
                let mut target = target.iter().peekable();
                let mut depth_error = None;
                loop {
                    let order = match (source.peek(), target.peek()) {
                        (Some((old, _)), Some((new, _))) => old.cmp(new),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => break,
                    };
                    let (key, old, new) = match order {
                        std::cmp::Ordering::Less => {
                            let (key, old) = source.next().unwrap();
                            (key, Some(old), None)
                        }
                        std::cmp::Ordering::Greater => {
                            let (key, new) = target.next().unwrap();
                            (key, None, Some(new))
                        }
                        std::cmp::Ordering::Equal => {
                            let (key, old) = source.next().unwrap();
                            let (_, new) = target.next().unwrap();
                            (key, Some(old), Some(new))
                        }
                    };
                    path.push_back(key.as_str());
                    // Delay wire-depth failures until all matching callbacks have
                    // run, as in the full native diff followed by depth checking.
                    let child_error = match (old, new) {
                        (Some(old), Some(new)) => self.walk(old, new, path, depth + 1)?,
                        (Some(old), None) => {
                            let error = if self.bounded_delta {
                                None
                            } else {
                                check_leaf_depth(depth + 1, &[old]).err()
                            };
                            self.operations
                                .push(PatchOperation::Remove(RemoveOperation {
                                    path: path.clone(),
                                }));
                            error
                        }
                        (None, Some(new)) => {
                            let error = if self.bounded_delta {
                                None
                            } else {
                                check_leaf_depth(depth + 1, &[new]).err()
                            };
                            self.operations.push(PatchOperation::Add(AddOperation {
                                path: path.clone(),
                                value: copy_addition(new),
                            }));
                            error
                        }
                        (None, None) => unreachable!("key came from one of the objects"),
                    };
                    depth_error = depth_error.or(child_error);
                    path.pop_back();
                }
                Ok(depth_error)
            }
            _ => {
                let error = if self.bounded_delta {
                    None
                } else {
                    check_leaf_depth(depth, &[left, right]).err()
                };
                self.operations
                    .push(PatchOperation::Replace(ReplaceOperation {
                        path: path.clone(),
                        value: right.clone(),
                    }));
                Ok(error)
            }
        }
    }
}

// Standard generation need not construct reversible tuples when no
// primitive item can survive. Containers and caller-defined pairing stay on
// the native matching path. Check the first target before allocating a set so
// that common reorders usually return immediately.
pub(crate) fn disjoint_array_patch(
    left: &Value,
    right: &Value,
    positional: bool,
) -> Result<Option<Patch>, Error> {
    let (Some(source), Some(target)) = (left.as_array(), right.as_array()) else {
        return Ok(None);
    };
    if let Some(first) = target.first() {
        if first.is_array() || first.is_object() || source.contains(first) {
            return Ok(None);
        }
    }
    if let Some(first) = source.first() {
        if first.is_array() || first.is_object() || target.contains(first) {
            return Ok(None);
        }
    }
    if source
        .iter()
        .chain(target)
        .any(|value| value.is_array() || value.is_object())
    {
        return Ok(None);
    }
    crate::diff::check_array_capacity(source, target, "")?;
    let originals: HashSet<_> = source.iter().collect();
    if target.iter().any(|value| originals.contains(value)) {
        return Ok(None);
    }
    let mut output = Vec::new();
    if positional {
        replace_disjoint(source, target, &PointerBuf::new(), &mut output);
    } else {
        output.reserve(source.len() + target.len());
        for index in (0..source.len()).rev() {
            output.push(PatchOperation::Remove(RemoveOperation {
                path: child(&PointerBuf::new(), index),
            }));
        }
        for (index, value) in target.iter().enumerate() {
            output.push(PatchOperation::Add(AddOperation {
                path: child(&PointerBuf::new(), index),
                value: value.clone(),
            }));
        }
    }
    Ok(Some(Patch(output)))
}

fn replace_disjoint(
    source: &[Value],
    target: &[Value],
    path: &PointerBuf,
    out: &mut Vec<PatchOperation>,
) {
    let overlap = source.len().min(target.len());
    out.reserve(source.len().max(target.len()));
    for (index, value) in target.iter().take(overlap).enumerate() {
        out.push(PatchOperation::Replace(ReplaceOperation {
            path: child(path, index),
            value: value.clone(),
        }));
    }
    for index in (overlap..source.len()).rev() {
        out.push(PatchOperation::Remove(RemoveOperation {
            path: child(path, index),
        }));
    }
    for (index, value) in target.iter().enumerate().skip(overlap) {
        out.push(PatchOperation::Add(AddOperation {
            path: child(path, index),
            value: value.clone(),
        }));
    }
}

pub(crate) fn export(left: &Value, change: &Delta) -> Result<Patch, Error> {
    let right = patch::apply(left, &change.0, true)?;
    export_known(left, &right, change, true)
}

// The diff pipeline already knows the target (or its filtered projection).
// Public Delta::to_json_patch still validates its baseline through export above.
pub(crate) fn export_known(
    left: &Value,
    right: &Value,
    change: &Delta,
    replace_disjoint_arrays: bool,
) -> Result<Patch, Error> {
    let mut output = Vec::new();
    walk(
        left,
        right,
        &change.0,
        &PointerBuf::new(),
        &mut output,
        replace_disjoint_arrays,
    )?;
    Ok(Patch(output))
}

fn child<'a>(path: &PointerBuf, token: impl Into<Token<'a>>) -> PointerBuf {
    let mut result = path.clone();
    result.push_back(token);
    result
}

// Occupied original/final slots have frequency one. Prefix sums give the
// current sequential rank while point updates move an item between its slots.
// This is the binary indexed tree from Fenwick (1994), DOI 10.1002/spe.4380240306.
struct Ranks(Vec<usize>);

impl Ranks {
    fn new(mut frequencies: Vec<usize>) -> Self {
        for index in 0..frequencies.len() {
            let parent = index | (index + 1);
            if parent < frequencies.len() {
                frequencies[parent] += frequencies[index];
            }
        }
        Self(frequencies)
    }

    fn before(&self, mut end: usize) -> usize {
        let mut rank = 0;
        while end > 0 {
            rank += self.0[end - 1];
            end &= end - 1;
        }
        rank
    }

    fn occupy(&mut self, mut index: usize, occupied: bool) {
        while index < self.0.len() {
            if occupied {
                self.0[index] += 1;
            } else {
                self.0[index] -= 1;
            }
            index |= index + 1;
        }
    }
}

fn walk(
    left: &Value,
    right: &Value,
    change: &Value,
    path: &PointerBuf,
    out: &mut Vec<PatchOperation>,
    replace_disjoint_arrays: bool,
) -> Result<(), Error> {
    match change {
        Value::Array(a) => match a.len() {
            1 => out.push(PatchOperation::Add(AddOperation {
                path: path.clone(),
                value: a[0].clone(),
            })),
            2 => out.push(PatchOperation::Replace(ReplaceOperation {
                path: path.clone(),
                value: a[1].clone(),
            })),
            3 if a[2] == json!(0) => out.push(PatchOperation::Remove(RemoveOperation {
                path: path.clone(),
            })),
            3 if a[2] == json!(2) => out.push(PatchOperation::Replace(ReplaceOperation {
                path: path.clone(),
                value: right.clone(),
            })),
            _ => {
                return Err(Error::new(
                    path.as_str(),
                    "invalid delta in JSON Patch export",
                ));
            }
        },
        Value::Object(map) if delta::is_array(map) => {
            let source = left
                .as_array()
                .ok_or_else(|| Error::new(path.as_str(), "array delta requires an array"))?;
            let target = right
                .as_array()
                .ok_or_else(|| Error::new(path.as_str(), "array target is missing"))?;
            let parts = delta::parts(map, path.as_str())?;
            // Without survivors, positional replacements need half as many
            // operations. Factorization retains remove/add so that later adds
            // can become copies; guarded generation retains its container tests.
            if replace_disjoint_arrays
                && parts.moves.is_empty()
                && parts.changes.is_empty()
                && parts.removals.len() == source.len()
                && parts.additions.len() == target.len()
            {
                replace_disjoint(source, target, path, out);
                return Ok(());
            }
            let mut moved = vec![false; source.len()];
            let mut removed = vec![false; source.len()];
            for &old in parts.removals.keys() {
                removed[old] = true;
            }
            for &old in parts.moves.values() {
                moved[old] = true;
            }
            for (&old, change) in parts.removals.iter().rev() {
                if change[2] == json!(0) {
                    out.push(PatchOperation::Remove(RemoveOperation {
                        path: child(path, old),
                    }));
                }
            }
            let mut remaining = (0..source.len()).filter(|&old| !removed[old]);
            let mut desired = Vec::new();
            let mut old_at_target = vec![None; target.len()];
            for (&new, &old) in &parts.moves {
                old_at_target[new] = Some(old);
            }
            let mut additions = parts.additions.keys().copied().peekable();
            for (new, old_slot) in old_at_target.iter_mut().enumerate() {
                if additions.peek() == Some(&new) {
                    additions.next();
                    continue;
                }
                let old = old_slot
                    .as_ref()
                    .copied()
                    .or_else(|| remaining.next())
                    .ok_or_else(|| Error::new(path.as_str(), "invalid array mapping"))?;
                *old_slot = Some(old);
                desired.push(old);
            }
            if !parts.moves.is_empty() {
                // Reserve a final slot for every moved item immediately before its
                // next survivor, or after all originals if no survivor follows.
                // Survivors are already in original order. Thus the final occupied
                // slots spell `desired` without a mutable Vec or an order-stat tree.
                let mut originals = vec![0; source.len()];
                let mut finals = vec![0; source.len()];
                let mut frequencies = Vec::with_capacity(desired.len() + parts.moves.len());
                let mut cursor = 0;
                for old in 0..source.len() {
                    if removed[old] && !moved[old] {
                        continue;
                    }
                    if !moved[old] {
                        while desired.get(cursor) != Some(&old) {
                            let pending = desired
                                .get(cursor)
                                .copied()
                                .filter(|&i| moved[i])
                                .ok_or_else(|| {
                                    Error::new(path.as_str(), "inconsistent survivor order")
                                })?;
                            finals[pending] = frequencies.len();
                            frequencies.push(0);
                            cursor += 1;
                        }
                        cursor += 1;
                    }
                    originals[old] = frequencies.len();
                    frequencies.push(1);
                }
                for &old in &desired[cursor..] {
                    if !moved[old] {
                        return Err(Error::new(path.as_str(), "inconsistent survivor order"));
                    }
                    finals[old] = frequencies.len();
                    frequencies.push(0);
                }
                let mut ranks = Ranks::new(frequencies);
                for &old in desired.iter().rev().filter(|&&old| moved[old]) {
                    let from = ranks.before(originals[old]);
                    ranks.occupy(originals[old], false);
                    let to = ranks.before(finals[old]);
                    ranks.occupy(finals[old], true);
                    if from != to {
                        out.push(PatchOperation::Move(MoveOperation {
                            from: child(path, from),
                            path: child(path, to),
                        }));
                    }
                }
            }
            for (&new, value) in &parts.additions {
                out.push(PatchOperation::Add(AddOperation {
                    path: child(path, new),
                    value: (*value).clone(),
                }));
            }
            for (&new, change) in &parts.changes {
                let old = old_at_target[new]
                    .ok_or_else(|| Error::new(path.as_str(), "cannot modify an added item"))?;
                walk(
                    &source[old],
                    &target[new],
                    change,
                    &child(path, new),
                    out,
                    replace_disjoint_arrays,
                )?;
            }
        }
        Value::Object(map) => {
            for (key, change) in map {
                walk(
                    left.get(key).unwrap_or(&Value::Null),
                    right.get(key).unwrap_or(&Value::Null),
                    change,
                    &child(path, key.as_str()),
                    out,
                    replace_disjoint_arrays,
                )?;
            }
        }
        _ => {
            return Err(Error::new(
                path.as_str(),
                "invalid delta in JSON Patch export",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod direct_tests {
    use super::*;
    use crate::{DiffOptions, DiffPatcher, JsonPatchOptions};
    use proptest::prelude::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn normalized_number_matches_serde_parser() {
        for raw in [
            "0",
            "-0",
            "-0.0",
            "1.0",
            "1E5",
            "1e5",
            "1e+5",
            "1e-005",
            "1.00e+000",
            "0e-0",
            "1.5e+999",
            "18446744073709551616",
            "-9223372036854775809",
            "",
            "01",
            "-01",
            "+1",
            ".1",
            "1.",
            "1e",
            "1e+",
            "1e-",
            "1e+2.0",
            "1e+2x",
            "1e++2",
            "NaN",
            "1 ",
            " 1",
            "1\n",
            "١",
            "null",
        ] {
            let parsed = raw.parse::<serde_json::Number>();
            let unchanged = parsed.as_ref().is_ok_and(|number| number.as_str() == raw);
            assert_eq!(normalized_number(raw), unchanged, "{raw:?}");
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases:4096, rng_seed:proptest::test_runner::RngSeed::Fixed(20261006), ..ProptestConfig::default() })]
        #[test]
        fn number_fast_path_only_skips_unchanged_parser_results(
            raw in "[0-9.eE+\\-a-z \\n]{0,64}",
            float in any::<f64>(),
        ) {
            if normalized_number(&raw) {
                let parsed = raw.parse::<serde_json::Number>().unwrap();
                prop_assert_eq!(parsed.as_str(), raw.as_str());
            }
            if let Some(number) = serde_json::Number::from_f64(float) {
                let value = Value::Number(number);
                prop_assert_eq!(copy_addition(&value), json!(&value));
            }
            if let Ok(number) = raw.parse::<serde_json::Number>() {
                let value = Value::Number(number);
                prop_assert_eq!(copy_addition(&value), json!(&value));
            }
        }
    }

    #[test]
    fn addition_copy_keeps_nested_number_normalization_and_patch_bytes() {
        for raw in [
            "0",
            "-0",
            "-0.0",
            "1.0",
            "1E+0003",
            "1.00e00",
            "1e9999",
            "18446744073709551616",
            "-9223372036854775809",
            "0.000000001234567890123456789",
            "123456789012345678901234567890",
        ] {
            let number = Value::Number(serde_json::Number::from_string_unchecked(raw.into()));
            let mut value = json!({"nested":[null,{"a/~🦀":null}],"empty":{},
                "empty_array":[],"nil":null,"bool":true,"text":"Unicode 🦀"});
            value["nested"][0] = number.clone();
            value["nested"][1]["a/~🦀"] = number;
            let original = value.clone();
            let actual = copy_addition(&value);
            let expected = json!(&value);
            assert_eq!(actual, expected, "{raw}");
            assert_eq!(
                serde_json::to_vec(&actual).unwrap(),
                serde_json::to_vec(&expected).unwrap()
            );
            let left = json!({"kept":0});
            let mut right = left.clone();
            right["added/~"] = value.clone();
            let actual = direct_standard(&left, &right).unwrap();
            let expected = native_standard(&left, &right).unwrap();
            assert_eq!(
                serde_json::to_vec(&actual).unwrap(),
                serde_json::to_vec(&expected).unwrap()
            );
            assert_eq!(value, original);
        }
    }

    #[test]
    fn addition_copy_keeps_malformed_unchecked_number_failure() {
        let message = |result: std::thread::Result<Value>| {
            let error = result.expect_err("malformed unchecked Number must fail");
            error.downcast::<String>().unwrap()
        };
        for raw in ["", "01", "+1", "NaN", "1e", "1 ", "null"] {
            let value = Value::Array(vec![Value::Number(
                serde_json::Number::from_string_unchecked(raw.into()),
            )]);
            assert_eq!(
                message(std::panic::catch_unwind(|| copy_addition(&value))),
                message(std::panic::catch_unwind(|| json!(&value))),
                "{raw}"
            );
        }
    }

    fn native_optimized(
        matching: &DiffOptions,
        left: &Value,
        right: &Value,
        options: &JsonPatchOptions,
    ) -> Result<Patch, Error> {
        let patcher = DiffPatcher::new(matching.clone());
        let Some(change) = patcher.diff(left, right)? else {
            return Ok(Patch::default());
        };
        let standard = export_known(left, right, &change, !options.factorize && !options.tests)?;
        crate::rfc::optimize(left, right, standard, options)
    }

    fn mixed_json() -> impl Strategy<Value = Value> {
        prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(|value| json!(value)),
            proptest::collection::vec(any::<char>(), 0..8)
                .prop_map(|value| json!(value.into_iter().collect::<String>())),
        ]
        .prop_recursive(4, 64, 6, |inner| {
            prop_oneof![
                proptest::collection::vec(inner.clone(), 0..5).prop_map(Value::Array),
                proptest::collection::btree_map("[a-z~/]{0,4}", inner, 0..5)
                    .prop_map(|values| Value::Object(values.into_iter().collect())),
            ]
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases:128, rng_seed:proptest::test_runner::RngSeed::Fixed(20261005), ..ProptestConfig::default() })]
        #[test]
        fn hybrid_matches_native_on_arbitrary_mixed_json(left in mixed_json(), right in mixed_json()) {
            let matching = DiffOptions { text_diff_min_length:None, ..Default::default() };
            let patcher = DiffPatcher::new(matching.clone());
            for factorize in [false,true] {
                for rationalize in [false,true] {
                    for tests in [false,true] {
                        let options = JsonPatchOptions { factorize,rationalize,tests };
                        let expected = native_optimized(&matching,&left,&right,&options).unwrap();
                        let actual = patcher.diff_json_patch(&left,&right,&options).unwrap();
                        // Plain guarded root arrays can now use positional
                        // edits behind one complete baseline array test.
                        if !(tests && !factorize && !rationalize && left.is_array() && right.is_array()) {
                            prop_assert_eq!(&actual,&expected);
                        }
                        let mut applied = left.clone();
                        json_patch::patch(&mut applied,&actual).unwrap();
                        prop_assert_eq!(&applied,&right);
                        let inverse = crate::invert_json_patch(&left,&actual).unwrap();
                        json_patch::patch(&mut applied,&inverse).unwrap();
                        prop_assert_eq!(&applied,&left);
                    }
                }
            }
        }
    }

    #[test]
    fn hybrid_generation_keeps_native_operations_and_matching_callbacks() {
        let left = json!({"a/~":[{"id":1,"body":"old".repeat(40)},{"id":2,"body":"unchanged"}],
            "fields":{"a":1,"b":false},"z":{"array":[[1,2],[3,4]],"removed":[0]},"gone":true});
        let right = json!({"a/~":[{"id":2,"body":"unchanged"},{"id":1,"body":"new".repeat(40)}],
            "fields":{"a":2,"b":true},"z":{"array":[[3,4],[1,5]],"added":[1]},"added":null});
        for mode in 0..3 {
            for detect_moves in [false, true] {
                let calls = Arc::new(Mutex::new(Vec::new()));
                let mut matching = DiffOptions {
                    detect_moves,
                    include_value_on_move: true,
                    text_diff_min_length: None,
                    ..Default::default()
                };
                if mode == 1 {
                    let calls = calls.clone();
                    matching.object_hash = Some(Arc::new(move |value, index| {
                        calls.lock().unwrap().push(format!("hash:{index}:{value}"));
                        value.get("id").map(Value::to_string)
                    }));
                } else if mode == 2 {
                    let calls = calls.clone();
                    matching.array_item_matcher = Some(Arc::new(move |path, left, right| {
                        calls
                            .lock()
                            .unwrap()
                            .push(format!("match:{path}:{left}:{right}"));
                        match (left.get("id"), right.get("id")) {
                            (Some(a), Some(b)) => a == b,
                            _ => left == right,
                        }
                    }));
                }
                for factorize in [false, true] {
                    for rationalize in [false, true] {
                        for tests in [false, true] {
                            let options = JsonPatchOptions {
                                factorize,
                                rationalize,
                                tests,
                            };
                            calls.lock().unwrap().clear();
                            let expected =
                                native_optimized(&matching, &left, &right, &options).unwrap();
                            let expected_calls = calls.lock().unwrap().clone();
                            calls.lock().unwrap().clear();
                            let actual = DiffPatcher::new(matching.clone())
                                .diff_json_patch(&left, &right, &options)
                                .unwrap();
                            assert_eq!(calls.lock().unwrap().as_slice(), expected_calls);
                            assert_eq!(actual, expected);
                            let mut applied = left.clone();
                            json_patch::patch(&mut applied, &actual).unwrap();
                            assert_eq!(applied, right);
                            let inverse = crate::invert_json_patch(&left, &actual).unwrap();
                            json_patch::patch(&mut applied, &inverse).unwrap();
                            assert_eq!(applied, left);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn hybrid_deep_errors_keep_later_matching_callbacks_and_inputs() {
        for depth in [0, 123, 124, 125, 126] {
            let wrap = |value| (0..depth).fold(value, |value, _| json!({"a":value}));
            let left = json!({"deep":wrap(json!([{"id":1,"n":1}])),"later":[{"id":2,"n":1}]});
            let right = json!({"deep":wrap(json!([{"id":1,"n":2}])),"later":[{"id":2,"n":2}]});
            let original = left.clone();
            let calls = Arc::new(Mutex::new(Vec::new()));
            let recorded = calls.clone();
            let matching = DiffOptions {
                object_hash: Some(Arc::new(move |value, index| {
                    recorded.lock().unwrap().push((value.clone(), index));
                    value.get("id").map(Value::to_string)
                })),
                text_diff_min_length: None,
                ..Default::default()
            };
            let options = JsonPatchOptions {
                factorize: false,
                rationalize: false,
                tests: false,
            };
            let expected = native_optimized(&matching, &left, &right, &options);
            let expected_calls = calls.lock().unwrap().clone();
            calls.lock().unwrap().clear();
            let actual = DiffPatcher::new(matching).diff_json_patch(&left, &right, &options);
            assert_eq!(actual, expected, "depth {depth}");
            assert_eq!(*calls.lock().unwrap(), expected_calls, "depth {depth}");
            assert_eq!(left, original);
        }
    }

    fn native_standard(left: &Value, right: &Value) -> Result<Patch, Error> {
        let patcher = DiffPatcher::new(DiffOptions {
            text_diff_min_length: None,
            ..Default::default()
        });
        match patcher.diff(left, right)? {
            Some(change) => export_known(left, right, &change, true),
            None => Ok(Patch::default()),
        }
    }

    fn direct_standard(left: &Value, right: &Value) -> Result<Patch, Error> {
        let options = DiffOptions {
            text_diff_min_length: None,
            ..Default::default()
        };
        let depth = delta::measure_depth(left, options.max_depth)?
            .max(delta::measure_depth(right, options.max_depth)?);
        direct_patch(
            left,
            right,
            &options,
            &JsonPatchOptions {
                factorize: false,
                rationalize: false,
                tests: false,
            },
            depth,
        )
    }

    #[test]
    fn direct_objects_keep_native_wire_order_and_payloads() {
        for raw in ["-0", "1E+0003", "1.00e00", "1.0", "1e9999"] {
            let number = Value::Number(serde_json::Number::from_string_unchecked(raw.into()));
            let mut right = json!({"":{"a/~🦀":2},"add":null,"z":true});
            right["add"] = number.clone();
            let left = json!({"":{"a/~🦀":1},"removed":[{"nested":true}],"z":false});
            for (left, right) in [
                (left.clone(), right.clone()),
                (right, left),
                (json!(null), number.clone()),
                (number, json!({"whole":[1,2]})),
            ] {
                let expected = native_standard(&left, &right).unwrap();
                let actual = direct_standard(&left, &right).unwrap();
                assert_eq!(actual, expected);
                assert_eq!(
                    serde_json::to_vec(&actual).unwrap(),
                    serde_json::to_vec(&expected).unwrap()
                );
            }
        }
        let left = json!({"array":[1,2],"nested":{"a":1}});
        let right = json!({"array":[1,2],"nested":{"a":2}});
        assert_eq!(
            direct_standard(&left, &right).unwrap(),
            native_standard(&left, &right).unwrap()
        );
        let right = json!({"array":[2,1]});
        assert_eq!(
            direct_standard(&left, &right).unwrap(),
            native_standard(&left, &right).unwrap()
        );
    }

    #[test]
    fn direct_generation_keeps_native_delta_depth_errors() {
        let options = JsonPatchOptions {
            factorize: false,
            rationalize: false,
            tests: false,
        };
        for depth in 124..=129 {
            let wrap = |value: Value| (0..depth).fold(value, |value, _| json!({"a":value}));
            let deep = wrap(json!(1));
            for (left, right) in [
                (deep.clone(), wrap(json!(2))),
                (json!({}), json!({"added":deep})),
                (wrap(json!(1)), json!(null)),
                (wrap(json!(1)), wrap(json!(1))),
            ] {
                assert_eq!(
                    DiffPatcher::default().diff_json_patch(&left, &right, &options),
                    native_standard(&left, &right)
                );
            }
        }
    }
}
