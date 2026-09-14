#!/usr/bin/env python3
"""Read tagged local samples, measure timbre and shortlist a compact kick family."""
import json
import math
import os
os.environ.setdefault('MPLCONFIGDIR','/tmp/pm-doom-mpl')
from pathlib import Path
import sqlite3

import numpy as np
import soundfile as sf
from scipy import signal
from scipy.spatial.distance import pdist, squareform
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

ROOT=Path(__file__).resolve().parents[2]
OUT=ROOT/'.local/pm-doom-kick'
OUT.mkdir(parents=True,exist_ok=True)
SR=16000


def read_sample(h):
    path=ROOT/'.local/samples'/f'{h}.wav'
    y,sr=sf.read(path,always_2d=True)
    mono=y.mean(axis=1)
    if sr != SR:
        divisor=math.gcd(sr,SR)
        mono=signal.resample_poly(mono,SR//divisor,sr//divisor)
    # Only remove leading silence, never align to the strongest body cycle.
    active=np.flatnonzero(np.abs(mono)>max(1e-7,np.max(np.abs(mono))*1e-3))
    onset=max(0,int(active[0])-16) if len(active) else 0
    return mono[onset:],sr,y.shape[1],onset/SR


def main():
    db=sqlite3.connect(f'file:{ROOT}/.local/samples.db?mode=ro',uri=True)
    rows=db.execute("""select s.hash,s.title from samples s where exists
        (select 1 from sample_tags st join tags t on st.tag_id=t.id where st.sample_id=s.id and t.name='MF DOOM' collate nocase)
        and exists (select 1 from sample_tags st join tags t on st.tag_id=t.id where st.sample_id=s.id and t.name='kick' collate nocase)
        order by s.title""").fetchall()
    results=[]; features=[]; waves=[]
    bands=np.geomspace(25,7500,25)
    for h,title in rows:
        raw,sr,channels,offset=read_sample(h)
        peak=float(np.max(np.abs(raw)))
        y=np.pad(raw,(0,max(0,SR-len(raw))))[:SR]/max(peak,1e-12)
        waves.append(y)
        f,t,z=signal.stft(y,SR,nperseg=512,noverlap=384)
        power=abs(z)**2
        energies=[]
        for a,b in zip(bands[:-1],bands[1:]):
            ix=(f>=a)&(f<b)
            if not ix.any():ix[np.argmin(abs(f-np.sqrt(a*b)))]=True
            for lo,hi in [(0,.025),(.025,.07),(.07,.15),(.15,.3),(.3,.65)]:
                energies.append(float(power[ix][:,(t>=lo)&(t<hi)].mean()))
        energy=np.sum(y*y)
        cumulative=np.cumsum(y*y)/max(energy,1e-20)
        t95=float(np.searchsorted(cumulative,.95)/SR)
        env=np.sqrt(np.mean(y[:16000].reshape(100,160)**2,axis=1))
        spec=abs(np.fft.rfft(y[:8000]*np.hanning(8000),65536))
        freqs=np.fft.rfftfreq(65536,1/SR)
        domain=(freqs>25)&(freqs<180)
        body=float(freqs[domain][np.argmax(spec[domain])])
        hi=float(np.sum(power[f>1000])/max(np.sum(power),1e-20))
        features.append(np.concatenate([np.clip(10*np.log10(np.maximum(energies,1e-10)),-70,0),
            .65*np.clip(20*np.log10(np.maximum(env,1e-6)),-60,0)]))
        results.append(dict(hash=h,title=title,sample_rate=sr,channels=channels,onset_s=offset,
            duration_s=len(raw)/SR,peak=peak,body_peak_hz=body,energy95_s=t95,high_energy_share=hi))
    features=np.array(features)
    distance=squareform(pdist(features))/np.sqrt(features.shape[1])
    families=[]
    for i in range(len(rows)):
        members=np.argsort(distance[i])[:5]
        score=float(np.mean(distance[np.ix_(members,members)]))
        families.append((score,tuple(sorted(map(int,members)))))
    unique=sorted(set(families))
    chosen=list(unique[0][1])
    report={'samples':results,'ranked_families':[{'distance_db':s,'members':[results[i]['title'] for i in group]}
                for s,group in unique[:8]],'selected':[results[i] for i in chosen]}
    (ROOT/'tools/pm-doom-kick/analysis.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps({'count':len(rows),'families':report['ranked_families'][:5]},indent=2))
    fig,axes=plt.subplots(9,5,figsize=(18,20),layout='constrained')
    for i,ax in enumerate(axes.flat):
        if i>=len(rows):ax.axis('off');continue
        ax.plot(np.arange(SR)/SR,waves[i],lw=.45)
        ax.set_xlim(0,.65);ax.set_ylim(-1,1)
        ax.set_title(f"{i}: {results[i]['title']}\n{results[i]['body_peak_hz']:.0f}Hz / t95 {results[i]['energy95_s']:.2f}s",fontsize=8)
        if i in chosen:ax.set_facecolor('#e1eee7')
    fig.savefig(OUT/'all-kicks.png',dpi=120)
    plt.close(fig)
    fig,axes=plt.subplots(5,2,figsize=(13,13),layout='constrained')
    reel=[]
    for j,i in enumerate(chosen):
        y=waves[i]
        axes[j,0].plot(np.arange(SR)/SR,y,lw=.6);axes[j,0].set_xlim(0,.5)
        axes[j,0].set_title(results[i]['title'])
        f,t,z=signal.stft(y,SR,nperseg=512,noverlap=480)
        axes[j,1].pcolormesh(t,f,20*np.log10(np.maximum(abs(z),1e-7)),vmin=-70,vmax=-5,shading='auto')
        axes[j,1].set_ylim(25,7000);axes[j,1].set_yscale('log');axes[j,1].set_xlim(0,.5)
        reel.extend([y,np.zeros(SR//4)])
    fig.savefig(OUT/'selected-kicks.png',dpi=140)
    sf.write(OUT/'selected-references.wav',np.concatenate(reel),SR,subtype='FLOAT')


if __name__=='__main__':main()
