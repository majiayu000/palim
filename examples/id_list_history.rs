//! Store reversible task-list changes, then undo and redo on the same baseline.
//! Run with `cargo run --example id_list_history --locked`.
use palim::{Delta, DiffOptions, DiffPatcher, apply_json_patch, patch, unpatch};
use serde_json::{Value, json};
use std::sync::Arc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = DiffPatcher::new(DiffOptions {
        object_hash: Some(Arc::new(|item, _| item.get("id").map(Value::to_string))),
        ..Default::default()
    });
    let initial = json!([
        {"id": "draft", "title": "Write draft", "done": false},
        {"id": "review", "title": "Review draft", "done": false},
        {"id": "publish", "title": "Publish", "done": false}
    ]);
    // Drag review to the front and edit it in the same change.
    let reordered = json!([
        {"id": "review", "title": "Review revised draft", "done": false},
        {"id": "draft", "title": "Write draft", "done": true},
        {"id": "publish", "title": "Publish", "done": false}
    ]);
    // Remove a completed task and insert a new one.
    let final_list = json!([
        {"id": "review", "title": "Review revised draft", "done": false},
        {"id": "checks", "title": "Run checks", "done": false},
        {"id": "publish", "title": "Publish", "done": false}
    ]);

    let mut document = initial.clone();
    let mut history = Vec::new();
    for target in [&reordered, &final_list] {
        let change = engine.diff(&document, target)?.ok_or("expected a change")?;
        // The same change can be sent to an RFC 6902 client.
        let standard = change.to_json_patch(&document)?;
        assert_eq!(apply_json_patch(&document, &standard)?, *target);
        document = patch(&document, &change)?;
        assert_eq!(document, *target);
        history.push(change);
    }
    let stored = serde_json::to_string(&history)?;
    println!("Stored {} changes in {} bytes", history.len(), stored.len());
    println!("{stored}");
    // History contains complete deltas, not one full document per action.
    let history: Vec<Delta> = serde_json::from_str(&stored)?;
    for (change, expected) in history.iter().rev().zip([&reordered, &initial]) {
        document = unpatch(&document, change)?;
        assert_eq!(document, *expected);
    }
    println!("Undo restored the initial list");
    for (change, expected) in history.iter().zip([&reordered, &final_list]) {
        document = patch(&document, change)?;
        assert_eq!(document, *expected);
    }
    println!("Redo restored the final list");
    Ok(())
}
