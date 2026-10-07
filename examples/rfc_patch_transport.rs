//! Encode an RFC 6902 patch, apply it at a consumer, and reject a stale baseline.
//! Run with `cargo run --example rfc_patch_transport --locked`.
use palim::{JsonPatchOptions, Patch, apply_json_patch, diff_json_patch, invert_json_patch};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let before = json!({"title": "Draft", "published": false});
    let after = json!({"title": "Final", "published": true});
    let patch = diff_json_patch(
        &before,
        &after,
        &JsonPatchOptions {
            tests: true,
            rationalize: false,
            ..Default::default()
        },
    )?;

    // These JSON bytes could be an HTTP body or a queue message; no network here.
    let body = serde_json::to_vec(&patch)?;
    println!("{}", std::str::from_utf8(&body)?);

    // The consumer decodes the standard operations and applies them atomically.
    let received: Patch = serde_json::from_slice(&body)?;
    let updated = apply_json_patch(&before, &received)?;
    assert_eq!(updated, after);

    // Tests guard the touched baseline values, not every field in the document.
    let stale = json!({"title": "Someone else's draft", "published": false});
    assert!(apply_json_patch(&stale, &received).is_err());

    // RFC patches omit old values, so inversion also needs the original baseline.
    let undo = invert_json_patch(&before, &received)?;
    assert_eq!(apply_json_patch(&updated, &undo)?, before);
    println!("Applied the transported patch, rejected stale data, and undid the edit");
    Ok(())
}
