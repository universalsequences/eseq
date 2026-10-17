#!/usr/bin/env python3
"""Writes content/instruments/Synths/Plaits.presets.

Each preset's level_db is set from a render so presets sit at a similar
loudness: rms over the note (gate held 1.5 s, or the whole hit for
percussive models) is brought to TARGET_DB, within [-24, 6] dB, with peaks kept under PEAK.
"""
import json, math, os, sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import plh  # noqa: E402  (sets compiler env)

REPO = plh.REPO
INSTRUMENT = os.path.join(REPO, "content", "instruments", "Synths", "Plaits")
TARGET_DB = -19.0
PEAK = 0.8


def dx(patch):
    """HARMONICS value centred on DX7 patch `patch` (HysteresisQuantizer2 on h * 1.02 * 32)."""
    return round((patch + 0.5) / (32 * 1.02), 4)


# (id, name, pitch, params)
PRESETS = [
    ("init", "init", "c3", {}),
    ("detuned-saws", "Detuned Saws", "c3", dict(model=0, harmonics=0.32, timbre=0.5, morph=1.0, decay=0.6, lpg=0.9)),
    ("fold-lead", "Fold Lead", "c3", dict(model=1, harmonics=0.4, timbre=0.55, morph=0.3, timbre_amt=0.35, decay=0.55, lpg=0.7)),
    ("fm-bell", "FM Bell", "c4", dict(model=2, harmonics=0.62, timbre=0.45, morph=0.45, timbre_amt=0.5, lpg_mode=0, decay=0.72, lpg=0.85)),
    ("formant-voice", "Formant Voice", "c3", dict(model=3, harmonics=0.4, timbre=0.45, morph=0.6, decay=0.6, lpg=0.8)),
    ("drawbars", "Drawbars", "c3", dict(model=4, harmonics=0.2, timbre=0.35, morph=0.75, decay=0.35, lpg=1.0, aux_mix=1.0)),
    ("wave-scan", "Wave Scan", "c3", dict(model=5, harmonics=0.3, timbre=0.5, morph=0.35, morph_amt=0.5, decay=0.6, lpg=0.8)),
    ("chord-stab", "Chord Stab", "c3", dict(model=6, harmonics=0.45, timbre=0.4, morph=0.6, lpg_mode=0, decay=0.45, lpg=0.6)),
    ("robot-words", "Robot Words", "c3", dict(model=7, harmonics=0.75, timbre=0.5, morph=0.4, lpg_mode=0, decay=0.6)),
    ("vowel-pad", "Vowel Pad", "c3", dict(model=7, harmonics=0.18, timbre=0.5, morph=0.3, morph_amt=0.4, decay=0.7, lpg=0.9)),
    ("swarm-pad", "Swarm Pad", "c3", dict(model=8, harmonics=0.3, timbre=0.65, morph=0.7, decay=0.8, lpg=0.9)),
    ("wind", "Wind", "c4", dict(model=9, harmonics=0.5, timbre=0.55, morph=0.75, decay=0.75, lpg=1.0)),
    ("dust", "Dust", "c4", dict(model=10, harmonics=0.4, timbre=0.55, morph=0.75, decay=0.75, lpg=1.0)),
    ("plucked-string", "Plucked String", "c3", dict(model=11, harmonics=0.3, timbre=0.65, morph=0.75)),
    ("modal-mallet", "Modal Mallet", "c4", dict(model=12, harmonics=0.35, timbre=0.4, morph=0.45)),
    ("deep-kick", "Deep Kick", "c1", dict(model=13, harmonics=0.35, timbre=0.4, morph=0.55)),
    ("snare", "Snare", "c2", dict(model=14, harmonics=0.55, timbre=0.5, morph=0.4)),
    ("closed-hat", "Closed Hat", "c3", dict(model=15, harmonics=0.5, timbre=0.6, morph=0.2)),
    ("acid-vcf", "Acid VCF", "c2", dict(model=16, harmonics=0.75, timbre=0.3, morph=0.35, timbre_amt=0.55, lpg_mode=0, decay=0.45, lpg=0.9)),
    ("pd-brass", "PD Brass", "c3", dict(model=17, harmonics=0.4, timbre=0.4, morph=0.5, timbre_amt=0.4, decay=0.55, lpg=0.8)),
    ("dx-bass", "DX Solid Bass", "c2", dict(model=18, harmonics=dx(0), timbre=0.5, morph=0.5)),
    ("dx-epiano", "DX E.Piano", "c3", dict(model=19, harmonics=dx(0), timbre=0.5, morph=0.5)),
    ("dx-tubular", "DX Tubular Bells", "c4", dict(model=19, harmonics=dx(22), timbre=0.5, morph=0.5)),
    ("dx-strings", "DX Strings", "c3", dict(model=20, harmonics=dx(23), timbre=0.5, morph=0.5)),
    ("dx-brass", "DX Brass", "c3", dict(model=20, harmonics=dx(29), timbre=0.5, morph=0.5)),
    ("terrain-drone", "Terrain Drone", "c2", dict(model=21, harmonics=0.4, timbre=0.5, morph=0.5, morph_amt=0.3, decay=0.8, lpg=1.0)),
    ("string-machine", "String Machine", "c3", dict(model=22, harmonics=0.3, timbre=0.4, morph=0.55, decay=0.75, lpg=1.0)),
    ("chip-arp", "Chip Arp", "c3", dict(model=23, harmonics=0.4, timbre=0.5, morph=0.5, decay=0.6, lpg=1.0)),
]


def main():
    inst = plh.A.Instrument(INSTRUMENT)
    user = [p["name"] for p in inst.manifest["params"] if not p["name"].startswith("__")]
    defaults = {n: inst.params[n]["default"] for n in user}
    out = []
    for pid, name, pitch, over in PRESETS:
        params = dict(defaults)
        params.update(over)
        params["level_db"] = 0.0
        y, _ = inst.render(3.0, pitch=pitch, params=params, gate_off=1.5)
        y = np.asarray(y)[:, 0]
        hop = 2400
        frames = [np.sqrt(np.mean(y[i:i + hop] ** 2)) for i in range(0, 96000, hop)]
        active = [f for f in frames if f > max(frames) * 0.1]  # the sounding part
        rms = math.sqrt(np.mean(np.square(active)))
        # Loudness target, but keep peaks under PEAK.
        level = TARGET_DB - 20 * math.log10(max(rms, 1e-6))
        level = min(level, 20 * math.log10(PEAK / max(float(np.abs(y).max()), 1e-6)))
        level = max(-24.0, min(6.0, round(level, 1)))
        if pid != "init":
            params["level_db"] = level
        else:
            params["level_db"] = defaults["level_db"]
        peak = float(np.abs(y).max()) * 10 ** (params["level_db"] / 20)
        print(f"{name:18s} rms {20 * math.log10(max(rms, 1e-9)):6.1f} dB  level {params['level_db']:6.1f}  peak {peak:.2f}")
        out.append(dict(id=pid, name=name, base_note_offset=0.0,
                        params={k: float(v) for k, v in sorted(params.items())}))
    path = os.path.join(os.path.dirname(INSTRUMENT), "Plaits.presets")
    with open(path, "w") as f:
        json.dump(dict(version=1, engine_name="Synths/Plaits/",
                       source_file="instruments/Synths/Plaits/dsp.lisp", presets=out), f, indent=2)
        f.write("\n")
    print("wrote", path)


if __name__ == "__main__":
    main()
