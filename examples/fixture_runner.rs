//! Development harness: JSON inputs on stdin/file, machine-readable checks and timings.
use palim::{Delta, DiffOptions, DiffPatcher, apply_json_patch, patch, reverse};
use serde_json::{Value, json};
use std::{error::Error, fs, hint::black_box, sync::Arc, time::Instant};
type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn run(dp: &DiffPatcher, input: &Value, mode: &str) -> Result<Value> {
    let (a, b) = (&input["left"], &input["right"]);
    let delta = if mode == "apply" {
        if input["delta"].is_null() {
            None
        } else {
            Some(Delta::from_value(input["delta"].clone())?)
        }
    } else {
        dp.diff(a, b)?
    };
    let actual = if let Some(d) = &delta {
        patch(a, d)?
    } else {
        a.clone()
    };
    if actual != *b {
        return Err("forward mismatch".into());
    }
    let inverse = delta.as_ref().map(reverse).transpose()?;
    let original = if let Some(d) = &inverse {
        patch(b, d)?
    } else {
        b.clone()
    };
    if original != *a {
        return Err("inverse mismatch".into());
    }
    let bytes = serde_json::to_vec(&delta)?.len();
    let mut output =
        json!({"name":input["name"],"ok":true,"patch_bytes":bytes,"delta":delta,"inverse":inverse});
    if mode == "apply" && !input["inverse"].is_null() {
        let upstream = Delta::from_value(input["inverse"].clone()).and_then(|d| patch(b, &d));
        output["upstream_inverse_ok"] = json!(upstream.as_ref().is_ok_and(|v| v == a));
        if let Err(e) = upstream {
            output["upstream_inverse_error"] = json!(e.to_string());
        }
    }
    if mode == "generate" || mode == "apply" {
        if let Some(d) = &delta {
            let p = d.to_json_patch(a)?;
            if apply_json_patch(a, &p)? != *b {
                return Err("RFC export mismatch".into());
            }
            output["json_patch"] = serde_json::to_value(p)?;
        } else {
            output["json_patch"] = json!([]);
        }
    }
    if mode == "benchmark" {
        let raw_a = serde_json::to_vec(a)?;
        let raw_b = serde_json::to_vec(b)?;
        for _ in 0..20 {
            black_box(dp.diff(a, b)?);
        }
        let start = Instant::now();
        black_box(dp.diff(a, b)?);
        let iterations = (0.025 / start.elapsed().as_secs_f64().max(0.000001))
            .floor()
            .clamp(1., 200.) as usize;
        let mut diff_samples = Vec::new();
        let mut pipeline_samples = Vec::new();
        let mut patch_samples = Vec::new();
        let mut reverse_samples = Vec::new();
        for _ in 0..5 {
            let start = Instant::now();
            for _ in 0..iterations {
                black_box(dp.diff(a, b)?);
            }
            diff_samples.push(start.elapsed().as_secs_f64() * 1000. / iterations as f64);
            let start = Instant::now();
            for _ in 0..iterations {
                let a: Value = serde_json::from_slice(&raw_a)?;
                let b: Value = serde_json::from_slice(&raw_b)?;
                black_box(serde_json::to_vec(&dp.diff(&a, &b)?)?);
            }
            pipeline_samples.push(start.elapsed().as_secs_f64() * 1000. / iterations as f64);
            let start = Instant::now();
            for _ in 0..iterations {
                if let Some(d) = &delta {
                    black_box(patch(a, d)?);
                } else {
                    black_box(a.clone());
                }
            }
            patch_samples.push(start.elapsed().as_secs_f64() * 1000. / iterations as f64);
            let start = Instant::now();
            for _ in 0..iterations {
                black_box(delta.as_ref().map(reverse).transpose()?);
            }
            reverse_samples.push(start.elapsed().as_secs_f64() * 1000. / iterations as f64);
        }
        diff_samples.sort_by(f64::total_cmp);
        pipeline_samples.sort_by(f64::total_cmp);
        patch_samples.sort_by(f64::total_cmp);
        reverse_samples.sort_by(f64::total_cmp);
        output = json!({"name":input["name"],"engine":"palim","ok":true,"patch_bytes":bytes,
            "diff_ms":diff_samples[2],"pipeline_ms":pipeline_samples[2],"patch_ms":patch_samples[2],"reverse_ms":reverse_samples[2],
            "diff_samples_ms":diff_samples,"pipeline_samples_ms":pipeline_samples,"iterations":iterations});
    }
    Ok(output)
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 || !["generate", "apply", "benchmark"].contains(&args[1].as_str()) {
        return Err("usage: fixture_runner generate|apply|benchmark FILE".into());
    }
    let data: Value = serde_json::from_slice(&fs::read(&args[2])?)?;
    let inputs = data
        .as_array()
        .map_or_else(|| vec![&data], |a| a.iter().collect());
    let dp = DiffPatcher::new(DiffOptions {
        object_hash: Some(Arc::new(|v, _| v.get("id").map(Value::to_string))),
        ..Default::default()
    });
    let mut failed = false;
    let mut outputs = Vec::new();
    for input in inputs {
        match run(&dp, input, &args[1]) {
            Ok(v) => outputs.push(v),
            Err(e) => {
                failed = true;
                outputs.push(json!({"name":input["name"],"ok":false,"error":e.to_string()}));
            }
        }
    }
    println!("{}", serde_json::to_string(&outputs)?);
    if failed {
        std::process::exit(1);
    }
    Ok(())
}
