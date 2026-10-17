#!/usr/bin/env python3
"""Assemble the Plaits instrument dsp.lisp from shell + common + engines.

usage: python3 tools/assemble.py OUT_DIR [--ref-tune] [--only name,name]
Engines whose file is missing (or not in --only) become silent stubs.
Copies every data file the included engines reference into OUT_DIR.
"""
import os, re, shutil, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# Data files live in the instrument folder (PLAITS_DATA overrides).
DATA = os.environ.get("PLAITS_DATA", os.path.join(os.path.dirname(os.path.dirname(ROOT)), "content", "instruments", "Synths", "Plaits", "data"))
ARGS = "bstart bphase rise high 1 p_note p_timbre p_morph harm_b accent"

# (name, gate condition on pidx, extra call args, number of return values)
ENGINES = [
    ("vavcf", "(eq pidx 0)", "", 2),
    ("pd", "(eq pidx 1)", "", 2),
    ("sixop", "(* (gte pidx 2) (lte pidx 4))", " (- pidx 2)", 2),
    ("terrain", "(eq pidx 5)", "", 2),
    ("strmach", "(eq pidx 6)", "", 2),
    ("chip", "(eq pidx 7)", " timbre_amt", 2),
    ("va", "(eq pidx 8)", "", 2),
    ("ws", "(eq pidx 9)", "", 2),
    ("fm", "(eq pidx 10)", "", 2),
    ("grain", "(eq pidx 11)", "", 2),
    ("additive", "(eq pidx 12)", "", 2),
    ("wavetable", "(eq pidx 13)", "", 2),
    ("chord", "(eq pidx 14)", "", 2),
    ("speech", "(eq pidx 15)", " fm_amt morph_amt", 3),
    ("swarm", "(eq pidx 16)", "", 2),
    ("noise", "(eq pidx 17)", "", 2),
    ("particle", "(eq pidx 18)", "", 2),
    ("string", "(eq pidx 19)", "", 2),
    ("modal", "(eq pidx 20)", "", 2),
    ("bd", "(eq pidx 21)", "", 2),
    ("sd", "(eq pidx 22)", "", 2),
    ("hh", "(eq pidx 23)", "", 2),
]


def main():
    out_dir = sys.argv[1]
    ref_tune = "--ref-tune" in sys.argv
    sub = ""
    if "--data-subdir" in sys.argv:
        sub = sys.argv[sys.argv.index("--data-subdir") + 1].strip("/") + "/"
    only = None
    if "--only" in sys.argv:
        only = set(sys.argv[sys.argv.index("--only") + 1].split(","))
    os.makedirs(out_dir, exist_ok=True)
    parts = [open(os.path.join(ROOT, "shell", "header.lisp")).read()]
    parts.append(f"\n(def pl_ref_tune {1.0026666756 if ref_tune else 1})\n\n")
    parts.append(open(os.path.join(ROOT, "lib", "common.lisp")).read())
    # Shared helper files, included once before the engines that need them.
    helpers = {"chordbank": ("chord", "strmach")}
    for helper, users in helpers.items():
        if any(os.path.exists(os.path.join(ROOT, "engines", u + ".lisp")) and (only is None or u in only)
               for u in users):
            hsrc = open(os.path.join(ROOT, "engines", helper + ".lisp")).read()
            parts.append("\n" + hsrc)
    calls, outs, auxs = [], [], []
    data = set()
    have_speech = False
    for name, cond, extra, nret in ENGINES:
        path = os.path.join(ROOT, "engines", name + ".lisp")
        if not os.path.exists(path) or (only is not None and name not in only):
            continue
        src = open(path).read()
        data.update(re.findall(r'@file\s+"([^"]+)"', src))
        if sub:
            src = re.sub(r'@file\s+"([^"]+)"', lambda m: f'@file "{sub}{m.group(1)}"', src)
        parts.append("\n" + src)
        if ";@ungated" in src:
            # The engine manages gating itself (delay lines cannot sit
            # upstream of a block-gate): it gets `active` as a last arg.
            calls.append(f"(def active_{name} {cond})")
            calls.append(f"(def ({'o_' + name} {'a_' + name}{' speech_enveloped' if nret == 3 else ''}) "
                         f"(plaits-{name} {ARGS}{extra} active_{name}))")
            if nret == 3:
                have_speech = True
        elif nret == 3:
            have_speech = True
            calls.append(f"(def (o_{name} a_{name} speech_enveloped) (block-gate {cond} "
                         f"(plaits-{name} {ARGS}{extra})))")
        else:
            calls.append(f"(def (o_{name} a_{name}) (block-gate {cond} (plaits-{name} {ARGS}{extra})))")
        outs.append(f"o_{name}")
        auxs.append(f"a_{name}")
    if not have_speech:
        calls.append("(def speech_enveloped 0)")
    zero = ["0"]
    calls.append(f"(def eng_out (+ {' '.join(outs + zero + zero)}))")
    calls.append(f"(def eng_aux (+ {' '.join(auxs + zero + zero)}))")
    # One tensor per data file: later definitions alias the first.
    seen = {}
    def alias(m):
        key = (m.group(2), m.group(3))
        if key in seen:
            return f"(def {m.group(1)} {seen[key]})"
        seen[key] = m.group(1)
        return m.group(0)
    tensor_def = re.compile(r'\(def (\S+) \(tensor @shape (\[[^\]]*\]) @file "([^"]+)"\)\)')
    parts = [tensor_def.sub(alias, p) for p in parts]
    voice = open(os.path.join(ROOT, "shell", "voice.lisp")).read()
    voice = voice.replace(";@ENGINES@", "\n".join(calls))
    parts.append("\n" + voice)
    with open(os.path.join(out_dir, "dsp.lisp"), "w") as f:
        f.write("".join(parts))
    if sub:
        os.makedirs(os.path.join(out_dir, sub), exist_ok=True)
    for d in sorted(data):
        src, dst = os.path.join(DATA, d), os.path.join(out_dir, sub, d)
        if os.path.abspath(src) != os.path.abspath(dst):
            shutil.copyfile(src, dst)
    print(f"assembled {len(outs)} engines, {len(data)} data files -> {out_dir}")


if __name__ == "__main__":
    main()
