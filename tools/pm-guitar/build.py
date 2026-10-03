#!/usr/bin/env python3
"""Generate PM Nylon Guitar from model.json (local install: the reference is a commercial recording)."""
import argparse
import hashlib
import json
from pathlib import Path
from string import Template

import numpy as np

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
NAME = 'PM Nylon Guitar'
DEST = ROOT/'.local/instruments/Physical Models'/NAME
PARTIALS = 32
EQ_SUPPORT_HZ = 3620.0          # above this the EQ holds (no clean partials measured)
# Body resonances: fast poles that pulled the low partials (fit_model.coupling).
BODY = [  # Hz, loss /s, gain
    (61.8, 7.0, 0.25),
    (101.6, 11.5, 0.9),          # air (Helmholtz) mode
    (198.5, 25.0, 0.8),          # top plate
    (253.3, 42.8, 0.45),
    (309.6, 60.0, 0.35),
    (398.0, 47.5, 0.4),
    (514.6, 44.4, 0.35),
    (615.0, 28.8, 0.25),
]
CONTACT_S = 0.0025
BUZZ = {'buzz_default': 0.7, 'relief_default': 0.5, 'hardness_default': 0.3, 'alpha_default': 1.5,
        'contact_loss_default': 0.2, 'wood_default': 8000.0, 'upper_decay_default': 1.0}
BUZZ_KEYS = set(BUZZ)
if (HERE/'buzz.json').exists():
    BUZZ.update({k: v for k, v in json.loads((HERE/'buzz.json').read_text()).items() if k in BUZZ_KEYS})
RELEASE_MS = 1.0
BUZZ_EQ_KNOTS = 13   # third octaves 1-16 kHz
HI_PARTIALS = 96
FRETS = 6            # frets between the fretting finger and the bridge     # partials 33-128: only the fret puts energy there
REACH_FACTOR = 0.6
PHASE_SEED = 20261002
OUTPUT_TRIM_DB = 0.0   # set by compare.py --calibrate
CONTACT_HZ = 2800.0
CONTACT_LEVEL = 0.35
# D#4 is a single bright, nail-like take (tilt 0.54 against 2.4-3.0 for C4,
# G3 and the unused F4/A#3 takes); as the source for every key above C4 it
# made the treble a harpsichord. Upper keys borrow the C4 plucks.
GROUPS = ['C#2', 'D#2', 'G#2', 'F3', 'G3', 'C4']
# Buzz per recorded string (buzz_survey.py): 2-9 kHz period-locked energy is
# -25..-31 dB under the string band on every C#2/D#2 take (above their 32nd
# partial: pure buzz) and -49..-52 dB on every G#2 take. Treble pitches have
# real partials there, so they cannot be read; they get a light default.
STRING_BUZZ = {'C#2': 1.0, 'D#2': 1.0, 'G#2': 0.0, 'F3': 0.15, 'G3': 0.15, 'C4': 0.15}


CALIBRATION = HERE/'calibration.json'


def calibration(n_knots):
    if CALIBRATION.exists():
        return json.loads(CALIBRATION.read_text())
    return {'eq_trim_db': [0.0]*n_knots, 'loss_trim': [0.0]*n_knots, 'knock': [0.0]*len(BODY), 'output_trim_db': 0.0,
            'group_trim_db': [0.0]*len(GROUPS)}


def take_energy(t, pitch, eq, knots):
    """Sum of squared partial amplitudes the engine gives a take at its own pitch (L excluded)."""
    k = np.arange(1, PARTIALS + 1)
    f = k*pitch['f0_hz']*np.sqrt((1 + pitch['B']*k*k)/(1 + pitch['B']))
    x = np.clip(2*np.log2(np.maximum(f, knots[0])/knots[0]), 0, len(knots) - 1 - 1e-6)
    i = np.floor(x).astype(int); w = x - i
    h = (1 - w)*eq[i] + w*eq[np.minimum(i + 1, len(knots) - 1)]
    a = np.sin(np.pi*k*t['beta'])*np.exp(h + np.array(t['detail_ln'][:PARTIALS]) - t['alpha']*np.log(k) - (f/t['fc_hz'])**2)
    return float(np.sum(a*a))


HERO = HERE/'hero.json'


def law_rate(hz, s0, model, cal, knots):
    """The engine's default loss for a partial (decay = damping = 1)."""
    q = model['loss_b2']*hz**2
    x = np.clip(2*np.log2(np.maximum(hz, knots[0])/knots[0]), 0, len(knots) - 1 - 1e-6)
    i = np.floor(x).astype(int); w = x - i
    trim = np.array(cal['loss_trim'])
    return np.maximum(0.8, s0 + (1 - w)*trim[i] + w*trim[np.minimum(i + 1, len(knots) - 1)] + q/(1 + q/model['loss_cap']))


def hero_pairs():
    """Per partial: (detune Hz, ln loss ratio, complex ratio) of the hero's partner pole, or None."""
    hero = json.loads(HERO.read_text())
    out = []
    for q in hero['partials']:
        pr = q.get('pair', [])
        if len(pr) < 2:
            out.append(None)
            continue
        a, b = pr[0], pr[1]
        ratio = complex(b['re'], b['im'])/complex(a['re'], a['im'])
        out.append((b['hz'] - a['hz'], np.log(max(b['rate'], 0.2)/max(a['rate'], 0.2)), ratio))
    return out


def polarization(n_rows):
    """Generic partner poles for every take: the hero's measured pattern, ratios
    capped (a partner never dominates a borrowed pluck), loss ratio bounded."""
    det = np.zeros(PARTIALS); lr = np.zeros(PARTIALS); ratio = np.zeros(PARTIALS, complex)
    if HERO.exists():
        for k, p in enumerate(hero_pairs()):
            if p is None:
                continue
            det[k] = np.clip(p[0], -7, 7)
            lr[k] = np.clip(p[1], -1.5, 1.5)
            ratio[k] = p[2]/max(1.0, abs(p[2]))*min(abs(p[2]), 1.0)
    rows = lambda v: np.tile(v, (n_rows, 1))
    return rows(det), rows(lr), rows(ratio.real), rows(ratio.imag)


def apply_hero(t, rows, groups, phases, eq, knots, cal, model):
    """The directly identified first pluck plays its measured partials (amplitude, phase, loss)."""
    if not HERO.exists():
        return None
    hero = json.loads(HERO.read_text())
    r = next((i for i, (g, tk) in enumerate(rows) if groups[g][1]['name'] == 'C#2'
              and abs(groups[g][1]['takes'][tk['take']]['onset_s'] - hero['onset_s']) < 0.005), None)
    if r is None:
        return None
    g, tk = rows[r]
    pitch = groups[g][1]
    k = np.arange(1, PARTIALS + 1)
    f = k*pitch['f0_hz']*np.sqrt((1 + pitch['B']*k*k)/(1 + pitch['B']))
    x = np.clip(2*np.log2(np.maximum(f, knots[0])/knots[0]), 0, len(knots) - 1 - 1e-6)
    i = np.floor(x).astype(int); w = x - i
    h = (1 - w)*eq[i] + w*eq[np.minimum(i + 1, len(knots) - 1)]
    comb = np.sin(np.pi*k*tk['beta'])
    # the global output trim scales every row; the measured partials are absolute
    pred = (np.exp(tk['level'])*10**(cal['output_trim_db']/20)*np.abs(comb)
            *np.exp(h - tk['alpha']*np.log(k) - (f/tk['fc_hz'])**2))
    c = np.array([complex(q['pair'][0]['re'], q['pair'][0]['im']) if q.get('pair') else q['re'] + 1j*q['im']
                  for q in hero['partials']])
    detail = np.log(np.maximum(np.abs(c), 1e-9)/np.maximum(pred, 1e-12))
    # the engine's state starts at i*c (output is its imaginary part)
    theta = np.angle(1j*c) + np.where(comb < 0, np.pi, 0.0)
    t['detail'][r] = np.clip(detail, -6, 6)
    phases[r] = theta
    s0 = t['s0'][g]
    meas = np.array([q['pair'][0]['rate'] if q.get('pair') else q['rate'] for q in hero['partials']])
    for kk, p in enumerate(hero_pairs()):
        if p is None:
            t['pol_re'][r, kk] = t['pol_im'][r, kk] = 0.0
            continue
        t['pol_detune'][r, kk], t['pol_rate'][r, kk] = p[0], p[1]
        t['pol_re'][r, kk], t['pol_im'][r, kk] = p[2].real, p[2].imag
    t['rate_detail'][r] = np.clip(np.log(np.maximum(meas, 0.2)/law_rate(f, s0, model, cal, knots)), -3, 3)
    # its two-pole splits are already merged into the measured partials
    t['cpl_re'][2*r:2*r + 2] = 0.0
    t['cpl_im'][2*r:2*r + 2] = 0.0
    return r


def fret_x():
    return [2**(-j/12) for j in range(1, FRETS + 1)]


def fret_code():
    """Unrolled per-fret contact (DGenLisp has no tensor-returning macros)."""
    out = []
    for j, x in enumerate(fret_x(), 1):
        out.append(f"""(def fphi{j} (latch (* contact_bl (sin (* pi ns {x:.9f}))) update_tick))
(def fhphi{j} (latch (* hcontact_bl (sin (* pi hns {x:.9f}))) update_tick))
(def fsq{j} (latch (max 0.000001 (+ (sum (* fphi{j} fphi{j})) (sum (* fhphi{j} fhphi{j})))) update_tick))
(def fgap{j} (* (peek fret_sw_table {j - 1}) (- 1 (* 0.97 buzz_amt)) (+ 1 (* relief_v {(j - 1)/(FRETS - 1):.6f}))))
(def feta{j} (+ (sum (* rx fphi{j})) (sum (* hrx fhphi{j}))))
(def fvel{j} (+ (sum (* -1 omega_a ry fphi{j})) (sum (* -1 homega_a hry fhphi{j}))))
(def fpen{j} (max 0 (- (- 0 fgap{j}) feta{j})))
(def fresp{j} (latch (max 0.000001 (+ (sum (* fphi{j} fphi{j} (/ rot_s omega_a))) (sum (* fhphi{j} fhphi{j} (/ hrot_s homega_a))))) update_tick))
(def fcap{j} (/ 4 (* {FRETS} fresp{j})))
(def fk{j} (min fcap{j} (* fcap{j} contact_v_k (pow (/ (max fpen{j} 0.0000000001) (* 0.01 (peek fret_sw_table 0))) alpha_m1))))
(def fforce{j} (+ (/ (* fk{j} fpen{j}) (+ 1 (* {FRETS} fk{j} fresp{j}))) (* (gt fpen{j} 0) closs (/ (max 0 (- 0 fvel{j})) (* {FRETS} fsq{j})))))""")
    kick = ' '.join(f'(* fforce{j} fphi{j})' for j in range(1, FRETS + 1))
    hkick = ' '.join(f'(* fforce{j} fhphi{j})' for j in range(1, FRETS + 1))
    return '\n'.join(out), kick, hkick


def fret_swings(t, rows, groups, r, eq, knots):
    """First pluck's swing at each fret (engine units, vel 0.6): the gap scale."""
    if r is None:
        return np.full(FRETS, 0.01)
    g, tk = rows[r]
    pitch = groups[g][1]
    k = np.arange(1, PARTIALS + 1)
    f = k*pitch['f0_hz']*np.sqrt((1 + pitch['B']*k*k)/(1 + pitch['B']))
    x = np.clip(2*np.log2(np.maximum(f, knots[0])/knots[0]), 0, len(knots) - 1 - 1e-6)
    i = np.floor(x).astype(int); w = x - i
    h = (1 - w)*eq[i] + w*eq[np.minimum(i + 1, len(knots) - 1)]
    raw = np.sin(np.pi*k*tk['beta'])*np.exp(h + t['detail'][r] - tk['alpha']*np.log(k) - (f/tk['fc_hz'])**2)
    return np.array([REACH_FACTOR*np.exp(t['level'][r])*np.sum(np.abs(raw*np.sin(np.pi*k*xf))) for xf in fret_x()])


def tables(model):
    pitches = {p['name']: (i, p) for i, p in enumerate(model['pitches'])}
    groups = [pitches[n] for n in GROUPS]
    rows, start, count = [], [], []
    for gi, (pi, p) in enumerate(groups):
        takes = sorted([t for t in model['takes'] if t['pitch'] == pi], key=lambda t: t['level'])
        start.append(len(rows)); count.append(len(takes))
        rows += [(gi, t) for t in takes]
    eq = np.array(model['eq_ln'])
    knots = np.array(model['eq_knots_hz'])
    hold = np.flatnonzero(knots <= EQ_SUPPORT_HZ)[-1]
    cal = calibration(len(knots))
    eq = eq + np.array(cal['eq_trim_db'])*np.log(10)/20
    # above the last measured partials the EQ (and its trim) holds: the
    # record's 4-9 kHz content there is room/other strings, not this pluck
    eq[hold + 1:] = eq[hold]
    # every group plays at the same median partial energy; takes keep their
    # spread (the fitted level L alone misleads: a flat-tilt pluck has a low
    # L but a lot of energy)
    energy = np.array([np.exp(2*t['level'])*take_energy(t, groups[g][1], eq, knots) for g, t in rows])
    g_med = {gi: np.median([e for (g, _), e in zip(rows, energy) if g == gi]) for gi in range(len(groups))}
    norm = np.median(energy)
    trim = cal.get('group_trim_db', [0.0]*len(groups))
    equalized = np.array([t['level'] + 0.5*np.log(norm/g_med[g]) + trim[g]*np.log(10)/20 for g, t in rows])
    level = equalized + calibration(len(model['eq_knots_hz']))['output_trim_db']*np.log(10)/20
    midi = np.array([p['midi'] for _, p in groups])
    key_group = np.array([int(np.argmin(np.abs(midi - k) + 1e-3*(midi > k))) for k in range(128)])
    t = {
        'group': key_group.astype(float),
        'start': np.array(start, float), 'count': np.array(count, float),
        'ref_hz': np.array([p['f0_hz'] for _, p in groups]),
        'b': np.array([p['B'] for _, p in groups]),
        # a single take's fundamental pole is contaminated by the strings
        # around it: those pitches use the multi-take median base loss
        's0': np.array([p['s0'] if len(p['takes']) >= 2 else float(np.median(
            [q['s0'] for q in model['pitches'] if len(q['takes']) >= 2])) for _, p in groups]),
        'level': level,
        'beta': np.array([tk['beta'] for _, tk in rows]),
        'alpha': np.array([tk['alpha'] for _, tk in rows]),
        'fc': np.array([tk['fc_hz'] for _, tk in rows]),
        'detail': np.array([tk['detail_ln'][:PARTIALS] for _, tk in rows]),
        'eq': eq,
        'loss_trim': np.array(cal['loss_trim']),
        'buzz_group': np.array([STRING_BUZZ[n] for n in GROUPS]),
        'buzz_eq': np.array(cal.get('buzz_eq_ln', [0.0]*BUZZ_EQ_KNOTS)),
        # body-coupled partners of partials 1-2: per row complex amplitude
        # relative to the string partial, frequency ratio and loss rate
        'cpl_re': np.array([[c['re'] for c in tk['coupled']] for _, tk in rows]).reshape(-1),
        'cpl_im': np.array([[c['im'] for c in tk['coupled']] for _, tk in rows]).reshape(-1),
        'cpl_ratio': np.array([[c['ratio'] for c in tk['coupled']] for _, tk in rows]).reshape(-1),
        'cpl_rate': np.array([[c['rate'] for c in tk['coupled']] for _, tk in rows]).reshape(-1),
    }
    rng = np.random.default_rng(PHASE_SEED)
    phase = rng.uniform(-np.pi, np.pi, PARTIALS)
    phase[:2] = 0.0                 # partials 1-2 start like the plain pluck: sine from zero
    phases = np.tile(phase, (len(rows), 1))
    t['rate_detail'] = np.zeros((len(rows), PARTIALS))
    t['pol_detune'], t['pol_rate'], t['pol_re'], t['pol_im'] = polarization(len(rows))
    hero_row = apply_hero(t, rows, groups, phases, eq, knots, cal, model)
    t['fret_sw'] = fret_swings(t, rows, groups, hero_row, eq, knots)
    t['buzz_reach'] = float(t['fret_sw'][0])
    # the fundamental is the least-damped plucked partial, partial 33 the
    # least-damped upper one (loss grows with frequency)
    t['first_mask'] = np.r_[1.0, np.zeros(PARTIALS - 1)]
    t['hfirst_mask'] = np.r_[1.0, np.zeros(HI_PARTIALS - 1)]
    t['phase_cos'] = np.cos(phases).reshape(-1)
    t['phase_sin'] = np.sin(phases).reshape(-1)
    t['rate_detail'] = t['rate_detail'].reshape(-1)
    for name in ('pol_detune', 'pol_rate', 'pol_re', 'pol_im'):
        t[name] = t[name].reshape(-1)
    body = {'body_hz_table': np.array([b[0] for b in BODY]), 'body_rate_table': np.array([b[1] for b in BODY]),
            'body_gain_table': np.array([b[2] for b in BODY]), 'body_knock_table': np.array(cal['knock'])}
    info = [{'group': GROUPS[g], 'onset_s': model['pitches'][groups[g][0]]['takes'][tk['take']]['onset_s'],
             'beta': tk['beta'], 'alpha': tk['alpha'], 'fc_hz': tk['fc_hz'], 'level_db': float(20/np.log(10)*lv),
             'level_shift_db': float(20/np.log(10)*(eqd - tk['level']))}
            for (g, tk), lv, eqd in zip(rows, level, equalized)]
    return t, body, info, knots


def table_source(t, body, model_bytes):
    text = [';; BEGIN GENERATED CALIBRATION',
            ';; Identified from plucks of a commercial nylon-guitar recording; see ATTRIBUTION.md.',
            ';; Model SHA256: '+hashlib.sha256(model_bytes).hexdigest()]
    def emit(name, a):
        a = np.asarray(a, float)
        assert np.isfinite(a).all(), name
        text.append(f'(def {name} (tensor @shape [{" ".join(map(str, a.shape))}] @data [')
        rows = a.reshape(-1, a.shape[-1]) if a.ndim > 1 else a.reshape(1, -1)
        text.extend('  '+' '.join(f'{x:.9g}' for x in row) for row in rows)
        text.append(']))')
    for name, a in t.items():
        if name in ('buzz_reach',):
            continue
        emit(name + '_table', a.reshape(-1) if name == 'detail' else a)
    for name, a in body.items():
        emit(name, a)
    return '\n'.join(text)+'\n;; END GENERATED CALIBRATION'


def presets():
    defaults = {'fret.level': 1.0, 'body.wood': BUZZ['wood_default'], 'string.shimmer': 1.0, 'fret.spread': 0.0, 'pluck.take': .5, 'pluck.vel_take': .6, 'pluck.humanize': .35, 'pluck.position': 0., 'pluck.finger': 0., 'pluck.vel_tone': .5, 'pluck.detail': 1.,
                'string.decay': 1., 'string.damping': 1., 'string.mute': 0., 'string.release': .8, 'string.stiffness': 1., 'string.tune': 0., 'string.vibrato': 0.,
                'string.vib_rate': 5.5, 'body.body': 1., 'body.resonance': 1., 'body.contact': 1., 'output.tone_hz': 14000., 'output.gain': 1.}
    variants = [('reference', 'Nascer', {}),
                ('thumb', 'Warm Thumb', {'pluck.finger': -.6, 'pluck.position': .06, 'body.resonance': 1.4}),
                ('nail', 'Nail Ponticello', {'pluck.finger': .8, 'pluck.position': -.15, 'body.contact': 1.8}),
                ('muted', 'Palm Muted', {'string.mute': .45, 'string.release': .15, 'body.resonance': .6}),
                ('bloom', 'Long Bloom', {'string.decay': 2.2, 'string.damping': .6, 'string.release': 3., 'string.vibrato': 8.}),
                ('even', 'Even Hands', {'pluck.humanize': 0., 'pluck.vel_take': 0., 'pluck.take': .6}),
                ('no-buzz', 'Clean Frets', {'fret.buzz': 0.})]
    return {'version': 1, 'engine_name': 'Physical Models/'+NAME,
            'source_file': f'instruments/Physical Models/{NAME}/dsp.lisp',
            'presets': [{'id': slug, 'name': name, 'base_note_offset': 0, 'params': defaults | values}
                        for slug, name, values in variants]}


def ui_source(model):
    # User-tier panels cannot use defwidget yet (eseq bead filed): text view.
    counts = {}
    for t in model['takes']:
        n = model['pitches'][t['pitch']]['name']
        if n in GROUPS:
            counts[n] = counts.get(n, 0) + 1
    rows = '\n'.join(f'      (gtr-caption "{n}: {counts.get(n, 0)} recorded plucks, soft to strong")' for n in GROUPS)
    view = f"""(def gtr-caption (text)
  (label text :width 35.3 :height 0.42 :v-align :center :font-size 8.5
    :color (rgba 0.16 0.06 0.11 1) :bg :transparent))

(def pluck-view ()
  (v-stack :gap 0.02
      (gtr-caption "Every key plays the nearest recorded pitch's plucks:")
{rows}))

"""
    return view + '''(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM NYLON GUITAR"
    (list
      '("PLUCK" ("pluck.take" "Take" 2 :linear) ("pluck.finger" "Finger" 2 :linear))
      '("STRING" ("string.decay" "Decay" 2 :log) ("string.mute" "Mute" 2 :linear))
      '("HAND" ("pluck.position" "Position" 2 :linear) ("pluck.humanize" "Humanize" 2 :linear))
      '("BODY & LEVEL" ("body.resonance" "Body" 2 :linear) ("output.gain" "Output" 2 :linear)))
    (list
      (dict :title "Pluck"
        :view (lambda () (pluck-view))
        :controls (lambda () '(("pluck.vel_take" "Vel > take" 2) ("pluck.vel_tone" "Vel > tone" 2) ("pluck.detail" "Detail" 2)))
        :hint "Each key borrows a recorded pluck of the nearest recorded pitch.")
      (dict :title "String"
        :view (lambda () (pluck-view))
        :controls (lambda () '(("string.damping" "Damping" 2) ("string.release" "Release s" 2) ("string.stiffness" "Stiffness" 2)))
        :hint "Damping scales high-partial loss; Release is the fretting hand lifting.")
      (dict :title "Pitch"
        :view (lambda () (pluck-view))
        :controls (lambda () '(("string.tune" "Tune ct" 0) ("string.vibrato" "Vibrato ct" 1) ("string.vib_rate" "Vib rate" 1)))
        :hint "Vibrato is the fretting finger rolling the string.")
      (dict :title "Fret"
        :view (lambda () (pluck-view))
        :controls (lambda () '(("fret.buzz" "Buzz" 2) ("fret.level" "Buzz level" 2) ("fret.spread" "Spread" 2) ("body.wood" "Wood Hz" 0)))
        :hint "Buzz sets the fret gap; Buzz level only how loud it is heard.")
      (dict :title "Body"
        :view (lambda () (pluck-view))
        :controls (lambda () '(("body.body" "Body EQ" 2) ("body.resonance" "Resonance" 2) ("body.contact" "Contact" 2)))
        :hint "Measured radiation EQ, air/top resonances and finger noise.")
      (dict :title "Output"
        :view (lambda () (pluck-view))
        :controls (lambda () '(("output.tone_hz" "Tone Hz" 0) ("output.gain" "Output" 2)))
        :hint "The reference is mono; both channels carry the same signal."))))
'''


def attribution(info):
    lines = [f'# {NAME} reference material', '',
             'Identified from the opening solo nylon-string guitar of a commercial recording',
             '("Nascer", first 15 s). Only fitted string/body coefficients are stored; no PCM,',
             'recorded phase or spectral frame ships with the instrument. Local use only.', '',
             'Recorded plucks (rows), soft to strong within each pitch:', '',
             '| Pitch | Onset (s) | Position | Tilt | Roll-off (Hz) | Level (dB) |', '| --- | ---: | ---: | ---: | ---: | ---: |']
    for r in info:
        lines.append(f"| {r['group']} | {r['onset_s']:.2f} | {r['beta']:.3f} | {r['alpha']:.2f} | {r['fc_hz']:.0f} | {r['level_db']:.1f} |")
    lines += ['', 'Analysis, generator and checks: `tools/pm-guitar/`.']
    return '\n'.join(lines) + '\n'


def outputs():
    model_bytes = (HERE/'model.json').read_bytes()
    model = json.loads(model_bytes)
    t, body, info, knots = tables(model)
    calls, kick, hkick = fret_code()
    dsp = Template((HERE/'engine.lisp.in').read_text()).substitute(
        fret_calls=calls, kick_sum=kick, hkick_sum=hkick, frets=FRETS, frets_m1=FRETS - 1,
        name=NAME, tables=table_source(t, body, model_bytes), partials=PARTIALS, body_modes=len(BODY),
        loss_b2=f"{model['loss_b2']:.6g}", loss_cap=f"{model['loss_cap']:.6g}",
        eq_lo=f'{knots[0]:g}', eq_last=f'{len(knots) - 1 - 1e-6:.6f}', eq_last_index=len(knots) - 1,
        buzz_reach=f"{t['buzz_reach']:.6g}", contact_s=CONTACT_S, release_ms=RELEASE_MS, beq_last=f'{BUZZ_EQ_KNOTS - 1 - 1e-6:.6f}', beq_last_index=BUZZ_EQ_KNOTS - 1, hi_first=PARTIALS + 1, hi_last=PARTIALS + HI_PARTIALS, hi_count=HI_PARTIALS, ref_hz_c2=f"{json.loads(HERO.read_text())['f0_hz']:.6g}" if HERO.exists() else '69.3', **{k: f'{v:.6g}' for k, v in BUZZ.items() if k in BUZZ_KEYS}, contact_hz=CONTACT_HZ, contact_level=CONTACT_LEVEL)
    ui = ui_source(model)
    generated = presets()
    pre = json.dumps(generated, indent=2) + '\n'
    # Presets saved in the app live in the same file: keep every id the
    # generator does not own, so a rebuild never deletes the user's presets.
    installed = DEST.parent/(NAME + '.presets')
    pre_local = pre
    if installed.exists():
        own = {q['id'] for q in generated['presets']}
        mine = [q for q in json.loads(installed.read_text()).get('presets', []) if q.get('id') not in own]
        if mine:
            pre_local = json.dumps(generated | {'presets': generated['presets'] + mine}, indent=2) + '\n'
    return {DEST/'dsp.lisp': dsp, DEST/'ui.lisp': ui, DEST/'instrument.json': '{"version":1,"run_mode":"instrument"}\n',
            DEST/'ATTRIBUTION.md': attribution(info), DEST.parent/(NAME + '.presets'): pre_local,
            HERE/'model.lisp': dsp, HERE/'ui.lisp': ui, HERE/'model.presets': pre,
            HERE/'build-report.json': json.dumps({'rows': info}, indent=1) + '\n'}


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--check', action='store_true')
    args = ap.parse_args()
    for path, text in outputs().items():
        if args.check:
            assert path.read_text() == text, f'Regenerate {path}'
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
    print('Verified' if args.check else 'Generated', DEST)


if __name__ == '__main__':
    main()
