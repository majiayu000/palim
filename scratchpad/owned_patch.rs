//! Experimental plain RFC add/replace generation and its measurement probe.
//! Loaded into a temporary copy of the library by run-owned-patch.py only.
use crate::{DiffPatcher, Error, JsonPatchOptions, delta, export};
use json_patch::{AddOperation, Patch, PatchOperation, ReplaceOperation, jsonptr::PointerBuf};
use serde_json::{Value, json};
use std::{fs, hint::black_box, time::Instant};

/// Consume an already-owned target. Changed array pairs and deletions are out of scope.
pub fn diff_json_patch_owned(left: &Value, right: Value) -> Result<Patch, Error> {
    let input_depth = match DiffPatcher::default().check_inputs(left, &right) {
        Ok(depth) => depth,
        Err(error) => {
            // A rejected target can be arbitrarily deep: recursive Value::drop
            // would otherwise turn this consuming API's error into a stack overflow.
            let mut pending = vec![right];
            while let Some(value) = pending.pop() {
                match value {
                    Value::Array(items) => pending.extend(items),
                    Value::Object(items) => pending.extend(items.into_values()),
                    _ => {}
                }
            }
            return Err(error);
        }
    };
    let mut operations = Vec::new();
    walk(
        left,
        right,
        &mut PointerBuf::new(),
        0,
        input_depth < delta::MAX_DELTA_DEPTH,
        &mut operations,
    )?;
    Ok(Patch(operations))
}

fn walk(
    left: &Value,
    right: Value,
    path: &mut PointerBuf,
    depth: usize,
    bounded_delta: bool,
    operations: &mut Vec<PatchOperation>,
) -> Result<(), Error> {
    if left == &right {
        return Ok(());
    }
    match (left, right) {
        (Value::Object(source), Value::Object(target)) => {
            // The production path also has a separate preserve_order fallback.
            if !source.keys().is_sorted() || !target.keys().is_sorted() {
                return Err(Error::new(path.as_str(), "prototype requires sorted maps"));
            }
            let mut source = source.iter().peekable();
            let mut target = target.into_iter().peekable();
            loop {
                let order = match (source.peek(), target.peek()) {
                    (Some((old, _)), Some((new, _))) => old.cmp(&new),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => break,
                };
                if order == std::cmp::Ordering::Less {
                    path.push_back(source.peek().unwrap().0.as_str());
                    return Err(Error::new(
                        path.as_str(),
                        "prototype does not support deletions",
                    ));
                }
                let (key, mut value) = target.next().unwrap();
                path.push_back(key.as_str());
                if order == std::cmp::Ordering::Equal {
                    let (_, old) = source.next().unwrap();
                    walk(old, value, path, depth + 1, bounded_delta, operations)?;
                } else {
                    if !bounded_delta {
                        export::check_leaf_depth(depth + 1, &[&value])?;
                    }
                    // Keep the production Add number contract while moving its
                    // containers and strings directly into the operation.
                    export::normalize_addition(&mut value);
                    operations.push(PatchOperation::Add(AddOperation {
                        path: path.clone(),
                        value,
                    }));
                }
                path.pop_back();
            }
            Ok(())
        }
        (Value::Array(_), Value::Array(_)) => Err(Error::new(
            path.as_str(),
            "prototype does not support changed array pairs",
        )),
        (_, right) => {
            if !bounded_delta {
                export::check_leaf_depth(depth, &[left, &right])?;
            }
            operations.push(PatchOperation::Replace(ReplaceOperation {
                path: path.clone(),
                value: right,
            }));
            Ok(())
        }
    }
}

type ProbeResult<T> = Result<T, Box<dyn std::error::Error>>;

fn plain() -> JsonPatchOptions {
    JsonPatchOptions {
        factorize: false,
        rationalize: false,
        tests: false,
    }
}

fn borrowed(engine: &str, left: &Value, right: &Value) -> ProbeResult<Patch> {
    Ok(match engine {
        "palim-borrowed" => DiffPatcher::default().diff_json_patch(left, right, &plain())?,
        "json-patch" => json_patch::diff(left, right),
        _ => return Err(format!("unknown borrowed engine: {engine}").into()),
    })
}

fn verify(left: &Value, right: &Value, patch: &Patch) -> ProbeResult<()> {
    let wire = serde_json::to_vec(patch)?;
    let decoded: Patch = serde_json::from_slice(&wire)?;
    let mut result = left.clone();
    json_patch::patch(&mut result, &decoded)?;
    assert_eq!(result, *right);
    let inverse = crate::invert_json_patch(left, &decoded)?;
    json_patch::patch(&mut result, &inverse)?;
    assert_eq!(result, *left);
    Ok(())
}

// Each operation uses the same clock boundary. For owned-ready Core only,
// preparing the pre-owned target is deliberately outside that boundary.
fn measure(
    mut once: impl FnMut() -> ProbeResult<(f64, f64)>,
) -> ProbeResult<(Vec<f64>, Vec<f64>, usize)> {
    for _ in 0..3 {
        once()?;
    }
    let mut iterations = 1;
    loop {
        let mut elapsed = 0.;
        for _ in 0..iterations {
            elapsed += once()?.0;
        }
        if elapsed >= 10. || iterations >= 2048 {
            break;
        }
        iterations *= 2;
    }
    let mut samples = Vec::new();
    let mut generation_samples = Vec::new();
    for _ in 0..9 {
        let mut elapsed = 0.;
        let mut generation = 0.;
        for _ in 0..iterations {
            let sample = once()?;
            elapsed += sample.0;
            generation += sample.1;
        }
        samples.push(elapsed / iterations as f64);
        generation_samples.push(generation / iterations as f64);
    }
    Ok((samples, generation_samples, iterations))
}

pub fn run() -> ProbeResult<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        return Err("usage: owned_probe ENGINE FIXTURE".into());
    }
    let mode = args[1].as_str();
    if !["palim-borrowed", "json-patch", "owned-ready", "owned-clone"].contains(&mode) {
        return Err(format!("unknown engine: {mode}").into());
    }
    let fixture: Value = serde_json::from_slice(&fs::read(&args[2])?)?;
    let left = fixture.get("left").ok_or("missing left")?;
    let right = fixture.get("right").ok_or("missing right")?;
    let raw_left = fixture["left_raw"]
        .as_str()
        .ok_or("missing raw left")?
        .as_bytes();
    let raw_right = fixture["right_raw"]
        .as_str()
        .ok_or("missing raw right")?
        .as_bytes();
    assert_eq!(serde_json::from_slice::<Value>(raw_left)?, *left);
    assert_eq!(serde_json::from_slice::<Value>(raw_right)?, *right);
    let initial = if mode.starts_with("owned-") {
        diff_json_patch_owned(left, right.clone())?
    } else {
        borrowed(mode, left, right)?
    };
    verify(left, right, &initial)?;
    // On these supported fixtures, compare exact output against Palim's existing
    // plain path as well as independently applying serialized operations.
    assert_eq!(initial, borrowed("palim-borrowed", left, right)?);
    let wire = serde_json::to_vec(&initial)?;
    let mut counts = std::collections::BTreeMap::<&str, usize>::new();
    for op in &initial.0 {
        let kind = match op {
            PatchOperation::Add(_) => "add",
            PatchOperation::Replace(_) => "replace",
            _ => "other",
        };
        *counts.entry(kind).or_default() += 1;
    }
    drop(initial);
    let (core, generation, core_iterations) = measure(|| {
        let prepared = (mode == "owned-ready").then(|| right.clone());
        let started = Instant::now();
        let result = if let Some(target) = prepared {
            diff_json_patch_owned(black_box(left), target)?
        } else if mode == "owned-clone" {
            diff_json_patch_owned(black_box(left), black_box(right).clone())?
        } else {
            borrowed(mode, black_box(left), black_box(right))?
        };
        let generated = started.elapsed().as_secs_f64() * 1000.;
        drop(black_box(result));
        Ok((started.elapsed().as_secs_f64() * 1000., generated))
    })?;
    let pipeline = if mode == "owned-clone" {
        None
    } else {
        Some(measure(|| {
            let started = Instant::now();
            let left: Value = serde_json::from_slice(black_box(raw_left))?;
            let right: Value = serde_json::from_slice(black_box(raw_right))?;
            let result = if mode == "owned-ready" {
                diff_json_patch_owned(&left, right)?
            } else {
                let result = borrowed(mode, &left, &right)?;
                drop(right);
                result
            };
            drop(black_box(serde_json::to_vec(&result)?));
            drop(result);
            drop(left);
            let elapsed = started.elapsed().as_secs_f64() * 1000.;
            Ok((elapsed, elapsed))
        })?)
    };
    println!(
        "{}",
        json!({"fixture":fixture["name"],"engine":mode,
            "core_samples_ms":core,"core_iterations":core_iterations,
            "generation_samples_ms":generation,
            "pipeline_samples_ms":pipeline.as_ref().map(|v| &v.0),
            "pipeline_iterations":pipeline.as_ref().map(|v| v.2),
            "wire_bytes":wire.len(),"wire":String::from_utf8(wire)?,
            "operation_counts":counts,"forward_verified":true,"inverse_verified":true,
            "plain_output_identical":true})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_changes_keep_wire_contract_and_move_storage() {
        let cases = [
            (json!({}), json!({"~/": [{"nested": "payload"}]})),
            (json!({"a": 1}), json!({"a": {"nested": [1, 2]}})),
            (json!([1, 2]), json!("root replacement")),
            (
                json!({"a": 1, "b": {"c": false}}),
                json!({"a": 2, "b": {"c": true, "d": 3}}),
            ),
            (json!({"a": [1, 2]}), json!({"a": [1, 2]})),
        ];
        for (left, right) in cases {
            let actual = diff_json_patch_owned(&left, right.clone()).unwrap();
            assert_eq!(actual, borrowed("palim-borrowed", &left, &right).unwrap());
            verify(&left, &right, &actual).unwrap();
        }
        let target = json!({"added": ["owned string", {"nested": true}]});
        let storage = target["added"].as_array().unwrap().as_ptr();
        let result = diff_json_patch_owned(&json!({}), target).unwrap();
        let PatchOperation::Add(add) = &result.0[0] else {
            panic!("expected Add")
        };
        assert_eq!(storage, add.value.as_array().unwrap().as_ptr());
    }

    #[test]
    fn numeric_additions_and_replacements_keep_distinct_contracts() {
        let object = |raw: &str| {
            Value::Object(serde_json::Map::from_iter([(
                "n".into(),
                Value::Number(serde_json::Number::from_string_unchecked(raw.into())),
            )]))
        };
        for raw in [
            "-0",
            "1E2",
            "1e2",
            "10000000000000000000000000",
            "1e+999999",
        ] {
            for (left, right) in [(json!({}), object(raw)), (json!({"n": 7}), object(raw))] {
                assert_eq!(
                    diff_json_patch_owned(&left, right.clone()).unwrap(),
                    borrowed("palim-borrowed", &left, &right).unwrap()
                );
            }
        }
        let malformed = object("invalid");
        assert!(
            std::panic::catch_unwind(|| diff_json_patch_owned(&json!({}), malformed.clone()))
                .is_err()
        );
        assert!(
            std::panic::catch_unwind(|| borrowed("palim-borrowed", &json!({}), &malformed))
                .is_err()
        );
    }

    #[test]
    fn depth_errors_match_and_rejected_owned_input_drops_iteratively() {
        for depth in 124..=129 {
            let target = (0..depth).fold(json!({"added": []}), |v, _| json!({"a": v}));
            let left = (0..depth).fold(json!({}), |v, _| json!({"a": v}));
            assert_eq!(
                diff_json_patch_owned(&left, target.clone()),
                DiffPatcher::default().diff_json_patch(&left, &target, &plain())
            );
        }
        let very_deep = (0..100_000).fold(Value::Null, |v, _| Value::Array(vec![v]));
        assert_eq!(
            diff_json_patch_owned(&Value::Null, very_deep).unwrap_err(),
            Error::new("", "JSON nesting exceeds max_depth")
        );
    }

    #[test]
    fn unsupported_changes_return_errors_instead_of_partial_patches() {
        assert_eq!(
            diff_json_patch_owned(&json!({"a": 1, "b": 2}), json!({"a": 3}))
                .unwrap_err()
                .path,
            "/b"
        );
        assert_eq!(
            diff_json_patch_owned(&json!({"a": [1]}), json!({"a": [2]}))
                .unwrap_err()
                .path,
            "/a"
        );
    }
}
