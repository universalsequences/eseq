"""Streamlit audition UI for scanner output.

    uv run --extra app streamlit run app/audition.py

Browse detected slices per track, see the chord/key labels, listen, and (if
sidecars include CLAP vectors) search slices by a text vibe.
"""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import streamlit as st

st.set_page_config(page_title="Sampledelica audition", layout="wide")

OUT = Path(st.sidebar.text_input("Output dir", "./out"))
sidecars = sorted(OUT.rglob("sidecar.json"))

st.title("🎛️ Sampledelica — audition")
if not sidecars:
    st.warning(f"No sidecar.json under {OUT}. Run the scanner first.")
    st.stop()

# ---- cross-album similarity (CLAP) ----
from sampledelica_scan import search as rsearch  # noqa: E402

regions = rsearch.load_index(OUT)
st.sidebar.markdown(f"**{len(regions)}** regions with embeddings")

st.sidebar.markdown("### Cross-album search")
if st.session_state.pop("clear_vibe_query", False):
    st.session_state["vibe_query"] = ""
query = st.sidebar.text_input("Vibe search (text → audio)", "", key="vibe_query")
kind_pick = st.sidebar.multiselect("Limit to kinds", ["chord", "stab", "break"], default=[])
cross_only = st.sidebar.checkbox("Cross-album only (for 'similar to')", value=True)

ranked: list[tuple] = []
sel_uid = st.session_state.get("similar_to")

if query and regions:
    try:
        from sampledelica_scan import embed

        tv = embed.embed_text(query)
        if tv is None:
            st.sidebar.info("CLAP not installed (uv sync --extra clap).")
        else:
            ranked = rsearch.rank_by_vector(
                regions, tv, kinds=set(kind_pick) or None, top_k=15)
    except Exception as e:  # noqa: BLE001
        st.sidebar.error(f"search unavailable: {e}")
elif sel_uid and regions:
    ranked = rsearch.similar_to_region(
        regions, sel_uid, cross_album_only=cross_only,
        same_kind=not kind_pick, kinds=set(kind_pick) or None, top_k=15)
    src = next((r for r in regions if r.uid == sel_uid), None)
    if src:
        st.sidebar.caption(f"Similar to **{src.label or src.kind}** "
                           f"from _{src.album}_")
        if st.sidebar.button("clear selection"):
            del st.session_state["similar_to"]
            st.rerun()

if ranked:
    st.sidebar.markdown("### Results")
    for r, sim in ranked:
        st.sidebar.write(f"`{sim:.2f}` **{r.label or r.kind}** — {r.album[:24]}")
        if Path(r.wav_path).exists():
            st.sidebar.audio(r.wav_path)

PITCH = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]


def keylabel(doc) -> str:
    return f"{PITCH[doc['key_pc']]} {doc['key_mode']}" if doc.get("key_pc") is not None else "?"


# ---- load every sidecar once, group by album ----
albums: dict[str, list[tuple[Path, dict]]] = {}
for sc_path in sidecars:
    doc = json.loads(sc_path.read_text())
    albums.setdefault(doc["album"], []).append((sc_path, doc))

# ---- sidebar filters ----
kind_filter = st.sidebar.multiselect("Show kinds", ["chord", "stab", "break"],
                                     default=["chord", "stab", "break"])
all_qualities = sorted({s.get("quality") for _, docs in albums.items()
                        for _, d in docs for s in d["slices"] if s.get("quality")})
qual_filter = st.sidebar.multiselect("Only qualities (empty = all)", all_qualities, default=[])


def slice_visible(s) -> bool:
    if s["kind"] not in kind_filter:
        return False
    if qual_filter and s.get("quality") not in qual_filter:
        return False
    return True


def region_uid(doc, s) -> str:
    return f"{doc['album']}//{doc['title']}//{s['slice_id']}"


def render_slice(base: Path, doc, s) -> None:
    wav = base / s["wav_path"]
    uid = region_uid(doc, s)
    has_embedding = bool(s.get("clap_vec"))
    c = st.columns([2, 1, 1, 1, 1, 4])
    c[0].markdown(f"**{s['slice_id']}** · {s['kind']}")
    c[1].markdown(f"`{s.get('label') or '—'}`")
    c[2].markdown(f"pcset `{s['pc_set']}`")
    c[3].markdown(f"{s['start_ms']/1000:.1f}s")
    if c[4].button("similar", key=f"similar-{uid}", disabled=not has_embedding,
                   help="Find embedded regions that sound like this slice"):
        st.session_state["similar_to"] = uid
        st.session_state["clear_vibe_query"] = True
        st.rerun()
    if wav.exists():
        c[5].audio(str(wav))


for album, docs in albums.items():
    docs = sorted(docs, key=lambda x: x[1]["title"])
    cover = next((d.get("cover_path") for _, d in docs if d.get("cover_path")), None)

    # ---- album-level rollup across ALL its tracks ----
    all_slices = [s for _, d in docs for s in d["slices"]]
    n_chord = sum(1 for s in all_slices if s["kind"] == "chord")
    n_break = sum(1 for s in all_slices if s["kind"] == "break")
    distinct_chords = sorted({s["label"] for s in all_slices
                              if s["kind"] == "chord" and s.get("label")})

    hcols = st.columns([1, 5])
    if cover and Path(cover).exists():
        hcols[0].image(cover, width=160)
    with hcols[1]:
        st.subheader(album)
        st.markdown(f"**{len(docs)} tracks** · {len(all_slices)} slices · "
                    f"**{n_chord} chords** · {n_break} breaks")
        if distinct_chords:
            st.markdown("**Chords found across the album:** "
                        + " ".join(f"`{c}`" for c in distinct_chords))

    # ---- per-track detail ----
    for sc_path, doc in docs:
        base = sc_path.parent
        visible = [s for s in doc["slices"] if slice_visible(s)]
        with st.expander(f"{doc['title']}   ·   key {keylabel(doc)} · {doc.get('bpm', 0):.0f} bpm "
                         f"· {len(visible)}/{len(doc['slices'])} slices", expanded=False):
            spec = base / "spectrogram.png"
            if spec.exists():
                st.image(str(spec), width="stretch")
            for s in visible:
                render_slice(base, doc, s)
    st.divider()
