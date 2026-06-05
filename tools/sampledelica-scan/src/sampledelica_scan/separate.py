"""Stem separation via Demucs (htdemucs). Runs as a subprocess for version
robustness, caches stems on disk keyed by input path mtime+size, and returns
paths to the 4 stems: drums, bass, other, vocals.

"other" is our harmonic stem (chords/synths/keys/guitars); "drums" drives
break detection.
"""

from __future__ import annotations

import hashlib
import subprocess
import sys
from pathlib import Path

STEMS = ("drums", "bass", "other", "vocals")


def _cache_key(audio: Path) -> str:
    st = audio.stat()
    h = hashlib.sha1(f"{audio.resolve()}:{st.st_size}:{int(st.st_mtime)}".encode())
    return h.hexdigest()[:16]


def separate(audio: Path, cache_dir: Path, model: str = "htdemucs",
             device: str | None = None) -> dict[str, Path]:
    """Return {stem_name: wav_path}. Uses cached stems when present."""
    audio = Path(audio)
    out_root = Path(cache_dir) / _cache_key(audio)
    stem_dir = out_root / model / audio.stem

    if all((stem_dir / f"{s}.wav").exists() for s in STEMS):
        return {s: stem_dir / f"{s}.wav" for s in STEMS}

    out_root.mkdir(parents=True, exist_ok=True)
    cmd = [sys.executable, "-m", "demucs", "-n", model, "-o", str(out_root)]
    if device:
        cmd += ["-d", device]
    cmd.append(str(audio))

    subprocess.run(cmd, check=True)

    found = {s: stem_dir / f"{s}.wav" for s in STEMS}
    missing = [s for s, p in found.items() if not p.exists()]
    if missing:
        raise RuntimeError(f"demucs did not produce stems {missing} in {stem_dir}")
    return found
