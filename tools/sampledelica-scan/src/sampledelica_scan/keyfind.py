"""Track-level key detection via Krumhansl-Schmuckler profile correlation over
an averaged chroma. Returns (key_pc, mode, confidence).
"""

from __future__ import annotations

from typing import Optional

import numpy as np

# Krumhansl-Kessler key profiles
_MAJOR = np.array([6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88])
_MINOR = np.array([6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17])


def detect_key(mix_mono: np.ndarray, sr: int) -> tuple[Optional[int], Optional[str], float]:
    import librosa

    chroma = librosa.feature.chroma_cqt(y=mix_mono, sr=sr)
    profile = chroma.mean(axis=1)
    if profile.sum() <= 0:
        return None, None, 0.0
    profile = profile / profile.sum()

    def best_corr(template):
        corrs = []
        for shift in range(12):
            rolled = np.roll(template, shift)
            corrs.append(np.corrcoef(profile, rolled)[0, 1])
        idx = int(np.argmax(corrs))
        return idx, float(corrs[idx])

    maj_pc, maj_c = best_corr(_MAJOR)
    min_pc, min_c = best_corr(_MINOR)

    if maj_c >= min_c:
        return maj_pc, "major", max(0.0, maj_c)
    return min_pc, "minor", max(0.0, min_c)
