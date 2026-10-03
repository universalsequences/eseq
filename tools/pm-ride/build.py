#!/usr/bin/env python3
"""Deterministically generate PM Ride Kit from analysis.json.

The shared template is the implementation; the generated standalone source
keeps loading and patch editing independent of these offline tools.
"""
import argparse
import hashlib
import json
from pathlib import Path
from string import Template

import numpy as np

from analyze import THIRD_OCTAVES

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
NAME = 'PM Ride Kit'
DEST = ROOT/'content/instruments/Drums'/NAME
FIRST_KEY = 60                 # C4 (the default step note); one measured stroke per semitone
CONTACT_SECONDS = 0.00025      # reference stick-contact convention, not a measured duration
CLICK_Q = 1.5
WASH_Q = 3.0                   # per stage; two stages make one third-octave band
LOSS_REFERENCE_HZ = 1000.0     # Muffle/Choke loss grows with sqrt(f / this)
OCTAVE_RANGE = (-3, 3)          # octaves of transposition either side of the recorded C4 octave
PAN_SEED = 20261001
CALIBRATION = HERE/'noise-calibration.json'
# Twelve strokes repeat every octave, in key order from C: reference stroke
# index (analysis.json) and listening name. Every other reference stroke is
# played by the installed stroke with the closest spectrum (SUBSTITUTES).
KEYS = [
    (20, 'ride', 'Ride'),
    (5, 'ride-push', 'Ride Push'),
    (13, 'ride-soft', 'Ride Soft'),
    (7, 'ghost', 'Ghost'),
    (14, 'tip', 'Tip'),
    (19, 'dark', 'Dark'),
    (12, 'shoulder', 'Shoulder'),
    (21, 'shoulder-accent', 'Shoulder Accent'),
    (8, 'shoulder-soft', 'Shoulder Soft'),
    (15, 'ping', 'Ping'),
    (10, 'bell', 'Bell'),
    (9, 'bell-open', 'Bell Open'),
]
SUBSTITUTES = {}


def ring_arrays(data):
    hz = np.array(data['ring']['hz'])
    rate = np.array(data['ring']['rate_per_s'])
    sc = np.array(data['ring']['sin_cos'])          # strokes, modes, (sin, cos)
    return hz, rate, sc


def attack_arrays(data):
    return (np.array(data['attack']['hz']), np.array(data['attack']['rate_per_s']),
            np.array(data['attack']['sin_cos']))


def strokes(data):
    """The installed stroke rows, in key order, named."""
    assert len(KEYS) == 12, len(KEYS)
    return [data['hits'][index] | {'name': name, 'slug': slug} for index, slug, name in KEYS]


def stroke_profile(data, index):
    """Normalized third-octave ring energy of one reference stroke (dB)."""
    hz, rate, sc = ring_arrays(data)
    e = (sc[index]**2).sum(1)/(2*rate)
    bands = np.searchsorted(THIRD_OCTAVES, hz)
    profile = np.array([e[bands == b].sum() for b in range(len(THIRD_OCTAVES) + 1)])
    return 10*np.log10(profile/profile.sum() + 1e-6)


def similar_keys(data):
    """Each reference stroke without a key -> the installed stroke with the closest spectrum."""
    keyed = [k[0] for k in KEYS]
    return {hit['index']: min(keyed, key=lambda i: float(np.abs(stroke_profile(data, i) - stroke_profile(data, hit['index'])).mean()))
            for hit in data['hits'] if hit['index'] not in keyed}


def stroke_row(data, index):
    """Row (key offset from C) that plays reference stroke `index`."""
    if not SUBSTITUTES:
        SUBSTITUTES.update(similar_keys(data))
    return [k[0] for k in KEYS].index(SUBSTITUTES.get(index, index))


def coefficients(data):
    hz, rate, sc = ring_arrays(data)
    ahz, arate, asc = attack_arrays(data)
    wash_hz = np.array([np.sqrt(w['band_hz'][0]*w['band_hz'][1]) for w in data['wash']])
    fast_rate = np.array([w['fast_rate_per_s'] for w in data['wash']])
    slow_rate = np.array([w['slow_rate_per_s'] for w in data['wash']])
    fast_power = np.array([w['fast_power'] for w in data['wash']]).T     # strokes, bands
    slow_power = np.array([w['slow_power'] for w in data['wash']]).T
    rows = strokes(data)
    tables = {'frequency': np.concatenate([hz, ahz]), 'rate': np.concatenate([rate, arate])}
    modes = len(tables['frequency'])
    tables['pan'] = np.random.default_rng(PAN_SEED).uniform(-1, 1, modes)
    for k in ['magnitude', 'sine', 'cosine']:
        tables[k] = np.zeros((len(rows), modes))
    for k in ['click', 'click_decay', 'click_hz']:
        tables[k] = np.zeros(len(rows))
    calibration = json.loads(CALIBRATION.read_text()) if CALIBRATION.exists() else {}
    tables['wash_hz'] = wash_hz
    tables['wash_buildup'] = np.array([1/w['buildup_s'] for w in data['wash']])
    tables['wash_fast_rate'] = fast_rate
    tables['wash_slow_rate'] = slow_rate
    # Per-band power calibration of the rendered wash (compare.py --calibrate-noise):
    # band-pass skirts overlap and warp near Nyquist, which the engine's
    # analytic unit-variance normalization ignores.
    gain = np.array(calibration.get('wash', [1.0]*len(wash_hz)))
    tables['wash_fast_power'] = np.array([fast_power[hit['index']] for hit in rows])*gain
    tables['wash_slow_power'] = np.array([slow_power[hit['index']] for hit in rows])*gain
    report = []
    for row, hit in enumerate(rows):
        s = hit['index']
        weights = np.concatenate([sc[s], asc[s]])     # modes, (sin, cos)
        tables['sine'][row] = weights[:, 0]
        tables['cosine'][row] = weights[:, 1]
        tables['magnitude'][row] = np.hypot(weights[:, 0], weights[:, 1])
        contact = hit['contact']
        tables['click'][row] = contact['click_rms']*calibration.get('click', {}).get(hit['slug'], 1.0)
        tables['click_decay'][row] = contact['click_decay_s']
        tables['click_hz'][row] = contact['centre_hz']
        report.append({'slug': hit['slug'], 'key': FIRST_KEY + row, 'reference_onset_s': hit['onset_s'],
                       'ring_energy_db': round(10*np.log10(hit['ring_energy']), 2), 'contact': contact})
    return tables, {'modes': modes, 'ring_modes': len(hz), 'attack_modes': len(ahz),
                    'dropped_ring_modes': data['ring']['identified_poles'] - len(hz), 'wash_bands': len(wash_hz),
                    'kept_ring_energy_fraction': data['ring']['kept_energy_fraction'], 'strokes': report}


def table_source(tables, analysis_bytes):
    text = [';; BEGIN GENERATED CALIBRATION',
            f';; Identified from the reference ride sample; see ATTRIBUTION.md.',
            ';; Analysis SHA256: '+hashlib.sha256(analysis_bytes).hexdigest()]
    for name, a in tables.items():
        assert np.isfinite(a).all(), name
        text.append(f'(def {name}_table (tensor @shape [{" ".join(map(str, a.shape))}] @data [')
        rows = a.reshape(-1, a.shape[-1]) if a.ndim > 1 else a.reshape(1, -1)
        for row in rows:
            for i in range(0, len(row), 16):
                text.append('  '+' '.join(f'{x:.7g}' for x in row[i:i + 16]))
        text.append(']))')
    return '\n'.join(text)+'\n;; END GENERATED CALIBRATION'


def source(data, analysis_bytes):
    tables, report = coefficients(data)
    text = Template((HERE/'engine.lisp.in').read_text()).substitute(
        name=NAME, tables=table_source(tables, analysis_bytes), modes=report['modes'],
        ring=report['ring_modes'], attack=report['attack_modes'], first_key=FIRST_KEY,
        dropped=round(report['dropped_ring_modes'], -2), wash_bands=report['wash_bands'], wash_q=WASH_Q,
        min_octave=OCTAVE_RANGE[0], max_octave=OCTAVE_RANGE[1], contact_seconds=CONTACT_SECONDS,
        click_q=CLICK_Q, loss_reference_hz=LOSS_REFERENCE_HZ)
    return text, report


def key_name(note):
    return ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'][note % 12] + str(note//12 - 1)


def presets():
    defaults = {'stick.hardness': .5, 'stick.dynamics': .5, 'stick.click': 1., 'stick.click_tone': 0.,
                'cymbal.decay': 1., 'cymbal.muffle': 0., 'cymbal.wash': 1., 'cymbal.choke': 0., 'cymbal.tune': 0.,
                'output.width': 0., 'output.drive': 0., 'output.tone_hz': 20000., 'output.gain': 1.,
                'voice_mode': 1.}
    variants = [('reference', 'Recorded', {}),
                ('wide', 'Wide Ride', {'output.width': .7}),
                ('dry', 'Taped', {'cymbal.decay': .6, 'cymbal.muffle': .35, 'output.width': .3}),
                ('dark', 'Dark Stick', {'stick.hardness': .2, 'stick.click': .5, 'output.tone_hz': 9000., 'output.width': .3}),
                ('bright', 'Bright Stick', {'stick.hardness': .85, 'stick.click': 1.8, 'stick.click_tone': .3, 'output.width': .3}),
                ('choke', 'Grab', {'cymbal.choke': .6, 'output.width': .3}),
                ('dusty', 'Dusty Record', {'output.drive': .35, 'output.tone_hz': 7000., 'cymbal.decay': .9})]
    return {'version': 1, 'engine_name': 'Drums/'+NAME,
            'source_file': f'instruments/Drums/{NAME}/dsp.lisp',
            'presets': [{'id': slug, 'name': name, 'base_note_offset': 0, 'params': defaults | values}
                        for slug, name, values in variants]}


def ring_reference(data):
    """Median loss rate (1/s) of the kept ring modes, weighted by energy: the ring widget's curve."""
    hz, rate, sc = ring_arrays(data)
    e = (sc**2).sum(axis=(0, 2))/(2*rate)
    order = np.argsort(rate)
    cumulative = np.cumsum(e[order])/e.sum()
    return float(rate[order][np.searchsorted(cumulative, 0.5)])


def ui_source(data):
    names = [f'{key_name(FIRST_KEY + i)[:-1]}  {h["name"]}' for i, h in enumerate(strokes(data))]
    columns = [names[i:i + 4] for i in range(0, len(names), 4)]
    stack = '\n'.join('          (v-stack :gap 0.05 ' + ' '.join(f'(ride-kit-stroke "{n}")' for n in col) + ')'
                      for col in columns)
    text = f'''(def ride-kit-stroke (text)
  (label text :width 11.6 :height 0.56 :v-align :center :font-size 8.5 :color (rgba 0.16 0.06 0.11 1) :bg :transparent))

(def ride-kit-caption (text)
  (label text :width 35.3 :height 0.45 :v-align :center :font-size 8.5
    :color (rgba 0.16 0.06 0.11 1) :bg :transparent))

(def ride-kit-strokes-view ()
  (v-stack :gap 0.15
    (ride-kit-caption "Strokes repeat every octave / {key_name(FIRST_KEY)} octave = recorded pitch")
    (h-stack :gap 0.2
{stack})))

(def ride-kit-bind (name) (eseq.effects.physical-model-surface/bind name))

;; Normalized two-pole stick force over 0-1.5 ms, as the DSP computes it.
(defwidget pm-ride-kit-stick
  :width 35.3 :height 3.1 :state (hardness dynamics) :bindable (hardness dynamics)
  :shader
  (let ((time (* 0.0015 0.5 (+ 1 (/ x aspect))))
        (hard (* {CONTACT_SECONDS} (pow 2 (* 3 (- 0.5 hardness)))))
        (soft (* hard (+ 1 (* 0.75 dynamics))))
        (u (/ time hard)) (v (/ time soft))
        (curve (- 0.8 (* 1.5 u (pow 2.7182818 (- 1 u)))))
        (quiet (- 0.8 (* 1.5 (/ hard soft) v (pow 2.7182818 (- 1 v))))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (rgba 0.24 0.93 0.81 1))
      (sdf/paint (max (- y 0.8) (- curve y)) (rgba 0.16 0.06 0.11 0.14))
      (sdf/paint (- (abs (- y quiet)) 0.018) (rgba 0.16 0.06 0.11 0.4))
      (sdf/paint (- (abs (- y curve)) 0.025) (rgba 0.16 0.06 0.11 1)))))
(def ride-kit-stick-view ()
  (v-stack :gap 0.15 (ride-kit-caption "Stick contact force / 0-1.5 ms / faint = half-velocity hit")
    (pm-ride-kit-stick :debug-name "pm-ride-kit-stick" :hardness (ride-kit-bind "stick.hardness")
      :dynamics (ride-kit-bind "stick.dynamics"))))

;; Ring of the plate's typical mode (measured $ring_rate/s at 1 kHz) with
;; decay and muffle loss; faint = the same mode once choked.
(defwidget pm-ride-kit-ring
  :width 35.3 :height 3.1 :state (decay muffle choke) :bindable (decay muffle choke)
  :shader
  (let ((time (* 3 0.5 (+ 1 (/ x aspect))))
        (rate (+ (/ $ring_rate decay) (* 40 muffle muffle)))
        (curve (- 0.8 (* 1.55 (pow 2.7182818 (* -1 rate time)))))
        (held (min time 0.5))
        (choked (- 0.8 (* 1.55 (pow 2.7182818 (- (* -1 rate time) (* 120 choke choke (- time held))))))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (rgba 0.24 0.93 0.81 1))
      (sdf/paint (max (- y 0.8) (- curve y)) (rgba 0.16 0.06 0.11 0.14))
      (sdf/paint (- (abs (- y choked)) 0.015) (rgba 0.16 0.06 0.11 0.45))
      (sdf/paint (- (abs (- y curve)) 0.025) (rgba 0.16 0.06 0.11 1)))))
(def ride-kit-ring-view ()
  (v-stack :gap 0.15 (ride-kit-caption "Plate ring / 0-3 s / faint = released at 0.5 s with Choke")
    (pm-ride-kit-ring :debug-name "pm-ride-kit-ring" :decay (ride-kit-bind "cymbal.decay")
      :muffle (ride-kit-bind "cymbal.muffle") :choke (ride-kit-bind "cymbal.choke"))))

;; Stereo scatter: every mode keeps a fixed pan; Width opens the fan.
(defwidget pm-ride-kit-width
  :width 35.3 :height 3.1 :state (amount) :bindable (amount)
  :shader
  (let ((spread (* aspect 0.9 amount))
        (fan (- (abs x) (+ 0.02 (* spread (- 0.8 (* 0.5 (+ y 0.8))))))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (rgba 0.24 0.93 0.81 1))
      (sdf/paint (max fan (- y 0.8) (- -0.8 y)) (rgba 0.16 0.06 0.11 0.25))
      (sdf/paint (- (abs (+ y 0.85)) 0.025) (rgba 0.16 0.06 0.11 1)))))
(def ride-kit-width-view ()
  (v-stack :gap 0.15 (ride-kit-caption "Mode scatter across the stereo field / 0 = recorded mono")
    (pm-ride-kit-width :debug-name "pm-ride-kit-width" :amount (ride-kit-bind "output.width"))))

(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM RIDE KIT"
    (list
      '("STICK" ("stick.hardness" "Hardness" 2 :linear) ("stick.click" "Click" 2 :linear))
      '("RING" ("cymbal.decay" "Decay" 2 :log) ("cymbal.choke" "Choke" 2 :linear))
      '("SPACE" ("output.width" "Width" 2 :linear) ("cymbal.tune" "Tune st" 1 :linear))
      '("LEVEL" ("output.tone_hz" "Tone Hz" 0 :log) ("output.gain" "Output" 2 :linear)))
    (list
      (dict :title "Strokes"
        :view (lambda () (ride-kit-strokes-view))
        :controls (lambda () '(("voice_mode" "Voicing" 0)))
        :hint "Voicing 1 = one cymbal: every hit lands on the same ringing plate.")
      (dict :title "Stick"
        :view (lambda () (ride-kit-stick-view))
        :controls (lambda () '(("stick.hardness" "Hardness" 2) ("stick.dynamics" "Vel > timbre" 2)
                               ("stick.click" "Click" 2) ("stick.click_tone" "Click tone" 2)))
        :hint "Harder sticks shorten contact; energy is redistributed, not added.")
      (dict :title "Ring"
        :view (lambda () (ride-kit-ring-view))
        :controls (lambda () '(("cymbal.decay" "Decay" 2) ("cymbal.muffle" "Muffle" 2) ("cymbal.choke" "Choke" 2) ("cymbal.wash" "Wash" 2)))
        :hint "Muffle is tape on the plate; Choke grabs it when the key is released.")
      (dict :title "Space"
        :view (lambda () (ride-kit-width-view))
        :controls (lambda () '(("output.width" "Width" 2) ("cymbal.tune" "Tune st" 1)))
        :hint "The record is one mono channel; Width scatters the plate's modes.")
      (dict :title "Output"
        :view (lambda () (eseq.effects.physical-model-surface/saron-output-view))
        :controls (lambda () '(("output.drive" "Drive" 2) ("output.tone_hz" "Tone Hz" 0) ("output.gain" "Output" 2)))
        :hint "Drive saturates the whole plate; Tone is a gentle low-pass."))))
'''
    return text.replace('$ring_rate', f'{ring_reference(data):.3g}')


def attribution(data):
    ref = data['reference']
    lines = [f'# {NAME} reference material', '',
             f'Identified from the sample library entry "{ref["title"]}" (tags: {", ".join(ref["tags"])};',
             f'`{ref["sample"]}`, SHA256 `{ref["sha256"]}`), left channel only (the ride;',
             'whispered voice sits in the right channel). The recording is commercial material in the',
             'local sample library: only fitted modal coefficients are stored; no PCM, recorded phase,',
             'or spectral frame ships with the instrument.', '',
             'Twelve strokes repeat in every octave. The C4 octave plays them at the recorded',
             'pitch; each octave up or down transposes the whole cymbal by an octave.', '',
             '| Key (any octave) | Stroke | Reference onset (s) |', '| --- | --- | ---: |']
    for i, h in enumerate(strokes(data)):
        lines.append(f'| {key_name(FIRST_KEY + i)[:-1]} | {h["name"]} | {h["onset_s"]:.3f} |')
    lines += ['', f'The other {len(SUBSTITUTES)} reference strokes are played by the installed stroke with the '
              'closest spectrum (see `tools/pm-ride/README.md`).']
    lines += ['', 'Analysis, generator and comparison: `tools/pm-ride/`.']
    return '\n'.join(lines) + '\n'


INSTRUMENT_JSON = json.dumps({'version': 1, 'run_mode': 'instrument', 'voice_controls': {'mode': 'voice_mode'}}) + '\n'


def outputs():
    analysis_bytes = (HERE/'analysis.json').read_bytes()
    data = json.loads(analysis_bytes)
    SUBSTITUTES.update(similar_keys(data))
    dsp, report = source(data, analysis_bytes)
    report['substitutes'] = {str(k): v for k, v in SUBSTITUTES.items()}
    ui = ui_source(data)
    preset_text = json.dumps(presets(), indent=2) + '\n'
    return {DEST/'dsp.lisp': dsp, DEST/'ui.lisp': ui, DEST/'instrument.json': INSTRUMENT_JSON,
            DEST/'ATTRIBUTION.md': attribution(data), DEST.parent/(NAME + '.presets'): preset_text,
            HERE/'build-report.json': json.dumps(report, indent=1) + '\n',
            # Copies beside the generator; the surface test reads these.
            HERE/'model.lisp': dsp, HERE/'ui.lisp': ui, HERE/'instrument.json': INSTRUMENT_JSON,
            HERE/'model.presets': preset_text}


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
