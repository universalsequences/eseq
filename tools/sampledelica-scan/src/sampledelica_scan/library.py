"""Walk a music library the same way the musicplayer crate does:
albums = directories that directly contain audio files; cover art = a
cover/folder/front image, else the first image in the folder.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Optional

AUDIO_EXTS = {".mp3", ".flac", ".wav", ".ogg", ".m4a", ".aiff", ".aif"}
IMAGE_EXTS = {".png", ".jpg", ".jpeg", ".webp"}
COVER_STEMS = ("cover", "folder", "front")


@dataclass
class Track:
    path: Path
    title: str
    album: str
    album_path: Path
    cover_path: Optional[Path]


def _find_cover(folder: Path) -> Optional[Path]:
    images = [p for p in folder.iterdir() if p.suffix.lower() in IMAGE_EXTS]
    for stem in COVER_STEMS:
        for img in images:
            if img.stem.lower() == stem:
                return img
    return sorted(images)[0] if images else None


def scan_library(root: Path) -> list[Track]:
    """Return all tracks found under `root`, grouped by their parent folder."""
    root = Path(root).expanduser().resolve()
    tracks: list[Track] = []
    for folder in sorted({p.parent for p in root.rglob("*") if p.suffix.lower() in AUDIO_EXTS}):
        cover = _find_cover(folder)
        for f in sorted(folder.iterdir()):
            if f.suffix.lower() in AUDIO_EXTS:
                tracks.append(
                    Track(
                        path=f,
                        title=f.stem,
                        album=folder.name,
                        album_path=folder,
                        cover_path=cover,
                    )
                )
    return tracks
