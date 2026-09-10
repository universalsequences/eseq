#!/usr/bin/env python3
"""Exercise the compiled shared cello: gestures, controls, host events and graph save."""
import argparse
import ctypes
import hashlib
import itertools
import json
import platform
from pathlib import Path

import numpy as np
from scipy.signal import resample_poly
import soundfile as sf

from analyze import HERE, ROOT, harmonics, mono, pitch_track, rms_envelope
from common import SOURCE, instrument, write_wav

FACTORY = SOURCE.parent.parent
BANK = FACTORY / 'PM Cello.presets'


def rms(y):
    return float(np.sqrt(np.mean(np.asarray(y, dtype=np.float64) ** 2)))


def passive_input(inst, params, gates):
    """No trigger pulses: exercise the real gate input and fresh silent state."""
    n = int(inst.sample_rate * 1.2)
    state = inst.fresh_memory()
    for name, value in params.items():
        state[inst.params[name]['cellId']] = value
    inputs = [np.zeros(n, dtype=np.float32) for _ in range(inst.n_in)]
    outputs = [np.zeros(n, dtype=np.float32) for _ in range(inst.n_out)]
    inputs[inst.inputs['pitch']][:] = 110
    inputs[inst.inputs['velocity']][:] = 1
    for start, end in gates:
        inputs[inst.inputs['gate']][int(start*inst.sample_rate):int(end*inst.sample_rate)] = 1
    block = inst.max_frames
    for offset in range(0, n, block):
        count = min(block, n-offset)
        def pointers(arrays):
            return (ctypes.POINTER(ctypes.c_float) * len(arrays))(*[
                a[offset:].ctypes.data_as(ctypes.POINTER(ctypes.c_float)) for a in arrays])
        inst.process_fn(pointers(inputs), pointers(outputs), count,
                        state.ctypes.data_as(ctypes.c_void_p), ctypes.byref(inst.context), None)
    return np.array(outputs).T, state


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--roundtrip', type=Path, required=True)
    args = parser.parse_args()
    out = HERE / 'output'
    out.mkdir(exist_ok=True)
    checks = []
    builds = set()

    def compiled(path=SOURCE, sr=48000, block=128):
        inst = instrument(path, sr, block)
        builds.add(str(inst.build_dir))
        return inst

    def check_audio(label, y, state):
        assert np.all(np.isfinite(y)) and np.all(np.isfinite(state)), (label, 'non-finite audio/state')
        peak = float(np.max(abs(y)))
        assert peak < 4, (label, 'excessive output', peak)
        checks.append({'case': label, 'peak': peak, 'rms': rms(y)})
        return y

    def render(inst, label, seconds=2, pitch=110, **kwargs):
        y, state = inst.render(seconds, pitch=pitch, **kwargs)
        return check_audio(label, y, state)

    inst = compiled()
    params = {name: p for name, p in inst.params.items() if not name.startswith('__')}
    defaults = {name: p['default'] for name, p in params.items()}
    presets = json.loads(BANK.read_text())['presets']
    by_id = {p['id']: p['params'] for p in presets}
    assert len(by_id) == len(presets)
    assert set(by_id) >= {'reference-crescendo', 'reference-pizzicato', 'reference-spiccato'}
    # Manifest values and the host parameter cells use Float32. The preset JSON
    # retains decimal authoring precision; compare the actual recalled values.
    assert set(by_id['reference-crescendo']) == set(defaults)
    assert all(np.float32(by_id['reference-crescendo'][name]) == np.float32(value)
               for name,value in defaults.items()), 'default must recall the crescendo voicing'
    destinations = {d['name'] for d in inst.manifest['modDestinations']}
    expected_mods = set(params) - {'amp.attack', 'amp.decay', 'amp.sustain', 'amp.release',
                                  'bow.stroke_ms', 'expression.vib_wait', 'expression.vel_bow'}
    assert destinations == expected_mods, destinations ^ expected_mods
    references = json.loads((HERE / 'reference-analysis.json').read_text())
    comparisons = {}
    for preset in presets:
        assert set(preset['params']) == set(params), preset['id']
        for name, value in preset['params'].items():
            assert params[name]['min'] <= value <= params[name]['max'], (preset['id'], name)
        for hz in [65.406, 110, 220, 389.616, 880]:
            y = render(inst, f"{preset['id']} {hz} Hz", seconds=5.5, pitch=hz, params=preset['params'], gate_off=3.6)
            assert rms(y) > .0005, (preset['id'], hz, 'silent articulation')
        if not preset['id'].startswith('reference-'):
            y=render(inst,preset['id']+' preview',seconds=4,pitch=110,params=preset['params'],gate_off=3)
            write_wav(str(out/(preset['id']+'.wav')),y,48000)
            continue
        key = preset['id'].removeprefix('reference-')
        ref = references[key]
        hz = ref['near_fundamental_peaks'][0][0]
        y = render(inst, key + ' comparison', seconds=ref['seconds'], pitch=hz, params=preset['params'])
        recorded, sr = sf.read(ROOT / 'samples-to-analyze' / ref['file'])
        assert hashlib.sha256((ROOT / 'samples-to-analyze' / ref['file']).read_bytes()).hexdigest() == ref['sha256']
        if sr != 48000:
            divisor = np.gcd(sr, 48000)
            recorded = resample_poly(recorded, 48000//divisor, sr//divisor, axis=0)
        n = min(len(y), len(recorded))
        actual_env, target_env = rms_envelope(y[:n], 48000), rms_envelope(recorded[:n], 48000)
        env_error = float(np.sqrt(np.mean((actual_env-target_env)**2)) / max(target_env))
        measured = np.array([harmonics(y, 48000, hz, w) for w in ref['analysis_windows_seconds']])
        weights = np.array([3,3,2,2,2,1.5,1.5,1,1,1,.8,.8]+[.4]*12)
        spectral_mse = float(np.mean(np.average((np.maximum(measured,-50)-np.maximum(ref['harmonics_db'],-50))**2,
                                               weights=weights,axis=1)))
        assert env_error < .15, (key, 'amplitude trajectory regression', env_error)
        assert spectral_mse < {'crescendo':45, 'pizzicato':22, 'spiccato':12}[key], (key, spectral_mse)
        comparisons[key] = {'pitch_hz':hz, 'reference_peak_seconds':float(np.argmax(target_env)*.02),
            'model_peak_seconds':float(np.argmax(actual_env)*.02), 'envelope_normalized_rmse':env_error,
            'weighted_harmonic_rmse_db':float(np.sqrt(spectral_mse)), 'harmonics_db':measured.tolist()}
        write_wav(str(out / (key + '.wav')), y, 48000)
        # A/B preserves recorded levels. First reference, 0.5 s silence, then synth.
        reference_stereo = np.repeat(recorded[:,None], 2, axis=1) if recorded.ndim==1 else recorded
        pair = np.concatenate([reference_stereo, np.zeros((24000,2)), y])
        write_wav(str(out / (key + '-ab.wav')), pair, 48000)
    print('Reference trajectories, preset recall and registers passed', flush=True)

    bow = {**defaults, 'amp.attack':30, 'amp.decay':200, 'amp.sustain':.8, 'amp.release':40,
           'bow.stroke_ms':0, 'section.blend':0, 'expression.vib_cent':0}
    pluck = {**by_id['reference-pizzicato'], 'section.blend':0, 'pluck.texture':0}
    for sr in [44100,48000,96000]:
        current = compiled(sr=sr)
        for kind, base in [('bow',bow),('pluck',pluck)]:
            for hz in [65.406,110,220,389.616,880]:
                y = render(current, f'tuning {kind} {sr}/{hz}', pitch=hz, params=base, gate_off=1.2)
                t, hz_track = pitch_track(y,sr,hz)
                # High, heavily damped plucks can finish in a few tenths of a
                # second. Measuring their numerical noise floor is not pitch.
                env=rms_envelope(y,sr)
                level=np.interp(t,np.arange(len(env))*.02+.01,env)
                use=(t>(.06 if kind=='pluck' else .2)) & (t<(.5 if kind=='pluck' else .9))
                use &= level>max(1e-7,float(max(env))*1e-4)
                assert np.count_nonzero(use)>=3,(kind,sr,hz,'insufficient voiced pitch frames')
                estimate = float(np.median(hz_track[use]))
                cents = float(1200*np.log2(estimate/hz))
                assert abs(cents)<25, (kind,sr,hz,cents)
                checks[-1]['pitch_error_cents'] = cents
    print('Bowed and plucked tuning at 44.1/48/96 kHz passed', flush=True)

    for hz in [65.406,389.616]:
        for name, p in params.items():
            base = pluck if name.startswith('pluck.') else bow
            for edge in ['min','max']:
                render(inst, f'{hz} {name} {edge}', pitch=hz, params={**base,name:p[edge]}, gate_off=1.2)
        print('Control extremes passed at',hz,'Hz',flush=True)
    corners = ['bow.pressure','bow.speed','bow.rosin','string.position','string.damping_hz','string.stiffness']
    for edges in itertools.product(['min','max'],repeat=len(corners)):
        values={**bow,'string.decay_s':12,'pluck.strength':.3,
                **{name:params[name][edge] for name,edge in zip(corners,edges)}}
        render(inst,'joint extremes '+ '/'.join(edges),seconds=3,params=values,gate_off=2)
    for name,p in params.items():
        render(inst,name+' automated/retrigger',params={**bow,'pluck.strength':.25,'section.blend':.7},
               ramps={name:[(0,p['min']),(.5,p['max']),(1,p['min']),(1.5,p['max'])]},
               gate_off=1.7,retrig=[.55,1.15])
    print('Joint extremes, automation and retriggers passed',flush=True)

    # Relevant gestures/motion remain active for dependent controls.
    expressive={**bow,'expression.vib_cent':18,'section.blend':.7,'section.spread':15}
    for name,p in params.items():
        base=pluck if name.startswith('pluck.') else expressive
        lo=render(inst,name+' minimum response',params={**base,name:p['min']},gate_off=1.2,vel=.7)
        # Zero follows the gate; a 6 s limit also follows this shorter test
        # note. Use an actual short stroke to exercise automatic bow lift.
        high=250 if name=='bow.stroke_ms' else p['max']
        hi=render(inst,name+' alternate response',params={**base,name:high},gate_off=1.2,vel=.7)
        assert rms(lo-hi)>1e-5,(name,'ineffective control')
    normal=render(inst,'normal gain',params=bow)
    half=render(inst,'half gain',params={**bow,'gain':bow['gain']*.5})
    mute=render(inst,'mute gain',params={**bow,'gain':0})
    silent=render(inst,'mute velocity',params=bow,vel=0)
    assert np.max(abs(half-normal*.5))<2e-5 and np.max(abs(mute))==0 and np.max(abs(silent))==0
    no_force=render(inst,'no bow or pluck',params={**bow,'bow.amount':0,'pluck.strength':0})
    assert np.max(abs(no_force))==0
    centered=render(inst,'center pluck travel and harmonic nodes',seconds=.5,pitch=98,params={
        **pluck,'string.position':.5,'string.damping_hz':12000,'string.decay_s':3,
        'body.wood':0,'body.tone_hz':16000,'pluck.width_ms':1,'pluck.strength':1})
    first=int(np.flatnonzero(np.max(abs(centered),axis=1)>1e-8)[0])
    assert 48000/196-3 <= first <= 48000/196+4, ('contact-to-bridge travel time',first)
    partials=harmonics(centered,48000,98,(.02,.4),count=8)
    assert partials[1]-partials[0]<-25 and partials[2]-partials[0]>-20, ('center pluck harmonic nodes',partials)
    checks[-1].update(first_audio_sample=first,harmonics_db=partials.tolist())
    for parameter in ['bow.pressure','string.stiffness','body.size','section.blend','gain']:
        y=render(inst,parameter+' host modulation',params={**bow,'__mod__'+parameter+'__active':1,
            '__mod__'+parameter+'__depth__slot1':.5},ramps={'mod1':[(0,1)]})
        assert rms(y-normal)>1e-4,(parameter,'modulation')
    for base in [bow,pluck]:
        y,state=passive_input(inst,base,[])
        check_audio('idle voice silence',y,state)
        assert np.max(abs(y))==0,'fresh voice sounded without a gate or trigger'
    y,state=passive_input(inst,{**pluck,'string.decay_s':.15},[(.2,.3),(.7,.8)])
    check_audio('pluck from gate rises without trigger',y,state)
    assert np.max(abs(y[:9600]))==0 and rms(y[10080:12480])>.001 and rms(y[34080:36480])>.001
    for base in [bow,pluck]:
        y=render(inst,'mono center player',params={**base,'section.blend':0})
        assert np.max(abs(y[:,0]-y[:,1]))<2e-6
        y=render(inst,'mono section width',params={**base,'section.blend':1,'section.width':0})
        assert np.max(abs(y[:,0]-y[:,1]))<2e-6
        y=render(inst,'stereo section',params={**base,'section.blend':1,'section.width':1})
        assert rms(y[:,0]-y[:,1])>.001
    print('Controls, silence, gate-only notes, stereo and host modulation passed',flush=True)

    for key in ['reference-crescendo','reference-pizzicato','reference-spiccato']:
        a=render(inst,key+' partition reference',seconds=4,params=by_id[key],gate_off=3.4,retrig=[1.1])
        for block in [32,64,256]:
            b=render(compiled(block=block),f'{key} partition {block}',seconds=4,params=by_id[key],gate_off=3.4,retrig=[1.1])
            delta=float(np.max(abs(a-b)))
            assert delta<1e-4,(key,block,delta)
            checks[-1]['partition_max_error']=delta
    saved=compiled(args.roundtrip.resolve()/'PM Cello/dsp.lisp')
    for preset in presets:
        a=render(inst,preset['id']+' authored source',seconds=4,params=preset['params'],gate_off=3.4)
        b=render(saved,preset['id']+' graph save',seconds=4,params=preset['params'],gate_off=3.4)
        delta=float(np.max(abs(a-b)))
        assert delta<3e-4,(preset['id'],'graph save changes audio',delta)
        checks[-1]['roundtrip_max_error']=delta
    release_path=Path(inst.compiler).resolve().parent/'RELEASE.json'
    report={'platform':platform.platform(),'compiler_sha256':inst.compiler_sha256,
            'compiler_release':json.loads(release_path.read_text()) if release_path.exists() else None,
            'source_sha256':hashlib.sha256(SOURCE.read_bytes()).hexdigest(),
            'presets_sha256':hashlib.sha256(BANK.read_bytes()).hexdigest(),
            'reference_sha256':{key:value['sha256'] for key,value in references.items()},
            'parameter_count':len(params),'modulation_destination_count':len(destinations),
            'fusion_checked_builds':len(builds),'comparisons':comparisons,'checks':checks}
    (HERE/'validation.json').write_text(json.dumps(report,indent=2)+'\n')
    print('PASS:',len(checks),'audio/state renders',flush=True)


if __name__=='__main__':
    main()
