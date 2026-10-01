import fs from 'node:fs';
import { performance } from 'node:perf_hooks';
import { isDeepStrictEqual } from 'node:util';
import { create } from 'jsondiffpatch/with-text-diffs';
const c = JSON.parse(fs.readFileSync(process.argv[2],'utf8'));
const dp = create({objectHash:v=>v?.id===undefined?undefined:JSON.stringify(v.id)});
const diff = (a,b)=>dp.diff(a,b)??null;
const delta=diff(c.left,c.right);
const actual=delta===null?structuredClone(c.left):dp.patch(structuredClone(c.left),structuredClone(delta));
const inv=delta===null?null:dp.reverse(delta);
const original=inv===null?structuredClone(c.right):dp.patch(structuredClone(c.right),structuredClone(inv));
if(!isDeepStrictEqual(actual,c.right)) throw new Error('forward mismatch');
const a=JSON.stringify(c.left),b=JSON.stringify(c.right);
for(let i=0;i<20;i++)diff(c.left,c.right);
const start=performance.now(); diff(c.left,c.right);
const iterations=Math.max(1,Math.min(200,Math.floor(25/Math.max(performance.now()-start,.001))));
const d=[],p=[],apply=[],rev=[];
let consumed=0;
for(let sample=0;sample<5;sample++) {
  let start=performance.now();
  for(let i=0;i<iterations;i++) { const value=diff(c.left,c.right); consumed+=value===null?0:1; }
  d.push((performance.now()-start)/iterations);
  start=performance.now();
  for(let i=0;i<iterations;i++) consumed+=JSON.stringify(diff(JSON.parse(a),JSON.parse(b))).length;
  p.push((performance.now()-start)/iterations);
  start=performance.now();
  for(let i=0;i<iterations;i++) { const value=structuredClone(c.left); if(delta!==null) dp.patch(value,structuredClone(delta)); }
  apply.push((performance.now()-start)/iterations);
  start=performance.now();
  for(let i=0;i<iterations;i++) if(delta!==null) dp.reverse(delta);
  rev.push((performance.now()-start)/iterations);
}
for(const s of [d,p,apply,rev])s.sort((a,b)=>a-b);
console.log(JSON.stringify([{name:c.name,engine:'jsondiffpatch-js-0.7.6',ok:true,inverse_ok:isDeepStrictEqual(original,c.left),
  diff_ms:d[2],pipeline_ms:p[2],patch_ms:apply[2],reverse_ms:rev[2],diff_samples_ms:d,pipeline_samples_ms:p,
  patch_bytes:Buffer.byteLength(JSON.stringify(delta)),iterations,consumed}]));
