#!/usr/bin/env python3
"""Render Modal Kick presets and verify its clean/drive and tuning contracts.

Uses the pinned compiler when ESEQ_DGENLISP_TOOL is set. The host
instrument_probe separately validates production macro resolution. Float WAVs
preserve overloads; measurements do not constitute a subjective sound verdict.
"""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile

import numpy as np

from audition import Instrument
from verify_r8_kick import float_wav, measure

ROOT = Path(__file__).resolve().parents[2]
VOICE = ROOT / 'content/instruments/Drums/Modal Kick'


def expanded_source():
    names = ('heat-soft-clip', 'heat-drive')
    source = '\n'.join((ROOT / 'content/defmacros' / name / 'macro.lisp').read_text()
                       for name in names) + '\n' + (VOICE / 'dsp.lisp').read_text()
    for name in names:
        source = source.replace(f'(use-defmacro {name})', '')
    return source


def compile_voice(directory, name, source, **options):
    path = directory / f'{name}.lisp'
    path.write_text(source)
    voice = Instrument(path, **options)
    subprocess.run([sys.executable, str(ROOT / 'tools/audition/check_fusion.py'),
                    str(Path(voice.build_dir) / 'patch.c')], check=True)
    return voice


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    source = expanded_source()
    presets = json.loads(VOICE.with_suffix('.presets').read_text())['presets']
    results = {'presets': {}, 'drives': {}, 'sample_rates': {}}
    with tempfile.TemporaryDirectory(prefix='modal-kick-check-') as temporary:
        directory = Path(temporary)
        voice = compile_voice(directory, 'voice', source)
        results['compiler_sha256'] = voice.compiler_sha256
        reel = []
        for preset in presets:
            # Host modulator settings are not parameters of the compiled voice.
            params = {}
            for name, value in preset['params'].items():
                if name not in voice.params:
                    assert name.startswith(('mod1_', 'mod2_', 'mod3_', 'mod4_')), name
                    continue
                meta = voice.params[name]
                assert meta['min'] <= value <= meta['max'], (preset['name'], name, value)
                params[name] = value
            stats = {}
            for pitch in (220, 440, 880):
                audio, _ = voice.render(1.5, pitch=pitch, params=params)
                stats[pitch] = measure(audio)
                assert .001 < stats[pitch]['peak'] < 1, (preset['name'], pitch, stats[pitch])
                if pitch == 440:
                    float_wav(args.out / f"{preset['id'].replace(' ', '-')}.wav", audio, 48000)
                    reel.extend((audio, np.zeros(12000, dtype=np.float32)))
            results['presets'][preset['name']] = stats
        float_wav(args.out / 'presets.wav', np.concatenate(reel), 48000)
        print('All presets valid and below full scale over two octaves', flush=True)

        # Compare with an independently wired clean route through the SAME
        # downstream tone/punch/DC stages, so saturation cannot hide in bypass.
        clean_source = source.replace('(def driven (mix punched drive-wet drive-v))',
                                      '(def driven punched)')
        assert clean_source != source
        clean = compile_voice(directory, 'clean', clean_source)
        params = {'click': 0, 'bank': 0}
        dry, _ = clean.render(.6, pitch=440, params=params)
        drive_reel = []
        for mode in range(7):
            bypass, _ = voice.render(.6, pitch=440, params=dict(params, drive=0, drive_mode=mode))
            assert np.max(abs(bypass - dry)) < 1e-6, ('zero-drive bypass', mode)
            audio, _ = voice.render(.6, pitch=440, params=dict(params, drive=.8, drive_mode=mode))
            results['drives'][mode] = measure(audio)
            if mode == 0:
                assert np.max(abs(audio - dry)) < 1e-6, 'Off must be clean at every amount'
            else:
                assert np.max(abs(audio - dry)) > .005, ('inaudible drive', mode)
            drive_reel.extend((audio, np.zeros(9600, dtype=np.float32)))
        float_wav(args.out / 'drive-options.wav', np.concatenate(drive_reel), 48000)

        # Isolate the actual shell output, including its excitation and decay.
        shell = compile_voice(directory, 'shell', source.replace(
            '(out toned-out 1 @name audio)', '(out shell-sig 1 @name audio)'))
        def shell_peak(pitch, tune=-36, offset=24):
            audio, _ = shell.render(.3, pitch=pitch, params={
                'tune': tune, 'shell_pitch': offset, 'shell_decay': 400})
            segment = audio[480:]
            spectrum = abs(np.fft.rfft(segment * np.hanning(len(segment)), n=131072))
            return np.argmax(spectrum) * 48000 / 131072
        frequencies = [shell_peak(220), shell_peak(440), shell_peak(880), shell_peak(440, -24)]
        assert np.allclose(frequencies, [110, 220, 440, 440], rtol=.02), frequencies
        results['shell_frequencies_hz'] = frequencies
        print('True clean bypass, six active drives, shell note/Tune tracking pass', flush=True)

        for sr in (44100, 48000, 96000):
            inst = voice if sr == 48000 else compile_voice(directory, f'voice-{sr}', source, sample_rate=sr)
            audio, _ = inst.render(1, pitch=440, params={'drive': .8, 'drive_mode': 6}, retrig=[.08, .2])
            results['sample_rates'][sr] = measure(audio)
            assert results['sample_rates'][sr]['peak'] < 1
            zero, _ = inst.render(.2, pitch=440, vel=0)
            assert np.max(abs(zero)) == 0
        # Contact-noise is intentionally excluded from exact block comparisons.
        small = compile_voice(directory, 'small', source, max_frames=64)
        large = compile_voice(directory, 'large', source, max_frames=512)
        params = {'click': 0, 'drive': .8, 'drive_mode': 6}
        a, _ = small.render(.6, pitch=440, params=params, retrig=[.039, .151, .303])
        b, _ = large.render(.6, pitch=440, params=params, retrig=[.039, .151, .303])
        delta = float(np.max(abs(a-b)))
        assert delta < 1e-6, ('block-size dependence', delta)
        results['block_size_max_difference'] = delta
        print('44.1/48/96 kHz retriggers, silence and 64/512-frame equivalence pass', flush=True)
    (args.out / 'validation.json').write_text(json.dumps(results, indent=2) + '\n')


if __name__ == '__main__':
    main()
