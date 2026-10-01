import fs from 'node:fs';
import { create } from '/Users/apple/Desktop/code/AI/tool/secrets/jsondiffpatch-rs/tools/node_modules/jsondiffpatch/lib/with-text-diffs.js';
let seed = 20261001;
const rand = n => { seed=(Math.imul(seed,1664525)+1013904223)>>>0; return seed%n; };
const words = ['😀🐱🦀中', 'éβ𝄞', '%/+?#', '\0\t', 'ASCII repeated text'];
const inputPath='/tmp/text-review-input.json';
if (process.argv[2]==='generate') {
  const counts=[2047,2048,4095,4096,4097,5000,8192,12000,1024,0];
  const cases=[];
  for (let c=0;c<100;c++) {
    const ending=c%2?'\r\n':'\n';
    const n=counts[c%counts.length];
    let lines=Array.from({length:n},(_,i)=> i%9===0?ending:`${i%3===0&&c%3===0?'repeat':i}:${words[rand(words.length)]}${rand(500)}${ending}`);
    if (c%10===9) lines=[`${words[c%words.length].repeat(3000)}${ending}`];
    const left=`OLD🐱${ending}${lines.join('')}TAIL🦀${c%4===0?'':ending}`;
    for (let e=0;e<8;e++) {
      if (e%2===0) {
        const index=rand(lines.length+1);
        lines.splice(index,rand(4),...Array.from({length:rand(4)},()=>`${words[rand(words.length)]}${rand(100)}${ending}${rand(2)?ending:''}`));
      } else if(lines.length) {
        const index=rand(lines.length), chars=Array.from(lines[index]);
        chars.splice(rand(chars.length+1),rand(4),...Array.from(words[rand(words.length)]));
        lines[index]=chars.join('');
      }
    }
    const right=`NEW🚀${ending}${c%3===0?ending:''}${lines.join('')}${ending}FINAL😀${c%5===0?'':ending}`;
    cases.push({name:`long-boundary-${c}`,left,right});
  }
  fs.writeFileSync(inputPath,JSON.stringify(cases));
  console.log(`generated ${cases.length} fixed-seed long mixed edit cases`);
} else {
  const cases=JSON.parse(fs.readFileSync(inputPath));
  const results=JSON.parse(fs.readFileSync('/tmp/text-review-output.json'));
  const dp=create({textDiff:{minLength:1}});
  let hunks=0;
  for(let i=0;i<cases.length;i++) {
    const c=cases[i], r=results[i];
    if(dp.patch(c.left, structuredClone(r.delta))!==c.right) throw Error(`JS forward ${c.name}`);
    if(dp.patch(c.right, structuredClone(r.inverse))!==c.left) throw Error(`JS reverse ${c.name}`);
    const wire=r.delta[0];
    let previousEnd=0;
    for(const match of wire.matchAll(/^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@$/gm)) {
      const length=match[4]===undefined?1:Number(match[4]);
      const start=Number(match[3])-(length===0?0:1);
      if(start<previousEnd) throw Error(`crossed generated target hunks ${c.name}`);
      previousEnd=start+length; hunks++;
    }
  }
  console.log(`JS 0.7.6 exact forward + Rust inverse: ${cases.length}/${cases.length} passed; target hunk intervals noncrossing (${hunks} hunks)`);
}
