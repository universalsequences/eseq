"""Run an eseq.modal.train experiment: the folder `eseq lisp run` wrote from a patch's
(t/emit-experiment ...): voice.lisp, morph.lisp, layout.json, experiment.json.

  python tools/modal/train/run.py EXPDIR [--until STAGE]

Stages, each cached in EXPDIR (delete its output to run it again):
  data     the training set (dataset.py) -> EXPDIR/data
  seed     starting rows -> EXPDIR/rows/seed.json: the parent's fitted rows for the samples and
           segments it has; for the others a dgen drum analysis (analyze-drums + drum-rows snare1)
           on the parts it covers, a base refine with the remaining parts silent and frozen, then
           each remaining part's seeder from that render (seeders.py), then the :level factors
  superset with a parent: the parent's samples rendered by this voice with every part the parent
           lacks silenced must match the parent's own render of them (warns when not)
  train    the recipe: refine stages (on the new samples with :refit :new, split into GPU-sized
           batches when one would overflow, merged, re-scored on all), pick, pretrain, path
  eval     fidelity (band / MR per sample), latent smoothness, report.md vs the parent
  ship     the instrument + Morph twin, installed to .local/instruments/<folder>, `eseq instrument check`

Tools: DGenLisp from $D (default ~/code/swift/dgen-lss/.build/release/DGenLisp); the scoring and
instrument builders of the latent-snare research toolkit from $ESEQ_TRAIN_TOOLKIT (default
.local/research/latent-snare-from-samples, with .local/research/break-snares for fidelity / knobs).
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
import time

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "../../.."))
sys.path.insert(0, HERE)
import dataset  # noqa: E402
import seeders  # noqa: E402

D = os.environ.get("D", os.path.expanduser("~/code/swift/dgen-lss/.build/release/DGenLisp"))
KIT = os.environ.get("ESEQ_TRAIN_TOOLKIT", os.path.join(ROOT, ".local/research/latent-snare-from-samples"))
BREAKS = os.path.join(ROOT, ".local/research/break-snares")
LIBRARY = os.environ.get("ESEQ_LIBRARY", os.path.join(ROOT, ".local"))
T0 = time.time()


def log(msg):
    print(f"[{time.strftime('%H:%M:%S')} +{int(time.time() - T0)}s] {msg}", flush=True)


def sh(cmd, env=None, check=True, cwd=None):
    r = subprocess.run(cmd, capture_output=True, text=True, env=dict(os.environ, **(env or {})), cwd=cwd)
    if check and r.returncode != 0:
        raise RuntimeError(f"{' '.join(cmd[:3])}... failed:\n{r.stdout[-2000:]}\n{r.stderr[-2000:]}")
    return r


def load(p):
    return json.load(open(p))


def save(obj, p):
    os.makedirs(os.path.dirname(p), exist_ok=True)
    json.dump(obj, open(p, "w"), indent=1)


class Exp:
    def __init__(self, d):
        self.dir = os.path.abspath(d)
        self.spec = load(os.path.join(d, "experiment.json"))
        self.layout = load(os.path.join(d, "layout.json"))
        self.voice = os.path.join(self.dir, "voice.lisp")
        self.morph = os.path.join(self.dir, "morph.lisp")
        self.data = os.path.join(self.dir, "data")
        self.frames = (self.spec.get("data") or {}).get("frames", 32768)

    def p(self, *a):
        return os.path.join(self.dir, *a)

    def manifest(self):
        return load(os.path.join(self.data, "manifest.json"))

    def parent_rows(self):
        par = (self.spec.get("seed") or {}).get("parent")
        if not par:
            return None, None
        par = os.path.join(ROOT, par)
        for cand in ("rows/fitted.json", "export/rows.json"):
            if os.path.exists(os.path.join(par, cand)):
                return par, load(os.path.join(par, cand))["rows"]
        raise SystemExit(f"parent {par}: no rows/fitted.json or export/rows.json")


# ── dgen train-dataset, with GPU batch splitting ─────────────────────────────

def subset_dataset(src, names, out):
    os.makedirs(out, exist_ok=True)
    man = [m for m in load(os.path.join(src, "manifest.json")) if m["name"] in names]
    for m in man:
        dst = os.path.join(out, m["name"] + ".wav")
        if not os.path.lexists(dst):
            os.symlink(os.path.join(src, m["name"] + ".wav"), dst)
    save(man, os.path.join(out, "manifest.json"))


def rows_doc(exp, rows):
    return {"layout": exp.layout["layout"], "rows": rows}


def refine(exp, name, rows, names, steps, lr, freeze=(), wave=0.0):
    """Refine `names` from `rows`; returns the fitted rows. Splits the batch on GPU index overflow."""
    out = exp.p("runs", name)
    if os.path.exists(os.path.join(out, "export/rows.json")):
        return load(os.path.join(out, "export/rows.json"))["rows"]
    ds = exp.p("runs", name + "_data")
    subset_dataset(exp.data, set(names), ds)
    rf = exp.p("runs", name + "_seed.json")
    save(rows_doc(exp, {n: rows[n] for n in names}), rf)
    cmd = [D, "train-dataset", "--patch", exp.voice, "--dataset", ds, "--job-dir", out, "--resume",
           "--stage", "refine", "--rows", rf, "--frames", str(exp.frames), "--steps", str(steps), "--lr", str(lr),
           "--report-every", "100", "--checkpoint-every", "50"]
    if freeze:
        cmd += ["--freeze", ",".join(freeze)]
    if wave:
        cmd += ["--wave-weight", str(wave)]
    r = sh(cmd, check=False)
    if r.returncode != 0 and "32-bit index range" in (r.stdout + r.stderr) and len(names) > 1:
        log(f"  {name}: {len(names)} rows overflow one GPU batch; splitting")
        shutil.rmtree(out, ignore_errors=True)
        half = sorted(names)
        a = refine(exp, name + "_a", rows, half[0::2], steps, lr, freeze, wave)
        b = refine(exp, name + "_b", rows, half[1::2], steps, lr, freeze, wave)
        merged = dict(a, **b)
        os.makedirs(os.path.join(out, "export"), exist_ok=True)
        save(rows_doc(exp, merged), os.path.join(out, "export/rows.json"))
        return merged
    if r.returncode != 0:
        raise RuntimeError(f"refine {name} failed:\n{r.stdout[-1500:]}\n{r.stderr[-1500:]}")
    return load(os.path.join(out, "export/rows.json"))["rows"]


def score(exp, name, rows, dataset_dir=None):
    """Render `rows` (every sample) through the voice: a refine with 0 steps (forward only, no
    gradient memory, so the whole set fits one batch). Returns the job dir."""
    out = exp.p("runs", name)
    if not os.path.exists(os.path.join(out, "summary-refine.json")):
        rf = exp.p("rows", name + ".json")
        save(rows_doc(exp, rows), rf)
        sh([D, "train-dataset", "--patch", exp.voice, "--dataset", dataset_dir or exp.data, "--job-dir", out,
            "--stage", "refine", "--rows", rf, "--frames", str(exp.frames), "--steps", "0"])
    return out


def summary(job, stage="refine"):
    s = load(os.path.join(job, f"summary-{stage}.json"))
    return f"band {s['band_median']:.3f}/{s['band_p90']:.3f}  MR {s['mr_median']:.4f}/{s['mr_p90']:.4f}"


# ── stages ───────────────────────────────────────────────────────────────────

def stage_data(exp):
    if os.path.exists(os.path.join(exp.data, "manifest.json")):
        return
    man = dataset.build(exp.spec["data"], exp.data, LIBRARY, ROOT)
    kept = sum(1 for c in load(os.path.join(exp.data, "crop.json")) if c.get("kept"))
    log(f"data: {len(man)} samples ({kept} kept crops) -> {exp.data}")


def stage_seed(exp):
    out = exp.p("rows/seed.json")
    if os.path.exists(out):
        return load(out)["rows"]
    lay = exp.layout
    lineage = lay.get("lineage") or {}
    added = set(lineage.get("added") or [])
    swapped = set(lineage.get("swapped") or [])
    names = [m["name"] for m in exp.manifest()]
    _, prows = exp.parent_rows()
    explicit = (exp.spec.get("seed") or {}).get("rows")
    erows = load(os.path.join(ROOT, explicit))["rows"] if explicit else {}
    rows, fresh, converted = {}, [], {}
    for n in names:
        src = (prows or {}).get(n) or erows.get(n)
        row = seeders.neutral(lay)
        if src:
            for s_ in lay["segments"]:
                if s_["part"] not in added and s_["name"] in src and len(src[s_["name"]]) == s_["count"]:
                    row[s_["name"]] = src[s_["name"]]
            converted[n] = seeders.convert_second_stroke(row, src, lay)
        else:
            fresh.append(n)
        rows[n] = row
    # parts the parent samples need seeded: added ones, and any whose segments the parent rows lack
    some = next(iter(prows.values())) if prows else (next(iter(erows.values())) if erows else {})
    missing = {s_["part"] for s_ in lay["segments"] if some and s_["name"] not in some and s_["field"] not in ("lvl2", "l2")}
    new_parts = sorted(added | missing)
    log(f"seed: {len(names) - len(fresh)} samples from {'the parent' if prows else 'rows'}, {len(fresh)} to analyse"
        + (f"; parts to seed on them: {', '.join(new_parts)}" if new_parts else "")
        + (f"; swapped: {', '.join(sorted(swapped))}" if swapped else ""))
    if any(converted.values()):
        log(f"  absolute second strokes seeded from the parent's relative ones: {sorted(set(sum(converted.values(), [])))}")
    analyze = (exp.spec.get("seed") or {}).get("analyze", "drum")
    if fresh and analyze == "drum":
        rows = analyse_fresh(exp, rows, fresh)
    known = [n for n in names if n not in fresh]
    if known and new_parts:
        rows = seed_parts(exp, rows, known, new_parts, "seed_known_render")
    for part, factor in (exp.spec.get("seed") or {}).get("level") or []:
        targets = fresh + (known if part in new_parts else [])
        for n in targets:
            seeders.scale_levels(rows[n], part, lay, factor)
    save(rows_doc(exp, rows), out)
    save({"fresh": fresh, "new_parts": new_parts, "added": sorted(added), "swapped": sorted(swapped)},
         exp.p("rows/seed-info.json"))
    return rows


def seed_parts(exp, rows, names, parts, job_name):
    """Seed `parts` on `names` from a render of their current rows (with those parts silent)."""
    lay = exp.layout
    job = score(exp, job_name, rows)
    rng = np.random.default_rng(1)
    byid = {p["id"]: p for p in lay["parts"]}
    for n in names:
        tgt = seeders.rd(os.path.join(exp.data, n + ".wav"))
        ren = os.path.join(job, "renders", n + ".wav")
        for pid in parts:
            p = byid[pid]
            if p["kind"] == "modes" and not p["follow-note"]:
                seeders.seed_free_modes(rows[n], pid, lay, tgt, ren, rng)
        for pid in parts:
            p = byid[pid]
            if p["kind"] == "template":
                seeders.seed_template(rows[n], pid, lay)
            elif p["kind"] == "wash":
                seeders.seed_wash(rows[n], pid, lay, p["on"])
            elif p["kind"] == "restrike":
                seeders.seed_restrike(rows[n], pid, lay, tgt)
    return rows


def analyse_fresh(exp, rows, fresh):
    """dgen drum analysis for the head / strikes / chain / wires / record parts, a base refine with the
    other parts silent and frozen, then the other parts' seeders from that render."""
    lay = exp.layout
    adir = exp.p("seed")
    os.makedirs(adir, exist_ok=True)
    ds = exp.p("seed/data")
    subset_dataset(exp.data, set(fresh), ds)
    if not os.path.exists(os.path.join(adir, "snare1.json")):
        log(f"  analyze-drums on {len(fresh)} samples")
        sh([D, "analyze-drums", "--kind", "snare", "--dataset", ds, "-o", os.path.join(adir, "analysis.json"),
            "--resynth-dir", os.path.join(adir, "resynth")])
        sh([D, "drum-rows", "--layout", "snare1", "--slots", "16", "--analysis", os.path.join(adir, "analysis.json"),
            "-o", os.path.join(adir, "snare1.json")])
    s1 = load(os.path.join(adir, "snare1.json"))["rows"]
    covered = set()
    for n in fresh:
        covered |= seeders.from_snare1(rows[n], s1.get(n, {}), lay)
    others = [p for p in lay["parts"] if p["id"] not in covered]
    frozen = [s["name"] for s in lay["segments"] if s["part"] in {p["id"] for p in others}]
    freq = [s["name"] for s in lay["segments"] if s["role"] in ("freq", "time") and s["name"] not in frozen]
    log(f"  base refine of {len(fresh)} samples, silent + frozen: {', '.join(p['id'] for p in others)}")
    base = refine(exp, "seed_base", rows, fresh, 400, 0.02, freeze=frozen + freq)
    for n in fresh:
        rows[n].update(base[n])
    return seed_parts(exp, rows, fresh, [p["id"] for p in others], "seed_render")


def parent_voice(exp, parent_dir):
    v = (exp.spec.get("seed") or {}).get("parent-voice")
    if v:
        return os.path.join(ROOT, v)
    v = os.path.join(parent_dir, "voice.lisp")
    return v if os.path.exists(v) else None


def stage_superset(exp, rows):
    """With the parts new to the parent silent, this voice must render the parent's samples as the
    parent's voice renders the parent's rows (both rendered here, same batch: a trainer's stored
    renders can lag its exported rows by a step)."""
    info = load(exp.p("rows/seed-info.json"))
    parent_dir, prows = exp.parent_rows()
    if not prows or os.path.exists(exp.p("runs/superset/superset.json")):
        return
    pv = parent_voice(exp, parent_dir)
    if not pv:
        log("superset: skipped (no parent voice: give (t/seed :parent-voice ...))")
        return
    shared = [n for n in rows if n not in info["fresh"]]
    silent = {}
    for n in shared:
        r = dict(rows[n])
        for s_ in exp.layout["segments"]:
            if s_["part"] in info["new_parts"] and s_["role"] == "level":
                r[s_["name"]] = [s_["lo"]] * s_["count"]
        silent[n] = r
    ds = exp.p("runs", "superset_data")
    subset_dataset(exp.data, set(shared), ds)
    job = score(exp, "superset", silent, ds)
    ref = exp.p("runs", "superset_parent")
    if not os.path.exists(os.path.join(ref, "summary-refine.json")):
        rf = exp.p("rows", "superset_parent.json")
        save({"layout": exp.layout["layout"], "rows": {n: prows[n] for n in shared}}, rf)
        sh([D, "train-dataset", "--patch", pv, "--dataset", ds, "--job-dir", ref, "--stage", "refine",
            "--rows", rf, "--frames", str(exp.frames), "--steps", "0"])
    worst, worst_n = 0.0, None
    for n in shared:
        a, b = seeders.rd(os.path.join(job, "renders", n + ".wav")), seeders.rd(os.path.join(ref, "renders", n + ".wav"))
        k = min(len(a), len(b))
        d = float(np.abs(a[:k] - b[:k]).max() / max(np.abs(b).max(), 1e-12))
        if d > worst:
            worst, worst_n = d, n
    sw = info.get("swapped") or []
    ok = worst < 1e-3
    verdict = ("OK" if ok else (f"differs, as expected: swapped {', '.join(sw)}" if sw else
                                "DIFFERS (a new part is not silent at its floor, or the shared parts changed)"))
    log(f"superset: {len(shared)} parent samples, max |diff|/peak {worst:.2e} ({worst_n}) {verdict}")
    save({"max_rel": worst, "worst": worst_n, "ok": ok, "swapped": sw}, exp.p("runs/superset/superset.json"))


def stage_train(exp, rows):
    info = load(exp.p("rows/seed-info.json"))
    names = [m["name"] for m in exp.manifest()]
    picks, cur, k = {}, rows, 0
    for st in exp.spec["train"]:
        kind = st["stage"]
        if kind == "refine":
            k += 1
            nm = f"refine{k}"
            fit = names if st.get("refit", "all") == "all" else info["fresh"]
            freeze = st.get("freeze")
            if freeze == "roles":
                freeze = [s["name"] for s in exp.layout["segments"] if s["role"] in ("freq", "time")]
            log(f"{nm}: {len(fit)} samples, {st['steps']} steps, lr {st['lr']}"
                + (f", wave {st['wave']}" if st.get("wave") else "") + (f", frozen {','.join(freeze)}" if freeze else ""))
            new = refine(exp, nm, cur, fit, st["steps"], st["lr"], freeze or (), st.get("wave", 0))
            cur = dict(cur, **new)
            save(rows_doc(exp, cur), exp.p("rows", nm + ".json"))
            job = score(exp, nm + "_all", cur)
            picks[nm] = (load(os.path.join(job, "summary-refine.json")), cur)
            log(f"  {nm} on all {len(names)}: {summary(job)}")
        elif kind == "pick":
            sc = {n: st["band"] * s["band_median"] + st["mr"] * s["mr_median"] for n, (s, _) in picks.items()}
            best = min(sc, key=sc.get)
            cur = picks[best][1]
            log(f"pick: {best} ({', '.join(f'{n} {v:.4f}' for n, v in sc.items())})")
            save({"picked": best}, exp.p("rows/pick.json"))
        elif kind == "pretrain":
            save(rows_doc(exp, cur), exp.p("rows/fitted.json"))
            out = exp.p("runs/pretrain")
            if not os.path.exists(os.path.join(out, "summary-pretrain.json")):
                sh([D, "train-dataset", "--patch", exp.voice, "--dataset", exp.data, "--job-dir", out, "--resume",
                    "--stage", "pretrain", "--rows", exp.p("rows/fitted.json"), "--steps", str(st["steps"]),
                    "--lr", str(st["lr"]), "--frames", str(exp.frames), "--report-every", str(st["steps"])])
            log(f"pretrain {st['steps']}: {summary(out, 'pretrain')}")
        elif kind == "path":
            out = exp.p("runs/final")
            if not os.path.exists(os.path.join(out, "summary-pretrain.json")):
                sh([D, "train-dataset", "--patch", exp.voice, "--dataset", exp.data, "--job-dir", out, "--resume",
                    "--stage", "pretrain", "--init-from", exp.p("runs/pretrain"), "--rows", exp.p("rows/fitted.json"),
                    "--path", str(st["lambda"]), "--seed", str(st["seed"]), "--steps", str(st["steps"]),
                    "--lr", str(st["lr"]), "--frames", str(exp.frames), "--report-every", str(st["steps"])])
            log(f"path {st['lambda']}: {summary(out, 'pretrain')}")


def fidelity(exp, job):
    if not os.path.exists(os.path.join(job, "fidelity.json")):
        sh([sys.executable, os.path.join(BREAKS, "eval/fidelity.py"), job], env={"DS": exp.data})
    return load(os.path.join(job, "fidelity.json"))


def stage_eval(exp):
    fin = exp.p("runs/final")
    f = fidelity(exp, fin)
    refined = sorted(d for d in os.listdir(exp.p("runs")) if d.startswith("refine") and d.endswith("_all"))
    if os.path.exists(exp.p("rows/pick.json")):
        fit_job = exp.p("runs", load(exp.p("rows/pick.json"))["picked"] + "_all")
    elif refined:
        fit_job = exp.p("runs", refined[-1])
    else:  # a latent-only recipe: the fitted rows are the seed (the parent's)
        fit_job = score(exp, "fitted_all", load(exp.p("rows/fitted.json"))["rows"])
    fit = fidelity(exp, fit_job)
    lat = exp.p("runs/final/latent_summary.json")
    if not os.path.exists(lat):
        r = sh([sys.executable, "latent_eval.py", fin, "--patch", exp.voice, "--pairs", "300", "--chunk", "154"],
               check=False, cwd=os.path.join(KIT, "eval"))
        if r.returncode == 0:
            open(lat, "w").write(r.stdout)
    report(exp, f, fit, load(lat) if os.path.exists(lat) and os.path.getsize(lat) else None)


def report(exp, f, fit, lat):
    man = {m["name"]: m for m in exp.manifest()}
    rows = f["rows"]
    parent_dir, _ = exp.parent_rows()
    pf = None
    if parent_dir:
        # the parent's instrument (its final latent run), not its fitted rows; a bare research run dir
        # (m2_rA) has its instrument run beside it (m2_p0.02)
        for cand in ("runs/final/fidelity.json", "../" + os.path.basename(parent_dir).replace("_rA", "_p0.02") + "/fidelity.json",
                     "fidelity.json"):
            q = os.path.normpath(os.path.join(parent_dir, cand))
            if os.path.exists(q):
                pf = load(q)["rows"]
                break
    kept = {c["name"] for c in load(os.path.join(exp.data, "crop.json")) if c.get("kept")}
    groups = {}
    for n, m in man.items():
        groups.setdefault(m.get("origin", "?") + (" (kept)" if n in kept else ""), []).append(n)
    L = [f"# {exp.spec['name']}", "", exp.spec.get("notes", ""), "",
         f"voice {exp.layout['name']}: {len(exp.layout['parts'])} parts, {exp.layout['width']} decoded values; "
         f"{len(man)} samples", "",
         "| | band median / p90 | MR median / p90 |", "|---|---|---|",
         f"| fitted rows (refine) | {fit['band_median']:.3f} / {fit['band_p90']:.3f} | {fit['mr_median']:.3f} / {fit['mr_p90']:.3f} |",
         f"| **instrument (latent)** | **{f['band_median']:.3f} / {f['band_p90']:.3f}** | **{f['mr_median']:.3f} / {f['mr_p90']:.3f}** |", ""]
    if lat:
        st = lat.get("straight", {})
        L += [f"latent straight-blend CV {st.get('cv', float('nan')):.3f} (lower = smoother morphs)", ""]
    L += ["| samples | n | band median | MR median |" + (" parent band | better than parent |" if pf else ""),
          "|---|---|---|---|" + ("---|---|" if pf else "")]
    for g, ns in groups.items():
        b = np.median([rows[n]["band"] for n in ns])
        mr = np.median([rows[n]["mr"] for n in ns])
        line = f"| {g} | {len(ns)} | {b:.3f} | {mr:.3f} |"
        if pf:
            shared = [n for n in ns if n in pf]
            if shared:
                line += f" {np.median([pf[n]['band'] for n in shared]):.3f} | {sum(rows[n]['band'] < pf[n]['band'] for n in shared)}/{len(shared)} |"
            else:
                line += " | |"
        L.append(line)
    L += ["", "worst: " + ", ".join(f"{n} {rows[n]['band']:.2f}" for n in sorted(rows, key=lambda n: -rows[n]["band"])[:10])]
    sup = exp.p("runs/superset/superset.json")
    if os.path.exists(sup):
        s = load(sup)
        sw = load(exp.p("rows/seed-info.json")).get("swapped") or []
        L += ["", f"superset check (parent samples, new parts silent): max |diff|/peak {s['max_rel']:.1e} "
                  + ("OK" if s["ok"] else (f"differs, as expected (swapped: {', '.join(sw)})" if sw else "DIFFERS"))]
    open(exp.p("report.md"), "w").write("\n".join(L) + "\n")
    print("\n".join(L))


def stage_ship(exp):
    sp = exp.spec.get("ship")
    if not sp:
        return
    name, mname = sp["name"], sp["name"] + " Morph"
    fin = exp.p("runs/final")
    stage = exp.p("stage")
    shutil.rmtree(stage, ignore_errors=True)
    os.makedirs(stage)
    env = {"ESEQ_DGENLISP_TOOL": os.path.join(KIT, "tools/dgenlisp/DGenLisp"), "DS": exp.data}
    B = os.path.join(HERE, "build")
    sh([sys.executable, os.path.join(B, "pick_pairs.py"), fin, os.path.join(fin, "pairs.json")], env=env)
    sh([sys.executable, os.path.join(B, "build_instrument.py"), fin, stage, "--name", name, "--voice", exp.voice,
        "--blends", os.path.join(fin, "pairs.json"), "--category", sp.get("folder", "Research")], env=env)
    if sp.get("morph", True):
        sh([sys.executable, os.path.join(B, "build_morph.py"), os.path.join(stage, name), stage, "--name", mname,
            "--voice", exp.morph, "--presets", os.path.join(fin, "pairs.json"), "--category", sp.get("folder", "Research")], env=env)
    built = [name] + ([mname] if sp.get("morph", True) else [])
    for n, v in ((name, exp.voice), (mname, exp.morph)):
        if n in built:
            extra = sp.get("extra-knobs") or []
            if extra:
                sh([sys.executable, os.path.join(B, "add_rhythm_knob.py"), os.path.join(stage, n, "ui.lisp"), v, ",".join(extra)], env=env)
            sh([sys.executable, os.path.join(B, "label_presets.py"), os.path.join(stage, n + ".presets")], env=env)
    dest = os.path.join(ROOT, ".local/instruments", sp.get("folder", "Research"))
    os.makedirs(dest, exist_ok=True)
    for n in built:
        for suffix in ("", ".presets"):
            d, src = os.path.join(dest, n + suffix), os.path.join(stage, n + suffix)
            if not os.path.exists(src):
                continue
            if os.path.isdir(d):
                shutil.rmtree(d)
            elif os.path.exists(d):
                os.remove(d)
            (shutil.copytree if os.path.isdir(src) else shutil.copyfile)(src, d)
    for n in built:
        r = sh([os.path.join(ROOT, "target/release/eseq"), "instrument", "check", f"{sp.get('folder', 'Research')}/{n}"],
               check=False, env=env)
        verdict = [l for l in (r.stdout + r.stderr).splitlines() if l.startswith(("PASS", "FAIL"))]
        log(f"ship: {n} -> {dest}: {verdict[-1] if verdict else 'check exited ' + str(r.returncode)}")


STAGES = ["data", "seed", "superset", "train", "eval", "ship"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("exp")
    ap.add_argument("--until", choices=STAGES, default="ship")
    a = ap.parse_args()
    exp = Exp(a.exp)
    log(f"{exp.spec['name']}: voice {exp.layout['name']} ({exp.layout['width']} values)")
    stage_data(exp)
    if a.until == "data":
        return
    rows = stage_seed(exp)
    if a.until == "seed":
        return
    stage_superset(exp, rows)
    if a.until == "superset":
        return
    stage_train(exp, rows)
    if a.until == "train":
        return
    stage_eval(exp)
    if a.until == "eval":
        return
    stage_ship(exp)


if __name__ == "__main__":
    main()
