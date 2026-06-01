"""CLAP embeddings (text<->audio) for cross-album similarity.

Gracefully no-ops if the `clap` extra isn't installed, so the core pipeline
always runs:  uv sync --extra clap

In the annotation model we embed a region *in memory* straight from the
original mix (no rendered file needed). CLAP expects 48 kHz mono float32.
"""

from __future__ import annotations

from functools import lru_cache
from typing import Optional

import numpy as np

CLAP_SR = 48000


@lru_cache(maxsize=1)
def _model():
    try:
        import laion_clap
    except Exception:
        return None
    model = laion_clap.CLAP_Module(enable_fusion=False)
    model.load_ckpt()  # downloads default 630k-audioset checkpoint on first use
    return model


def available() -> bool:
    return _model() is not None


def _to_clap(y: np.ndarray, sr: int) -> np.ndarray:
    """mono, 48 kHz, float32, shape (1, n) as CLAP wants."""
    if y.ndim > 1:
        y = y.mean(axis=0)
    if sr != CLAP_SR:
        import librosa

        y = librosa.resample(y, orig_sr=sr, target_sr=CLAP_SR)
    return np.ascontiguousarray(y[np.newaxis, :], dtype=np.float32)


def embed_region(y: np.ndarray, sr: int) -> Optional[list[float]]:
    """Embed an in-memory audio region (the original mix slice)."""
    m = _model()
    if m is None:
        return None
    x = _to_clap(y, sr)
    vec = m.get_audio_embedding_from_data(x=x, use_tensor=False)[0]
    return np.asarray(vec, dtype=np.float32).tolist()


def embed_audio(wav_path: str) -> Optional[list[float]]:
    """Embed a file on disk (kept for convenience / audition tooling)."""
    m = _model()
    if m is None:
        return None
    vec = m.get_audio_embedding_from_filelist([wav_path], use_tensor=False)[0]
    return np.asarray(vec, dtype=np.float32).tolist()


def embed_text(text: str) -> Optional[np.ndarray]:
    m = _model()
    if m is None:
        return None
    return np.asarray(m.get_text_embedding([text, ""], use_tensor=False)[0], dtype=np.float32)


def cosine(a: np.ndarray, b: np.ndarray) -> float:
    return float(a @ b / (np.linalg.norm(a) * np.linalg.norm(b) + 1e-9))
