//! Temporary developer-only allocation exploration; not part of the production crate.
//! Counts requested allocation bytes, not resident bytes or unique/live memory.
use jsondiffpatch_rs::{Delta, DiffOptions, DiffPatcher, JsonPatchOptions, Patch};
use serde_json::{Number, Value, json, value::RawValue};
use std::{alloc::{GlobalAlloc, Layout, System}, collections::HashMap, hash::{Hash, Hasher}, hint::black_box, sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}}, time::Instant};

#[path = "/Users/apple/Desktop/code/AI/tool/secrets/jsondiffpatch-rs/src/numbers.rs"]
mod numbers;

static ENABLED: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);
static REALLOCS: AtomicUsize = AtomicUsize::new(0);
static REALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);
struct Counter;
// SAFETY: Every allocation operation forwards its original pointer and layout
// unchanged to System. Counter updates allocate no memory, retain no pointers,
// and do not change System's allocation/deallocation contract.
unsafe impl GlobalAlloc for Counter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: GlobalAlloc's caller supplies a valid allocation layout.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() && ENABLED.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: GlobalAlloc's caller supplies a valid allocation layout.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() && ENABLED.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: GlobalAlloc's caller supplies a live System-allocated pointer
        // and its matching original layout; this allocator never changes them.
        unsafe { System.dealloc(pointer, layout) };
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: GlobalAlloc's caller supplies a live allocation and a valid
        // new nonzero size; forwarding preserves the allocator contract.
        let next = unsafe { System.realloc(pointer, layout, size) };
        if !next.is_null() && ENABLED.load(Ordering::Relaxed) {
            REALLOCS.fetch_add(1, Ordering::Relaxed);
            REALLOC_BYTES.fetch_add(size, Ordering::Relaxed);
        }
        next
    }
}
#[global_allocator]
static GLOBAL: Counter = Counter;

fn measure<I, O>(name: &str, phase: &str, mut prepare: impl FnMut() -> I, mut work: impl FnMut(I) -> O, mut check: impl FnMut(&O), rows: &mut Vec<Value>) {
    // Warmups include checks with counting disabled. Each measured output is
    // checked and dropped after the counter stops. Setup is outside the scope.
    for _ in 0..2 { let result = work(prepare()); check(&result); }
    let mut samples = Vec::new();
    for _ in 0..5 {
        let input = prepare();
        for counter in [&ALLOCS, &ALLOC_BYTES, &REALLOCS, &REALLOC_BYTES] { counter.store(0, Ordering::Relaxed); }
        let started = Instant::now();
        ENABLED.store(true, Ordering::Relaxed);
        let result = black_box(work(input));
        ENABLED.store(false, Ordering::Relaxed);
        let micros = started.elapsed().as_nanos() as f64 / 1000.0;
        let allocations = ALLOCS.load(Ordering::Relaxed);
        let allocation_bytes = ALLOC_BYTES.load(Ordering::Relaxed);
        let reallocations = REALLOCS.load(Ordering::Relaxed);
        let realloc_requested_bytes = REALLOC_BYTES.load(Ordering::Relaxed);
        check(&result);
        samples.push(json!({"allocations":allocations,"allocation_bytes":allocation_bytes,"reallocations":reallocations,"realloc_requested_bytes":realloc_requested_bytes,"gross_requested_bytes":allocation_bytes+realloc_requested_bytes,"instrumented_us":micros}));
    }
    rows.push(json!({"case":name,"phase":phase,"samples":samples}));
}
fn engine() -> DiffPatcher {
    DiffPatcher::new(DiffOptions {object_hash:Some(Arc::new(|value, _| value.get("id").map(Value::to_string))),..Default::default()})
}
fn fixtures() -> Vec<(String,Value,Value)> {
    let base="/Users/apple/Desktop/code/AI/tool/secrets/jsondiffpatch-rs/results/fixtures";
    let mut cases=Vec::new();
    for file in ["keyed-rotate-edit-2000.json","long-text-ascii.json","long-text-unicode.json"] {
        let value: Value = serde_json::from_str(&std::fs::read_to_string(format!("{base}/{file}")).unwrap()).unwrap();
        cases.push((value["name"].as_str().unwrap().to_owned(),value["left"].clone(),value["right"].clone()));
    }
    let left=json!({"subtree":{"blob":"x".repeat(1024*1024),"enabled":false},"keep":42});
    let mut right=left.clone();right["subtree"]["enabled"]=json!(true);
    cases.push(("one-mib-subtree-single-field".into(),left,right));
    cases.push(("one-mib-root-replacement".into(),json!({"payload":"x".repeat(1024*1024)}),Value::Null));
    cases
}
fn main() {
    let mut rows=Vec::new();
    let mut metadata=Vec::new();
    let engine=engine();
    for (name,left,right) in fixtures() {
        let left_json=serde_json::to_string(&left).unwrap();
        let right_json=serde_json::to_string(&right).unwrap();
        let delta=engine.diff(&left,&right).unwrap().unwrap();
        let native_wire=serde_json::to_vec(&delta).unwrap();
        let standard=delta.to_json_patch(&left).unwrap();
        metadata.push(json!({"case":name,"left_json_bytes":left_json.len(),"right_json_bytes":right_json.len(),"native_delta_bytes":native_wire.len(),"rfc_unoptimized_bytes":serde_json::to_vec(&standard).unwrap().len()}));
        let value_check=|value:&Value| assert_eq!(value,&right);
        let wire_check=|wire:&Vec<u8>| assert_eq!(wire,&native_wire);
        let patch_check=|patch:&Patch| assert_eq!(jsondiffpatch_rs::apply_json_patch(&left,patch).unwrap(),right);
        measure(&name,"baseline_value_clone",||(),|()| left.clone(),|v:&Value|assert_eq!(v,&left),&mut rows);
        measure(&name,"delta_value_clone",||(),|()|delta.as_value().clone(),|v:&Value|assert_eq!(v,delta.as_value()),&mut rows);
        measure(&name,"native_diff_parsed",||(),|()|engine.diff(&left,&right).unwrap().unwrap(),|d:&Delta|assert_eq!(d,&delta),&mut rows);
        measure(&name,"native_patch_borrowed",||(),|()|jsondiffpatch_rs::patch(&left,&delta).unwrap(),value_check,&mut rows);
        measure(&name,"native_patch_owned_transfer",||left.clone(),|input|jsondiffpatch_rs::patch_owned(input,&delta).unwrap(),value_check,&mut rows);
        measure(&name,"native_patch_owned_copy_inside",||(),|()|jsondiffpatch_rs::patch_owned(left.clone(),&delta).unwrap(),value_check,&mut rows);
        measure(&name,"rfc_apply_borrowed",||(),|()|jsondiffpatch_rs::apply_json_patch(&left,&standard).unwrap(),value_check,&mut rows);
        measure(&name,"rfc_apply_owned_transfer",||left.clone(),|input|jsondiffpatch_rs::apply_json_patch_owned(input,&standard).unwrap(),value_check,&mut rows);
        measure(&name,"rfc_apply_owned_copy_inside",||(),|()|jsondiffpatch_rs::apply_json_patch_owned(left.clone(),&standard).unwrap(),value_check,&mut rows);
        measure(&name,"native_delta_serialize_to_vec",||(),|()|serde_json::to_vec(&delta).unwrap(),wire_check,&mut rows);
        measure(&name,"native_delta_serialize_writer_presized",||(),|()|{let mut output=Vec::with_capacity(native_wire.len());serde_json::to_writer(&mut output,&delta).unwrap();output},wire_check,&mut rows);
        measure(&name,"native_delta_serialize_writer_reused",||Vec::with_capacity(native_wire.len()),|mut output|{serde_json::to_writer(&mut output,&delta).unwrap();output},wire_check,&mut rows);
        measure(&name,"native_pipeline_parse_diff_serialize",||(),|()|{let a:Value=serde_json::from_str(&left_json).unwrap();let b:Value=serde_json::from_str(&right_json).unwrap();serde_json::to_vec(&engine.diff(&a,&b).unwrap().unwrap()).unwrap()},wire_check,&mut rows);
        measure(&name,"rfc_export_from_existing_delta",||(),|()|delta.to_json_patch(&left).unwrap(),patch_check,&mut rows);
        let plain=JsonPatchOptions{factorize:false,rationalize:false,tests:false};
        let factored=JsonPatchOptions{factorize:true,rationalize:false,tests:false};
        let rationalized=JsonPatchOptions{factorize:false,rationalize:true,tests:false};
        for (phase,options) in [("rfc_generate_no_optim",&plain),("rfc_generate_factorize",&factored),("rfc_generate_rationalize",&rationalized),("rfc_generate_default",&JsonPatchOptions::default())] {
            measure(&name,phase,||(),|()|engine.diff_json_patch(&left,&right,options).unwrap(),patch_check,&mut rows);
        }
        measure(&name,"rfc_pipeline_parse_diff_serialize",||(),|()|{let a:Value=serde_json::from_str(&left_json).unwrap();let b:Value=serde_json::from_str(&right_json).unwrap();let patch=engine.diff_json_patch(&a,&b,&JsonPatchOptions::default()).unwrap();serde_json::to_vec(&patch).unwrap()},|wire:&Vec<u8>|{let patch:Patch=serde_json::from_slice(wire).unwrap();patch_check(&patch)},&mut rows);
        measure(&name,"parse_left_value",||(),|()|serde_json::from_str::<Value>(&left_json).unwrap(),|v:&Value|assert_eq!(v,&left),&mut rows);
        measure(&name,"parse_left_raw_value",||(),|()|serde_json::from_str::<&RawValue>(&left_json).unwrap(),|v:&&RawValue|assert_eq!(v.get(),left_json),&mut rows);
        if name == "one-mib-root-replacement" {
            measure(&name,"single_leaf_borrowed_delta_serialize",||(),|()|serde_json::to_vec(&[&left,&right]).unwrap(),wire_check,&mut rows);
            measure(&name,"single_leaf_raw_delta_parse_serialize",||(),|()|{let a:&RawValue=serde_json::from_str(&left_json).unwrap();let b:&RawValue=serde_json::from_str(&right_json).unwrap();serde_json::to_vec(&[a,b]).unwrap()},wire_check,&mut rows);
        }
    }
    for (name,unique) in [("decimal-2000-repeat-100",100),("decimal-2000-unique",2000),("integer-2000-repeat-100",100)] {
        let values:Vec<Number>=(0..2000).map(|i|{let text=if name.starts_with("integer") {format!("{}",i%unique)} else {format!("1234567890.{:04}00000000000000000000000",i%unique)};serde_json::from_str(&text).unwrap()}).collect();
        let compute=||{let mut hash=std::collections::hash_map::DefaultHasher::new();for _ in 0..4 {for value in &values {numbers::number_key(value).hash(&mut hash);}}hash.finish()};
        let expected=compute();
        measure(name,"canonical_uncached_four_passes",||(),|()|compute(),|hash:&u64|assert_eq!(*hash,expected),&mut rows);
        measure(name,"canonical_local_cache_four_passes",||(),|()|{let mut keys=HashMap::new();let mut hash=std::collections::hash_map::DefaultHasher::new();for _ in 0..4 {for value in &values {keys.entry(value.as_str()).or_insert_with(||numbers::number_key(value)).hash(&mut hash);}}hash.finish()},|hash:&u64|assert_eq!(*hash,expected),&mut rows);
    }
    println!("{}",serde_json::to_string_pretty(&json!({"method":{"samples":5,"warmups":2,"allocation_bytes":"sum of requested sizes for successful alloc/alloc_zeroed","realloc_requested_bytes":"sum of requested NEW sizes, not incremental memory","gross_requested_bytes":"allocation bytes + realloc requested sizes, not resident/live/unique memory","instrumented_us":"exploration only; allocation counter and concurrent agents affect CPU timing","transfer_setup":"owned-transfer clones fixture before measured scope; copy-inside charges that clone"},"cases":metadata,"rows":rows})).unwrap());
}
