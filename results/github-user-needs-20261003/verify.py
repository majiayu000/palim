#!/usr/bin/env python3
"""Replay GitHub user scenarios without modifying Palim's production source.
Usage: python3 verify.py PALIM_REPO OUTPUT_DIRECTORY JS_TOOLS_DIRECTORY
"""
import hashlib
import json
from pathlib import Path
import platform
import subprocess
import sys
import tempfile

RUST = r"""
use palim::{CompareOptions,Delta,DiffOptions,DiffPatcher,JsonPatchOptions,Patch,TextPatchOptions};
use serde_json::{Value,json};
use std::{error::Error,fs,sync::{Arc,Mutex},time::Instant};
type Result<T> = std::result::Result<T,Box<dyn Error>>;
fn options(c:&Value)->DiffOptions {
    let mut opts=DiffOptions::default();
    if let Some(key)=c["identity"].as_str() { let key=key.to_owned(); opts.object_hash=Some(Arc::new(move |v,_|v.get(&key).map(Value::to_string))); }
    if let Some(path)=c["exclude"].as_str() { let path=path.to_owned(); opts.node_filter=Some(Arc::new(move |p,_,_|p!=path)); }
    if let Some(name)=c["property_exclude"].as_str() { let name=name.to_owned(); opts.property_filter=Some(Arc::new(move |p,_,_,_|p!=name)); }
    if c["matcher"]=="strip_digits" {
        opts.array_item_matcher=Some(Arc::new(|_,a,b|match(a.as_str(),b.as_str()) {
            (Some(a),Some(b))=>a.trim_end_matches(|ch:char|ch.is_ascii_digit())==b.trim_end_matches(|ch:char|ch.is_ascii_digit()),_=>false
        }));
    }
    opts
}
fn incoming(c:&Value,a:&Value,b:&Value)->Value {
    if c.get("incoming_delta").is_none() && c.get("delta").is_none() { return Value::Null; }
    let raw=c.get("incoming_delta").unwrap_or(&c["delta"]).clone();
    if raw.is_null() {return json!({"forward":a==b,"undo":a==b});}
    match Delta::from_value(raw) {
        Ok(d)=>{
            let f=palim::patch(a,&d);
            let back=palim::reverse(&d).and_then(|r|palim::patch(b,&r));
            let mut v=json!({"forward":f.as_ref().is_ok_and(|x|x==b),"undo":back.as_ref().is_ok_and(|x|x==a)});
            if let Err(e)=f {v["forward_error"]=json!(e.to_string());}
            if let Err(e)=back {v["undo_error"]=json!(e.to_string());}
            if let Some(inverse)=c.get("incoming_inverse") {
                if !inverse.is_null() {
                    match Delta::from_value(inverse.clone()) {
                        Ok(d)=>{
                            let exact=palim::patch(b,&d);let fuzzy=palim::patch_fuzzy(b,&d,&TextPatchOptions::default());
                            v["js_inverse_exact"]=json!(exact.as_ref().is_ok_and(|x|x==a));
                            v["js_inverse_fuzzy"]=json!(fuzzy.as_ref().is_ok_and(|x|x==a));
                            if let Err(e)=fuzzy {v["js_inverse_error"]=json!(e.to_string());}
                        },Err(e)=>v["js_inverse_error"]=json!(e.to_string())
                    }
                }
            }
            v
        },Err(e)=>json!({"forward":false,"undo":false,"error":e.to_string()})
    }
}
fn run(c:&Value)->Result<Value> {
    let(a,b)=(&c["left"],&c["right"]);
    let dp=DiffPatcher::new(options(c));
    match c["kind"].as_str().ok_or("missing kind")? {
        "native"=>{
            let expected=c.get("expected").unwrap_or(b);
            let start=Instant::now();let d=dp.diff(a,b)?;let elapsed=start.elapsed().as_secs_f64()*1000.;
            let actual=d.as_ref().map_or_else(||Ok(a.clone()),|d|dp.patch(a,d))?;
            let restored=d.as_ref().map_or_else(||Ok(actual.clone()),|d|dp.unpatch(&actual,d))?;
            let before=serde_json::to_value(&d)?;
            let plain=dp.diff_json_patch(a,b,&JsonPatchOptions{factorize:false,rationalize:false,tests:false})?;
            let optimized=dp.diff_json_patch(a,b,&JsonPatchOptions::default())?;
            let mut independent=a.clone();json_patch::patch(&mut independent,&plain)?;
            let inverse=palim::invert_json_patch(a,&plain)?;json_patch::patch(&mut independent,&inverse)?;
            let out=palim::apply_json_patch(a,&optimized)?;
            let mut row=json!({"native_forward":actual==*expected,"native_undo":restored==*a,"plain_forward":palim::apply_json_patch(a,&plain)?==*expected,"plain_inverse_independent":independent==*a,"optimized_forward":out==*expected,"plain_operations":plain.0.len(),"optimized_operations":optimized.0.len(),"native_bytes":serde_json::to_vec(&d)?.len(),"first_diff_ms":elapsed,"actual":actual,"incoming":incoming(c,a,b)});
            if serde_json::to_vec(&d)?.len()<20_000 {row["delta"]=before;row["plain_patch"]=serde_json::to_value(plain)?;}
            if c["measure"]==true {
                let mut samples=Vec::new();for _ in 0..3 {let start=Instant::now();std::hint::black_box(dp.diff(a,b)?);samples.push(start.elapsed().as_secs_f64()*1000.);}
                row["loaded_diff_samples_ms"]=json!(samples);
                row.as_object_mut().ok_or("result is not object")?.remove("actual");
            }
            Ok(row)
        },
        "standard"=>{
            let p:Patch=serde_json::from_str(&serde_json::to_string(&c["patch"])?)?;
            match palim::apply_json_patch(a,&p) {
                Ok(actual)=>{
                    let inv=palim::invert_json_patch(a,&p)?;
                    Ok(json!({"forward":actual==*b,"undo":palim::apply_json_patch(&actual,&inv)?==*a,"actual":actual,"inverse":inv,"non_mut_input":true,"standard_to_native_roundtrip":match dp.diff(a,&actual)?{Some(d)=>dp.patch(a,&d)?==actual,None=>a==&actual}}))
                },Err(e)=>Ok(json!({"error":e.to_string(),"rejected":true}))
            }
        },
        "sequence"=>{
            let middle=&c["middle"];
            let p1=dp.diff(a,middle)?.ok_or("missing p1")?;let p2=dp.diff(middle,b)?.ok_or("missing p2")?;
            let frozen1=p1.clone();let frozen2=p2.clone();
            let intermediate=dp.patch(a,&p1)?;let result=dp.patch(&intermediate,&p2)?;
            let restored=dp.unpatch(&dp.unpatch(&result,&p2)?,&p1)?;
            Ok(json!({"forward":result==*b,"undo":restored==*a,"delta1_unchanged":p1==frozen1,"delta2_unchanged":p2==frozen2,"source_unchanged":a==&c["left"]}))
        },
        "drift"=>{
            let d=dp.diff(a,b)?.ok_or("missing delta")?;
            let drift=&c["drift"];
            let wrong=dp.unpatch(drift,&d);
            let standard=dp.diff_json_patch(a,b,&JsonPatchOptions{tests:true,..Default::default()})?;
            let inverse=palim::invert_json_patch(a,&standard)?;
            let guarded=palim::apply_json_patch(drift,&inverse);
            Ok(json!({"reordered_undo":match wrong {Ok(v)=>json!({"accepted":true,"value":v}),Err(e)=>json!({"accepted":false,"error":e.to_string()})},"guarded_reordered_undo":match guarded {Ok(v)=>json!({"accepted":true,"value":v}),Err(e)=>json!({"accepted":false,"error":e.to_string()})},"original_baseline_undo":dp.unpatch(b,&d)?==*a}))
        },
        "compare"=>{
            let mut opts=CompareOptions::default();
            if c["unordered"]==true {opts.unordered=Some(Arc::new(|_|true));}
            if c["filter"]=="both_present" {opts.node_filter=Some(Arc::new(|_,a,b|a.is_some()&&b.is_some()));}
            if c["filter"]=="right_present" {opts.node_filter=Some(Arc::new(|_,_,b|b.is_some()));}
            if let Some(path)=c["exclude"].as_str() {let path=path.to_owned();opts.node_filter=Some(Arc::new(move |p,_,_|p!=path));}
            let calls=Arc::new(Mutex::new(Vec::new()));
            if c["track_custom"]==true {let calls=calls.clone();opts.custom_equal=Some(Arc::new(move |path,_,_|{calls.lock().expect("local callback lock").push(path.to_owned());None}));}
            let report=palim::compare(a,b,&opts)?;
            Ok(json!({"report":report,"callback_paths":*calls.lock().map_err(|_|"poisoned callback lock")?}))
        },_=>Err("unknown kind".into())
    }
}
fn main()->Result<()> {
    let input:Value=serde_json::from_slice(&fs::read(std::env::args().nth(1).ok_or("missing input")?)?)?;
    let mut rows=Vec::new();
    for c in input.as_array().ok_or("inputs must be array")? {
        let mut v=match run(c) {Ok(v)=>v,Err(e)=>json!({"probe_error":e.to_string()})};
        v["name"]=c["name"].clone();v["issue"]=c["issue"].clone();rows.push(v);
    }
    println!("{}",serde_json::to_string(&rows)?);Ok(())
}
"""

JS = r"""
import fs from 'node:fs';
import {create} from 'jsondiffpatch/with-text-diffs';
import {format} from 'jsondiffpatch/formatters/jsonpatch';
import {isDeepStrictEqual as eq} from 'node:util';
import {performance} from 'node:perf_hooks';
import fast from 'fast-json-patch';
const inputs=JSON.parse(fs.readFileSync(process.argv[2]));
const rows=[],incoming=[];
for(const c of inputs){
 const row={name:c.name};
 const dp=create({...c.identity?{objectHash:v=>v?.[c.identity]===undefined?undefined:JSON.stringify(v[c.identity])}:{},...c.js_match_by_position_false?{matchByPosition:false}:{}});
 if(c.js_match_by_position_false)row.option='matchByPosition:false';
 try{
  if(c.kind==='native'){
   const start=performance.now();const delta=dp.diff(c.left,c.right);row.first_diff_ms=performance.now()-start;
   row.has_delta=delta!==undefined;
   row.native_forward=eq(delta?dp.patch(structuredClone(c.left),structuredClone(delta)):c.left,c.right);
   try{row.native_undo=eq(delta?dp.unpatch(structuredClone(c.right),structuredClone(delta)):c.right,c.left)}catch(e){row.undo_error=e.message}
   try{const patch=format(delta,c.left);row.rfc_forward=eq(fast.applyPatch(structuredClone(c.left),patch,true,true).newDocument,c.right);row.rfc_operations=patch.length;}catch(e){row.rfc_error=e.message}
   if(c.measure){row.loaded_diff_samples_ms=[];for(let i=0;i<3;i++){const start=performance.now();dp.diff(c.left,c.right);row.loaded_diff_samples_ms.push(performance.now()-start)}}
   else{incoming.push({...c,incoming_delta:delta??null,incoming_inverse:delta?dp.reverse(delta):null});}
  }else if(c.kind==='sequence'){
   const p1=dp.diff(c.left,c.middle),p2=dp.diff(c.middle,c.right),frozen=JSON.stringify(p1);
   let result=dp.patch(structuredClone(c.left),p1);result=dp.patch(result,p2);
   row.forward=eq(result,c.right);row.delta1_unchanged=JSON.stringify(p1)===frozen;
  }
 }catch(e){row.error=e.message}
 rows.push(row);
}
console.log(JSON.stringify({rows,incoming}));
"""

def run(command, **kwargs):
    r = subprocess.run(command, text=True, capture_output=True, **kwargs)
    if r.returncode:
        raise RuntimeError(f"{command}: {r.returncode}\n{r.stderr[-3000:]}")
    return r

repo, out, js_tools = map(lambda x: Path(x).resolve(), sys.argv[1:4])
fixtures = Path(__file__).with_name('fixtures.json')
inputs = json.loads(fixtures.read_text())
def digest(path): return hashlib.sha256(path.read_bytes()).hexdigest()
result = {'platform': platform.platform(), 'palim_commit': run(['git','rev-parse','HEAD'],cwd=repo).stdout.strip(), 'source_sha256':{str(p.relative_to(repo)):digest(p) for p in sorted((repo/'src').glob('*.rs'))}, 'manifest_sha256':digest(repo/'Cargo.toml'), 'fixture_sha256':digest(fixtures), 'runner_sha256':digest(Path(__file__)), 'rustc':run(['rustc','-Vv']).stdout, 'node':run(['node','--version']).stdout.strip(), 'note':'Loaded input diagnostic timings, large-array scenario: three independent processes/engine, alternating order, three timed samples/process; no end-to-end or general ranking claim. Upstream version is frozen, and default JS lacks a custom matcher equivalent.'}
with tempfile.TemporaryDirectory(prefix='palim-issue-probe-') as scratch:
    project=Path(scratch)
    (project/'src').mkdir()
    (project/'src/main.rs').write_text(RUST)
    (project/'Cargo.toml').write_text('[package]\nname="palim-github-issue-probe"\nversion="0.0.0"\nedition="2024"\n[dependencies]\npalim={path='+json.dumps(str(repo))+'}\nserde_json={version="1",features=["arbitrary_precision","float_roundtrip"]}\njson-patch="4.2.0"\n')
    (project/'Cargo.lock').write_bytes((repo/'Cargo.lock').read_bytes())
    built=run(['cargo','build','--release','--offline'],cwd=project,timeout=180)
    result['build']={'exit_code':built.returncode,'stderr':built.stderr,'lock_sha256':digest(project/'Cargo.lock')}
    binary=project/'target/release/palim-github-issue-probe'
    result['binary_sha256']=digest(binary)
    rust=run([str(binary),str(fixtures)],timeout=60)
    result['palim']=json.loads(rust.stdout)
    small=project/'small.json';small.write_text(json.dumps([c for c in inputs if not c.get('measure') and c['kind'] in ['native','sequence']]))
    (project/'node_modules').symlink_to(js_tools/'node_modules',target_is_directory=True)
    script=project/'upstream.mjs';script.write_text(JS)
    upstream=run(['node','--max-old-space-size=1024',str(script),str(small)],timeout=60)
    data=json.loads(upstream.stdout); result['javascript']=data['rows']
    result['js_packages']={name:json.loads((js_tools/'node_modules'/name/'package.json').read_text())['version'] for name in ['jsondiffpatch','fast-json-patch']}
    imported=project/'incoming.json';imported.write_text(json.dumps(data['incoming']))
    result['palim_import_js_delta']=json.loads(run([str(binary),str(imported)],timeout=60).stdout)
    large=project/'large.json'
    large_inputs=[c for c in inputs if c.get('measure')]
    large.write_text(json.dumps(large_inputs+[dict(c,name=c['name']+'-js-workaround',js_match_by_position_false=True) for c in large_inputs]))
    try:
        r=subprocess.run(['node','--max-old-space-size=1024',str(script),str(large)],text=True,capture_output=True,timeout=30)
        result['js_large']={'exit_code':r.returncode,'stderr':r.stderr[-1500:],'memory_limit_mib':1024,'timeout_seconds':30}
        if r.returncode==0: result['javascript']+=json.loads(r.stdout)['rows']
    except subprocess.TimeoutExpired:
        result['js_large']={'timed_out':True,'memory_limit_mib':1024,'timeout_seconds':30}
    # Two further independent processes, alternating engine order. Keep the
    # original run too; all outputs are validated outside the timed diff.
    result['large_processes']=[{'process':1,'palim':[r for r in result['palim'] if 'loaded_diff_samples_ms' in r],'javascript':[r for r in result['javascript'] if 'loaded_diff_samples_ms' in r]}]
    rust_large=project/'rust-large.json';rust_large.write_text(json.dumps(large_inputs))
    for repeat in [2,3]:
        entry={'process':repeat}
        for engine in (['javascript','palim'] if repeat==2 else ['palim','javascript']):
            if engine=='palim':entry[engine]=json.loads(run([str(binary),str(rust_large)],timeout=30).stdout)
            else:entry[engine]=json.loads(run(['node','--max-old-space-size=1024',str(script),str(large)],timeout=30).stdout)['rows']
        result['large_processes'].append(entry)
(out/'verification.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
for row in result['palim']:
    if row.get('probe_error'): print(row['name'],'ERROR',row['probe_error'])
    else: print(row['name'],{k:v for k,v in row.items() if k in ['native_forward','native_undo','plain_forward','optimized_forward','forward','undo','rejected','plain_operations','callback_paths','delta1_unchanged','reordered_undo','loaded_diff_samples_ms']})
print('JS large',result['js_large'])
