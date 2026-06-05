"""Candidate segmentation: find breaks (drum-dominant regions) and stabs
(isolated harmonic onsets / "lone chords"). All heuristic — this is the part
you'll tune while auditioning.
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np


@dataclass
class Segment:
    kind: str          # "stab" | "break"
    start_s: float
    end_s: float
    is_tonal: bool


def _frame_rms(y: np.ndarray, sr: int, hop: int = 512) -> tuple[np.ndarray, np.ndarray]:
    import librosa

    rms = librosa.feature.rms(y=y, hop_length=hop)[0]
    times = librosa.frames_to_time(np.arange(len(rms)), sr=sr, hop_length=hop)
    return rms, times


def beat_grid(mix_mono: np.ndarray, sr: int) -> tuple[float, np.ndarray]:
    import librosa

    tempo, beats = librosa.beat.beat_track(y=mix_mono, sr=sr, units="time")
    return float(np.atleast_1d(tempo)[0]), np.asarray(beats)


def detect_breaks(drum_mono: np.ndarray, harm_mono: np.ndarray, sr: int,
                  beats: np.ndarray, min_bars: int = 1) -> list[Segment]:
    """Beats where drums dominate and harmony is quiet, grouped into regions."""
    drms, dt = _frame_rms(drum_mono, sr)
    hrms, ht = _frame_rms(harm_mono, sr)
    if len(beats) < 5:
        return []

    def energy(rms, times, a, b):
        m = (times >= a) & (times < b)
        return float(rms[m].mean()) if m.any() else 0.0

    drummy = []
    for a, b in zip(beats[:-1], beats[1:]):
        de, he = energy(drms, dt, a, b), energy(hrms, ht, a, b)
        drummy.append(de > 0.02 and de > 2.5 * (he + 1e-6))

    segs: list[Segment] = []
    i = 0
    while i < len(drummy):
        if drummy[i]:
            j = i
            while j < len(drummy) and drummy[j]:
                j += 1
            if (j - i) >= min_bars * 4:  # ~4 beats/bar
                segs.append(Segment("break", float(beats[i]), float(beats[j]), is_tonal=False))
            i = j
        else:
            i += 1
    return segs


def detect_stabs(harm_mono: np.ndarray, sr: int, max_dur: float = 1.5,
                 min_dur: float = 0.12, top_k: int = 24) -> list[Segment]:
    """Onsets in the harmonic stem whose energy attacks then decays = stabs."""
    import librosa

    onsets = librosa.onset.onset_detect(
        y=harm_mono, sr=sr, units="time", backtrack=True
    )
    if len(onsets) == 0:
        return []

    rms, times = _frame_rms(harm_mono, sr)
    dur = len(harm_mono) / sr
    bounds = list(onsets) + [dur]

    cands: list[tuple[float, Segment]] = []
    for t, nxt in zip(bounds[:-1], bounds[1:]):
        end = min(t + max_dur, nxt)
        if end - t < min_dur:
            continue
        m = (times >= t) & (times < end)
        if not m.any():
            continue
        env = rms[m]
        peak = float(env.max())
        tail = float(env[-max(1, len(env) // 4):].mean())
        if peak < 0.01:
            continue
        decayed = tail < 0.6 * peak
        if decayed:
            cands.append((peak, Segment("stab", float(t), float(end), is_tonal=True)))

    cands.sort(key=lambda x: x[0], reverse=True)
    return [s for _, s in cands[:top_k]]
