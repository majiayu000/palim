#!/usr/bin/env python3
"""Freeze two source trees; measure public APIs and allocator requests separately."""
import hashlib
import io
import json
from pathlib import Path
import platform
import shutil
import statistics
import subprocess
import sys
import time
import zipfile

baseline, candidate, output = [Path(arg).resolve() for arg in sys.argv[1:4]]
output.mkdir(parents=True, exist_ok=False)

RUST = r'''
use palim::{Delta,DiffOptions,DiffPatcher,JsonPatchOptions,Patch};
use serde_json::{Value,json};
use std::{error::Error,fs,hint::black_box,sync::Arc,time::Instant};
type Result<T> = std::result::Result<T,Box<dyn Error>>;
enum Output { Native(Option<Delta>), Standard(Patch) }
fn generate(dp:&DiffPatcher,a:&Value,b:&Value,mode:&str)->Result<Output> {
    Ok(if mode=="native" { Output::Native(dp.diff(a,b)?) } else {
        Output::Standard(dp.diff_json_patch(a,b,&JsonPatchOptions {
            factorize:true,rationalize:true,tests:mode=="guarded"
        })?)
    })
}
fn main()->Result<()> {
    let args:Vec<_>=std::env::args().collect();
    let fixture:Value=serde_json::from_slice(&fs::read(&args[1])?)?;
    let mode=&args[2];
    let (a,b)=(&fixture["left"],&fixture["right"]);
    let dp=DiffPatcher::new(DiffOptions {
        object_hash:Some(Arc::new(|v,_|v.get("id").map(Value::to_string))),
        ..Default::default()
    });
    let initial=generate(&dp,a,b,mode)?;
    let (wire,operations)=match &initial {
        Output::Native(Some(delta))=>{
            assert_eq!(palim::patch(a,delta)?,*b);
            assert_eq!(palim::patch(b,&palim::reverse(delta)?)?,*a);
            (serde_json::to_vec(delta)?,None)
        },
        Output::Native(None)=>{assert_eq!(a,b);(b"null".to_vec(),None)},
        Output::Standard(patch)=>{
            let mut result=a.clone();json_patch::patch(&mut result,patch)?;assert_eq!(result,*b);
            let inverse=palim::invert_json_patch(a,patch)?;
            json_patch::patch(&mut result,&inverse)?;assert_eq!(result,*a);
            (serde_json::to_vec(patch)?,Some(patch.0.len()))
        },
    };
    drop(initial);
    for _ in 0..3 { black_box(generate(&dp,a,b,mode)?); }
    let mut samples=Vec::new();
    let mut iterations=1;
    if !counter::ENABLED {
        loop {
            let start=Instant::now();
            for _ in 0..iterations { black_box(generate(&dp,a,b,mode)?); }
            if start.elapsed().as_secs_f64()>=0.01||iterations>=2048 {break;}
            iterations*=2;
        }
        for _ in 0..9 {
            let start=Instant::now();
            for _ in 0..iterations { black_box(generate(&dp,a,b,mode)?); }
            samples.push(start.elapsed().as_secs_f64()*1000.0/iterations as f64);
        }
    }
    let allocation=if counter::ENABLED {
        let mark=counter::begin();
        let generated=generate(&dp,a,b,mode)?;
        black_box(&generated);
        drop(generated);
        Some(counter::finish(mark))
    } else {None};
    println!("{}",json!({"case":fixture["name"],"mode":mode,
        "samples_ms":samples,"iterations":iterations,"allocation":allocation,
        "wire_bytes":wire.len(),"operations":operations,
        "wire":String::from_utf8(wire)?,"roundtrip_verified":true}));
    Ok(())
}
'''

COUNTER = r'''
use std::alloc::{GlobalAlloc,Layout,System};
use std::sync::atomic::{AtomicBool,AtomicIsize,AtomicUsize,Ordering::Relaxed};
use serde_json::{Value,json};
pub const ENABLED:bool=true;
static LIVE:AtomicIsize=AtomicIsize::new(0);
static PEAK:AtomicIsize=AtomicIsize::new(0);
static ACTIVE:AtomicBool=AtomicBool::new(false);
static CALLS:AtomicUsize=AtomicUsize::new(0);
static BYTES:AtomicUsize=AtomicUsize::new(0);
struct Counted;
fn allocated(size:usize) {
    let live=LIVE.fetch_add(size as isize,Relaxed)+size as isize;
    if ACTIVE.load(Relaxed) {
        CALLS.fetch_add(1,Relaxed);BYTES.fetch_add(size,Relaxed);PEAK.fetch_max(live,Relaxed);
    }
}
// This benchmark forwards the original pointer/Layout unchanged to System.
// Statistics never alter allocation success, ownership, size or alignment.
unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self,layout:Layout)->*mut u8 {
        let pointer=unsafe {System.alloc(layout)};
        if !pointer.is_null(){allocated(layout.size());}
        pointer
    }
    unsafe fn alloc_zeroed(&self,layout:Layout)->*mut u8 {
        let pointer=unsafe {System.alloc_zeroed(layout)};
        if !pointer.is_null(){allocated(layout.size());}
        pointer
    }
    unsafe fn dealloc(&self,pointer:*mut u8,layout:Layout) {
        unsafe {System.dealloc(pointer,layout)};
        LIVE.fetch_sub(layout.size() as isize,Relaxed);
    }
    unsafe fn realloc(&self,pointer:*mut u8,layout:Layout,size:usize)->*mut u8 {
        let resized=unsafe {System.realloc(pointer,layout,size)};
        if !resized.is_null(){LIVE.fetch_sub(layout.size() as isize,Relaxed);allocated(size);}
        resized
    }
}
#[global_allocator] static ALLOCATOR:Counted=Counted;
pub fn begin()->isize {
    let mark=LIVE.load(Relaxed);PEAK.store(mark,Relaxed);
    CALLS.store(0,Relaxed);BYTES.store(0,Relaxed);ACTIVE.store(true,Relaxed);mark
}
pub fn finish(mark:isize)->Value {
    ACTIVE.store(false,Relaxed);
    let calls=CALLS.load(Relaxed);
    let bytes=BYTES.load(Relaxed);
    let peak=PEAK.load(Relaxed)-mark;
    let remaining=LIVE.load(Relaxed)-mark;
    json!({"allocation_or_reallocation_calls":calls,
        "requested_bytes":bytes,"peak_additional_live_bytes":peak,
        "final_live_delta_bytes":remaining})
}
'''

STUB = r'''
pub const ENABLED:bool=false;
pub fn begin()->isize{0}
pub fn finish(_:isize)->serde_json::Value{serde_json::Value::Null}
'''

def run(command, cwd=None, timeout=600):
    result = subprocess.run(command, cwd=cwd, capture_output=True, text=True, timeout=timeout)
    if result.returncode:
        raise RuntimeError(f"{command}\n{result.stdout[-2000:]}\n{result.stderr[-4000:]}")
    return result

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

cases = []
frozen = baseline / "results/standard-benchmark-pipeline-20260930T214341.446185Z/fixtures"
for name, mode in [
    ("small-config-edit", "native"), ("small-config-edit", "optimized"),
    ("rotate-2000", "native"), ("disjoint-2000", "optimized"),
    ("ambiguous-ids-2000", "optimized"),
]:
    fixture = json.loads((frozen / f"{name}.json").read_text())
    fixture["name"] = name
    cases.append((fixture, mode))
for count in [100, 400]:
    before = [dict(id=i, a_long_property_name=i, b_long_property_name=i, c_long_property_name=i) for i in range(count)]
    after = [dict(id=i, a_long_property_name=i+100000, b_long_property_name=i+200000, c_long_property_name=i+300000) for i in range(count)]
    for mode in ["optimized", "guarded"]:
        cases.append((dict(name=f"many-accepted-{count}", left=before, right=after), mode))
lines = [f"line {i:05d} Unicode 🦀 café text\n" for i in range(7000)]
old = "".join(lines)
changed = lines.copy()
with zipfile.ZipFile(baseline/"results/core-leadership-20261003/evidence.zip") as outer:
    with zipfile.ZipFile(io.BytesIO(outer.read("baseline-competitors.zip"))) as frozen_text:
        text_fixture = json.loads(frozen_text.read("fixtures/unicode-three-edits.json"))
        cases.append((text_fixture, "native"))
cases.append((dict(name="unicode-disjoint-24000", left="🦀界\n"*24000, right="🚀é\n"*24000), "native"))
cases.append((dict(name="single-text-edit", left=old, right=old+"!"), "native"))

environment = dict(platform=platform.platform(), rustc=run(["rustc","-Vv"]).stdout,
    timing="Loaded values; generation and output destruction; no allocator instrumentation, parse, serialization or verification in timing",
    samples="3 warmups, >=10ms calibration cap2048, 9 batches, 3 independent processes/version, alternating order",
    allocation="Separate instrumented binary; one generation/drop after warmup; requested allocations, not RSS or allocator internals",
    source={}, builds={}, fixtures={}, runner_sha256=sha(Path(__file__)))
binaries = {}
for label, source in [("baseline", baseline), ("candidate", candidate)]:
    snapshot = output/"sources"/label
    snapshot.mkdir(parents=True)
    for name in ["Cargo.toml","Cargo.lock","README.md"]:
        shutil.copy2(source/name,snapshot/name)
    for directory in ["src","benches","examples"]:
        shutil.copytree(source/directory,snapshot/directory)
    environment["source"][label] = dict(commit=run(["git","rev-parse","HEAD"],cwd=source).stdout.strip(),
        hashes={str(path.relative_to(snapshot)):sha(path) for path in snapshot.rglob("*") if path.is_file()})
    project = output/"probes"/label
    (project/"src/bin").mkdir(parents=True)
    for name, counter in [("timing", STUB), ("allocation", COUNTER)]:
        (project/f"src/bin/{name}.rs").write_text("mod counter {"+counter+"}\n"+RUST)
    (project/"Cargo.toml").write_text('[package]\nname="palim-remaining-probe"\nversion="0.0.0"\nedition="2024"\n[dependencies]\npalim={path='+json.dumps(str(snapshot))+'}\nserde_json={version="1.0",features=["float_roundtrip","arbitrary_precision"]}\njson-patch="4.2.0"\n')
    shutil.copy2(source/"Cargo.lock",project/"Cargo.lock")
    built = run(["cargo","build","--release","--offline","--bins"],cwd=project)
    environment["builds"][label] = dict(stderr=built.stderr,lock_sha256=sha(project/"Cargo.lock"))
    for name in ["timing","allocation"]:
        binaries[(label,name)] = project/f"target/release/{name}"
        environment["builds"][label][name+"_sha256"] = sha(binaries[(label,name)])
    print("built",label,flush=True)

(output/"fixtures").mkdir()
paths = {}
for fixture, mode in cases:
    name = fixture["name"]
    path = output/f"fixtures/{name}.json"
    path.write_text(json.dumps(fixture,ensure_ascii=False))
    paths[name] = path
    environment["fixtures"][name] = sha(path)
records = []
for repeat in range(3):
    for index, (fixture, mode) in enumerate(cases):
        labels = ["baseline","candidate"] if (repeat+index)%2==0 else ["candidate","baseline"]
        for kind in ["timing","allocation"]:
            for label in labels:
                started = time.monotonic()
                row = json.loads(run([str(binaries[(label,kind)]),str(paths[fixture["name"]]),mode],timeout=300).stdout)
                row["wire_sha256"] = hashlib.sha256(row.pop("wire").encode()).hexdigest()
                row.update(version=label,kind=kind,repeat=repeat,wall_seconds=time.monotonic()-started)
                if row["samples_ms"]:
                    row["median_ms"] = statistics.median(row["samples_ms"])
                records.append(row)
                (output/"measurements.json").write_text(json.dumps(records,indent=2))
    print("completed process repeat",repeat+1,flush=True)
summary = []
for fixture, mode in cases:
    row = dict(case=fixture["name"],mode=mode)
    selected = [r for r in records if r["case"]==fixture["name"] and r["mode"]==mode]
    assert len({r["wire_sha256"] for r in selected})==1, (fixture["name"],mode,"output changed")
    row["identical_output"] = True
    for label in ["baseline","candidate"]:
        timing = [r for r in selected if r["version"]==label and r["kind"]=="timing"]
        memory = [r for r in selected if r["version"]==label and r["kind"]=="allocation"]
        row[label] = dict(median_ms=statistics.median(r["median_ms"] for r in timing),
            process_medians_ms=[r["median_ms"] for r in timing],wire_bytes=timing[0]["wire_bytes"],
            operations=timing[0]["operations"],allocation=[r["allocation"] for r in memory])
    row["speedup"] = row["baseline"]["median_ms"]/row["candidate"]["median_ms"]
    summary.append(row)
(output/"summary.json").write_text(json.dumps(summary,indent=2))
(output/"environment.json").write_text(json.dumps(environment,indent=2))
print(json.dumps(summary,indent=2))
