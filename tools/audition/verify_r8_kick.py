#!/usr/bin/env python3
"""Exercise the shipping R8 voice, its complete musical surface and presets.

Uses the compiled DSP, not a surrogate. See instrument-audition-harness.md
for compiler environment variables. Writes a preset reel and machine-readable
measurements; exits nonzero on a dead control, invalid preset, non-finite
signal, tail leak, or block-size dependence. No full Cargo suite is needed.
"""
import argparse
import json
from pathlib import Path
import struct
import subprocess
import sys

import numpy as np

from audition import Instrument

ROOT = Path(__file__).resolve().parents[2]
INSTRUMENT = ROOT / 'content/instruments/Drums/R8 Kick 03'


def float_wav(path, samples, sr):
    """Unclipped float WAV: validation artifacts must not hide overloads."""
    data = np.asarray(samples, dtype='<f4').tobytes()
    fmt = struct.pack('<HHIIHH', 3, 1, sr, sr*4, 4, 32)
    Path(path).write_bytes(b'RIFF'+struct.pack('<I', 36+len(data))+b'WAVEfmt '+struct.pack('<I',16)+fmt+b'data'+struct.pack('<I',len(data))+data)


def measure(y):
    assert np.isfinite(y).all(), 'non-finite audio'
    return {'peak': float(np.max(np.abs(y))), 'rms': float(np.sqrt(np.mean(y*y)))}


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--out', required=True)
    args = ap.parse_args()
    out = Path(args.out); out.mkdir(parents=True, exist_ok=True)
    presets = json.loads(INSTRUMENT.with_suffix('.presets').read_text())['presets']
    inst = Instrument(str(INSTRUMENT))
    subprocess.run([sys.executable, str(ROOT/'tools/audition/check_fusion.py'), str(Path(inst.build_dir)/'patch.c')], check=True)
    params = {k:v for k,v in inst.params.items() if not k.startswith('__')}
    assert len(params) == 24, f'unexpected surface: {list(params)}'
    for name,p in params.items():
        assert p['min'] <= p['default'] <= p['max'], name
    default,_ = inst.render(.7, pitch=261.63)
    result = {'default': measure(default), 'controls': {}, 'presets': {}, 'sampleRates': {}}
    assert .05 < result['default']['peak'] < 1.01, result['default']
    length = params['length']['default']/1000
    assert np.max(np.abs(default[int((length+.005)*48000):])) == 0, 'default leaks beyond ROM fade'
    silent,_ = inst.render(.3, pitch=261.63, vel=0)
    assert np.max(np.abs(silent)) == 0, 'zero velocity is not silent'
    float_wav(out/'instrument-full.wav', default, 48000)

    for name,p in params.items():
        # Keytracking is neutral at C4; dynamics is neutral at velocity 1.
        pitch = 523.26 if name == 'track' else 261.63
        velocity = .45 if name == 'dynamics' else 1.
        a,_ = inst.render(1.3, pitch=pitch, vel=velocity, params={name:p['min']})
        b,_ = inst.render(1.3, pitch=pitch, vel=velocity, params={name:p['max']})
        delta = float(np.max(np.abs(a-b)))
        assert delta > 1e-6, f'dead control {name}: {delta}'
        result['controls'][name] = {'min':measure(a), 'max':measure(b), 'maxDifference':delta}
        assert np.max(np.abs(a[-480:])) == 0 and np.max(np.abs(b[-480:])) == 0, f'{name}: tail leak'
    print('24/24 controls change the sound; all endpoints finite and tails silent', flush=True)

    reel = []
    seen = set()
    for preset in presets:
        name = preset['name']
        assert preset['id'] not in seen, f'duplicate preset {name}'
        seen.add(preset['id'])
        for key,value in preset['params'].items():
            assert key in params, f'{name}: unknown control {key}'
            assert params[key]['min'] <= value <= params[key]['max'], f'{name}: invalid {key}'
        assert preset.get('base_note_offset',0) == 0, 'tuning must not be applied twice'
        y,_ = inst.render(1.3,pitch=261.63,params=preset['params'])
        stats = measure(y)
        assert .01 < stats['peak'] <= 1.01, f'{name}: overload/silence {stats}'
        result['presets'][name] = stats
        float_wav(out/f'preset-{preset["id"].replace(" ","-")}.wav',y,48000)
        reel.extend([y,np.zeros(12000,dtype=np.float32)])
    float_wav(out/'presets.wav',np.concatenate(reel),48000)
    print(f'{len(presets)} presets valid with headroom', flush=True)

    for sr in (44100,48000,96000):
        voice = inst if sr == 48000 else Instrument(str(INSTRUMENT),sample_rate=sr)
        subprocess.run([sys.executable,str(ROOT/'tools/audition/check_fusion.py'),str(Path(voice.build_dir)/'patch.c')],check=True)
        y,_ = voice.render(.7,pitch=261.63)
        result['sampleRates'][sr] = measure(y)
        assert np.max(np.abs(y[int((length+.005)*sr):])) == 0
        preset_peaks = {}
        for preset in presets:
            audio,_ = voice.render(1.3,pitch=261.63,params=preset['params'])
            stats = measure(audio)
            assert .01 < stats['peak'] <= 1.0, f'{sr} Hz / {preset["name"]}: {stats}'
            preset_peaks[preset['name']] = stats['peak']
        result['sampleRates'][sr]['presetPeaks'] = preset_peaks
        extremes = {k:p['max'] for k,p in params.items()}
        # Exercise strongest excitation without saturation hiding a blowup.
        extremes.update(level=.5,drive=0,crush=0)
        # Each layer now stretches its own cut as well as its decay. The old
        # global 180 ms cut hid long DECAY/RING/CONTACT settings. Allow the
        # longest legal layer, the output filter exit, and a quiet margin.
        tail_seconds = extremes['length']/1000*max(extremes[k] for k in ('decay','ring','contact'))
        torture,_ = voice.render(.401+tail_seconds+.22,pitch=523.26,params=extremes,retrig=[.031,.137,.401],
                                ramps={'pitch':[(0.,65.4075),(.2,1046.52),(.8,261.63)]})
        result['sampleRates'][sr]['retriggerExtremes'] = measure(torture)
        assert np.max(np.abs(torture[-int(.1*sr):])) == 0, 'retrigger tail leak'
    print('44.1/48/96 kHz, pitch automation and retrigger extremes finite', flush=True)

    small = Instrument(str(INSTRUMENT),max_frames=64)
    large = Instrument(str(INSTRUMENT),max_frames=512)
    for voice in (small,large):
        subprocess.run([sys.executable,str(ROOT/'tools/audition/check_fusion.py'),str(Path(voice.build_dir)/'patch.c')],check=True)
    a,_ = small.render(.7,pitch=261.63,retrig=[.039,.151,.303])
    b,_ = large.render(.7,pitch=261.63,retrig=[.039,.151,.303])
    delta = float(np.max(np.abs(a-b)))
    assert delta < 1e-6, f'block-size dependence: {delta}'
    result['blockSizeMaxDifference'] = delta
    (out/'validation.json').write_text(json.dumps(result,indent=2)+'\n')
    print(f'64/512-frame equivalence: {delta:.3g}; wrote {out}/validation.json',flush=True)


if __name__ == '__main__':
    main()
