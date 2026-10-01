//! Standard JSON Patch comparison. Every engine parses the same two byte
//! strings, generates a typed Patch, and serializes it inside the timed loop.
use palim::{
    DiffOptions, DiffPatcher, JsonPatchOptions, Patch, PatchOperation, apply_json_patch,
    invert_json_patch,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, error::Error, fs, hint::black_box, sync::Arc, time::Instant};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn generate(engine: &str, dp: &DiffPatcher, left: &Value, right: &Value) -> Result<Patch> {
    match engine {
        "json-patch" => Ok(json_patch::diff(left, right)),
        "plain" => Ok(dp.diff_json_patch(
            left,
            right,
            &JsonPatchOptions {
                factorize: false,
                rationalize: false,
                tests: false,
            },
        )?),
        "optimized" => Ok(dp.diff_json_patch(left, right, &JsonPatchOptions::default())?),
        "guarded" => Ok(dp.diff_json_patch(
            left,
            right,
            &JsonPatchOptions {
                factorize: false,
                rationalize: false,
                tests: true,
            },
        )?),
        _ => Err(format!("unknown engine: {engine}").into()),
    }
}

fn pipeline(engine: &str, dp: &DiffPatcher, left: &[u8], right: &[u8]) -> Result<Vec<u8>> {
    let left: Value = serde_json::from_slice(left)?;
    let right: Value = serde_json::from_slice(right)?;
    let patch = generate(engine, dp, &left, &right)?;
    Ok(serde_json::to_vec(&patch)?)
}

fn operation_counts(patch: &Patch) -> BTreeMap<&'static str, usize> {
    let mut counts = BTreeMap::new();
    for op in &patch.0 {
        let name = match op {
            PatchOperation::Add(_) => "add",
            PatchOperation::Remove(_) => "remove",
            PatchOperation::Replace(_) => "replace",
            PatchOperation::Move(_) => "move",
            PatchOperation::Copy(_) => "copy",
            PatchOperation::Test(_) => "test",
        };
        *counts.entry(name).or_default() += 1;
    }
    counts
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 3 {
        return Err(
            "usage: standard_bench ENGINE FILE [--task pipeline|export|apply|inverse] [--dry-run]"
                .into(),
        );
    }
    let engine = &args[1];
    let mut dry_run = false;
    let mut task = "pipeline";
    let mut index = 3;
    while index < args.len() {
        match args[index].as_str() {
            "--dry-run" => dry_run = true,
            "--task" if index + 1 < args.len() => {
                index += 1;
                task = &args[index];
            }
            flag => return Err(format!("unknown or incomplete argument: {flag}").into()),
        }
        index += 1;
    }
    if !["pipeline", "export", "apply", "inverse"].contains(&task) {
        return Err(format!("unknown task: {task}").into());
    }
    if (task == "export" || task == "inverse") && engine != "plain" {
        return Err("export and inverse are native API tasks; select engine plain".into());
    }
    if task == "apply" && !["plain", "json-patch"].contains(&engine.as_str()) {
        return Err(
            "application compares plain and typed json-patch on identical supplied operations"
                .into(),
        );
    }
    let fixture: Value = serde_json::from_slice(&fs::read(&args[2])?)?;
    let left = fixture.get("left").ok_or("fixture has no left document")?;
    let right = fixture
        .get("right")
        .ok_or("fixture has no right document")?;
    let dp = DiffPatcher::new(DiffOptions {
        object_hash: Some(Arc::new(|v, _| v.get("id").map(Value::to_string))),
        ..Default::default()
    });
    let raw_left = serde_json::to_vec(left)?;
    let raw_right = serde_json::to_vec(right)?;
    let native = if task == "export" {
        dp.diff(left, right)?
    } else {
        None
    };
    let mut change = if task == "export" {
        match &native {
            Some(delta) => delta.to_json_patch(left)?,
            None => Patch::default(),
        }
    } else if task != "pipeline" && fixture.get("patch").is_some() {
        serde_json::from_value(fixture["patch"].clone())?
    } else {
        generate(engine, &dp, left, right)?
    };
    let mut applied = left.clone();
    json_patch::patch(&mut applied, &change)?;
    assert_eq!(applied, *right, "standard patch failed for {engine}");
    let apply_inverse = task == "apply" && fixture["apply_inverse"] == true;
    let (baseline, expected) = if apply_inverse {
        change = invert_json_patch(left, &change)?;
        (right, left)
    } else {
        (left, right)
    };
    let execute = || -> Result<Vec<u8>> {
        match task {
            "pipeline" => pipeline(engine, &dp, &raw_left, &raw_right),
            "export" => Ok(serde_json::to_vec(&match &native {
                Some(delta) => delta.to_json_patch(left)?,
                None => Patch::default(),
            })?),
            "apply" => {
                let result = if engine == "json-patch" {
                    let mut result = baseline.clone();
                    json_patch::patch(&mut result, &change)?;
                    result
                } else {
                    apply_json_patch(baseline, &change)?
                };
                Ok(serde_json::to_vec(&result)?)
            }
            "inverse" => Ok(serde_json::to_vec(&invert_json_patch(left, &change)?)?),
            _ => unreachable!("task was checked above"),
        }
    };
    let serialized = execute()?;
    // Verify the actual serialized typed result through an independent standard
    // application API before any timings. A failed patch is never ranked.
    let reported = if task == "apply" {
        assert_eq!(serde_json::from_slice::<Value>(&serialized)?, *expected);
        change.clone()
    } else {
        let produced: Patch = serde_json::from_slice(&serialized)?;
        let mut applied = if task == "inverse" {
            right.clone()
        } else {
            left.clone()
        };
        json_patch::patch(&mut applied, &produced)?;
        assert_eq!(
            applied,
            if task == "inverse" {
                left.clone()
            } else {
                right.clone()
            }
        );
        produced
    };
    let counts = operation_counts(&reported);
    if fixture["name"] == "halfswap-500" && engine == "plain" && task != "inverse" {
        assert_eq!(counts.get("move"), Some(&250));
    }
    let boundary = match task {
        "pipeline" => "parse both documents -> typed standard diff -> serialize Patch",
        "export" => "precomputed native Delta -> typed RFC export -> serialize Patch",
        "apply" => {
            "immutable baseline -> apply supplied typed Patch including baseline copy -> serialize document"
        }
        "inverse" => "baseline and supplied typed Patch -> inverse -> serialize Patch",
        _ => unreachable!(),
    };
    let mut result = json!({
        "name": fixture["name"], "engine": engine, "ok": true,
        "task": task,
        "dry_run": dry_run, "measured": !dry_run,
        "input_bytes": raw_left.len() + raw_right.len(),
        "patch_bytes": serde_json::to_vec(&reported)?.len(), "output_bytes": serialized.len(),
        "operations": reported.0.len(),
        "operation_counts": counts,
        "boundary": boundary,
        "samples_ms": [], "iterations_per_sample": 0,
    });
    let mut samples = Vec::new();
    if !dry_run {
        for _ in 0..20 {
            black_box(execute()?);
        }
        // Calibrate the metric being measured. Fast fixtures can exceed 200
        // iterations so that microsecond work is not sampled for only 1 ms.
        let mut iterations = 1;
        loop {
            let start = Instant::now();
            for _ in 0..iterations {
                black_box(execute()?);
            }
            if start.elapsed().as_secs_f64() >= 0.025 || iterations >= 16_384 {
                break;
            }
            iterations *= 2;
        }
        samples.reserve(10);
        for _ in 0..10 {
            let start = Instant::now();
            for _ in 0..iterations {
                black_box(execute()?);
            }
            samples.push(start.elapsed().as_secs_f64() * 1000.0 / iterations as f64);
        }
        let mut sorted = samples.clone();
        sorted.sort_by(f64::total_cmp);
        result[format!("{task}_ms")] = json!((sorted[4] + sorted[5]) / 2.0);
        result["samples_ms"] = json!(samples);
        result["iterations_per_sample"] = json!(iterations);
    }
    if task == "pipeline" {
        result["pipeline_samples_ms"] = json!(samples);
    }
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
