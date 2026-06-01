"""Cross-album region search over scanner sidecars.

Builds a flat in-memory index of every detected region (across all albums) that
carries a CLAP embedding, and ranks regions by:
  - similarity to a TEXT query ("dusty tape-saturated snare")
  - similarity to ANOTHER region ("more like this one")

This is the Python mirror of what the Rust `region_embeddings` similarity query
will do in the DAW. Keep the shapes here aligned with the eventual schema.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

import numpy as np


@dataclass
class Region:
    uid: str                  # f"{album}//{title}//{slice_id}"
    album: str
    title: str
    slice_id: str
    kind: str                 # chord | stab | break
    label: Optional[str]
    quality: Optional[str]
    pc_set: int
    start_ms: int
    end_ms: int
    source_path: str          # the ORIGINAL audio file (annotation target)
    wav_path: str             # rendered audition clip (may go away later)
    vec: np.ndarray           # CLAP embedding (normalized)


def _norm(v: np.ndarray) -> np.ndarray:
    return v / (np.linalg.norm(v) + 1e-9)


def load_index(out_dir: Path) -> list[Region]:
    """Load every embedded region from sidecars under out_dir."""
    regions: list[Region] = []
    for sc_path in sorted(Path(out_dir).rglob("sidecar.json")):
        doc = json.loads(sc_path.read_text())
        base = sc_path.parent
        for s in doc["slices"]:
            vec = s.get("clap_vec")
            if not vec:
                continue
            regions.append(
                Region(
                    uid=f"{doc['album']}//{doc['title']}//{s['slice_id']}",
                    album=doc["album"],
                    title=doc["title"],
                    slice_id=s["slice_id"],
                    kind=s["kind"],
                    label=s.get("label"),
                    quality=s.get("quality"),
                    pc_set=s.get("pc_set", 0),
                    start_ms=s["start_ms"],
                    end_ms=s["end_ms"],
                    source_path=doc["source_path"],
                    wav_path=str(base / s["wav_path"]),
                    vec=_norm(np.asarray(vec, dtype=np.float32)),
                )
            )
    return regions


def _matrix(regions: list[Region]) -> np.ndarray:
    return np.vstack([r.vec for r in regions]) if regions else np.zeros((0, 512))


def rank_by_vector(regions: list[Region], q: np.ndarray,
                   kinds: Optional[set[str]] = None,
                   cross_album_only_from: Optional[str] = None,
                   top_k: int = 20) -> list[tuple[Region, float]]:
    """Rank regions by cosine to a query vector q (already any scale)."""
    if not regions:
        return []
    q = _norm(np.asarray(q, dtype=np.float32))
    sims = _matrix(regions) @ q
    order = np.argsort(-sims)
    out: list[tuple[Region, float]] = []
    for i in order:
        r = regions[i]
        if kinds and r.kind not in kinds:
            continue
        if cross_album_only_from and r.album == cross_album_only_from:
            continue
        out.append((r, float(sims[i])))
        if len(out) >= top_k:
            break
    return out


def similar_to_region(regions: list[Region], uid: str,
                      cross_album_only: bool = True,
                      same_kind: bool = True,
                      kinds: Optional[set[str]] = None,
                      top_k: int = 20) -> list[tuple[Region, float]]:
    """'More like this' — nearest neighbors of one region across the library."""
    src = next((r for r in regions if r.uid == uid), None)
    if src is None:
        return []
    kind_filter = kinds if kinds is not None else ({src.kind} if same_kind else None)
    ranked = rank_by_vector(
        regions, src.vec, kinds=kind_filter,
        cross_album_only_from=src.album if cross_album_only else None,
        top_k=top_k + 1,
    )
    return [(r, s) for (r, s) in ranked if r.uid != uid][:top_k]
