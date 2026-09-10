#!/usr/bin/env python3
"""Deterministically generate factory sources from physical coefficients.

The shared template is the implementation; generated standalone sources keep
factory loading and patch editing independent of these offline Python tools.
"""
import argparse
import hashlib
import json
import sys
from string import Template

import numpy as np
from scipy.optimize import linear_sum_assignment

from families import FACTORY, FAMILIES, HERE, ROOT

sys.path.insert(0, str(ROOT / "tools/audition"))
from modal_reduction import compact_modes


def coefficients(family, data):
    rows, modes, layers = len(data['units']), family.modes, len(family.strengths)
    arrays = {name: np.zeros((rows, modes)) for name in ['ratio', 'rate', 'rise', 'direct']}
    amplitude = np.zeros((rows, layers, modes))
    for row, unit in enumerate(data['units']):
        order = [unit['pitch_mode']] + [i for i in range(len(unit['frequencies_hz'])) if i != unit['pitch_mode']]
        ratio = np.array(unit['frequencies_hz'])[order]/unit['fundamental_hz']
        amp = np.array([r['modal_amplitudes'] for r in unit['recordings']])[:, order]
        energy = np.mean(amp**2, axis=0)/np.array(unit['rates_per_s'])[order]
        if row == 0:
            assigned = np.array([0] + sorted(range(1, len(order)), key=lambda i: -energy[i]))
            slots = np.arange(len(order))
        else:
            prev = arrays['ratio'][row-1]
            old_energy = np.mean(amplitude[row-1]**2, axis=0)/np.maximum(arrays['rate'][row-1], .03)
            cost = np.log(np.maximum(prev[:, None], .1)/ratio[None, :])**2
            cost += .015*np.log(np.maximum(old_energy[:, None], 1e-10)/np.maximum(energy[None, :], 1e-10))**2
            cost[prev == 0] = 2
            cost[0, 1:] = 1e6
            cost[1:, 0] = 1e6
            slots, assigned = linear_sum_assignment(cost)
        for slot, index in zip(slots, assigned):
            arrays['ratio'][row, slot] = ratio[index]
            for name, source in [('rate', 'rates_per_s'), ('rise', 'rise_seconds'), ('direct', 'direct_fraction')]:
                arrays[name][row, slot] = unit[source][order[index]]
            amplitude[row, :, slot] = amp[:, index]
    for slot in range(modes):
        present = np.flatnonzero(arrays['ratio'][:, slot] > 0)
        for name, value in [('ratio', 1), ('rate', 1), ('rise', .001), ('direct', 1)]:
            arrays[name][:, slot] = (np.interp(np.arange(rows), present, arrays[name][present, slot])
                                    if len(present) else value)
    arrays['amplitude'] = amplitude.reshape(rows*layers, modes)
    arrays['tuning'] = np.array([1200*np.log2(u['fundamental_hz']/(440*2**((u['midi']-69)/12))) for u in data['units']])
    arrays['mode_pan'] = np.sin(np.arange(modes)*2.39996323)*.7
    arrays['mode_pan'][0] = 0
    return arrays


def table_source(arrays, analysis_bytes):
    text = [';; BEGIN GENERATED CALIBRATION',
            ';; Latent Sonorities / memeshift, CC BY-NC 4.0; see ATTRIBUTION.md.',
            ';; Analysis SHA256: '+hashlib.sha256(analysis_bytes).hexdigest()]
    for name, a in arrays.items():
        assert np.isfinite(a).all()
        text.append(f'(def {name}_table (tensor @shape [{" ".join(map(str, a.shape))}] @data [')
        text.extend('  '+' '.join(f'{x:.9g}' for x in row) for row in a.reshape(-1, a.shape[-1]))
        text.append(']))')
    return '\n'.join(text)+'\n;; END GENERATED CALIBRATION'


def coordinate(values, symbol):
    terms = [f'(clip (/ (- {symbol} {a:g}) {b-a:g}) 0 1)' for a, b in zip(values, values[1:])]
    return '(+ '+' '.join(terms)+')' if terms else '0'


def replace_once(text, original, replacement):
    if text.count(original) != 1:
        raise ValueError(f'Single-pot specialization no longer matches the template: {original}')
    return text.replace(original, replacement)


def source(family, data, analysis_bytes):
    arrays, _ = compact_modes(coefficients(family, data), family.runtime_modes)
    if len(family.units) == 1:
        for name in ['ratio', 'rate', 'rise', 'direct']:
            arrays[name] = arrays[name].reshape(-1)
    text = Template((HERE/'engine.lisp.in').read_text()).substitute(
        name=family.name, body=family.body.lower(), tables=table_source(arrays, analysis_bytes),
        row_expression=coordinate([u['midi'] for u in data['units']], 'note'),
        velocity_expression=coordinate(family.velocities, 'strength'),
        minimum_velocity=family.velocities[0], last_row=len(data['units'])-1,
        layers=len(family.velocities), contact_seconds=family.contact_seconds, modes=family.runtime_modes)
    if len(family.units) == 1:
        text = '\n'.join(line for line in text.split('\n') if not line.startswith('(param register '))
        text = text.replace('(gamelan-row (+ key_note (gamelan-smooth (clip (mod register) -24 24) 8)))', '0')
        text = text.replace('(def row (gamelan-hold 0 update_tick))', '(def row 0)')
        for field in ['ratio', 'rate', 'rise', 'direct']:
            text = replace_once(text, f'(gamelan-row-mix {field}_table low_indices high_indices row_mix)', f'{field}_table')
        text = replace_once(text, '(def amplitude (mix\n  (gamelan-row-mix amplitude_table a00 a01 velocity_mix)\n  (gamelan-row-mix amplitude_table a10 a11 velocity_mix) row_mix))',
                            '(def amplitude (gamelan-row-mix amplitude_table a00 a01 velocity_mix))')
    return text


def ui_source(family, data):
    mid = data['units'][len(data['units'])//2]
    index = mid['pitch_mode']
    offset = 1200*np.log2(mid['fundamental_hz']/(440*2**((mid['midi']-69)/12)))
    register = '("voicing.register" "Register st" 1 :linear)' if len(family.units) > 1 else '("body.loss" "Upper loss" 2 :log)'
    loss = '("body.loss" "Upper-mode loss" 2)' if len(family.units) > 1 else ''
    key_hint = 'Measured MIDI keys: '+', '.join(str(u['midi']) for u in data['units'])+'.'
    if len(family.units) == 1:
        key_hint += ' Other pitches transpose this pot.'
    return f'''(defsynth-ui
  (eseq.effects.physical-model-surface/panel "{family.name.upper()}"
    (list
      '("MALLET" ("mallet.hardness" "Hardness" 2 :linear) ("mallet.contact" "Contact" 2 :log))
      '("{family.body.upper()}" ("body.decay" "Decay" 2 :log) ("body.bloom" "Bloom" 2 :linear))
      '("MODES" ("body.inharmonicity" "Inharmonic" 2 :linear) {register})
      '("DAMPING & LEVEL" ("damper.touch" "Hand damp" 2 :linear) ("output.gain" "Output" 2 :linear)))
    (list
      (dict :title "Mallet"
        :view (lambda () (eseq.effects.physical-model-surface/gamelan-mallet-view {family.contact_seconds*12000:g}))
        :controls (lambda () '(("mallet.spread" "Mallet spread" 2) ("mallet.dynamics" "Vel > timbre" 2)))
        :hint "{len(family.strengths)} measured strike strengths; continuous force and color.")
      (dict :title "Resonance"
        :view (lambda () (eseq.effects.physical-model-surface/gamelan-body-view
          {mid['rates_per_s'][index]:.6g} {mid['rise_seconds'][index]:.6g} {mid['direct_fraction'][index]:.6g}))
        :controls (lambda () '({loss} ("body.color" "Spectral color" 2)))
        :hint "Representative measured mode: resonance build-up and natural loss.")
      (dict :title "Tuning"
        :view (lambda () (eseq.effects.physical-model-surface/gamelan-tuning-view {offset:.6g}))
        :controls (lambda () '(("tuning.amount" "Recorded tuning" 2) ("tuning.tune" "Tune cents" 1)))
        :hint "{key_hint}")
      (dict :title "Damping"
        :view (lambda () (eseq.effects.physical-model-surface/gamelan-damper-view {mid['rates_per_s'][index]:.6g}))
        :controls (lambda () '(("damper.release_s" "Release seconds" 2) ("damper.lift" "Hand lift" 2)))
        :hint "Hand lift lets key-up ring naturally; Hand damp mutes.")
      (dict :title "Output"
        :view (lambda () (eseq.effects.physical-model-surface/saron-output-view))
        :controls (lambda () '(("output.width" "Stereo spread" 2) ("output.drive" "Drive" 2) ("output.tone_hz" "Tone Hz" 0)))
        :hint "Stereo spread places the modes across the panorama."))))
'''


def presets(family):
    defaults = {'mallet.hardness': .5, 'mallet.contact': 1., 'mallet.spread': 0., 'mallet.dynamics': 1.,
        'body.decay': 1., 'body.loss': 1., 'body.inharmonicity': 1., 'body.bloom': 1., 'body.color': 0.,
        'tuning.amount': 1., 'tuning.tune': 0., 'damper.touch': 0., 'damper.release_s': .4, 'damper.lift': 0.,
        'output.width': 0., 'output.drive': 0., 'output.tone_hz': 20000., 'output.gain': 1.}
    if len(family.units) > 1:
        defaults['voicing.register'] = 0.
    variants = [('reference', family.name.removeprefix('PM ')+' Reference', {}),
                ('open', 'Open Ring', {'damper.lift': 1., 'output.width': .4}),
                ('felt', 'Soft Contact', {'mallet.hardness': .2, 'mallet.contact': 1.6, 'mallet.spread': .15}),
                ('muted', 'Hand Muted', {'damper.touch': .25, 'body.decay': .65, 'mallet.hardness': .65}),
                ('harmonic', 'Harmonic Metal', {'body.inharmonicity': 0., 'body.decay': 1.4, 'output.width': .6}),
                ('bloom', 'Long Bloom', {'body.bloom': 1.8, 'body.decay': 1.7, 'damper.lift': 1., 'output.width': .7})]
    return {'version': 1, 'engine_name': 'Physical Models/'+family.name,
            'source_file': f'instruments/Physical Models/{family.name}/dsp.lisp',
            'presets': [{'id': slug, 'name': name, 'base_note_offset': 0, 'params': defaults | values}
                        for slug, name, values in variants]}


def outputs(slug):
    family = FAMILIES[slug]
    analysis_bytes = (HERE/f'{slug}-analysis.json').read_bytes()
    data = json.loads(analysis_bytes)
    credits = (HERE/'ATTRIBUTION.txt').read_text()
    attribution = f'# {family.name} reference material\n\n'
    attribution += f'Calibrated from {sum(len(u["recordings"]) for u in data["units"])} **{family.prefix}** recordings.\n'
    attribution += f'Exact filenames and SHA256 hashes: `tools/pm-gamelan/{slug}-analysis.json`.\n\n'+credits
    _, reduction = compact_modes(coefficients(family, data), family.runtime_modes)
    result = {HERE/f'{slug}-reduction.json': json.dumps(reduction, indent=2)+'\n',
              family.source: source(family, data, analysis_bytes),
              family.source.parent/'ui.lisp': ui_source(family, data),
              family.source.parent/'ATTRIBUTION.md': attribution,
              FACTORY/(family.name+'.presets'): json.dumps(presets(family), indent=2)+'\n'}
    for page in range(5):
        fixture = f'(capture-project (track :instrument "factory:Physical Models/{family.name}"))\n'
        if page:
            fixture += f'(def capture-after-sync ()\n  ((eseq.effects.custom-ui-sections/ui-section-select-callback {page}) false))\n'
        result[ROOT/f'crates/sequencer/ui/capture-fixtures/pm-{slug}-{page}.lisp'] = fixture
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('families', nargs='*', choices=list(FAMILIES))
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    for slug in args.families or FAMILIES:
        for path, text in outputs(slug).items():
            if args.check:
                assert path.read_text() == text, f'Regenerate {path}'
            else:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(text)
        print('Verified' if args.check else 'Generated', FAMILIES[slug].name)


if __name__ == '__main__':
    main()
