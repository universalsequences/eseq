#!/usr/bin/env python3
"""Validate compiled DSP, controls, retriggers, and the recorded fit error."""
import os
os.environ.setdefault('MPLCONFIGDIR','/tmp/pm-doom-mpl')
import sys,json,hashlib,subprocess,platform,re
from pathlib import Path
import numpy as np
import soundfile as sf
import matplotlib.pyplot as plt
ROOT=Path(__file__).resolve().parents[2];HERE=Path(__file__).parent;OUT=ROOT/'.local/pm-break-kick'
sys.path.insert(0,str(ROOT/'tools/audition'));os.environ.setdefault('AUDITION_CACHE',str(ROOT/'.local/pm-break-kick-cache'))
from audition import Instrument
from model_fit import render,read_sample,HASH,old_metrics
from build import PARAMS

def instrument(sr,block=128):
    target={('Darwin','arm64'):'DGenLisp-macos-arm64',('Linux','x86_64'):'DGenLisp-linux-x86_64'}[(platform.system(),platform.machine())]
    inst=Instrument(HERE/'model.lisp',compiler=str(ROOT/'crates/sequencer/tools'/target),toolchain_root=str(ROOT/'crates/sequencer/tools/dgen-toolchain'),sample_rate=sr,max_frames=block)
    subprocess.run([sys.executable,str(ROOT/'tools/audition/check_fusion.py'),str(Path(inst.build_dir)/'patch.c')],capture_output=True,check=True)
    # The compiler stores the RNG uint32 bits in the float memory arena.
    # Those cells are integers, so interpreting their bit patterns as floats
    # can produce NaN without any non-finite DSP state. Discover typed cells
    # from the generated code rather than assuming a fixed memory offset.
    source=(Path(inst.build_dir)/'patch.c').read_text()
    inst.integer_slots={int(i) for i in re.findall(r"memcpy\(&\w+, &memory\[(\d+)\], sizeof\(uint32_t\)\)",source)}
    return inst

def audio(inst,**kw):
    kw.setdefault("pitch",220)
    y,state=inst.render(**kw)
    numeric=np.array([v for i,v in enumerate(state) if i not in inst.integer_slots])
    assert np.isfinite(y).all() and np.isfinite(numeric).all()
    assert np.array_equal(y[:,0],y[:,1]);return y[:,0]

def main():
    fit=json.loads((HERE/'fit-results.json').read_text());report={'source_sha256':hashlib.sha256((HERE/'model.lisp').read_bytes()).hexdigest(),'rates':{}}
    for sr in [16000,44100,48000,96000]:
        inst=instrument(sr);y=audio(inst,seconds=.8,params={'air':0})
        r=render(fit['modes'],len(y),sr,fit['ceiling']);t=np.arange(len(y))/sr;u=np.clip((t-.68)/.02,0,1);r*=1-u*u*(3-2*u)
        err=float(max(abs(y-r)));assert err<.001,err
        report['rates'][sr]={'equation_max_abs':err,'peak':float(max(abs(y)))}
    report['compiler_sha256']=inst.compiler_sha256
    report['uint32_memory_slots']=sorted(inst.integer_slots)
    inst=instrument(16000);y=audio(inst,seconds=.8);raw,*_=read_sample(HASH);ref=np.pad(raw,(0,len(y)-len(raw)))
    report['reference_metrics']=old_metrics(y,ref,16000)
    sf.write(OUT/'reference-then-model.wav',np.concatenate([ref,np.zeros(4000),y,np.zeros(8000)]),16000,subtype='FLOAT')
    sf.write(OUT/'compiled-model.wav',y,16000,subtype='FLOAT')
    fig,axes=plt.subplots(3,1,figsize=(12,10),layout='constrained');t=np.arange(len(y))/16000
    for a in axes[:2]:a.plot(t,ref,label='Reference',lw=.7);a.plot(t,y,label='Compiled model',alpha=.8,lw=.7);a.legend()
    axes[0].set_xlim(0,.75);axes[1].set_xlim(0,.09)
    rms=lambda x:np.sqrt(np.mean(x.reshape(-1,80)**2,axis=1))
    axes[2].plot(t[::80],rms(ref),label='Reference 5 ms RMS');axes[2].plot(t[::80],rms(y),label='Compiled 5 ms RMS');axes[2].legend()
    fig.savefig(OUT/'comparison.png',dpi=140)
    a=instrument(48000,32);b=instrument(48000,256)
    x=audio(a,seconds=.9,params={'air':0},retrig=[.137,.321]);y=audio(b,seconds=.9,params={'air':0},retrig=[.137,.321]);delta=float(max(abs(x-y)));assert delta<2e-5;report['partition_max_abs']=delta
    assert max(abs(audio(a,seconds=.8,vel=0)))==0
    assert max(abs(audio(a,seconds=.8,ramps={'trigger':[(0,0)],'gate':[(0,0)]})))==0
    for name,d,lo,hi in PARAMS:
        base={'air':0} if not name.startswith('air') else {'air':3}
        x=audio(a,seconds=.8,params=base|{name:lo});y=audio(a,seconds=.8,params=base|{name:hi})
        assert max(abs(x-y))>1e-5,name
    for ix in [2,3]:
        params={p[0]:p[ix] for p in PARAMS}
        for pitch in [110,440,1760]:audio(a,seconds=2.1,pitch=pitch,params=params,retrig=[.123,.517])
    report['controls_checked']=len(PARAMS)
    (HERE/'validation.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
if __name__=='__main__':main()
