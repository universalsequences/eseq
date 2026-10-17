"""Float64 Python model of elements::Resonator::Process (constant params,
32 kHz, 16-sample blocks) for diagnosing the port."""
import json, os, math
import numpy as np
DATA = os.environ.get("ELEMENTS_DATA")
def lut(name):
    return np.array(json.load(open(os.path.join(DATA, name + ".json")))["data"], dtype=np.float64)
STIFF = lut("lut_stiffness"); DEC4 = lut("lut_4_decades")
def interp(t, x, size):
    x *= size; i = int(x); f = x - i
    return t[i] + (t[i + 1] - t[i]) * f
def tanfast(f):
    a = 3.260e-01 * math.pi ** 3; b = 1.823e-01 * math.pi ** 5
    return f * (math.pi + f * f * (a + b * f * f))
def cos_coef(freq):
    sign = 16.0; freq -= 0.25
    if freq < 0: freq = -freq
    elif freq > 0.5: freq -= 0.5
    else: sign = -16.0
    return sign * freq * (1 - 2 * freq)
class SVF:
    def __init__(s): s.g = tanfast(0.01); s.r = 0.01; s.h = 1/(1+s.r*s.g+s.g*s.g); s.s1 = s.s2 = 0
    def set(s, g, r): s.g = g; s.r = r; s.h = 1/(1+r*g+g*g)
    def bp(s, x):
        hp = (x - s.r*s.s1 - s.g*s.s1 - s.s2)*s.h; bp = s.g*hp + s.s1; s.s1 = s.g*hp + bp
        lp = s.g*bp + s.s2; s.s2 = s.g*bp + lp; return bp
def run(raw, f, geometry, brightness, damping, position, mod_freq=0.5/32000, mod_offset=0.1, bowed=True, modes=True, divider=True):
    N = 52; f_ = [SVF() for _ in range(N)]; fb = [SVF() for _ in range(8)]
    lines = [np.zeros(1024) for _ in range(8)]; wp = [0]*8; per = [1]*8
    cd = 0; lfo = 0.0; prev_pos = 0.0; bow_sig = 0.0
    center = np.zeros(len(raw)); sides = np.zeros(len(raw))
    for b0 in range(0, len(raw), 16):
        cd += 1
        stiff = interp(STIFF, geometry, 256); harmonic = f; sf = 1.0
        q = 500 * interp(DEC4, damping * 0.8, 256)
        ba = (1 - geometry) ** 8
        br = brightness * (1 - 0.2 * ba)
        ql = br * (2 - br) * 0.85 + 0.15; qr = geometry * (2 - geometry) * 0.1
        nm = 0
        for i in range(N):
            upd = (not divider) or i <= 24 or ((i & 1) == (cd & 1))
            pf = harmonic * sf
            if pf >= 0.49: pf = 0.49
            else: nm = i + 1
            if upd:
                f_[i].set(tanfast(pf), 1/(1 + pf*q))
                if i < 8:
                    p = int(1.0/pf)
                    while p >= 1024: p >>= 1
                    per[i] = p
                    fb[i].set(f_[i].g, 1/(1 + pf*1500))
            sf += stiff
            stiff *= 0.93 if stiff < 0 else 0.98
            ql += qr * (1 - ql); harmonic += f; q *= ql
        nwg = min(8, nm)
        inc = (position - prev_pos) / 16
        for j in range(16):
            n = b0 + j
            if n >= len(raw): break
            lfo += mod_freq
            if lfo >= 1: lfo -= 1
            prev_pos += inc
            l = 1 - lfo if lfo > 0.5 else lfo
            c1 = cos_coef(prev_pos); c2 = cos_coef(mod_offset + l)
            x = raw[n] * 0.125
            sc = ss = 0.0
            y1, y0 = c1*0.25, 0.5; z1, z0 = c2*0.25, 0.5
            for i in range(nm):
                s = f_[i].bp(x)
                t = y0; y0 = c1*y0 - y1; y1 = t; a = t + 0.5
                t = z0; z0 = c2*z0 - z1; z1 = t; a2 = t + 0.5
                if modes:
                    sc += s * a; ss += s * a2
            sides[n] = ss - sc
            bs = 0.0; xin = x + bow_sig
            y1, y0 = c1*0.25, 0.5
            for i in range(nwg):
                s = 0.99 * lines[i][(wp[i] + per[i]) % 1024]
                bs += s
                s = fb[i].bp(xin + s) * fb[i].r
                lines[i][wp[i]] = s; wp[i] = (wp[i] - 1) % 1024
                t = y0; y0 = c1*y0 - y1; y1 = t
                if bowed: sc += s * (t + 0.5) * 8
            xx = 0.0 - bs
            bw = (abs(xx*6) + 0.75) ** 4
            bow_sig = xx * min(max(0.25/bw, 0.0025), 0.245)
            center[n] = sc
    return center, sides
