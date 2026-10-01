#!/usr/bin/env python3
"""Bounded, sequential benchmark with fixed JSON inputs and per-process RSS."""
import argparse, copy, hashlib, json, os, platform, re, signal, subprocess, time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT/'results'
def dump(path,data):
    path.parent.mkdir(parents=True,exist_ok=True)
    path.write_text(json.dumps(data,ensure_ascii=True,indent=2)+'\n')
def inputs():
    cases=[]
    def add(name,a,b,skip=False): cases.append(dict(name=name,left=a,right=b,skip_js=skip))
    add('small-config-edit',{'settings':{f'key{i}':i for i in range(30)}},{'settings':{f'key{i}':99 if i==5 else i for i in range(30)}})
    for n in (2000,20000):
        a=[f'entry-{i:08d}' for i in range(n)]
        add(f'identical-{n}',a,a)
        add(f'prepend-{n}',a,['new']+a)
        add(f'middle-insert-{n}',a,a[:n//2]+['new']+a[n//2:])
        add(f'rotate-{n}',a,a[-1:]+a[:-1],n>4000)
        add(f'reverse-{n}',a,a[::-1],n>4000)
        add(f'disjoint-{n}',a,[f'other-{i:08d}' for i in range(n)],n>4000)
        b=a.copy()
        for i in range(0,n,max(1,n//20)): b[i]='modified-'+b[i]
        add(f'sparse-edit-{n}',a,b,n>4000)
    for n in (100,2000):
        a=[dict(id=i,name=f'item-{i}',meta=dict(enabled=bool(i%2),score=i)) for i in range(n)]
        b=copy.deepcopy(a[-1:]+a[:-1]);b[n//2]['meta']['score']+=10
        add(f'keyed-rotate-edit-{n}',{'items':a},{'items':b})
    snapshot=ROOT/'tools/crates-snapshot.json'
    a=json.loads(snapshot.read_text()); b=copy.deepcopy(a);b['crates']=b['crates'][-1:]+b['crates'][:-1]
    b['crates'][50]['description']='benchmark controlled metadata change'
    add('crates-metadata-rotate-edit-100',a,b)
    for name,unit in [('ascii','a line of text with stable context\n'),('unicode','你好 🦀 stable Unicode 文本\n')]:
        a=unit*5000;b=a[:len(a)//3]+'EDIT '+a[len(a)//3:]
        add(f'long-text-{name}',a,b)
    return cases
def run(cmd):
    timecmd=['/usr/bin/time','-l'] if platform.system()=='Darwin' else ['/usr/bin/time','-v']
    start=time.monotonic()
    p=subprocess.Popen(timecmd+cmd,cwd=ROOT,text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE,start_new_session=True)
    try: out,err=p.communicate(timeout=60)
    except subprocess.TimeoutExpired:
        os.killpg(p.pid,signal.SIGKILL);out,err=p.communicate()
        return {'ok':False,'error':'60 second timeout'}
    if p.returncode: return {'ok':False,'error':err[-4000:],'stdout':out[-2000:]}
    result=json.loads(out.strip().splitlines()[-1])[0]
    m=re.search(r'(\d+)\s+maximum resident set size',err) if platform.system()=='Darwin' else re.search(r'Maximum resident set size \(kbytes\):\s*(\d+)',err)
    result['rss_mib']=int(m[1])/(1024**2 if platform.system()=='Darwin' else 1024) if m else None
    result['process_seconds']=time.monotonic()-start
    return result
def main():
    cases=inputs(); results=[]; manifest={}
    for c in cases:
        file=OUT/'fixtures'/(c['name']+'.json');dump(file,c)
        manifest[c['name']]=hashlib.sha256(file.read_bytes()).hexdigest()
        commands=[('palim',[str(ROOT/'target/release/examples/fixture_runner'),'benchmark',str(file)])]
        if c['skip_js']: results.append({'name':c['name'],'engine':'jsondiffpatch-js-0.7.6','skipped':'20k LCS matrix exceeds this experiment allocation budget; not a measured failure'})
        else: commands.append(('jsondiffpatch-js-0.7.6',['node','--max-old-space-size=768','tools/bench-js.mjs',str(file)]))
        for engine,cmd in commands:
            r=run(cmd);r.setdefault('name',c['name']);r.setdefault('engine',engine);results.append(r)
            print(c['name'],engine,'ok' if r['ok'] else r.get('error'),flush=True)
        dump(OUT/'comparison.json',results)
    environment={key:subprocess.check_output(cmd,text=True).strip() for key,cmd in [('rustc',['rustc','-Vv']),('node',['node','-v']),('cpu',['sysctl','-n','machdep.cpu.brand_string'])]}
    environment.update(platform=platform.platform(),date=time.strftime('%Y-%m-%d'),fixture_sha256=manifest,
        crates_snapshot_source='https://crates.io/api/v1/crates?page=1&per_page=100 (snapshot 2026-09-30)',
        crates_snapshot_sha256=hashlib.sha256((ROOT/'tools/crates-snapshot.json').read_bytes()).hexdigest())
    dump(OUT/'benchmark-environment.json',environment)
    assert all(r.get('ok',True) for r in results),'a measured benchmark failed verification'
    print(json.dumps({'measured':sum('ok' in r for r in results),'skipped':sum('skipped' in r for r in results)}))
if __name__=='__main__':main()
