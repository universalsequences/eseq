#!/usr/bin/env python3
"""Compare compiled candidate with the model equations and the five real targets."""
import ctypes
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
os.environ.setdefault('MPLCONFIGDIR','/tmp/pm-doom-mpl')
import numpy as np
import soundfile as sf
import matplotlib.pyplot as plt
from scipy import signal
from analyze import ROOT,OUT,SR,read_sample
from fit import NAMES,render as reference,metrics

sys.path.insert(0,str(ROOT/'tools/audition'))
os.environ.setdefault('AUDITION_CACHE',str(ROOT/'.local/pm-doom-kick-cache'))
from audition import Instrument
SOURCE=ROOT/'tools/pm-doom-kick/model.lisp'


def instrument(sr=48000,block=128):
    target={('Darwin','arm64'):'DGenLisp-macos-arm64',('Linux','x86_64'):'DGenLisp-linux-x86_64'}[(platform.system(),platform.machine())]
    inst=Instrument(SOURCE,compiler=str(ROOT/'crates/sequencer/tools'/target),
        toolchain_root=str(ROOT/'crates/sequencer/tools/dgen-toolchain'),sample_rate=sr,max_frames=block)
    subprocess.run([sys.executable,str(ROOT/'tools/audition/check_fusion.py'),str(Path(inst.build_dir)/'patch.c')],check=True,capture_output=True)
    return inst


def audio(inst,**kwargs):
    kwargs.setdefault("pitch",220)
    y,m=inst.render(**kwargs)
    assert np.isfinite(y).all() and np.isfinite(m).all(),kwargs
    assert np.array_equal(y[:,0],y[:,1])
    return y[:,0]


def main():
    fits=json.loads((ROOT/'tools/pm-doom-kick/selection.json').read_text())['selected']
    result={'source_sha256':hashlib.sha256(SOURCE.read_bytes()).hexdigest(),'presets':[], 'rates':{}}
    versions={sr:instrument(sr) for sr in (16000,44100,48000,96000)}
    result['compiler_sha256']=versions[48000].compiler_sha256
    max_equation_error=0
    for sr,inst in versions.items():
        peaks=[]
        for row in fits:
            p=row['params']
            y=audio(inst,seconds=.55,pitch=220,params=p)
            ref=reference([p[n] for n in NAMES],len(y),sr)
            error=float(np.max(np.abs(y-ref)))
            max_equation_error=max(max_equation_error,error)
            assert error<.001,(sr,row['title'],error)
            assert np.max(np.abs(y[int(.5*sr):]))==0
            peaks.append(float(np.max(abs(y))))
        result['rates'][sr]={'preset_peaks':peaks}
    result['compiled_equation_max_abs']=max_equation_error
    inst=versions[16000]; reel=[]
    fig,axes=plt.subplots(5,2,figsize=(13,14),layout='constrained')
    for i,row in enumerate(fits):
        raw,*_=read_sample(row['hash'])
        target=np.pad(raw,(0,max(0,6400-len(raw))))[:6400]
        y=audio(inst,seconds=.4,pitch=220,params=row['params'])
        m=metrics(y,target,16000)
        result['presets'].append({'title':row['title'],'hash':row['hash'],'metrics':m})
        slug=Path(row['title']).stem.replace(' ','-')
        pair=np.concatenate([target,np.zeros(4000),y,np.zeros(8000)])
        sf.write(OUT/f'{slug}-compiled-AB.wav',pair,16000,subtype='FLOAT')
        reel.append(pair)
        t=np.arange(6400)/16000
        axes[i,0].plot(t,target,label='Reference',lw=.8)
        axes[i,0].plot(t,y,label='Compiled model',lw=.7,alpha=.8)
        axes[i,0].set_xlim(0,.28);axes[i,0].set_title(row['title']);axes[i,0].legend(fontsize=8)
        hop=80
        env=lambda a:np.sqrt(np.mean(a.reshape(-1,hop)**2,axis=1))
        axes[i,1].plot(t[::hop],env(target),label='Reference')
        axes[i,1].plot(t[::hop],env(y),label='Compiled model')
        axes[i,1].set_xlim(0,.28)
        axes[i,1].set_title(f"Envelope error {m['envelope_nrmse']:.1%}; waveform correlation {m['correlation']:.4f}")
    fig.savefig(OUT/'compiled-comparison.png',dpi=140)
    sf.write(OUT/'five-kicks-reference-then-model.wav',np.concatenate(reel),16000,subtype='FLOAT')
    a=instrument(48000,32);b=instrument(48000,256)
    delta=0
    for row in fits:
        x=audio(a,seconds=.8,pitch=220,params=row['params'],retrig=[.137,.321])
        y=audio(b,seconds=.8,pitch=220,params=row['params'],retrig=[.137,.321])
        delta=max(delta,float(np.max(abs(x-y))))
    assert delta<2e-5,delta
    result['block_partition_max_abs']=delta
    live=versions[48000]
    assert np.max(abs(audio(live,seconds=.3,vel=0)))==0
    assert np.max(abs(audio(live,seconds=.3,ramps={'trigger':[(0,0)],'gate':[(0,0)]})))==0
    assert np.max(abs(audio(live,seconds=.3,ramps={'trigger':[(0,0)]})))>.01
    for name,meta in live.params.items():
        lo=audio(live,seconds=.5,params={name:meta['min']})
        # -pi and +pi are the same phase; compare a half-turn instead.
        high=0 if name.endswith('_phase') else meta['max']
        hi=audio(live,seconds=.5,params={name:high})
        # Some modes have little fitted energy; exercise control semantics on
        # a clearly audible mode instead of accepting a numerical epsilon.
        if name.startswith('mode3_'):
            base={'mode3_gain':.3}
            lo=audio(live,seconds=.5,params=base|{name:meta['min']})
            hi=audio(live,seconds=.5,params=base|{name:high})
        assert np.max(abs(lo-hi))>1e-6,name
    for extreme in ('min','max'):
        p={n:v[extreme] for n,v in live.params.items()}
        for hz in (220,440,880):
            audio(live,seconds=.7,pitch=hz,params=p,retrig=[.029,.117])
    result['controls_checked']=len(live.params)
    (ROOT/'tools/pm-doom-kick/validation.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result,indent=2))


if __name__=='__main__':main()
