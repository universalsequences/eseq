#!/usr/bin/env python3
"""Bake the mode tables for drums/modal-snare-wired (numpy only, no scipy).

Circular-membrane eigenmodes psi_mn(r,th) = J_m(j_mn r) cos(m th) (cos set)
and J_m(j_mn r) sin(m th) (sin set: the degenerate partner of every m>0
mode). Modal mass M = integral psi^2 dA. A force F at x0 drives modal
coordinate q_n with weight psi_n(x0)/M_n; displacement at x is sum q_n psi_n(x).

Tables are [12 6]: rows 0-5 = the 36 cos modes sorted by frequency (m<=7,
the same list modal-snare bakes), rows 6-11 = their sin partners (zero for
m=0, which the scalar proxies own).

Run from the repo root; rewrites the block between the GENERATED markers in
dsp.lisp next to this file.
"""
import os, re
import numpy as np
_trapz = getattr(np, "trapezoid", None) or np.trapz

# ── geometry (angles in degrees, radii as a fraction of the head radius) ──
STRIKE_TH = 20.0      # strike azimuth relative to the anisotropy axis; both
                      # orientations of every pair get excited (the beating)
OPEN_R, EDGE_R = 0.40, 0.93
TIP = 0.06            # baked stick-tip lowpass exp(-(j_mn tip)^2)
BAT_MIC = (0.55, 60.0)
RES_MIC = (0.50, 152.0)
WIRE_TH = 110.0       # diameter the wire contact points lie on
# 4 contact zones, 3 strands each. Measured 2026-09-03: 12 individual
# points cost 2.3 percentage points of CPU per voice for NO change in rattle
# flatness/periodicity (the projection contact carries the chaos, not the
# point diversity); 3 points started to phase-lock again (periodicity 0.17).
WIRE_OFF = [-0.55, -0.15, 0.15, 0.55]
PALM = (0.5, 200.0, 0.35)   # palm bump centre (r, th) + gaussian sigma
MAX_M = 7
NMODES = 36
SEED = 7

TAU = np.linspace(0.0, np.pi, 4001)

def J(m, x):
    x = np.asarray(x, dtype=float)
    xs = x.reshape(-1, 1)
    f = np.cos(m * TAU - xs * np.sin(TAU))
    return (_trapz(f, TAU, axis=1) / np.pi).reshape(x.shape)

def zeros(m, count):
    xs = np.arange(0.5, 60.0, 0.01)
    v = J(m, xs)
    out = []
    for i in range(len(xs) - 1):
        if v[i] == 0 or v[i] * v[i + 1] > 0:
            continue
        a, b = xs[i], xs[i + 1]
        fa = v[i]
        for _ in range(60):
            c = 0.5 * (a + b)
            fc = J(m, np.array([c]))[0]
            if fa * fc <= 0:
                b = c
            else:
                a, fa = c, fc
        z = 0.5 * (a + b)
        if z > 1e-3:
            out.append(z)
        if len(out) == count:
            break
    return out

modes = []
for m in range(MAX_M + 1):
    for n, z in enumerate(zeros(m, 8), start=1):
        modes.append((z, m, n))
modes.sort()
modes = modes[:NMODES]
j01 = modes[0][0]

def modal_mass(m, j):
    r = np.linspace(0, 1, 4001)
    rad = _trapz(J(m, j * r) ** 2 * r, r)
    return 2 * np.pi * rad if m == 0 else np.pi * rad

def psi(m, j, r, th_deg, orient):
    th = np.radians(th_deg)
    ang = np.cos(m * th) if orient == "cos" else np.sin(m * th)
    return J(m, np.array([j * r]))[0] * ang

def table(fn):
    """fn(m, j, orient) -> value; returns [12 6] rows cos then sin."""
    rows = []
    for orient in ("cos", "sin"):
        vals = [fn(m, j, orient) if not (orient == "sin" and m == 0) else 0.0
                for (j, m, n) in modes]
        rows.append(np.array(vals).reshape(6, 6))
    return np.vstack(rows)

def fmt(name, t, comment=""):
    lines = [f"(def {name} (tensor @shape [12 6] @data ["]
    if comment:
        lines.insert(0, f"; {comment}")
    for row in t:
        lines.append("  " + " ".join(f"{v: .4f}" for v in row))
    lines.append("]))")
    return "\n".join(lines)

mass = {(m, n): modal_mass(m, j) for (j, m, n) in modes}
ratio = table(lambda m, j, o: j / j01)
ratio[6:] = ratio[:6]          # sin rows share the frequencies
def strike(r):
    return table(lambda m, j, o: psi(m, j, r, STRIKE_TH, o) / mass[(m, [n for (jj, mm, n) in modes if jj == j][0])]
                 * np.exp(-(j * TIP) ** 2))
open_w = strike(OPEN_R)
edge_w = strike(EDGE_R)
# global scale: keep the (0,1) open weight at modal-snare's 0.2855 so the
# scalar proxies' constants and the gain staging carry over unchanged
scale = 0.2855 / open_w[0, 0]
open_w *= scale
edge_w *= scale
bat_mic = table(lambda m, j, o: psi(m, j, BAT_MIC[0], BAT_MIC[1], o))
res_mic = table(lambda m, j, o: psi(m, j, RES_MIC[0], RES_MIC[1], o))
asym = table(lambda m, j, o: 0.0 if m == 0 else 1.0)

# palm energy fraction: integral of psi^2 over a gaussian bump / integral psi^2
rr = np.linspace(0, 1, 301)
tt = np.linspace(0, 2 * np.pi, 361)
R, T = np.meshgrid(rr, tt, indexing="ij")
X, Y = R * np.cos(T), R * np.sin(T)
px, py = PALM[0] * np.cos(np.radians(PALM[1])), PALM[0] * np.sin(np.radians(PALM[1]))
bump = np.exp(-((X - px) ** 2 + (Y - py) ** 2) / (2 * PALM[2] ** 2))
def palm_frac(m, j, o):
    ang = np.cos(m * T) if o == "cos" else np.sin(m * T)
    p2 = (J(m, j * rr)[:, None] * ang) ** 2 * R
    return _trapz(_trapz(p2 * bump, tt, axis=1), rr) / _trapz(_trapz(p2, tt, axis=1), rr)
palm = table(palm_frac)
palm /= palm.max()

# split deviation: sin partner detuned by a fixed pseudo-random +-[0.5,1]
rng = np.random.default_rng(SEED)
dev = np.zeros((12, 6))
for i, (j, m, n) in enumerate(modes):
    if m > 0:
        dev[6 + i // 6, i % 6] = rng.choice([-1, 1]) * rng.uniform(0.5, 1.0)

# wire contact points along the WIRE_TH diameter
wire_pts = []
for d in WIRE_OFF:
    th = WIRE_TH if d >= 0 else WIRE_TH + 180.0
    wire_pts.append(table(lambda m, j, o: psi(m, j, abs(d), th, o)))
# (0,n) proxies seen from each contact point (J0(j_0n |d|)), n = 1..3
prox = [(j, n) for (j, m, n) in modes if m == 0][:3]
wire_prox = [[J(0, np.array([j * abs(d)]))[0] for (j, n) in prox] for d in WIRE_OFF]

out = []
out.append("; modes (m,n) in frequency order, rows 0-5 = cos set, 6-11 = sin partners:")
out.append("; " + " ".join(f"({m},{n})" for (j, m, n) in modes))
out.append(fmt("bat-ratio", ratio, "frequency ratio to the (0,1) fundamental"))
out.append(fmt("bat-open", open_w, f"open strike r={OPEN_R} th={STRIKE_TH}: psi/M * tip lowpass, scaled so (0,1)=0.2855"))
out.append(fmt("bat-edge", edge_w, f"rim strike r={EDGE_R} th={STRIKE_TH}"))
out.append(fmt("bat-mic", bat_mic, f"batter mic psi at r={BAT_MIC[0]} th={BAT_MIC[1]}"))
out.append(fmt("bat-mic-abs", np.abs(bat_mic), "|bat-mic| for the BRIGHT normalisation"))
out.append(fmt("res-mic", res_mic, f"reso mic psi at r={RES_MIC[0]} th={RES_MIC[1]}"))
out.append(fmt("bat-palm", palm, f"palm energy fraction, gaussian bump at r={PALM[0]} th={PALM[1]} sigma={PALM[2]}, max-normalised"))
out.append(fmt("bat-split", dev, "sin-partner detune sign*magnitude (x split), 0 on the cos set and on m=0"))
out.append(fmt("bat-asym", asym, "m>0 selector: (0,n) slots belong to the scalar proxies"))
for k, (d, t) in enumerate(zip(WIRE_OFF, wire_pts), start=1):
    out.append(fmt(f"wpt{k}", t, f"reso head psi at wire {k} contact point, offset {d:+.2f} along the {WIRE_TH} deg diameter"))
out.append("; proxy (0,1) (0,2) (0,3) displacement at each wire contact point: J0(j_0n |offset|)")
for k, w in enumerate(wire_prox, start=1):
    out.append(f"(def wpx{k}a {w[0]:.4f}) (def wpx{k}b {w[1]:.4f}) (def wpx{k}c {w[2]:.4f})")
# scalar constants the dsp needs
o0 = [open_w[0, 0], open_w[0, 3], open_w[1, 2]]
e0 = [edge_w[0, 0], edge_w[0, 3], edge_w[1, 2]]
vol = [2 * np.pi * J(1, np.array([j]))[0] / j for (j, n) in prox]
vol = [v / vol[0] for v in vol]
out.append("; (0,n) proxy constants: open/edge strike weights, air volume (normalised to (0,1)),")
out.append("; mic readouts, palm fractions, ratios")
out.append(f"(def px-open1 {o0[0]:.4f}) (def px-open2 {o0[1]:.4f}) (def px-open3 {o0[2]:.4f})")
out.append(f"(def px-edge1 {e0[0]:.4f}) (def px-edge2 {e0[1]:.4f}) (def px-edge3 {e0[2]:.4f})")
out.append(f"(def px-vol2 {vol[1]:.4f}) (def px-vol3 {vol[2]:.4f})")
out.append(f"(def px-bmic1 {bat_mic[0,0]:.4f}) (def px-bmic2 {bat_mic[0,3]:.4f}) (def px-bmic3 {bat_mic[1,2]:.4f})")
out.append(f"(def px-rmic1 {res_mic[0,0]:.4f}) (def px-rmic2 {res_mic[0,3]:.4f}) (def px-rmic3 {res_mic[1,2]:.4f})")
out.append(f"(def px-palm1 {palm[0,0]:.4f}) (def px-palm2 {palm[0,3]:.4f}) (def px-palm3 {palm[1,2]:.4f})")
out.append(f"(def px-r2 {ratio[0,3]:.4f}) (def px-r3 {ratio[1,2]:.4f})")
out.append(f"(def mic-abs-sum {np.abs(bat_mic).sum():.4f})")
block = "\n".join(out) + "\n"

here = os.path.dirname(os.path.abspath(__file__))
dsp = os.path.join(here, "dsp.lisp")
if os.path.exists(dsp):
    src = open(dsp).read()
    new = re.sub(r"(;; BEGIN GENERATED TABLES.*?\n).*?(;; END GENERATED TABLES)",
                 lambda m: m.group(1) + block + m.group(2), src, flags=re.S)
    open(dsp, "w").write(new)
    print("wrote tables into", dsp)
else:
    print(block)
# sanity: compare against modal-snare's baked cos tables
print("modes:", [(m, n) for (j, m, n) in modes])
print("(0,1) open", open_w[0,0], "(1,1) open", open_w[0,1], "max|open|", np.abs(open_w).max())
print("ratio row0", np.round(ratio[0], 4))
print("bat-mic row0", np.round(bat_mic[0], 4), "abs sum", np.abs(bat_mic).sum())
print("res-mic row0", np.round(res_mic[0], 4))
print("edge (0,n)", e0, "vol", vol)
print("palm row0", np.round(palm[0], 3))
