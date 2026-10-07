//! Report configuration changes without generating an applicable patch.
//! Run with `cargo run --example config_report --locked`.
use palim::{CompareOptions, compare};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let before = json!({
        "http": {"port": 8080, "timeout_seconds": 30},
        "logging": {"level": "info"},
        "legacy_metrics": true
    });
    let after = json!({
        "http": {"port": 8081, "timeout_seconds": 30},
        "logging": {"level": "debug"},
        "health_check": {"path": "/health"}
    });
    let report = compare(&before, &after, &CompareOptions::default())?;
    for difference in &report.differences {
        match (&difference.left, &difference.right) {
            (None, Some(value)) => println!("added   {}: {value}", difference.path),
            (Some(value), None) => println!("removed {}: {value}", difference.path),
            (Some(old), Some(new)) => {
                println!("changed {}: {old} -> {new}", difference.path);
            }
            (None, None) => unreachable!("a difference has at least one value"),
        }
    }
    println!("{} changed paths", report.differences.len());
    // None means absence; a present JSON null is Some(Value::Null).
    // CompareReport is for inspection, not replay or undo.
    Ok(())
}
