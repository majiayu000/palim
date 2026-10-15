//! Reversible edit histories with an application-owned baseline check.
//! Task-list edits and Helixflow-schema graph edits below are simulated, not
//! captured user history. Run with `cargo run --example id_list_history --locked`.
use palim::{Delta, DiffPatcher, apply_json_patch, patch, unpatch};
use serde_json::{Value, json};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

// This is application metadata, not a Palim version or concurrency protocol.
// A real store must atomically check its current immutable revision when writing.
fn apply_at_revision(
    document: &Value,
    actual_revision: usize,
    saved_base_revision: usize,
    change: &Delta,
) -> Result<Value> {
    if actual_revision != saved_base_revision {
        return Err("saved change belongs to a different baseline revision".into());
    }
    Ok(patch(document, change)?)
}

fn check_history(name: &str, states: &[Value], engine: &DiffPatcher) -> Result<()> {
    let mut document = states[0].clone();
    let mut history = Vec::new();
    let mut current_revision = 0;
    for (base_revision, target) in states.iter().skip(1).enumerate() {
        let change = engine.diff(&document, target)?.ok_or("expected a change")?;
        let standard = change.to_json_patch(&document)?;
        assert_eq!(apply_json_patch(&document, &standard)?, *target);
        document = apply_at_revision(&document, current_revision, base_revision, &change)?;
        current_revision += 1;
        assert_eq!(document, *target);
        history.push((base_revision, change));
    }
    let stored = serde_json::to_vec(&history)?;
    // Store deltas plus the application's base revision, not a full snapshot per action.
    let history: Vec<(usize, Delta)> = serde_json::from_slice(&stored)?;
    for (base, change) in history.iter().rev() {
        assert_eq!(current_revision, base + 1);
        document = unpatch(&document, change)?;
        current_revision = *base;
        assert_eq!(document, states[*base]);
    }
    assert_eq!(document, states[0]);
    for (base, change) in &history {
        document = apply_at_revision(&document, current_revision, *base, change)?;
        current_revision += 1;
        assert_eq!(document, states[base + 1]);
    }
    // Re-open at the saved initial checkpoint and replay decoded stored history.
    let mut reopened: Value = serde_json::from_slice(&serde_json::to_vec(&states[0])?)?;
    let mut reopened_revision = 0;
    for (base, change) in &history {
        reopened = apply_at_revision(&reopened, reopened_revision, *base, change)?;
        reopened_revision = base + 1;
    }
    assert_eq!(reopened, document);
    // Another writer advanced the application revision. Reject before patching;
    // native Delta validates operations, not the identity of the whole baseline.
    let before_rejection = document.clone();
    let error = apply_at_revision(&document, history.len(), history[0].0, &history[0].1)
        .expect_err("stale history must be rejected by the application");
    assert_eq!(document, before_rejection);
    println!(
        "{name}: changes={} stored_bytes={} undo=ok redo=ok reopened_replay=ok stale_baseline={error}",
        history.len(),
        stored.len()
    );
    Ok(())
}

fn main() -> Result<()> {
    let initial = json!([
        {"id": "draft", "title": "Write draft", "done": false},
        {"id": "review", "title": "Review draft", "done": false},
        {"id": "publish", "title": "Publish", "done": false}
    ]);
    // Move the same ID and edit it; then remove and insert another item.
    let reordered = json!([
        {"id": "review", "title": "Review revised draft", "done": false},
        {"id": "draft", "title": "Write draft", "done": true},
        {"id": "publish", "title": "Publish", "done": false}
    ]);
    let final_list = json!([
        {"id": "review", "title": "Review revised draft", "done": false},
        {"id": "checks", "title": "Run checks", "done": false},
        {"id": "publish", "title": "Publish", "done": false}
    ]);
    // An edit to an untouched item can survive a valid patch on the wrong
    // baseline. A successful patch alone is not proof of the intended target.
    let first = DiffPatcher::by_key("id")
        .diff(&initial, &reordered)?
        .unwrap();
    let mut wrong_baseline = initial.clone();
    wrong_baseline[2]["title"] = json!("Changed by another writer");
    assert_ne!(patch(&wrong_baseline, &first)?, reordered);
    assert!(apply_at_revision(&wrong_baseline, 1, 0, &first).is_err());
    println!("wrong-baseline: raw patch can succeed; application revision check rejects it");
    check_history(
        "simulated ID task list",
        &[initial, reordered, final_list],
        &DiffPatcher::by_key("id"),
    )?;

    // Field shape from Helixflow WorkflowGraph / GraphNode / GraphEdge at
    // 2667a007. Nodes are a map and edges have no ID: do not invent ID-array semantics.
    // Values and edit sequence are authored examples, not a production capture.
    let graph = json!({"schema_version":1,"nodes":{
        "source":{"node_type":"input.text","title":"Source","params":{"text":"fixture"},"pos":[0.0,0.0]},
        "model":{"node_type":"model.text","title":"Draft","params":{"temperature":0.75},"pos":[200.0,0.0]}
    },"edges":[{"from":["source","text"],"to":["model","prompt"],"edge_type":"data"}]});
    let mut edited = graph.clone();
    edited["nodes"]["model"]["pos"] = json!([240.5, 40.0]);
    edited["nodes"]["model"]["params"]["temperature"] = json!(0.5);
    let mut extended = edited.clone();
    extended["nodes"]["review"] =
        json!({"node_type":"output.text","title":"Review","params":{},"pos":[440.0,40.0]});
    extended["edges"]
        .as_array_mut()
        .unwrap()
        .push(json!({"from":["model","text"],"to":["review","text"],"edge_type":"data"}));
    check_history(
        "simulated Helixflow-schema graph",
        &[graph, edited, extended],
        &DiffPatcher::default(),
    )?;
    Ok(())
}
