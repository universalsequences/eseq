"""Rename presets 'NAME B07' -> 'NAME B07 Wanda Robinson' (titles from $DS/manifest.json). usage: label_presets.py PRESETS"""
import json, os, re, sys
R = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
title = {m["name"]: m["title"] for m in json.load(open(os.path.join(R, os.environ.get("DS", "data/train"), "manifest.json")))}
def short(t):
    t = re.sub(r"\.(wav|flac)", "", t); t = re.sub(r"\s*\d+[–-]\d+ ms$", "", t)
    t = re.sub(r"^[%\d.\s]+", "", t); return re.sub(r"\s+", " ", re.sub(r"[^A-Za-z0-9 '\-]+", " ", t)).strip()[:28]
p = sys.argv[1]; d = json.load(open(p))
for pr in d["presets"]:
    pr["name"] = re.sub(r"\b(" + "|".join(sorted(map(re.escape, title), key=len, reverse=True)) + r")\b",
                        lambda m: f"{m.group(1)} {short(title.get(m.group(1), ''))}", pr["name"])
json.dump(d, open(p, "w"), indent=2); print("labelled", len(d["presets"]), "presets")
