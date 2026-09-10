#!/usr/bin/env python3
"""Verify the corrected local banks across chip-clock rates and host rates.

The probe excites and reads the production filter states. It bypasses the
nonlinear input/output stages for frequency measurement, without substituting
a filter model. The full instrument separately checks low-resonance stability.
"""
import argparse
import json
from pathlib import Path
import tempfile

import numpy as np

from verify_modal_kick import compile_voice, expanded_source


def measured_peak(audio, sr, start=.4):
    signal = audio[round(start * sr):]
    spectrum = abs(np.fft.rfft(signal, n=524288))
    return float(np.argmax(spectrum) * sr / 524288)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--instrument', choices=('modal-kick', '808-tom', '808-clap'), default='modal-kick')
    args = parser.parse_args()
    instrument_name = {'808-tom': '808 Tom', '808-clap': '808 Clap'}.get(args.instrument)
    source = (expanded_source() if args.instrument == 'modal-kick' else
              (Path(__file__).resolve().parents[2] / 'content/instruments/Drums' / instrument_name / 'dsp.lisp').read_text())
    excitation = '(def xin (+ x thump))'
    output = '(mix sig wet_recon wet_amt))'
    assert source.count(excitation) == source.count(output) == 1
    # A short pulse spans multiple chip-clock events even at low cutoff;
    # a one-host-sample impulse can fall entirely between those events.
    probe = source.replace(excitation, '(def xin (* (id-env triggered 1) 0.0001))')
    report = {'instrument': args.instrument, 'tuning': [], 'stability': [], 'sub_bass': []}
    with tempfile.TemporaryDirectory(prefix='modal-bank-clock-') as temporary:
        directory = Path(temporary)
        for sr in (44100, 48000, 96000):
            voice = compile_voice(directory, f'frequency-{sr}', probe.replace(output, 'f1)'), sample_rate=sr)
            for cutoff in (0, .2, .5, .8, 1):
                target = 30 * np.exp(5.586 * cutoff)
                for crush in (0, .5, 1):
                    audio, _ = voice.render(1.2, pitch=440, params={
                        'bank': 1, 'bank_track': 0, 'bank_freq': cutoff, 'bank_env': 0,
                        'bank_crunch': crush, 'bank_res': .85}, retrig=[.4])
                    actual = measured_peak(audio, sr)
                    # This is a nonlinear, damped resonator rather than an
                    # ideal oscillator. Bound its spectral peak to 6% of fc.
                    assert abs(actual / target - 1) < .06, (sr, cutoff, crush, actual, target)
                    report['tuning'].append(dict(sr=sr, cutoff=cutoff, crush=crush,
                                                 target_hz=target, peak_hz=actual))
            full = compile_voice(directory, f'full-{sr}', source, sample_rate=sr)
            for crush in (0, .5, 1):
                audio, memory = full.render(2, pitch=440, params={
                    'bank': 1, 'bank_track': 0, 'bank_freq': 1, 'bank_env': 0,
                    'bank_crunch': crush, 'bank_res': 0, 'bank_harm': 0})
                assert np.isfinite(audio).all() and np.isfinite(memory).all()
                tail = float(np.sqrt(np.mean(audio[-round(.1 * sr):] ** 2)))
                assert tail < .0001, ('unwanted oscillation at zero resonance', sr, crush, tail)
                report['stability'].append(dict(sr=sr, crush=crush, tail_rms=tail))
            # F2 remains free to resonate below the fundamental: no new
            # high-pass, minimum cutoff, or low-frequency damping is allowed.
            second = compile_voice(directory, f'sub-{sr}', probe.replace(output, 'f2)'), sample_rate=sr)
            audio, _ = second.render(2.4, pitch=440, params={
                'bank': 1, 'bank_track': 0, 'bank_freq': .2, 'bank_env': 0,
                'bank_crunch': 1, 'bank_res': .85, 'bank_harm': 5.5}, retrig=[.4])
            actual = measured_peak(audio, sr)
            target = 30 * np.exp(5.586 * .2) / 4.5
            assert abs(actual / target - 1) < .08, ('divided sub-bass resonance', sr, actual, target)
            report['sub_bass'].append(dict(sr=sr, target_hz=target, peak_hz=actual))
            print(f'{sr} Hz: cutoff tracking, high-cutoff stability and divided sub-bass pass', flush=True)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
