"""Sidecar record shapes — deliberately mirror the planned SQLite Tier-2 tables.

If a field is awkward to fill or query while auditioning, fix it HERE before
writing any Rust. The experiment is the schema validation.

Tier-2 target tables (see project notes):
  sample_chords(root_pc, quality, bass_pc, pc_set, pc_set_norm, label, confidence, ...)
  sample_musical(key_pc, key_mode, key_conf, bpm, bpm_conf, is_tonal, chord_count, chroma)
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field, asdict
from typing import Optional

PITCH_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]


def pc_name(pc: Optional[int]) -> Optional[str]:
    return None if pc is None else PITCH_NAMES[pc % 12]


@dataclass
class SliceRecord:
    """One candidate sample extracted from a track."""

    # --- identity / provenance (feeds sample_origins on the Rust side) ---
    slice_id: str                 # stable id within this track, e.g. "stab-003"
    wav_path: str                 # rendered slice, relative to sidecar
    kind: str                     # "stab" | "chord" | "break"
    start_ms: int
    end_ms: int
    method: str                   # how it was detected, e.g. "onset+basicpitch"

    # --- harmonic content (feeds sample_chords) ---
    root_pc: Optional[int] = None
    quality: Optional[str] = None         # maj, min, dom7, maj7, min7, dim, ...
    bass_pc: Optional[int] = None
    pc_set: int = 0                       # 12-bit absolute pitch-class bitmask
    pc_set_norm: Optional[int] = None     # rotated so root=0 (transpose-invariant)
    label: Optional[str] = None           # display: "Cmaj7", "Am/E"
    chord_conf: float = 0.0
    note_count: int = 0                   # distinct sounding notes in the window

    # --- per-slice musical summary (feeds sample_musical) ---
    is_tonal: bool = True
    chord_count: int = 0                  # 1 = lone chord/stab, >1 = progression

    # --- optional embedding for similarity / text search ---
    clap_vec: Optional[list[float]] = None

    @property
    def display(self) -> str:
        if self.kind == "break":
            return f"break {self.start_ms/1000:.1f}–{self.end_ms/1000:.1f}s"
        return self.label or "(unpitched)"


@dataclass
class TrackSidecar:
    """Everything detected for one track. Serialized as <track>.json."""

    source_path: str              # absolute path to the original audio file
    album: str
    album_path: str
    title: str
    cover_path: Optional[str]
    duration_ms: int
    sample_rate: int

    # track-level musical summary
    bpm: Optional[float] = None
    bpm_conf: float = 0.0
    key_pc: Optional[int] = None
    key_mode: Optional[str] = None        # "major" | "minor"
    key_conf: float = 0.0

    slices: list[SliceRecord] = field(default_factory=list)
    scanner_version: str = "0.1.0"

    def to_json(self) -> str:
        return json.dumps(asdict(self), indent=2)
