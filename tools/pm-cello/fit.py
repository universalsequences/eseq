#!/usr/bin/env python3
"""Fit physical controls of the same compiled cello to each reference gesture.

Uses harmonic and amplitude trajectories, not phase matching or PCM playback.
Writes candidate reports; factory presets are reviewed and installed separately.
"""
import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from scipy.optimize import differential_evolution, minimize
from scipy.signal import resample_poly
import soundfile as sf

from analyze import HERE, ROOT, harmonics, rms_envelope
from common import SOURCE, instrument, write_wav


def configuration(key):
    body = {'body.size': 1, 'expression.vib_cent': 0, 'string.stiffness': 0,
            'amp.decay': 100, 'amp.sustain': 1, 'pluck.strength': 0, 'section.width': .7}
    if key == 'crescendo':
        fixed = {**body, 'bow.amount': 1, 'body.high': 1, 'section.blend': 0}
        ranges = {
            'bow.pressure': (.08, .85), 'bow.speed': (.1, .5), 'bow.rosin': (2, 6), 'bow.noise': (.002, .12),
            'string.position': (.08, .4), 'string.damping_hz': (2200, 13000), 'string.decay_s': (1.5, 4),
            'amp.attack': (1500, 3800), 'bow.stroke_ms': (3150, 3650), 'amp.release': (10, 220),
            'body.wood': (.7, 1), 'body.resonance': (1, 7), 'body.low_hz': (80, 150), 'body.low': (0, 1),
            'body.mid_hz': (175, 300), 'body.mid': (0, 1), 'body.high_hz': (480, 650),
            'body.air_hz': (700, 1800), 'body.air': (0, 1), 'body.tone_hz': (5000, 16000),
        }
        seed = {'bow.stroke_ms': 3370, 'amp.attack': 2800, 'string.decay_s': 2.7, 'body.wood': .9}
        duration = 4.6
    elif key == 'pizzicato':
        fixed = {**body, 'bow.amount': 0, 'pluck.strength': 1, 'body.low': 1}
        ranges = {
            'pluck.width_ms': (.3, 12), 'pluck.texture': (0, .5), 'string.position': (.28, .5),
            'string.damping_hz': (500, 6000), 'string.decay_s': (1, 5),
            'body.wood': (.6, 1), 'body.resonance': (.5, 5), 'body.low_hz': (70, 170),
            'body.mid_hz': (180, 360), 'body.mid': (0, 1), 'body.high_hz': (280, 900), 'body.high': (0, 1),
            'body.air_hz': (650, 3000), 'body.air': (0, .5), 'body.tone_hz': (700, 5000),
            'section.blend': (.4, 1), 'section.spread': (0, 35), 'section.lag_ms': (0, 35),
        }
        seed = {'pluck.width_ms': 3, 'string.position': .47, 'string.damping_hz': 1800,
                'string.decay_s': 2.5, 'section.blend': .8, 'section.spread': 15, 'section.lag_ms': 15,
                'body.wood': .9, 'body.mid': .3, 'body.high': .1, 'body.air': .02, 'body.tone_hz': 2000}
        duration = 1.6
    else:
        fixed = {**body, 'bow.amount': 1, 'body.mid': 1, 'body.low': .1}
        ranges = {
            'bow.pressure': (.08, .8), 'bow.speed': (.08, .6), 'bow.rosin': (1.5, 6), 'bow.noise': (0, .2),
            'bow.stroke_ms': (170, 370), 'amp.attack': (20, 200), 'amp.release': (10, 180),
            'string.position': (.08, .4), 'string.damping_hz': (700, 8000), 'string.decay_s': (.5, 2.5),
            'body.wood': (.5, 1), 'body.resonance': (.5, 5), 'body.mid_hz': (260, 500),
            'body.high_hz': (550, 1500), 'body.high': (0, 1), 'body.air_hz': (1200, 3200), 'body.air': (0, 1),
            'body.tone_hz': (600, 5000), 'section.blend': (.4, 1), 'section.spread': (0, 35), 'section.lag_ms': (0, 35),
        }
        seed = {'bow.speed': .4, 'bow.stroke_ms': 245, 'amp.attack': 75, 'amp.release': 40,
                'string.decay_s': 1.3, 'body.mid_hz': 389, 'body.wood': .8, 'body.high': .25,
                'body.air': .1, 'body.tone_hz': 2000, 'section.blend': .8, 'section.spread': 15, 'section.lag_ms': 12}
        duration = 1.2
    return fixed, ranges, seed, duration


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('reference', choices=['crescendo', 'pizzicato', 'spiccato'])
    parser.add_argument('--iterations', type=int, default=32)
    parser.add_argument('--seed', type=int, default=23)
    parser.add_argument('--resume', type=Path)
    parser.add_argument('--local', action='store_true', help='Refine the supplied seed with bounded Powell search')
    parser.add_argument('--radius', type=float, default=1,
                        help='Search radius around the seed, as a fraction of each control range')
    args = parser.parse_args()
    reference = json.loads((HERE / 'reference-analysis.json').read_text())[args.reference]
    fixed, ranges, seed, duration = configuration(args.reference)
    inst = instrument()
    source_sha256 = hashlib.sha256(SOURCE.read_bytes()).hexdigest()
    defaults = {name: p['default'] for name, p in inst.params.items() if not name.startswith('__')}
    recorded, sr = sf.read(ROOT / 'samples-to-analyze' / reference['file'])
    if sr != 48000:
        divisor = np.gcd(sr, 48000)
        recorded = resample_poly(recorded, 48000 // divisor, sr // divisor, axis=0)
    recorded = recorded[:int(duration * 48000)]
    target_env = rms_envelope(recorded, 48000)
    target_peak = max(target_env)
    target_spectra = np.maximum(reference['harmonics_db'], -50)
    weights = np.array([3, 3, 2, 2, 2, 1.5, 1.5, 1, 1, 1, .8, .8] + [.4] * 12)
    windows = reference['analysis_windows_seconds']
    # Ensemble beating makes short-window pitch tracks wander. Render at the
    # strongest fundamental peak over the attack/sustain window.
    pitch = reference['near_fundamental_peaks'][0][0]
    names = list(ranges)
    bounds = list(ranges.values())
    if args.resume:
        seed.update(json.loads(args.resume.read_text())['best']['params'])
    seed_values = [float(np.clip(seed.get(name, defaults[name]), low + 1e-6, high - 1e-6))
                   for name, (low, high) in ranges.items()]
    if not 0 < args.radius <= 1:
        parser.error('--radius must be in (0, 1]')
    bounds = [(max(low, value - args.radius * (high - low)),
               min(high, value + args.radius * (high - low)))
              for value, (low, high) in zip(seed_values, bounds)]
    best = [float('inf'), None]
    evaluations = [0]
    output = HERE / 'output'
    output.mkdir(exist_ok=True)

    def objective(values):
        params = {**defaults, **fixed, **dict(zip(names, values)), 'gain': 1}
        y, state = inst.render(duration, pitch=pitch, params=params)
        evaluations[0] += 1
        if not np.all(np.isfinite(y)) or not np.all(np.isfinite(state)):
            raise RuntimeError('Non-finite cello during voicing')
        if np.max(abs(y)) > 4:
            return 10000
        env = rms_envelope(y, 48000)
        if max(env) < 1e-6:
            return 10000
        scale = float(np.clip(np.dot(env, target_env) / max(np.dot(env, env), 1e-20), .001, 1))
        actual_env = env * scale
        floor = target_peak * .025
        envelope_db = 20 * np.log10((actual_env + floor) / (target_env + floor))
        envelope_mse = float(np.average(envelope_db ** 2, weights=.2 + target_env / target_peak))
        measured = np.array([harmonics(y, 48000, pitch, w) for w in windows])
        spectral_mse = float(np.mean(np.average((np.maximum(measured, -50) - target_spectra) ** 2,
                                               weights=weights, axis=1)))
        loss = .65 * spectral_mse + .35 * envelope_mse
        if loss < best[0]:
            params['gain'] = scale
            best[:] = [loss, {'params': params, 'harmonics_db': measured.tolist(),
                             'spectral_mse': spectral_mse, 'envelope_db_mse': envelope_mse,
                             'envelope_normalized_rmse': float(np.sqrt(np.mean((actual_env-target_env)**2)) / target_peak),
                             'peak_rms_seconds': float(np.argmax(actual_env)*.02)}]
        return loss

    def report(message):
        data = {'reference': args.reference, 'reference_sha256': reference['sha256'],
                'source_sha256': source_sha256,
                'compiler_sha256': inst.compiler_sha256, 'pitch_hz': pitch, 'duration': duration,
                'evaluations': evaluations[0], 'initial_loss': baseline, 'best_loss': best[0],
                'best': best[1], 'optimizer_message': message}
        (output / (args.reference + '-fit.json')).write_text(json.dumps(data, indent=2) + '\n')

    baseline = objective(seed_values)
    print(args.reference, 'initial', baseline, flush=True)

    def checkpoint(values, convergence):
        print(args.reference, evaluations[0], 'loss', round(best[0], 3),
              'spectral', round(best[1]['spectral_mse'], 3), 'envelope', round(best[1]['envelope_db_mse'], 3), flush=True)
        report('Search in progress')

    if args.local:
        # Unit coordinates keep Hz and normalized physical controls equally
        # represented in the bounded local search.
        low, high = np.array(bounds).T
        result = minimize(lambda x: objective(low + x * (high - low)),
                          (np.array(seed_values) - low) / (high - low), method='Powell',
                          bounds=[(0, 1)] * len(names),
                          options={'maxfev': args.iterations * 100, 'xtol': .001, 'ftol': .001},
                          callback=lambda x: checkpoint(x, None))
    else:
        result = differential_evolution(objective, bounds, seed=args.seed, x0=seed_values,
                                        maxiter=args.iterations, popsize=5, polish=False, workers=1,
                                        callback=checkpoint)
    report(str(result.message))
    y, _ = inst.render(reference['seconds'], pitch=pitch, params=best[1]['params'])
    write_wav(str(output / (args.reference + '-candidate.wav')), y, 48000)
    print(args.reference, 'finished', best[0], flush=True)


if __name__ == '__main__':
    main()
