#!/usr/bin/env python3
"""Validate the actual Digi Drift voice, gain placement, and native-rate extremes.

This is a behavioral/stability gate for the emulation, not a native-match gate.
The compiler audit and generated-code fusion check remain enabled.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/audition"))
from audition import Instrument


def signal_check(y, memory):
    if not np.isfinite(y).all() or not np.isfinite(memory).all():
        raise ValueError("Non-finite output or voice state")
    peak = float(np.max(abs(y)))
    if peak > 3:
        raise ValueError(f"Unexpected voice output peak: {peak}")
    return dict(peak=peak, rms=float(np.sqrt(np.mean(y.astype(float)**2))))


def check(compiler, toolchain):
    source = ROOT / "content/instruments/Synths/Digi Drift/dsp.lisp"
    bank = json.loads((source.parent.parent / "Digi Drift.presets").read_text())
    rows, gain_checks, presets = [], [], []
    base = dict(osc1_wave=0, osc2_on=0, noise_gain_db=-60,
                drift=0, spread=0, voice_pan=0, keytrack=0,
                lp_mod1_amt=0, lp_mod2_amt=0, env1_sustain=1,
                volume_db=-12, env1_release=50)
    for rate in (22050, 32000, 44100, 48000, 96000):
        instrument = Instrument(source, sample_rate=rate,
                                compiler=str(compiler.resolve()),
                                toolchain_root=str(toolchain.resolve()))
        subprocess.run([sys.executable, str(ROOT / "tools/audition/check_fusion.py"),
                        str(Path(instrument.build_dir) / "patch.c")], check=True)
        if "filter_drive" in instrument.params:
            raise ValueError("Obsolete Drive parameter remains")
        for kind in (0, 1):
            for cutoff in (20, 1000, 18000):
                for resonance in (0, .8, 1):
                    params = dict(base, filter_type=kind, lp_freq=cutoff,
                                  lp_res=resonance, osc1_gain_db=12)
                    y, memory = instrument.render(.3, pitch=130.8128, params=params)
                    rows.append(dict(sample_rate=rate, type=kind, cutoff=cutoff,
                                     resonance=resonance, **signal_check(y, memory)))
            y, memory = instrument.render(2, pitch=130.8128,
                params=dict(base, filter_type=kind, osc1_gain_db=12),
                ramps=dict(lp_freq=[(0,20),(.3,18000),(.31,20),(.6,18000),(1,20)],
                           lp_res=[(0,0),(.2,1),(1,1)],
                           hp_freq=[(0,20),(.5,10000),(.6,20)],
                           pitch=[(0,32.7),(.8,8000),(1,65.4)]), gate_off=1.2)
            signal_check(y, memory)
            if float(np.max(abs(y[-rate//10:]))) > 1e-5:
                raise ValueError("Voice did not become silent after release")
        if rate != 48000:
            continue
        for kind in (0, 1):
            params = dict(base, filter_type=kind, osc1_gain_db=6, lp_res=.8, lp_freq=1000)
            loud, memory = instrument.render(1, pitch=130.8128, params=params)
            signal_check(loud, memory)
            quiet, memory = instrument.render(1, pitch=130.8128,
                                              params=dict(params, volume_db=-24))
            signal_check(quiet, memory)
            residual = float(np.linalg.norm(loud-quiet*10**(.6))/np.linalg.norm(loud))
            if residual > 1e-5:
                raise ValueError(f"Volume is changing the drive character: {residual}")
            low, _ = instrument.render(1, pitch=130.8128,
                params=dict(params, osc1_gain_db=-24, lp_res=0))
            high, _ = instrument.render(1, pitch=130.8128, params=dict(params, lp_res=0))
            weights = np.hanning(24000)
            carrier = np.exp(-2j*np.pi*130.8128*np.arange(24000)/rate)
            fundamental = lambda y: abs(np.sum(y[24000:,0]*weights*carrier))
            compression = float(20*np.log10(fundamental(high)/fundamental(low))-30)
            if compression > -1:
                raise ValueError(f"Oscillator level is not driving the filter: {compression} dB")
            gain_checks.append(dict(type=kind, volume_scaling_relative_error=residual,
                                    oscillator_level_compression_db=compression))
        for preset in bank["presets"]:
            if any("filter_drive" in name for name in preset["params"]):
                raise ValueError("Obsolete Drive preset binding remains")
            params = {name:value for name,value in preset["params"].items()
                      if name in instrument.params}
            pitch = 220*2**(preset.get("base_note_offset",0)/12)
            y, memory = instrument.render(.5, pitch=pitch, params=params)
            metrics = signal_check(y, memory)
            if metrics["peak"] < 1e-5:
                raise ValueError(f"Silent preset: {preset['name']}")
            presets.append(dict(name=preset["name"], **metrics))
    return dict(source_sha256=hashlib.sha256(source.read_bytes()).hexdigest(),
                compiler_sha256=instrument.compiler_sha256, scope=__doc__,
                static_cases=rows, dynamic_rate_type_cases=10,
                gain_checks=gain_checks, presets=presets, passed=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--toolchain", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    result = check(args.compiler, args.toolchain)
    args.out.write_text(json.dumps(result, indent=2)+"\n")
    print(f"Passed {len(result['static_cases'])} static cases, "
          f"{result['dynamic_rate_type_cases']} dynamic cases, "
          f"{len(result['presets'])} presets, and both gain-placement checks")
