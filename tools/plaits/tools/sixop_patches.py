#!/usr/bin/env python3
"""Unpack the three Plaits DX7 banks (syx_bank_0..2) into a numeric patch
tensor for engines/sixop.lisp.

Row 0 = defaults of an LFO that never had a patch loaded (fm::Lfo::Init);
row 1 + bank * 32 + patch = patch data. Everything the C++ precomputes at
patch-load time (fm::Voice::Setup, OperatorEnvelope::Set, PitchEnvelope::Set,
Lfo::Set, Algorithms<6>) is precomputed here in float32.

Writes data/sixop_patches.json ({"shape":[97, NCOL]}, column-major) and
data/sixop_cube_root.json. Column indices are exported via COLS for the
generator (tools/gen_sixop.py).
"""
import json, os, re, sys
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
# Data files live in the instrument folder (PLAITS_DATA overrides).
DATA = os.environ.get("PLAITS_DATA", os.path.join(os.path.dirname(os.path.dirname(ROOT)), "content", "instruments", "Synths", "Plaits", "data"))
MI = os.environ["EURORACK"]
sys.path.insert(0, HERE)
from lut import array  # noqa: E402

F = np.float32


def c_array(path, name):
    src = open(path).read()
    m = re.search(r"\b" + re.escape(name) + r"\[\]\s*=\s*\{(.*?)\};", src, re.S)
    body = re.sub(r"//.*", "", m.group(1))
    return [float(v) for v in re.findall(r"-?\d+\.?\d*(?:e[-+]?\d+)?", body)]


UNITS = os.path.join(MI, "stmlib", "dsp", "units.cc")
RATIO_HIGH = [F(v) for v in c_array(UNITS, "lut_pitch_ratio_high")]
RATIO_LOW = [F(v) for v in c_array(UNITS, "lut_pitch_ratio_low")]

LUT_COARSE = [F(v) for v in [
    -12.000000, 0.000000, 12.000000, 19.019550, 24.000000, 27.863137, 31.019550, 33.688259,
    36.000000, 38.039100, 39.863137, 41.513180, 43.019550, 44.405276, 45.688259, 46.882687,
    48.000000, 49.049554, 50.039100, 50.975130, 51.863137, 52.707809, 53.513180, 54.282743,
    55.019550, 55.726274, 56.405276, 57.058650, 57.688259, 58.295772, 58.882687, 59.450356]]
LUT_AMS = [F(0.0), F(0.2588), F(0.4274), F(1.0)]
LUT_PMS = [F(v) for v in [0.0, 0.0781250, 0.1562500, 0.2578125, 0.4296875, 0.7187500, 1.1953125, 2.0]]
LUT_CUBE_ROOT = [0.0, 0.39685062976, 0.5, 0.57235744065, 0.62996081605, 0.67860466725,
                 0.72112502092, 0.75914745216, 0.79370070937, 0.82548197054, 0.85498810729,
                 0.88258719406, 0.90856038354, 0.93312785379, 0.95646563396, 0.97871693135, 1.0]

SR = F(47872.34)
ONE_HZ = F(1.0) / SR
ENV_SCALE = F(44100.0) * ONE_HZ

# Algorithms<6>::opcodes_ (algorithms.cc).
MOD = lambda n: n << 4
ADD = lambda n: n | 4
OUT = lambda n: n
FB_SRC = 0x40
FB_DST = MOD(3)
FB = FB_SRC | FB_DST
NO_MOD = MOD(0)
OUTPUT = ADD(0)
OPCODES = [
    [FB | OUT(1), MOD(1) | OUT(1), MOD(1) | OUT(1), MOD(1) | OUTPUT, NO_MOD | OUT(1), MOD(1) | ADD(0)],
    [NO_MOD | OUT(1), MOD(1) | OUT(1), MOD(1) | OUT(1), MOD(1) | OUTPUT, FB | OUT(1), MOD(1) | ADD(0)],
    [FB | OUT(1), MOD(1) | OUT(1), MOD(1) | OUTPUT, NO_MOD | OUT(1), MOD(1) | OUT(1), MOD(1) | ADD(0)],
    [FB_DST | NO_MOD | OUT(1), MOD(1) | OUT(1), FB_SRC | MOD(1) | OUTPUT, NO_MOD | OUT(1), MOD(1) | OUT(1), MOD(1) | ADD(0)],
    [FB | OUT(1), MOD(1) | OUTPUT, NO_MOD | OUT(1), MOD(1) | ADD(0), NO_MOD | OUT(1), MOD(1) | ADD(0)],
    [FB_DST | NO_MOD | OUT(1), FB_SRC | MOD(1) | OUTPUT, NO_MOD | OUT(1), MOD(1) | ADD(0), NO_MOD | OUT(1), MOD(1) | ADD(0)],
    [FB | OUT(1), MOD(1) | OUT(1), NO_MOD | ADD(1), MOD(1) | OUTPUT, NO_MOD | OUT(1), MOD(1) | ADD(0)],
    [NO_MOD | OUT(1), MOD(1) | OUT(1), FB | ADD(1), MOD(1) | OUTPUT, NO_MOD | OUT(1), MOD(1) | ADD(0)],
    [NO_MOD | OUT(1), MOD(1) | OUT(1), NO_MOD | ADD(1), MOD(1) | OUTPUT, FB | OUT(1), MOD(1) | ADD(0)],
    [NO_MOD | OUT(1), NO_MOD | ADD(1), MOD(1) | OUTPUT, FB | OUT(1), MOD(1) | OUT(1), MOD(1) | ADD(0)],
    [FB | OUT(1), NO_MOD | ADD(1), MOD(1) | OUTPUT, NO_MOD | OUT(1), MOD(1) | OUT(1), MOD(1) | ADD(0)],
    [NO_MOD | OUT(1), NO_MOD | ADD(1), NO_MOD | ADD(1), MOD(1) | OUTPUT, FB | OUT(1), MOD(1) | ADD(0)],
    [FB | OUT(1), NO_MOD | ADD(1), NO_MOD | ADD(1), MOD(1) | OUTPUT, NO_MOD | OUT(1), MOD(1) | ADD(0)],
    [FB | OUT(1), NO_MOD | ADD(1), MOD(1) | OUT(1), MOD(1) | OUTPUT, NO_MOD | OUT(1), MOD(1) | ADD(0)],
    [NO_MOD | OUT(1), NO_MOD | ADD(1), MOD(1) | OUT(1), MOD(1) | OUTPUT, FB | OUT(1), MOD(1) | ADD(0)],
    [FB | OUT(1), MOD(1) | OUT(1), NO_MOD | OUT(2), MOD(2) | ADD(1), NO_MOD | ADD(1), MOD(1) | OUTPUT],
    [NO_MOD | OUT(1), MOD(1) | OUT(1), NO_MOD | OUT(2), MOD(2) | ADD(1), FB | ADD(1), MOD(1) | OUTPUT],
    [NO_MOD | OUT(1), MOD(1) | OUT(1), MOD(1) | OUT(1), FB | ADD(1), NO_MOD | ADD(1), MOD(1) | OUTPUT],
    [FB | OUT(1), MOD(1) | OUTPUT, MOD(1) | ADD(0), NO_MOD | OUT(1), MOD(1) | OUT(1), MOD(1) | ADD(0)],
    [NO_MOD | OUT(1), NO_MOD | ADD(1), MOD(1) | OUTPUT, FB | OUT(1), MOD(1) | ADD(0), MOD(1) | ADD(0)],
    [NO_MOD | OUT(1), MOD(1) | OUTPUT, MOD(1) | ADD(0), FB | OUT(1), MOD(1) | ADD(0), MOD(1) | ADD(0)],
    [FB | OUT(1), MOD(1) | OUTPUT, MOD(1) | ADD(0), MOD(1) | ADD(0), NO_MOD | OUT(1), MOD(1) | ADD(0)],
    [FB | OUT(1), MOD(1) | OUTPUT, MOD(1) | ADD(0), NO_MOD | OUT(1), MOD(1) | ADD(0), NO_MOD | ADD(0)],
    [FB | OUT(1), MOD(1) | OUTPUT, MOD(1) | ADD(0), MOD(1) | ADD(0), NO_MOD | ADD(0), NO_MOD | ADD(0)],
    [FB | OUT(1), MOD(1) | OUTPUT, MOD(1) | ADD(0), NO_MOD | ADD(0), NO_MOD | ADD(0), NO_MOD | ADD(0)],
    [FB | OUT(1), NO_MOD | ADD(1), MOD(1) | OUTPUT, NO_MOD | OUT(1), MOD(1) | ADD(0), NO_MOD | ADD(0)],
    [NO_MOD | OUT(1), NO_MOD | ADD(1), MOD(1) | OUTPUT, FB | OUT(1), MOD(1) | ADD(0), NO_MOD | ADD(0)],
    [NO_MOD | OUTPUT, FB | OUT(1), MOD(1) | OUT(1), MOD(1) | ADD(0), NO_MOD | OUT(1), MOD(1) | ADD(0)],
    [FB | OUT(1), MOD(1) | OUTPUT, NO_MOD | OUT(1), MOD(1) | ADD(0), NO_MOD | ADD(0), NO_MOD | ADD(0)],
    [NO_MOD | OUTPUT, FB | OUT(1), MOD(1) | OUT(1), MOD(1) | ADD(0), NO_MOD | ADD(0), NO_MOD | ADD(0)],
    [FB | OUT(1), MOD(1) | OUTPUT, NO_MOD | ADD(0), NO_MOD | ADD(0), NO_MOD | ADD(0), NO_MOD | ADD(0)],
    [FB | OUTPUT, NO_MOD | ADD(0), NO_MOD | ADD(0), NO_MOD | ADD(0), NO_MOD | ADD(0), NO_MOD | ADD(0)],
]

# ---- column layout -------------------------------------------------------
G = ["alg", "fb_scale", "reset_phase", "lfo_freq", "lfo_di0", "lfo_di1", "lfo_wave",
     "lfo_reset", "lfo_amd", "lfo_pmd",
     "pl0", "pl1", "pl2", "pl3", "pi0", "pi1", "pi2", "pi3", "pd0", "pd1", "pd2", "pd3"]
OPF = ["l0", "l1", "l2", "l3", "i0", "i1", "i2", "i3", "d0", "d1", "d2", "d3",
       "headroom", "ratio", "rs", "bp", "ld", "rd", "lc", "rc", "vs", "ams", "is_mod",
       "src", "dst", "add", "fbsrc", "kc", "kb", "rt"]
COLS = {n: i for i, n in enumerate(G)}
for o in range(6):
    for j, n in enumerate(OPF):
        COLS[f"o{o}_{n}"] = len(G) + o * len(OPF) + j
NCOL = len(G) + 6 * len(OPF)
NROW = 97


def semitones_to_ratio(s):
    pitch = F(s) + F(128.0)
    i = int(pitch)
    frac = F(pitch - F(i))
    return F(RATIO_HIGH[i] * RATIO_LOW[int(F(frac * F(256.0)))])


def semitones_to_ratio_safe(s):
    s = F(s)
    scale = F(1.0)
    while s > 120.0:
        s = F(s - F(120.0)); scale = F(scale * F(1024.0))
    while s < -120.0:
        s = F(s + F(120.0)); scale = F(scale * F(1.0 / 1024.0))
    return F(scale * semitones_to_ratio(s))


def operator_level(level):
    tlc = int(level)
    if level < 20:
        tlc = (tlc * (36 - tlc)) >> 3 if tlc < 15 else 27 + tlc
    else:
        tlc += 28
    return tlc


def pitch_env_level(level):
    l = F((F(level) - F(50.0)) / F(32.0))
    tail = F(max(F(abs(F(l + F(0.02)))) - F(1.0), F(0.0)))
    return F(l * F(F(1.0) + F(F(tail * tail) * F(5.3056))))


def op_env_increment(rate):
    rs = (rate * 41) >> 6
    mantissa = 4 + (rs & 3)
    exponent = 2 + (rs >> 2)
    return F(F(mantissa << exponent) / F(1 << 24))


def pitch_env_increment(rate):
    r = F(F(rate) * F(0.01))
    num = F(F(1.0) + F(F(F(192.0) * r) * F(F(r * r * r * r) + F(0.3333))))
    return F(num / F(F(21.3) * F(44100.0)))


def lfo_frequency(rate):
    rs = 1 if rate == 0 else (rate * 165) >> 6
    rs *= 11 if rs < 160 else (11 + ((rs - 160) >> 4))
    return F(F(rs) * F(0.005865))


def lfo_delay(delay):
    if delay == 0:
        return F(100000.0), F(100000.0)
    d = 99 - delay
    d = (16 + (d & 15)) << (1 + (d >> 4))
    return F(F(d) * F(0.005865)), F(F(max(0x80, d & 0xff80)) * F(0.005865))


def unpack(data):
    p = {"op": []}
    for i in range(6):
        od = data[i * 17:(i + 1) * 17]
        o = {
            "rate": [min(od[j] & 0x7f, 99) for j in range(4)],
            "level": [min(od[4 + j] & 0x7f, 99) for j in range(4)],
            "bp": min(od[8] & 0x7f, 99), "ld": min(od[9] & 0x7f, 99), "rd": min(od[10] & 0x7f, 99),
            "lc": od[11] & 3, "rc": (od[11] >> 2) & 3,
            "rs": od[12] & 7, "ams": od[13] & 3, "vs": (od[13] >> 2) & 7,
            "olevel": min(od[14] & 0x7f, 99), "mode": od[15] & 1, "coarse": (od[15] >> 1) & 0x1f,
            "fine": min(od[16] & 0x7f, 99), "detune": min((od[12] >> 3) & 0xf, 14),
        }
        p["op"].append(o)
    p["prate"] = [min(data[102 + j] & 0x7f, 99) for j in range(4)]
    p["plevel"] = [min(data[106 + j] & 0x7f, 99) for j in range(4)]
    p["alg"] = data[110] & 0x1f
    p["fb"] = data[111] & 7
    p["reset_phase"] = (data[111] >> 3) & 1
    p["lrate"] = min(data[112] & 0x7f, 99)
    p["ldelay"] = min(data[113] & 0x7f, 99)
    p["lpmd"] = min(data[114] & 0x7f, 99)
    p["lamd"] = min(data[115] & 0x7f, 99)
    p["lreset"] = data[116] & 1
    p["lwave"] = min((data[116] >> 1) & 7, 5)
    p["lpms"] = data[116] >> 4
    p["name"] = bytes(b & 0x7f for b in data[118:128]).decode("ascii", "replace")
    return p


def op_envelope(o):
    global_level = operator_level(o["olevel"])
    lv = []
    for i in range(4):
        ls = operator_level(o["level"][i])
        ls = (ls & ~1) + global_level - 133
        lv.append(F(F(0.125) * (F(0.5) if ls < 1 else F(ls))))
    inc = []
    for i in range(4):
        increment = op_env_increment(o["rate"][i])
        fr, to = lv[(i - 1) % 4], lv[i]
        if fr == to:
            increment = F(increment * F(0.6))
            if i == 0 and not o["level"][i]:
                increment = F(increment * F(20.0))
        elif fr < to:
            fr = max(F(6.7), fr); to = max(F(6.7), to)
            if fr == to:
                increment = F(1.0)
            else:
                increment = F(increment * F(F(7.2) / F(to - fr)))
        else:
            increment = F(increment * F(F(1.0) / F(fr - to)))
        inc.append(F(increment * ENV_SCALE))
    return lv, inc


def pitch_envelope(p):
    lv = [pitch_env_level(l) for l in p["plevel"]]
    inc = []
    for i in range(4):
        fr, to = lv[(i - 1) % 4], lv[i]
        increment = pitch_env_increment(p["prate"][i])
        if fr != to:
            increment = F(increment * F(F(1.0) / F(abs(F(fr - to)))))
        elif i != 3:
            increment = F(0.2)
        inc.append(F(increment * ENV_SCALE))
    return lv, inc


def frequency_ratio(o):
    detune = F(F(1.0) + F(F(0.01) * F(o["fine"]))) if (o["mode"] == 0 and o["fine"]) else F(1.0)
    if o["mode"] == 0:
        base = LUT_COARSE[o["coarse"]]
    else:
        base = F(F((o["coarse"] & 3) * 100 + o["fine"]) * F(0.39864))
    base = F(base + F(F(F(o["detune"]) - F(7.0)) * F(0.015)))
    return F(semitones_to_ratio_safe(base) * detune)


def row_for(p):
    r = [0.0] * NCOL
    c = COLS
    r[c["alg"]] = p["alg"]
    r[c["fb_scale"]] = float(F(F(1 << p["fb"]) / F(512.0))) if p["fb"] else 0.0
    r[c["reset_phase"]] = p["reset_phase"]
    r[c["lfo_freq"]] = float(F(lfo_frequency(p["lrate"]) * ONE_HZ))
    d0, d1 = lfo_delay(p["ldelay"])
    r[c["lfo_di0"]] = float(F(d0 * ONE_HZ))
    r[c["lfo_di1"]] = float(F(d1 * ONE_HZ))
    r[c["lfo_wave"]] = p["lwave"]
    r[c["lfo_reset"]] = 1 if p["lreset"] else 0
    r[c["lfo_amd"]] = float(F(F(p["lamd"]) * F(0.01)))
    r[c["lfo_pmd"]] = float(F(F(F(p["lpmd"]) * F(0.01)) * LUT_PMS[p["lpms"]]))
    lv, inc = pitch_envelope(p)
    for j in range(4):
        r[c[f"pl{j}"]] = float(lv[j]); r[c[f"pi{j}"]] = float(inc[j])
        r[c[f"pd{j}"]] = float(F(F(1.0) / inc[j]))
    opc = OPCODES[p["alg"]]
    for i, o in enumerate(p["op"]):
        k = lambda n: c[f"o{i}_{n}"]
        lv, inc = op_envelope(o)
        for j in range(4):
            r[k(f"l{j}")] = float(lv[j]); r[k(f"i{j}")] = float(inc[j])
            r[k(f"d{j}")] = float(F(F(1.0) / inc[j]))
        r[k("headroom")] = float(127 - operator_level(o["olevel"]))
        sign = 1.0 if o["mode"] == 0 else -1.0
        r[k("ratio")] = sign * float(frequency_ratio(o))
        r[k("rs")] = o["rs"]
        for n in ("bp", "ld", "rd", "lc", "rc", "vs"):
            r[k(n)] = o[n]
        r[k("ams")] = float(LUT_AMS[o["ams"]])
        op = opc[i]
        r[k("is_mod")] = 1 if (op & 3) else 0
        r[k("src")] = (op & 0x30) >> 4
        r[k("dst")] = op & 3
        r[k("add")] = 1 if op & 4 else 0
        r[k("fbsrc")] = 1 if op & 0x40 else 0
        assert (op & 3) != 0 or (op & 4), "output writes must be additive"
        # packed integer fields (exact in float32), decoded with power-of-2 math
        r[k("kc")] = o["lc"] + 4 * o["rc"] + 16 * o["rs"] + 128 * o["vs"] + 1024 * o["ams"] + 4096 * r[k("is_mod")]
        r[k("kb")] = o["bp"] + 128 * o["ld"] + 16384 * o["rd"]
        r[k("rt")] = r[k("src")] + 4 * r[k("dst")] + 16 * r[k("add")] + 32 * r[k("fbsrc")]
    return r


def default_row():
    r = [0.0] * NCOL
    r[COLS["lfo_freq"]] = 0.1
    r[COLS["lfo_di0"]] = 0.1
    r[COLS["lfo_di1"]] = 0.1
    for j in range(4):
        r[COLS[f"pi{j}"]] = 0.001; r[COLS[f"pd{j}"]] = 1000.0
        for i in range(6):
            r[COLS[f"o{i}_i{j}"]] = 0.001; r[COLS[f"o{i}_d{j}"]] = 1000.0
    return r


def build():
    rows = [default_row()]
    names = []
    for b in range(3):
        data = array(f"syx_bank_{b}")
        assert len(data) >= 32 * 128, len(data)
        for i in range(32):
            p = unpack(data[i * 128:(i + 1) * 128])
            names.append(p["name"])
            rows.append(row_for(p))
    flat = [rows[r][col] for col in range(NCOL) for r in range(NROW)]
    with open(os.path.join(DATA, "sixop_patches.json"), "w") as f:
        json.dump({"shape": [NROW, NCOL],
                   "source": "plaits/resources.cc syx_bank_0..2 unpacked by tools/sixop_patches.py (MIT, Emilie Gillet)",
                   "data": flat}, f, separators=(",", ":"))
    cr = LUT_CUBE_ROOT + [1.0]
    with open(os.path.join(DATA, "sixop_cube_root.json"), "w") as f:
        json.dump({"shape": [len(cr), 1], "source": "plaits/dsp/fm/dx_units.cc lut_cube_root (+1 pad)",
                   "data": cr}, f, separators=(",", ":"))
    return names


if __name__ == "__main__":
    names = build()
    print(f"{NROW} rows x {NCOL} cols")
    for b in range(3):
        print(b, names[b * 32:(b + 1) * 32])
