#!/usr/bin/env python3
"""Fit one reduced resonant kick topology; no waveform tables or residual playback."""
import json
import os
os.environ.setdefault('MPLCONFIGDIR','/tmp/pm-doom-mpl')
from pathlib import Path
import time
import numpy as np
from scipy import signal,optimize
import soundfile as sf
from analyze import ROOT,OUT,SR,read_sample

FIT_SR=8000
# Two tension relaxation times, three resonant coordinates, a finite gate,
# and a saturating recording stage. Modes share the tension law.
NAMES=['frequency','bend_fast','fast_ms','bend_slow','slow_ms','attack_ms','hold_ms','fade_ms',
       'level','saturation','shape',
       'mode1_gain','mode1_decay','mode1_phase','mode2_ratio','mode2_gain','mode2_decay','mode2_phase',
       'mode3_ratio','mode3_gain','mode3_decay','mode3_phase',
       'mode2_bend','mode3_bend','mode2_attack_ms','mode3_attack_ms']
LOW=np.array([15,0,.25,0,5,.03,60,2,.05,0,2, .01,8,-np.pi, .65,0,5,-np.pi, 1.8,0,1,-np.pi, 0,0,.03,.03])
HIGH=np.array([100,30,30,10,250,20,350,100,1.2,1,24, 8,400,np.pi, 1.8,3,300,np.pi, 8,1,100,np.pi, 2,2,50,20])
LOG=set([0,2,4,5,7,12,16,20,24,25])


def encode(p):
    return np.array([np.log(v) if i in LOG else v for i,v in enumerate(p)])

def decode(z):
    return np.array([np.exp(v) if i in LOG else v for i,v in enumerate(z)])


def render(p,frames,sr):
    f,fast,ft,slow,st,attack,hold,fade,level,sat,shape=p[:11]
    t=np.arange(frames)/sr
    tau1=ft*.001;tau2=st*.001
    bend=fast*tau1*(-np.expm1(-t/tau1))+slow*tau2*(-np.expm1(-t/tau2))
    y=np.zeros(frames)
    for ratio,gain,decay,phase,bend_scale,attack_time in [(1,*p[11:14],1,attack),(*p[14:18],p[22],p[24]),(*p[18:22],p[23],p[25])]:
        cycles=f*ratio*(t+bend_scale*bend)
        rise=-np.expm1(-t/(attack_time*.001))
        y += gain*np.exp(-t/(decay*.001))*np.sin(2*np.pi*cycles+phase)*rise
    # A bounded smooth soft/hard saturation continuum. Shape controls knee.
    compressed=y/(1+np.abs(y)**shape)**(1/shape)
    y=level*((1-sat)*y+sat*compressed)
    u=np.clip((t-hold*.001)/(fade*.001),0,1)
    return y*(1-u*u*(3-2*u))


def pitch_seed(y,sr):
    low=signal.sosfiltfilt(signal.butter(3,450,fs=sr,output='sos'),y)
    ix=np.flatnonzero((low[:-1]<=0)&(low[1:]>0))
    crosses=(ix-low[ix]/(low[ix+1]-low[ix]))/sr
    ts=(crosses[1:]+crosses[:-1])/2
    hz=1/np.diff(crosses)
    valid=(hz>20)&(hz<500)&(ts<.19)
    ts,hz=ts[valid],hz[valid]
    x0=[35,250,.006,90,.045]
    def fun(x):return (x[0]+x[1]*np.exp(-ts/x[2])+x[3]*np.exp(-ts/x[4])-hz)
    fit=optimize.least_squares(fun,x0,bounds=([15,0,.00025,0,.005],[100,2000,.03,1000,.25]),loss='soft_l1',f_scale=12,max_nfev=500)
    f,a,ta,b,tb=fit.x
    return [f,a/f,ta*1000,b/f,tb*1000]


def metrics(y,target,sr):
    error=np.linalg.norm(y-target)/np.linalg.norm(target)
    hop=int(.005*sr)
    length=len(y)//hop*hop
    env=lambda a:np.sqrt(np.mean(a[:length].reshape(-1,hop)**2,axis=1))
    a,b=env(y),env(target)
    band=[]
    for width in (128,512,1024):
        _,_,za=signal.stft(y,sr,nperseg=width,noverlap=width*3//4)
        _,_,zb=signal.stft(target,sr,nperseg=width,noverlap=width*3//4)
        band.append(float(np.linalg.norm(abs(za)-abs(zb))/np.linalg.norm(abs(zb))))
    return {'wave_nrmse':float(error),'correlation':float(np.corrcoef(y,target)[0,1]),
        'envelope_nrmse':float(np.linalg.norm(a-b)/np.linalg.norm(b)),
        'spectral_convergence':float(np.mean(band)),
        'level_error_db':float(20*np.log10(np.linalg.norm(y)/np.linalg.norm(target)))}


def main():
    report=json.loads((ROOT/'tools/pm-doom-kick/analysis.json').read_text())
    fits=[];start=time.monotonic()
    names=set(v for family in report['ranked_families'][:5] for v in family['members'])
    # Recorded scalar warm starts make this run independent of local scratch history.
    previous=json.loads((Path(__file__).parent/'fit-seeds.json').read_text())
    rng=np.random.default_rng(20260912)
    for row in [x for x in report['samples'] if x['title'] in names]:
        raw,*_=read_sample(row['hash'])
        target=signal.resample_poly(raw,1,2)
        frames=int(.4*FIT_SR)
        target=np.pad(target,(0,max(0,frames-len(target))))[:frames]
        seed=pitch_seed(target,FIT_SR)
        p=np.array(seed+[2,180,40,.9,.4,6, 1,100,0,1.25,.3,70,0,3,.05,10,0,1,1,2,.5],float)
        # Distinct phase seeds avoid forcing a wrong half-cycle alignment.
        lo,hi=encode(LOW),encode(HIGH)
        best=None
        starts=[p.copy() for _ in range(8)]
        for j,start_p in enumerate(starts):
            start_p[13]=(-2.,0.,2.)[j%3]
            start_p[14]=(.8,1.05,1.25,1.5)[j%4]
            start_p[17]=rng.uniform(-np.pi,np.pi)
        if row['hash'] in previous:
            old=previous[row['hash']]
            starts.insert(0,np.array([old.get(n,p[i]) for i,n in enumerate(NAMES)]))
        for p in starts:
            initial=np.clip(encode(np.clip(p,LOW,HIGH)),lo+1e-9,hi-1e-9)
            weights=1/np.sqrt(.15+np.arange(frames)/frames)
            def residual(z):
                y=render(decode(z),frames,FIT_SR)
                return (y-target)*weights
            fitted=optimize.least_squares(residual,initial,bounds=(lo,hi),max_nfev=700,ftol=1e-8,diff_step=1e-5)
            if best is None or np.sum(fitted.fun**2)<np.sum(best.fun**2):best=fitted
        p=decode(best.x)
        y=render(p,frames,FIT_SR)
        result=row|{'params':dict(zip(NAMES,map(float,p))),'metrics':metrics(y,target,FIT_SR),'evaluations':best.nfev}
        fits.append(result)
        slug=Path(row['title']).stem.replace(' ','-')
        sf.write(OUT/f'{slug}-reference.wav',raw,SR,subtype='FLOAT')
        full=render(p,int(.4*SR),SR)
        sf.write(OUT/f'{slug}-model.wav',full,SR,subtype='FLOAT')
        sf.write(OUT/f'{slug}-AB.wav',np.concatenate([np.pad(raw,(0,max(0,int(.4*SR)-len(raw))))[:int(.4*SR)],np.zeros(SR//4),full]),SR,subtype='FLOAT')
        (ROOT/'tools/pm-doom-kick/fit-results.json').write_text(json.dumps({'topology':'three resonances, shared biexponential tension relaxation, saturation and gate','fits':fits},indent=2)+'\n')
        print(row['title'],result['metrics'],'elapsed',round(time.monotonic()-start,1),flush=True)


if __name__=='__main__':main()
