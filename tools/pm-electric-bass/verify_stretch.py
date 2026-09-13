#!/usr/bin/env python3
"""Independent staircase-read reference, block/event tests and audible stretch comparisons."""
import argparse
import ctypes
import hashlib
import json
import tempfile
import time
from pathlib import Path

import numpy as np
from verify import BANK, OUT, ROOT, SOURCE, instrument, render, rms, write_wav


def reference(dry, sr, slow_ms, onsets, slow_at=lambda t: 0.0):
    """Apply the slice-repeat read pattern to a dry render.

    grain clock: pos(t) = (t+1) mod G, boundary at pos == 0 unless an onset.
    tap: 0 at onset, += floor(G*slow/100) at each boundary, capped at 95000.
    output: linear crossfade over xfade samples from the previous tap to the
    current one (raised cosine) after every tap change; taps are integer sample delays.
    """
    grain_ms, xfade_ms = slow_ms
    G = max(2, int(np.floor(0.001*sr*grain_ms)))
    X = max(1.0, 0.001*sr*xfade_ms)
    onset = np.zeros(len(dry), bool); onset[list(onsets)] = True
    y = np.zeros_like(dry)
    tap = 0; old = 0; age = 10**9
    for t in range(len(dry)):
        pos = (t+1) % G
        switched = onset[t] or pos == 0
        if onset[t]:
            old, tap = tap, 0
        elif pos == 0:
            old, tap = tap, min(95000, tap + int(np.floor(G*0.01*slow_at(t))))
        age = 0 if switched else age + 1
        xf = 0.5 - 0.5*np.cos(np.pi*min(1.0, age / X))
        a = dry[t-old] if t-old >= 0 else 0.0
        b = dry[t-tap] if t-tap >= 0 else 0.0
        y[t] = (1-xf)*a + xf*b
    return y


def modulation(y, sr):
    """Level-modulation depth (min, max of fast/slow envelope) and dominant rate."""
    w = sr//50
    e = np.sqrt(np.convolve(y**2, np.ones(w)/w, 'same') + 1e-12)
    m = (e/np.convolve(e, np.ones(sr//5)/(sr//5), 'same'))[sr//5:int(.8*sr)]
    spec = np.abs(np.fft.rfft((m-m.mean())*np.hanning(len(m))))
    fr = np.fft.rfftfreq(len(m), 1/sr); band = (fr > 4) & (fr < 80)
    return float(m.min()), float(m.max()), float(fr[band][np.argmax(spec[band])])


def benchmark(inst, slow=0):
    frames = inst.max_frames
    mem = inst.fresh_memory()
    if "texture.slow" in inst.params:
        mem[inst.params["texture.slow"]["cellId"]] = slow
    ins = [np.zeros(frames,np.float32) for _ in range(inst.n_in)]
    outs = [np.zeros(frames,np.float32) for _ in range(inst.n_out)]
    ins[inst.inputs['pitch']][:] = 55
    ins[inst.inputs['velocity']][:] = .8
    ins[inst.inputs['gate']][:] = 1
    ptr = ctypes.POINTER(ctypes.c_float)
    ip = (ptr*inst.n_in)(*[a.ctypes.data_as(ptr) for a in ins])
    op = (ptr*inst.n_out)(*[a.ctypes.data_as(ptr) for a in outs])
    args = (ip,op,frames,mem.ctypes.data_as(ctypes.c_void_p),ctypes.byref(inst.context),None)
    times = []
    for _ in range(5):
        ins[inst.inputs['trigger']][0] = 1
        inst.process_fn(*args)
        ins[inst.inputs['trigger']][0] = 0
        start = time.perf_counter()
        for _ in range(400):
            inst.process_fn(*args)
        times.append((time.perf_counter()-start)/400*1e6)
    return float(np.median(times))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, help="Optional saved earlier DSP source")
    args = parser.parse_args()
    errors = []
    params = BANK[0]['params']
    with tempfile.TemporaryDirectory(prefix='pm-bass-stretch-') as temp:
        source = Path(temp)/'dsp.lisp'
        # Tap the pre-tone stretched string so the reference needs no filter model.
        source.write_text(SOURCE.read_text().replace('(out signal ', '(out stretched '))
        for sr in (44100,48000,96000):
            inst = instrument(sr=sr,source=source)
            for slow in (8,18,30,40):
                for grain, xfade in ((20,0.5),(55,2.5),(120,8),(200,30)):
                    p = params | {'texture.slow':slow,'texture.grain_ms':grain,'texture.xfade_ms':xfade}
                    retrig = [.173]
                    y = render(inst,seconds=.4,pitch=55,params=p,retrig=retrig)
                    dry = render(inst,seconds=.4,pitch=55,params=p|{'texture.slow':0},retrig=retrig)
                    ref = reference(dry,sr,(grain,xfade),[0]+[int(t*sr) for t in retrig],lambda t: slow)
                    error = rms(y-ref)/rms(ref)
                    errors.append(error)
                    assert error < 1e-5, (sr,slow,grain,xfade,error)
    inst = instrument()
    blocks = [instrument(block=b) for b in (32,128,256)]
    max_partition = 0
    for slow in (10,25,40):
        p = {'texture.slow':slow,'texture.grain_ms':37}
        versions = [render(b,seconds=.6,pitch=55,params=p,retrig=[.173,.317],gate_off=.431) for b in blocks]
        error = max(float(np.max(np.abs(versions[0]-v))) for v in versions[1:])
        max_partition = max(max_partition,error)
        assert error < 2e-5,error
        # The tap can lag wall clock by up to slow% of the note, so allow for that.
        # Note-off is applied after the stretch in wall-clock time: a 0.12 s
        # release must be 60 dB down within 0.2 s of the gate, however far
        # the tap lags.
        y = render(inst,seconds=2.2,pitch=55,params=p|{'texture.slow':40},gate_off=1.0)
        assert rms(y[int(1.25*48000):]) < rms(y[int(.9*48000):int(1.0*48000)])*1e-3
        assert rms(y[int(1.9*48000):]) < 1e-6
        assert np.max(np.abs(render(inst,seconds=.2,params=p,ramps={'trigger':[(0,0)],'gate':[(0,0)]}))) == 0
    # Slow 0 is a bypass within float rounding: no delay-line coloration.
    with tempfile.TemporaryDirectory(prefix='pm-bass-bypass-') as temp:
        source = Path(temp)/'dsp.lisp'
        source.write_text(SOURCE.read_text().replace('(out signal ', '(out string '))
        dry_string = render(instrument(source=source),seconds=.5,pitch=55,retrig=[.173])
        source.write_text(SOURCE.read_text().replace('(out signal ', '(out stretched '))
        bypass = float(np.max(np.abs(dry_string - render(instrument(source=source),seconds=.5,pitch=55,retrig=[.173]))))
        assert bypass < 1e-6, bypass
    # A live slow change only alters later steps: within any slice the output
    # is an exact integer shift of the dry string, never a mid-slice jump.
    with tempfile.TemporaryDirectory(prefix='pm-bass-live-') as temp:
        source = Path(temp)/'dsp.lisp'
        source.write_text(SOURCE.read_text().replace('(out signal ', '(out stretched '))
        live = instrument(source=source)
        dry = render(live,seconds=.5,pitch=55,params={'texture.grain_ms':50})
        y = render(live,seconds=.5,pitch=55,params={'texture.grain_ms':50},
                   ramps={'texture.slow':[(0,10),(.2,10),(.2001,35)]})
        G = 2400; X = int(.008*48000)+1
        for k in range(1, len(y)//G):
            seg = slice(k*G-1+X, (k+1)*G-1)
            shifts = [s for s in range(0,G*k) if np.max(np.abs(y[seg] - dry[seg.start-s:seg.stop-s])) < 1e-6]
            assert len(shifts) == 1, (k, shifts)
    # Level wobble exists and its rate follows the slice length.
    wobble = {}
    for grain in (40,55,80):
        for f in (41.2,55,73.4):
            lo, hi, rate = modulation(render(inst,seconds=1,pitch=f,params={'texture.slow':20,'texture.grain_ms':grain,'string.mute':0}),48000)
            wobble[f'{grain}ms@{f}Hz'] = {'depth_min':lo,'depth_max':hi,'rate_hz':rate}
            assert lo < .9 or hi > 1.1, (grain,f,lo,hi)
    # Original phrase timing is identical across three slow settings.
    phrase = [(0,28,.74,.66),(.75,28,.45,.19),(1.5,35,.63,.32),(2.25,38,.57,.28),
              (3,33,.71,.62),(4,28,.76,.68),(5.25,40,.5,.22),(5.75,38,.62,.31),
              (6.5,35,.66,.35),(7.25,26,.55,.28)]
    beat = 60/88
    demos = []
    for slow in (0,12,25):
        demo = np.zeros(int((8*beat+1.5)*48000),np.float32)
        for onset,note,velocity,length in phrase:
            y = render(inst,seconds=length*beat+1.0,pitch=440*2**((note-69)/12),vel=velocity,
                params={'texture.slow':slow},gate_off=length*beat)
            start = int(onset*beat*48000)
            demo[start:start+len(y)] += y
        assert np.max(np.abs(demo)) < 1
        write_wav(str(OUT/f'stretch-{slow}.wav'),demo,48000)
        demos.extend([demo,np.zeros(12000,np.float32)])
    write_wav(str(OUT/'stretch-comparison.wav'),np.concatenate(demos),48000)
    report = {'reference_cases':len(errors),'max_reference_nrmse':max(errors),
              'max_partition_abs':max_partition,'bypass_max_abs':bypass,'live_slow_slice_exact':True,
              'wobble':wobble,
              'native_call_us_128_frames':benchmark(inst),
              'active_stretch_native_call_us_128_frames':benchmark(inst,20),'compiler_sha256':inst.compiler_sha256,
              'source_sha256':hashlib.sha256(SOURCE.read_bytes()).hexdigest()}
    baseline = args.baseline
    if baseline is not None:
        report['baseline_sha256'] = hashlib.sha256(baseline.read_bytes()).hexdigest()
        old = instrument(source=baseline)
        old_y = render(old,seconds=.5,pitch=55,retrig=[.173])
        new_y = render(inst,seconds=.5,pitch=55,retrig=[.173])
        report['zero_stretch_baseline_max_abs'] = float(np.max(np.abs(old_y-new_y)))
        assert report['zero_stretch_baseline_max_abs'] < 2e-6
        report['baseline_native_call_us_128_frames'] = benchmark(old)
    (ROOT/'tools/pm-electric-bass/stretch-validation.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))


if __name__ == '__main__':
    main()
