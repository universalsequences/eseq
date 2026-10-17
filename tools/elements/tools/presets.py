#!/usr/bin/env python3
"""Writes content/instruments/Synths/Elements.presets.

Each preset's level_db is set from a render so presets sit at a similar
loudness: rms over the sounding part of a note (gate held 1.5 s) is brought
to TARGET_DB, within [-36, 24] dB, with peaks kept under PEAK. Also writes a
demo WAV per preset (4 s, retrigger at 1 s, gate off at 2 s) when
ELEMENTS_DEMOS names a folder.
"""
import json, math, os, sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import elh  # noqa: E402  (sets compiler env)

REPO = elh.REPO
INSTRUMENT = elh.INSTRUMENT
TARGET_DB = -19.0
PEAK = 0.8

# mallet: < 0.4 recorded samples, 0.4-0.6 mallet, 0.6-0.8 plectrum, > 0.8 dust
# model: 0 modal, 1 string, 2 strings (geometry = chord), 3 ominous
PRESETS = [
    ("init", "init", "c3", {}),
    ("marimba", "Marimba", "c4", dict(geometry=0.32, brightness=0.42, damping=0.45, position=0.22, mallet=0.48, strike_timbre=0.45, space=0.55)),
    ("glass-bell", "Glass Bell", "c5", dict(geometry=0.72, brightness=0.8, damping=0.72, position=0.35, mallet=0.55, strike_timbre=0.7, space=0.8)),
    ("gamelan", "Gamelan", "c4", dict(geometry=0.46, brightness=0.62, damping=0.62, position=0.3, mallet=0.5, strike_timbre=0.6, space=0.7)),
    ("gong", "Gong", "c2", dict(geometry=0.9, brightness=0.6, damping=0.85, position=0.45, strike=1.0, mallet=0.45, strike_timbre=0.35, space=0.8)),
    ("wood-block", "Wood Block", "c5", dict(geometry=0.28, brightness=0.35, damping=0.28, position=0.3, mallet=0.47, strike_timbre=0.55, space=0.4)),
    ("tine", "Tine", "c4", dict(geometry=0.58, brightness=0.45, damping=0.5, position=0.15, mallet=0.46, strike_timbre=0.35, space=0.6)),
    ("sampled-hit", "Sampled Hit", "c3", dict(geometry=0.4, brightness=0.55, damping=0.55, position=0.3, mallet=0.15, strike_timbre=0.4, space=0.6)),
    ("bowed-string", "Bowed String", "c3", dict(strike=0.0, bow=0.9, bow_timbre=0.4, contour=0.62, geometry=0.25, brightness=0.55, damping=0.72, position=0.18, space=0.75)),
    ("bowed-glass", "Bowed Glass", "c4", dict(strike=0.0, bow=0.85, bow_timbre=0.6, contour=0.6, geometry=0.75, brightness=0.75, damping=0.8, position=0.35, space=0.85)),
    ("bowed-plate", "Bowed Plate", "c3", dict(strike=0.0, bow=0.8, bow_timbre=0.3, contour=0.7, geometry=0.08, brightness=0.6, damping=0.78, position=0.4, space=0.8)),
    ("breath-tube", "Breath Tube", "c4", dict(strike=0.0, blow=0.9, flow=0.5, blow_timbre=0.4, contour=0.6, geometry=0.25, brightness=0.5, damping=0.6, position=0.25, space=0.7)),
    ("breathy-pad", "Breathy Pad", "c3", dict(strike=0.0, blow=0.6, flow=0.3, blow_timbre=0.55, contour=0.85, geometry=0.5, brightness=0.6, damping=0.8, position=0.4, space=0.92)),
    ("chimes", "Wind Chimes", "c5", dict(geometry=0.8, brightness=0.75, damping=0.8, position=0.3, strike=0.7, mallet=0.88, strike_timbre=0.75, space=0.85)),
    ("plucked-string", "Plucked String", "c3", dict(model=1, mallet=0.7, strike_timbre=0.6, geometry=0.25, brightness=0.55, damping=0.62, position=0.2, space=0.6)),
    ("sitar", "Sitar", "c3", dict(model=1, mallet=0.72, strike_timbre=0.7, geometry=0.96, brightness=0.7, damping=0.7, position=0.12, space=0.65)),
    ("bowed-cello", "Bowed Cello", "c2", dict(model=1, strike=0.0, bow=0.9, bow_timbre=0.35, contour=0.6, geometry=0.2, brightness=0.45, damping=0.75, position=0.25, space=0.75)),
    ("harp-chord", "Harp Chord", "c3", dict(model=2, mallet=0.7, strike_timbre=0.55, geometry=0.8, brightness=0.6, damping=0.65, position=0.25, space=0.75)),
    ("string-drone", "String Drone", "c2", dict(model=2, strike=0.0, bow=0.85, bow_timbre=0.4, contour=0.7, geometry=0.2, brightness=0.55, damping=0.8, position=0.3, space=0.85)),
    ("sus-pad", "Sus Pad", "c3", dict(model=2, strike=0.0, blow=0.65, flow=0.4, blow_timbre=0.5, contour=0.85, geometry=1.0, brightness=0.6, damping=0.82, position=0.35, space=0.9)),
    ("ominous-drone", "Ominous Drone", "c2", dict(model=3, contour=0.55, blow=0.8, flow=0.3, blow_timbre=0.5, strike=0.6, mallet=0.6, strike_timbre=0.4, bow=0.55, bow_timbre=0.4, geometry=0.3, brightness=0.35, damping=0.4, position=0.6, space=0.85)),
    ("ominous-bass", "Ominous Bass", "c2", dict(model=3, contour=0.2, blow=0.9, flow=0.15, blow_timbre=0.4, strike=0.0, bow_timbre=0.3, geometry=0.1, brightness=0.3, damping=0.85, position=0.1, space=0.3)),
    ("ominous-bell", "Ominous FM Bell", "c4", dict(model=3, contour=0.15, blow=0.7, flow=0.75, blow_timbre=0.6, strike=0.5, mallet=0.9, strike_timbre=0.5, bow=0.6, bow_timbre=0.2, geometry=0.65, brightness=0.55, damping=0.6, position=0.5, space=0.8)),
    ("cathedral", "Cathedral", "c3", dict(geometry=0.6, brightness=0.55, damping=0.75, position=0.3, mallet=0.52, strike_timbre=0.5, space=1.0)),
]


def main():
    inst = elh.A.Instrument(INSTRUMENT)
    user = [p["name"] for p in inst.manifest["params"] if not p["name"].startswith("__")]
    defaults = {n: inst.params[n]["default"] for n in user}
    demos = os.environ.get("ELEMENTS_DEMOS")
    out = []
    for pid, name, pitch, over in PRESETS:
        params = dict(defaults)
        params.update(over)
        params["level_db"] = 0.0
        params["vel_amt"] = 0.0
        y, _ = inst.render(3.0, pitch=pitch, params=params, gate_off=1.5)
        y = np.asarray(y)[:, :2]
        hop = 2400
        frames = [np.sqrt(np.mean(y[i:i + hop] ** 2)) for i in range(0, len(y) - hop + 1, hop)]
        active = [f for f in frames if f > max(frames) * 0.1]
        rms = math.sqrt(np.mean(np.square(active)))
        level = TARGET_DB - 20 * math.log10(max(rms, 1e-6))
        level = min(level, 20 * math.log10(PEAK / max(float(np.abs(y).max()), 1e-6)))
        level = max(-36.0, min(24.0, round(level, 1)))
        params["vel_amt"] = defaults["vel_amt"]
        params["level_db"] = level if pid != "init" else defaults["level_db"]
        peak = float(np.abs(y).max()) * 10 ** (params["level_db"] / 20)
        print(f"{name:18s} rms {20 * math.log10(max(rms, 1e-9)):6.1f} dB  level {params['level_db']:6.1f}  peak {peak:.2f}", flush=True)
        if demos:
            os.makedirs(demos, exist_ok=True)
            d, _ = inst.render(4.0, pitch=pitch, params=dict(params, vel_amt=0.0), retrig=[1.0], gate_off=2.0)
            elh.A.write_wav(os.path.join(demos, f"{pid}.wav"), np.asarray(d)[:, :2], 48000.0)
        out.append(dict(id=pid, name=name, base_note_offset=0.0,
                        params={k: float(v) for k, v in sorted(params.items())}))
    path = os.path.join(os.path.dirname(INSTRUMENT), "Elements.presets")
    with open(path, "w") as f:
        json.dump(dict(version=1, engine_name="Synths/Elements/",
                       source_file="instruments/Synths/Elements/dsp.lisp", presets=out), f, indent=2)
        f.write("\n")
    print("wrote", path)


if __name__ == "__main__":
    main()
