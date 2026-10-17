#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Felucca FM6 factory instrument: writes dsp.lisp, ui.lisp and the preset bank.

  python3 tools/felucca-fm6/generate.py

A port of the FM6 engine of the Felucca firmware for the FM-1
(https://github.com/hugelton/Felucca, firmware/src/eng_fm6.c + fm6_core.c),
itself a C port of msfa, the 6-operator FM core of Dexed:
  Copyright 2012 Google Inc., 2016-2025 Pascal Gauthier (Apache-2.0);
  C port and engine: Copyright (C) 2026 Leo Kuroshita (@kurogedelic),
  Hügelton Instruments (GPL-3.0-only).
The tables below are msfa's (fm6_core.c / tools/gen_tables.py) and the
patches are Felucca's own factory patches (tools/gen_fm6_patches.py), copied
here so the instrument regenerates without a Felucca checkout.
"""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "content/instruments/Synths/Felucca FM6"
PRESETS = ROOT / "content/instruments/Synths/Felucca FM6.presets"

# ---------------------------------------------------------------- msfa tables
LEVELLUT = [0, 5, 9, 13, 17, 20, 23, 25, 27, 29, 31, 33, 35, 37, 39, 41, 42, 43, 45, 46]
STATICS = [
    1764000, 1764000, 1411200, 1411200, 1190700, 1014300, 992250, 882000, 705600, 705600, 584325, 507150, 502740,
    441000, 418950, 352800, 308700, 286650, 253575, 220500, 220500, 176400, 145530, 145530, 125685, 110250, 110250,
    88200, 88200, 74970, 61740, 61740, 55125, 48510, 44100, 37485, 31311, 30870, 27562, 27562, 22050, 18522, 17640,
    15435, 14112, 13230, 11025, 9261, 9261, 7717, 6615, 6615, 5512, 5512, 4410, 3969, 3969, 3439, 2866, 2690, 2249,
    1984, 1896, 1808, 1411, 1367, 1234, 1146, 926, 837, 837, 705, 573, 573, 529, 441, 441]
PENV_RATE = [
    1, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14, 14, 15, 16, 16, 17, 18, 18,
    19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 30, 31, 33, 34, 36, 37, 38, 39, 41, 42, 44, 46, 47, 49, 51, 53, 54, 56,
    58, 60, 62, 64, 66, 68, 70, 72, 74, 76, 79, 82, 85, 88, 91, 94, 98, 102, 106, 110, 115, 120, 125, 130, 135, 141,
    147, 153, 159, 165, 171, 178, 185, 193, 202, 211, 232, 243, 254, 255]
PENV_TAB = [
    -128, -116, -104, -95, -85, -76, -68, -61, -56, -52, -49, -46, -43, -41, -39, -37, -35, -33, -32, -31, -30, -29,
    -28, -27, -26, -25, -24, -23, -22, -21, -20, -19, -18, -17, -16, -15, -14, -13, -12, -11, -10, -9, -8, -7, -6,
    -5, -4, -3, -2, -1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 38, 40, 43, 46, 49, 53, 58, 65, 73, 82, 92, 103, 115, 127]
VELDATA = [
    0, 70, 86, 97, 106, 114, 121, 126, 132, 138, 142, 148, 152, 156, 160, 163, 166, 170, 173, 174, 178, 181, 184,
    186, 189, 190, 194, 196, 198, 200, 202, 205, 206, 209, 211, 214, 216, 218, 220, 222, 224, 225, 227, 229, 230,
    232, 233, 235, 237, 238, 240, 241, 242, 243, 244, 246, 246, 248, 249, 250, 251, 252, 253, 254]
EXPSCALE = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 14, 16, 19, 23, 27, 33, 39, 47, 56, 66,
            80, 94, 110, 126, 142, 158, 174, 190, 206, 222, 238, 250]
PMS = [0, 10, 20, 33, 55, 92, 153, 255]
AMSENS = [0, 4342338, 7171437, 16777216]
LFO_SOURCE = [   # msfa lfo.cc: the LFO rates 0..99 (Hz)
    0.062541, 0.125031, 0.312393, 0.437120, 0.624610, 0.750694, 0.936330, 1.125302, 1.249609, 1.436782,
    1.560915, 1.752081, 1.875117, 2.062494, 2.247191, 2.374451, 2.560492, 2.686728, 2.873976, 2.998950,
    3.188013, 3.369840, 3.500175, 3.682224, 3.812065, 4.000800, 4.186202, 4.310716, 4.501260, 4.623209,
    4.814636, 4.930480, 5.121901, 5.315191, 5.434783, 5.617346, 5.750431, 5.946717, 6.062811, 6.248438,
    6.431695, 6.564264, 6.749460, 6.868132, 7.052186, 7.250580, 7.375719, 7.556294, 7.687577, 7.877738,
    7.993605, 8.181967, 8.372405, 8.504848, 8.685079, 8.810573, 8.986341, 9.122423, 9.300595, 9.500285,
    9.607994, 9.798158, 9.950249, 10.117361, 11.251125, 11.384335, 12.562814, 13.676149, 13.904338, 15.092062,
    16.366612, 16.638935, 17.869907, 19.193858, 19.425019, 20.833333, 21.034918, 22.502250, 24.003841, 24.260068,
    25.746653, 27.173913, 27.578599, 29.052876, 30.693677, 31.191516, 32.658393, 34.317090, 34.674064, 36.416606,
    38.197097, 38.550501, 40.387722, 40.749796, 42.625746, 44.326241, 44.883303, 46.772685, 48.590865, 49.261084]
# per operator, the sixth first: in bus (bits 4-5), out bus (0-1), add (2), feedback in+out (6, 7)
ALG = [
    [0xc1, 0x11, 0x11, 0x14, 0x01, 0x14], [0x01, 0x11, 0x11, 0x14, 0xc1, 0x14], [0xc1, 0x11, 0x14, 0x01, 0x11, 0x14],
    [0xc1, 0x11, 0x94, 0x01, 0x11, 0x14], [0xc1, 0x14, 0x01, 0x14, 0x01, 0x14], [0xc1, 0x94, 0x01, 0x14, 0x01, 0x14],
    [0xc1, 0x11, 0x05, 0x14, 0x01, 0x14], [0x01, 0x11, 0xc5, 0x14, 0x01, 0x14], [0x01, 0x11, 0x05, 0x14, 0xc1, 0x14],
    [0x01, 0x05, 0x14, 0xc1, 0x11, 0x14], [0xc1, 0x05, 0x14, 0x01, 0x11, 0x14], [0x01, 0x05, 0x05, 0x14, 0xc1, 0x14],
    [0xc1, 0x05, 0x05, 0x14, 0x01, 0x14], [0xc1, 0x05, 0x11, 0x14, 0x01, 0x14], [0x01, 0x05, 0x11, 0x14, 0xc1, 0x14],
    [0xc1, 0x11, 0x02, 0x25, 0x05, 0x14], [0x01, 0x11, 0x02, 0x25, 0xc5, 0x14], [0x01, 0x11, 0x11, 0xc5, 0x05, 0x14],
    [0xc1, 0x14, 0x14, 0x01, 0x11, 0x14], [0x01, 0x05, 0x14, 0xc1, 0x14, 0x14], [0x01, 0x14, 0x14, 0xc1, 0x14, 0x14],
    [0xc1, 0x14, 0x14, 0x14, 0x01, 0x14], [0xc1, 0x14, 0x14, 0x01, 0x14, 0x04], [0xc1, 0x14, 0x14, 0x14, 0x04, 0x04],
    [0xc1, 0x14, 0x14, 0x04, 0x04, 0x04], [0xc1, 0x05, 0x14, 0x01, 0x14, 0x04], [0x01, 0x05, 0x14, 0xc1, 0x14, 0x04],
    [0x04, 0xc1, 0x11, 0x14, 0x01, 0x14], [0xc1, 0x14, 0x01, 0x14, 0x04, 0x04], [0x04, 0xc1, 0x11, 0x14, 0x04, 0x04],
    [0xc1, 0x14, 0x04, 0x04, 0x04, 0x04], [0xc4, 0x04, 0x04, 0x04, 0x04, 0x04]]


# ----------------------------------------------------- Felucca's own patches
def op(r=(99, 99, 99, 99), l=(99, 99, 99, 0), ol=0, fc=1, ff=0, det=7, mode=0, kvs=0, ams=0, rs=0,
       bp=39, ld=0, rd=0, lc=0, rc=0):
    return dict(r=r, l=l, ol=ol, fc=fc, ff=ff, det=det, mode=mode, kvs=kvs, ams=ams, rs=rs, bp=bp, ld=ld, rd=rd,
                lc=lc, rc=rc)


def voice(name, alg, ops, fb=0, oks=1, pr=(99, 99, 99, 99), pl=(50, 50, 50, 50), lfs=35, lfd=0, lpmd=0, lamd=0,
          lks=1, lfw=0, lpms=3, trnsp=24):
    """OP1..OP6; alg 1..32 (oks: the format's oscillator key sync, unused by FM6: phases restart from silence)"""
    return dict(name=name, alg=alg, ops=ops, fb=fb, pr=pr, pl=pl, lfs=lfs, lfd=lfd, lpmd=lpmd, lamd=lamd,
                lks=lks, lfw=lfw, lpms=lpms, trnsp=trnsp)


INIT = voice("INIT VOICE", 1, [op(ol=99)] + [op() for _ in range(5)], lpms=3)
PATCHES = [   # (preset name, patch, Felucca's preset macros on top of it)
    ("Tine EP", voice("TINE EP", 5, [
        op(r=(96, 25, 25, 67), l=(99, 75, 0, 0), ol=98, kvs=2, rs=3),
        op(r=(95, 50, 35, 78), l=(99, 75, 0, 0), ol=58, kvs=7, rs=3),
        op(r=(95, 20, 20, 50), l=(99, 95, 0, 0), ol=90, kvs=2, det=8, rs=2),
        op(r=(97, 62, 40, 60), l=(99, 60, 0, 0), ol=68, fc=14, kvs=6, rs=3),
        op(r=(95, 30, 20, 60), l=(99, 90, 0, 0), ol=78, det=6, kvs=1),
        op(r=(95, 40, 30, 60), l=(99, 80, 0, 0), ol=52, kvs=3)],
        fb=3, lfs=34, lfd=33, lfw=4, lpms=2), {}),
    ("Bell", voice("GLASS BELL", 5, [
        op(r=(99, 40, 25, 35), l=(99, 80, 0, 0), ol=96, kvs=1),
        op(r=(99, 35, 25, 35), l=(99, 70, 0, 0), ol=78, fc=3, ff=50, kvs=3),
        op(r=(99, 45, 30, 40), l=(99, 70, 0, 0), ol=84, fc=2, ff=38, det=9, kvs=1),
        op(r=(99, 50, 30, 40), l=(99, 60, 0, 0), ol=70, fc=5, kvs=3),
        op(r=(99, 60, 40, 45), l=(99, 50, 0, 0), ol=78, fc=5, ff=40, det=5),
        op(r=(99, 60, 40, 45), l=(99, 50, 0, 0), ol=60)],
        fb=0, lpms=0), {}),
    ("FM Bass", voice("ROUND BASS", 5, [
        op(r=(99, 40, 30, 75), l=(99, 85, 70, 0), ol=99, kvs=2),
        op(r=(99, 60, 40, 75), l=(99, 72, 55, 0), ol=80, kvs=5),
        op(r=(99, 55, 40, 75), l=(99, 70, 50, 0), ol=80, det=8, kvs=1),
        op(r=(99, 72, 50, 75), l=(99, 65, 40, 0), ol=78, fc=2, kvs=6),
        op(ol=0),
        op(ol=0)],
        fb=0, lpms=0), {}),
    ("Brass", voice("BRASS SECT", 5, [
        op(r=(65, 50, 40, 60), l=(99, 92, 90, 0), ol=98, kvs=1),
        op(r=(55, 50, 40, 60), l=(99, 88, 85, 0), ol=80, kvs=3),
        op(r=(62, 50, 40, 60), l=(99, 92, 90, 0), ol=92, det=9, kvs=1),
        op(r=(50, 50, 40, 60), l=(99, 88, 85, 0), ol=78, kvs=3),
        op(r=(62, 50, 40, 60), l=(99, 92, 90, 0), ol=88, det=5, kvs=1),
        op(r=(52, 50, 40, 60), l=(99, 85, 82, 0), ol=72, kvs=2)],
        fb=5, pr=(80, 60, 99, 60), pl=(46, 50, 50, 50), lfs=33, lfd=50, lpmd=6, lfw=4, lpms=3), {}),
    ("Pad", voice("SOFT PAD", 5, [
        op(r=(40, 30, 40, 40), l=(99, 95, 90, 0), ol=92, det=5),
        op(r=(35, 30, 40, 40), l=(85, 90, 80, 0), ol=60, kvs=1),
        op(r=(40, 30, 40, 40), l=(99, 95, 90, 0), ol=92, det=10),
        op(r=(30, 25, 40, 40), l=(80, 90, 85, 0), ol=55, fc=2, kvs=1),
        op(r=(38, 30, 40, 40), l=(99, 95, 90, 0), ol=80, fc=2),
        op(r=(35, 30, 40, 40), l=(85, 90, 85, 0), ol=50)],
        fb=2, lfs=30, lfd=60, lpmd=4, lfw=4, lpms=2), {"detune_spread": 30}),
    ("Marimba", voice("WOOD BARS", 5, [
        op(r=(99, 52, 40, 55), l=(99, 0, 0, 0), ol=99, kvs=2, rs=2),
        op(r=(99, 75, 50, 60), l=(99, 0, 0, 0), ol=70, fc=4, kvs=5, rs=3),
        op(r=(99, 70, 50, 60), l=(99, 0, 0, 0), ol=72, fc=4, kvs=3, rs=3),
        op(r=(99, 80, 50, 60), l=(99, 0, 0, 0), ol=45, kvs=3, rs=3),
        op(ol=0),
        op(ol=0)],
        fb=0, lpms=0), {}),
    ("Organ", voice("DRAWBARS", 32, [
        op(r=(99, 99, 99, 85), l=(99, 99, 99, 0), ol=90),
        op(r=(99, 99, 99, 85), l=(99, 99, 99, 0), ol=84, fc=0),
        op(r=(99, 99, 99, 85), l=(99, 99, 99, 0), ol=86, fc=2),
        op(r=(99, 99, 99, 85), l=(99, 99, 99, 0), ol=76, fc=3),
        op(r=(99, 99, 99, 85), l=(99, 99, 99, 0), ol=78, fc=4),
        op(r=(99, 99, 99, 85), l=(99, 99, 99, 0), ol=72, fc=1, ff=50)],
        fb=0, lfs=60, lpmd=3, lfw=4, lpms=3), {}),
    ("Pluck", voice("NYLON PICK", 5, [
        op(r=(99, 45, 30, 60), l=(99, 60, 0, 0), ol=98, kvs=2, rs=2),
        op(r=(99, 70, 40, 60), l=(99, 40, 0, 0), ol=76, fc=3, kvs=4, rs=2),
        op(r=(99, 60, 40, 60), l=(99, 30, 0, 0), ol=74, fc=2, det=8, kvs=2, rs=2),
        op(r=(99, 75, 40, 60), l=(99, 30, 0, 0), ol=68, kvs=3, rs=2),
        op(ol=0),
        op(ol=0)],
        fb=0, lpms=0), {}),
]

# ------------------------------------------------------------------ params
OP_FIELDS = [   # (suffix, patch key, default from op(), min, max)
    ("rate1", ("r", 0), 0, 99), ("rate2", ("r", 1), 0, 99), ("rate3", ("r", 2), 0, 99), ("rate4", ("r", 3), 0, 99),
    ("level1", ("l", 0), 0, 99), ("level2", ("l", 1), 0, 99), ("level3", ("l", 2), 0, 99),
    ("level4", ("l", 3), 0, 99),
    ("out", "ol", 0, 99), ("coarse", "fc", 0, 31), ("fine", "ff", 0, 99), ("detune", "det", 0, 14),
    ("fixed", "mode", 0, 1), ("vel", "kvs", 0, 7), ("ams", "ams", 0, 3), ("rate_scale", "rs", 0, 7),
    ("break", "bp", 0, 99), ("ldepth", "ld", 0, 99), ("rdepth", "rd", 0, 99), ("lcurve", "lc", 0, 3),
    ("rcurve", "rc", 0, 3)]
VOICE_FIELDS = [   # (name, getter, min, max)
    ("algorithm", lambda v: v["alg"], 1, 32), ("feedback", lambda v: v["fb"], 0, 7),
    ("transpose", lambda v: v["trnsp"] - 24, -24, 24),
    ("peg_rate1", lambda v: v["pr"][0], 0, 99), ("peg_rate2", lambda v: v["pr"][1], 0, 99),
    ("peg_rate3", lambda v: v["pr"][2], 0, 99), ("peg_rate4", lambda v: v["pr"][3], 0, 99),
    ("peg_level1", lambda v: v["pl"][0], 0, 99), ("peg_level2", lambda v: v["pl"][1], 0, 99),
    ("peg_level3", lambda v: v["pl"][2], 0, 99), ("peg_level4", lambda v: v["pl"][3], 0, 99),
    ("lfo_speed", lambda v: v["lfs"], 0, 99), ("lfo_delay", lambda v: v["lfd"], 0, 99),
    ("lfo_pmd", lambda v: v["lpmd"], 0, 99), ("lfo_amd", lambda v: v["lamd"], 0, 99),
    ("lfo_sync", lambda v: v["lks"], 0, 1), ("lfo_wave", lambda v: v["lfw"], 0, 5),
    ("lfo_pms", lambda v: v["lpms"], 0, 7)]
# Felucca's EDIT macros on top of the patch (ALG and FB are the patch's own params here)
MACROS = [("mod_level", -64, 63), ("mod_ratio", -16, 16), ("mod_env", -64, 63), ("mod_vel", -7, 7),
          ("detune_spread", 0, 127)]
MOD_PARAMS = {"feedback", "mod_level", "detune_spread", "gain", "lfo_speed"}


def op_value(o, key):
    return o[key[0]][key[1]] if isinstance(key, tuple) else o[key]


def patch_params(v, macros=None):
    p = {}
    for n, o in enumerate(v["ops"], 1):
        for suffix, key, _, _ in OP_FIELDS:
            p[f"op{n}_{suffix}"] = float(op_value(o, key))
    for name, get, _, _ in VOICE_FIELDS:
        p[name] = float(get(v))
    for name, _, _ in MACROS:
        p[name] = float((macros or {}).get(name, 0))
    p["gain"] = 0.5
    return p


def param_line(name, default, lo, hi, unit=None):
    s = f"(param {name} @default {default:g} @min {lo:g} @max {hi:g}"
    if unit:
        s += f" @unit {unit}"
    if name in MOD_PARAMS:
        span = hi - lo
        s += f" @mod true @mod-mode additive @mod-depth-min {-span:g} @mod-depth-max {span:g}"
    return s + ")"


def tensor(name, values):
    return f"(def {name} (tensor @shape [{len(values)}] @data [{' '.join(f'{x:g}' for x in values)}]))"


# --------------------------------------------------------------------- DSP
HEADER = """\
; Felucca FM6: classic six-operator FM, ported from the FM6 engine of the
; Felucca firmware for the FM-1 (https://github.com/hugelton/Felucca,
; firmware/src/eng_fm6.c, fm6_core.c). GENERATED by
; tools/felucca-fm6/generate.py: edit the generator, not this file.
;
; The synthesis is msfa, the FM core of Dexed: Copyright 2012 Google Inc.,
; 2016-2025 Pascal Gauthier, Apache-2.0 (its envelope, pitch envelope, LFO,
; scaling tables and 32 algorithms). Felucca's C port, engine and factory
; patches: Copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hugelton
; Instruments, GPL-3.0-only, as is eseq.
;
; Per voice: six sine operators with DX-style 4-rate/4-level envelopes,
; keyboard level and rate scaling, velocity sensitivity, ratio or fixed
; frequency, detune, amplitude modulation sensitivity; one of 32 algorithms
; with feedback on its feedback operator; a pitch envelope; an LFO with
; delay, pitch and amplitude depth. Felucca's EDIT macros sit on top of the
; patch: Mod Level (non-carrier output, +-36 dB), Mod Ratio (non-carrier
; coarse), Mod EG (non-carrier rates, + slower), Mod Vel (non-carrier
; velocity sensitivity) and Detune Spread (carriers 0, +1, -1, +2, -2, +3
; steps of up to 12 cents).
;
; Changes from the firmware: envelopes, pitch envelope and LFO step every
; sample rather than every 32-sample block (rates converted from 44.1 kHz);
; the LFO is per voice (the firmware has one per track); the operator output
; threshold that skips silent operators is gone (they compute zeros).

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))
"""

MACRO_DEFS = r"""
; msfa's rates and times are 44.1 kHz ones.
(def sr_scale (/ 44100 samplerate))
(def inv_q24 (/ 1 16777216))
(def ln2 0.6931471805599453)

; A 0..99 output level on msfa's 0..127 scale.
(defmacro fz-scaleout (l)
  (gswitch (gte l 20) (+ 28 l) (peek FZ_LEVELLUT (clip l 0 19))))

; An envelope level's target, in 6 dB units (msfa: Q24 log2, 2^24 = 6 dB).
(defmacro fz-target (l outlevel)
  (/ (max 16 (+ (* (floor (/ (fz-scaleout l) 2)) 64) outlevel -4256)) 256))

; The per-sample step of an envelope rate (msfa's qrate, stepped per 32-sample block).
(defmacro fz-inc (rate rscale)
  (def q (min 63 (+ (floor (/ (* rate 41) 64)) rscale)))
  (* (+ 4 (% q 4)) (pow 2 (+ 2 (floor (/ q 4)))) inv_q24 sr_scale))

; How long (44.1 kHz samples) a stage holds when its level does not move, or
; an attack to level 0 (a delayed attack). 0 = it moves.
(defmacro fz-static (rate l target lvl attack rscale live)
  (def still (or (eq target lvl) (and attack (eq l 0))))
  (def sr (min 99 (+ rate rscale)))
  (def hold_n (gswitch (lt sr 77) (peek FZ_STATICS (clip sr 0 76)) (* 20 (- 99 sr))))
  (def hold_s (gswitch (and attack (eq l 0) (lt sr 77)) (floor (/ hold_n 20)) hold_n))
  (* live still hold_s))

; msfa's operator envelope: four rates, four levels, attack from silence
; (with a floor), exponential-in-dB decay; stage 3 holds while the key is
; down. Returns the level in 6 dB units.
(defmacro fz-env (trig key r1 r2 r3 r4 l1 l2 l3 l4 outlevel rscale)
  (make-history fz_lvl_h)
  (make-history fz_ix_h)
  (make-history fz_statc_h)
  (make-history fz_held_h)
  (def t1 (fz-target l1 outlevel))
  (def t2 (fz-target l2 outlevel))
  (def t3 (fz-target l3 outlevel))
  (def t4 (fz-target l4 outlevel))
  (def keyon (gt trig 0.5))
  (def keyoff (* (not keyon) (gt (read-history fz_held_h) 0.5) (lte key 0.5)))
  (def ix0 (gswitch keyon 0 (gswitch keyoff 3 (read-history fz_ix_h))))
  (def lvl0 (gswitch keyon 0 (read-history fz_lvl_h)))
  (def st0 (fz-static (selector (+ ix0 1) r1 r2 r3 r4 0) (selector (+ ix0 1) l1 l2 l3 l4 0)
    (selector (+ ix0 1) t1 t2 t3 t4 -1) lvl0 (eq ix0 0) rscale (lt ix0 4)))
  (def statc0 (gswitch (or keyon keyoff) st0 (read-history fz_statc_h)))
  (def holding (gt statc0 0))
  (def sdec (- statc0 sr_scale))
  (def expired (* holding (lte sdec 0)))
  (def active (or (lt ix0 3) (and (eq ix0 3) (lte key 0.5))))
  (def tgt (selector (+ ix0 1) t1 t2 t3 t4 lvl0))
  (def inc (selector (+ ix0 1) (fz-inc r1 rscale) (fz-inc r2 rscale) (fz-inc r3 rscale) (fz-inc r4 rscale) 0))
  (def floor_lvl (max lvl0 6.703125))
  (def up (min tgt (+ floor_lvl (* (floor (- 17 floor_lvl)) inc))))
  (def down (max tgt (- lvl0 inc)))
  (def moving (* active (not holding)))
  (def lvl1 (gswitch moving (gswitch (gt tgt lvl0) up down) lvl0))
  (def adv (or expired (* moving (eq lvl1 tgt))))
  (def ix1 (gswitch adv (+ ix0 1) ix0))
  (def st1 (fz-static (selector (+ ix1 1) r1 r2 r3 r4 0) (selector (+ ix1 1) l1 l2 l3 l4 0)
    (selector (+ ix1 1) t1 t2 t3 t4 -1) lvl1 (eq ix1 0) rscale (lt ix1 4)))
  (write-history fz_statc_h (gswitch adv st1 (gswitch holding sdec 0)))
  (write-history fz_ix_h ix1)
  (write-history fz_held_h key)
  (write-history fz_lvl_h lvl1))

; msfa's pitch envelope, in octaves: starts at L4, moves through L1..L3,
; holds L3 while the key is down, returns to L4.
(defmacro fz-pitch-env (trig key r1 r2 r3 r4 l1 l2 l3 l4)
  (make-history fz_plvl_h)
  (make-history fz_pix_h)
  (make-history fz_pheld_h)
  (def p1 (/ (peek FZ_PENV_TAB l1) 32))
  (def p2 (/ (peek FZ_PENV_TAB l2) 32))
  (def p3 (/ (peek FZ_PENV_TAB l3) 32))
  (def p4 (/ (peek FZ_PENV_TAB l4) 32))
  (def unit (/ sr_scale (* 21.3 44100)))
  (def keyon (gt trig 0.5))
  (def keyoff (* (not keyon) (gt (read-history fz_pheld_h) 0.5) (lte key 0.5)))
  (def ix0 (gswitch keyon 0 (gswitch keyoff 3 (read-history fz_pix_h))))
  (def lvl0 (gswitch keyon p4 (read-history fz_plvl_h)))
  (def active (or (lt ix0 3) (and (eq ix0 3) (lte key 0.5))))
  (def tgt (selector (+ ix0 1) p1 p2 p3 p4 lvl0))
  (def inc (* unit (selector (+ ix0 1) (peek FZ_PENV_RATE r1) (peek FZ_PENV_RATE r2)
    (peek FZ_PENV_RATE r3) (peek FZ_PENV_RATE r4) 0)))
  (def lvl1 (gswitch active
    (gswitch (gt tgt lvl0) (min tgt (+ lvl0 inc)) (max tgt (- lvl0 inc))) lvl0))
  (def ix1 (gswitch (* active (eq lvl1 tgt)) (+ ix0 1) ix0))
  (write-history fz_pix_h ix1)
  (write-history fz_pheld_h key)
  (write-history fz_plvl_h lvl1))

; msfa's LFO, 0..1: triangle, saw down, saw up, square, sine, sample & hold.
; Key sync restarts it at mid-cycle on each note.
(defmacro fz-lfo (trig speed wave sync)
  (make-history fz_lfo_prev_h)
  (make-history fz_lfo_rnd_h)
  (def ph (wrap (+ (phasor (peek FZ_LFO_HZ speed) (* trig sync)) 0.5) 0 1))
  (def wrapped (lt ph (read-history fz_lfo_prev_h)))
  (write-history fz_lfo_prev_h ph)
  (def prev_rnd (read-history fz_lfo_rnd_h))
  (def rnd (gswitch wrapped (% (+ (* prev_rnd 179) 17) 256) prev_rnd))
  (write-history fz_lfo_rnd_h rnd)
  (selector (+ wave 1)
    (gswitch (lt ph 0.5) (* 2 ph) (- 2 (* 2 ph)))
    (wrap (- 1.5 ph) 0 1)
    (wrap (+ ph 0.5) 0 1)
    (lt ph 0.5)
    (+ 0.5 (* 0.5 (sin (* twopi ph))))
    (/ (+ (% (+ rnd 128) 256) 1) 256)))

; msfa's LFO delay, 0..1: silent for the first half of its time, then a ramp.
(defmacro fz-lfo-delay (trig lfd)
  (make-history fz_dly_h)
  (def a (- 99 lfd))
  (def a1 (* (+ 16 (% a 16)) (pow 2 (+ 1 (floor (/ a 16))))))
  (def a2 (max 128 (* (floor (/ a1 128)) 128)))
  (def unit (* (/ 18279 (* 32 4294967296)) sr_scale))
  (def prev (gswitch (gt trig 0.5) 0 (read-history fz_dly_h)))
  (def next (min 1 (+ prev (* unit (gswitch (lt prev 0.5) a1 a2)))))
  (write-history fz_dly_h next)
  (gswitch (eq a 99) 1 (gswitch (lt next 0.5) 0 (* 2 (- next 0.5)))))

; One operator: a sine at hz, phase-modulated by `in` (cycles) and by its own
; last two outputs times fb (the feedback operator only), scaled by gain.
(defmacro fz-op (hz reset gain in fb)
  (make-history fz_y1_h)
  (make-history fz_y2_h)
  (def y1 (gswitch reset 0 (read-history fz_y1_h)))
  (def y2 (gswitch reset 0 (read-history fz_y2_h)))
  (def y (* gain (sin (* twopi (+ (phasor hz reset) in (* fb (+ y1 y2)))))))
  (write-history fz_y2_h y1)
  (write-history fz_y1_h y)
  y)
"""


def gen_dsp():
    L = [HEADER, "; ---- msfa's tables (fm6_core.c), and each operator's routing per algorithm"]
    L.append(tensor("FZ_LEVELLUT", LEVELLUT))
    L.append(tensor("FZ_STATICS", STATICS))
    L.append(tensor("FZ_PENV_RATE", PENV_RATE))
    L.append(tensor("FZ_PENV_TAB", PENV_TAB))
    L.append(tensor("FZ_VELDATA", VELDATA))
    L.append(tensor("FZ_EXPSCALE", EXPSCALE))
    L.append(tensor("FZ_PMS", PMS))
    L.append(tensor("FZ_AMSENS", [a / 16777216 for a in AMSENS]))
    # msfa's LFO phase step: lforatio / 2^32 cycles per Hz of the table
    L.append(tensor("FZ_LFO_HZ", [round(s * 4437500000 / 4294967296, 6) for s in LFO_SOURCE]))
    for n in range(1, 7):
        k = 6 - n                                   # the table lists the sixth operator first
        L.append(tensor(f"FZ_OP{n}_IN", [(a[k] >> 4) & 3 for a in ALG]))
        L.append(tensor(f"FZ_OP{n}_OUT", [a[k] & 3 for a in ALG]))
        L.append(tensor(f"FZ_OP{n}_ADD", [(a[k] >> 2) & 1 for a in ALG]))
        L.append(tensor(f"FZ_OP{n}_FB", [1 if (a[k] & 0xc0) == 0xc0 else 0 for a in ALG]))
    L.append("")
    L.append("; ---- the patch: OP1..OP6 (DX-style voice parameters), then the voice")
    init = patch_params(INIT)
    for n in range(1, 7):
        for suffix, _, lo, hi in OP_FIELDS:
            L.append(param_line(f"op{n}_{suffix}", init[f"op{n}_{suffix}"], lo, hi))
    for name, _, lo, hi in VOICE_FIELDS:
        L.append(param_line(name, init[name], lo, hi, "st" if name == "transpose" else None))
    L.append("; ---- Felucca's macros")
    for name, lo, hi in MACROS:
        L.append(param_line(name, 0, lo, hi))
    L.append(param_line("gain", 0.5, 0, 1))
    L.append(MACRO_DEFS)
    L.append("""; ---- the note
(def held (gt gate 0.5))
(def tr (round transpose))
(def note (latch (clip (+ (round (+ 69 (* 12 (/ (log (/ (max pitch 1) 440)) ln2)))) tr) 0 127) trigger))
(def vel (latch (clip (round (* velocity 127)) 0 127) trigger))
(def pitch_t (* pitch (pow 2 (/ tr 12))))
(def alg (- (clip (round algorithm) 1 32) 1))
; msfa's detune step of this note, in octaves
(def note_oct (+ 3.031359 (/ note 12)))
(def detune_step (* (/ 0.0209 7) (exp (* -0.396 note_oct)) note_oct))
; the macros
(def meg (* (sign mod_env) (floor (/ (* (abs (round mod_env)) 40) 64))))
(def mrat (round mod_ratio))
(def vmod (round mod_vel))
(def mlvl (/ (* (clip (round (mod mod_level)) -64 63) 24) 256))
(def dtun_cents (/ (* (clip (mod detune_spread) 0 127) 12) 127))
(def fb_amt (gswitch (gt (mod feedback) 0) (pow 2 (- (clip (mod feedback) 0 7) 9)) 0))

; ---- LFO and pitch envelope
(def lfo (fz-lfo trigger (round (clip (mod lfo_speed) 0 99)) (round lfo_wave) (round lfo_sync)))
(def lfo_dly (fz-lfo-delay trigger (round lfo_delay)))
(def pm_oct (/ (* (floor (/ (* (round lfo_pmd) 165) 64)) (peek FZ_PMS (round lfo_pms)) lfo_dly (- lfo 0.5)) 32768))
(def am_frac (* (/ (floor (/ (* (round lfo_amd) 165) 64)) 256) lfo_dly (- 1 lfo)))
(def peg (fz-pitch-env trigger held (round peg_rate1) (round peg_rate2) (round peg_rate3) (round peg_rate4)
  (round peg_level1) (round peg_level2) (round peg_level3) (round peg_level4)))
(def pitch_mod (+ peg pm_oct))
""")
    # carriers and their detune spread order (OP1 first)
    for n in range(1, 7):
        L.append(f"(def car{n} (eq (peek FZ_OP{n}_OUT alg) 0))")
    L.append("(def ncar1 0)")
    for n in range(2, 7):
        L.append(f"(def ncar{n} (+ ncar{n - 1} car{n - 1}))")
    # fresh notes restart phases and feedback: a trigger while every carrier is silent
    L.append("(make-history fz_alive_h)")
    L.append("(def fresh (* (gt trigger 0.5) (lt (read-history fz_alive_h) 0.0001)))")
    L.append("")
    for n in range(1, 7):
        p = f"op{n}_"
        L.append(f"""; ---- OP{n}
(def {p}mod (- 1 car{n}))
(def {p}rs (round {p}rate_scale))
(def {p}rscale (floor (/ (* {p}rs (clip (- (floor (/ note 3)) 7) 0 31)) 8)))
(def {p}off (- note (round {p}break) 17))
(def {p}up (gte {p}off 0))
(def {p}group (gswitch {p}up (floor (/ (+ {p}off 1) 3)) (floor (/ (- 1 {p}off) 3))))
(def {p}depth (gswitch {p}up (round {p}rdepth) (round {p}ldepth)))
(def {p}curve (gswitch {p}up (round {p}rcurve) (round {p}lcurve)))
(def {p}lin (or (eq {p}curve 0) (eq {p}curve 3)))
(def {p}scl_mag (gswitch {p}lin (floor (/ (* {p}group {p}depth 329) 4096))
  (floor (/ (* (peek FZ_EXPSCALE (min {p}group 32)) {p}depth 329) 32768))))
(def {p}scl (gswitch (lt {p}curve 2) (- 0 {p}scl_mag) {p}scl_mag))
(def {p}kvs (clip (+ (round {p}vel) (* {p}mod vmod)) 0 7))
(def {p}velscl (* (floor (/ (+ (* {p}kvs (- (peek FZ_VELDATA (floor (/ vel 2))) 239)) 7) 8)) 16))
(def {p}outlevel (latch (max 0 (+ (* (min 127 (+ (fz-scaleout (round {p}out)) {p}scl)) 32) {p}velscl)) trigger))
(def {p}r1 (clip (- (round {p}rate1) (* {p}mod meg)) 0 99))
(def {p}r2 (clip (- (round {p}rate2) (* {p}mod meg)) 0 99))
(def {p}r3 (clip (- (round {p}rate3) (* {p}mod meg)) 0 99))
(def {p}r4 (clip (- (round {p}rate4) (* {p}mod meg)) 0 99))
(def {p}env (fz-env trigger held {p}r1 {p}r2 {p}r3 {p}r4 (round {p}level1) (round {p}level2)
  (round {p}level3) (round {p}level4) {p}outlevel (latch {p}rscale trigger)))
(def {p}ams (peek FZ_AMSENS (round {p}ams)))
(def {p}env_am (gswitch (gt {p}ams 0) (* {p}env (- 1 (* (exp (+ 12.2 (* 4.48 am_frac {p}ams))) inv_q24))) {p}env))
(def {p}gain_raw (pow 2 (- (clip (+ {p}env_am (* {p}mod mlvl)) 0 17) 14)))
; msfa skips an operator below kLevelThresh (1120 / 2^24)
(def {p}gain (* {p}gain_raw (gte {p}gain_raw 0.0000668)))
(def {p}fc (clip (+ (round {p}coarse) (* {p}mod (- 1 (round {p}fixed)) mrat)) 0 31))
(def {p}ratio_oct (+ (/ (log (max {p}fc 0.5)) ln2) (/ (log (+ 1 (/ (round {p}fine) 100))) ln2)
  (* detune_step (- (round {p}detune) 7))))
(def {p}spread (* car{n} (selector (+ ncar{n} 1) 0 1 -1 2 -2 3) (/ dtun_cents 1200)))
(def {p}fixed_hz (* (pow 10 (+ (% (round {p}coarse) 4) (/ (round {p}fine) 100)))
  (pow 2 (* (max 0 (- (round {p}detune) 7)) 0.000802))))
(def {p}hz (gswitch (round {p}fixed) {p}fixed_hz
  (* pitch_t (pow 2 (+ {p}ratio_oct pitch_mod {p}spread)))))
(def {p}in_bus (peek FZ_OP{n}_IN alg))
(def {p}out_bus (peek FZ_OP{n}_OUT alg))
(def {p}add (peek FZ_OP{n}_ADD alg))
""")
    # the buses, in the table's order: OP6 first
    L.append("; ---- the algorithm: operators in msfa's order (OP6 first) over two buses and the output")
    L.append("(def bus1_0 0)\n(def bus2_0 0)\n(def out_0 0)")
    for step, n in enumerate(range(6, 0, -1), 1):
        p = f"op{n}_"
        a = step - 1
        L.append(f"""(def {p}y (fz-op {p}hz fresh {p}gain
  (+ (* (eq {p}in_bus 1) bus1_{a}) (* (eq {p}in_bus 2) bus2_{a})) (* (peek FZ_OP{n}_FB alg) fb_amt)))
(def bus1_{step} (gswitch (eq {p}out_bus 1) (+ (* {p}add bus1_{a}) {p}y) bus1_{a}))
(def bus2_{step} (gswitch (eq {p}out_bus 2) (+ (* {p}add bus2_{a}) {p}y) bus2_{a}))
(def out_{step} (gswitch (eq {p}out_bus 0) (+ (* {p}add out_{a}) {p}y) out_{a}))""")
    alive = " ".join(f"(* car{n} op{n}_gain)" for n in range(1, 7))
    L.append(f"(write-history fz_alive_h (+ {alive}))")
    L.append("""
; ---- out: msfa's output with Felucca's ~7 Hz DC blocker (FM puts sidebands at 0 Hz)
(make-history fz_dc_h)
(def dc_prev (gswitch fresh 0 (read-history fz_dc_h)))
(def dc_in (- out_6 dc_prev))
(write-history fz_dc_h (+ dc_prev (* dc_in (/ sr_scale 1024))))
; 0.1 sits one full carrier with the other factory synths.
(def voice_out (* dc_in (clip (mod gain) 0 1) 0.1))
(out voice_out 1 @name left)
(out voice_out 2 @name right)
(use-defmacro release-window)
(use-defmacro voice-amp)
(out (voice-amp (release-window gate 50) voice_out voice_out) 3 @name amp @amp true)
""")
    return "\n".join(L)


def alg_routes():
    """per algorithm: its routing as text ('6>5>4>3  2>1  fb 6  out 1 3') and its carriers (OP1..OP6, 0/1)"""
    texts, carriers = [], []
    for a in ALG:
        bus, edges, fb = {0: [], 1: [], 2: []}, [], None
        for k in range(6):
            n, f = 6 - k, a[k]
            ib, ob, add = (f >> 4) & 3, f & 3, (f >> 2) & 1
            if (f & 0xc0) == 0xc0:
                fb = n
            if ib and bus[ib]:
                edges.append(("+".join(map(str, sorted(bus[ib], reverse=True))), n))
            bus[ob] = bus[ob] + [n] if add else [n]
        chains = []                                 # join single-input links into chains: 6>5>4>3
        for src, dst in edges:
            for c in chains:
                if c[-1] == src:
                    c.append(str(dst))
                    break
            else:
                chains.append([src, str(dst)])
        texts.append("  ".join(">".join(c) for c in chains) + f"   fb {fb}   out {' '.join(map(str, sorted(bus[0])))}")
        carriers.append([1 if n in bus[0] else 0 for n in range(1, 7)])
    return texts, carriers


UI = r""";; Felucca FM6: the FM6 engine of the Felucca FM-1 firmware (msfa / Dexed core).
;; GENERATED by tools/felucca-fm6/generate.py: edit the generator, not this file.
;; Knob panels: the algorithm, Felucca's EDIT macros, the LFO and pitch envelope;
;; the six operator boxes select their page in the display.
(def fz-alg-text '(@ALG_TEXT@))
(def fz-carriers '(@CARRIERS@))
(def fz-alg () (max 1 (min 32 (round (eseq.effects.mnm-surface/mnm-value "algorithm")))))
(def fz-carrier? (n) (= (nth (nth fz-carriers (- (fz-alg) 1)) (- n 1)) 1))
(def fz-num (name title width decimals)
  (fz-num-ink name title width decimals :black))
(def fz-num-ink (name title width decimals base-ink)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (ink (if (eseq.effects.custom-ui-runtime/custom-ui-param-mod-highlighted? p) :white base-ink)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "fz-mod-" name)
      (v-stack :width width :height 1.3 :gap 0
        (label title :height 0.65 :font-size 8.5 :v-align :center :color ink :bg :transparent)
        (number-picker :debug-name (str "fz-num-" name) :width width :height 0.65
          :noui true :font-size 10 :decimals decimals :step (pow 10 (- 0 decimals))
          :value (eseq.effects.mnm-surface/mnm-bind name)
          :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
          :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
          :text-align :left :text-color ink :edit-color ink :cursor-color ink
          :plock-style :underline
          :plock-active (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active-prop p)
          :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback p))))))
(def fz-row (pairs)
  (h-stack :gap 0.3
    (each pairs |pair| (fz-num (nth pair 0) (nth pair 1) 5.6 0))))

;; A DX-style envelope: from L4 through L1, L2, L3 (held while the key is
;; down), back to L4. Faster rates draw shorter segments. Levels 0..99.
(defwidget fz-env-view
  :width 35.2 :height 2.0 :state (r1 r2 r3 r4 l1 l2 l3 l4)
  :shader
  (let ((w1 (+ 0.08 (* 0.92 (- 1 (/ r1 99)))))
        (w2 (+ 0.08 (* 0.92 (- 1 (/ r2 99)))))
        (w3 (+ 0.08 (* 0.92 (- 1 (/ r3 99)))))
        (w4 (+ 0.08 (* 0.92 (- 1 (/ r4 99)))))
        (k (/ (* 2 aspect 0.94) (+ w1 w2 w3 w4 0.6)))
        (x0 (* aspect -0.94))
        (x1 (+ x0 (* k w1)))
        (x2 (+ x1 (* k w2)))
        (x3 (+ x2 (* k w3)))
        (x4 (+ x3 (* k 0.6)))
        (x5 (+ x4 (* k w4)))
        (y1 (- 0.75 (* 1.5 (/ l1 99))))
        (y2 (- 0.75 (* 1.5 (/ l2 99))))
        (y3 (- 0.75 (* 1.5 (/ l3 99))))
        (y4 (- 0.75 (* 1.5 (/ l4 99))))
        (d (min (sdf/line x0 y4 x1 y1) (min (sdf/line x1 y1 x2 y2)
             (min (sdf/line x2 y2 x3 y3) (min (sdf/line x3 y3 x4 y3) (sdf/line x4 y3 x5 y4)))))))
    (sdf/layer
      (sdf/region :env (sdf/rect width height) :yellow)
      (sdf/paint (sdf/translate x4 0 (sdf/rect 0.006 height)) (rgba 0 0 0 0.25))
      (sdf/paint (- d 0.03) :black))))
(def fz-env (prefix)
  (fz-env-view :debug-name (str "fz-env-" prefix)
    :r1 (eseq.effects.mnm-surface/mnm-bind (str prefix "rate1")) :r2 (eseq.effects.mnm-surface/mnm-bind (str prefix "rate2"))
    :r3 (eseq.effects.mnm-surface/mnm-bind (str prefix "rate3")) :r4 (eseq.effects.mnm-surface/mnm-bind (str prefix "rate4"))
    :l1 (eseq.effects.mnm-surface/mnm-bind (str prefix "level1")) :l2 (eseq.effects.mnm-surface/mnm-bind (str prefix "level2"))
    :l3 (eseq.effects.mnm-surface/mnm-bind (str prefix "level3")) :l4 (eseq.effects.mnm-surface/mnm-bind (str prefix "level4"))))

(def fz-op-page (n)
  (let ((op (str "op" n "_")))
    (v-stack :gap 0.15
      (fz-env op)
      (fz-row (list (list (str op "rate1") "Rate 1") (list (str op "rate2") "Rate 2") (list (str op "rate3") "Rate 3") (list (str op "rate4") "Rate 4") (list (str op "out") "Out") (list (str op "vel") "Vel")))
      (fz-row (list (list (str op "level1") "Level 1") (list (str op "level2") "Level 2") (list (str op "level3") "Level 3") (list (str op "level4") "Level 4") (list (str op "coarse") "Coarse") (list (str op "fine") "Fine")))
      (fz-row (list (list (str op "detune") "Detune") (list (str op "fixed") "Fixed") (list (str op "ams") "AMS") (list (str op "rate_scale") "Rate Scl") (list (str op "break") "Break") (list (str op "lcurve") "L Curve")))
      (h-stack :gap 0.3
        (fz-row (list (list (str op "ldepth") "L Depth") (list (str op "rdepth") "R Depth") (list (str op "rcurve") "R Curve")))
        (eseq.effects.mnm-surface/mnm-caption "Curves: 0 -lin 1 -exp 2 +exp 3 +lin")))))

(def fz-algo-page ()
  (v-stack :gap 0.3
    (subtree :key (str "fz-alg-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" (fz-alg))
      (v-stack :gap 0.2
        (label (str "Algorithm " (fz-alg)) :width 35.2 :height 0.9 :font-size 12 :v-align :center
          :color :black :bg :transparent)
        (label (nth fz-alg-text (- (fz-alg) 1)) :width 35.2 :height 0.9 :font-size 12 :v-align :center
          :color :black :bg :transparent)))
    (eseq.effects.mnm-surface/mnm-caption "a>b: a modulates b. Feedback acts on the fb operator; out lists the carriers.")
    (h-stack :gap 0.3
      (fz-num "algorithm" "Algorithm" 8.4 0) (fz-num "feedback" "Feedback" 8.4 2)
      (fz-num "transpose" "Transpose st" 8.4 0) (fz-num "gain" "Level" 8.4 2))))
(def fz-macro-page ()
  (v-stack :gap 0.2
    (eseq.effects.mnm-surface/mnm-caption "Felucca's EDIT macros, on top of the patch; all act on modulators only,")
    (eseq.effects.mnm-surface/mnm-caption "except Detune, which spreads the carriers apart (beats).")
    (eseq.effects.mnm-surface/mnm-caption "Mod Lvl: +-36 dB of brightness. Mod EG: + slower. Mod Ratio: + coarse.")
    (h-stack :gap 0.3
      (fz-num "mod_level" "Mod Lvl" 6.8 0) (fz-num "mod_ratio" "Mod Ratio" 6.8 0)
      (fz-num "mod_env" "Mod EG" 6.8 0) (fz-num "mod_vel" "Mod Vel" 6.8 0)
      (fz-num "detune_spread" "Detune" 6.8 0))))
(def fz-lfo-page ()
  (v-stack :gap 0.25
    (eseq.effects.mnm-surface/mnm-caption "One LFO per voice. Delay fades it in after each note;")
    (eseq.effects.mnm-surface/mnm-caption "PMD x PMS bends the pitch, AMD moves each operator by its AMS.")
    (h-stack :gap 0.3
      (eseq.effects.mnm-surface/mnm-option "lfo_wave" '("Triangle" "Saw down" "Saw up" "Square" "Sine" "Sample & hold"))
      (eseq.effects.mnm-surface/mnm-option "lfo_sync" '("Free running" "Key sync")))
    (h-stack :gap 0.3
      (fz-num "lfo_speed" "Speed" 6.8 0) (fz-num "lfo_delay" "Delay" 6.8 0)
      (fz-num "lfo_pmd" "PMD" 6.8 0) (fz-num "lfo_amd" "AMD" 6.8 0)
      (fz-num "lfo_pms" "PMS" 6.8 0))))
(def fz-peg-page ()
  (v-stack :gap 0.15
    (fz-env "peg_")
    (eseq.effects.mnm-surface/mnm-caption "Pitch envelope: level 50 is the note, 0 / 99 about 4 octaves down / up.")
    (fz-row (list (list "peg_rate1" "Rate 1") (list "peg_rate2" "Rate 2") (list "peg_rate3" "Rate 3") (list "peg_rate4" "Rate 4") (list "transpose" "Transpose")))
    (fz-row (list (list "peg_level1" "Level 1") (list "peg_level2" "Level 2") (list "peg_level3" "Level 3") (list "peg_level4" "Level 4")))))

;; An operator box: its number, carrier or modulator in this algorithm, and
;; its output level. Clicking it opens its page.
(def fz-op-box (n)
  (box :debug-name (str "fz-op-" n) :width 7.15 :height 3.13 :padding 0.2
    :background-color (if (= (eseq.effects.mnm-surface/mnm-section) n) :instrument-panel-bg :instrument-group-bg)
    :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback n)
    (v-stack :gap 0.15
      (subtree :key (str "fz-op-role-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" n "-" (fz-alg))
        (box :width 6.7 :height 0.7 :background-color (eseq.effects.mnm-surface/mnm-accent)
          (label (str "OP" n (if (fz-carrier? n) "  carrier" "  mod")) :width 6.7 :height 0.7 :font-size 8
            :h-align :center :v-align :center :color :black :bg :transparent)))
      (fz-num-ink (str "op" n "_out") "Out" 6.7 0 (eseq.effects.mnm-surface/mnm-accent)))))

(defsynth-ui
  (h-stack :height 9.8 :gap 0.3 :align :start
    (v-stack :gap 0.2
      (eseq.effects.mnm-surface/mnm-panel 0 "ALGORITHM" 21.4
        (h-stack :gap 0.2
          (eseq.effects.mnm-surface/mnm-knob 0 "algorithm" "Algorithm" 6.8 0 :linear)
          (eseq.effects.mnm-surface/mnm-knob 0 "feedback" "Feedback" 6.8 2 :linear)
          (eseq.effects.mnm-surface/mnm-knob 0 "gain" "Level" 6.8 2 :linear)))
      (eseq.effects.mnm-surface/mnm-panel 7 "MACRO" 21.4
        (h-stack :gap 0.2
          (eseq.effects.mnm-surface/mnm-knob 7 "mod_level" "Mod Lvl" 6.8 0 :linear)
          (eseq.effects.mnm-surface/mnm-knob 7 "mod_env" "Mod EG" 6.8 0 :linear)
          (eseq.effects.mnm-surface/mnm-knob 7 "detune_spread" "Detune" 6.8 0 :linear))))
    (eseq.effects.mnm-surface/mnm-display "FELUCCA FM6"
      '("ALGORITHM" "OP1" "OP2" "OP3" "OP4" "OP5" "OP6" "MACRO" "LFO" "PITCH EG")
      (list fz-algo-page
        (lambda () (fz-op-page 1)) (lambda () (fz-op-page 2)) (lambda () (fz-op-page 3))
        (lambda () (fz-op-page 4)) (lambda () (fz-op-page 5)) (lambda () (fz-op-page 6))
        fz-macro-page fz-lfo-page fz-peg-page))
    (v-stack :gap 0.2
      (h-stack :gap 0.3 (fz-op-box 1) (fz-op-box 2))
      (h-stack :gap 0.3 (fz-op-box 3) (fz-op-box 4))
      (h-stack :gap 0.3 (fz-op-box 5) (fz-op-box 6)))
    (v-stack :gap 0.2
      (eseq.effects.mnm-surface/mnm-panel 8 "LFO" 14.6
        (h-stack :gap 0.2
          (eseq.effects.mnm-surface/mnm-knob 8 "lfo_speed" "Speed" 7 0 :linear)
          (eseq.effects.mnm-surface/mnm-knob 8 "lfo_pmd" "Pitch Depth" 7 0 :linear)))
      (eseq.effects.mnm-surface/mnm-panel 9 "PITCH EG" 14.6
        (h-stack :gap 0.2
          (eseq.effects.mnm-surface/mnm-knob 9 "peg_level1" "Level 1" 7 0 :linear)
          (eseq.effects.mnm-surface/mnm-knob 9 "peg_rate1" "Rate 1" 7 0 :linear))))))
"""


def gen_ui():
    texts, carriers = alg_routes()
    return (UI.replace("@ALG_TEXT@", " ".join(f'"{t}"' for t in texts))
            .replace("@CARRIERS@", " ".join("(" + " ".join(map(str, c)) + ")" for c in carriers)))


def gen_presets():
    presets = [{"id": "Init Voice", "name": "Init Voice", "base_note_offset": 0.0, "params": patch_params(INIT)}]
    for name, v, macros in PATCHES:
        presets.append({"id": name, "name": name, "base_note_offset": 0.0, "params": patch_params(v, macros)})
    return {"version": 1, "engine_name": "Synths/Felucca FM6/",
            "source_file": "instruments/Synths/Felucca FM6/dsp.lisp", "presets": presets}


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "dsp.lisp").write_text(gen_dsp())
    (OUT / "ui.lisp").write_text(gen_ui())
    PRESETS.write_text(json.dumps(gen_presets(), indent=2) + "\n")
    print(f"wrote {OUT}/dsp.lisp, ui.lisp and {PRESETS}")


if __name__ == "__main__":
    main()
