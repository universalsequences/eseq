#!/usr/bin/env python3
"""Check the 808-style retrigger mix against independent isolated-hit renders."""
import os,json,sys,subprocess,platform,hashlib
from pathlib import Path
import numpy as np
import soundfile as sf
ROOT=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(ROOT/'tools/audition'))
os.environ.setdefault('AUDITION_CACHE',str(ROOT/'.local/kick-retrigger-cache'))
from audition import Instrument
SR=48000

def instrument(path,block=128):
    target={('Darwin','arm64'):'DGenLisp-macos-arm64',('Linux','x86_64'):'DGenLisp-linux-x86_64'}[(platform.system(),platform.machine())]
    inst=Instrument(path,compiler=str(ROOT/'crates/sequencer/tools'/target),toolchain_root=str(ROOT/'crates/sequencer/tools/dgen-toolchain'),sample_rate=SR,max_frames=block)
    subprocess.run([sys.executable,str(ROOT/'tools/audition/check_fusion.py'),str(Path(inst.build_dir)/'patch.c')],check=True,capture_output=True)
    return inst

def render(inst,**kw):
    y,_=inst.render(**kw);assert np.isfinite(y).all();return y[:,0]

def expected_mix(single,triggers):
    n=len(single);out=np.zeros(n);ages=[n,n];weights=np.array([0.,0.]);slot=0
    coefficient=np.exp(-np.log(1000)/(SR*.005))
    events=set(triggers)
    boundary=[]
    for i in range(n):
        ongoing=sum(weights[j]*(single[ages[j]] if ages[j]<n else 0) for j in (0,1))
        if i in events:
            slot=1-slot;ages[slot]=0
            if i==0:weights[slot]=1
        out[i]=sum(weights[j]*(single[ages[j]] if ages[j]<n else 0) for j in (0,1))
        if i in events and i:boundary.append(abs(out[i]-ongoing))
        target=np.zeros(2);target[slot]=1;weights=target+coefficient*(weights-target)
        ages=[age+1 for age in ages]
    return out,max(boundary,default=0)

def main():
    report={}
    for folder in ['pm-doom-kick','pm-break-kick']:
        root=ROOT/'tools'/folder;inst=instrument(root/'model.lisp')
        presets=json.loads((root/'model.presets').read_text())['presets'];results=[];reel=[]
        for preset in presets:
            params=preset['params'].copy()
            if 'air' in params:params['air']=0
            pitch=440*2**(preset.get('base_note_offset',0)/12)
            single=render(inst,seconds=.6,pitch=pitch,params=params)
            for interval in [.005,.011,.03125,.0625]:
                triggers=[0]+[int(t*SR) for t in np.arange(.101,.5,interval)]
                y=render(inst,seconds=.6,pitch=pitch,params=params,retrig=[n/SR for n in triggers[1:]])
                # Integer event conversion matches the harness's truncation.
                triggers=[0]+[int((n/SR)*SR) for n in triggers[1:]]
                expected,boundary=expected_mix(single,triggers)
                error=float(max(abs(y-expected)))
                assert error<2e-4,(folder,preset['name'],interval,error)
                assert boundary<.003*max(abs(single)),(folder,interval,boundary)
                # The old reset behavior has jumps equal to the outgoing hit.
                if interval==.03125:
                    direct=np.zeros(len(single));last=0
                    for i in range(len(direct)):
                        if i in triggers:last=i
                        direct[i]=single[i-last]
                    reel.extend([direct,np.zeros(SR//4),y,np.zeros(SR//2)])
                results.append({'preset':preset['name'],'interval_ms':interval*1000,'equation_max_abs':error,'trigger_discontinuity':boundary})
            held=render(inst,seconds=.6,pitch=pitch,params=params,ramps={'trigger':[(0,1)]})
            assert max(abs(held-single))<2e-5,'Held trigger must not repeatedly restart the hit'
        a=instrument(root/'model.lisp',32);b=instrument(root/'model.lisp',256)
        params={'air':0} if folder=='pm-break-kick' else {}
        ramps={'pitch':[(0,220),(.1,880),(.3,110)],'velocity':[(0,.8),(.1,.3),(.3,1)]}
        # Harness ramps are block-rate: compare exact partitioning without ramps.
        x=render(a,seconds=.6,params=params,retrig=[.101,.112,.123,.15425]);y=render(b,seconds=.6,params=params,retrig=[.101,.112,.123,.15425]);assert max(abs(x-y))<2e-5
        # Stress changes of pitch/velocity while the outgoing hit is audible.
        render(inst,seconds=.6,params=params,ramps=ramps,retrig=[.101,.112,.123,.15425])
        out=ROOT/'.local'/folder;sf.write(out/'fast-retrigger-before-after.wav',np.concatenate(reel),SR,subtype='FLOAT')
        report[folder]={'source_sha256':hashlib.sha256((root/'model.lisp').read_bytes()).hexdigest(),'cases':results}
    (Path(__file__).parent/'kick-retrigger-validation.json').write_text(json.dumps(report,indent=2)+'\n')
    print({k:{'cases':len(v['cases']),'max_error':max(x['equation_max_abs'] for x in v['cases'])} for k,v in report.items()})
if __name__=='__main__':main()
