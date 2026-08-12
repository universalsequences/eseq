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

**Copy-on-write editing.** "Edit this factory instrument/module" in-app
copies it into the user tier and edits the copy; the factory original
stays pristine underneath; "revert to factory" deletes the copy. This
restores the just-hack-everything feel without update clobbering.

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
2. **`sounds/` and `samples/`**: pool-referenced by existing projects —
   moving them requires a project-path migration or a resolve-time
   fallback chain. Decide before T2.
3. **Instrument name collisions across tiers**: user `instruments/foo/`
   shadows factory `foo` (consistent with load-path semantics), but
   projects that referenced factory-`foo` silently change sound. Probably
   want provenance recorded in the project (factory vs user vs
   package-qualified id).
4. **Naming**: `assets/` vs `content/` for the repo dir; `~/.eseq.d` is
   locked by the module spec.
