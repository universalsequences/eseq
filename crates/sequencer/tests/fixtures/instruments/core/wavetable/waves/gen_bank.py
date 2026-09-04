#!/usr/bin/env python3
"""Generate the Wavetable instrument bank.

Output: bank.json, wave-major (`wave * 512 + sample`), shape
`[512, len(SETS) * 16]`,
matching the DGenLisp `wavetable-read` peek convention.
All waves are DC-removed and peak-normalized to 0.85.
"""
import json
import math
import random

N = 512
WAVES_PER_SET = 16
TWO_PI = 2.0 * math.pi
X = [i / N for i in range(N)]


def sine(x, h=1, ph=0.0):
    return math.sin(TWO_PI * h * x + ph)


def additive(x, partials):
    """partials: list of (harmonic, amp, phase)."""
    return sum(a * math.sin(TWO_PI * h * x + p) for h, a, p in partials)


def saw_bl(x, nh, tilt=1.0):
    return sum((1.0 / h ** tilt) * math.sin(TWO_PI * h * x) for h in range(1, nh + 1))


def square_bl(x, nh):
    return sum((1.0 / h) * math.sin(TWO_PI * h * x) for h in range(1, nh + 1, 2))


def pulse_bl(x, width, nh):
    # band-limited pulse via difference of two saws
    return saw_bl(x, nh) - saw_bl((x + width) % 1.0, nh)


def tri_bl(x, nh):
    out = 0.0
    sign = 1.0
    for h in range(1, nh + 1, 2):
        out += sign * math.sin(TWO_PI * h * x) / (h * h)
        sign = -sign
    return out


def normalize(wave):
    n = len(wave)
    dc = sum(wave) / n
    wave = [v - dc for v in wave]
    peak = max(abs(v) for v in wave) or 1.0
    return [0.85 * v / peak for v in wave]


def lerp_wave(a, b, t):
    return [(1 - t) * va + t * vb for va, vb in zip(a, b)]


def gen_set(fn):
    return [normalize([fn(w, x) for x in X]) for w in range(WAVES_PER_SET)]


PRIMES = [2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53]
COMPOSITES = [4, 6, 8, 9, 10, 12, 14, 15, 16, 18, 20, 21, 22, 24, 25, 26]


def set_basic_shapes():
    # morph sine -> triangle -> saw -> square -> narrow pulse
    anchors = [
        normalize([sine(x) for x in X]),
        normalize([tri_bl(x, 31) for x in X]),
        normalize([saw_bl(x, 48) for x in X]),
        normalize([square_bl(x, 47) for x in X]),
        normalize([pulse_bl(x, 0.12, 48) for x in X]),
    ]
    waves = []
    for w in range(WAVES_PER_SET):
        t = w / (WAVES_PER_SET - 1) * (len(anchors) - 1)
        i = min(int(t), len(anchors) - 2)
        waves.append(normalize(lerp_wave(anchors[i], anchors[i + 1], t - i)))
    return waves


def set_harmonic_series():
    return gen_set(lambda w, x: sine(x, w + 1))


def set_sub():
    # pure sine growing soft low harmonics: dark sub material
    def fn(w, x):
        t = w / 15.0
        return sine(x) + 0.5 * t * sine(x, 2) + 0.25 * t * t * sine(x, 3)
    return gen_set(fn)


def set_saw_dual():
    # two saws, second shifted by growing phase offset (comb-like detune snapshot)
    def fn(w, x):
        off = w / 15.0 * 0.5
        return saw_bl(x, 40) + saw_bl((x + off) % 1.0, 40)
    return gen_set(fn)


def set_saw_harmonics():
    return gen_set(lambda w, x: saw_bl(x, 1 + w * 4))


def set_pulse_pw():
    return gen_set(lambda w, x: pulse_bl(x, 0.5 - w / 15.0 * 0.44, 48))


def set_quad_saw():
    def fn(w, x):
        s = w / 15.0
        offs = [0.0, 0.13 * s, 0.29 * s, 0.46 * s]
        return sum(saw_bl((x + o) % 1.0, 32) for o in offs)
    return gen_set(fn)


def set_beating():
    rng = random.Random(11)
    phases = [rng.uniform(0, TWO_PI) for _ in range(4)]
    def fn(w, x):
        t = w / 15.0
        return (sine(x) + sine(x, 2, phases[0] + t * 5.2) * (0.4 + 0.5 * t)
                + sine(x, 3, phases[1] + t * 2.6) * 0.3 * t
                + sine(x, 5, phases[2] + t * 7.1) * 0.22 * t)
    return gen_set(fn)


def set_fifth_brutal():
    def fn(w, x):
        t = w / 15.0
        nh = 6 + int(t * 36)
        return saw_bl(x, nh, 1.0) + (0.4 + 0.6 * t) * saw_bl((x * 3) % 1.0, max(2, nh // 3))
    return gen_set(fn)


def _synced(x, ratio, render):
    # hard sync: slave at `ratio` x master, reset each master cycle
    return render((x * ratio) % 1.0)


def set_sync_additive():
    def fn(w, x):
        ratio = 1.0 + w / 15.0 * 4.0
        win = 0.5 - 0.5 * math.cos(TWO_PI * x)  # soften the reset edge a touch
        return _synced(x, ratio, lambda p: math.sin(TWO_PI * p)) * (0.35 + 0.65 * win)
    return gen_set(fn)


def set_sync_digital():
    def fn(w, x):
        ratio = 1.0 + w / 15.0 * 5.0
        return _synced(x, ratio, lambda p: 1.0 if p < 0.5 else -1.0) * (1.0 - 0.3 * x)
    return gen_set(fn)


def set_fm_feedback():
    def fn(w, x):
        b = w / 15.0 * 2.8
        inner = math.sin(TWO_PI * x)
        mid = math.sin(TWO_PI * x + b * inner)
        return math.sin(TWO_PI * x + b * mid)
    return gen_set(fn)


def set_fm_fold():
    def fn(w, x):
        idx = w / 15.0
        y = math.sin(TWO_PI * x + 3.2 * idx * math.sin(2 * TWO_PI * x))
        g = 1.0 + 2.5 * idx
        return 1.0 - abs(((y * g + 1.0) % 4.0) - 2.0)
    return gen_set(fn)


def set_fm_harmonics():
    def fn(w, x):
        ratio = (w % 4) + 1
        idx = 0.4 + (w // 4) * 1.1
        return math.sin(TWO_PI * x + idx * math.sin(TWO_PI * ratio * x))
    return gen_set(fn)


def _partial_stack(numbers, seed):
    rng = random.Random(seed)
    phases = [rng.uniform(0, TWO_PI) for _ in numbers]
    def fn(w, x):
        count = 1 + w  # one more partial per wave
        out = math.sin(TWO_PI * x) * 0.7
        for i in range(min(count, len(numbers))):
            h = numbers[i]
            out += math.sin(TWO_PI * h * x + phases[i]) / math.sqrt(h)
        return out
    return gen_set(fn)


def set_primes():
    return _partial_stack(PRIMES, 7)


def set_no_primes():
    return _partial_stack(COMPOSITES, 13)


def set_galactica():
    rng = random.Random(42)
    amps = [[rng.random() ** 2 for _ in range(24)] for _ in range(4)]
    phs = [[rng.uniform(0, TWO_PI) for _ in range(24)] for _ in range(4)]
    def fn(w, x):
        t = w / 15.0 * 3.0
        i = min(int(t), 2)
        ft = t - i
        out = 0.0
        for h in range(24):
            a = (1 - ft) * amps[i][h] + ft * amps[i + 1][h]
            p = (1 - ft) * phs[i][h] + ft * phs[i + 1][h]
            out += a * math.sin(TWO_PI * (h + 1) * x + p) / (h + 1) ** 0.5
        return out
    return gen_set(fn)


def set_squeeze():
    def fn(w, x):
        k = 1.0 + w / 15.0 * 9.0
        p = (k * x) / (1.0 + (k - 1.0) * x)
        return math.sin(TWO_PI * p)
    return gen_set(fn)


def set_organ():
    # drawbar-style registrations getting progressively brighter
    bars = [1, 2, 3, 4, 6, 8, 10, 12, 16]
    def fn(w, x):
        t = w / 15.0
        out = 0.0
        for i, h in enumerate(bars):
            lvl = max(0.0, 1.0 - abs(i - t * (len(bars) - 1)) / 3.0)
            out += lvl * math.sin(TWO_PI * h * x)
        return out
    return gen_set(fn)


def set_noise():
    rng = random.Random(99)
    base = [rng.uniform(-1, 1) for _ in range(N)]
    waves = []
    for w in range(WAVES_PER_SET):
        # decreasing smoothing: dark rumble -> raw noise
        passes = (WAVES_PER_SET - 1 - w) * 6
        cur = base[:]
        for _ in range(passes):
            cur = [(cur[i - 1] + cur[i] + cur[(i + 1) % N]) / 3.0 for i in range(N)]
        waves.append(normalize(cur))
    return waves


# --- vocal / formant material -------------------------------------------
# Single-cycle formant synthesis: shape an additive harmonic stack with
# resonance peaks. Nominal fundamental 110 Hz, so harmonic h sits at h*110 Hz.
F0 = 110.0

# Classic male vowel formants (F1, F2, F3) in Hz.
VOWELS = {
    "A": (730, 1090, 2440),
    "E": (530, 1840, 2480),
    "I": (270, 2290, 3010),
    "O": (570, 840, 2410),
    "U": (300, 870, 2240),
}


def formant_amps(formants, bws, nh=64, source_tilt=0.35, floor=0.02):
    """Per-harmonic amplitudes: glottal-ish 1/h^tilt source through resonances."""
    amps = []
    for h in range(1, nh + 1):
        f = h * F0
        res = sum(1.0 / (1.0 + ((f - ff) / bw) ** 2) for ff, bw in zip(formants, bws))
        amps.append((floor + res) / h ** source_tilt)
    return amps


def render_additive(amps, phases):
    return [sum(a * math.sin(TWO_PI * (h + 1) * x + p)
                for h, (a, p) in enumerate(zip(amps, phases)))
            for x in X]


def _vowel_anchor_set(order, bws, seed, tilt=0.35):
    """Morph through vowel formant targets; shared phases keep the morph smooth."""
    rng = random.Random(seed)
    phases = [rng.uniform(0, TWO_PI) for _ in range(64)]
    anchors = [formant_amps(VOWELS[v], bws, source_tilt=tilt) for v in order]
    waves = []
    for w in range(WAVES_PER_SET):
        t = w / (WAVES_PER_SET - 1) * (len(anchors) - 1)
        i = min(int(t), len(anchors) - 2)
        ft = t - i
        amps = [(1 - ft) * a + ft * b for a, b in zip(anchors[i], anchors[i + 1])]
        waves.append(normalize(render_additive(amps, phases)))
    return waves


def set_vowels():
    # the classic talkbox sweep: ah -> eh -> ee -> oh -> oo
    return _vowel_anchor_set(["A", "E", "I", "O", "U"], (90, 110, 140), 21)


def set_choir():
    # wider bandwidths + detuned unison pairs around each harmonic = airy choir
    rng = random.Random(33)
    phases = [rng.uniform(0, TWO_PI) for _ in range(64)]
    phases2 = [rng.uniform(0, TWO_PI) for _ in range(64)]
    order = ["O", "A", "E", "U", "I"]
    anchors = [formant_amps(VOWELS[v], (160, 200, 260), source_tilt=0.5) for v in order]
    waves = []
    for w in range(WAVES_PER_SET):
        t = w / (WAVES_PER_SET - 1) * (len(anchors) - 1)
        i = min(int(t), len(anchors) - 2)
        ft = t - i
        amps = [(1 - ft) * a + ft * b for a, b in zip(anchors[i], anchors[i + 1])]
        body = render_additive(amps, phases)
        shimmer = render_additive([a * 0.6 for a in amps], phases2)
        waves.append(normalize([b + s for b, s in zip(body, shimmer)]))
    return waves


def set_throat():
    # growly low formants sweeping down into sub-vowel grit; saturate for bite
    def fn(w, x):
        t = w / (WAVES_PER_SET - 1)
        f1 = 700.0 - 420.0 * t
        f2 = 1300.0 - 500.0 * t
        out = 0.0
        for h in range(1, 49):
            f = h * F0
            res = (1.0 / (1.0 + ((f - f1) / 70.0) ** 2)
                   + 0.8 / (1.0 + ((f - f2) / 90.0) ** 2)
                   + 0.25 / (1.0 + ((f - 2600.0) / 200.0) ** 2))
            out += (0.03 + res) / h ** 0.2 * math.sin(TWO_PI * h * x + 0.7 * h)
        return math.tanh((1.0 + 2.5 * t) * out)
    return gen_set(fn)


def set_talk_box():
    # saw through two narrow resonances scanning in opposite directions
    def fn(w, x):
        t = w / (WAVES_PER_SET - 1)
        f1 = 250.0 + 750.0 * t
        f2 = 2600.0 - 1700.0 * t
        out = 0.0
        for h in range(1, 57):
            f = h * F0
            res = (1.0 / (1.0 + ((f - f1) / 55.0) ** 2)
                   + 0.9 / (1.0 + ((f - f2) / 70.0) ** 2))
            out += (0.015 + res) / h ** 0.3 * math.sin(TWO_PI * h * x)
        return out
    return gen_set(fn)


def set_phoneme():
    # 16 unrelated random formant snapshots: drastic AKWF-style adjacency
    rng = random.Random(77)
    waves = []
    for w in range(WAVES_PER_SET):
        formants = sorted(rng.uniform(200, 3200) for _ in range(3))
        bws = [rng.uniform(50, 180) for _ in range(3)]
        tilt = rng.uniform(0.15, 0.7)
        phases = [rng.uniform(0, TWO_PI) for _ in range(64)]
        amps = formant_amps(formants, bws, source_tilt=tilt)
        waves.append(normalize(render_additive(amps, phases)))
    return waves


def set_vox_sync():
    # hard sync windowed by a vowel envelope: sync ratio reads as a vocal formant
    def fn(w, x):
        t = w / (WAVES_PER_SET - 1)
        ratio = 2.0 + t * 9.0
        win = 0.5 - 0.5 * math.cos(TWO_PI * x)
        slave = math.sin(TWO_PI * ((x * ratio) % 1.0))
        slave2 = math.sin(TWO_PI * ((x * ratio * 1.5) % 1.0))
        return (slave + 0.5 * slave2 * t) * win ** 1.5
    return gen_set(fn)


def set_sfx_chaos():
    # fully unrelated random additive spectra per wave: sparse spikes + noise floor
    rng = random.Random(123)
    waves = []
    for w in range(WAVES_PER_SET):
        amps = [0.0] * 64
        for _ in range(rng.randint(3, 9)):
            amps[rng.randint(0, 63)] += rng.uniform(0.3, 1.0)
        for h in range(64):
            amps[h] += rng.uniform(0, 0.06) / (h + 1) ** 0.3
        phases = [rng.uniform(0, TWO_PI) for _ in range(64)]
        wave = render_additive(amps, phases)
        if rng.random() < 0.5:  # half get folded for digital nastiness
            g = rng.uniform(1.5, 3.5)
            peak = max(abs(v) for v in wave) or 1.0
            wave = [1.0 - abs(((v / peak * g + 1.0) % 4.0) - 2.0) for v in wave]
        waves.append(normalize(wave))
    return waves


def set_bit_vox():
    # vowel morph crushed to decreasing bit depths: speak-and-spell territory
    rng = random.Random(55)
    phases = [rng.uniform(0, TWO_PI) for _ in range(64)]
    order = ["A", "I", "O", "E", "U"]
    anchors = [formant_amps(VOWELS[v], (80, 100, 130), source_tilt=0.25) for v in order]
    waves = []
    for w in range(WAVES_PER_SET):
        t = w / (WAVES_PER_SET - 1) * (len(anchors) - 1)
        i = min(int(t), len(anchors) - 2)
        ft = t - i
        amps = [(1 - ft) * a + ft * b for a, b in zip(anchors[i], anchors[i + 1])]
        wave = render_additive(amps, phases)
        peak = max(abs(v) for v in wave) or 1.0
        levels = 24.0 - w  # 24 -> 9 quantization levels across the set
        wave = [round(v / peak * levels) / levels for v in wave]
        waves.append(normalize(wave))
    return waves


SETS = [
    ("Basic Shapes", set_basic_shapes),
    ("Harmonics", set_harmonic_series),
    ("Sub", set_sub),
    ("Saw Dual", set_saw_dual),
    ("Saw Harmonics", set_saw_harmonics),
    ("Pulse PW", set_pulse_pw),
    ("Quad Saw", set_quad_saw),
    ("Beating", set_beating),
    ("5th Brutal", set_fifth_brutal),
    ("Sync Additive", set_sync_additive),
    ("Sync Digital", set_sync_digital),
    ("FM Feedback", set_fm_feedback),
    ("FM Fold", set_fm_fold),
    ("FM Harmonics", set_fm_harmonics),
    ("Primes", set_primes),
    ("No Primes", set_no_primes),
    ("Galactica", set_galactica),
    ("Squeeze", set_squeeze),
    ("Organ", set_organ),
    ("Noise", set_noise),
    ("Vowels", set_vowels),
    ("Choir", set_choir),
    ("Throat", set_throat),
    ("Talk Box", set_talk_box),
    ("Phoneme", set_phoneme),
    ("Vox Sync", set_vox_sync),
    ("SFX Chaos", set_sfx_chaos),
    ("Bit Vox", set_bit_vox),
]


def main():
    data = []
    for name, fn in SETS:
        waves = fn()
        assert len(waves) == WAVES_PER_SET, name
        for wave in waves:
            assert len(wave) == N, name
            data.extend(round(v, 5) for v in wave)
    out = {
        "shape": [N, len(SETS) * WAVES_PER_SET],
        "kind": "wavetable-bank",
        "layout": "wave-major: index = wave * 512 + sample",
        "source": "procedurally generated by gen_bank.py",
        "sets": [name for name, _ in SETS],
        "waves_per_set": WAVES_PER_SET,
        "data": data,
    }
    import os
    here = os.path.dirname(os.path.abspath(__file__))
    with open(os.path.join(here, "bank.json"), "w") as f:
        json.dump(out, f)
    print(f"wrote bank.json: {len(SETS)} sets, {len(data)} floats")


if __name__ == "__main__":
    main()
