#!/usr/bin/env python3
"""Identify formant poles and signed bandpass weights from complex responses.

The rational fit is an identification step, not the shipping DSP. Reject an
ill-conditioned or unstable fit rather than silently replacing its poles.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re

import numpy as np

from analyze_reference import load


def identify(response, frequencies):
    sample_rate = 48000
    # Dimensionless frequency improves conditioning of the polynomial solve.
    scale = np.tan(np.pi * 833.782 / sample_rate)
    s = 1j * np.tan(np.pi * frequencies / sample_rate) / scale
    powers = np.array([s**degree for degree in range(7)]).T
    matrix = np.column_stack((-response[:, None] * powers[:, 1:], powers))
    weight = 1 / (1 + abs(s)**6)
    matrix *= weight[:, None]
    target = response * weight
    coefficients = np.linalg.lstsq(
        np.concatenate((matrix.real, matrix.imag)),
        np.concatenate((target.real, target.imag)), rcond=1e-12)[0]
    denominator = np.r_[1, coefficients[:6]]
    poles = np.polynomial.polynomial.polyroots(denominator)
    if np.any(poles.real >= 0):
        raise ValueError(f"Unstable identified poles: {poles}")
    poles = np.array(sorted((pole for pole in poles if pole.imag > 0), key=abs))
    if len(poles) != 3:
        raise ValueError(f"Expected three conjugate pole pairs, got {poles}")
    centers = np.arctan(abs(poles) * scale) * sample_rate / np.pi
    q = abs(poles) / (-2 * poles.real)
    normalized = (1j * np.tan(np.pi * frequencies[:, None] / sample_rate)
                  / np.tan(np.pi * centers[None, :] / sample_rate))
    bandpasses = normalized / (normalized**2 + normalized / q + 1)
    gains = np.linalg.lstsq(
        np.concatenate((bandpasses.real, bandpasses.imag)),
        np.concatenate((response.real, response.imag)), rcond=None)[0]
    predicted = bandpasses @ gains
    relative_error = float(np.linalg.norm(predicted - response) / np.linalg.norm(response))
    if not np.isfinite(relative_error) or relative_error > 0.001:
        raise ValueError(f"Inadequate formant fit: relative complex error {relative_error}")
    return dict(frequencies=centers.tolist(), q=q.tolist(), gain=gains.tolist(),
                relative_complex_error=relative_error)


def analyze(folder, prefix):
    ledger_path = folder / f"{prefix}.json"
    ledger = json.loads(ledger_path.read_text())
    frequencies = np.arange(1, 257) * 32.68445
    time = np.arange(48000) / 48000
    basis = np.exp(-2j * np.pi * frequencies[:, None] * time) * np.hanning(48000)

    def harmonics(path):
        return basis @ load(path)[48000:96000, 0].astype(float)

    source_path = folder / f"{prefix} calibration-saw.wav"
    source = harmonics(source_path)
    report = dict(sample_rate=48000, frequencies_hz=frequencies.tolist(),
                  source_sha256=hashlib.sha256(source_path.read_bytes()).hexdigest(),
                  ledger_sha256=hashlib.sha256(ledger_path.read_bytes()).hexdigest(), cases=[])
    for case in ledger["cases"]:
        name = case["name"]
        if name.startswith("calibration-"):
            continue
        # Live may append an export disambiguator even to a unique track name.
        # Accept that spelling only when it resolves to exactly one capture.
        spelling = re.compile(re.escape(f"{prefix} {name}") + r"(?:-\d+)?\.wav")
        candidates = [path for path in folder.glob("*.wav") if spelling.fullmatch(path.name)]
        if len(candidates) != 1:
            raise ValueError(f"Expected one capture for {name}, found {candidates}")
        path = candidates[0]
        response = harmonics(path) / source
        fit = identify(response, frequencies)
        parameters = case["parameters"]
        report["cases"].append(dict(
            name=name, capture_file=path.name, sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
            native_cutoff=float(parameters["SignalChain1/FilterCutoffFrequency"]),
            native_vowel=float(parameters["SignalChain1/FilterQFactor"]),
            native_type=int(parameters["SignalChain1/FilterType"]),
            response_real=response.real.tolist(), response_imag=response.imag.tolist(), **fit))
        print(f"{name}: {fit['frequencies']}, error {fit['relative_complex_error']:.7f}", flush=True)
    return report


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("folder", type=Path)
    parser.add_argument("--prefix", default="formants")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    result = analyze(args.folder, args.prefix)
    args.out.write_text(json.dumps(result, indent=2) + "\n")
