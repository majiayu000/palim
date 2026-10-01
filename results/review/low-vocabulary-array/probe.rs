use imara_diff::{Algorithm, Diff, InternedInput, Interner};
use serde_json::{Value, json};
use std::time::Instant;

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let engine = &args[1];
    let period: usize = args[2].parse().unwrap();
    let mode = &args[3];
    let before: Vec<usize> = (0..20_000).map(|index| index % period).collect();
    let mut after = before.clone();
    if mode == "replace3" {
        for (offset, index) in [0, 10_000, 19_999].into_iter().enumerate() {
            after[index] = period + offset;
        }
    } else {
        after.rotate_left(1);
    }
    let mut interner = Interner::new(before.len() + after.len());
    let input = InternedInput {
        before: before.iter().map(|&value| interner.intern(value)).collect(),
        after: after.iter().map(|&value| interner.intern(value)).collect(),
        interner,
    };
    let snapshot = json!({"before":before,"after":after,
        "before_tokens":input.before.iter().map(|&token|u32::from(token)).collect::<Vec<_>>(),
        "after_tokens":input.after.iter().map(|&token|u32::from(token)).collect::<Vec<_>>()});
    std::fs::write(format!("/tmp/jsondiffpatch-low-vocab-period{period}-{mode}.json"),
        serde_json::to_vec(&snapshot).unwrap()).unwrap();
    if engine == "json" {
        let (left, right) = (json!(before), json!(after));
        let start = Instant::now();
        let delta = jsondiffpatch_rs::diff(&left, &right).unwrap().unwrap();
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        let correct = jsondiffpatch_rs::patch(&left, &delta).unwrap() == right;
        let native_ops = delta.as_value().as_object().unwrap().len() - 1;
        println!("{}", json!({"engine":engine,"period":period,"mode":mode,
            "diff_ms":elapsed,"native_ops":native_ops,"correct":correct}));
    } else {
        let algorithm = if engine == "histogram" { Algorithm::Histogram } else { Algorithm::Myers };
        let start = Instant::now();
        let diff = Diff::compute(algorithm, &input);
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        let hunks: Vec<_> = diff.hunks().collect();
        let edits: usize = hunks.iter().map(|hunk|
            (hunk.before.end-hunk.before.start+hunk.after.end-hunk.after.start) as usize).sum();
        println!("{}", json!({"engine":engine,"period":period,"mode":mode,
            "diff_ms":elapsed,"hunks":hunks.len(),"insert_delete_tokens":edits}));
    }
}
