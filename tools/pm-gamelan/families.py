"""Explicit recording groups and reference-key conventions, not runtime assets."""
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
SAMPLES = ROOT / 'samples-to-analyze/gamelan'
FACTORY = ROOT / 'content/instruments/Physical Models'


@dataclass(frozen=True)
class Family:
    name: str
    prefix: str
    # Recording label, MIDI key, approximate pitch used only to select a mode.
    units: tuple
    strengths: tuple = ('medium', 'harder', 'hardest')
    velocities: tuple = (.6, .8, 1.)
    modes: int = 32
    contact_seconds: float = .00006
    fit_seconds: float = 5.
    spectrum_seconds: float = .6
    body: str = 'Pot'
    resolve_attack: bool = False

    @property
    def source(self):
        return FACTORY / self.name / 'dsp.lisp'


FAMILIES = {
    'slenthem': Family('PM Slenthem', 'slenthem-pelog-slenthemmalletpaddedside',
        tuple(zip(map(str, range(1, 8)), (50, 52, 53, 56, 57, 58, 60),
                  (148.4, 163.1, 174.2, 203.5, 221.5, 236.5, 263.1))),
        modes=16, contact_seconds=.0003, fit_seconds=8., body='Bar & tube'),
    'bonang': Family('PM Bonang', 'bonangbarung-slendro-bonangmalletwoodenside',
        # The pack includes an additional damaged pot. It gets its own adjacent
        # key, not a replacement for intact pot 2 or an averaged calibration.
        tuple(zip(('1l', '2l', '3l', '5l', '6l', '1', '2-broken', '2', '3', '5', '6', '1h', '2h'),
                  (60, 63, 65, 68, 70, 72, 74, 75, 77, 80, 82, 84, 87),
                  (272.5, 313.7, 358.1, 412.4, 473.5, 546.4, 612.9, 624.9, 715.8, 815.8, 933., 1089.7, 1251.4)))),
    'slenthem-slendro': Family('PM Slenthem Slendro', 'slenthem-slendro-slenthemmalletwoodenside',
        tuple(zip(('6l', '1', '2', '3', '5', '6', '1h'), (46, 48, 51, 53, 56, 58, 60),
                  (117.5, 134.6, 155.1, 177.1, 203.6, 235.4, 270.))),
        modes=24, fit_seconds=8., body='Bar & tube', resolve_attack=True),
    'kempyang': Family('PM Kempyang', 'kempyang-slendro-bonangmalletwoodenside',
        (('', 82, 927.7),), modes=24),
    'kethuk': Family('PM Kethuk', 'kethuk-slendro-bonangmalletwoodenside',
        (('', 58, 238.3),)),
}


def references(family, label):
    stem = family.prefix + ('-' + label if label else '')
    result = []
    for strength in family.strengths:
        paths = list(SAMPLES.glob(f'*__*-{stem}-{strength}.wav'))
        if len(paths) != 1:
            raise ValueError(f'Expected one {stem}-{strength}: {paths}')
        result.append(paths[0])
    return result
