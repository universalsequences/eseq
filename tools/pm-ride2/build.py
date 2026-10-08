#!/usr/bin/env python3
"""Deterministically generate PM Ride release 2 from analysis.json.

Release 1 (the reduced-plate model of tools/pm-cymbals) stays frozen under
content/instruments/Physical Models/PM Ride/versions/1; this writes the
current release at the top of the lineage folder.
"""
import argparse
import hashlib
import json
from pathlib import Path
from string import Template

import numpy as np

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
NAME = 'PM Ride'
LINEAGE = 'Physical Models/' + NAME
DEST = ROOT/'content/instruments'/LINEAGE
FIRST_KEY = 60                 # Tracking pivot: C4 plays the recorded pitch
CONTACT_SECONDS = 0.00025      # reference stick-contact convention, not a measured duration
CLICK_Q = 1.5
WASH_Q = 3.0                   # per stage; two stages make one third-octave band
LOSS_REFERENCE_HZ = 1000.0     # Muffle/Choke loss grows with sqrt(f / this)
BELL_PARTIALS = 12             # each cymbal's strongest ring partials, reweighted by Bell
LEVEL_SECONDS = 0.5            # cymbals are matched in energy over their first half second
OUTPUT_TRIM = 0.55             # the hottest cymbal peaks near 0.5 at velocity 1 (fixed headroom, no limiter)
DEFAULT_CHARACTER = 2
PAN_SEED = 20261001
CALIBRATION = HERE/'noise-calibration.json'


def references(data):
    return list(data['references'].items())


def cymbal_energy(ref, seconds=LEVEL_SECONDS):
    """Energy of the identified cymbal over its first `seconds` (modes and wash)."""
    total = 0.0
    for part in ('ring', 'attack'):
        r = np.array(ref[part]['rate_per_s'])
        a2 = (np.array(ref[part]['sin_cos'])**2).sum(1)
        total += float((a2/(4*r)*(1 - np.exp(-2*r*seconds))).sum())
    for w in ref['wash']:
        for kind in ('fast', 'slow'):
            r = w[f'{kind}_rate_per_s']
            total += w[f'{kind}_power']/(2*r)*(1 - np.exp(-2*r*seconds))
    return total


def level_gains(data):
    """Amplitude gain per cymbal so every Character plays at the same energy."""
    energies = np.array([cymbal_energy(ref) for _, ref in references(data)])
    target = float(np.exp(np.log(energies).mean()))
    return OUTPUT_TRIM*np.sqrt(target/energies)


def coefficients(data):
    refs = references(data)
    modes = len(refs[0][1]['ring']['hz']) + len(refs[0][1]['attack']['hz'])
    bands = len(refs[0][1]['wash'])
    calibration = json.loads(CALIBRATION.read_text()) if CALIBRATION.exists() else {}
    gains = level_gains(data)
    t = {k: np.zeros((len(refs), modes)) for k in ['frequency', 'rate', 'magnitude', 'sine', 'cosine', 'bell']}
    t['pan'] = np.random.default_rng(PAN_SEED).uniform(-1, 1, modes)
    for k in ['click', 'click_decay', 'click_hz']:
        t[k] = np.zeros(len(refs))
    t['wash_hz'] = np.array([np.sqrt(w['band_hz'][0]*w['band_hz'][1]) for w in refs[0][1]['wash']])
    for k in ['wash_buildup', 'wash_fast_rate', 'wash_slow_rate', 'wash_fast_power', 'wash_slow_power']:
        t[k] = np.zeros((len(refs), bands))
    report = []
    for row, (slug, ref) in enumerate(refs):
        assert len(ref['ring']['hz']) + len(ref['attack']['hz']) == modes and len(ref['wash']) == bands, slug
        g = gains[row]
        sc = np.array(ref['ring']['sin_cos'] + ref['attack']['sin_cos'])*g
        t['frequency'][row] = ref['ring']['hz'] + ref['attack']['hz']
        t['rate'][row] = ref['ring']['rate_per_s'] + ref['attack']['rate_per_s']
        t['sine'][row], t['cosine'][row] = sc[:, 0], sc[:, 1]
        t['magnitude'][row] = np.hypot(sc[:, 0], sc[:, 1])
        ring = len(ref['ring']['hz'])
        t['bell'][row, np.argsort(-t['magnitude'][row, :ring])[:BELL_PARTIALS]] = 1.0
        c = ref['contact']
        t['click'][row] = c['click_rms']*g*calibration.get('click', {}).get(slug, 1.0)
        t['click_decay'][row] = c['click_decay_s']
        t['click_hz'][row] = c['centre_hz']
        wash_gain = np.array(calibration.get('wash', {}).get(slug, [1.0]*bands))
        t['wash_buildup'][row] = [1/w['buildup_s'] for w in ref['wash']]
        t['wash_fast_rate'][row] = [w['fast_rate_per_s'] for w in ref['wash']]
        t['wash_slow_rate'][row] = [w['slow_rate_per_s'] for w in ref['wash']]
        t['wash_fast_power'][row] = np.array([w['fast_power'] for w in ref['wash']])*g**2*wash_gain
        t['wash_slow_power'][row] = np.array([w['slow_power'] for w in ref['wash']])*g**2*wash_gain
        report.append({'slug': slug, 'name': ref['name'], 'file': ref['file'], 'level_gain_db': round(float(20*np.log10(g)), 2),
                       'identified_ring_poles': ref['ring']['identified_poles'],
                       'kept_ring_energy_fraction': ref['ring']['kept_energy_fraction']})
    for k, v in t.items():
        assert np.isfinite(v).all(), k
    return t, {'modes': modes, 'ring_modes': len(refs[0][1]['ring']['hz']), 'attack_modes': len(refs[0][1]['attack']['hz']),
               'wash_bands': bands, 'cymbals': report}


def table_source(tables, analysis_bytes):
    text = [';; BEGIN GENERATED CALIBRATION',
            ';; Identified from Donit\'s cymbal recordings; see ATTRIBUTION.md.',
            ';; Analysis SHA256: '+hashlib.sha256(analysis_bytes).hexdigest()]
    for name, a in tables.items():
        text.append(f'(def {name}_table (tensor @shape [{" ".join(map(str, a.shape))}] @data [')
        rows = a.reshape(-1, a.shape[-1]) if a.ndim > 1 else a.reshape(1, -1)
        for row in rows:
            for i in range(0, len(row), 16):
                text.append('  '+' '.join(f'{x:.7g}' for x in row[i:i + 16]))
        text.append(']))')
    return '\n'.join(text)+'\n;; END GENERATED CALIBRATION'


def source(data, analysis_bytes):
    tables, report = coefficients(data)
    dropped = np.mean([r['identified_ring_poles'] for r in report['cymbals']]) - report['ring_modes']
    text = Template((HERE/'engine.lisp.in').read_text()).substitute(
        tables=table_source(tables, analysis_bytes), modes=report['modes'], ring=report['ring_modes'],
        attack=report['attack_modes'], cymbals=len(report['cymbals']), max_character=len(report['cymbals']) - 1,
        default_character=DEFAULT_CHARACTER, dropped=int(round(dropped, -2)), wash_bands=report['wash_bands'],
        wash_q=WASH_Q, first_key=FIRST_KEY, contact_seconds=CONTACT_SECONDS, click_q=CLICK_Q,
        loss_reference_hz=LOSS_REFERENCE_HZ)
    return text, report


def presets(data):
    defaults = {'cymbal.character': float(DEFAULT_CHARACTER), 'cymbal.bell': 1., 'cymbal.size': 1., 'cymbal.tune': 0.,
                'cymbal.tracking': 0., 'stick.hardness': .5, 'stick.dynamics': .5, 'stick.click': 1.,
                'stick.click_tone': 0., 'ring.decay': 1., 'ring.muffle': 0., 'ring.wash': 1., 'ring.choke': 0.,
                'output.width': .3, 'output.drive': 0., 'output.tone_hz': 20000., 'output.gain': 1., 'voice_mode': 1.}
    slugs = [slug for slug, _ in references(data)]
    variants = [('reference', 'Classic Ride', {}),
                ('dark', 'Dark Ride', {'cymbal.character': float(slugs.index('dark')), 'stick.hardness': .4}),
                ('warm', 'Warm Ride', {'cymbal.character': float(slugs.index('warm'))}),
                ('dry', 'Dry Ride', {'cymbal.character': float(slugs.index('dry')), 'ring.decay': .8}),
                ('bright', 'Bright Ride', {'cymbal.character': float(slugs.index('bright'))}),
                ('crisp', 'Crisp Ride', {'cymbal.character': float(slugs.index('crisp')), 'stick.hardness': .6}),
                ('bell', 'Bell Ride', {'cymbal.bell': 2.2, 'stick.hardness': .7, 'ring.wash': .7}),
                ('taped', 'Taped Ride', {'ring.muffle': .35, 'ring.decay': .6}),
                ('grab', 'Grab Ride', {'ring.choke': .6})]
    return {'version': 1, 'engine_name': LINEAGE, 'source_file': f'instruments/{LINEAGE}/dsp.lisp',
            'presets': [{'id': slug, 'name': name, 'base_note_offset': 0, 'params': defaults | values}
                        for slug, name, values in variants]}


def ring_reference(data):
    """Energy-weighted median ring loss (1/s) over every cymbal: the ring widget's curve."""
    rates, energies = [], []
    for _, ref in references(data):
        r = np.array(ref['ring']['rate_per_s'])
        rates += list(r)
        energies += list((np.array(ref['ring']['sin_cos'])**2).sum(1)/(2*r))
    order = np.argsort(rates)
    cumulative = np.cumsum(np.array(energies)[order])/np.sum(energies)
    return float(np.array(rates)[order][np.searchsorted(cumulative, 0.5)])


def ui_source(data):
    names = [f'{i}  {ref["name"]}' for i, (_, ref) in enumerate(references(data))]
    columns = [names[i:i + 3] for i in range(0, len(names), 3)]
    stack = '\n'.join('          (v-stack :gap 0.05 ' + ' '.join(f'(ride-row "{n}")' for n in col) + ')' for col in columns)
    text = f'''(def ride-row (text)
  (label text :width 17.4 :height 0.56 :v-align :center :font-size 8.5 :color (rgba 0.16 0.06 0.11 1) :bg :transparent))

(def ride-caption (text)
  (label text :width 35.3 :height 0.45 :v-align :center :font-size 8.5
    :color (rgba 0.16 0.06 0.11 1) :bg :transparent))

(def ride-cymbals-view ()
  (v-stack :gap 0.15
    (ride-caption "Character / six measured rides, dark to bright; never blended")
    (h-stack :gap 0.2
{stack})))

(def ride-bind (name) (eseq.effects.physical-model-surface/bind name))

;; Normalized two-pole stick force over 0-1.5 ms, as the DSP computes it.
(defwidget pm-ride-stick
  :width 35.3 :height 3.1 :state (hardness dynamics)
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
(def ride-stick-view ()
  (v-stack :gap 0.15 (ride-caption "Stick contact force / 0-1.5 ms / faint = half-velocity hit")
    (pm-ride-stick :debug-name "pm-ride-stick" :hardness (ride-bind "stick.hardness")
      :dynamics (ride-bind "stick.dynamics"))))

;; Ring of a typical mode (measured $ring_rate/s) with decay and muffle loss;
;; faint = the same mode once choked.
(defwidget pm-ride-ring
  :width 35.3 :height 3.1 :state (decay muffle choke)
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
(def ride-ring-view ()
  (v-stack :gap 0.15 (ride-caption "Plate ring / 0-3 s / faint = released at 0.5 s with Choke")
    (pm-ride-ring :debug-name "pm-ride-ring" :decay (ride-bind "ring.decay")
      :muffle (ride-bind "ring.muffle") :choke (ride-bind "ring.choke"))))

;; Stereo scatter: every mode keeps a fixed pan; Width opens the fan.
(defwidget pm-ride-width
  :width 35.3 :height 3.1 :state (amount)
  :shader
  (let ((spread (* aspect 0.9 amount))
        (fan (- (abs x) (+ 0.02 (* spread (- 0.8 (* 0.5 (+ y 0.8))))))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (rgba 0.24 0.93 0.81 1))
      (sdf/paint (max fan (- y 0.8) (- -0.8 y)) (rgba 0.16 0.06 0.11 0.25))
      (sdf/paint (- (abs (+ y 0.85)) 0.025) (rgba 0.16 0.06 0.11 1)))))
(def ride-width-view ()
  (v-stack :gap 0.15 (ride-caption "Mode scatter across the stereo field / 0 = recorded mono")
    (pm-ride-width :debug-name "pm-ride-width" :amount (ride-bind "output.width"))))

(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM RIDE"
    (list
      '("CYMBAL" ("cymbal.character" "Character" 0 :linear) ("cymbal.bell" "Bell" 2 :linear))
      '("STICK" ("stick.hardness" "Hardness" 2 :linear) ("stick.click" "Click" 2 :linear))
      '("RING" ("ring.decay" "Decay" 2 :log) ("ring.choke" "Choke" 2 :linear))
      '("LEVEL" ("output.width" "Width" 2 :linear) ("output.gain" "Output" 2 :linear)))
    (list
      (dict :title "Cymbal"
        :view (lambda () (ride-cymbals-view))
        :controls (lambda () '(("cymbal.character" "Character" 0) ("cymbal.bell" "Bell" 2) ("cymbal.size" "Size" 2)
                               ("cymbal.tune" "Tune st" 1) ("cymbal.tracking" "Tracking" 2) ("voice_mode" "Voicing" 0)))
        :hint "Voicing 1 = one cymbal: every hit lands on the same ringing plate.")
      (dict :title "Stick"
        :view (lambda () (ride-stick-view))
        :controls (lambda () '(("stick.hardness" "Hardness" 2) ("stick.dynamics" "Vel > timbre" 2)
                               ("stick.click" "Click" 2) ("stick.click_tone" "Click tone" 2)))
        :hint "Harder sticks shorten contact; energy is redistributed, not added.")
      (dict :title "Ring"
        :view (lambda () (ride-ring-view))
        :controls (lambda () '(("ring.decay" "Decay" 2) ("ring.muffle" "Muffle" 2) ("ring.choke" "Choke" 2) ("ring.wash" "Wash" 2)))
        :hint "Muffle is tape on the plate; Choke grabs it when the key is released.")
      (dict :title "Space"
        :view (lambda () (ride-width-view))
        :controls (lambda () '(("output.width" "Width" 2)))
        :hint "The recordings are mono; Width scatters the plate's modes.")
      (dict :title "Output"
        :view (lambda () (eseq.effects.physical-model-surface/saron-output-view))
        :controls (lambda () '(("output.drive" "Drive" 2) ("output.tone_hz" "Tone Hz" 0) ("output.gain" "Output" 2)))
        :hint "Drive saturates the whole plate; Tone is a gentle low-pass."))))
'''
    return text.replace('$ring_rate', f'{ring_reference(data):.3g}')


def attribution(data):
    lines = [f'# {NAME} (release 2) reference material', '',
             'Identified from rides in **Acoustic Cymbals Vol.1 by Donit**, supplied locally in',
             '`samples-to-analyze` (see `versions/1/ATTRIBUTION.md` for the pack\'s credit and terms).',
             'Only fitted modal coefficients are stored; no recordings, recorded phase, or sampled',
             'amplitude envelopes ship with the instrument.', '',
             '| Character | Name | Reference | SHA256 |', '| ---: | --- | --- | --- |']
    for i, (_, ref) in enumerate(references(data)):
        lines.append(f'| {i} | {ref["name"]} | `{ref["file"].rsplit("/", 1)[-1]}` | `{ref["sha256"][:16]}…` |')
    lines += ['', 'Analysis, generator and comparison: `tools/pm-ride2/`. Release 1 (the reduced-plate',
              'model) is frozen in `versions/1/`.']
    return '\n'.join(lines) + '\n'


INSTRUMENT_JSON = json.dumps({
    'version': 1, 'run_mode': 'instrument', 'voice_controls': {'mode': 'voice_mode'}, 'current': 2,
    'releases': {'1': {'path': 'versions/1', 'name': LINEAGE}, '2': {'path': '.', 'name': LINEAGE}}}, indent=2) + '\n'


def outputs():
    analysis_bytes = (HERE/'analysis.json').read_bytes()
    data = json.loads(analysis_bytes)
    dsp, report = source(data, analysis_bytes)
    ui = ui_source(data)
    preset_text = json.dumps(presets(data), indent=2) + '\n'
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
