#!/usr/bin/env python3
"""Compare loaded-input public APIs, using separate locked release builds."""
import datetime
import hashlib
import json
import platform
from pathlib import Path
import statistics
import subprocess
import sys
import tempfile

baseline, candidate, out = map(lambda s: Path(s).resolve(), sys.argv[1:4])
out.mkdir(parents=True, exist_ok=False)

RUST = r'''
use palim::{Delta,DiffOptions,DiffPatcher,JsonPatchOptions,Patch};
use serde_json::{Value,json};
use std::{error::Error,fs,hint::black_box,sync::Arc,time::Instant};
type Result<T> = std::result::Result<T,Box<dyn Error>>;
enum Output { Native(Option<Delta>), Standard(Patch) }
fn generate(dp:&DiffPatcher,a:&Value,b:&Value,mode:&str)->Result<Output>{
    Ok(match mode {
        "native"|"custom-native"=>Output::Native(dp.diff(a,b)?),
        "plain"=>Output::Standard(dp.diff_json_patch(a,b,&JsonPatchOptions{factorize:false,rationalize:false,tests:false})?),
        "optimized"=>Output::Standard(dp.diff_json_patch(a,b,&JsonPatchOptions::default())?),
        _=>return Err("unknown mode".into()),
    })
}
fn main()->Result<()> {
    let args=std::env::args().collect::<Vec<_>>();
    let mode=&args[1];
    let fixture:Value=serde_json::from_slice(&fs::read(&args[2])?)?;
    let (a,b)=(&fixture["left"],&fixture["right"]);
    let dp=DiffPatcher::new(if mode=="custom-native" {
        DiffOptions { array_item_matcher:Some(Arc::new(|_,_,_|false)),..Default::default() }
    } else {
        DiffOptions { object_hash:Some(Arc::new(|v,_|v.get("id").map(Value::to_string))),..Default::default() }
    });
    let initial=generate(&dp,a,b,mode)?;
    let (wire,operations)=match initial {
        Output::Native(Some(delta))=>{
            assert_eq!(dp.patch(a,&delta)?,*b);
            assert_eq!(dp.unpatch(b,&delta)?,*a);
            assert_eq!(dp.patch(b,&dp.reverse(&delta)?)?,*a);
            (serde_json::to_vec(&delta)?,None)
        },
        Output::Native(None)=>{assert_eq!(a,b);(b"null".to_vec(),None)},
        Output::Standard(patch)=>{
            let mut forward=a.clone();json_patch::patch(&mut forward,&patch)?;assert_eq!(forward,*b);
            let inverse=palim::invert_json_patch(a,&patch)?;
            json_patch::patch(&mut forward,&inverse)?;assert_eq!(forward,*a);
            (serde_json::to_vec(&patch)?,Some(patch.0.len()))
        },
    };
    for _ in 0..3 { black_box(generate(&dp,a,b,mode)?); }
    let mut iterations=1;
    loop {
        let start=Instant::now();
        for _ in 0..iterations { black_box(generate(&dp,a,b,mode)?); }
        if start.elapsed().as_secs_f64()>=0.005||iterations>=2048 { break; }
        iterations*=2;
    }
    let mut samples=Vec::new();
    for _ in 0..8 {
        let start=Instant::now();
        for _ in 0..iterations { black_box(generate(&dp,a,b,mode)?); }
        samples.push(start.elapsed().as_secs_f64()*1000.0/iterations as f64);
    }
    let mut sorted=samples.clone();sorted.sort_by(f64::total_cmp);
    println!("{}",json!({"case":fixture["name"],"mode":mode,"median_ms":(sorted[3]+sorted[4])/2.0,
        "samples_ms":samples,"iterations":iterations,"wire_bytes":wire.len(),"operations":operations,
        "output":serde_json::from_slice::<Value>(&wire)?,"roundtrip_verified":true}));
    Ok(())
}
'''

def run(args, cwd=None, timeout=600):
    r = subprocess.run(args, cwd=cwd, text=True, capture_output=True, timeout=timeout)
    if r.returncode:
        raise RuntimeError(f"{args}\n{r.stdout[-4000:]}\n{r.stderr[-4000:]}")
    return r

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

fixtures = {}
frozen = baseline / "results/standard-benchmark-pipeline-20260930T214341.446185Z/fixtures"
for name in ["small-config-edit", "disjoint-2000", "rotate-2000", "ambiguous-ids-2000"]:
    fixtures[name] = json.loads((frozen / f"{name}.json").read_text())
for n in [20000, 40000]:
    a = [i % 8 for i in range(n)]
    fixtures[f"periodic-{n}"] = {"left": a, "right": a[1:] + a[:1]}
large = 'A🦀\\"' * 32768
fixtures["large-unchanged-sibling"] = {"left": {"keep": large, "edit": {"x": 1}}, "right": {"keep": large, "edit": {"x": 99}}}
fixtures["root-long-text"] = {"left": large, "right": large + "!"}
fixtures["custom-empty-8000"] = {"left": [], "right": list(range(8000))}
fixtures["custom-narrow-8000"] = {"left": [-1], "right": list(range(8000))}
modes = {name: "optimized" for name in fixtures}
modes.update({"rotate-2000": "plain", "periodic-20000": "native", "periodic-40000": "native", "custom-empty-8000": "custom-native", "custom-narrow-8000": "custom-native"})
fixtures["disjoint-plain-2000"] = fixtures["disjoint-2000"].copy()
modes["disjoint-plain-2000"] = "plain"

records = []
environment = {
    "recorded_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "platform": platform.platform(), "rustc": run(["rustc", "-Vv"]).stdout,
    "method": "Loaded inputs; generation and output destruction timed; parsing, serialization and roundtrip checks excluded. Three warmups, >=5ms calibration (cap2048), 8 samples, 3 independent processes/version; version order alternates.",
    "versions": {}, "cases": modes, "builds": {},
    "runner_sha256": sha(Path(__file__)),
}
with tempfile.TemporaryDirectory(prefix="palim-fix-bench-") as temporary:
    scratch = Path(temporary)
    binaries = {}
    for label, source in [("baseline", baseline), ("fixed", candidate)]:
        project = scratch / label
        (project / "src").mkdir(parents=True)
        (project / "src/main.rs").write_text(RUST)
        (project / "Cargo.toml").write_text('[package]\nname="palim-performance-fixes-probe"\nversion="0.0.0"\nedition="2024"\n[dependencies]\npalim={path=' + json.dumps(str(source)) + '}\nserde_json={version="1.0",features=["float_roundtrip","arbitrary_precision"]}\njson-patch="4.2.0"\n')
        (project / "Cargo.lock").write_bytes((source / "Cargo.lock").read_bytes())
        built = run(["cargo", "build", "--release", "--offline"], cwd=project)
        environment["builds"][label] = {"command": "cargo build --release --offline", "exit_code": 0, "stderr": built.stderr, "lock_sha256": sha(project / "Cargo.lock")}
        binaries[label] = project / "target/release/palim-performance-fixes-probe"
        environment["versions"][label] = {"commit":run(["git","rev-parse","HEAD"],cwd=source).stdout.strip(),"source_sha256":{str(p.relative_to(source)):sha(p) for p in sorted((source/"src").glob("*.rs"))},"manifest_sha256":sha(source/"Cargo.toml"),"lock_sha256":sha(source/"Cargo.lock"),"binary_sha256":sha(binaries[label])}
        print(f"built {label} in its own target", flush=True)
    paths = {}
    for name, fixture in fixtures.items():
        fixture["name"] = name
        paths[name] = scratch / f"{name}.json"
        paths[name].write_text(json.dumps(fixture,ensure_ascii=False))
    environment["fixture_sha256"] = {name: sha(path) for name, path in paths.items()}
    environment["generated_fixtures"] = {name:fixtures[name] for name in fixtures if name not in ["small-config-edit","disjoint-2000","rotate-2000","ambiguous-ids-2000"]}
    for repeat in range(3):
        for i, (name, mode) in enumerate(modes.items()):
            labels = ["baseline", "fixed"] if (repeat+i)%2 == 0 else ["fixed", "baseline"]
            for label in labels:
                measured = run([str(binaries[label]), mode, str(paths[name])], timeout=300)
                row = json.loads(measured.stdout)
                row["output_sha256"] = hashlib.sha256(json.dumps(row.pop("output"),sort_keys=True,separators=(",",":"),ensure_ascii=False).encode()).hexdigest()
                row.update(version=label, repeat=repeat+1)
                assert row["roundtrip_verified"]
                records.append(row)
        print(f"completed process repeat {repeat+1}/3", flush=True)

comparison = []
for name, mode in modes.items():
    row = {"case":name,"mode":mode}
    for label in ["baseline","fixed"]:
        selected=[r for r in records if r["case"]==name and r["version"]==label]
        row[label] = {"median_ms":statistics.median(r["median_ms"] for r in selected),"process_medians_ms":[r["median_ms"] for r in selected],"bytes":selected[0]["wire_bytes"],"operations":selected[0]["operations"]}
        assert all(r["output_sha256"]==selected[0]["output_sha256"] for r in selected)
    old = next(r["output_sha256"] for r in records if r["case"]==name and r["version"]=="baseline")
    new = next(r["output_sha256"] for r in records if r["case"]==name and r["version"]=="fixed")
    row["identical_output"] = old == new
    row["speedup"] = row["baseline"]["median_ms"]/row["fixed"]["median_ms"]
    comparison.append(row)
(out/"environment.json").write_text(json.dumps(environment,ensure_ascii=False,indent=2))
(out/"summary.json").write_text(json.dumps({"comparison":comparison,"records":records},ensure_ascii=False,indent=2))
print(json.dumps(comparison,ensure_ascii=False,indent=2))
