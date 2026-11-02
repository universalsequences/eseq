"""Stage a latent instrument from a trained run (eseq.modal.train; from the latent-snare research builder).
The latent may have any number of dims D (codes.json); the panel (the Latent Snare layout) shows z1..z4,
later dims are params (automation, mods).
dsp.lisp = front end (Snare A / Snare B / Blend straight line + z offsets over the run's codes; no warp table)
         + voice/snare1.lisp VERBATIM (the trained source; checksum recorded)
         + the @amp tail.
usage: build_instrument.py RUN OUT_DIR [--name "Latent Snare (Samples)"] [--blends pairs.json]
Latent range and index range are sized from the data (codes beyond +-3 must stay reachable)."""
import argparse, hashlib, json, os, re, shutil
import numpy as np
import panel_dims
HERE = os.path.dirname(os.path.abspath(__file__)); R = HERE
ESEQ = os.path.abspath(os.path.join(HERE, "../../../.."))
OLD = os.path.join(ESEQ, "content/instruments/Drums/Latent Snare")

def code_box(Z):
    """Clip / param range for the latent: covers every learned code with a margin (at least 3)."""
    return float(max(3.0, np.ceil((np.abs(np.asarray(Z)).max() + 0.5) * 2) / 2))

def fmt(xs): return " ".join(f"{float(x):.6g}" for x in xs)

def onehot(d, D): return "[" + " ".join("1" if k == d else "0" for k in range(D)) + "]"

def front(codes, N, member="snare"):
    B = code_box(codes)
    D = np.asarray(codes).shape[1]
    zs = "\n".join(
        f"(def e-z{d+1} (clip (+ (* (- 1 e-t) (peek LSS-CODE e-a {d})) (* e-t (peek LSS-CODE e-b {d})) (mod z{d+1})) -{B:g} {B:g}))"
        for d in range(D))
    zparams = "\n".join(f"(param z{d+1} @default 0 @min -{B:g} @max {B:g} @mod true @mod-mode additive)" for d in range(D))
    fz = " ".join(f"(* e-z{d+1} (tensor @shape [1 {D}] @data {onehot(d, D)}))" for d in range(D))
    M = member.capitalize()
    return f""";; ---- front end (build/build_instrument.py): {M} A, {M} B, Blend, z offsets ----
;; Blend moves along the straight line between the two {member}s' codes (no per-pair warp table).
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))
{zparams}
(param {member}_a @default 0 @min 0 @max {N-1})
(param {member}_b @default 0 @min 0 @max {N-1})
(param blend @default 0 @min 0 @max 1 @mod true @mod-mode additive)
;; learned codes [{N} x {D}]; data channel-major, as peek reads it
(def LSS-CODE (tensor @shape [{N} {D}] @data [{fmt(np.array(codes).T.ravel())}]))
(def e-a (floor (+ (clip {member}_a 0 {N-1}) 0.5)))
(def e-b (floor (+ (clip {member}_b 0 {N-1}) 0.5)))
(def e-t (clip (mod blend) 0 1))
{zs}
(def front-z (+ {fz}))

"""

TAIL = """
;; ---- host amp output (build/build_instrument.py) ----
(use-defmacro voice-amp)
(out (voice-amp 0 voice-out voice-out) 3 @name amp @amp true)
"""

def peak_env(path, n):
    from scipy.io import wavfile
    _, x = wavfile.read(path); x = np.abs(x.astype(float) / (32768.0 if x.dtype == np.int16 else 1.0))
    if x.ndim > 1: x = x.mean(1)
    hop = max(1, len(x) // n); e = np.array([x[i * hop:(i + 1) * hop].max() for i in range(n)])
    return (e / max(e.max(), 1e-9)).round(4).tolist()

def family(run, names, Z, template):
    """family.json for the map: axes z1, z3, z2 (= the template's), members = peak envelopes of the run's renders."""
    fam = json.load(open(template))
    ax = [int(a[1:]) - 1 for a in fam["axes"]]
    old_n = len(fam["lattice"]["members"][0])
    members = [peak_env(os.path.join(run, "renders", f"{k}.wav"), old_n) for k in names]
    fam.update(voice="latent-snare-samples", names=names, scores=Z[:, ax].round(5).tolist(), codes=Z.round(5).tolist(),
               lattice={"members": members, "pairs": {}})
    return fam

if __name__ == "__main__":
    ap = argparse.ArgumentParser(); ap.add_argument("run"); ap.add_argument("out")
    ap.add_argument("--name", default="Latent Snare (Samples)"); ap.add_argument("--category", default="Research")
    ap.add_argument("--voice", default=os.path.join(R, "voice/snare1.lisp"))
    ap.add_argument("--blends", help="json list of {a, b} pairs: blend-0.5 presets")
    ap.add_argument("--member", default="snare", help="what one sample is: Snare A / Snare B, params snare_a / snare_b")
    ap.add_argument("--tone", default="tune,decay,snappy,level", help="the TONE row's knobs (the voice's params)")
    a = ap.parse_args()
    ex = os.path.join(a.run, "export")
    codes = json.load(open(os.path.join(ex, "codes.json")))
    names = codes["names"]; Z = np.array(codes["codes"]); N = len(names); B = code_box(Z)
    voice = open(a.voice).read()
    inst = os.path.join(a.out, a.name); os.makedirs(os.path.join(inst, "data"), exist_ok=True)
    open(os.path.join(inst, "dsp.lisp"), "w").write(
        f";; {a.name} -- built by tools/modal/train/build/build_instrument.py\n"
        f";; voice sha256 {hashlib.sha256(voice.encode()).hexdigest()} (trained source, verbatim below)\n\n"
        + front(Z, N, a.member) + voice + TAIL)
    for f in ("w1.json", "b1.json", "w2.json", "b2.json", "w3.json", "b3.json"):
        shutil.copy(os.path.join(ex, f), inst)
    json.dump({"version": 1, "run_mode": "instrument"}, open(os.path.join(inst, "instrument.json"), "w"))
    json.dump({"names": names, "codes": Z.tolist(), "run": os.path.abspath(a.run)}, open(os.path.join(inst, "codes_std.json"), "w"), indent=1)
    json.dump(family(a.run, names, Z, os.path.join(OLD, "data/family.json")), open(os.path.join(inst, "data/family.json"), "w"))
    # panel: the Latent Snare panel layout with this run's codes, labels, ranges and family path
    ui = open(os.path.join(OLD, "ui.lisp")).read()
    ui = ui[ui.index("(import eseq.kinds"):]  # the map binds transport.recording / playing
    block = "\n".join(f'    ("{n}" {fmt(z[:4])})' for n, z in zip(names, Z))  # the panel's 4 dims
    ui, k = re.subn(r"\(def ls-codes \(\)\n  '\(\n.*?\)\)\)\n", "(def ls-codes ()\n  '(\n" + block + "))\n", ui, flags=re.S); assert k == 1
    ui = ui.replace('"instruments/Drums/Latent Snare/data/family.json"', f'"instruments/{a.category}/{a.name}/data/family.json"')
    ui, k = re.subn(r"\(def ls-slot-labels \(\) '\(.*?\)\)\)\n", "(def ls-slot-labels () '(" + " ".join(f'({i} "{n}")' for i, n in enumerate(names)) + "))\n", ui, flags=re.S); assert k == 1
    # hard-coded counts and latent range in the template
    for old, new in (("(def ls-index (name) (min 31 (max 0", f"(def ls-index (name) (min {N - 1} (max 0"),
                     ("(def ls-clip3 (x) (min 3 (max -3 x)))", f"(def ls-clip3 (x) (min {B:g} (max -{B:g} x)))"),
                     ('\n        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "even" "even" 6.4 (ls-accent) 0)', "")):
        assert old in ui, old
        ui = ui.replace(old, new)
    # tone: the given knobs (tune is a ratio here, not semitones) in the width of 4 at 5.6; the rest stay params
    tone, w = panel_dims.tone_row(a.tone, voice)
    old_tone = ui[ui.index('        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 2 "tune"'):ui.index("(defsynth-ui")]
    new_tone = "".join(f'        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 2 "{p}" "{p}" {w:g} (ls-accent) 2)\n'
                       for p in tone).rstrip("\n") + "))))\n\n"
    ui = ui.replace(old_tone, new_tone)
    # the member: Snare A / B slots and params, the SNARES badge
    if a.member != "snare":
        ui = ui.replace('"snare_a"', f'"{a.member}_a"').replace('"snare_b"', f'"{a.member}_b"').replace('"SNARES"', f'"{a.member.upper()}S"')
    ui = panel_dims.offsets(ui, Z.shape[1])
    ui = (f";; {a.name} A/B panel: layout from the Latent Snare panel; codes, labels and map data from\n"
          ";; tools/modal/train/build/build_instrument.py. The map is the latent space: two latent dims as the axes, the\n"
          f";; {a.member}s at their learned codes, the ring at the z the decoder decodes on the next hit (Blend walks\n"
          f";; the straight line from {a.member.capitalize()} A to {a.member.capitalize()} B, plus the z offsets). The\n"
          ";; knobs off the panel stay params at their defaults: reachable from mods and automation.\n") + ui
    open(os.path.join(inst, "ui.lisp"), "w").write(ui)
    # presets: every snare + blend midpoints of the given pairs
    pid = re.sub(r"[^a-z0-9]+", "-", a.name.lower()).strip("-")
    zero = {f"z{d+1}": 0 for d in range(Z.shape[1])}
    presets = [{"id": f"{pid}-{n.lower()}", "name": f"{a.name} {n}", "base_note_offset": 0,
                "params": {f"{a.member}_a": i, f"{a.member}_b": i, "blend": 0, **zero}} for i, n in enumerate(names)]
    for p in (json.load(open(a.blends)) if a.blends else []):
        presets.append({"id": f"{pid}-{p['a'].lower()}-x-{p['b'].lower()}", "name": f"{a.name} {p['a']} x {p['b']}",
                        "base_note_offset": 0, "params": {f"{a.member}_a": names.index(p["a"]), f"{a.member}_b": names.index(p["b"]),
                                                          "blend": 0.5, **zero}})
    json.dump({"version": 1, "engine_name": f"{a.category}/{a.name}", "source_file": f"instruments/{a.category}/{a.name}/dsp.lisp",
               "presets": presets}, open(os.path.join(a.out, f"{a.name}.presets"), "w"), indent=2)
    print("staged", inst, len(presets), "presets, latent box", B)
