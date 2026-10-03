#!/usr/bin/env python3
"""Generate PM Milagre Brass (dsp.lisp, ui.lisp, instrument.json, presets)
from fit.json. engine.lisp.in is the implementation; this fills in the
identified bore (mode ratios and peak heights), the body response and the
fitted defaults.

Usage: build.py [--dest DIR] [--check]
"""
import argparse
import json
import re
import sys
from pathlib import Path
from string import Template

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from common import DEST, NAME  # noqa: E402


def num(x):
    return f'{float(x):.6g}'


def horn_chain(fit):
    m = fit['modes']
    K = len(m['ratio'])
    g = fit['globals']
    lines = [
        ';; Identified bore: mode k rings at ratio_k x the bore frequency with',
        ';; peak height 10^(gain_k/20), rolled off above the bell cutoff.',
        f'(def nsel (clip partial 1 {K}))',
        f"(def ratio (selector nsel 1 {' '.join(num(r) for r in m['ratio'][1:])}))",
        '(def sounding (* played vibrato))',
        '(def bore_hz (exp (mb-glide (log (/ aim ratio)) slide trigger)))',
        f"(def height (* {num(g['C0'])} (clip (mod impedance) 0.1 4)))",
        '(def cutoff (clip (mod bell_hz) 300 8000))',
        f"(def q_base (* {num(g['Q0'])} (clip (mod bore_q) 0.2 4)))",
        '(make-history u1)',
        '(make-history u2)',
    ]
    hist, direct, writes = [], [], []
    for k, (r, gain) in enumerate(zip(m['ratio'], m['gain']), start=1):
        lines += [
            f'(make-history y1_{k})',
            f'(make-history y2_{k})',
            f'(def f{k} (min (* bore_hz {num(r)}) (* 0.45 samplerate)))',
            f'(def c{k} (* height {num(10**(gain/20))} (exp (* -1 (/ f{k} cutoff) (/ f{k} cutoff)))))',
            f"(def r{k} (exp (/ (* -1 pi (/ f{k} (* q_base {num(r**g['qexp'])}))) samplerate)))",
            f'(def a1_{k} (* 2 r{k} (cos (/ (* twopi f{k}) samplerate))))',
            f'(def a2_{k} (* r{k} r{k}))',
            f'(def b0_{k} (* 0.5 (- 1 a2_{k})))',
            f'(def h{k} (- (* a1_{k} (read-history y1_{k})) (* a2_{k} (read-history y2_{k})) (* b0_{k} (read-history u2))))',
        ]
        hist.append(f'(* c{k} h{k})')
        direct.append(f'(* c{k} b0_{k})')
        writes += [f'(write-history y2_{k} (read-history y1_{k}))',
                   f'(write-history y1_{k} (+ h{k} (* b0_{k} flow)))']
    lines += [
        ';; Bounding the returning pressure bounds the flow, and so every mode:',
        ';; extreme settings saturate instead of running away.',
        f"(def returning (clip (+ {' '.join(hist)}) -50 50))",
        f"(def direct (+ {num(g['Zr'])} {' '.join(direct)}))",
        ';; Lip opening: the buzz excursion grows with breath and narrows in the upper register.',
        '(def excursion (* (clip (mod buzz) 0 8) (pow be buzz_curve)',
        '  (pow (/ played horn_hz) (- (clip (mod register) -1 2.5)))))',
        ';; The rest opening follows the breath envelope: the lips close when the',
        ';; player stops blowing instead of leaving the valve open on a ringing bore.',
        '(def opening (max 0 (+ (* env (clip (mod lips) -1.5 1.5)) (* excursion (sin (* twopi (phasor sounding)))))))',
        '(def mouth (* be (clip (mod pressure) 0 20) (+ 1 (* (clip (mod air) 0 0.5) (noise)))))',
        ';; Bernoulli flow u = h sign(dp) sqrt|dp|, dp = mouth - (returning + direct u):',
        ';; s^2 + direct h s - |A| = 0 solved exactly.',
        '(def across (- mouth returning))',
        '(def zh (* direct opening))',
        '(def root (* 0.5 (- (sqrt (+ (* zh zh) (* 4 (abs across)))) zh)))',
        '(def flow (* (sign across) opening root))',
        '(def mouthpiece (+ returning (* direct flow)))',
        '(write-history u2 (read-history u1))',
        '(write-history u1 flow)',
    ] + writes + [
        '(make-history dcx)',
        '(make-history dcy)',
        '(def radiated (+ (- mouthpiece (read-history dcx)) (* (exp (/ (* -20 twopi) samplerate)) (read-history dcy))))',
        '(write-history dcx mouthpiece)',
        '(write-history dcy radiated)',
    ]
    return '\n'.join(lines)


def body_chain(fit):
    lines, sig = [], 'radiated'
    for i, (hz, q, db) in enumerate(fit['body']['sections']):
        lines.append(f'(def body{i} (mb-peak {sig} {num(hz)} {num(q)} {num(db)} amount))')
        sig = f'body{i}'
    lines.append(f'(def bodied {sig})')
    return '\n'.join(lines)


def dsp_source(fit):
    d = fit['defaults']
    g = fit['globals']
    values = dict(
        modes=len(fit['modes']['ratio']), horn_chain=horn_chain(fit), body_chain=body_chain(fit),
        trim=num(10**(fit['body']['trim']/20)),
        breath=num(d['breath']), breath_ms=num(d['breath_ms']), pressure=num(g['P']), air=num(d['air']),
        buzz=num(g['Lp']), buzz_curve=num(g['alpha']), register=num(g['beta']), lips=num(g['h0']),
        bell_hz=num(g['fc']), kappa=num(g['kappa']), breath_scale=num(g['breath_scale']), tune=num(d['tune']), slide=num(d['slide']), vib_cent=num(d['vib_cent']),
        vib_hz=num(d['vib_hz']), attack=num(d['attack']), release=num(d['release']),
        hall=num(d['hall']), hall_s=num(d['hall_s']))
    return Template((HERE/'engine.lisp.in').read_text()).substitute(values)


# Presets: the fitted reference voicing, then variations (short param names).
PRESETS = [
    ('milagre', 'Milagre', {}),
    ('milagre-dry', 'Milagre Dry', dict(hall=0.0)),
    ('horn-in-f', 'Natural Horn in F', dict(horn=41, tune=0.0, hall=0.2)),
    ('mellow', 'Mellow Pedal', dict(buzz_curve=1.2, bell_hz=1400, body=0.7)),
    ('brassy', 'Brassy Fanfare', dict(lips=0.25, reg_breath=0.9, hall=0.15)),
    ('breathy', 'Breathy Shepherd', dict(air=0.06, vib_cent=6.0, vib_hz=4.5, hall=0.45)),
]


def presets(source):
    params = {}
    for m in re.finditer(r'\(param (\S+)([^)]*)\)', source):
        name, attrs = m.groups()
        group = re.search(r'@group (\S+)', attrs)
        default = float(re.search(r'@default (\S+)', attrs).group(1))
        params[name] = (f'{group.group(1)}.{name}' if group else name, default)
    out = []
    for pid, label, overrides in PRESETS:
        values = {full: v for full, v in params.values()}
        for k, v in overrides.items():
            values[params[k][0]] = float(v)
        values.pop('voice_mode', None)
        out.append(dict(id=pid, name=label, base_note_offset=0, params=values))
    return dict(version=1, engine_name=f'Physical Models/{NAME}',
                source_file=f'instruments/Physical Models/{NAME}/dsp.lisp', presets=out)


def files(fit):
    dsp = dsp_source(fit)
    out = {'dsp.lisp': dsp,
           'ui.lisp': (HERE/'ui.lisp').read_text(),
           'instrument.json': json.dumps({'version': 1, 'run_mode': 'instrument',
                                          'voice_controls': {'mode': 'voice_mode'}}, indent=2) + '\n'}
    return out, presets(dsp)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--dest', default=str(DEST))
    ap.add_argument('--fit', default=str(HERE/'fit.json'))
    ap.add_argument('--check', action='store_true')
    a = ap.parse_args()
    fit = json.loads(Path(a.fit).read_text())
    dest = Path(a.dest)
    out, presets = files(fit)
    preset_path = dest.parent/f'{dest.name}.presets'
    stale = []
    for name, text in out.items():
        p = dest/name
        if a.check:
            if not p.exists() or p.read_text() != text:
                stale.append(str(p))
        else:
            dest.mkdir(parents=True, exist_ok=True)
            p.write_text(text)
    if presets is not None:
        text = json.dumps(presets, indent=2) + '\n'
        if a.check:
            if not preset_path.exists() or preset_path.read_text() != text:
                stale.append(str(preset_path))
        else:
            preset_path.write_text(text)
    if a.check:
        if stale:
            sys.exit('stale: ' + ', '.join(stale))
        print(f'{NAME}: generated files are current')
    else:
        print(f'wrote {dest}')


if __name__ == '__main__':
    main()
