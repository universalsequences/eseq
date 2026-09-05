#!/usr/bin/env python3
"""Generate the Triton wavetable bank from the AKWF-FREE single-cycle
waveform collection (Adventure Kid Waveforms, CC0 / public domain).

  https://github.com/KristofferKarlAxelEkstrand/AKWF-FREE

Output: bank.json, wave-major (`wave * 512 + sample`), shape [512, 512]
(32 sets x 16 waves), matching the format used by
content/instruments/core/wavetable/waves/bank.json.

Usage:
    python3 extract_bank.py [path-to-AKWF-FREE-clone]

Default clone path matches the scratchpad location used to build this bank;
pass an explicit path if you have AKWF-FREE checked out elsewhere. Only this
script, bank.json and README.md are checked into the repo -- the AKWF-FREE
clone (WAV files) is NOT part of the repo tree.

Every wave is:
  1. loaded as-is from its native-length single-cycle WAV (AKWF waves are
     600 samples / 16-bit mono / 44.1kHz, verified against the actual files),
  2. resampled to 512 samples via FFT (rfft -> truncate/zero-pad bins ->
     irfft), which preserves exact periodicity since these are single-cycle
     waveforms,
  3. DC-removed,
  4. peak-normalized to 0.85,
  5. rounded to 5 decimals.

The wave selections below were hand-curated by scanning each AKWF category
folder, computing per-wave spectral centroid (and pulse duty for the square
set), and picking 16 spectrally-distinct waves per set spread across the
centroid range (dark -> bright), baked in as explicit filenames so
regeneration is fully reproducible even if the upstream repo changes.
"""
import glob
import json
import os
import sys
import wave

import numpy as np

DEFAULT_AKWF_ROOT = (
    "/private/tmp/claude-501/-Users-alecresende-code-learning-anthropic-eseq/"
    "491610b9-2082-4724-a7fb-2d98ffa6ed8b/scratchpad/akwf/AKWF"
)

N = 512
WAVES_PER_SET = 16
PEAK = 0.85

SOURCE = (
    "AKWF-FREE (Adventure Kid Waveforms), public domain / CC0 1.0 Universal -- "
    "https://github.com/KristofferKarlAxelEkstrand/AKWF-FREE"
)

# ---------------------------------------------------------------------------
# Curated set definitions: (name, [folder/filename, ...]) x 32, 16 waves each.
# Order within each set is dark -> bright (by spectral centroid), except
# "Square PW" which is ordered wide -> narrow pulse (by duty cycle symmetry).
# ---------------------------------------------------------------------------

SETS = [
    ("Square PW", [
        "AKWF_bw_squ/AKWF_squ_0003.wav",
        "AKWF_bw_squ/AKWF_squ_0017.wav",
        "AKWF_bw_squ/AKWF_squ_0013.wav",
        "AKWF_bw_squ/AKWF_squ_0057.wav",
        "AKWF_bw_squ/AKWF_squ_0060.wav",
        "AKWF_bw_squ/AKWF_squ_0011.wav",
        "AKWF_bw_squ/AKWF_squ_0030.wav",
        "AKWF_bw_squ/AKWF_squ_0032.wav",
        "AKWF_bw_squ/AKWF_squ_0035.wav",
        "AKWF_bw_squ/AKWF_squ_0067.wav",
        "AKWF_bw_squ/AKWF_squ_0038.wav",
        "AKWF_bw_squ/AKWF_squ_0042.wav",
        "AKWF_bw_squ/AKWF_squ_0062.wav",
        "AKWF_bw_squ/AKWF_squ_0048.wav",
        "AKWF_bw_squ/AKWF_squ_0070.wav",
        "AKWF_bw_squ/AKWF_squ_0051.wav",
    ]),
    ("Bright Saw", [
        "AKWF_bw_saw/AKWF_saw_0002.wav",
        "AKWF_bw_saw/AKWF_saw_0008.wav",
        "AKWF_bw_saw/AKWF_saw_0031.wav",
        "AKWF_bw_saw/AKWF_saw_0027.wav",
        "AKWF_bw_saw/AKWF_saw_0023.wav",
        "AKWF_bw_saw/AKWF_saw_0019.wav",
        "AKWF_bw_sawbright/AKWF_bsaw_0001.wav",
        "AKWF_bw_sawbright/AKWF_bsaw_0008.wav",
        "AKWF_bw_saw/AKWF_saw_0014.wav",
        "AKWF_bw_saw/AKWF_saw_0005.wav",
        "AKWF_bw_saw/AKWF_saw_0035.wav",
        "AKWF_bw_saw/AKWF_saw_0036.wav",
        "AKWF_bw_saw/AKWF_saw_0039.wav",
        "AKWF_bw_saw/AKWF_saw_0043.wav",
        "AKWF_bw_saw/AKWF_saw_0007.wav",
        "AKWF_bw_saw/AKWF_saw_0016.wav",
    ]),
    ("Round Saw", [
        "AKWF_bw_sawrounded/AKWF_R_sym_saw_01.wav",
        "AKWF_bw_sawrounded/AKWF_R_asym_saw_03.wav",
        "AKWF_bw_sawrounded/AKWF_R_asym_saw_06.wav",
        "AKWF_bw_sawrounded/AKWF_R_sym_saw_11.wav",
        "AKWF_bw_sawrounded/AKWF_R_sym_saw_14.wav",
        "AKWF_bw_sawrounded/AKWF_R_sym_saw_17.wav",
        "AKWF_bw_sawrounded/AKWF_R_sym_saw_20.wav",
        "AKWF_bw_sawrounded/AKWF_R_sym_saw_23.wav",
        "AKWF_bw_sawrounded/AKWF_R_sym_saw_26.wav",
        "AKWF_bw_sawgap/AKWF_gapsaw_0004.wav",
        "AKWF_bw_sawgap/AKWF_gapsaw_0010.wav",
        "AKWF_bw_sawgap/AKWF_gapsaw_0042.wav",
        "AKWF_bw_sawgap/AKWF_gapsaw_0018.wav",
        "AKWF_bw_sawgap/AKWF_gapsaw_0034.wav",
        "AKWF_bw_sawgap/AKWF_gapsaw_0032.wav",
        "AKWF_bw_sawgap/AKWF_gapsaw_0031.wav",
    ]),
    ("Sub Sine", [
        "AKWF_bw_perfectwaves/AKWF_sin.wav",
        "AKWF_bw_sin/AKWF_sin_0012.wav",
        "AKWF_bw_sin/AKWF_sin_0007.wav",
        "AKWF_bw_sin/AKWF_sin_0010.wav",
        "AKWF_bw_sin/AKWF_sin_0002.wav",
        "AKWF_bw_sin/AKWF_sin_0005.wav",
        "AKWF_bw_sin/AKWF_sin_0003.wav",
        "AKWF_bw_sin/AKWF_sin_0001.wav",
        "AKWF_sinharm/AKWF_sinharm_0015.wav",
        "AKWF_sinharm/AKWF_sinharm_0007.wav",
        "AKWF_sinharm/AKWF_sinharm_0009.wav",
        "AKWF_sinharm/AKWF_sinharm_0005.wav",
        "AKWF_sinharm/AKWF_sinharm_0016.wav",
        "AKWF_bw_perfectwaves/AKWF_tri.wav",
        "AKWF_bw_sin/AKWF_sin_0004.wav",
        "AKWF_bw_perfectwaves/AKWF_saw.wav",
    ]),
    ("Tri Wave", [
        "AKWF_bw_tri/AKWF_tri_0006.wav",
        "AKWF_bw_tri/AKWF_tri_0005.wav",
        "AKWF_bw_tri/AKWF_tri_0002.wav",
        "AKWF_bw_tri/AKWF_tri_0004.wav",
        "AKWF_bw_tri/AKWF_tri_0019.wav",
        "AKWF_bw_tri/AKWF_tri_0003.wav",
        "AKWF_bw_tri/AKWF_tri_0017.wav",
        "AKWF_bw_tri/AKWF_tri_0001.wav",
        "AKWF_bw_tri/AKWF_tri_0016.wav",
        "AKWF_bw_tri/AKWF_tri_0022.wav",
        "AKWF_bw_tri/AKWF_tri_0015.wav",
        "AKWF_bw_tri/AKWF_tri_0013.wav",
        "AKWF_bw_tri/AKWF_tri_0012.wav",
        "AKWF_bw_tri/AKWF_tri_0023.wav",
        "AKWF_bw_tri/AKWF_tri_0024.wav",
        "AKWF_bw_tri/AKWF_tri_0008.wav",
    ]),
    ("E.Organ Lo", [
        "AKWF_eorgan/AKWF_eorgan_0070.wav",
        "AKWF_eorgan/AKWF_eorgan_0011.wav",
        "AKWF_eorgan/AKWF_eorgan_0112.wav",
        "AKWF_eorgan/AKWF_eorgan_0004.wav",
        "AKWF_eorgan/AKWF_eorgan_0001.wav",
        "AKWF_eorgan/AKWF_eorgan_0152.wav",
        "AKWF_eorgan/AKWF_eorgan_0067.wav",
        "AKWF_eorgan/AKWF_eorgan_0092.wav",
        "AKWF_eorgan/AKWF_eorgan_0113.wav",
        "AKWF_eorgan/AKWF_eorgan_0002.wav",
        "AKWF_eorgan/AKWF_eorgan_0064.wav",
        "AKWF_eorgan/AKWF_eorgan_0108.wav",
        "AKWF_eorgan/AKWF_eorgan_0052.wav",
        "AKWF_eorgan/AKWF_eorgan_0069.wav",
        "AKWF_eorgan/AKWF_eorgan_0023.wav",
        "AKWF_eorgan/AKWF_eorgan_0090.wav",
    ]),
    ("E.Organ Hi", [
        "AKWF_eorgan/AKWF_eorgan_0153.wav",
        "AKWF_eorgan/AKWF_eorgan_0140.wav",
        "AKWF_eorgan/AKWF_eorgan_0073.wav",
        "AKWF_eorgan/AKWF_eorgan_0126.wav",
        "AKWF_eorgan/AKWF_eorgan_0134.wav",
        "AKWF_eorgan/AKWF_eorgan_0114.wav",
        "AKWF_eorgan/AKWF_eorgan_0066.wav",
        "AKWF_eorgan/AKWF_eorgan_0003.wav",
        "AKWF_eorgan/AKWF_eorgan_0035.wav",
        "AKWF_eorgan/AKWF_eorgan_0082.wav",
        "AKWF_eorgan/AKWF_eorgan_0147.wav",
        "AKWF_eorgan/AKWF_eorgan_0098.wav",
        "AKWF_eorgan/AKWF_eorgan_0053.wav",
        "AKWF_eorgan/AKWF_eorgan_0117.wav",
        "AKWF_eorgan/AKWF_eorgan_0080.wav",
        "AKWF_eorgan/AKWF_eorgan_0121.wav",
    ]),
    ("Organ Reed", [
        "AKWF_eorgan/AKWF_eorgan_0097.wav",
        "AKWF_eorgan/AKWF_eorgan_0059.wav",
        "AKWF_eorgan/AKWF_eorgan_0122.wav",
        "AKWF_eorgan/AKWF_eorgan_0127.wav",
        "AKWF_eorgan/AKWF_eorgan_0154.wav",
        "AKWF_eorgan/AKWF_eorgan_0034.wav",
        "AKWF_eorgan/AKWF_eorgan_0083.wav",
        "AKWF_eorgan/AKWF_eorgan_0116.wav",
        "AKWF_eorgan/AKWF_eorgan_0048.wav",
        "AKWF_eorgan/AKWF_eorgan_0077.wav",
        "AKWF_eorgan/AKWF_eorgan_0031.wav",
        "AKWF_eorgan/AKWF_eorgan_0049.wav",
        "AKWF_eorgan/AKWF_eorgan_0039.wav",
        "AKWF_eorgan/AKWF_eorgan_0041.wav",
        "AKWF_eorgan/AKWF_eorgan_0046.wav",
        "AKWF_eorgan/AKWF_eorgan_0042.wav",
    ]),
    ("E.Piano Soft", [
        "AKWF_piano/AKWF_piano_0010.wav",
        "AKWF_epiano/AKWF_epiano_0019.wav",
        "AKWF_epiano/AKWF_epiano_0011.wav",
        "AKWF_epiano/AKWF_epiano_0067.wav",
        "AKWF_epiano/AKWF_epiano_0056.wav",
        "AKWF_epiano/AKWF_epiano_0042.wav",
        "AKWF_epiano/AKWF_epiano_0065.wav",
        "AKWF_epiano/AKWF_epiano_0045.wav",
        "AKWF_epiano/AKWF_epiano_0053.wav",
        "AKWF_epiano/AKWF_epiano_0044.wav",
        "AKWF_epiano/AKWF_epiano_0040.wav",
        "AKWF_epiano/AKWF_epiano_0027.wav",
        "AKWF_epiano/AKWF_epiano_0036.wav",
        "AKWF_epiano/AKWF_epiano_0015.wav",
        "AKWF_epiano/AKWF_epiano_0018.wav",
        "AKWF_epiano/AKWF_epiano_0001.wav",
    ]),
    ("E.Piano Bell", [
        "AKWF_epiano/AKWF_epiano_0072.wav",
        "AKWF_epiano/AKWF_epiano_0047.wav",
        "AKWF_epiano/AKWF_epiano_0031.wav",
        "AKWF_piano/AKWF_piano_0016.wav",
        "AKWF_epiano/AKWF_epiano_0033.wav",
        "AKWF_epiano/AKWF_epiano_0032.wav",
        "AKWF_epiano/AKWF_epiano_0046.wav",
        "AKWF_piano/AKWF_piano_0023.wav",
        "AKWF_piano/AKWF_piano_0013.wav",
        "AKWF_epiano/AKWF_epiano_0017.wav",
        "AKWF_piano/AKWF_piano_0004.wav",
        "AKWF_piano/AKWF_piano_0011.wav",
        "AKWF_piano/AKWF_piano_0014.wav",
        "AKWF_piano/AKWF_piano_0001.wav",
        "AKWF_epiano/AKWF_epiano_0013.wav",
        "AKWF_piano/AKWF_piano_0022.wav",
    ]),
    ("Clavinet", [
        "AKWF_clavinet/AKWF_clavinet_0018.wav",
        "AKWF_clavinet/AKWF_clavinet_0017.wav",
        "AKWF_clavinet/AKWF_clavinet_0010.wav",
        "AKWF_clavinet/AKWF_clavinet_0013.wav",
        "AKWF_clavinet/AKWF_clavinet_0029.wav",
        "AKWF_clavinet/AKWF_clavinet_0032.wav",
        "AKWF_clavinet/AKWF_clavinet_0033.wav",
        "AKWF_clavinet/AKWF_clavinet_0016.wav",
        "AKWF_clavinet/AKWF_clavinet_0021.wav",
        "AKWF_clavinet/AKWF_clavinet_0008.wav",
        "AKWF_clavinet/AKWF_clavinet_0015.wav",
        "AKWF_clavinet/AKWF_clavinet_0007.wav",
        "AKWF_clavinet/AKWF_clavinet_0005.wav",
        "AKWF_clavinet/AKWF_clavinet_0004.wav",
        "AKWF_clavinet/AKWF_clavinet_0003.wav",
        "AKWF_clavinet/AKWF_clavinet_0001.wav",
    ]),
    ("Elec Bass", [
        "AKWF_ebass/AKWF_ebass_0026.wav",
        "AKWF_ebass/AKWF_ebass_0017.wav",
        "AKWF_ebass/AKWF_ebass_0019.wav",
        "AKWF_ebass/AKWF_ebass_0024.wav",
        "AKWF_ebass/AKWF_ebass_0062.wav",
        "AKWF_ebass/AKWF_ebass_0057.wav",
        "AKWF_ebass/AKWF_ebass_0031.wav",
        "AKWF_ebass/AKWF_ebass_0061.wav",
        "AKWF_ebass/AKWF_ebass_0035.wav",
        "AKWF_ebass/AKWF_ebass_0040.wav",
        "AKWF_ebass/AKWF_ebass_0011.wav",
        "AKWF_ebass/AKWF_ebass_0028.wav",
        "AKWF_ebass/AKWF_ebass_0012.wav",
        "AKWF_ebass/AKWF_ebass_0047.wav",
        "AKWF_ebass/AKWF_ebass_0051.wav",
        "AKWF_ebass/AKWF_ebass_0023.wav",
    ]),
    ("Dist Bass", [
        "AKWF_dbass/AKWF_dbass_0032.wav",
        "AKWF_dbass/AKWF_dbass_0029.wav",
        "AKWF_dbass/AKWF_dbass_0038.wav",
        "AKWF_dbass/AKWF_dbass_0007.wav",
        "AKWF_dbass/AKWF_dbass_0050.wav",
        "AKWF_dbass/AKWF_dbass_0061.wav",
        "AKWF_dbass/AKWF_dbass_0021.wav",
        "AKWF_dbass/AKWF_dbass_0048.wav",
        "AKWF_dbass/AKWF_dbass_0052.wav",
        "AKWF_dbass/AKWF_dbass_0025.wav",
        "AKWF_dbass/AKWF_dbass_0017.wav",
        "AKWF_dbass/AKWF_dbass_0059.wav",
        "AKWF_dbass/AKWF_dbass_0044.wav",
        "AKWF_dbass/AKWF_dbass_0011.wav",
        "AKWF_dbass/AKWF_dbass_0041.wav",
        "AKWF_dbass/AKWF_dbass_0010.wav",
    ]),
    ("DX Bell", [
        "AKWF_fmsynth/AKWF_fmsynth_0070.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0021.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0090.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0079.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0067.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0091.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0114.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0101.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0073.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0042.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0085.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0117.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0068.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0100.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0012.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0046.wav",
    ]),
    ("FM Metal", [
        "AKWF_fmsynth/AKWF_fmsynth_0043.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0084.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0083.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0112.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0075.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0097.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0110.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0098.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0063.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0094.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0096.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0113.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0069.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0050.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0001.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0086.wav",
    ]),
    ("FM Pluck", [
        "AKWF_fmsynth/AKWF_fmsynth_0081.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0066.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0029.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0092.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0011.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0032.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0016.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0058.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0057.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0060.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0040.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0008.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0035.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0055.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0003.wav",
        "AKWF_fmsynth/AKWF_fmsynth_0026.wav",
    ]),
    ("Chip Digi", [
        "AKWF_oscchip/AKWF_oscchip_0026.wav",
        "AKWF_oscchip/AKWF_oscchip_0153.wav",
        "AKWF_oscchip/AKWF_oscchip_0032.wav",
        "AKWF_oscchip/AKWF_oscchip_0121.wav",
        "AKWF_oscchip/AKWF_oscchip_0078.wav",
        "AKWF_oscchip/AKWF_oscchip_0052.wav",
        "AKWF_oscchip/AKWF_oscchip_0142.wav",
        "AKWF_oscchip/AKWF_oscchip_0143.wav",
        "AKWF_oscchip/AKWF_oscchip_0119.wav",
        "AKWF_oscchip/AKWF_oscchip_0103.wav",
        "AKWF_oscchip/AKWF_oscchip_0021.wav",
        "AKWF_oscchip/AKWF_oscchip_0012.wav",
        "AKWF_oscchip/AKWF_oscchip_0105.wav",
        "AKWF_oscchip/AKWF_oscchip_0073.wav",
        "AKWF_oscchip/AKWF_oscchip_0064.wav",
        "AKWF_oscchip/AKWF_oscchip_0085.wav",
    ]),
    ("VGame Lead", [
        "AKWF_vgame/AKWF_vgame_0096.wav",
        "AKWF_vgame/AKWF_vgame_0092.wav",
        "AKWF_vgame/AKWF_vgame_0080.wav",
        "AKWF_vgame/AKWF_vgame_0069.wav",
        "AKWF_vgame/AKWF_vgame_0022.wav",
        "AKWF_vgame/AKWF_vgame_0050.wav",
        "AKWF_vgame/AKWF_vgame_0037.wav",
        "AKWF_vgame/AKWF_vgame_0046.wav",
        "AKWF_vgame/AKWF_vgame_0013.wav",
        "AKWF_vgame/AKWF_vgame_0030.wav",
        "AKWF_vgame/AKWF_vgame_0109.wav",
        "AKWF_vgame/AKWF_vgame_0127.wav",
        "AKWF_vgame/AKWF_vgame_0128.wav",
        "AKWF_vgame/AKWF_vgame_0100.wav",
        "AKWF_vgame/AKWF_vgame_0105.wav",
        "AKWF_vgame/AKWF_vgame_0119.wav",
    ]),
    ("Voice Ooh", [
        "AKWF_hvoice/AKWF_hvoice_0083.wav",
        "AKWF_hvoice/AKWF_hvoice_0025.wav",
        "AKWF_hvoice/AKWF_hvoice_0076.wav",
        "AKWF_hvoice/AKWF_hvoice_0008.wav",
        "AKWF_hvoice/AKWF_hvoice_0026.wav",
        "AKWF_hvoice/AKWF_hvoice_0003.wav",
        "AKWF_hvoice/AKWF_hvoice_0002.wav",
        "AKWF_hvoice/AKWF_hvoice_0078.wav",
        "AKWF_hvoice/AKWF_hvoice_0082.wav",
        "AKWF_hvoice/AKWF_hvoice_0027.wav",
        "AKWF_hvoice/AKWF_hvoice_0077.wav",
        "AKWF_hvoice/AKWF_hvoice_0006.wav",
        "AKWF_hvoice/AKWF_hvoice_0092.wav",
        "AKWF_hvoice/AKWF_hvoice_0079.wav",
        "AKWF_hvoice/AKWF_hvoice_0039.wav",
        "AKWF_hvoice/AKWF_hvoice_0014.wav",
    ]),
    ("Voice Choir", [
        "AKWF_hvoice/AKWF_hvoice_0086.wav",
        "AKWF_hvoice/AKWF_hvoice_0064.wav",
        "AKWF_hvoice/AKWF_hvoice_0035.wav",
        "AKWF_hvoice/AKWF_hvoice_0065.wav",
        "AKWF_hvoice/AKWF_hvoice_0042.wav",
        "AKWF_hvoice/AKWF_hvoice_0028.wav",
        "AKWF_hvoice/AKWF_hvoice_0071.wav",
        "AKWF_hvoice/AKWF_hvoice_0052.wav",
        "AKWF_hvoice/AKWF_hvoice_0103.wav",
        "AKWF_hvoice/AKWF_hvoice_0098.wav",
        "AKWF_hvoice/AKWF_hvoice_0044.wav",
        "AKWF_hvoice/AKWF_hvoice_0020.wav",
        "AKWF_hvoice/AKWF_hvoice_0059.wav",
        "AKWF_hvoice/AKWF_hvoice_0050.wav",
        "AKWF_hvoice/AKWF_hvoice_0053.wav",
        "AKWF_hvoice/AKWF_hvoice_0061.wav",
    ]),
    ("Str.Machine", [
        "AKWF_stringbox/AKWF_cheeze_0002.wav",
        "AKWF_violin/AKWF_violin_0014.wav",
        "AKWF_stringbox/AKWF_cheeze_0001.wav",
        "AKWF_violin/AKWF_violin_0011.wav",
        "AKWF_violin/AKWF_violin_0012.wav",
        "AKWF_violin/AKWF_violin_0009.wav",
        "AKWF_stringbox/AKWF_cheeze_0004.wav",
        "AKWF_violin/AKWF_violin_0010.wav",
        "AKWF_violin/AKWF_violin_0013.wav",
        "AKWF_cello/AKWF_cello_0019.wav",
        "AKWF_cello/AKWF_cello_0003.wav",
        "AKWF_cello/AKWF_cello_0006.wav",
        "AKWF_cello/AKWF_cello_0017.wav",
        "AKWF_violin/AKWF_violin_0005.wav",
        "AKWF_cello/AKWF_cello_0012.wav",
        "AKWF_stringbox/AKWF_cheeze_0006.wav",
    ]),
    ("Str.Bowed", [
        "AKWF_cello/AKWF_cello_0007.wav",
        "AKWF_violin/AKWF_violin_0008.wav",
        "AKWF_cello/AKWF_cello_0011.wav",
        "AKWF_cello/AKWF_cello_0005.wav",
        "AKWF_violin/AKWF_violin_0003.wav",
        "AKWF_cello/AKWF_cello_0002.wav",
        "AKWF_cello/AKWF_cello_0016.wav",
        "AKWF_violin/AKWF_violin_0007.wav",
        "AKWF_violin/AKWF_violin_0002.wav",
        "AKWF_cello/AKWF_cello_0015.wav",
        "AKWF_cello/AKWF_cello_0010.wav",
        "AKWF_cello/AKWF_cello_0008.wav",
        "AKWF_cello/AKWF_cello_0013.wav",
        "AKWF_cello/AKWF_cello_0014.wav",
        "AKWF_cello/AKWF_cello_0004.wav",
        "AKWF_cello/AKWF_cello_0001.wav",
    ]),
    ("Grit Dist", [
        "AKWF_distorted/AKWF_distorted_0020.wav",
        "AKWF_distorted/AKWF_distorted_0021.wav",
        "AKWF_distorted/AKWF_distorted_0017.wav",
        "AKWF_distorted/AKWF_distorted_0040.wav",
        "AKWF_distorted/AKWF_distorted_0005.wav",
        "AKWF_distorted/AKWF_distorted_0002.wav",
        "AKWF_distorted/AKWF_distorted_0033.wav",
        "AKWF_distorted/AKWF_distorted_0006.wav",
        "AKWF_distorted/AKWF_distorted_0034.wav",
        "AKWF_distorted/AKWF_distorted_0030.wav",
        "AKWF_distorted/AKWF_distorted_0027.wav",
        "AKWF_distorted/AKWF_distorted_0012.wav",
        "AKWF_distorted/AKWF_distorted_0015.wav",
        "AKWF_distorted/AKWF_distorted_0031.wav",
        "AKWF_distorted/AKWF_distorted_0043.wav",
        "AKWF_distorted/AKWF_distorted_0045.wav",
    ]),
    ("Nylon Gtr", [
        "AKWF_aguitar/AKWF_aguitar_0007.wav",
        "AKWF_aguitar/AKWF_aguitar_0018.wav",
        "AKWF_aguitar/AKWF_aguitar_0005.wav",
        "AKWF_aguitar/AKWF_aguitar_0002.wav",
        "AKWF_aguitar/AKWF_aguitar_0008.wav",
        "AKWF_aguitar/AKWF_aguitar_0009.wav",
        "AKWF_aguitar/AKWF_aguitar_0010.wav",
        "AKWF_aguitar/AKWF_aguitar_0014.wav",
        "AKWF_aguitar/AKWF_aguitar_0013.wav",
        "AKWF_aguitar/AKWF_aguitar_0027.wav",
        "AKWF_eguitar/AKWF_eguitar_0003.wav",
        "AKWF_eguitar/AKWF_eguitar_0006.wav",
        "AKWF_eguitar/AKWF_eguitar_0019.wav",
        "AKWF_eguitar/AKWF_eguitar_0013.wav",
        "AKWF_eguitar/AKWF_eguitar_0004.wav",
        "AKWF_eguitar/AKWF_eguitar_0014.wav",
    ]),
    ("Reed Winds", [
        "AKWF_oboe/AKWF_oboe_0009.wav",
        "AKWF_flute/AKWF_flute_0008.wav",
        "AKWF_clarinett/AKWF_clarinett_0011.wav",
        "AKWF_altosax/AKWF_altosax_0002.wav",
        "AKWF_clarinett/AKWF_clarinett_0018.wav",
        "AKWF_oboe/AKWF_oboe_0004.wav",
        "AKWF_flute/AKWF_flute_0015.wav",
        "AKWF_altosax/AKWF_altosax_0024.wav",
        "AKWF_altosax/AKWF_altosax_0013.wav",
        "AKWF_clarinett/AKWF_clarinett_0005.wav",
        "AKWF_clarinett/AKWF_clarinett_0025.wav",
        "AKWF_altosax/AKWF_altosax_0009.wav",
        "AKWF_clarinett/AKWF_clarinett_0001.wav",
        "AKWF_oboe/AKWF_oboe_0002.wav",
        "AKWF_altosax/AKWF_altosax_0007.wav",
        "AKWF_altosax/AKWF_altosax_0018.wav",
    ]),
    ("Overtone", [
        "AKWF_overtone/AKWF_overtone_0001.wav",
        "AKWF_overtone/AKWF_overtone_0043.wav",
        "AKWF_overtone/AKWF_overtone_0020.wav",
        "AKWF_overtone/AKWF_overtone_0006.wav",
        "AKWF_overtone/AKWF_overtone_0004.wav",
        "AKWF_overtone/AKWF_overtone_0031.wav",
        "AKWF_overtone/AKWF_overtone_0025.wav",
        "AKWF_overtone/AKWF_overtone_0012.wav",
        "AKWF_overtone/AKWF_overtone_0024.wav",
        "AKWF_overtone/AKWF_overtone_0041.wav",
        "AKWF_overtone/AKWF_overtone_0003.wav",
        "AKWF_overtone/AKWF_overtone_0026.wav",
        "AKWF_overtone/AKWF_overtone_0035.wav",
        "AKWF_overtone/AKWF_overtone_0021.wav",
        "AKWF_overtone/AKWF_overtone_0038.wav",
        "AKWF_overtone/AKWF_overtone_0034.wav",
    ]),
    ("Odd Harm", [
        "AKWF_symetric/AKWF_symetric_0009.wav",
        "AKWF_symetric/AKWF_symetric_0002.wav",
        "AKWF_symetric/AKWF_symetric_0016.wav",
        "AKWF_symetric/AKWF_symetric_0014.wav",
        "AKWF_symetric/AKWF_symetric_0013.wav",
        "AKWF_symetric/AKWF_symetric_0015.wav",
        "AKWF_symetric/AKWF_symetric_0001.wav",
        "AKWF_symetric/AKWF_symetric_0012.wav",
        "AKWF_symetric/AKWF_symetric_0011.wav",
        "AKWF_symetric/AKWF_symetric_0003.wav",
        "AKWF_symetric/AKWF_symetric_0005.wav",
        "AKWF_symetric/AKWF_symetric_0017.wav",
        "AKWF_symetric/AKWF_symetric_0010.wav",
        "AKWF_symetric/AKWF_symetric_0007.wav",
        "AKWF_symetric/AKWF_symetric_0006.wav",
        "AKWF_symetric/AKWF_symetric_0004.wav",
    ]),
    ("Theremin", [
        "AKWF_theremin/AKWF_theremin_0004.wav",
        "AKWF_theremin/AKWF_tannerin_0001.wav",
        "AKWF_theremin/AKWF_tannerin_0003.wav",
        "AKWF_theremin/AKWF_theremin_0018.wav",
        "AKWF_theremin/AKWF_tannerin_0004.wav",
        "AKWF_theremin/AKWF_theremin_0015.wav",
        "AKWF_theremin/AKWF_tannerin_0002.wav",
        "AKWF_theremin/AKWF_theremin_0014.wav",
        "AKWF_theremin/AKWF_theremin_0011.wav",
        "AKWF_theremin/AKWF_theremin_0008.wav",
        "AKWF_theremin/AKWF_theremin_0013.wav",
        "AKWF_theremin/AKWF_theremin_0021.wav",
        "AKWF_theremin/AKWF_theremin_0009.wav",
        "AKWF_theremin/AKWF_theremin_0002.wav",
        "AKWF_theremin/AKWF_theremin_0012.wav",
        "AKWF_theremin/AKWF_theremin_0022.wav",
    ]),
    ("Hand Drawn", [
        "AKWF_hdrawn/AKWF_hdrawn_0005.wav",
        "AKWF_hdrawn/AKWF_hdrawn_0018.wav",
        "AKWF_hdrawn/AKWF_hdrawn_0015.wav",
        "AKWF_hdrawn/AKWF_hdrawn_0045.wav",
        "AKWF_hdrawn/AKWF_hdrawn_0022.wav",
        "AKWF_hdrawn/AKWF_hdrawn_0009.wav",
        "AKWF_hdrawn/AKWF_hdrawn_0040.wav",
        "AKWF_hdrawn/AKWF_hdrawn_0044.wav",
        "AKWF_hdrawn/AKWF_hdrawn_0025.wav",
        "AKWF_hdrawn/AKWF_hdrawn_0035.wav",
        "AKWF_hdrawn/AKWF_hdrawn_0049.wav",
        "AKWF_hdrawn/AKWF_hdrawn_0036.wav",
        "AKWF_hdrawn/AKWF_hdrawn_0029.wav",
        "AKWF_hdrawn/AKWF_hdrawn_0021.wav",
        "AKWF_hdrawn/AKWF_hdrawn_0003.wav",
        "AKWF_hdrawn/AKWF_hdrawn_0026.wav",
    ]),
    ("Bit Crush", [
        "AKWF_bitreduced/AKWF_tri8bit.wav",
        "AKWF_bitreduced/AKWF_bitreduced_0012.wav",
        "AKWF_bitreduced/AKWF_tri6bit.wav",
        "AKWF_bitreduced/AKWF_bitreduced_0009.wav",
        "AKWF_bitreduced/AKWF_bitreduced_0006.wav",
        "AKWF_bitreduced/AKWF_bitreduced_0005.wav",
        "AKWF_bitreduced/AKWF_squ2bit.wav",
        "AKWF_bitreduced/AKWF_bitreduced_0036.wav",
        "AKWF_bitreduced/AKWF_saw5bit.wav",
        "AKWF_bitreduced/AKWF_saw7bit.wav",
        "AKWF_bitreduced/AKWF_bitreduced_0002.wav",
        "AKWF_bitreduced/AKWF_bitreduced_0028.wav",
        "AKWF_bitreduced/AKWF_bitreduced_0026.wav",
        "AKWF_bitreduced/AKWF_bitreduced_0017.wav",
        "AKWF_bitreduced/AKWF_bitreduced_0034.wav",
        "AKWF_bitreduced/AKWF_bitreduced_0032.wav",
    ]),
    ("C64 Chip", [
        "AKWF_c604/AKWF_c604_0013.wav",
        "AKWF_c604/AKWF_c604_0026.wav",
        "AKWF_c604/AKWF_c604_0015.wav",
        "AKWF_c604/AKWF_c604_0009.wav",
        "AKWF_c604/AKWF_c604_0008.wav",
        "AKWF_c604/AKWF_c604_0006.wav",
        "AKWF_c604/AKWF_c604_0024.wav",
        "AKWF_c604/AKWF_c604_0028.wav",
        "AKWF_c604/AKWF_c604_0021.wav",
        "AKWF_c604/AKWF_c604_0012.wav",
        "AKWF_c604/AKWF_c604_0007.wav",
        "AKWF_c604/AKWF_c604_0019.wav",
        "AKWF_c604/AKWF_c604_0003.wav",
        "AKWF_c604/AKWF_c604_0032.wav",
        "AKWF_c604/AKWF_c604_0016.wav",
        "AKWF_c604/AKWF_c604_0030.wav",
    ]),
    ("Granular", [
        "AKWF_granular/AKWF_granular_0022.wav",
        "AKWF_granular/AKWF_granular_0026.wav",
        "AKWF_granular/AKWF_granular_0009.wav",
        "AKWF_granular/AKWF_granular_0014.wav",
        "AKWF_granular/AKWF_granular_0043.wav",
        "AKWF_granular/AKWF_granular_0011.wav",
        "AKWF_granular/AKWF_granular_0027.wav",
        "AKWF_granular/AKWF_granular_0012.wav",
        "AKWF_granular/AKWF_granular_0030.wav",
        "AKWF_granular/AKWF_granular_0018.wav",
        "AKWF_granular/AKWF_granular_0036.wav",
        "AKWF_granular/AKWF_granular_0003.wav",
        "AKWF_granular/AKWF_granular_0034.wav",
        "AKWF_granular/AKWF_granular_0016.wav",
        "AKWF_granular/AKWF_granular_0017.wav",
        "AKWF_granular/AKWF_granular_0008.wav",
    ]),
]

assert len(SETS) == 32, f"expected 32 sets, got {len(SETS)}"
for name, files in SETS:
    assert len(files) == WAVES_PER_SET, f"{name}: expected {WAVES_PER_SET} waves, got {len(files)}"


def load_wav(path):
    """Load a mono 16-bit AKWF single-cycle WAV as float64 samples in [-1, 1]."""
    w = wave.open(path, "rb")
    n = w.getnframes()
    ch = w.getnchannels()
    sw = w.getsampwidth()
    raw = w.readframes(n)
    if sw != 2:
        raise ValueError(f"{path}: expected 16-bit samples, got {sw * 8}-bit")
    data = np.frombuffer(raw, dtype="<i2").astype(np.float64) / 32768.0
    if ch > 1:
        data = data.reshape(-1, ch)[:, 0]
    return data


def resample_fft(x, n_out):
    """FFT-based resample of a single-cycle waveform: rfft -> truncate/zero-pad
    bins -> irfft. Exact for periodic single-cycle waves (no windowing needed
    since the cycle is already periodic end-to-end)."""
    n_in = len(x)
    spec = np.fft.rfft(x)
    n_bins_in = len(spec)
    n_bins_out = n_out // 2 + 1
    if n_bins_out <= n_bins_in:
        out_spec = spec[:n_bins_out].copy()
        # zero the Nyquist bin if we truncated exactly onto it to avoid
        # an asymmetric real/imag split
        if n_out % 2 == 0:
            out_spec[-1] = out_spec[-1].real
    else:
        out_spec = np.zeros(n_bins_out, dtype=complex)
        out_spec[:n_bins_in] = spec
    out = np.fft.irfft(out_spec, n=n_out)
    # irfft normalizes by n_out already relative to its own bin count, but the
    # forward transform was normalized relative to n_in -- rescale so the
    # resampled cycle has the same amplitude as the source.
    out *= n_out / n_in
    return out


def normalize(x):
    x = x - np.mean(x)
    peak = np.max(np.abs(x))
    if peak < 1e-12:
        peak = 1.0
    return x * (PEAK / peak)


def process_wave(root, rel_path):
    x = load_wav(os.path.join(root, rel_path))
    x = resample_fft(x, N)
    x = normalize(x)
    return [round(float(v), 5) for v in x]


def main():
    akwf_root = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_AKWF_ROOT
    if not os.path.isdir(akwf_root):
        raise SystemExit(
            f"AKWF-FREE root not found: {akwf_root}\n"
            "Clone it with:\n"
            "  git clone --depth 1 "
            "https://github.com/KristofferKarlAxelEkstrand/AKWF-FREE.git <dest>\n"
            "then pass <dest>/AKWF as the argument to this script."
        )

    data = []
    set_names = []
    for name, files in SETS:
        set_names.append(name)
        for rel_path in files:
            wave_data = process_wave(akwf_root, rel_path)
            assert len(wave_data) == N, rel_path
            data.extend(wave_data)

    out = {
        "shape": [N, len(SETS) * WAVES_PER_SET],
        "kind": "wavetable-bank",
        "layout": "wave-major: index = wave * 512 + sample",
        "source": SOURCE,
        "sets": set_names,
        "waves_per_set": WAVES_PER_SET,
        "data": data,
    }

    here = os.path.dirname(os.path.abspath(__file__))
    out_path = os.path.join(here, "bank.json")
    with open(out_path, "w") as f:
        json.dump(out, f)

    verify(out_path)
    print(f"wrote {out_path}: {len(SETS)} sets x {WAVES_PER_SET} waves, {len(data)} floats")


def verify(bank_path):
    """Self-check: shape/lengths, peak normalization, DC removal."""
    with open(bank_path) as f:
        bank = json.load(f)

    assert bank["shape"] == [N, 32 * WAVES_PER_SET], bank["shape"]
    assert bank["kind"] == "wavetable-bank", bank["kind"]
    assert len(bank["sets"]) == 32, len(bank["sets"])
    assert bank["waves_per_set"] == WAVES_PER_SET
    assert len(bank["data"]) == N * 32 * WAVES_PER_SET, len(bank["data"])
    assert "AKWF-FREE" in bank["source"] and "public domain" in bank["source"].lower()

    data = np.array(bank["data"], dtype=np.float64)
    n_waves = 32 * WAVES_PER_SET
    waves = data.reshape(n_waves, N)
    for i, wv in enumerate(waves):
        peak = np.max(np.abs(wv))
        assert abs(peak - PEAK) < 0.001, f"wave {i}: peak {peak} != {PEAK}"
        dc = np.mean(wv)
        assert abs(dc) < 0.01, f"wave {i}: DC offset {dc}"

    # spot-check: the Square PW set (index 0) should show clear duty-cycle
    # variation via zero-crossing counts and should look squarish (few
    # transitions per cycle, sign roughly bimodal).
    squ_set = waves[0:WAVES_PER_SET]
    for wv in squ_set:
        signs = np.sign(wv)
        crossings = int(np.sum(np.abs(np.diff(signs)) > 0))
        assert 1 <= crossings <= 6, f"Square PW wave has {crossings} zero crossings, expected ~2"

    print(f"verify OK: {n_waves} waves, peak={PEAK}+/-0.001, |DC|<0.01, "
          f"Square PW zero-crossings sane")


if __name__ == "__main__":
    main()
