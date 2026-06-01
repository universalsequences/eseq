"""Note transcription (BasicPitch) + chord identification.

We transcribe the harmonic stem ONCE into note events, then for any time window
collect the sounding notes, build a 12-bit pitch-class set, and template-match
to a (root, quality). pc_set + pc_set_norm are exactly the Tier-2 search fields.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Optional

import numpy as np

# Chord templates as pitch-class offsets from the root.
TEMPLATES: dict[str, frozenset[int]] = {
    "5":     frozenset({0, 7}),
    "maj":   frozenset({0, 4, 7}),
    "min":   frozenset({0, 3, 7}),
    "dim":   frozenset({0, 3, 6}),
    "aug":   frozenset({0, 4, 8}),
    "sus2":  frozenset({0, 2, 7}),
    "sus4":  frozenset({0, 5, 7}),
    "maj7":  frozenset({0, 4, 7, 11}),
    "dom7":  frozenset({0, 4, 7, 10}),
    "min7":  frozenset({0, 3, 7, 10}),
    "min7b5": frozenset({0, 3, 6, 10}),
    "dim7":  frozenset({0, 3, 6, 9}),
    "maj6":  frozenset({0, 4, 7, 9}),
    "min6":  frozenset({0, 3, 7, 9}),
    "add9":  frozenset({0, 2, 4, 7}),
}

PITCH_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]


@dataclass
class NoteEvent:
    start: float
    end: float
    midi: int
    amp: float


@dataclass
class ChordGuess:
    root_pc: Optional[int]
    quality: Optional[str]
    bass_pc: Optional[int]
    pc_set: int
    pc_set_norm: Optional[int]
    label: Optional[str]
    confidence: float
    note_count: int


def transcribe(stem_path: str) -> list[NoteEvent]:
    """Run BasicPitch on a stem file, return note events."""
    from basic_pitch.inference import predict

    _, _, note_events = predict(stem_path)
    out = []
    for ev in note_events:
        # (start_s, end_s, pitch_midi, amplitude, pitch_bends?)
        start, end, pitch, amp = ev[0], ev[1], int(ev[2]), float(ev[3])
        out.append(NoteEvent(start, end, pitch, amp))
    return out


def notes_in_window(notes: list[NoteEvent], start: float, end: float) -> list[NoteEvent]:
    return [n for n in notes if n.start < end and n.end > start]


def _pc_bitmask(pcs: set[int]) -> int:
    mask = 0
    for pc in pcs:
        mask |= 1 << (pc % 12)
    return mask


def _rotate_norm(pc_set: int, root: int) -> int:
    """Rotate a 12-bit pc_set so `root` becomes bit 0."""
    r = root % 12
    rotated = ((pc_set >> r) | (pc_set << (12 - r))) & 0xFFF
    return rotated


def identify(notes: list[NoteEvent]) -> ChordGuess:
    """Template-match the sounding pitch classes to (root, quality)."""
    if not notes:
        return ChordGuess(None, None, None, 0, None, None, 0.0, 0)

    # weight pitch classes by total sounding energy
    weights = np.zeros(12)
    for n in notes:
        weights[n.midi % 12] += n.amp * (n.end - n.start)
    present = {pc for pc in range(12) if weights[pc] > 0}
    if not present:
        return ChordGuess(None, None, None, 0, None, None, 0.0, 0)
    pc_set = _pc_bitmask(present)
    bass_pc = min(notes, key=lambda n: n.midi).midi % 12

    # A single sustained pitch class is a note one-shot, not a power chord.
    if len(present) == 1:
        root = next(iter(present))
        return ChordGuess(
            root_pc=root, quality="note", bass_pc=root, pc_set=pc_set,
            pc_set_norm=_rotate_norm(pc_set, root), label=PITCH_NAMES[root],
            confidence=0.5, note_count=1,
        )

    best: Optional[tuple[float, int, str]] = None  # (score, root, quality)
    total_w = weights.sum() + 1e-9
    for root in present:
        for quality, tmpl in TEMPLATES.items():
            chord_pcs = {(root + iv) % 12 for iv in tmpl}
            hit = sum(weights[pc] for pc in chord_pcs if pc in present)
            extra = sum(weights[pc] for pc in present if pc not in chord_pcs)
            missing = len(chord_pcs - present)
            # reward covered energy, penalize out-of-chord energy and missing tones
            score = (hit - 0.5 * extra) / total_w - 0.15 * missing
            # prefer the root actually being the bass / strongly present
            score += 0.05 * (weights[root] / total_w)
            if best is None or score > best[0]:
                best = (score, root, quality)

    if best is None:
        return ChordGuess(None, None, None, pc_set, None, None, 0.0, len(present))
    score, root, quality = best
    conf = float(max(0.0, min(1.0, score)))
    label = f"{PITCH_NAMES[root]}{'' if quality == 'maj' else quality}"
    if bass_pc != root:
        label += f"/{PITCH_NAMES[bass_pc]}"
    return ChordGuess(
        root_pc=root,
        quality=quality,
        bass_pc=bass_pc,
        pc_set=pc_set,
        pc_set_norm=_rotate_norm(pc_set, root),
        label=label,
        confidence=conf,
        note_count=len(present),
    )
