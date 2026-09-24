#!/usr/bin/env python3
"""Render the factory FM DSP and assert envelope, routing and gain behavior.

The host probe separately covers import resolution. Here the two explicit
imports are expanded for the raw compiler audition harness, with unknown
imports rejected. No alternative DSP implementation is substituted.
"""
import json
import ctypes
import os
import platform
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import time
import numpy as np
sys.dont_write_bytecode = True
from build import ROOT, DEST, ALGORITHMS, PARTIALS, TABLE_SAMPLES, anchors
sys.path.insert(0, str(ROOT / 'tools/audition'))
from audition import Instrument

os.environ.setdefault('DGEN_BINARY_AUDIT_TOOL', str(Path.home() / 'code/swift/dgen/scripts/audit-dgen-dylib.sh'))
COMPILER = os.environ.get('ESEQ_DGENLISP_TOOL') or ROOT / ('crates/sequencer/tools/DGenLisp-macos-arm64' if platform.system()=='Darwin' else 'crates/sequencer/tools/DGenLisp-linux-x86_64')
TOOLCHAIN = ROOT / 'crates/sequencer/tools/dgen-toolchain'

def expand(source):
    def load(match):
        name = match[1]
        assert name in {'drift-filter-core', 'digi-fm-envelope'}, name
        return (ROOT / 'content/defmacros' / name / 'macro.lisp').read_text()
    return re.sub(r'\(use-defmacro ([\w-]+)\)', load, source)

def compile_source(directory, name, source, sr=48000, block=128):
    path = directory / (name + '.lisp')
    path.write_text(expand(source))
    inst = Instrument(path, sample_rate=sr, max_frames=block, compiler=COMPILER, toolchain_root=str(TOOLCHAIN))
    subprocess.run([sys.executable, str(ROOT/'tools/audition/check_fusion.py'), str(Path(inst.build_dir)/'patch.c')], check=True)
    return inst

def rms(y): return float(np.sqrt(np.mean(y.astype(float)**2)))

def main():
    results = {}
    with tempfile.TemporaryDirectory(prefix='digi-fm-validation-') as tmp:
        directory = Path(tmp)
        shutil.copyfile(DEST / 'spectra.json', directory / 'spectra.json')
        source = (DEST/'dsp.lisp').read_text()
        inst = compile_source(directory, 'fm', source)
        small = compile_source(directory, 'block16', source, block=16)
        a,_ = inst.render(.1,gate_off=.037)
        b,_ = small.render(.1,gate_off=.037)
        np.testing.assert_allclose(a,b,atol=1e-7,rtol=1e-6)
        results['block_size_parity'] = '16 and 128 frames passed'
        for algorithm in range(1, 9):
            y, state = inst.render(.25, pitch=220, params={'algorithm':algorithm})
            assert np.isfinite(y).all() and np.isfinite(state).all()
            assert rms(y) > .001
            results[f'algorithm_{algorithm}_rms'] = rms(y)
        # Independent graph evaluator checks generated same-sample ordering,
        # output taps, feedback placement and bipolar spectral assignment.
        kernel_source = source[:source.index('; Discrete routing changes')]
        kernel_source += """
(def (kx ky ka k1 k2) (df-step algorithm .13 .27 .39 .51 220 330 440 550 .8 .3 .5 .2 .6 .4 1.2 1.7 (mod harmonics) .2 .12 -.23 .34))
(out kx 1) (out ky 2)
"""
        kernel = compile_source(directory, 'routing', kernel_source)
        phases = dict(c=.13,a=.27,b1=.39,b2=.51)
        gains = dict(c=.8,a=.3,b1=.5,b2=.2)
        old = dict(a=.12,b1=-.23,b2=.34)
        def wave(phase, h):
            pos = abs(h); lo = min(5,int(pos)); frac = pos-lo
            # Independent table oracle: sum the original coefficients at the
            # two surrounding phase samples, then linearly interpolate.
            index = (phase % 1) * TABLE_SAMPLES
            left = np.floor(index)
            def at(sample):
                return sum(((1-frac)*anchors(n)[lo]+frac*anchors(n)[lo+1])
                           * np.sin(2*np.pi*n*sample/TABLE_SAMPLES)
                           for n in range(1, PARTIALS+1))
            return at(left) + (index-left) * (at(left+1)-at(left))
        for i, graph in enumerate(ALGORITHMS,1):
            for harm in [-6,-2.5,0,2.5,6]:
                values = {}
                def evaluate(op):
                    if op in values: return values[op]
                    phase = phases[op] + (.2*old[op] if graph['feedback']==op else 0)
                    for start,end in graph['edges']:
                        if end==op:
                            phase += evaluate(start)*gains[start]*(.6*1.2 if start=='a' else .4*1.7)
                    color = -harm if op=='c' and harm<0 else (harm if op in ['a','b1'] and harm>0 else 0)
                    values[op] = np.sin(2*np.pi*phase) if op=='b2' else wave(phase,color)
                    return values[op]
                expected = [sum(evaluate(op)*gains[op]*((.6 if op=='a' else .4) if envelope else 1) for op,envelope in graph[bus]) for bus in ['x','y']]
                y,_ = kernel.render(.003,params={'algorithm':i,'harmonics':harm})
                np.testing.assert_allclose(y[-1],expected,atol=2e-5,rtol=2e-5,err_msg=f'algorithm {i}, harmonic {harm}')
        results['routing_spectrum_checks'] = '8 graphs × 5 bipolar spectra match independent graph/table reference'
        # All anchor spectra and both directions must survive high notes/feedback.
        for h in range(-6, 7):
            y, state = inst.render(.12, pitch=3500, params={'harmonics':h,'feedback':2,'source_db':12,'resonance':1})
            assert np.isfinite(y).all() and np.isfinite(state).all(), h
            assert np.max(np.abs(y)) <= 2
        transition_source = source[:source.index('(out (* signal')]+ '(out routing-gain 1)\n(out alg 2)\n'
        transition = compile_source(directory, 'transition', transition_source)
        y,_ = transition.render(.1,ramps={'algorithm':[(0,2),(.049,2),(.05,7)]})
        changes = np.flatnonzero(np.diff(y[:,1]))+1
        assert len(changes)==1
        assert y[changes[0]-1,0]==0 and y[changes[0],0]<.011
        assert y[-1,0]==1 and y[-1,1]==7
        base, _ = inst.render(.3, params={'volume_db':-12})
        quiet, _ = inst.render(.3, params={'volume_db':-18})
        np.testing.assert_allclose(quiet, base * 10**(-6/20), atol=1e-6)
        y, _ = inst.render(.4, gate_off=.1, params={'amp_release_ms':100})
        assert np.max(np.abs(y[-4800:])) < 1e-6, 'amp must end held timbre voice'
        # Observe the real shared envelope, with the rest of the DSP left intact.
        envelope = source[:source.index('(out (* signal')]+ '(out env_a 1)\n(out env_b 2)\n'
        env = compile_source(directory, 'envelope', envelope)
        for off in [.002, .03, .15]:
            y, _ = env.render(.3, gate_off=off, params={'a_attack_ms':10,'a_decay_ms':80,'a_end':.3})
            point = round(off*48000)
            np.testing.assert_array_equal(y[point:,0], np.full(len(y)-point,y[point-1,0]))
        y, _ = env.render(13, gate_off=.1)
        np.testing.assert_array_equal(y[4800:,0],np.full(len(y)-4800,y[4799,0]))
        y, _ = env.render(.3, gate_off=.1, params={'a_gated':1,'a_hold':0,'a_attack_ms':10,'a_decay_ms':80,'a_end':.3})
        np.testing.assert_allclose(y[600:4700,0],1,atol=1e-6)
        np.testing.assert_allclose(y[-4800:,0],.3,atol=1e-5)
        y, _ = env.render(.4, gate_off=.1, retrig=[.2], params={'a_attack_ms':10,'a_decay_ms':80,'a_end':.3})
        assert y[9600,0] < .01 and y[10080,0] > .98, 'retrigger must clear hold latch'
        results['envelope_checks'] = 'attack/decay/end hold, gated decay, retrigger, amp release passed'
        # Magnitude comparison to a 192 kHz host / 768 kHz internal reference.
        # Exclude feedback: its intentional one-substep delay changes with rate.
        core_source = source[:source.index('(out (* signal')]+ '(out lp3_4 1)\n'
        core = compile_source(directory, 'core48', core_source)
        reference = compile_source(directory, 'core192', core_source, sr=192000)
        errors = {}
        for name,pitch,depth,harm in [('sine',220,0,0),('keys',220,.5,0),('bright_high',1760,1,4)]:
            params={'feedback':0,'a_depth':depth,'b_depth':depth,'a_end':1,'b_end':1,'harmonics':harm}
            low,_ = core.render(1, pitch=pitch, params=params)
            high,_ = reference.render(1, pitch=pitch, params=params)
            # Steady half-second, equal 2 Hz bin spacing; compare below 10 kHz.
            a=np.abs(np.fft.rfft(low[24000:]))/24000
            b=np.abs(np.fft.rfft(high[96000:]))/96000
            errors[name]=float(np.linalg.norm(a[:5000]-b[:5000])/np.linalg.norm(b[:5000]))
        assert errors['sine'] < .001 and errors['keys'] < .05, errors
        results['high_rate_reference_relative_spectral_error_below_10khz'] = errors
        # Compare the refactored Drift to its unmodified tracked implementation.
        before = subprocess.check_output(['git','show','HEAD:content/instruments/Synths/Digi Drift/dsp.lisp'],cwd=ROOT,text=True)
        after = (ROOT/'content/instruments/Synths/Digi Drift/dsp.lisp').read_text()
        old = compile_source(directory, 'drift_before', before)
        new = compile_source(directory, 'drift_after', after)
        for filter_type in [0,1]:
            params={'drift':0,'filter_type':filter_type}
            a,_ = old.render(.3,params=params)
            b,_ = new.render(.3,params=params)
            np.testing.assert_allclose(a,b,atol=1e-7,rtol=1e-6)
        results['drift_filter_parity'] = 'both filter types passed'
        # Eight separate voice states, one render deadline; excludes host graph
        # scheduling but includes all oscillators, envelopes and both filters.
        ptr = ctypes.POINTER(ctypes.c_float)
        inputs = [np.zeros(128,np.float32) for _ in range(inst.n_in)]
        inputs[inst.inputs['gate']][:]=1; inputs[inst.inputs['velocity']][:]=1
        inputs[inst.inputs['pitch']][:]=220
        outputs=[np.zeros(128,np.float32) for _ in range(inst.n_out)]
        ip=(ptr*len(inputs))(*[v.ctypes.data_as(ptr) for v in inputs])
        op=(ptr*len(outputs))(*[v.ctypes.data_as(ptr) for v in outputs])
        voices=[inst.fresh_memory() for _ in range(8)]
        for memory in voices:
            memory[inst.params['harmonics']['cellId']]=5
        pointers=[v.ctypes.data_as(ctypes.c_void_p) for v in voices]
        context=ctypes.byref(inst.context)
        inputs[inst.inputs['trigger']][0]=1
        for memory in pointers: inst.process_fn(ip,op,128,memory,context,None)
        inputs[inst.inputs['trigger']][0]=0
        timings=[]
        for _ in range(5):
            start=time.perf_counter()
            for _ in range(200):
                for memory in pointers: inst.process_fn(ip,op,128,memory,context,None)
            timings.append((time.perf_counter()-start)/200)
        results['eight_voices_128_frames_median_ms']=float(np.median(timings)*1000)
        results['eight_voices_deadline_ms']=128/48000*1000
        t = time.perf_counter()
        inst.render(2,params={'harmonics':5,'algorithm':5,'feedback':1})
        results['render_seconds_per_audio_second_including_python'] = (time.perf_counter()-t)/2
    print(json.dumps(results,indent=2))

if __name__=='__main__': main()
