#!/usr/bin/env python3
"""Deterministically generate PM Bongos from analysis.json.

The shared template is the implementation; the generated standalone source
keeps loading and patch editing independent of these offline tools.
"""
import argparse
import hashlib
import json
from pathlib import Path
from string import Template

import numpy as np

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
NAME = 'PM Bongos'
DEST = ROOT/'content/instruments/Drums'/NAME
MODES = 32
STRIKES = 3
FIRST_KEY = 60                 # C4 (the default step note); one measured stroke per semitone
CONTACT_SECONDS = 0.00025      # reference hand-contact convention, not a measured duration
NOISE_BANDS_FROM = 900.0       # below this the modal residual is misfit, not contact noise
CALIBRATION = HERE/'noise-calibration.json'
# Twelve strokes repeat every octave. The loop's second open mid tone is a
# near-duplicate of Mid Open, so it is played by that stroke.
SUBSTITUTES = {'mid-open-2': 'mid-open'}
OCTAVE_RANGE = (-3, 3)          # octaves of transposition either side of the recorded C4 octave


def strokes(data):
    """The installed stroke rows, in key order."""
    rows = [h for h in data['hits'] if h['slug'] not in SUBSTITUTES]
    assert len(rows) == 12, len(rows)
    return rows


def stroke_row(data, slug):
    """Row (key offset from C) that plays a reference hit."""
    slug = SUBSTITUTES.get(slug, slug)
    return [h['slug'] for h in strokes(data)].index(slug)


def row_modes(hit):
    f = np.array([m['hz'] for m in hit['modes']])
    r = np.array([m['rate_per_s'] for m in hit['modes']])
    A = np.array([m['residue'] for m in hit['modes']])          # modes, strikes, channels
    Q = np.array([m['sin_cos'] for m in hit['modes']])          # modes, strikes, (sin, cos)
    window = hit['window_s'][1] - hit['window_s'][0]
    energy = (A**2).sum(axis=(1, 2))*(1 - np.exp(-2*r*window))/(2*r)
    keep = np.argsort(-energy)[:MODES]
    keep = keep[np.argsort(f[keep])]
    return f[keep], r[keep], A[keep], Q[keep], energy[keep].sum()/energy.sum()


def skin_model(hit):
    """Contact click/sizzle per strike slot: only slots at detected onsets."""
    contacts = hit['contact']
    first = contacts[0]
    edges = first['band_edges_hz']
    centres = np.array([np.sqrt(a*b) for a, b in zip(edges, edges[1:])])[1:]   # 2-16 kHz
    power = np.array(first['band_power_per_bin'])[1:]
    weights = power/power.sum()
    centre = float(np.exp((weights*np.log(centres)).sum()))
    spread = float(np.sqrt((weights*np.log2(centres/centre)**2).sum()))        # octaves
    q = float(np.clip(1/(2**(spread + .5) - 2**-(spread + .5)), .35, 2))
    click, sizzle = [], []
    for delay in hit['strike_delays_s']:
        match = [i for i, o in enumerate(hit['detected_onsets_s']) if abs(o - delay) < 0.003]
        c = contacts[match[0]] if match else None
        click.append(c['click_rms'] if c else 0.0)
        sizzle.append(c['sizzle_rms'] if c else 0.0)
    return {'hz': centre, 'q': q, 'click_decay_s': first['click_decay_s'],
            'sizzle_decay_s': first['sizzle_decay_s'], 'click': click, 'sizzle': sizzle}


def coefficients(data):
    rows = len(strokes(data))
    tables = {k: np.zeros((rows, MODES)) for k in ['frequency', 'rate', 'pan']}
    for k in ['magnitude', 'sine', 'cosine']:
        tables[k] = np.zeros((rows*STRIKES, MODES))
    tables['delay'] = np.zeros(rows*STRIKES)
    for k in ['click', 'sizzle']:
        tables[k] = np.zeros(rows*STRIKES)
    for k in ['click_decay', 'sizzle_decay', 'noise_hz', 'noise_q']:
        tables[k] = np.zeros(rows)
    report = []
    calibration = json.loads(CALIBRATION.read_text()) if CALIBRATION.exists() else {}
    for row, hit in enumerate(strokes(data)):
        f, r, A, Q, kept = row_modes(hit)
        n = len(f)
        power = (A**2).sum(1)                                   # modes, channels
        tables['frequency'][row, :n] = f
        tables['rate'][row, :n] = r
        tables['pan'][row, :n] = (power[:, 0] - power[:, 1])/np.maximum(power.sum(1), 1e-30)
        # Output = Im[(sin + i cos) e^{i w t}]: identified phasors are injected as-is.
        phasor = Q[:, :, 0] + 1j*Q[:, :, 1]
        strikes = len(hit['strike_delays_s'])
        for s in range(strikes):
            # Stereo magnitude per strike, mono-sum phase.
            magnitude = np.sqrt((A[:, s, :]**2).mean(1))
            unit = phasor[:, s]/np.maximum(np.abs(phasor[:, s]), 1e-30)
            tables['magnitude'][row*STRIKES + s, :n] = magnitude
            tables['sine'][row*STRIKES + s, :n] = magnitude*unit.real
            tables['cosine'][row*STRIKES + s, :n] = magnitude*unit.imag
            tables['delay'][row*STRIKES + s] = hit['strike_delays_s'][s]
        noise = skin_model(hit)
        click_scale, sizzle_scale = calibration.get(hit['slug'], [1.0, 1.0])
        for s, (c, z) in enumerate(zip(noise['click'], noise['sizzle'])):
            tables['click'][row*STRIKES + s] = c*click_scale
            tables['sizzle'][row*STRIKES + s] = z*sizzle_scale
        tables['click_decay'][row] = noise['click_decay_s']
        tables['sizzle_decay'][row] = noise['sizzle_decay_s']
        tables['noise_hz'][row] = noise['hz']
        tables['noise_q'][row] = noise['q']
        # Unused slots stay silent: zero weight, harmless finite coefficients.
        tables['frequency'][row, n:] = 100.0
        tables['rate'][row, n:] = 50.0
        report.append({'slug': hit['slug'], 'key': FIRST_KEY + row, 'modes': n,
                       'kept_energy_fraction': round(float(kept), 5), 'noise': noise,
                       'strike_delays_s': hit['strike_delays_s']})
    return tables, report


def table_source(tables, analysis_bytes):
    text = [';; BEGIN GENERATED CALIBRATION',
            ';; Identified from the bongo-breaks reference loop; see ATTRIBUTION.md.',
            ';; Analysis SHA256: '+hashlib.sha256(analysis_bytes).hexdigest()]
    for name, a in tables.items():
        assert np.isfinite(a).all(), name
        text.append(f'(def {name}_table (tensor @shape [{" ".join(map(str, a.shape))}] @data [')
        rows = a.reshape(-1, a.shape[-1]) if a.ndim > 1 else a.reshape(1, -1)
        text.extend('  '+' '.join(f'{x:.9g}' for x in row) for row in rows)
        text.append(']))')
    return '\n'.join(text)+'\n;; END GENERATED CALIBRATION'


def source(data, analysis_bytes):
    tables, report = coefficients(data)
    text = Template((HERE/'engine.lisp.in').read_text()).substitute(
        tables=table_source(tables, analysis_bytes), modes=MODES, strikes=STRIKES,
        first_key=FIRST_KEY, min_octave=OCTAVE_RANGE[0], max_octave=OCTAVE_RANGE[1],
        contact_seconds=CONTACT_SECONDS)
    return text, report


def key_name(note):
    return ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'][note % 12] + str(note//12 - 1)


def presets():
    defaults = {'hand.hardness': .5, 'hand.dynamics': .5, 'hand.flam': 1., 'hand.flam_level': 1.,
                'head.tune': 0., 'head.decay': 1., 'head.muffle': 0., 'head.glide': 0.,
                'contact.skin': 1., 'contact.skin_tone': 0., 'output.width': 1., 'output.drive': 0.,
                'output.tone_hz': 16000., 'output.gain': 1.}
    variants = [('reference', 'Bird of Prey', {}),
                ('tight', 'Tight Studio', {'head.decay': .7, 'head.muffle': .08, 'output.width': .6}),
                ('open', 'Open Ring', {'head.decay': 1.6, 'hand.hardness': .4, 'head.glide': .3}),
                ('slappy', 'Hard Hands', {'hand.hardness': .8, 'contact.skin': 1.8, 'contact.skin_tone': .3, 'head.glide': .6}),
                ('soft', 'Soft Fingers', {'hand.hardness': .2, 'contact.skin': .5, 'hand.dynamics': .8}),
                ('dusty', 'Dusty Break', {'output.drive': .35, 'output.tone_hz': 7000., 'output.width': .5, 'head.decay': .9})]
    return {'version': 1, 'engine_name': 'Drums/'+NAME,
            'source_file': f'instruments/Drums/{NAME}/dsp.lisp',
            'presets': [{'id': slug, 'name': name, 'base_note_offset': 0, 'params': defaults | values}
                        for slug, name, values in variants]}


def ui_source(data):
    names = [f'{key_name(FIRST_KEY + i)[:-1]}  {h["name"]}' for i, h in enumerate(strokes(data))]
    columns = [names[i:i + 4] for i in range(0, len(names), 4)]
    stack = '\n'.join('          (v-stack :gap 0.05 ' + ' '.join(f'(stroke-row "{n}")' for n in col) + ')'
                      for col in columns)
    return f'''(def stroke-row (text)
  (label text :width 11.6 :height 0.56 :v-align :center :font-size 8.5 :color (rgba 0.16 0.06 0.11 1) :bg :transparent))

(def strokes-view ()
  (v-stack :gap 0.15
    (bongo-caption "Strokes repeat every octave / {key_name(FIRST_KEY)} octave = recorded pitch")
    (h-stack :gap 0.2
{stack})))

(def bongo-bind (name) (eseq.effects.physical-model-surface/bind name))

(def bongo-caption (text)
  (label text :width 35.3 :height 0.45 :v-align :center :font-size 8.5
    :color (rgba 0.16 0.06 0.11 1) :bg :transparent))

;; Normalized two-pole contact force over 0-1.5 ms, as the DSP computes it.
(defwidget pm-bongo-hand
  :width 35.3 :height 3.1 :state (hardness dynamics)
  :shader
  (let ((time (* 0.0015 0.5 (+ 1 (/ x aspect))))
        (hard (* 0.00025 (pow 2 (* 3 (- 0.5 hardness)))))
        (soft (* hard (+ 1 (* 0.75 dynamics))))
        (u (/ time hard)) (v (/ time soft))
        (curve (- 0.8 (* 1.5 u (pow 2.7182818 (- 1 u)))))
        (quiet (- 0.8 (* 1.5 (/ hard soft) v (pow 2.7182818 (- 1 v))))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (rgba 0.24 0.93 0.81 1))
      (sdf/paint (max (- y 0.8) (- curve y)) (rgba 0.16 0.06 0.11 0.14))
      (sdf/paint (- (abs (- y quiet)) 0.018) (rgba 0.16 0.06 0.11 0.4))
      (sdf/paint (- (abs (- y curve)) 0.025) (rgba 0.16 0.06 0.11 1)))))
(def hand-view ()
  (v-stack :gap 0.15 (bongo-caption "Hand contact force / 0-1.5 ms / faint = half-velocity hit")
    (pm-bongo-hand :debug-name "pm-bongo-hand" :hardness (bongo-bind "hand.hardness")
      :dynamics (bongo-bind "hand.dynamics"))))

;; Low-head fundamental ring (measured 11.5/s) with decay and muffle loss.
(defwidget pm-bongo-head
  :width 35.3 :height 3.1 :state (decay muffle glide)
  :shader
  (let ((time (* 0.6 0.5 (+ 1 (/ x aspect))))
        (rate (+ (/ 11.5 decay) (* 90 muffle muffle)))
        (curve (- 0.8 (* 1.55 (pow 2.7182818 (* -1 rate time)))))
        (bend (- 0.75 (* 12 0.03 glide (pow 2.7182818 (/ time -0.028))))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (rgba 0.24 0.93 0.81 1))
      (sdf/paint (max (- y 0.8) (- curve y)) (rgba 0.16 0.06 0.11 0.14))
      (sdf/paint (- (abs (- y bend)) 0.015) (rgba 0.16 0.06 0.11 0.4))
      (sdf/paint (- (abs (- y curve)) 0.025) (rgba 0.16 0.06 0.11 1)))))
(def head-view ()
  (v-stack :gap 0.15 (bongo-caption "Low head ring / 0-0.6 s / faint = tension glide")
    (pm-bongo-head :debug-name "pm-bongo-head" :decay (bongo-bind "head.decay")
      :muffle (bongo-bind "head.muffle") :glide (bongo-bind "head.glide"))))

;; Skin noise band: level and centre shift around a 2 kHz reference.
(defwidget pm-bongo-skin
  :width 35.3 :height 3.1 :state (skin tone)
  :shader
  (let ((octave (* 4 (/ x aspect)))
        (centre (* 1.5 tone))
        (d (- octave centre))
        (curve (- 0.8 (* 0.52 skin (/ 1 (+ 1 (* 1.6 d d)))))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (rgba 0.24 0.93 0.81 1))
      (sdf/paint (max (- y 0.8) (- curve y)) (rgba 0.16 0.06 0.11 0.14))
      (sdf/paint (- (abs (- y curve)) 0.025) (rgba 0.16 0.06 0.11 1)))))
(def skin-view ()
  (v-stack :gap 0.15 (bongo-caption "Skin contact noise / 125 Hz-32 kHz, log / band per stroke")
    (pm-bongo-skin :debug-name "pm-bongo-skin" :skin (bongo-bind "contact.skin")
      :tone (bongo-bind "contact.skin_tone"))))

(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM BONGOS\"
    (list
      '("HAND" ("hand.hardness" "Hardness" 2 :linear) ("hand.flam" "Flam" 2 :linear))
      '("HEADS" ("head.decay" "Decay" 2 :log) ("head.muffle" "Muffle" 2 :linear))
      '("PITCH" ("head.tune" "Tune st" 1 :linear) ("head.glide" "Glide" 2 :linear))
      '("SKIN & LEVEL" ("contact.skin" "Skin" 2 :linear) ("output.gain" "Output" 2 :linear)))
    (list
      (dict :title "Strokes"
        :view (lambda () (strokes-view))
        :controls (lambda () '(("hand.flam" "Flam spacing" 2) ("hand.flam_level" "Flam level" 2)))
        :hint "Flam scales the measured contact spacing.")
      (dict :title "Hand"
        :view (lambda () (hand-view))
        :controls (lambda () '(("hand.hardness" "Hardness" 2) ("hand.dynamics" "Vel > timbre" 2)))
        :hint "Harder hands shorten contact; energy is redistributed, not added.")
      (dict :title "Heads"
        :view (lambda () (head-view))
        :controls (lambda () '(("head.muffle" "Muffle" 2) ("head.glide" "Tension glide" 2)))
        :hint "Muffle rests a hand on the head. Glide bends hard hits.")
      (dict :title "Skin"
        :view (lambda () (skin-view))
        :controls (lambda () '(("contact.skin" "Skin noise" 2) ("contact.skin_tone" "Skin tone" 2)))
        :hint "Hand-on-skin contact noise, band-limited per stroke.")
      (dict :title "Output"
        :view (lambda () (eseq.effects.physical-model-surface/saron-output-view))
        :controls (lambda () '(("output.width" "Stereo" 2) ("output.drive" "Drive" 2) ("output.tone_hz" "Tone Hz" 0)))
        :hint "Stereo 1 reproduces the measured head placement."))))
'''


def attribution(data):
    ref = data['reference']
    lines = [f'# {NAME} reference material', '',
             f'Identified from the sampler track "{ref["track"]}" in `{ref["project"]}`',
             f'(`{ref["sample"]}`, SHA256 `{ref["sha256"]}`). The recording is a commercial',
             'record in the local sample library: only fitted modal coefficients are stored;',
             'no PCM, recorded phase or spectral frame ships with the instrument.', '',
             'Twelve strokes repeat in every octave. The C4 octave plays them at the recorded',
             'pitch; each octave up or down transposes the whole kit by an octave.', '',
             '| Key (any octave) | Stroke | Contact events (ms) |', '| --- | --- | --- |']
    for i, h in enumerate(strokes(data)):
        lines.append(f'| {key_name(FIRST_KEY + i)[:-1]} | {h["name"]} | '
                     + ', '.join(f'{1000*d:.1f}' for d in h['strike_delays_s']) + ' |')
    lines += ['', 'The loop\'s second open mid tone (' + ', '.join(SUBSTITUTES) + ') is a near-duplicate '
              'and is played by Mid Open (F).']
    lines += ['', 'Analysis, generator and comparison: `tools/pm-bongos/`.']
    return '\n'.join(lines) + '\n'


def outputs():
    analysis_bytes = (HERE/'analysis.json').read_bytes()
    data = json.loads(analysis_bytes)
    dsp, report = source(data, analysis_bytes)
    return {DEST/'dsp.lisp': dsp, DEST/'ui.lisp': ui_source(data),
            DEST/'instrument.json': '{"version":1,"run_mode":"instrument"}\n',
            DEST/'ATTRIBUTION.md': attribution(data),
            DEST.parent/(NAME + '.presets'): json.dumps(presets(), indent=2) + '\n',
            HERE/'build-report.json': json.dumps(report, indent=1) + '\n',
            # Copies beside the generator; the surface test reads these.
            HERE/'model.lisp': dsp, HERE/'ui.lisp': ui_source(data),
            HERE/'model.presets': json.dumps(presets(), indent=2) + '\n'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    for path, text in outputs().items():
        if args.check:
            assert path.read_text() == text, f'Regenerate {path}'
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
    print('Verified' if args.check else 'Generated', DEST)


if __name__ == '__main__':
    main()
