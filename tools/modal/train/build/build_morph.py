"""Stage Latent Snare (Samples) Morph next to a staged Latent Snare (Samples): voice/snare1_morph.lisp as
dsp.lisp, the same decoder weights, the Latent Snare Morph panel layout with this run's codes and map.
Presets: START/END pairs (given), morph 300 ms.
usage: build_morph.py STATIC_INSTRUMENT_DIR OUT_DIR [--name "Latent Snare (Samples) Morph"] [--presets pairs.json]"""
import argparse, json, os, re, shutil
import numpy as np
import panel_dims
HERE = os.path.dirname(os.path.abspath(__file__)); R = HERE
ESEQ = os.path.abspath(os.path.join(HERE, "../../../.."))
OLD = os.path.join(ESEQ, "content/instruments/Drums/Latent Snare Morph")
def fmt(xs): return " ".join(f"{float(x):.5f}" for x in xs)
ap = argparse.ArgumentParser(); ap.add_argument("static"); ap.add_argument("out")
ap.add_argument("--name", default="Latent Snare (Samples) Morph"); ap.add_argument("--category", default="Research")
ap.add_argument("--presets"); ap.add_argument("--voice", default=os.path.join(R, "voice/snare1_morph.lisp")); a = ap.parse_args()
inst = os.path.join(a.out, a.name); os.makedirs(os.path.join(inst, "data"), exist_ok=True)
cs = json.load(open(os.path.join(a.static, "codes_std.json"))); names = cs["names"]; Z = np.array(cs["codes"]); N = len(names)
# latent range: covers every learned code with a margin (at least 3); the template voice and panel assume [-3, 3]
B = float(max(3.0, np.ceil((np.abs(Z).max() + 0.5) * 2) / 2))
voice = open(a.voice).read()
D = Z.shape[1]
for d in [f"z{k+1}" for k in range(D)] + [f"b{k+1}" for k in range(D)]:
    for old, new in ((f"(param {d} @default 0 @min -3 @max 3 @mod true", f"(param {d} @default 0 @min -{B:g} @max {B:g} @mod true"),
                     (f"(clip (mod {d}) -3 3)", f"(clip (mod {d}) -{B:g} {B:g})")):
        assert old in voice, old
        voice = voice.replace(old, new)
open(os.path.join(inst, "dsp.lisp"), "w").write(voice)
for f in ("w1.json", "b1.json", "w2.json", "b2.json", "w3.json", "b3.json", "codes_std.json", "instrument.json"):
    shutil.copy(os.path.join(a.static, f), inst)
shutil.copy(os.path.join(a.static, "data/family.json"), os.path.join(inst, "data/family.json"))
ui = open(os.path.join(OLD, "ui.lisp")).read()
ui = ui[ui.index("(import eseq.kinds"):]  # the map binds transport.recording / playing
ui = (f";; {a.name} panel: the Latent Snare Morph layout (one map of the latent space with the 32 MF DOOM\n"
      ";; snares at their learned codes; START ring z1..z4 = where the hit strikes, END ring b1..b4 = where it\n"
      ";; rings out to; MORPH; TONE) over this model's latent space; codes, labels and map from\n"
      ";; tools/modal/train/build/build_morph.py.\n") + ui
ui = ui.replace(";; Written by e6/build_panel.py; do not edit by hand.", ";; Written by build/build_morph.py; do not edit by hand.")
block = "\n".join(f'    ("{n}" {fmt(z)})' for n, z in zip(names, Z))  # every latent dim: a load must set all of them
for old, new in (("(def lm-z (name) (min 3 (max -3 (lm-effective name 0))))", f"(def lm-z (name) (min {B:g} (max -{B:g} (lm-effective name 0))))"),
                 ('"instruments/Drums/Latent Snare Morph/data/family.json"', f'"instruments/{a.category}/{a.name}/data/family.json"')):
    assert old in ui, old
    ui = ui.replace(old, new)
ui, k = re.subn(r"\(def ls-codes \(\)\n  '\(\n.*?\)\)\)\n", "(def ls-codes ()\n  '(\n" + block + "))\n", ui, flags=re.S); assert k == 1
ui, k = re.subn(r"\(def ls-slot-labels \(\) '\(.*?\)\)\)\n", "(def ls-slot-labels () '(" + " ".join(f'({i} "{n}")' for i, n in enumerate(names)) + "))\n", ui, flags=re.S); assert k == 1
old_tone = ui[ui.index('        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 5 "tune"'):ui.index("\n", ui.index('(eseq.effects.custom-ui-lego/ui-lego-knob-full-s 5 "level"')) + 1]
new_tone = "".join(f'        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 5 "{p}" "{p}" 5.6 (lm-accent) 2)\n'
                   for p in ("tune", "decay", "snappy")) + '        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 5 "level" "level" 5.6 (lm-accent) 2)))))\n'
ui = ui.replace(old_tone, new_tone)
ui = panel_dims.depth(ui, D)
# The template panel is 4-D: names, nearest snare and code loading are rewritten for this model's D dims
# (a load that left z5.. at their old values would decode a different snare).
names_fn = "(def lm-names (side) (list " + " ".join(f'(str side "{k+1}")' for k in range(D)) + "))\n"
nearest_fn = ("(def lm-nearest (side)\n  (let ((n (lm-names side)) (codes (ls-codes)))\n    (nth\n      (reduce |acc i|\n"
              "        (let ((c (nth codes i)))\n          (let ((d (+ " + " ".join(f"(lm-sq (- (nth c {k+1}) (lm-z (nth n {k}))))" for k in range(D)) + ")))\n"
              "            (if (< d (nth acc 1)) (list i d) acc)))\n        (list 0 1000000)\n        (range (len codes)))\n      0)))\n")
load_fn = ("(def lm-load-code (scope side index)\n  (let ((c (nth (ls-codes) index)) (n (lm-names side)))\n"
           "    (eseq.effects.custom-ui-runtime/custom-ui-set-envelope-in-scope scope\n      (list " + " ".join(f"(list :p{k+1} (nth n {k}))" for k in range(D)) + ")\n"
           "      (dict " + " ".join(f":p{k+1} (nth c {k+1})" for k in range(D)) + " :active false))))\n")
for pat, new in ((r"\(def lm-names \(side\).*?\n", names_fn), (r"\(def lm-nearest \(side\)\n.*?\n        0\)\)\)\)\n", nearest_fn),
                 (r"\(def lm-load-code \(scope side index\)\n.*?:active false\)\)\)\)\n", load_fn)):
    ui, k = re.subn(pat, lambda m: new, ui, flags=re.S); assert k == 1, pat
ui = ui.replace(";; (name z1 z2 z3 z4) in knob space", f";; (name z1 .. z{D}) in knob space")
open(os.path.join(inst, "ui.lisp"), "w").write(ui)
pid = re.sub(r"[^a-z0-9]+", "-", a.name.lower()).strip("-")
pairs = json.load(open(a.presets)) if a.presets else []
presets = []
for p in pairs:
    i, j = names.index(p["a"]), names.index(p["b"])
    q = {f"z{d+1}": round(float(Z[i][d]), 5) for d in range(D)}
    q.update({f"b{d+1}": round(float(Z[j][d]), 5) for d in range(D)})
    q.update(morph=float(p.get("morph", 300)), curve=float(p.get("curve", 0)), amount=1.0, follow=1.0)
    presets.append({"id": f"{pid}-{p['a'].lower()}-to-{p['b'].lower()}", "name": p.get("name", f"{p['a']} to {p['b']}"),
                    "base_note_offset": 0, "params": q})
json.dump({"version": 1, "engine_name": f"{a.category}/{a.name}", "source_file": f"instruments/{a.category}/{a.name}/dsp.lisp",
           "presets": presets}, open(os.path.join(a.out, f"{a.name}.presets"), "w"), indent=2)
print("staged", inst, len(presets), "presets, latent box", B)
