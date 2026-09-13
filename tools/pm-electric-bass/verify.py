#!/usr/bin/env python3
"""Validate the compiled bass ABI and render a short original audition phrase."""
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
os.environ.setdefault('AUDITION_CACHE', str(ROOT / '.local/pm-electric-bass-cache'))
sys.path.insert(0, str(ROOT / 'tools/audition'))
from audition import Instrument, write_wav

SOURCE = ROOT / 'content/instruments/Physical Models/PM Electric Bass/dsp.lisp'
BANK = json.loads(SOURCE.parent.with_suffix('.presets').read_text())['presets']
OUT = ROOT / '.local/pm-electric-bass'
OUT.mkdir(parents=True, exist_ok=True)
checks = []


def instrument(sr=48000, block=128, source=SOURCE):
    target = {('Darwin', 'arm64'): 'DGenLisp-macos-arm64',
              ('Linux', 'x86_64'): 'DGenLisp-linux-x86_64'}[(platform.system(), platform.machine())]
    inst = Instrument(str(source), sample_rate=sr, max_frames=block,
        compiler=os.environ.get('ESEQ_DGENLISP_TOOL', str(ROOT / 'crates/sequencer/tools' / target)),
        toolchain_root=str(ROOT / 'crates/sequencer/tools/dgen-toolchain'))
    subprocess.run([sys.executable, str(ROOT / 'tools/audition/check_fusion.py'),
                    str(Path(inst.build_dir) / 'patch.c')], check=True, capture_output=True)
    return inst


def render(inst, **kwargs):
    y, mem = inst.render(**kwargs)
    assert np.isfinite(y).all() and np.isfinite(mem).all(), kwargs
    assert np.array_equal(y[:, 0], y[:, 1]), 'mono pickup must give identical channels'
    checks.append('finite render')
    return y[:, 0]


def rms(y):
    return float(np.sqrt(np.mean(np.square(y.astype(float)))))


def fundamental(y, sr, expected):
    x = y[int(.06*sr):int(.85*sr)]
    n = 2 ** int(np.ceil(np.log2(len(x)*8)))
    mag = np.abs(np.fft.rfft(x*np.hanning(len(x)), n))
    lo, hi = int(expected*.93*n/sr), int(expected*1.07*n/sr)
    k = lo + np.argmax(mag[lo:hi+1])
    a,b,c = np.log(np.maximum(mag[k-1:k+2],1e-30))
    return (k + .5*(a-c)/(a-2*b+c))*sr/n


def main():
    inst = instrument()
    public = {n for n in inst.params if not n.startswith('__')}
    max_peak = 0
    for preset in BANK:
        assert set(preset['params']) == public, preset['name']
        for note in (23,28,33,40,52,64):
            y = render(inst, seconds=1.2, pitch=440*2**((note-69)/12), params=preset['params'], gate_off=.65)
            assert rms(y[:4800]) > .001, (preset['name'], note)
            max_peak = max(max_peak, float(np.max(np.abs(y))))
    assert max_peak < .9, max_peak
    cents = []
    for sr in (44100,48000,96000):
        test = instrument(sr)
        for note in (23,28,33,40,52,64):
            f = 440*2**((note-69)/12)
            y = render(test, seconds=1, pitch=f, params={'string.mute':0,'string.damping':0})
            cents.append(float(1200*np.log2(fundamental(y,sr,f)/f)))
    assert max(map(abs,cents)) < 3, cents
    # Physical controls change decay and excitation, independently of gain.
    open_note = render(inst, seconds=1, pitch=55, params={'string.mute':0})
    muted = render(inst, seconds=1, pitch=55, params={'string.mute':.8})
    assert rms(muted[24000:]) < rms(open_note[24000:])*.01
    released = render(inst, seconds=1, pitch=55, params={'string.mute':0}, gate_off=.2)
    assert rms(released[36000:]) < rms(open_note[36000:])*.001
    quiet = render(inst, seconds=.5, pitch=55, vel=.3)
    loud = render(inst, seconds=.5, pitch=55, vel=1)
    assert rms(quiet) < rms(loud)*.3
    for name in sorted(public):
        p = inst.params[name]
        a = render(inst, seconds=.5, pitch=55, params=({'texture.slow':25} if name in ('texture.grain_ms','texture.xfade_ms') else {}) | {name:p['min']}, gate_off=.15, vel=.7)
        b = render(inst, seconds=.5, pitch=55, params=({'texture.slow':25} if name in ('texture.grain_ms','texture.xfade_ms') else {}) | {name:p['max']}, gate_off=.15, vel=.7)
        assert np.max(np.abs(a-b)) > 1e-5, name
    for boundary in ('min','max'):
        params = {n:inst.params[n][boundary] for n in public if n != 'output.gain'}
        for pitch in (25,55,1200):
            render(inst, seconds=.5, pitch=pitch, params=params, gate_off=.2, retrig=[.13,.29])
    render(inst, seconds=2, pitch=55, retrig=[.173,.317,.501], gate_off=1,
        ramps={n:[(0,inst.params[n]['min']),(.7,inst.params[n]['max']),(1.4,inst.params[n]['min'])]
               for n in public if n != 'output.gain'})
    silent = render(inst, seconds=.2, ramps={'trigger':[(0,0)],'gate':[(0,0)]})
    assert np.max(np.abs(silent)) == 0
    gate_only = render(inst, seconds=.2, pitch=55, ramps={'trigger':[(0,0)]})
    assert np.max(np.abs(gate_only)) > .01
    outputs = [render(instrument(block=b), seconds=.7, pitch=55, retrig=[.173,.317], gate_off=.43)
               for b in (32,128,256)]
    partition_error = max(float(np.max(np.abs(outputs[0]-y))) for y in outputs[1:])
    assert partition_error < 2e-5, partition_error
    # Independent closed-form modal solution, before the output tone filter.
    # Checks physical frequencies, signed excitation, pickup notches, decay,
    # initialization and recurrence scheduling together, not just nonzero audio.
    with tempfile.TemporaryDirectory(prefix='pm-bass-reference-') as temp:
        path = Path(temp)/'dsp.lisp'
        path.write_text(SOURCE.read_text().replace('(out signal ', '(out string '))
        raw = instrument(source=path)
        params = BANK[0]['params'] | {'string.mute':0,'string.stiffness':0}
        actual = render(raw, seconds=.35, pitch=55, params=params)
        n = np.arange(1,65,dtype=float)
        t = np.arange(len(actual))/48000
        p = params['pluck.position']; w = .004+.075*params['pluck.softness']
        f = 55*n
        rate = np.log(1000)/params['string.decay_s'] + (.2+24*params['string.damping']**2)*(f/1000)**2
        aperture = params['pickup.aperture']
        amp = 2*np.sin(np.pi*n*p)/(np.pi**2*n*p*(1-p)) * np.exp(-.5*(np.pi*n*w)**2)
        amp *= np.sin(np.pi*n*params['pickup.position'])*np.sinc(n*aperture/2)
        reference = np.sum(amp[:,None]*np.exp(-rate[:,None]*t)*np.sin(2*np.pi*f[:,None]*t),axis=0)
        reference_error = rms(actual-reference)/rms(reference)
        assert reference_error < .002, reference_error
    # An original two-bar phrase rendered independently for each voicing.
    beat = 60/88
    phrase = [(0,28,.74,.66),(.75,28,.45,.19),(1.5,35,.63,.32),(2.25,38,.57,.28),
              (3,33,.71,.62),(4,28,.76,.68),(5.25,40,.5,.22),(5.75,38,.62,.31),
              (6.5,35,.66,.35),(7.25,26,.55,.28)]
    for preset in BANK:
        demo = np.zeros(int((8*beat+1)*48000),np.float32)
        for onset,note,vel,length in phrase:
            y = render(inst, seconds=length*beat+.65, pitch=440*2**((note-69)/12),
                       vel=vel, params=preset['params'], gate_off=length*beat)
            start = int(onset*beat*48000)
            demo[start:start+len(y)] += y
        write_wav(str(OUT/(preset['id']+'.wav')), demo,48000)
    report = {'renders':len(checks),'max_preset_peak':max_peak,'max_pitch_error_cents':max(map(abs,cents)),
              'partition_max_abs':partition_error,'modal_reference_nrmse':reference_error,
              'source_sha256':hashlib.sha256(SOURCE.read_bytes()).hexdigest(),
              'compiler_sha256':inst.compiler_sha256,'platform':platform.platform()}
    (ROOT/'tools/pm-electric-bass/validation.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))


if __name__ == '__main__':
    main()
