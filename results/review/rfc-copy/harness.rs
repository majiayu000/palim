use jsondiffpatch_rs::{DiffOptions, DiffPatcher, JsonPatchOptions, PatchOperation, apply_json_patch, invert_json_patch};
use serde_json::Value;
use std::{fs, hint::black_box, sync::Arc, time::Instant};

fn main() {
    let file=std::env::args().nth(1).unwrap();
    let input: Value=serde_json::from_slice(&fs::read(file).unwrap()).unwrap();
    let left=&input["left"]; let right=&input["right"];
    let engine=DiffPatcher::new(DiffOptions {object_hash:Some(Arc::new(|v,_|v.get("id").map(Value::to_string))), ..Default::default()});
    for (name, factorize, rationalize, tests) in [
        ("plain", false, false, false), ("factorize", true, false, false),
        ("rationalize", false, true, false), ("optimized", true, true, false),
        ("guarded", false, false, true), ("optimized-guarded", true, true, true)
    ] {
        let options=JsonPatchOptions{factorize,rationalize,tests};
        let mut samples=Vec::new();
        let mut counts=[0usize;6]; let mut size=0;
        for iteration in 0..4 {
            let start=Instant::now();
            let patch=engine.diff_json_patch(black_box(left),black_box(right),black_box(&options)).unwrap();
            let elapsed=start.elapsed().as_secs_f64()*1000.;
            if iteration>0 {samples.push(elapsed);}
            if iteration==0 {
                assert_eq!(apply_json_patch(left,&patch).unwrap(), *right);
                let inverse=invert_json_patch(left,&patch).unwrap();
                assert_eq!(apply_json_patch(right,&inverse).unwrap(), *left);
                for op in &patch.0 {counts[match op {PatchOperation::Add(_)=>0,PatchOperation::Remove(_)=>1,PatchOperation::Replace(_)=>2,PatchOperation::Move(_)=>3,PatchOperation::Copy(_)=>4,PatchOperation::Test(_)=>5}]+=1;}
                size=serde_json::to_vec(&patch).unwrap().len();
            }
            black_box(patch);
        }
        samples.sort_by(f64::total_cmp);
        println!("{name}: median={:.3}ms min={:.3}ms bytes={size} add/remove/replace/move/copy/test={counts:?}",samples[samples.len()/2],samples[0]);
    }
}
