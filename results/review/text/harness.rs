use jsondiffpatch_rs::{Delta, DiffOptions, DiffPatcher, patch, reverse};
use serde_json::{Value, json};
use std::{error::Error, fs};

fn main() -> Result<(), Box<dyn Error>> {
    let input: Vec<Value> = serde_json::from_slice(&fs::read("/tmp/text-review-input.json")?)?;
    let engine = DiffPatcher::new(DiffOptions { text_diff_min_length: Some(1), ..Default::default() });
    let mut output = Vec::new();
    for case in input {
        let left = &case["left"];
        let right = &case["right"];
        let delta = engine.diff(left, right)?.ok_or("expected a change")?;
        let decoded: Delta = serde_json::from_slice(&serde_json::to_vec(&delta)?)?;
        if patch(left, &decoded)? != *right { return Err(format!("forward mismatch: {}", case["name"]).into()); }
        let inverse = reverse(&decoded)?;
        if patch(right, &inverse)? != *left { return Err(format!("inverse mismatch: {}", case["name"]).into()); }
        output.push(json!({"name":case["name"],"delta":decoded,"inverse":inverse}));
    }
    fs::write("/tmp/text-review-output.json", serde_json::to_vec(&output)?)?;
    println!("Rust exact forward + reverse + wire roundtrip: {}/{} passed", output.len(), output.len());
    Ok(())
}
