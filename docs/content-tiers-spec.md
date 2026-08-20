# Content Tiers — Directory Layout, the Load Path, and the Extensibility Vision

Status: rev 1 draft, 2026-08-11 — for discussion. Companion to
`docs/module-system-spec.md` (which owns symbols, `import`, hooks, and
`override`); this spec owns **where files live** — in the repo, in the
installed app, and on the user's machine — and how the tiers layer.
Prerequisite for the embedded-toolchain Phase 5 release bundle
(`crates/sequencer/docs/embedded-dgen-toolchain-v0.1-spec.md`).

## 1. The vision (previously unwritten)

eseq's UI, instruments, effects, and workflows are lisp. The end state we
are building toward is the Emacs property: **a user can reshape any part of
their environment without forking it** — reskin one step toggle, replace
the track strip, add a whole new buffer bound to a key chord, or install
someone else's published pack — and still take every app update cleanly.

Emacs gets this not by letting users edit the installed files (its core
elisp ships read-only inside `Emacs.app/Contents/Resources/lisp/` and
nobody touches it) but by three properties we are deliberately replicating:

1. **The core is immutable; user space layers on top.** Doom Emacs — an
   entire re-imagined environment — modifies zero files of the Emacs
   installation. It is "just lisp" loaded after the core.
2. **Resolution is a search path.** User directories come before core, so
   a same-named file/module shadows the factory one.
3. **The language is late-bound and hookable.** Redefinition, advice
   (`override`, module spec §6.1), and hooks (`defhook`, §6) change
   behavior at runtime without touching source that isn't yours.

The user-facing escalation ladder (module spec §6.1), each rung trading
power for update-fragility:

> **customize** (`defcustom`, theme slots) → **extend** (`add-hook`, new
> buffers, keybindings from `~/.eseq.d/init.lisp`) → **override** (replace
> one component; survives updates) → **shadow** (your own module file /
> distro earlier in the load path).

This spec makes rung 4 — and the release bundle itself — physically
possible by sorting all content into tiers with a defined search order.

## 2. Current state (the problem)

Lisp and assets are scattered across two crates, mixed with user data:

- `crates/eseqlisp/` root: genuine core (`init.lisp`, `themes.lisp`,
  `sdf-stdlib.lisp` — the latter `include_str!`ed at `runtime.rs:1245`)
  interleaved with ~25 demo/test files (`sdf-demo.lisp`,
  `scroll-test.lisp`, …).
- `crates/sequencer/`: factory content — `ui/` (123 lisp files),
  `instruments/` (196), `effects/` (66), `defmacros/` (10), `midi-fx/`
  (13), `scripts/` (27), `assets/`, `presets/`, `sample-assets/` — sitting
  next to **user data that must never ship in a bundle**: `projects/`,
  `recordings/`, `sounds/`, `samples/`, `samples.db`,
  `sequencer-crash.log`.
- Path resolution is mostly relative to `enter_sequencer_dir()`;
  `AppPaths` (`crates/sequencer/src/app_paths/mod.rs`) exists with a Dev
  arm and a stubbed Release arm, but `instrument_storage.rs` still resolves
  `INSTRUMENTS_DIR`/`EFFECTS_DIR` relatively (~10 name-lookup sites), and
  the eseqlisp init candidates (`crates/sequencer/src/paths.rs:29-50`)
  have no `$HOME` entry.

Three distinct kinds of content, three different fates:

| Kind | Examples | Fate when installed |
|---|---|---|
| **Factory** | ui lisp, core instruments/effects, defmacros, themes, sdf stdlib, shaders, sample-assets | Read-only inside the bundle; replaced wholesale on update |
| **User** | projects, recordings, samples.db, user instruments, `init.lisp` overrides | `~/Library/Application Support` + `~/.eseq.d`; never touched by updates |
| **Packages** | published instrument packs, distros, shared UI mods | Drop-in directory; versioned via manifest (module spec §8) |

## 3. Repo layout (target)

Pull content out of the crates into a top-level `assets/` that is exactly
the factory tier — packaging becomes "copy `assets/` into
`Contents/Resources/`":

```
eseq/
  assets/                      ;; == factory tier, ships verbatim in the bundle
    core/                      ;; eseqlisp runtime lisp: init, themes, sdf-stdlib, shaders
    ui/                        ;; the sequencer UI modules (today crates/sequencer/ui/)
    instruments/               ;; curated factory instruments
    effects/
    defmacros/
    midi-fx/
    presets/
    sample-assets/
    dgen-toolchain.lock        ;; (stage itself remains gitignored / bundle-time)
  crates/
    eseqlisp/                  ;; code + tests + examples/ (demos move here)
    sequencer/                 ;; code only; no content directories
  docs/
```

Explicitly **not** in the repo after the split: `projects/`,
`recordings/`, `samples.db`, `sounds/` — these move to the dev-mode user
dir (§4) and out of version control. (`crates/sequencer/scripts/` needs a
per-file triage: sequencer demo scripts are factory content; build/tool
scripts stay with the crate.)

## 4. Installed + user layout

```
ESeq.app/Contents/
  MacOS/        metal_seq, DGenLisp, dgen-clang, ld64.lld      ;; Phase 5
  Resources/    <assets/ copied verbatim> + dgen-toolchain/

~/Library/Application Support/eseq/       ;; user tier: big/binary/machine-managed
  projects/  recordings/  samples/  samples.db  sounds/
  instruments/  effects/                  ;; user-authored + copy-on-write forks

~/Library/Caches/eseq/                    ;; dgen dylib cache (cache v2 tier)

~/.eseq.d/                                ;; user tier: hand-edited lisp (dotfile-able)
  init.lisp                               ;; loaded LAST (module spec §7 inversion)
  modules/                                ;; user modules; shadow factory by name
  packages/                               ;; drop-in packs (module spec §8)
```

Split rationale for two user roots: `~/.eseq.d/` is the part users edit,
version-control, and publish (their "dotfiles"); Application Support is
the part the app manages (projects, recordings, caches of record). Emacs
users expect exactly this split.

**Load path** (module spec §7, made concrete — earlier wins for file
shadowing; `init.lisp` still evaluates last so user code wins):

1. `~/.eseq.d/modules/`
2. `~/.eseq.d/packages/<pkg>/src/` (and any configured distro)
3. factory: `Resources/` (release) or `<repo>/assets/` (dev)

### 4.0 Packages: the tier shape recurses; a repo IS a package

A package directory mirrors the factory layout in miniature — any content
type loadable from a tier is loadable from a package with zero new code
per type:

```
alec.acid-tools/            ;; = one git repo
  manifest.json             ;; name, version, deps, declared assets (module spec §8)
  src/                      ;; modules under alec.acid-tools.*
  instruments/303/          ;; dsp.lisp + ui.lisp — same shape as factory (§4.1)
  effects/  midi-fx/  themes/  samples/
```

Distribution is git, not a registry (precedent: straight.el/Doom pins,
Homebrew taps, Strudel's `samples('github:user/repo')` + `strudel.json` —
runtime fetch keyed by repo path, no registry, and the repo name in code
doubles as visible provenance): the convention above IS the standard, and
install v1 is
`git clone` into `~/.eseq.d/packages/`. An `install` command is a thin
convenience — clone, validate manifest, verify declared asset hashes.
Packages ship **source, not binaries**: the embedded dgen toolchain
compiles instrument dsp on the user's machine (no Xcode needed) and the
dgen audit checks the compiled output regardless of origin.

**Projects become portable** as a consequence of qualified ids: a project
referencing `pkg:alec.acid-tools/303@1.2` carries its dependency list, so
opening a shared project can offer one-click install-and-compile of
missing packages.

Deferred deliberately: central index (a name→git-URL repo, later, without
format changes); version resolution (manifest pins a tag/commit; newest
wins + loud warning on conflict); sandboxing — **packages are trusted
code**: installing one runs its lisp, and the mitigations are provenance
visibility and source-form distribution, not a sandbox. Do not design as
if a sandbox exists.

**Copy-on-write editing.** "Edit this factory instrument/module" in-app
copies it into the user tier and edits the copy; the factory original
stays pristine underneath; "revert to factory" deletes the copy. This
restores the just-hack-everything feel without update clobbering.

### 4.1 Instruments / effects / midi-fx: same shape, two roots, NO shadowing

The app ships a curated factory set; users author their own via the
patcher, the in-app agent, or an external Claude Code session. Rules:

- **Identical directory format in every tier** — `dsp.lisp` + `ui.lisp`
  (+ `waves/`, presets) whether under `Resources/instruments/` (factory),
  `…/Application Support/eseq/instruments/` (user), or
  `~/.eseq.d/packages/<pkg>/instruments/`. All tooling (audition harness,
  probe validation, hot-swap compile, external agents writing plain files)
  works on any tier unchanged. `effects/` and `midi-fx/` are symmetric.
- **All creation flows write to the user tier only.** Post-Phase-5 the
  signed bundle makes factory writes physically impossible; any runtime
  code path writing into the factory content dirs today is a bug (audit
  during T1).
- **Browser shows the union with provenance badges** (Factory / Yours /
  package name).
- **Unlike code modules, instruments do NOT shadow by name.** Projects
  reference instruments; a user instrument silently replacing a same-named
  factory one changes the sound of old projects. Project serialization
  records a tier-qualified id — `factory:core/wavetable`, `user:my-kick`,
  `pkg:alec.acid-tools/303` — so cross-tier name collisions are legal and
  unambiguous. This resolves open question 3.
- **"Customize a factory instrument" = fork**: copy into the user tier
  under a new id, rebind in the current project. Mechanics per
  `docs/instrument-fork-spec.md` (notably its p-lock `param_index`
  positional-remap gotcha — the fork path carries the care, not load).
- **Compiled dylibs never live with source**: dgen cache v2 (Caches dir,
  content+toolchain-keyed) makes tier invisible to the engine.
- **Migration**: existing projects reference bare names; on first load
  after T3, resolve bare → `factory:` if present there else `user:`, and
  stamp the qualified id on next save.

## 5. AppPaths is the choke point

`AppPaths` grows tier-aware accessors — sketch:

```rust
impl AppPaths {
    fn factory_root(&self) -> &Path;      // Dev: <repo>/assets ; Release: Resources/
    fn user_data_root(&self) -> &Path;    // Dev: <repo>/.local (gitignored) ; Release: App Support
    fn user_lisp_root(&self) -> &Path;    // ~/.eseq.d (both arms; env-overridable in dev)
    fn load_path(&self) -> Vec<PathBuf>;  // §4 order
    fn instruments_dirs(&self) -> Vec<PathBuf>; // user first, then factory
}
```

Dev arm maps `factory_root` to the checkout and `user_data_root` to a
gitignored `<repo>/.local/` (so dev projects/recordings live next to the
code but out of git). Migration discipline (same as toolchain E1): first
convert every hardcoded relative lookup to an `AppPaths` call **with
unchanged behavior**, then flip the roots — the directory move itself
becomes one line per root, not a repo-wide grep.

## 6. Migration slices

- **T1 — AppPaths coverage.** Every content lookup
  (`instrument_storage.rs` ~10 sites, `paths.rs` init candidates, ui/
  loads, presets, sample-assets) goes through `AppPaths`; zero behavior
  change; grep-clean for `"instruments/"`-style relative literals.
- **T2 — repo split.** `git mv` content into `assets/`, demos into
  `crates/eseqlisp/examples/`; flip the Dev roots; move
  `projects/`/`recordings/`/`samples.db`/`sounds/` to `.local/` and
  gitignore. One commit, mostly renames.
- **T3 — user tier.** Create-on-first-run for App Support + `~/.eseq.d`;
  `$HOME` init candidate; load-path resolution (lands with module spec
  slice 4's init inversion — coordinate).
- **T4 — copy-on-write UX + packages dir.** In-app "fork to user tier" /
  "revert to factory"; `packages/` scan (module spec slice 5).
- Release-arm activation and bundle copying remain Phase 5 of the
  toolchain spec; T1–T2 are its prerequisites.

## 7. Open questions

1. **Dev user-data location**: `<repo>/.local/` (proposed — keeps dev data
   near the code) vs. sharing the real `~/Library/Application Support/eseq`
   even in dev (one project set everywhere, but dev experiments pollute
   it). Leaning `.local/` + an env override.
2. ~~`sounds/` and `samples/` project-path migration~~ — **resolved
   2026-08-20 (eseq-tiers.2), and smaller than it looked.** Measured over
   all 262 saved projects:
   - **`sounds/` is not referenced by projects at all** (zero hits). It is
     a browsable library directory — `list_sound_presets` reads it
     (`project.rs:2636`), `save_container_preset` writes it. Moving it is
     one constant in T1; no migration, no fallback.
   - **`samples/` is referenced only as `sample_path`, and it is already
     content-addressed**: `samples/<sha256>.wav` (366 occurrences in the
     first 80 projects). The sibling `sample_name` (`"Kick72.wav"`) is
     display metadata and is not used for resolution.

   **Decision: no project migration, and no fallback chain either.**
   `sample_path` is an identity with a directory prefix stapled on, so the
   resolver takes one rule — *strip the directory, resolve the hash
   against the sample store* — rather than a chain of guesses. This is the
   permanent design, not legacy compat: it makes projects portable across
   machines and tiers for free, which is what §4.0 already promises for
   packages. New saves may stamp an honest form (`sample:<hash>`); that is
   a save-side change, never a rewrite pass over existing files.

   Noted while measuring: `project.rs:2635` calls
   `create_dir_all(SOUNDS_DIR)` on a **relative** content path — exactly
   the forbidden runtime-write pattern in §4.1, and a hard failure inside
   a signed bundle. Audit target for T1.
3. ~~Instrument name collisions across tiers~~ — resolved, see §4.1:
   instruments do not shadow; projects record tier-qualified ids.
4. ~~Naming: `assets/` vs `content/`~~ — **resolved 2026-08-20:
   `content/`.** `assets/` is already taken with a narrower meaning:
   `crates/sequencer/assets/` holds filter-tables and IRs (binary blobs),
   and `crates/sequencer/sample-assets/` holds artwork. Reusing "assets"
   for the whole factory tree collides with that established local sense,
   and this document's own vocabulary is *content* throughout (content
   tiers, factory content, user content). `~/.eseq.d` stays locked by the
   module spec.
