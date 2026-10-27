# Package native tools — packages that ship executables

Rev 1 (2026-10-26). Extends `docs/attachable-packages-spec.md` and
`docs/content-tiers-spec.md`. First consumer: the modal/finite-element
solver now baked into the sequencer crate.

## 1. Problem

The modal work put about 5,000 lines of special-purpose Rust into the app:
`crates/sequencer/src/modal_fe/` (finite-element mesh, element matrices,
skyline storage, eigensolver; about 3,200 lines), the five `modal-*` natives in
`lisp_host/eseq/modal_patch.rs` (1,228 lines including tests), and the
shape-lab worker in `ui/host_commands/shape_lab.rs` (631 lines). The Lisp
half (`content/ui/modal.lisp`, `content/ui/modal/*.lisp`) is already a
package in all but name. The only reason the rest is in-tree is that the solver
has to be native to be fast.

Every future experiment of this shape (a solver, an analyser, a model
trainer) would grow the core the same way. The goal is the opposite: a large
package ecosystem with a small core. The package system already handles the Lisp,
content, samples, kinds and namespacing. What it lacks is any way for a package
to run native code.

## 2. Decision

**A package may ship native executables ("tools") and its Lisp calls them
as subprocesses: JSON request on stdin, JSON lines on stdout.** This is the
Emacs model (`call-process` / `make-process`, the way Emacs talks to LSP
servers), not a plugin ABI.

Why a subprocess and not the alternatives:

- **dylib plugins**: Rust has no stable ABI, and a crashing solver takes the app
  (and audio) down with it. A tool that crashes only returns an error string.
- **WASM**: sandboxed and portable, so it stays on the table for untrusted
  code (§9). But it brings in a runtime dependency, threads and SIMD are
  awkward for an eigensolver, and the JSON contract below would carry over
  unchanged if we ever add a WASM backend.

**Scope limit.** Tools are for *offline* work: computing data that Lisp then
turns into an artifact (dgenlisp source, a mode table, a preset). Nothing
that runs every audio block or every UI frame goes through a tool. DSP
already has its extension path (generated dgenlisp). Spawning a process
costs a few milliseconds and an FE solve takes seconds, so the overhead
doesn't matter for this class of work.

The draft → compile → artifact pipeline, the generator machinery, and the
generic widgets (`mesh_widget.rs`) stay in Rust. They are not modal-specific.

## 3. Verified facts (2026-10-26, this machine, macOS 26.5 / Darwin 25.5)

| Fact | Evidence |
|---|---|
| The release app is not sandboxed and does not set `LSFileQuarantineEnabled`. | `dist/macos/build.sh` writes the Info.plist with neither key. `dist/macos/entitlements.plist` grants only `com.apple.security.cs.disable-library-validation`. |
| The release build signs nested executables one by one from an allowlist and **fails on any unlisted executable**. | `build.sh` `HELPERS` array plus the "unlisted executable in bundle needs a signing decision" check. |
| Hardened runtime is on, with library validation already disabled for the generated DGen dylibs. | Same file. So "subprocess avoids library validation" is *not* a reason here. The reasons are crash isolation and ABI. |
| A file written by a process that doesn't quarantine its output has no `com.apple.quarantine`. | A binary built with `cc` in a scratch dir carried only `com.apple.provenance`. The same is expected for a reqwest download from eseq (not tested directly). |
| Extracting a quarantined `.zip` spreads quarantine to every extracted file, with **both** `ditto -x -k` and `unzip`. | Archive tagged `0083;…;Safari;<uuid>`, both extractors produced `0283;…;;<same uuid>` on `bin/solver`. `package_install::extract_archive` uses exactly these two. |
| Copying a quarantined file keeps the quarantine (`cp`, `shutil.copy2`). | Same test. Rust `std::fs::copy` on macOS uses `fclonefileat`/`fcopyfile`, which copy xattrs, so `copy_dir_recursive` keeps it too (inferred, not tested). |
| `git clone` produces files with no quarantine. | git isn't quarantine-aware. `stage_git_package` is safe. |
| **Running a quarantined, ad-hoc-signed binary from a child process hangs** (Gatekeeper assessment / dialog). It doesn't fail cleanly. | `qdir/bin/solver` didn't return within 120 s and was killed. |
| The solver already exists as a CLI. | `eseq modal solve SPEC.json OUT.modes.json` (`bin/eseq.rs:64`). |
| eseqlisp has no general JSON parse/encode natives. | Only domain natives (`json-sparse`, `json-project-mask`). `modal_patch.rs` has private `json_to_lisp`/`lisp_to_json`. |

## 4. Manifest: declaring tools

`manifest.json` gains a `tools` list. A tool's identity is its name inside
the package. Tools are not global: two packages can each ship a `solve`.

```json
{
  "name": "universalsequences/modal",
  "version": "0.1.0",
  "entry": "modal",
  "tools": [
    {
      "name": "solve",
      "protocol": 1,
      "targets": {
        "arm64-apple-macos":        { "path": "bin/arm64-apple-macos/modal-solve",
                                      "sha256": "…" },
        "x86_64-unknown-linux-gnu": { "url": "https://…/modal-solve-0.1.0-x86_64-linux.tar.zst",
                                      "member": "modal-solve",
                                      "sha256": "…" }
      }
    }
  ]
}
```

- Each target either has a `path` inside the package or a `url` (plus an
  optional archive `member`) that is fetched at install. `sha256` always
  covers the executable itself, not the archive. These are the same target
  names and the same per-target url+sha256 shape as `content/dgenlisp.lock`
  and `content/dgen-toolchain.lock`.
- `protocol` is the stdin/stdout contract version (§5.2). The host refuses
  a tool whose protocol it doesn't speak.
- A missing target for the running platform is not an install error. The
  package installs, and calling the tool fails with "`solve` has no build
  for x86_64-unknown-linux-gnu".
- **Executables a package doesn't declare are never run.** Installation
  clears the exec bit on every file that isn't a declared tool for the
  running platform. This is the package-side counterpart of `build.sh`'s
  allowlist.
- Verification reuses the existing `verify_asset` path in
  `eseqlisp/src/package.rs`. `ExternalAsset` is the precedent for "a file
  the manifest pins by hash".

## 5. Lisp surface

### 5.1 Natives

Named "tool", not "process", because *process* already means the Max-style
process lanes (`docs/process-channels-spec.md`).

```lisp
;; Synchronous: blocks the caller, returns the result value or raises.
(tool-call 'solve request)

;; Asynchronous: returns a handle at once.
(tool-start 'solve request
  :on-progress (fn (p) …)      ; each {"progress": …} line
  :on-done     (fn (result) …) ; the {"result": …} line
  :on-error    (fn (message) …))
(tool-cancel handle)            ; SIGTERM, then SIGKILL after 2 s

(json-parse "…") (json-encode value) ; general, independent of tools
```

- **Resolution is lexical by package.** `'solve` resolves against the
  manifest of the package that owns the *calling module*. Another package
  that wants the solver imports `universalsequences.modal` and calls its Lisp
  API. It can't name another package's tools directly, so every tool sits
  behind its owner's Lisp interface.
- `request` is any Lisp value that converts to JSON (dict → object, list
  → array, keyword → string). The conversion goes in one shared place: the
  current private `lisp_to_json`/`json_to_lisp` in `modal_patch.rs`, made
  public as the `json-*` natives.
- **Runtimes.** Tool natives are registered on the UI runtime and on the
  headless `eseq` VM (`eseq generate`, `eseq instrument check`). They are
  **not** registered on the scheduler or midi-fx runtimes. Those replay
  scratch on every version bump and must never block on, or start,
  processes. Calling a tool there is an unknown-function error, which is
  the clearest way to fail.
- `tool-call` on the UI runtime blocks the UI exactly as `modal-solve`
  does today (the first evaluation of a solid pays for the solve). That isn't
  a regression, but long work should use `tool-start`. The shape lab does
  (§8).
- `tool-start` callbacks run on the UI runtime from the event loop, drained
  like the shape lab's pending solve today. A reader thread per child owns
  its stdout.

### 5.2 Wire protocol (v1)

- argv is **fixed**: the executable path plus `--eseq-tool-protocol 1`.
  There is no shell, and no request data in argv, so there's nothing to
  inject into.
- stdin: one JSON document (the request), then EOF.
- stdout: newline-delimited JSON objects: any number of
  `{"progress": <any>}` lines, then exactly one of `{"result": <any>}` or
  `{"error": "<message>"}`. Anything else is a protocol error naming the
  line.
- stderr: kept (last 64 KiB) and attached to any error, so a crash shows
  its panic message.
- Exit status: non-zero with no `result` line means failure, with stderr as
  the message.
- Environment: a cleared environment plus `PATH`, `HOME`, `TMPDIR`, and
  `ESEQ_TOOL_CACHE`, a per-package cache directory under
  `~/Library/Caches/com.universalsequences.eseq/tools/<author>/<name>/`
  (the Linux equivalent under `$XDG_CACHE_HOME`). The tool owns its cache.
  For the solver, this is where the spec-hash-keyed solve cache lives
  (`solve_solid_cached` today).
- Working directory: the package root, read-only by convention. Paths in
  the request (`.obj` meshes) are absolute, resolved by the Lisp side
  against the calling script's directory, as `solid_spec` does now.

A long-running server mode (JSON-RPC over stdio, as with LSP, to keep a mesh
warm between solves) is deliberately out of v1. `agent/harness/mcp_client.rs`
is the in-tree precedent if it's ever needed. Bump `protocol` to add it.

## 6. Trust

Installing a package used to mean installing Lisp that runs inside our VM.
With tools, it means running arbitrary native code with the user's
privileges. That has to be an explicit, visible decision.

- **Consent is per (package identity, tool name, sha256)**, stored in
  user data (`tool-consent.json`). Changing the hash (a package update)
  asks again. Removing the package forgets the consent.
- **When:** the install confirmation modal (`PackageSummary`) gains a
  line "Runs native programs: `solve` (arm64-apple-macos, sha256 …)".
  Confirming the install grants consent. A tool first seen any other way,
  such as a package copied into the packages directory by hand, prompts on
  its first call. `tool-call` then fails with "waiting for permission"
  instead of blocking the UI on a modal.
- **Tiers.** Factory-tier tools, bundled inside the signed app, are trusted
  implicitly. The manifest-free local workspace (`~/.eseq.d/packages/local`)
  trusts by *path*, not hash, since the hash changes on every rebuild
  during development. Installed user-tier packages always go by hash.
- **Headless.** `eseq` CLI commands need consent already recorded, or
  `--allow-tools <package>` for that one invocation. They never prompt.
- **Honest limit.** The manifest's sha256 comes from the same archive as
  the binary, so it proves integrity (the bytes are what the manifest says)
  but not authenticity. Authenticity comes from the source: the package
  catalog, or a git remote the user chose. A signed catalog carrying the
  hashes would close that gap and is future work (§9).

## 7. macOS: Gatekeeper, quarantine, notarization

### 7.1 Three install routes

| Route | Quarantined? | What the binary needs |
|---|---|---|
| **Bundled in `ESeq.app`** (factory tier) | No (stapled app) | Developer ID signature with hardened runtime, no entitlements: add it to `build.sh` `HELPERS`. It's notarized as part of the app. |
| **In-app install**: git clone, catalog download, or a manifest `url` fetched with reqwest | No (§3) | The arm64 ad-hoc signature the linker already writes. No Developer ID or notarization required. |
| **Sideload**: the user downloads a `.zip`/`.eseqpack` in a browser and imports it | **Yes**: spreads through `ditto`/`unzip` (§3) | Without handling, spawning hangs. See §7.2. |

The middle route is how third-party authors publish without a $99/yr Apple
Developer account. It's the Homebrew model, with eseq's hash pin and consent
prompt doing Gatekeeper's job for package tools. **Don't add
`LSFileQuarantineEnabled` to the Info.plist**: that would quietly make
every third-party tool require notarization.

### 7.2 Quarantine handling (required, not optional)

1. **Never spawn a quarantined tool.** Before every spawn, the host checks
   `com.apple.quarantine` on the resolved executable. If it's present, the
   call fails at once with "`solve` is quarantined. Reinstall the package or
   approve it in the Packages tab", instead of hanging on Gatekeeper (§3).
2. **On sideload install**, after verifying sha256 and recording consent,
   the installer removes `com.apple.quarantine` from **exactly the declared
   tool files** for the running platform. It does not touch the whole package,
   and it never touches undeclared files (which have no exec bit anyway, §4).
   This is a deliberate security decision: consent plus the hash pin is our
   replacement for Gatekeeper's check on these files, and it's the same
   trade Homebrew makes.
3. A tool that *is* Developer ID signed and notarized works quarantined or
   not, so authors who notarize lose nothing. Notarizing a bare Mach-O
   can't be stapled (tickets staple only to `.app`/`.pkg`/`.dmg`), so it
   relies on Gatekeeper's online check. That's another reason we don't depend
   on Gatekeeper for tools.

### 7.3 Our own packages

- First-party packages get Developer ID signing and notarization anyway
  (cheap, the pipeline already exists in `build.sh`), so they also survive
  if someone moves them outside eseq.
- The website should link to the in-app installer (`eseq://install?pkg=…`
  or "download the `.eseqpack`, then drag it onto eseq") rather than offer
  raw binaries. That keeps almost everyone on the unquarantined route.
- `com.apple.provenance` is on every file the test produced. We haven't
  seen it block a launch, and nothing here relies on it either way.

### 7.4 Linux

No quarantine. The exec bit plus sha256 plus consent is everything.

## 8. Modal migration (first consumer)

Moves into a new `universalsequences/modal` package (its own repo, with its own
CI building the per-target binaries):

| Today | Becomes |
|---|---|
| `modal_fe/*` (mesh, element, skyline, eigen, spec, obj) | the `modal-solve` binary: a small crate in the package repo. `bin/eseq.rs`'s `eseq modal solve` is its starting point. |
| `modal-solve`, `modal-mesh`, `modal-circ-plate-modes` natives | Lisp functions in the package over `(tool-call 'solve …)`, with the request `op` one of `solve` / `mesh` / `circ-plate`. |
| `modal-read-data`, `modal-nearest-point` | `modal-nearest-point` is a few lines of Lisp. `modal-read-data` is `json-parse` of a file plus the unit-modal-mass scaling, in Lisp or in the tool. |
| `solve_solid_cached` | the tool's own cache in `ESEQ_TOOL_CACHE`. |
| `shape_lab.rs` worker thread, cancel, progress (`SolveProgress`) | `tool-start` / `tool-cancel` / `:on-progress`. The protocol carries progress as `{"progress": {"phase": …, "done": …}}`. |
| `shape_lab.rs` `shape-lab-apply` source rewrite | open question (§10.1) |
| `content/ui/modal.lisp`, `content/ui/modal/*.lisp` | the package's `src/` |
| `mesh_widget.rs` | **stays.** A generic 3-D mesh widget any package can use. |
| draft-save / generator / artifact flow | **stays.** |

Until the package is installable through the catalog, the modal package
can live as a bundled factory-tier package. The tool binary is then just
one more `HELPERS` entry, and nothing in eseq refers to modal by name.

Acceptance: `rg modal_fe crates/` is empty, the existing modal tests move
into the package repo (Lisp-level tests run against the tool), and a modal
instrument generated before and after the migration produces
byte-identical dgenlisp.

## 9. Not in v1

- Long-running server tools (JSON-RPC over stdio). §5.2.
- WASM tools: sandboxed, single artifact. Worth revisiting if the consent
  prompt turns out to be the main obstacle to sharing.
- A signed package catalog, for authenticity rather than just integrity (§6).
- Resource limits (CPU time, memory) beyond cancel.
- Windows.

## 10. Open questions

1. **`shape-lab-apply`** rewrites the arguments of a `(… solid 'id …)` call
   in the script source through the Rust parser (`solid_calls`,
   `rewrite_solid`). Is there enough sexp-editing surface in eseqlisp
   (span-aware read + replace-region) to do this in package Lisp? If not,
   a generic "rewrite the keyword args of the call to F with id X" editor
   native is core-worthy. It isn't modal-specific.
2. **Blocking `tool-call` on the UI runtime**: keep it, matching today's
   `modal-solve`, or require `tool-start` on the UI runtime and allow
   `tool-call` only headless? Keeping it is simpler and no worse than now.
3. **Consent UI** for a tool first seen outside an install (hand-copied
   package): a Packages-tab badge plus an approve action, or a modal on
   first call?
4. Should `ESEQ_TOOL_CACHE` be cleared when the tool's hash changes? The
   solver's cache is keyed by spec hash, so a solver fix wouldn't
   invalidate stale solves unless the tool also keys on its own version.

## 11. Slices

1. **`json-parse` / `json-encode` natives** in eseqlisp, from
   `modal_patch.rs`'s converters. Standalone, useful right away.
2. **Manifest `tools` + install**: parse, verify sha256, fetch `url`
   targets with reqwest, clear exec bits on undeclared files, cache dir.
3. **Tool natives + protocol v1**: `tool-call`, `tool-start`,
   `tool-cancel`, lexical resolution, runtime registration (UI and headless
   only), stderr capture, the quarantine pre-spawn check.
4. **Consent**: `tool-consent.json`, install-modal line, first-call
   prompt, `--allow-tools`, tier rules.
5. **Sideload quarantine strip** (§7.2.2) plus a test that imports a
   quarantined archive and spawns its tool.
6. **`build.sh`**: sign bundled factory-tier package tools (`HELPERS`
   driven by manifests, not hand-listed).
7. **Modal extraction** (§8): tool crate, Lisp wrappers, shape lab on
   `tool-start`, delete `modal_fe/` and the modal natives.
