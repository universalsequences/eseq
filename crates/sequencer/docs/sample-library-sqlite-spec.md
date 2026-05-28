# Sample Library: SQLite + Tag-Based Browser Spec

## Motivation

The current sample browser walks `crates/sequencer/samples/` and renders a folder tree. Folders are the only way to organize, which means:

- A sample can only live in one place — no multi-category browsing (e.g. a "dark 909 kick" must pick one of `drums/909/` or some hypothetical `dark/` folder).
- Rich metadata (mood, source, processing, era) has nowhere to live.
- The current `samples/` folder is a small hand-picked subset of a much larger library that already exists, with full tag data, in an older project at `~/code/swift/samplemgmt/`.

This spec describes migrating to a SQLite-backed, tag-based sample library, importing the full old library, and rewriting existing project files to point at the new content-addressed sample paths.

## Goals

- Replace folder-based sample browsing with a tag-based query model.
- Import the full sample library (~14k files) from the legacy Postgres + IPFS store, preserving all original tags.
- Rewrite existing projects in `crates/sequencer/projects/*.json` so their sample references continue to work after the layout change.
- Keep the Lisp UI side untouched — the browser module returns the same `SampleTreeNode` shape it does today.

## Non-Goals (for this spec)

- Tag-chip / tag-editing UI in the browser pane.
- Favorites UI.
- Provenance, cover art, YouTube, IPFS hashes-as-content-addressing-protocol, user attribution — all present in the old schema, all out of scope here.
- Online sync between machines.

## Data Model

A new SQLite database at `crates/sequencer/samples.db` (gitignored).

```sql
CREATE TABLE samples (
    id        INTEGER PRIMARY KEY,
    hash      TEXT NOT NULL UNIQUE,           -- content hash, matches samples/<hash>.wav
    title     TEXT,                           -- display name (from old DB; nullable)
    favorited INTEGER NOT NULL DEFAULT 0,
    added_at  INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE tags (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE
);

CREATE TABLE sample_tags (
    sample_id INTEGER NOT NULL REFERENCES samples(id) ON DELETE CASCADE,
    tag_id    INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (sample_id, tag_id)
);

CREATE INDEX idx_sample_tags_tag ON sample_tags(tag_id);
```

Audio storage is flat and content-addressed: `crates/sequencer/samples/<hash>.wav`. The hash in the filename equals `samples.hash`.

Rationale for content-addressed flat layout:

- No filename collisions to resolve.
- Trivially deduped — two paths with the same content collapse to one entry.
- Project files reference samples by stable paths that never change due to renaming or re-tagging.
- The human-readable name lives only in `samples.title`, surfaced in the browser UI.

## Migration Source

Legacy project at `~/code/swift/samplemgmt/`:

- `docker-compose.yml` boots Postgres on `localhost:5434` (user `sampleuser`, pass `samplepass`, db `samplesdb`).
- Schema (`schema.sql`) has one `samples` table: `ipfs_hash PK, title, tags TEXT[], video_id, discogs_id, cover_art_hash, favorited, created_at`.
- Audio files live at `~/code/swift/samplemgmt/ipfs/<hash>/<hash>` (~14,147 entries, not all guaranteed to be WAV).
- `src/export-by-tag.ts` contains existing classification heuristics (DRUM_KEYWORDS, INSTRUMENT_KEYWORDS, MANUFACTURER_PREFIXES, PRODUCER_NAMES, GENRE_KEYWORDS, etc.) that we want to reuse for assigning a primary-category tag.

## Migration Script

Lives at `~/code/swift/samplemgmt/src/migrate-to-eseq.ts` (Bun). Bun is chosen over Rust because:

- The legacy project already has `bun.lock` and a working Postgres client setup.
- `export-by-tag.ts` provides categorization logic we want to reuse mostly verbatim.
- `bun:sqlite` is built-in, no dependency setup.
- It's a one-shot tool, not part of the running app — no reason to pull `tokio-postgres` into the sequencer crate.

### CLI

```
bun run src/migrate-to-eseq.ts \
    --eseq-root <path-to-eseq-repo> \
    [--ipfs-dir <path>]            # default: ./ipfs
    [--projects-dir <path>]        # default: <eseq-root>/crates/sequencer/projects
    [--samples-dir <path>]         # default: <eseq-root>/crates/sequencer/samples
    [--db <path>]                  # default: <eseq-root>/crates/sequencer/samples.db
    [--dry-run]                    # print actions, write nothing
    [--skip-projects]              # migrate samples only, leave projects alone
    [--yes]                        # bypass interactive "OK to wipe samples/?" prompt
```

### Algorithm

Phase A — index existing samples for project rewriting

1. Walk current `samples/**/*.wav`. For each file, compute SHA-256 of its bytes. Build `oldRelativePath -> contentSha256`.

Phase B — index IPFS payloads

2. Walk `<ipfs-dir>/*/<same-name-as-dir>`. For each file:
   - Verify it begins with `RIFF` (WAV magic). Skip with warning if not.
   - Compute SHA-256 of bytes. Note both the IPFS dirname and the content sha256 — they may or may not be equal depending on the legacy hashing convention. The DB key is the dirname; the join key with Phase A is the sha256.

3. Build `ipfsDirName -> contentSha256`.

Phase C — load legacy metadata

4. Connect to Postgres. `SELECT ipfs_hash, title, tags, favorited FROM samples`.

Phase D — write new sample library

5. If not `--dry-run`: snapshot `<projects-dir>` to `<projects-dir>.backup.<unix-timestamp>/` (cp -r).
6. If not `--dry-run` and not `--yes`: interactive confirm before wiping `<samples-dir>`.
7. Initialize SQLite (apply schema; safe to re-run because of `INSERT OR IGNORE`).
8. For each row from Phase C:
   - Look up the file in Phase B's index by `ipfs_hash` (i.e. the dirname).
   - If missing or non-WAV: skip, count toward `missing_audio` summary.
   - Copy IPFS file to `<samples-dir>/<ipfs_hash>.wav`.
   - `INSERT OR IGNORE INTO samples(hash, title, favorited)`.
   - For each tag in `tags`: upsert into `tags`, then insert into `sample_tags`.
   - Run categorization (`export-by-tag.ts` keyword logic) on `title` + existing tags; if it yields a primary category and that tag isn't already attached, add it. This gives every sample at least one top-level category tag, matching how the current folder layout works.

Phase E — rewrite projects

9. Skip if `--skip-projects`.
10. Build `oldRelativePath -> newRelativePath` by joining Phase A and Phase B/D maps via content sha256. `newRelativePath` is `samples/<ipfs_hash>.wav`.
11. For each `<projects-dir>/*.json`:
    - Parse JSON.
    - Recursively walk the value tree. For every string ending in `.wav` and starting with `samples/`:
      - If it's in the map, rewrite it to the new path.
      - Otherwise, leave it and count toward `orphaned_refs` per project.
    - If anything changed: write to `<file>.tmp` then atomic rename.

Phase F — report

12. Print summary:
    - DB samples inserted / skipped (already present).
    - Audio files copied / skipped (missing / non-WAV / hash mismatch).
    - Tags created / reused.
    - Projects rewritten / unchanged.
    - Sample refs rewritten / orphaned (with per-project breakdown for orphans).

### Idempotency & Safety

- Re-running is safe: `INSERT OR IGNORE` on `samples.hash` and `tags.name`. File copies overwrite (same content, same hash).
- `--dry-run` performs Phases A–C and computes Phase D/E actions but writes nothing — used to eyeball the diff before committing.
- Project rewrite only modifies strings that are both `samples/...wav` AND present in the lookup map. A string that happens to match the path prefix but isn't a known sample is left alone.
- Project backup is taken before any project file is touched.
- The `samples/` wipe is gated behind explicit confirmation (or `--yes`).

## Rust Runtime Read Path

Add to `crates/sequencer/Cargo.toml`:

```toml
rusqlite = { version = "0.31", features = ["bundled"] }
```

`tokio-postgres` is **not** added — migration is external.

New module `crates/sequencer/src/db.rs` exposing (rough sketch):

```rust
pub struct SampleDb { /* connection, prepared statements */ }

impl SampleDb {
    pub fn open(path: &Path) -> rusqlite::Result<Self>;

    pub fn list_tags(&self) -> rusqlite::Result<Vec<String>>;

    /// Returns samples matching all `include_tags` and none of `exclude_tags`,
    /// optionally filtered by free-text match against `title`. Ordered by title.
    pub fn query(
        &self,
        include_tags: &[&str],
        exclude_tags: &[&str],
        text: Option<&str>,
        favorites_only: bool,
    ) -> rusqlite::Result<Vec<SampleRow>>;

    pub fn tags_for(&self, hash: &str) -> rusqlite::Result<Vec<String>>;
    pub fn add_tag(&self, hash: &str, tag: &str) -> rusqlite::Result<()>;
    pub fn remove_tag(&self, hash: &str, tag: &str) -> rusqlite::Result<()>;
    pub fn set_favorite(&self, hash: &str, favorited: bool) -> rusqlite::Result<()>;
}

pub struct SampleRow {
    pub hash: String,
    pub title: Option<String>,
    pub favorited: bool,
}
```

The db is opened once per process at app start, behind a `OnceLock` or similar.

## Browser Integration

`crates/sequencer/src/bin/metal_seq/browser.rs`:

- Add `build_sample_tree_from_db(query: &str, include_tags: &[&str], exclude_tags: &[&str]) -> Value`. Returns the same `SampleTreeNode`-shaped `Value` the Lisp UI already consumes — no UI changes required to render results.
- Tree grouping: when no tag filters are active, group by primary-category tag (drums, instruments, manufacturers, producers, genres, fx, vocals, hardware) for parity with the current folder layout. When tag filters are active, return a flat list.
- Leaf nodes: `label = title` (or `hash` if title is null), `path = samples/<hash>.wav`.
- Fallback: if the DB file is absent, fall through to the existing `build_sample_tree_node` folder walk so the app remains usable before migration.

Tag-chip and tag-editing UI are explicit follow-up work — this spec only delivers the DB-backed query path equivalent to today's tree.

## Open Risks

- **Legacy Postgres availability.** The migration assumes the docker volume `postgres_data` from `~/code/swift/samplemgmt/docker-compose.yml` still has the data. If the volume is gone, the source is gone — there is no other copy of the tag data.
- **IPFS dirname vs content hash.** We treat the dirname as the canonical key in our new DB, and use content sha256 only as the join key for project rewriting. We assume the dirname is stable. If a small number of IPFS entries are mis-named or duplicated, they will be visible in the Phase F report.
- **Null / empty titles.** Some legacy rows may have no title. Browser falls back to displaying the hash. The hash is also always available as a tooltip / detail.
- **Orphaned project refs.** Any sample in a project that wasn't part of the legacy library (or whose source file is missing) will not be rewritten. The migration reports these per project so they can be re-pointed manually before the project is opened.

## Order of Operations

1. Land: Cargo dep + schema + `db.rs` module (compiles, but unused).
2. Write the migration script in `~/code/swift/samplemgmt/src/migrate-to-eseq.ts`.
3. `docker-compose up postgres` in the legacy project. Run migration with `--dry-run`. Inspect the report.
4. Run migration for real. Verify SQLite contents and project diffs.
5. Swap `browser.rs` to use `build_sample_tree_from_db` with the fallback.
6. Open the app, sanity-check sample browsing.
7. Land tag UI work in follow-up PRs.
