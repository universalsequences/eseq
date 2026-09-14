"""Fit the original three-resonance model to Kick 53 without changing its bounds."""
import sys,json,argparse
from pathlib import Path
import numpy as np
from scipy import signal,optimize
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'pm-doom-kick'))
from fit import NAMES,LOW,HIGH,encode,decode,pitch_seed,render,metrics
from analyze import read_sample
raw,*_=read_sample('18b0fd84278df8a7bab6def01af9d61b170c511b3718c6206ce0cd45dd1ac063')
y=signal.resample_poly(raw,1,2);y=np.pad(y,(0,5760-len(y)));t=np.arange(len(y))/8000
seeds=json.loads((Path(__file__).resolve().parents[1]/'pm-doom-kick/selection.json').read_text())['selected']
ap=argparse.ArgumentParser();ap.add_argument('--original-bounds',action='store_true');args=ap.parse_args()
# Give the old topology the full tail, so a short gate does not handicap it.
HIGH=HIGH.copy()
if not args.original_bounds:
    HIGH[NAMES.index('hold_ms')]=800;HIGH[NAMES.index('fade_ms')]=300
lo,hi=encode(LOW),encode(HIGH);best=None
for row in seeds:
    p=[row['params'][n] for n in NAMES]
    fit=optimize.least_squares(lambda z:(render(decode(z),len(y),8000)-y)/np.sqrt(.3+t/.72),np.clip(encode(p),lo+1e-8,hi-1e-8),bounds=(lo,hi),max_nfev=350,ftol=1e-7)
    if best is None or np.linalg.norm(fit.fun)<np.linalg.norm(best.fun):best=fit
p=decode(best.x);r={'metrics':metrics(render(p,len(y),8000),y,8000),'params':dict(zip(NAMES,map(float,p)))}
Path(__file__).with_name('baseline.json' if args.original_bounds else 'baseline-long-tail.json').write_text(json.dumps(r,indent=2)+'\n');print(r['metrics'])
