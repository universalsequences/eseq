#!/usr/bin/env python3
"""Identify a reduced damped-resonance model of a processed kick recording."""
import os
os.environ.setdefault('MPLCONFIGDIR','/tmp/pm-doom-mpl')
import sys,json,time
from pathlib import Path
import numpy as np
from scipy import signal,optimize
import soundfile as sf
ROOT=Path(__file__).resolve().parents[2]
OUT=ROOT/'.local/pm-break-kick'; OUT.mkdir(parents=True,exist_ok=True)
sys.path.append(str(ROOT/'tools/pm-doom-kick'))
from analyze import read_sample
from fit import metrics as old_metrics
HASH='18b0fd84278df8a7bab6def01af9d61b170c511b3718c6206ce0cd45dd1ac063'
SR=8000; FRAMES=5760

def basis(p,t):
    # Each normal coordinate has a damped sinusoidal response and exponential
    # frequency relaxation. Free measured modes do not identify drum geometry.
    f,decay,bend,tau,attack=p
    phase=2*np.pi*(f*t+bend*tau*(-np.expm1(-t/tau)))
    env=np.exp(-t/decay)*(-np.expm1(-t/attack))
    return np.column_stack((env*np.cos(phase),env*np.sin(phase)))

def render(rows,frames,sr,ceiling):
    t=np.arange(frames)/sr
    y=sum(basis(r['shape'],t)@np.array(r['weights']) for r in rows)
    return np.clip(y,-ceiling,ceiling)

def main():
    y,*_=read_sample(HASH);target=signal.resample_poly(y,1,2)
    target=np.pad(target,(0,max(0,FRAMES-len(target))))[:FRAMES];t=np.arange(FRAMES)/SR
    ceiling=float(np.quantile(abs(target),.997))
    # Learn modal structure from unclipped samples; restore clipping in the
    # joint fit. Weight attacks without discarding the long tail.
    wt=1/np.sqrt(.3+t/.72);wt[abs(target)>ceiling*.98]*=.15
    residual=target.copy(); rows=[];start=time.monotonic()
    for count in range(8):
        best=None
        for f in np.geomspace(30,1300,22):
            initial=[f,.12,0,.02,.001]
            lo=[max(20,f*.72),.008,-f*.4,.001,.0001]
            hi=[f*1.4,.65,f*2,.15,.025]
            def solve(z,ret=False):
                p=np.exp(z);p[2]-=f*.4
                a=basis(p,t); aw=a*wt[:,None]
                w=np.linalg.lstsq(aw,residual*wt,rcond=None)[0]
                return (p,w) if ret else (a@w-residual)*wt
            z0=np.log([f,.12,f*.4+.001,.02,.001])
            low=np.log([lo[0],lo[1],.00001,lo[3],lo[4]])
            high=np.log([hi[0],hi[1],f*2.4,hi[3],hi[4]])
            r=optimize.least_squares(solve,z0,bounds=(low,high),max_nfev=80,ftol=1e-5)
            err=np.linalg.norm(r.fun)
            if best is None or err<best[0]:best=(err,*solve(r.x,True))
        _,p,w=best;rows.append({'shape':p.tolist(),'weights':w.tolist()})
        # Re-estimate all readout weights together.
        a=np.column_stack([basis(r['shape'],t) for r in rows]);w=np.linalg.lstsq(a*wt[:,None],target*wt,rcond=None)[0]
        for i,r in enumerate(rows):r['weights']=w[2*i:2*i+2].tolist()
        residual=target-a@w
        print('mode',count+1,p,'residual',np.linalg.norm(residual)/np.linalg.norm(target),'sec',round(time.monotonic()-start),flush=True)
    # Joint nonlinear fit including the recording ceiling, with bounded modal
    # poles and no reference-dependent envelope trajectories or stored residual.
    def pack(rows):return np.array([v for r in rows for v in [r['shape'][0],r['shape'][1],r['shape'][2]/r['shape'][0],*r['shape'][3:],*r['weights']]])
    def unpack(v):return [{'shape':[v[i],v[i+1],v[i+2]*v[i],v[i+3],v[i+4]],'weights':v[i+5:i+7].tolist()} for i in range(0,len(v),7)]
    p=pack(rows)
    lo=np.tile([20,.006,-.8,.001,.0001,-8,-8],len(rows));hi=np.tile([3000,.8,20,.2,.035,8,8],len(rows))
    def fun(v):return (render(unpack(v),FRAMES,SR,ceiling)-target)/np.sqrt(.3+t/.72)
    fit=optimize.least_squares(fun,np.clip(p,lo+1e-8,hi-1e-8),bounds=(lo,hi),x_scale='jac',max_nfev=350,ftol=1e-7)
    rows=unpack(fit.x);pred=render(rows,FRAMES,SR,ceiling)
    result={'hash':HASH,'title':'Boom-Bap Kick 53.wav','sample_rate':SR,'ceiling':ceiling,'modes':rows,'metrics':old_metrics(pred,target,SR),'seconds':time.monotonic()-start}
    (Path(__file__).parent/'fit-results.json').write_text(json.dumps(result,indent=2)+'\n')
    sf.write(OUT/'modal-model.wav',render(rows,11520,16000,ceiling),16000,subtype='FLOAT')
    print(result['metrics'],flush=True)
if __name__=='__main__':main()
