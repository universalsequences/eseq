"""10 contrasting pairs for blend presets and morph renders: greedy farthest pairs in the learned latent,
each snare used once. usage: pick_pairs.py RUN OUT.json [--n 10] [--morph 300]"""
import argparse, itertools, json, os
import numpy as np
ap = argparse.ArgumentParser(); ap.add_argument("run"); ap.add_argument("out"); ap.add_argument("--n", type=int, default=10)
ap.add_argument("--morph", type=float, default=300); a = ap.parse_args()
c = json.load(open(os.path.join(a.run, "export/codes.json"))); names = c["names"]; Z = np.array(c["codes"])
pairs = sorted(itertools.combinations(range(len(names)), 2), key=lambda p: -np.linalg.norm(Z[p[0]] - Z[p[1]]))
used, out = set(), []
for i, j in pairs:
    if i in used or j in used: continue
    used |= {i, j}; out.append({"a": names[i], "b": names[j], "morph": a.morph, "dist": float(np.linalg.norm(Z[i] - Z[j]))})
    if len(out) >= a.n: break
json.dump(out, open(a.out, "w"), indent=1); print(" ".join(f"{p['a']}>{p['b']}" for p in out))
