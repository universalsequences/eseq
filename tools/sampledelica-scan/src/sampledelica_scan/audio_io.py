"""Audio decode/resample helpers. Prefers libsndfile (soundfile) which reads
mp3/flac/wav/ogg directly on modern builds; falls back to librosa/audioread
(needs ffmpeg) for formats like m4a.
"""

from __future__ import annotations

from pathlib import Path

import numpy as np


def decode(path: Path, sr: int = 44100, mono: bool = False) -> tuple[np.ndarray, int]:
    """Return (samples, sr). Stereo shape is (channels, n); mono is (n,)."""
    import soundfile as sf

    try:
        data, file_sr = sf.read(str(path), always_2d=True, dtype="float32")
        y = data.T  # (channels, n)
    except Exception:
        import librosa

        y, file_sr = librosa.load(str(path), sr=None, mono=False)
        if y.ndim == 1:
            y = y[np.newaxis, :]

    if mono:
        y = y.mean(axis=0)

    if file_sr != sr:
        import librosa

        y = librosa.resample(y, orig_sr=file_sr, target_sr=sr)

    return y.astype(np.float32), sr


def to_mono(y: np.ndarray) -> np.ndarray:
    return y if y.ndim == 1 else y.mean(axis=0)
