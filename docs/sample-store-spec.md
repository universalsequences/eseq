# Sample Store — Content Addressing, the Metadata Index, and Package Sample Contribution

Status: rev 1, 2026-08-20. Companion to `docs/content-tiers-spec.md` (§4.0
packages, §4.1 tier shape) and `docs/module-system-spec.md` §8. Prerequisite
for `eseq-mods.6` (S5: packages) and `eseq-tiers.6` (T4: packages dir).

## 1. Why this document exists

Content tiers §7 question 2 resolved how *projects* reference samples: the
`sample_path` string `samples/<sha256>.wav` is an identity with a directory
prefix stapled on, so resolution is one rule — strip the prefix, resolve the
hash against the sample store. That resolves projects.

It does not resolve the other half. Once packages exist, many parties
contribute samples and, more importantly, *statements about* samples:

- the factory set,
- the user's own imports and hand-curation,
- any number of installed packages.

Those statements are titles, tags, sources, provenance, and origin. Today
every one of them lives in a single mutable SQLite file that is the sole
system of record. That does not survive contact with packages. This document
specifies what is stored where, what a package ships, and how the pieces
merge.

## 2. Current state

**Store.** `crates/sequencer/samples/` — 13,783 files named `<sha256>.wav`,
12 GB, gitignored. Written by `sample_import::import_one`.

**Index.** `crates/sequencer/samples.db` — 10 MB SQLite, schema in
`sample_db.rs::SCHEMA_SQL`. Row counts as measured 2026-08-20:

| table | rows |
| --- | ---: |
| `samples` | 13,783 |
| `tags` | 1,712 |
| `sample_tags` | 49,170 |
| `sources` | 12,299 |
| `source_refs` | 14,700 |
| `sample_origins` | 13,760 |
| `source_assets` | 297 |

Two schema facts carry the whole design:

- **`samples.hash` is `TEXT NOT NULL UNIQUE`** — the portable identity
  already exists.
- **`tags.name` is `TEXT NOT NULL UNIQUE COLLATE NOCASE`** — the merge key
  for tags is already a case-insensitive string, and
  `sample_import::normalize_tags` already dedupes case-insensitively while
  preserving the first spelling.

Every `INTEGER PRIMARY KEY` in the schema is a **local surrogate key**. Ids
are meaningful only on one machine and must never appear in anything
shipped or shared.

### 2.1 The stored file is a transcode, and its name is not its own hash

`sample_import::import_one` decodes the source file
(`decode_audio_file`), writes a canonical WAV (`write_wav`), and names the
result `{hash}.wav` — where `hash` is `hex_sha256` of the **original source
bytes** (`sample_import.rs:190`), not of the WAV that was written.

Consequences, all of which this spec must respect:

- **A stored file cannot be verified by re-hashing it.** The store is
  content-*named*, not content-*verified*. Any integrity or dedupe check
  must compare against the recorded source hash, never against the bytes on
  disk.
- **Dedupe is by source bytes.** The same original file imported twice
  dedupes correctly (`contains_sample` short-circuits at stage time). The
  same *sound* shipped in two encodings (an mp3 and a normalized wav) has
  two hashes and becomes two rows. Accepted; not worth solving.
- **Ingest is a transcode, not a copy.** A copy-on-write clone
  (APFS `clonefile`) is a legal optimization only when the input is already
  a canonical WAV. Do not design the package install path as if it were
  always a cheap link.

### 2.2 The actual fragility

Delete `samples.db` today and 49,170 hand-made tag assignments are gone
permanently. Nothing else records them. The index is the system of record
while looking like a cache. Every question below gets easier once that is
untrue, so it is fixed first.

## 3. Principle: the index is derived; the facts are files

Split the two roles that `samples.db` currently plays.

- **Facts of record** are plain, line-oriented, per-tier text files. The
  user's curation is one such file; each package ships its own.
- **`samples.db` is a rebuildable index** over the union of those files.
  Deleting it is a non-event; it is reconstructed by re-ingesting every
  tier.

This is what makes one database viable, makes package install/uninstall a
pair of ordinary SQL statements, and makes corruption recoverable.

## 4. Storage layout

**One database, in the user tier, rebuildable.**

```
~/Library/Application Support/eseq/
  samples/            ;; the content-addressed store: <sha256>.wav
  samples.db          ;; DERIVED index; safe to delete
  samples.jsonl       ;; the user's own facts of record (append-only)
```

**Rejected: one database per tier, `ATTACH`ed and `UNION`ed at query time.**
It reads tidy but each attached database carries its own integer id space,
so every cross-tier tag or source query becomes a join across mismatched
surrogate keys, and `sample_db.rs`'s existing queries
(`query_samples_for_browser`, `adjacent_tags`) would each need a
multi-database rewrite. Not worth it when the merge key is a string.

The single index gains a provenance dimension instead (§6).

## 5. What a package ships

**A package never ships a `.db` file.** Packages are git repos (content
tiers §4.0); a binary SQLite blob in git is a merge-conflict disaster and is
opaque to review. Packages ship text.

### 5.1 The pretty tree is the source form

Package authors keep a human directory layout — the thing they edit in
Finder, commit, and diff:

```
alec.acid-tools/
  manifest.json           ;; package identity (module spec §8)
  samples/
    kicks/808/long.wav
    kicks/808/short.wav
    hats/closed/tight.wav
  samples.jsonl           ;; GENERATED — see §5.2
```

Nobody hand-authors a directory of sha256 filenames, and committing both a
pretty tree and a content-addressed tree doubles the bytes in git. So the
package ships the pretty tree, and the manifest carries the hashes.

### 5.2 `samples.jsonl` is generated, hash-keyed, and line-oriented

`eseq package index` walks `samples/`, hashes each file, and writes one
JSON object per line. The author never types a hash; the tool writes them.

```jsonl
{"hash":"79b54e69…","path":"kicks/808/long.wav","title":"808 Long","tags":["kicks","808"]}
{"hash":"0bb46702…","path":"hats/closed/tight.wav","title":"Tight","tags":["hats","closed"],"source":"pkg:alec.acid-tools/roland-cr78"}
{"kind":"source","id":"pkg:alec.acid-tools/roland-cr78","title":"Roland CR-78","refs":[{"provider":"discogs","ref_kind":"release","ref_value":"12345"}]}
```

Properties this buys:

- **Merges with no id negotiation.** The join key is `hash`, already
  `UNIQUE` in `samples`. Local integer ids are never exported.
- **Tags dedupe for free.** `drums` from a package and `Drums` from the
  user are the same tag by `UNIQUE COLLATE NOCASE`, and `normalize_tags`
  already implements the matching policy.
- **Git-friendly.** One line per sample diffs and merges; a package update
  shows as a readable changeset.
- **Verifiable payload.** The hash pins the *package file* — a shipped
  sample can be checked against its manifest entry on install. Note the
  §2.1 limit: this verifies the package payload, **not** the transcoded
  copy that lands in the store.
- **Metadata-only packages fall out for free.** A package may carry entries
  for hashes it does not ship — e.g. genre tags for a corpus the user
  already owns. Same format, no special case.

### 5.3 Directory structure ingests into tags

Strudel's `{"bd": [...], "sd": [...]}` map is not a second addressing
scheme — it is *tags with ordering*, which `tags` + `sample_tags` already
model. So the pretty tree is not a parallel namespace to maintain; it is
tag input:

```
kicks/808/long.wav   →   tags: kicks, 808     title: 808 Long
```

Path segments become tags, the file stem becomes the default title
(`default_title_for_path` already does the `_`/`-` → space normalization).
`eseq package index` derives these on generation; the manifest may then
override any of them explicitly. The manifest carries only what a path
cannot express: source and provenance, external refs, license, explicit
ordering, name overrides.

Note this does not exist today for user imports either —
`sample_import::stage_one` always sets `tags: Vec::new()`, and only
`batch_tags` reach `insert_sample_with_tags`. Path-derived tagging is new
work shared by both paths.

### 5.4 Does a package ship the audio?

Both are legal, and the manifest is identical either way:

- **Audio-bearing** — wavs committed in the repo. Simple, offline, large.
- **Manifest-only + fetch** — precedent is Strudel's
  `samples('github:user/repo')`. Needs network at install.

Because every entry is hash-keyed, missing audio is *detectable* rather
than fatal: the package loads, its metadata merges, and entries whose bytes
are absent render as unavailable in the browser. Do not make audio presence
a load-time requirement.

## 6. Merge semantics

**Provenance is a table, not a column.** Content addressing means two
packages can legitimately ship the same hash; the file is stored once and
carries *two* origins. A column on `samples` cannot express that.

```sql
CREATE TABLE sample_provenance (
    sample_id INTEGER NOT NULL REFERENCES samples(id) ON DELETE CASCADE,
    origin    TEXT NOT NULL,   -- 'factory' | 'user' | 'pkg:<name>'
    PRIMARY KEY (sample_id, origin)
);
```

This is what the browser's provenance badges (content tiers §4.1) read.

**Source ids must be namespaced on the wire.** `sources.id` is an INTEGER.
Packages use string ids scoped by package name
(`pkg:alec.acid-tools/roland-cr78`), mapped to local integers at ingest —
the same device as tier-qualified instrument ids in content tiers §4.1.

**`source_refs` collides globally and should dedupe.** Its
`UNIQUE(provider, ref_kind, ref_value)` has no `source_id` in it, so two
packages both citing Discogs release 12345 collide by construction. The
correct resolution is to **converge on one source row** — that is the
desired behavior, since it really is one release — but ingest must do so
deliberately rather than failing the insert.

**Tag attribution.** `sample_tags` has no origin column, so uninstalling a
package cannot currently retract exactly the tags it added without also
removing the same tag the user applied by hand. Either add an origin to
`sample_tags` or accept that uninstall leaves tags behind. **Open
question 1.**

**Uninstall.** With provenance recorded, uninstall is
`DELETE … WHERE origin = 'pkg:<name>'` plus a refcount check before any
file is removed from the store — a hash contributed by two packages, or
also imported by the user, must survive removing one of them.

## 7. Attribution: two axes, not one

"Which package did this come from?" and "who made this sound?" are
different questions, and the schema already separates them. Do not collapse
them into one field.

**Axis 1 — delivery / provenance.** Which tier or package put this file on
*this machine*. Modeled by `sample_provenance` (§6). Cardinality: a sample
has one origin per contributor, usually one.

**Axis 2 — credit / attribution.** Who made the sound and what release it
came from. Already modeled, and already populated:
`sample_origins` → `sources` → `source_contributors` / `source_refs` /
`source_assets`.

These are orthogonal. One package delivers hundreds of samples spanning
dozens of unrelated artists and records; browsing by package is not
browsing by artist. Both are worth having.

### 7.1 What is already in the database and invisible

Measured 2026-08-20 on the developer's `samples.db`:

- **13,760 of 13,783 samples already carry a source origin.**
- `sources` holds 12,299 rows — 11,345 `unknown_media`, 637
  `youtube_video`, 298 `discogs_release`, 19 `local_file` — with real
  titles (`George Benson - In Flight` covers 32 samples,
  `Piero Umiliani - Percussioni ed…` covers 20).
- `source_refs` holds 14,700 external references: 13,741 legacy IPFS
  payloads, 637 YouTube videos, 303 Discogs releases.
- `source_assets` holds 297 artwork images
  (`crates/sequencer/sample-assets/*.jpg`).

**The source graph is dormant at both ends — nothing reads it, and almost
nothing writes it.**

*Nothing reads it.* `SampleDb::query` selects only
`s.id, s.hash, s.title, s.favorited`, filters only on tags / text /
favorites, and `SampleRow` carries only hash, title, favorited, tags.
`ui/browser.rs` does not mention `sources`, `source_id`, `release_title`,
or `source_assets` at all.

*Almost nothing writes it.* Of 13,760 `sample_origins` rows, **13,741 have
`method = 'legacy_migration'` and only 19 have `method =
'local_sample_import'`.** The provider breakdown says the same thing:
13,741 `legacy_ipfs` refs against 637 YouTube and 303 Discogs. The source
graph is a **one-time import of a previous system's data**, not something
the running app meaningfully produces. Every sample added through the
normal import path since then has arrived with no source at all.

Two consequences for this spec:

1. **Do not treat the source tables as a live, growing dataset.** They are
   a frozen snapshot with a nearly-dead write path. Any "browse by
   artist/record" work is archaeology over legacy rows plus a real decision
   about whether to start populating them again.
2. **Packages are the natural way to revive them.** §7.3's manifest source
   lines would make installed packages the *primary* producer of source and
   contributor rows going forward — properly credited at authoring time,
   rather than guessed at during import. That is a better answer than
   retrofitting metadata inference onto the local import path.

If sources stay dormant, axis 2 simply does not ship, and `sample_provenance`
(axis 1, §7.2) still delivers browse-by-package on its own — the two axes
are independent, and package browsing does not depend on any of this. The
remaining gap for axis 2, whenever it is picked up, is that
`source_contributors` has **0 rows**: artist is baked into the source
*title* string (`"George Benson - In Flight"`) rather than split out.

This is also the substrate the sampledelica idea (chord-searchable album
extraction) would build on, which is the strongest argument for not
deleting it.

### 7.2 Browse by package

**Rejected: making provenance a reserved tag** (`pkg:alec.acid-tools` as a
row in `tags`). It is tempting because it would ride every existing
mechanism for free — `query`'s include/exclude paths, `adjacent_tags`, the
chip UI, filtering — with zero new code. Reject it anyway: **tags are
user-editable opinions; provenance is a fact.** As a tag it could be
deleted, or applied by hand to a sample the package never shipped, and it
would entangle package uninstall with the tag-attribution question in §6.

**Instead: keep the table, expose it as a facet.** Concretely:

- `SampleDb::query` gains an `origins: &[&str]` filter, applied as an
  `EXISTS` clause over `sample_provenance` in the same shape as the
  existing tag clauses.
- `SampleRow` gains its origins, so rows can render a provenance badge
  (content tiers §4.1 already requires Factory / Yours / package name).
- The browser gets a package facet — a rail or chip group listing installed
  packages, selectable like a tag but rendered as what it is.
- `adjacent_tags` should stay tag-only; co-occurrence between packages is
  not meaningful the way tag co-occurrence is.

Filtering to one package then composes with everything else for free: this
package **and** tag `kick` **and** favorites.

### 7.3 What a package ships for attribution

The `{"kind":"source", …}` manifest lines in §5.2 extend to carry credit,
which is what fills the currently-empty `source_contributors`:

```jsonl
{"kind":"source","id":"pkg:alec.acid-tools/roland-cr78",
 "title":"CR-78 Factory Patterns","release_title":"Roland CR-78",
 "contributors":[{"role":"manufacturer","name":"Roland"}],
 "refs":[{"provider":"discogs","ref_kind":"release","ref_value":"12345"}],
 "assets":[{"kind":"cover","hash":"…","path":"art/cr78.jpg"}]}
```

Package authors credit sources properly, those credits merge into the same
tables the local library uses, and a sample delivered by a package is
browsable by *both* its package and its artist.

## 8. Migration slices

- **M1 — user facts of record.** Write user curation (import, tag add /
  remove, favorite, title) to `samples.jsonl` in addition to the database.
  Add an index rebuild that reconstructs `samples.db` from the store plus
  the journal. Exit: deleting `samples.db` loses nothing. Independent of
  packages; do it first, it is the fragility fix.
- **M2 — provenance + path-derived tags.** Add `sample_provenance`;
  backfill every existing row as `user`. Derive tags from path segments on
  import and populate `StagedSample.tags`.
- **M3 — the manifest format + `eseq package index`.** Generate and read
  `samples.jsonl`; verify shipped payloads against their hashes.
- **M4 — package ingest.** Scan `~/.eseq.d/packages/*/samples.jsonl` on
  install, merge under `pkg:<name>` provenance, namespace source ids,
  dedupe `source_refs`, refcount on uninstall.
- **M5 — browse by package (§7.2).** Add the `origins` filter to
  `SampleDb::query`, carry origins on `SampleRow`, add the package facet
  and the provenance badges content tiers §4.1 already requires. Depends
  only on M2's `sample_provenance`, **not** on the source graph.
- **M6 — source graph revival (§7.1), optional and separable.** Surface
  record/artist rows, Discogs and YouTube refs, and `source_assets` cover
  art; backfill `source_contributors` by splitting artist out of the source
  title. Only worth doing if sources are going to be populated going
  forward — which realistically means M3/M4 package manifests, since the
  local import path has produced 19 source rows in the library's lifetime.

M1, M2, and M5 are useful on their own and do not wait for the package
system. M3 and M4 land with `eseq-mods.6` / `eseq-tiers.6`. M6 is
discretionary and should not gate anything.

## 9. Open questions

1. **Tag attribution on uninstall** — add an origin column to
   `sample_tags`, or accept residue? (§6)
2. **Journal format for user facts** — append-only event log (replayable,
   preserves history, grows) vs. a rewritten snapshot (compact, loses
   history). Leaning append-only with periodic compaction, matching how the
   store itself is append-only.
3. **Licensing.** Distributing sample packs makes license terms a real
   field, not a nicety. Per-package default with a per-source override is
   the obvious shape (`sources` already has `notes`), but whether the
   browser must *display* it, and whether an unlicensed package should warn
   on install, is undecided.
4. **Should the store filename become the hash of the stored bytes?**
   That would make the store self-verifying (§2.1) but changes 13,783
   filenames and every `sample_path` in 262 projects — which the tiers.2
   decision explicitly declined to do. Probably no; record the reasoning
   rather than revisiting it each time someone notices.
