"""Per-track orchestration: decode -> separate -> key/bpm -> segment ->
chord-id -> render slices + spectrogram -> write sidecar JSON.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

import numpy as np

from . import audio_io, chords, embed, keyfind, segment, separate
from .library import Track
from .schema import SliceRecord, TrackSidecar

SR = 44100


@dataclass
class Options:
    cache_dir: Path
    device: str | None = None
    do_embed: bool = False
    do_breaks: bool = True
    do_stabs: bool = True
    max_stabs: int = 24
    render_from_stem: bool = False  # default: cut slices from the ORIGINAL full mix
                                    # (stems are for detection only — stem audio is gurgly)


def _write_wav(path: Path, y: np.ndarray, sr: int) -> None:
    import soundfile as sf

    path.parent.mkdir(parents=True, exist_ok=True)
    data = y.T if y.ndim > 1 else y
    sf.write(str(path), data, sr)


def _spectrogram(out_png: Path, mix_mono: np.ndarray, sr: int,
                 slices: list[SliceRecord]) -> None:
    import librosa
    import librosa.display
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    S = librosa.amplitude_to_db(np.abs(librosa.stft(mix_mono)), ref=np.max)
    fig, ax = plt.subplots(figsize=(14, 4))
    librosa.display.specshow(S, sr=sr, x_axis="time", y_axis="log", ax=ax)
    for s in slices:
        color = "cyan" if s.kind == "break" else "yellow"
        ax.axvspan(s.start_ms / 1000, s.end_ms / 1000, color=color, alpha=0.18)
        ax.text(s.start_ms / 1000, sr / 2, s.display, color=color,
                fontsize=7, rotation=90, va="top")
    fig.tight_layout()
    out_png.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(str(out_png), dpi=90)
    plt.close(fig)


def process_track(track: Track, out_dir: Path, opts: Options) -> TrackSidecar:
    out_dir = Path(out_dir)
    rel = f"{track.album}/{track.title}"
    track_out = out_dir / rel
    slices_dir = track_out / "slices"

    # 1. decode (full mix, both for analysis and rendering)
    mix_st, _ = audio_io.decode(track.path, sr=SR, mono=False)
    mix_mono = audio_io.to_mono(mix_st)
    duration_ms = int(len(mix_mono) / SR * 1000)

    # 2. separate
    stems = separate.separate(track.path, opts.cache_dir, device=opts.device)
    harm_st, _ = audio_io.decode(stems["other"], sr=SR, mono=False)
    harm_mono = audio_io.to_mono(harm_st)
    drum_mono = audio_io.to_mono(audio_io.decode(stems["drums"], sr=SR, mono=True)[0])

    # 3. track-level key + tempo
    key_pc, key_mode, key_conf = keyfind.detect_key(mix_mono, SR)
    bpm, beats = segment.beat_grid(mix_mono, SR)

    sidecar = TrackSidecar(
        source_path=str(track.path),
        album=track.album,
        album_path=str(track.album_path),
        title=track.title,
        cover_path=str(track.cover_path) if track.cover_path else None,
        duration_ms=duration_ms,
        sample_rate=SR,
        bpm=bpm,
        bpm_conf=0.5,
        key_pc=key_pc,
        key_mode=key_mode,
        key_conf=key_conf,
    )

    # 4. candidate segments
    segs: list[segment.Segment] = []
    if opts.do_stabs:
        segs += segment.detect_stabs(harm_mono, SR, top_k=opts.max_stabs)
    if opts.do_breaks:
        segs += segment.detect_breaks(drum_mono, harm_mono, SR, beats)
    segs.sort(key=lambda s: s.start_s)

    # 5. transcribe harmonic stem once (for chord-id of tonal segments)
    notes = chords.transcribe(str(stems["other"])) if any(s.is_tonal for s in segs) else []

    # 6. build records + render
    counters = {"stab": 0, "break": 0, "chord": 0}
    for seg in segs:
        counters[seg.kind] += 1
        sid = f"{seg.kind}-{counters[seg.kind]:03d}"
        a, b = int(seg.start_s * SR), int(seg.end_s * SR)

        # Cut the rendered sample from the ORIGINAL full mix by default. Stems
        # are only used to *locate* chords/breaks; stem audio quality is poor.
        src = harm_st if (opts.render_from_stem and seg.is_tonal) else mix_st
        clip = src[:, a:b] if src.ndim > 1 else src[a:b]
        wav_path = slices_dir / f"{sid}.wav"
        _write_wav(wav_path, clip, SR)

        rec = SliceRecord(
            slice_id=sid,
            wav_path=str(wav_path.relative_to(track_out)),
            kind=seg.kind,
            start_ms=int(seg.start_s * 1000),
            end_ms=int(seg.end_s * 1000),
            method="onset+basicpitch" if seg.is_tonal else "drum-energy",
            is_tonal=seg.is_tonal,
        )

        if seg.is_tonal:
            g = chords.identify(chords.notes_in_window(notes, seg.start_s, seg.end_s))
            rec.root_pc = g.root_pc
            rec.quality = g.quality
            rec.bass_pc = g.bass_pc
            rec.pc_set = g.pc_set
            rec.pc_set_norm = g.pc_set_norm
            rec.label = g.label
            rec.chord_conf = g.confidence
            rec.note_count = g.note_count
            rec.chord_count = 1  # single-window stab; progressions come later
            # 2+ distinct pitch classes = a chord; a single pitch = a note one-shot
            rec.kind = "chord" if (g.quality not in (None, "note")) else "stab"

        if opts.do_embed:
            # Embed the ORIGINAL mix region in memory (timbre fingerprint for
            # cross-album similarity) — independent of how the slice is rendered.
            region = mix_st[:, a:b] if mix_st.ndim > 1 else mix_st[a:b]
            rec.clap_vec = embed.embed_region(region, SR)

        sidecar.slices.append(rec)

    # 7. spectrogram + sidecar json
    _spectrogram(track_out / "spectrogram.png", mix_mono, SR, sidecar.slices)
    (track_out / "sidecar.json").write_text(sidecar.to_json())
    return sidecar
