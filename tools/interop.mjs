import fs from 'node:fs';
import { spawnSync } from 'node:child_process';
import { isDeepStrictEqual } from 'node:util';
import { create } from 'jsondiffpatch/with-text-diffs';
import fast from 'fast-json-patch';
const root = new URL('../', import.meta.url);
const path = s => new URL(s, root).pathname;
const dp = create({ objectHash: v => v?.id === undefined ? undefined : JSON.stringify(v.id), textDiff: { minLength: 1 } });
const clone = structuredClone;
let seed = 20260930;
const rand = n => { seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0; return seed % n; };
const cases = JSON.parse(fs.readFileSync(path('tests/fixtures/correctness.json')));
for (let i=0;i<600;i++) {
  const a = Array.from({length:rand(50)}, (_,id)=>({id: id%13,v:rand(100), nested:{flag:rand(2)===1}}));
  const b = clone(a).filter(()=>rand(5)!==0);
  for (let j=b.length-1;j>0;j--) { const k=rand(j+1); [b[j],b[k]]=[b[k],b[j]]; }
  for (let j=0;j<rand(6);j++) b.splice(rand(b.length+1),0,{id:rand(13),v:rand(100),nested:{flag:true}});
  for (const v of b) if (rand(4)===0) v.v++;
  cases.push({name:`keyed-mixed-${i}`,left:{items:a},right:{items:b}});
}
for (let i=0;i<100;i++) {
  const words = ['ASCII text /%+\n','中文字符与空格 ','😀🐱🦀👩🏽‍💻 ', 'éΚαλημέρα ','\u0000\r\n'];
  const a = words[i%words.length].repeat(30);
  const b = Array.from(a);
  for (let j=0;j<8;j++) b.splice(rand(b.length+1),rand(4),...Array.from(words[rand(words.length)]));
  cases.push({name:`text-${i}`,left:a,right:b.join('')});
}
fs.mkdirSync(path('results'),{recursive:true});
fs.writeFileSync(path('results/interop-inputs.json'),JSON.stringify(cases));
const runner = path('target/release/examples/fixture_runner');
function rust(mode,file) {
  const result = spawnSync(runner,[mode,path(file)],{encoding:'utf8',maxBuffer:50*1024*1024,timeout:60000});
  if (result.error) throw result.error;
  if (!result.stdout) throw new Error(result.stderr);
  return {status:result.status, results:JSON.parse(result.stdout)};
}
const generated = rust('generate','results/interop-inputs.json');
fs.writeFileSync(path('results/rust-deltas.json'),JSON.stringify(generated.results));
const results = []; const incoming = [];
for (let i=0;i<cases.length;i++) {
  const c=cases[i], r=generated.results[i]; const result={name:c.name};
  try {
    if (!r.ok) throw new Error(r.error);
    const actual = r.delta===null ? clone(c.left) : dp.patch(clone(c.left),clone(r.delta));
    const original = r.inverse===null ? clone(c.right) : dp.patch(clone(c.right),clone(r.inverse));
    const rfc = fast.applyPatch(clone(c.left),r.json_patch,true,true).newDocument;
    result.rust_delta_js_forward=isDeepStrictEqual(actual,c.right);
    result.rust_inverse_js_backward=isDeepStrictEqual(original,c.left);
    result.rust_rfc_js_forward=isDeepStrictEqual(rfc,c.right);
  } catch(e) { result.rust_delta_error=e.message; }
  try {
    const delta=dp.diff(c.left,c.right)??null;
    const inverse=delta===null?null:dp.reverse(delta);
    incoming.push({...c,delta,inverse});
    const native=delta===null?clone(c.left):dp.patch(clone(c.left),clone(delta));
    const nativeBack=inverse===null?clone(c.right):dp.patch(clone(c.right),clone(inverse));
    result.js_native_forward=isDeepStrictEqual(native,c.right);
    result.js_native_backward=isDeepStrictEqual(nativeBack,c.left);
  } catch(e) { result.js_generation_error=e.message; }
  results.push(result);
}
fs.writeFileSync(path('results/js-deltas.json'),JSON.stringify(incoming));
const applied=rust('apply','results/js-deltas.json');
fs.writeFileSync(path('results/js-deltas-rust-check.json'),JSON.stringify(applied.results));
const byName=new Map(applied.results.map(r=>[r.name,r]));
for (const r of results) {
  const incoming=byName.get(r.name);
  r.js_delta_rust_roundtrip=incoming?.ok??false;
  if (!incoming?.ok) r.js_delta_rust_error=incoming?.error??'no delta';
}
fs.writeFileSync(path('results/interop.json'),JSON.stringify(results,null,2));
const fields=['rust_delta_js_forward','rust_inverse_js_backward','rust_rfc_js_forward','js_native_forward','js_delta_rust_roundtrip'];
const inverseCases=results.filter(r=>r.js_native_backward).map(r=>byName.get(r.name));
const summary={cases:cases.length, counts:Object.fromEntries([...fields,'js_native_backward'].map(f=>[f,results.filter(r=>r[f]===true).length])),
  upstream_inverse_compatibility: {
    js_self_successes: inverseCases.length,
    malformed: inverseCases.filter(r=>!r.upstream_inverse_valid).map(r=>({name:r.name,error:r.upstream_inverse_error})),
    strict_successes: inverseCases.filter(r=>r.upstream_inverse_ok).length,
    zero_error_fuzzy_successes: inverseCases.filter(r=>r.upstream_inverse_zero_error_fuzzy_ok).length,
    default_fuzzy_successes: inverseCases.filter(r=>r.upstream_inverse_fuzzy_ok).length,
    valid_fuzzy_failures: inverseCases.filter(r=>r.upstream_inverse_valid&&!r.upstream_inverse_fuzzy_ok).map(r=>({name:r.name,error:r.upstream_inverse_fuzzy_error??'output differs from baseline'})),
  },
  upstream_inverse_failures:results.filter(r=>r.js_native_backward!==true).map(r=>r.name),
  failures:results.filter(r=>fields.some(f=>r[f]!==true))};
fs.writeFileSync(path('results/interop-summary.json'),JSON.stringify(summary,null,2));
console.log(JSON.stringify({...summary,upstream_inverse_failures:summary.upstream_inverse_failures.length,failures:summary.failures.slice(0,8)},null,2));
if(summary.failures.length) process.exitCode=1;
