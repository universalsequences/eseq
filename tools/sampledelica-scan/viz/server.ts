/**
 * Sampledelica viz server (Bun).
 *
 *   bun run viz/server.ts            # serves http://localhost:5173
 *   OUT=./out bun run viz/server.ts  # point at a different scan output dir
 *
 * Loads every sidecar.json, builds a flat region index (with CLAP vectors held
 * in memory for cosine similarity), and serves:
 *   GET /                      the single-page app
 *   GET /api/index             all sources + slices (no vectors — light)
 *   GET /api/similar?uid=..    nearest neighbors of one region (CLAP cosine)
 *   GET /audio?path=..         stream an original mix or slice wav (range-aware)
 *   GET /cover?path=..         stream a cover image
 *
 * Audio is always served from the ORIGINAL files (or the original-mix slice
 * wavs) — never the analysis stems. Path access is whitelisted to OUT and the
 * music library root that the sidecars reference.
 */

import { join, resolve, dirname } from "node:path";

const ROOT = resolve(import.meta.dir, "..");
const OUT = resolve(process.env.OUT ?? join(ROOT, "out"));
const PORT = Number(process.env.PORT ?? 5173);

const PITCH = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

type Slice = {
  uid: string;
  slice_id: string;
  kind: string;
  start_ms: number;
  end_ms: number;
  root_pc: number | null;
  quality: string | null;
  bass_pc: number | null;
  pc_set: number;
  pc_set_norm: number | null;
  label: string | null;
  chord_conf: number;
  note_count: number;
  is_tonal: boolean;
  chord_count: number;
  has_vec: boolean;
  wav_path: string; // absolute, original-mix slice clip
};

type Source = {
  id: string;
  album: string;
  title: string;
  source_path: string;
  cover_path: string | null;
  duration_ms: number;
  sample_rate: number;
  bpm: number | null;
  key_pc: number | null;
  key_mode: string | null;
  key_label: string | null;
  slices: Slice[];
};

// in-memory CLAP index
const vecs = new Map<string, Float32Array>(); // uid -> normalized vector
const regionMeta = new Map<string, { album: string; title: string; sourceId: string }>();

function normalize(arr: number[]): Float32Array {
  const v = new Float32Array(arr);
  let n = 0;
  for (let i = 0; i < v.length; i++) n += v[i] * v[i];
  n = Math.sqrt(n) + 1e-9;
  for (let i = 0; i < v.length; i++) v[i] /= n;
  return v;
}

function cosine(a: Float32Array, b: Float32Array): number {
  let d = 0;
  const n = Math.min(a.length, b.length);
  for (let i = 0; i < n; i++) d += a[i] * b[i];
  return d;
}

const allowedRoots = new Set<string>([OUT]);

async function loadIndex(): Promise<Source[]> {
  const glob = new Bun.Glob("**/sidecar.json");
  const sources: Source[] = [];
  for await (const rel of glob.scan({ cwd: OUT })) {
    const scPath = join(OUT, rel);
    const base = dirname(scPath);
    const doc = JSON.parse(await Bun.file(scPath).text());
    const id = `${doc.album}//${doc.title}`;
    // whitelist the original file's directory so /audio can serve it
    if (doc.source_path) allowedRoots.add(dirname(resolve(doc.source_path)));

    const slices: Slice[] = (doc.slices ?? []).map((s: any) => {
      const uid = `${doc.album}//${doc.title}//${s.slice_id}`;
      if (Array.isArray(s.clap_vec) && s.clap_vec.length) {
        vecs.set(uid, normalize(s.clap_vec));
        regionMeta.set(uid, { album: doc.album, title: doc.title, sourceId: id });
      }
      return {
        uid,
        slice_id: s.slice_id,
        kind: s.kind,
        start_ms: s.start_ms,
        end_ms: s.end_ms,
        root_pc: s.root_pc ?? null,
        quality: s.quality ?? null,
        bass_pc: s.bass_pc ?? null,
        pc_set: s.pc_set ?? 0,
        pc_set_norm: s.pc_set_norm ?? null,
        label: s.label ?? null,
        chord_conf: s.chord_conf ?? 0,
        note_count: s.note_count ?? 0,
        is_tonal: s.is_tonal ?? true,
        chord_count: s.chord_count ?? 0,
        has_vec: Array.isArray(s.clap_vec) && s.clap_vec.length > 0,
        wav_path: resolve(base, s.wav_path),
      };
    });

    sources.push({
      id,
      album: doc.album,
      title: doc.title,
      source_path: doc.source_path,
      cover_path: doc.cover_path ?? null,
      duration_ms: doc.duration_ms ?? 0,
      sample_rate: doc.sample_rate ?? 44100,
      bpm: doc.bpm ?? null,
      key_pc: doc.key_pc ?? null,
      key_mode: doc.key_mode ?? null,
      key_label:
        doc.key_pc != null ? `${PITCH[doc.key_pc % 12]} ${doc.key_mode ?? ""}`.trim() : null,
      slices,
    });
  }
  sources.sort((a, b) => (a.album + a.title).localeCompare(b.album + b.title));
  return sources;
}

console.log(`[viz] loading index from ${OUT} ...`);
let SOURCES = await loadIndex();
console.log(
  `[viz] ${SOURCES.length} sources, ` +
    `${SOURCES.reduce((n, s) => n + s.slices.length, 0)} slices, ` +
    `${vecs.size} embedded.`,
);

function pathAllowed(p: string): boolean {
  const r = resolve(p);
  for (const root of allowedRoots) if (r.startsWith(root)) return true;
  return false;
}

function similar(
  uid: string,
  opts: { crossAlbum: boolean; sameKind: boolean; k: number },
): Array<{ uid: string; sim: number }> {
  const q = vecs.get(uid);
  if (!q) return [];
  const srcMeta = regionMeta.get(uid)!;
  const srcSlice = findSlice(uid);
  const scored: Array<{ uid: string; sim: number }> = [];
  for (const [other, v] of vecs) {
    if (other === uid) continue;
    const m = regionMeta.get(other)!;
    if (opts.crossAlbum && m.album === srcMeta.album) continue;
    if (opts.sameKind && srcSlice) {
      const os = findSlice(other);
      if (os && os.kind !== srcSlice.kind) continue;
    }
    scored.push({ uid: other, sim: cosine(q, v) });
  }
  scored.sort((a, b) => b.sim - a.sim);
  return scored.slice(0, opts.k);
}

const sliceIndex = new Map<string, Slice>();
const sourceById = new Map<string, Source>();
function rebuildLookups() {
  sliceIndex.clear();
  sourceById.clear();
  for (const s of SOURCES) {
    sourceById.set(s.id, s);
    for (const sl of s.slices) sliceIndex.set(sl.uid, sl);
  }
}
rebuildLookups();
function findSlice(uid: string): Slice | undefined {
  return sliceIndex.get(uid);
}

// A fully self-contained pad: everything the player needs to render + trigger.
function padObject(uid: string, origin: "source" | "neighbor", sim: number | null) {
  const sl = findSlice(uid)!;
  const m = regionMeta.get(uid)!;
  const src = sourceById.get(m.sourceId)!;
  return {
    uid,
    origin,
    scope: null as string | null, // for neighbors: "album" | "cross"
    sim,
    kind: sl.kind,
    label: sl.label,
    quality: sl.quality,
    root_pc: sl.root_pc,
    pc_set: sl.pc_set,
    start_ms: sl.start_ms,
    end_ms: sl.end_ms,
    sourceId: m.sourceId,
    album: m.album,
    title: m.title,
    cover_path: src.cover_path,
    source_path: src.source_path, // original mix (fallback playback)
    wav_path: sl.wav_path, // original-mix clip (preload target)
  };
}

function json(data: unknown): Response {
  return new Response(JSON.stringify(data), {
    headers: { "content-type": "application/json" },
  });
}

const server = Bun.serve({
  port: PORT,
  async fetch(req) {
    const url = new URL(req.url);
    const p = url.pathname;

    if (p === "/" || p === "/index.html") {
      return new Response(Bun.file(join(import.meta.dir, "public", "index.html")));
    }

    if (p === "/api/index") {
      return json({
        out: OUT,
        sources: SOURCES,
        stats: {
          sources: SOURCES.length,
          slices: SOURCES.reduce((n, s) => n + s.slices.length, 0),
          embedded: vecs.size,
          albums: [...new Set(SOURCES.map((s) => s.album))],
        },
      });
    }

    if (p === "/api/similar") {
      const uid = url.searchParams.get("uid") ?? "";
      const crossAlbum = url.searchParams.get("crossAlbum") !== "0";
      const sameKind = url.searchParams.get("sameKind") !== "0";
      const k = Number(url.searchParams.get("k") ?? 18);
      const ranked = similar(uid, { crossAlbum, sameKind, k }).map((r) => {
        const sl = findSlice(r.uid)!;
        const m = regionMeta.get(r.uid)!;
        const src = sourceById.get(m.sourceId)!;
        return {
          uid: r.uid,
          sim: r.sim,
          sourceId: m.sourceId,
          album: m.album,
          title: m.title,
          cover_path: src.cover_path,
          kind: sl.kind,
          label: sl.label,
          start_ms: sl.start_ms,
          end_ms: sl.end_ms,
        };
      });
      return json({ uid, results: ranked });
    }

    if (p === "/api/kit") {
      const sourceId = url.searchParams.get("source") ?? "";
      const pads = Number(url.searchParams.get("pads") ?? 45);
      const src = sourceById.get(sourceId);
      if (!src) return json({ error: "unknown source", pads: [] });

      const used = new Set<string>();
      const out: any[] = [];
      // 1. this source's own regions, in time order
      const own = [...src.slices].sort((a, b) => a.start_ms - b.start_ms);
      for (const sl of own) {
        if (out.length >= pads) break;
        used.add(sl.uid);
        out.push(padObject(sl.uid, "source", null));
      }
      // 2. top up the rest of the keyboard with CLAP neighbors — a blend of
      //    same-album (other tracks) and cross-album material, interleaved so
      //    you always get both.
      if (out.length < pads) {
        const cand = new Map<string, number>(); // uid -> best sim
        for (const sl of own) {
          if (!vecs.has(sl.uid)) continue;
          for (const n of similar(sl.uid, { crossAlbum: false, sameKind: false, k: 16 })) {
            if (used.has(n.uid)) continue;
            const prev = cand.get(n.uid);
            if (prev === undefined || n.sim > prev) cand.set(n.uid, n.sim);
          }
        }
        const all = [...cand.entries()].map(([uid, sim]) => ({
          uid,
          sim,
          album: regionMeta.get(uid)!.album,
        }));
        const same = all.filter((c) => c.album === src.album).sort((a, b) => b.sim - a.sim);
        const cross = all.filter((c) => c.album !== src.album).sort((a, b) => b.sim - a.sim);
        let i = 0,
          j = 0;
        while (out.length < pads && (i < same.length || j < cross.length)) {
          // alternate same/cross; fall through when one pool is exhausted
          const takeSame = out.length % 2 === 0 ? i < same.length : j >= cross.length;
          const c = takeSame ? same[i++] : cross[j++];
          if (!c) continue;
          used.add(c.uid);
          const po = padObject(c.uid, "neighbor", c.sim);
          po.scope = c.album === src.album ? "album" : "cross";
          out.push(po);
        }
      }
      return json({ sourceId, source: { title: src.title, album: src.album }, pads: out });
    }

    if (p === "/audio" || p === "/cover") {
      const fpath = url.searchParams.get("path") ?? "";
      if (!fpath || !pathAllowed(fpath)) {
        return new Response("forbidden", { status: 403 });
      }
      const file = Bun.file(resolve(fpath));
      if (!(await file.exists())) return new Response("not found", { status: 404 });
      const total = file.size;
      const range = req.headers.get("range");
      const m = range && /bytes=(\d+)-(\d*)/.exec(range);
      if (m) {
        const start = Number(m[1]);
        const end = m[2] ? Number(m[2]) : total - 1;
        return new Response(file.slice(start, end + 1), {
          status: 206,
          headers: {
            "content-type": file.type,
            "content-range": `bytes ${start}-${end}/${total}`,
            "accept-ranges": "bytes",
            "content-length": String(end - start + 1),
          },
        });
      }
      return new Response(file, {
        headers: { "content-type": file.type, "accept-ranges": "bytes" },
      });
    }

    return new Response("not found", { status: 404 });
  },
});

console.log(`[viz] ▶  http://localhost:${server.port}`);
