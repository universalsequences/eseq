#!/usr/bin/env python3
"""Generate three self-contained factory models from identified coefficients."""
import argparse
import copy
import hashlib
import json

import numpy as np
from scipy.optimize import linear_sum_assignment

from common import FACTORY, HERE, NAMES, ROOT, digest
from engine import PARAMS, basis_source, source

FIELDS = {'material': 8, 'band_gains': 18, 'mode_frequencies': 8,
          'mode_rates': 8, 'mode_gains': 8}
VOICE_DEFAULTS = {'crash': {'bell': .5}, 'ride': {'bell': .25}, 'hihat': {'bell': .5}}


def match_modes(records):
    records = copy.deepcopy(records)
    for previous, current in zip(records, records[1:]):
        cost = np.log(np.array(previous['mode_frequencies'])[:, None]
                      /np.array(current['mode_frequencies'])[None, :])**2
        _, order = linear_sum_assignment(cost)
        for field in ['mode_frequencies', 'mode_rates', 'mode_gains']:
            current[field] = np.array(current[field])[order].tolist()
    return records


def tables(records, prefix):
    text = []
    for field, width in FIELDS.items():
        rows = np.array([r['region_rates']+[r['contact_s'], r['direct']] if field == 'material' else r[field] for r in records]).reshape(len(records), width)
        assert np.isfinite(rows).all() and (rows >= 0).all(), field
        # Flat data and shared row offsets avoid implicit tensor wrapping rules.
        text.append(f'(def {prefix}_{field} (tensor @shape [{rows.size}] @data [')
        text.extend('  '+' '.join(f'{v:.10g}' for v in row) for row in rows)
        text.append(']))')
    return '\n'.join(text)


def voicing(records, prefix):
    n = len(records)
    text = f'''(def {prefix}_row (* character_v {n-1}))
(def {prefix}_lo (floor {prefix}_row))
(def {prefix}_hi (min {n-1} (+ {prefix}_lo 1)))
(def {prefix}_mix (- {prefix}_row {prefix}_lo))
'''
    for field, width in FIELDS.items():
        text += f'(def {prefix}_{field}_v (mix (gather {prefix}_{field} (+ (iota {width}) (* {prefix}_lo {width}))) (gather {prefix}_{field} (+ (iota {width}) (* {prefix}_hi {width}))) {prefix}_mix))\n'
    return text


def dsp(slug, calibration):
    records = calibration['references']
    if slug == 'hihat':
        by_file = {r['source'].rsplit('/', 1)[-1]: r for r in records}
        # Three deliberate stick-hat voices. Pedal/muted references must not
        # silently become the default stick sound in a centroid-sorted list.
        closed = match_modes([by_file[n] for n in ['Hihat - Close_7.wav', 'Hihat - Close_8.wav', 'Hihat - Close_3.wav']])
        opened = match_modes([by_file[n] for n in ['Hihat - Open_4.wav', 'Hihat - Open_3.wav', 'Hihat - Open.wav']])
        # The pack gives no matched pairs. Match nearby spectral mode slots for
        # smooth interpolation; this makes no claim about shared object identity.
        for i, row in enumerate(opened):
            target = np.array(closed[round(i*(len(closed)-1)/max(1, len(opened)-1))]['mode_frequencies'])
            _, order = linear_sum_assignment(np.log(target[:, None]/np.array(row['mode_frequencies'])[None, :])**2)
            for field in ['mode_frequencies', 'mode_rates', 'mode_gains']:
                row[field] = np.array(row[field])[order].tolist()
        groups = {'closed': closed, 'open': opened}
        extra = [('openness', 'contact', 0, 0, 1, True)]
    else:
        groups = {'voice': match_modes(records)}
        extra = []
    table_text = ';; Calibration SHA256: '+digest(HERE/f'{slug}-calibration.json')+'\n'
    table_text += '\n'.join(tables(rows, prefix) for prefix, rows in groups.items())
    text = '(def character_v (event-hold (cymbal-smooth (clip (mod character) 0 1) 12) tick))\n'
    text += '\n'.join(voicing(rows, prefix) for prefix, rows in groups.items())
    if slug == 'hihat':
        text += '(def openness_v (event-hold (cymbal-smooth (clip (mod openness) 0 1) 2) tick))\n'
    for field in FIELDS:
        expression = (f'(mix closed_{field}_v open_{field}_v openness_v)' if slug == 'hihat'
                      else f'voice_{field}_v')
        text += f'(def {field} {expression})\n'
    for i, name in enumerate([f'base_rate{i}' for i in range(6)]+['base_contact_s', 'base_direct']):
        value = f'(sample material {i/8:g})'
        if name == 'base_direct':
            value = f'(latch {value} tick)'
        text += f'(def {name} {value})\n'
    # Default pickups favor diffuse shell motion over isolated modes.
    # Bell remains available across its full range for other striking positions.
    return source(NAMES[slug], table_text, text, extra_params=extra, hat=slug == 'hihat',
                  defaults=VOICE_DEFAULTS.get(slug))


def ui(slug):
    hat = slug == 'hihat'
    open_control = '("contact.openness" "Openness" 2 :linear)' if hat else '("body.wash" "Wash" 2 :linear)'
    wash_detail = '("body.wash" "Wash" 2)' if hat else ''
    return f'''(defsynth-ui
  (eseq.effects.physical-model-surface/panel "{NAMES[slug].upper()}"
    (list
      '("METAL" ("voicing.character" "Voicing" 2 :linear) ("body.size" "Size" 2 :log))
      '("RING" ("body.decay" "Decay" 2 :log) ("body.damping" "Upper loss" 2 :log))
      '("CONTACT" ("stick.hardness" "Hardness" 2 :linear) {open_control})
      '("BELL & LEVEL" ("body.bell" "Bell" 2 :linear) ("output.gain" "Output" 2 :linear)))
    (list
      (dict :title "Metal"
        :view (lambda () (eseq.effects.physical-model-surface/cymbal-body-view))
        :controls (lambda () '(("tuning.tracking" "Key tracking" 2)))
        :hint "Voicing follows the reference collection; Size changes the body.")
      (dict :title "Ring"
        :view (lambda () (eseq.effects.physical-model-surface/cymbal-loss-view))
        :controls (lambda () '(("contact.touch" "Choke" 2)))
        :hint "Choke dissipates the ringing state; key-up leaves it ringing.")
      (dict :title "Contact"
        :view (lambda () (eseq.effects.physical-model-surface/cymbal-contact-view {'true' if hat else 'false'}))
        :controls (lambda () '({wash_detail} ("output.color" "Spectral color" 2)))
        :hint "{'Openness changes contact losses and the closed/open voicing.' if hat else 'Finite stick contact excites a freely ringing metal body.'}")
      (dict :title "Output"
        :view (lambda () (eseq.effects.physical-model-surface/cymbal-output-view))
        :controls (lambda () '(("output.width" "Stereo spread" 2)))
        :hint "Bell and Wash balance resolved and dense body resonances."))))
'''


def presets(slug):
    defaults = {group+'.'+name: VOICE_DEFAULTS.get(slug, {}).get(name, value)
                for name, group, value, *_ in PARAMS}
    if slug == 'hihat':
        defaults['contact.openness'] = 0
        variants = [('closed', 'Closed Hat', {}), ('open', 'Open Hat', {'contact.openness': 1}),
                    ('loose', 'Loose Hat', {'contact.openness': .3, 'body.decay': 1.3}),
                    ('soft', 'Soft Tick', {'body.decay': .6, 'body.bell': .6, 'stick.hardness': .15}),
                    ('tight', 'Tight Bright', {'body.decay': .55, 'voicing.character': .85, 'contact.touch': .08}),
                    ('wide', 'Wide Shimmer', {'contact.openness': 1, 'body.decay': 1.8, 'output.width': .7})]
    else:
        variants = [('reference', NAMES[slug][3:]+' Reference', {}),
                    ('dark', 'Dark Bronze', {'voicing.character': .15, 'body.size': 1.15}),
                    ('bright', 'Bright Metal', {'voicing.character': .85, 'stick.hardness': .7}),
                    ('bell', 'Bell Focus', {'body.wash': .4, 'body.bell': 1.5, 'stick.hardness': .8}),
                    ('muted', 'Hand Muted', {'contact.touch': .2, 'body.decay': .7}),
                    ('wash', 'Wide Wash', {'body.wash': 1.3, 'body.bell': .5, 'body.decay': 1.6, 'output.width': .65})]
    return {'version': 1, 'engine_name': 'Physical Models/'+NAMES[slug],
            'source_file': f'instruments/Physical Models/{NAMES[slug]}/dsp.lisp',
            'presets': [{'id': key, 'name': name, 'base_note_offset': 0, 'params': defaults | values}
                        for key, name, values in variants]}


def outputs(slug):
    path = HERE/f'{slug}-calibration.json'
    calibration = json.loads(path.read_text())
    analysis = json.loads((HERE/f'{slug}-analysis.json').read_text())
    assert len(calibration['references']) == len(analysis['references']), 'Incomplete calibration'
    expected_basis = hashlib.sha256(basis_source(hat=slug == 'hihat').encode()).hexdigest()
    assert calibration.get('basis_source_sha256') == expected_basis, 'Calibration must be refined against this exact engine before generation'
    folder = FACTORY/NAMES[slug]
    result = {folder/'dsp.lisp': dsp(slug, calibration), folder/'ui.lisp': ui(slug),
              FACTORY/(NAMES[slug]+'.presets'): json.dumps(presets(slug), indent=2)+'\n',
              folder/'ATTRIBUTION.md': (HERE/'ATTRIBUTION.md').read_text()}
    for page in range(4):
        fixture = f'(capture-project (track :instrument "factory:Physical Models/{NAMES[slug]}"))\n'
        if page:
            fixture += f'(def capture-after-sync ()\n  ((eseq.effects.custom-ui-sections/ui-section-select-callback {page}) false))\n'
        result[ROOT/f'crates/sequencer/ui/capture-fixtures/pm-{slug}-{page}.lisp'] = fixture
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('families', nargs='*')
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--install', action='store_true', help='Install validated outputs in the live factory; default builds stay under output/staging.')
    args = parser.parse_args()
    for slug in args.families or NAMES:
        for path, text in outputs(slug).items():
            if not args.install:
                path = HERE/'output/staging'/path.relative_to(ROOT)
            if args.check:
                assert path.read_text() == text, f'Regenerate {path}'
            else:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(text)
        print('Verified' if args.check else 'Generated', NAMES[slug])


if __name__ == '__main__':
    main()
