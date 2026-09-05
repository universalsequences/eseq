#!/usr/bin/env python3
"""Perceptual-structure regressions and level-matched R8 control auditions.

Unlike a max-sample-difference test, these measure spectral concentration,
attack/tail balance, decay duration and band balance. They cannot replace ears,
but they reject the known comb-ring/weak-articulation implementation.
Requires scipy in addition to the normal audition dependencies.
"""
import argparse
import json
from pathlib import Path
import subprocess
import sys

import numpy as np
from scipy import signal

from audition import Instrument
from verify_r8_kick import float_wav, measure, INSTRUMENT

SR = 48000


def duration(y):
    """Time containing the middle 90% of energy (gain independent)."""
    energy = np.cumsum(np.asarray(y, dtype=np.float64)**2)
    if energy[-1] <= 1e-15:
        raise AssertionError('cannot measure a silent articulation')
    bounds = np.searchsorted(energy, energy[-1]*np.array([.05,.95]))
    return float((bounds[1]-bounds[0])/SR)


def band_energy(y, lo, hi, start=0., end=.18):
    sos = signal.butter(3, [lo,hi], btype='band', fs=SR, output='sos')
    filtered = signal.sosfilt(sos, y)[round(start*SR):round(end*SR)]
    return float(np.mean(filtered**2))


def db_ratio(a,b):
    return float(10*np.log10((a+1e-20)/(b+1e-20)))


def attack_balance(y):
    return db_ratio(np.mean(y[:round(.012*SR)]**2),
                    np.mean(y[round(.035*SR):round(.100*SR)]**2))


def concentration(y, start=.035, end=.18):
    """Long-window concentration and within-band flatness, not spectral tilt."""
    segment = y[round(start*SR):round(end*SR)]
    power = np.abs(np.fft.rfft(segment*np.hanning(len(segment))))**2
    hz = np.fft.rfftfreq(len(segment),1/SR)
    mid = power[(hz>=350)&(hz<4500)]
    top10 = float(np.sort(mid)[-10:].sum()/max(mid.sum(),1e-20))
    flat = []
    for lo,hi in zip((400,600,900,1350,2000),(600,900,1350,2000,3000)):
        p = power[(hz>=lo)&(hz<hi)]
        if len(p) >= 5 and p.sum() > power.sum()*1e-6:
            flat.append(float(np.exp(np.mean(np.log(p+1e-20)))/np.mean(p)))
    return {'top10Share':top10, 'bandFlatness':float(np.mean(flat))}


def matched_reel(path, sounds):
    """One RMS gain per complete sound, then one common headroom gain."""
    rms = [np.sqrt(np.mean(y*y)) for y in sounds]
    assert min(rms) > 1e-8, 'cannot loudness match silence'
    gains = [rms[0]/v for v in rms]
    copies = [y*g for y,g in zip(sounds,gains)]
    headroom = min(1., .9/max(np.max(np.abs(y)) for y in copies))
    gap = np.zeros(SR//5,dtype=np.float32)
    reel = np.concatenate([part for _ in range(3) for y in copies for part in (y,gap)])
    float_wav(path,reel*headroom,SR)
    return [float(g*headroom) for g in gains]


def evaluate(inst,out,presets,old=None,old_presets=None,check=True):
    def render(params=None,seconds=.9):
        y,_ = inst.render(seconds,pitch=261.63,params=params)
        measure(y)
        return y

    results = {}
    ring_lo = render({'ring':.2})
    ring_hi = render({'ring':4.})
    changed = ring_hi-ring_lo
    results['ring'] = concentration(changed)
    shell = {'weight':0.,'head':0.,'beater':0.,'knock':0.,'air':1.}
    short = render(dict(shell,ring=.2))
    long = render(dict(shell,ring=4.))
    results['ring']['durationRatio'] = duration(long)/duration(short)
    # Ring must not prolong the finite impact or re-open the membrane cut.
    dry_lo = render({'air':0.,'ring':.2})
    dry_hi = render({'air':0.,'ring':4.})
    results['ring']['dryMaxDifference'] = float(np.max(np.abs(dry_hi-dry_lo)))
    if check:
        assert results['ring']['bandFlatness'] > .25, results['ring']
        assert results['ring']['top10Share'] < .35, results['ring']
        assert results['ring']['durationRatio'] > 4., results['ring']
        assert results['ring']['dryMaxDifference'] < 1e-5, results['ring']
    matched_reel(out/'ring-short-long.wav',[ring_lo,ring_hi])
    # Repeated real triggers advance the noise stream. A single fortunate
    # realization must not be sufficient to pass the diffuse-shell contract.
    hits = [0.,.8,1.6,2.4,3.2]
    a,_ = inst.render(4.,pitch=261.63,params={'ring':.2},retrig=hits[1:])
    b,_ = inst.render(4.,pitch=261.63,params={'ring':4.},retrig=hits[1:])
    result_per_hit = [concentration(b-a,start=t+.035,end=t+.18) for t in hits]
    results['ring']['retriggerRealizations'] = result_per_hit
    if check:
        for stats in result_per_hit:
            assert stats['bandFlatness'] > .25 and stats['top10Share'] < .35, stats

    # Hardness is tested on actual excitation, without the bass/tail hiding it.
    impact = {'weight':0.,'head':0.,'air':0.,'knock':.8,'beater':.6}
    soft = render(dict(impact,hardness=-1.))
    hard = render(dict(impact,hardness=1.))
    brightness = lambda y: db_ratio(band_energy(y,2500,12000,end=.04),band_energy(y,150,1200,end=.04))
    results['hardness'] = {'softToHardDurationRatio':duration(soft)/duration(hard),
                           'brightnessDeltaDb':brightness(hard)-brightness(soft)}
    if check:
        assert results['hardness']['softToHardDurationRatio'] > 2.5, results['hardness']
        assert results['hardness']['brightnessDeltaDb'] > 10., results['hardness']
    matched_reel(out/'felt-wood.wav',[soft,hard])

    fast = render({'decay':.2})
    slow = render({'decay':4.})
    results['decay'] = {'durationRatio':duration(slow)/duration(fast)}
    if check:
        assert results['decay']['durationRatio'] > 3., results['decay']
    matched_reel(out/'decay-short-long.wav',[fast,slow])

    open_hit = render()
    damped = render({'damp':1.})
    results['damp'] = {'attackBalanceDeltaDb':attack_balance(damped)-attack_balance(open_hit)}
    if check:
        assert results['damp']['attackBalanceDeltaDb'] > 10., results['damp']
    matched_reel(out/'open-muffled.wav',[open_hit,damped])

    punch = render({'punch':1.})
    results['punch'] = {'attackBalanceDeltaDb':attack_balance(punch)-attack_balance(open_hit)}
    if check:
        assert results['punch']['attackBalanceDeltaDb'] > 3., results['punch']
    matched_reel(out/'punch-neutral-full.wav',[open_hit,punch])

    def bass_balance(y):
        return db_ratio(band_energy(y,30,90,start=.02),band_energy(y,100,300,start=.02))
    weight_lo = render({'weight':.2})
    weight_hi = render({'weight':2.})
    results['weight'] = {'bassBalanceDeltaDb':bass_balance(weight_hi)-bass_balance(weight_lo)}
    if check:
        assert results['weight']['bassBalanceDeltaDb'] > 8., results['weight']
    matched_reel(out/'weight-light-heavy.wav',[weight_lo,weight_hi])

    hp = next(p['params'] for p in presets if p['id']=='Hard Knocker')
    knocker = render(hp)
    results['hardKnocker'] = {'attackBalanceDb':attack_balance(knocker),
                              'durationSeconds':duration(knocker), **measure(knocker)}
    if old is not None:
        a,_ = old.render(.9,pitch=261.63,params={'ring':.2})
        b,_ = old.render(.9,pitch=261.63,params={'ring':4.})
        results['oldRing'] = concentration(b-a)
        results['ringABGains'] = matched_reel(out/'ring-old-new.wav',[b,ring_hi])
        if old_presets:
            op = next(p['params'] for p in old_presets if p['id']=='Hard Knocker')
            before,_ = old.render(.9,pitch=261.63,params=op)
            results['oldHardKnocker'] = {'attackBalanceDb':attack_balance(before),
                                        'durationSeconds':duration(before), **measure(before)}
            results['hardKnockerABGains'] = matched_reel(out/'hard-knocker-old-new.wav',[before,knocker])
            if check:
                delta = results['hardKnocker']['attackBalanceDb']-results['oldHardKnocker']['attackBalanceDb']
                assert delta > 6., f'Hard Knocker attack/tail improvement only {delta:.2f} dB'
    (out/'articulation.json').write_text(json.dumps(results,indent=2)+'\n')
    print(json.dumps(results,indent=2),flush=True)
    return results


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--instrument',type=Path,default=INSTRUMENT)
    ap.add_argument('--old-instrument',type=Path)
    ap.add_argument('--old-presets',type=Path)
    ap.add_argument('--presets',type=Path,default=INSTRUMENT.with_suffix('.presets'))
    ap.add_argument('--out',type=Path,required=True)
    ap.add_argument('--measure-only',action='store_true',help='diagnostic pass before thresholds are met')
    args = ap.parse_args()
    args.out.mkdir(parents=True,exist_ok=True)
    inst = Instrument(str(args.instrument))
    subprocess.run([sys.executable,str(Path(__file__).with_name('check_fusion.py')),str(Path(inst.build_dir)/'patch.c')],check=True)
    old = Instrument(str(args.old_instrument)) if args.old_instrument else None
    old_presets = json.loads(args.old_presets.read_text())['presets'] if args.old_presets else None
    presets = json.loads(args.presets.read_text())['presets']
    evaluate(inst,args.out,presets,old,old_presets,check=not args.measure_only)


if __name__=='__main__':
    main()
