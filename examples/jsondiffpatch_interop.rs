//! Read a complete JS jsondiffpatch delta, replay it, undo it, and export RFC 6902.
//! Run with `cargo run --example jsondiffpatch_interop --locked`.
use palim::{Delta, apply_json_patch, patch, unpatch};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let before = json!({
        "tasks": [{"id": "a", "title": "Draft"}, {"id": "b", "title": "Review"}],
        "status": "draft"
    });
    let after = json!({
        "tasks": [
            {"id": "b", "title": "Review revised"},
            {"id": "a", "title": "Draft"},
            {"id": "c", "title": "Publish"}
        ],
        "status": "ready"
    });

    // Generated and verified with jsondiffpatch 0.7.6 using this exact JS:
    // import {create} from "jsondiffpatch";
    // const left = {tasks:[{id:"a",title:"Draft"},{id:"b",title:"Review"}],status:"draft"};
    // const right = {tasks:[{id:"b",title:"Review revised"},{id:"a",title:"Draft"},{id:"c",title:"Publish"}],status:"ready"};
    // const engine = create({objectHash: item => JSON.stringify(item.id)});
    // console.log(JSON.stringify(engine.diff(left, right)));
    // Keep the producer's old values: omitRemovedValues must remain false.
    // Array key _1 is the moved source index; 0 and 2 are target indices.
    let received = r#"{
        "tasks": {
            "0": {"title": ["Review", "Review revised"]},
            "2": [{"id": "c", "title": "Publish"}],
            "_t": "a",
            "_1": ["", 0, 3]
        },
        "status": ["draft", "ready"]
    }"#;
    let delta: Delta = serde_json::from_str(received)?;
    let updated = patch(&before, &delta)?;
    assert_eq!(updated, after);
    assert_eq!(unpatch(&updated, &delta)?, before);

    // RFC clients can consume this change without understanding the native delta.
    let standard = delta.to_json_patch(&before)?;
    assert_eq!(apply_json_patch(&before, &standard)?, after);
    println!("{}", serde_json::to_string_pretty(&standard)?);
    println!("Replayed and undid a JS delta, and verified its RFC export");
    Ok(())
}
