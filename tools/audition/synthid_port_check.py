#!/usr/bin/env python3
"""Fill a dsp.lisp template with SynthID-recovered params and prove the
eseq instrument reproduces the fit's learned render.

    python3 tools/audition/synthid_port_check.py \
        --run ~/code/swift/dgen/output/<run> \
        --template .claude/skills/identify-drum/noise-voice-dsp-template.lisp \
        --instrument "content/instruments/Drums/<Name>" \
        --fit-module ~/code/swift/dgen/Examples/SynthID/scripts/fit_<name>.py

Placeholders in the template are __KEY__ with KEY = the recovered_params.json
key upper-cased (fc1 -> __FC1__). __OUTGAIN__ gets the fit's 0.9 peak
normalisation folded in so the instrument at its defaults IS learned.wav.
Prints: out-of-range defaults, max-abs parity, and the gate metric for both
renders (they must agree to ~1e-4). Needs the audition-harness env vars
(DGEN_RUNTIME_INCLUDE, DGEN_BINARY_AUDIT_TOOL) — see
docs/instrument-audition-harness.md.
"""
import argparse, importlib.util, json, os, re, sys
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from audition import Instrument, write_wav  # noqa: E402


def load_module(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.path.insert(0, os.path.dirname(os.path.abspath(path)))
    spec.loader.exec_module(mod)
    return mod


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--run", required=True, help="fit output dir with recovered_params.json, target.wav")
    ap.add_argument("--template", required=True)
    ap.add_argument("--instrument", required=True, help="instrument folder; dsp.lisp is written there")
    ap.add_argument("--fit-module", required=True, help="the fit_<name>.py whose render() is the reference")
    ap.add_argument("--pitch", type=float, default=261.63)
    args = ap.parse_args()

    fit = load_module(args.fit_module, "fit_voice")
    compare = load_module(os.path.join(os.path.dirname(os.path.abspath(args.fit_module)), "compare.py"), "compare")
    rep = json.load(open(os.path.join(args.run, "recovered_params.json")))
    p, sr, frames = rep["params"], rep["sampleRate"], rep["frames"]

    y_ref = fit.render(p, frames, sr)
    pk = float(np.abs(y_ref).max())
    scale = 0.9 / pk if pk > 0.9 else 1.0
    ref = (y_ref * scale).astype(np.float32)

    s = open(args.template).read()
    def literal(v):
        # dgenlisp reads plain decimals; keep tiny values out of exponent notation.
        text = f"{v:.6g}"
        if "e" in text:
            text = f"{v:.12f}".rstrip("0")
        return text
    for k, v in p.items():
        if k == "outGain":
            v = v * scale
        s = s.replace(f"__{k.upper()}__", literal(v))
    left = sorted(set(re.findall(r"__[A-Z0-9_]+__", s)))
    if left:
        raise SystemExit(f"unfilled placeholders: {left}")
    os.makedirs(args.instrument, exist_ok=True)
    dst = os.path.join(args.instrument, "dsp.lisp")
    open(dst, "w").write(s)
    print(f"wrote {dst} (peak normalisation {scale:.4f} folded into out_gain)")

    bad = []
    for name, default, lo, hi in re.findall(r"\(param (\w+) @default (\S+) @min (\S+) @max ([-0-9.e]+)", s):
        if not (float(lo) <= float(default) <= float(hi)):
            bad.append((name, default, lo, hi))
    for b in bad:
        print("OUT OF RANGE default:", b)

    inst = Instrument(args.instrument, sample_rate=float(sr))
    y, _ = inst.render(frames / sr, pitch=args.pitch, vel=1.0)
    y = np.asarray(y)[:frames].astype(np.float32)
    d = np.abs(y - ref)
    print(f"parity: max abs {d.max():.2e}, samples >1e-3: {int((d > 1e-3).sum())} of {frames}, ref rms {np.sqrt(np.mean(ref ** 2)):.4f}")
    target, _ = compare.read_wav(os.path.join(args.run, "target.wav"))
    hp = lambda x: compare.capture_highpass(x, sr, compare.DEFAULT_HIGHPASS_HZ)
    gi, gl = compare.mrstft(hp(y), hp(target)), compare.mrstft(hp(ref), hp(target))
    print(f"gate: instrument {gi:.4f} vs learned.wav {gl:.4f}")
    write_wav(os.path.join(args.run, "instrument.wav"), y, sr)
    ok = d.max() < 5e-3 and abs(gi - gl) < 1e-3 and not bad
    print("PARITY OK" if ok else "PARITY FAILED")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
