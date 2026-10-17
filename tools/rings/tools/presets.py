#!/usr/bin/env python3
"""Writes content/instruments/Synths/Rings.presets.

Each preset's level_db is set from a render so presets sit at a similar
loudness: rms over the sounding part of a 3 s note is brought to TARGET_DB,
within [-24, 18] dB, with peaks kept under PEAK.
"""
import json, math, os, sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rh  # noqa: E402  (sets compiler env)

REPO = rh.REPO
INSTRUMENT = os.path.join(REPO, "content", "instruments", "Synths", "Rings")
TARGET_DB = -19.0
PEAK = 0.8


def chord(i):
    """STRUCTURE value selecting chord i (0..10) in the chord models."""
    return round(i / 10, 3)


# (id, name, pitch, params)
PRESETS = [
    ("init", "init", "c3", {}),
    ("glass-bowl", "Glass Bowl", "c4", dict(model=0, structure=0.9, brightness=0.65, damping=0.8, position=0.3)),
    ("wood-mallet", "Wood Mallet", "c3", dict(model=0, structure=0.2, brightness=0.35, damping=0.45, position=0.2)),
    ("tuned-bars", "Tuned Bars", "c4", dict(model=0, structure=0.27, brightness=0.55, damping=0.7, position=0.6)),
    ("gong", "Gong", "c2", dict(model=0, structure=0.95, brightness=0.8, damping=0.92, position=0.75)),
    ("sitar-drone", "Sitar Drone", "c3", dict(model=1, structure=0.3, brightness=0.7, damping=0.75, position=0.15)),
    ("harp-strings", "Harp Strings", "c3", dict(model=1, structure=0.75, brightness=0.5, damping=0.65, position=0.5)),
    ("tanpura", "Tanpura", "c2", dict(model=1, structure=0.1, brightness=0.6, damping=0.85, position=0.2)),
    ("nylon-pluck", "Nylon Pluck", "c3", dict(model=2, structure=0.25, brightness=0.45, damping=0.55, position=0.25)),
    ("steel-pluck", "Steel Pluck", "c3", dict(model=2, structure=0.5, brightness=0.75, damping=0.6, position=0.15)),
    ("piano-wire", "Piano Wire", "c2", dict(model=2, structure=0.85, brightness=0.65, damping=0.7, position=0.2)),
    ("bent-bridge", "Bent Bridge", "c2", dict(model=2, structure=0.05, brightness=0.6, damping=0.7, position=0.4)),
    ("fm-bell", "FM Bell", "c4", dict(model=3, structure=0.55, brightness=0.6, damping=0.6, position=0.5)),
    ("fm-organ", "FM Organ", "c3", dict(model=3, structure=0.25, brightness=0.35, damping=0.95, position=0.5)),
    ("fm-growl", "FM Growl", "c2", dict(model=3, structure=0.4, brightness=0.75, damping=0.5, position=0.9)),
    ("minor-harp", "Minor Harp", "c3", dict(model=4, structure=chord(3), brightness=0.55, damping=0.7, position=0.3)),
    ("maj7-shimmer", "Maj7 Shimmer", "c3", dict(model=4, structure=chord(9), brightness=0.7, damping=0.8, position=0.6)),
    ("ambient-pluck", "Ambient Pluck", "c3", dict(model=5, structure=0.25, brightness=0.5, damping=0.6, position=0.7)),
    ("cathedral-wire", "Cathedral Wire", "c2", dict(model=5, structure=0.85, brightness=0.6, damping=0.85, position=0.4)),
    ("peace-vowels", "Peace Vowels", "c3", dict(model=6, structure=chord(5), brightness=0.4, damping=0.55, position=0.5)),
    ("peace-drone", "Peace Drone", "c2", dict(model=7, structure=chord(1), brightness=0.6, damping=1.0, position=0.6)),
    ("peace-hall", "Peace Hall", "c3", dict(model=8, structure=chord(4), brightness=0.5, damping=0.4, position=0.8)),
    ("peace-choir", "Peace Choir", "c3", dict(model=9, structure=chord(2), brightness=0.25, damping=0.65, position=0.3)),
    ("peace-strings", "Peace Strings", "c3", dict(model=10, structure=chord(10), brightness=0.65, damping=0.75, position=0.8)),
]


def main():
    inst = rh.A.Instrument(INSTRUMENT)
    user = [p["name"] for p in inst.manifest["params"] if not p["name"].startswith("__")]
    defaults = {n: inst.params[n]["default"] for n in user}
    out = []
    demo = os.environ.get("RINGS_DEMOS")
    for pid, name, pitch, over in PRESETS:
        params = dict(defaults)
        params.update(over)
        params["level_db"] = 0.0
        y, _ = inst.render(3.0, pitch=pitch, params=params, gate_off=1.5)
        y = np.asarray(y)
        hop = 2400
        frames = [np.sqrt(np.mean(y[i:i + hop] ** 2)) for i in range(0, len(y), hop)]
        active = [f for f in frames if f > max(frames) * 0.1]  # the sounding part
        rms = math.sqrt(np.mean(np.square(active)))
        level = TARGET_DB - 20 * math.log10(max(rms, 1e-6))
        level = min(level, 20 * math.log10(PEAK / max(float(np.abs(y).max()), 1e-6)))
        level = max(-24.0, min(18.0, round(level, 1)))
        params["level_db"] = defaults["level_db"] if pid == "init" else level
        peak = float(np.abs(y).max()) * 10 ** (params["level_db"] / 20)
        print(f"{name:16s} rms {20 * math.log10(max(rms, 1e-9)):6.1f} dB  level {params['level_db']:6.1f}  peak {peak:.2f}")
        if demo:
            # 4 s demo: strum at 0, again at 1 s and 2 s (gate off at 2.5 s)
            d, _ = inst.render(4.0, pitch=pitch, params=params, retrig=[1.0, 2.0], gate_off=2.5)
            import wave
            with wave.open(os.path.join(demo, f"{pid}.wav"), "wb") as w:
                w.setnchannels(2); w.setsampwidth(2); w.setframerate(48000)
                w.writeframes((np.clip(np.asarray(d), -1, 1) * 32767).astype("<i2").tobytes())
        out.append(dict(id=pid, name=name, base_note_offset=0.0,
                        params={k: float(v) for k, v in sorted(params.items())}))
    path = os.path.join(os.path.dirname(INSTRUMENT), "Rings.presets")
    with open(path, "w") as f:
        json.dump(dict(version=1, engine_name="Synths/Rings/",
                       source_file="instruments/Synths/Rings/dsp.lisp", presets=out), f, indent=2)
        f.write("\n")
    print("wrote", path)


if __name__ == "__main__":
    main()
