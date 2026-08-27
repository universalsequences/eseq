# Embedded DGen Connector — Implementation Spec

Status: ready to implement
Parent: `embedded-dgen-toolchain-v0.1-spec.md` (the v0.1 product spec)
Scope: the "initial version" milestone — ESeq compiles and plays DGen
instruments/effects through the embedded clang/lld toolchain with no Xcode or
Command Line Tools required by the production compile path, from a dev build.
Signing, notarization, and `.app` bundle layout (parent spec Phase 5) and the
single-compilation-service refactor (parent Phase 4) are explicitly out of
scope here; this spec ends where a dev-layout hermetic compile works end to
end.

## 1. Baseline (verified 2026-08-10)

### dgen repository (`~/code/swift/dgen`) — Phases 1+2 complete

- Hermetic toolchain: `scripts/build-toolchain.sh` builds pinned LLVM 20.1.8
  (clang, ld64.lld, compiler-rt builtins, AArch64 only), stages
  `bin/dgen-clang`, `bin/ld64.lld`, clang resource headers,
  `include/dgen_runtime.h`, `lib/libSystem.tbd` (DGen-authored stub),
  `empty-sdk/`, and writes `toolchain/VERSION.json`
  (`distribution_version 2`, `dgen_abi_version 1`, llvm 20.1.8) plus
  `toolchain/SIZE.txt` (~47 MB compressed / ~147 MB installed). Local
  `.toolchain/stage/` is empty; the built archive
  `.toolchain/dgen-toolchain-20.1.8-arm64.tar.gz` must be untarred to use it.
- Closed ABI v1 (`toolchain/include/dgen_runtime.h`):

  ```c
  void dgen_process_v1(const float * const *in, float * const *out,
                       uint32_t nframes, void *state,
                       const DGenProcessContextV1 *context,
                       const DGenHostServicesV1 *host);
  void dgen_set_param_value_v1(int32_t cell_id, float value); /* emitted as a
                       no-op stub today; hosts write state memory directly */
  ```

  `DGenProcessContextV1 { abi_version, struct_size, sample_rate, reserved }`.
  `DGenHostServicesV1 { abi_version, struct_size, fft_setup_create_fn,
  fft_forward_fn, fft_inverse_fn, complex_multiply_accumulate_fn }` — exactly
  four block-level callbacks. Generated C casts `state` to a **single**
  `float *memory` span (`CRenderer.swift:352-373`); the old split
  read/write-buffer pair no longer exists. Sample rate is read from
  `context`, guarded by `abi_version`/`struct_size` checks.
- 4-lane vecLib math (`vsinf`/`vcosf`/`vtanhf`/`vexpf`/`vlogf`) is inline NEON
  polynomials in `dgen_runtime.h:97-206`; `vtanf`/`vatanf`/`vsqrtf`/
  `vatan2f`/`vpowf` are lane-wise scalar libm macros. Generated C includes
  only `dgen_runtime.h`; no `-framework Accelerate` on the link line; sole
  load dependency is `/usr/lib/libSystem.B.dylib`.
- Compile policy: `Sources/DGen/DGenToolchainPolicy.swift` — env
  `DGEN_TOOLCHAIN_STAGE_ROOT` set → embedded invocation (preflight-checks each
  staged file); unset → `/usr/bin/clang` dev fallback. `policySignature`
  feeds dgen's own cache key.
- Audit: `scripts/audit-dgen-dylib.sh` enforces arch/minos/export allowlist
  (`toolchain/abi/exports-v1.txt`)/undefined allowlist
  (`toolchain/abi/libsystem-symbols-v1.txt`, 23 symbols)/load-command and
  path hygiene — but it shells out to `file`/`nm`/`otool`/`strings`, i.e.
  requires Command Line Tools.
- Reference host: `Sources/DGenHostSupport/DGenHostSupport.c` implements the
  four callbacks over Accelerate (`vDSP_create_fftsetup`, `vDSP_fft_zip`
  fwd/inv, `vDSP_zvma`); `toolchain/harness/toolchain_harness.c` is the
  standalone C equivalent. Proof: `scripts/prove-toolchain.sh`,
  `docs/toolchain-hermetic-proof.md` (max abs error ≤ 9.7e-08 vs 2e-5
  tolerance across four fixture classes).

### ESeq — still on the pre-v1 ABI, workspace-path-bound

- FFI (`crates/sequencer/src/lisp_host/dgen/dgen_ffi.rs`):
  `DGenProcessFn(inputs, outputs, frame_count: c_int, memory_read,
  memory_write, host_sample_rate)` (line 46); state layout = 10 header slots
  (enabled, sample rate, canary, fn-pointer smuggled through f32 slots 6..10)
  + **two** buffer spans with redzones (`dgen_total_state_slots`, line 92).
  The process call happens in Rust — `dgenlisp_wrapper_process` (line 130)
  is the `NodeVTable.process` hook — so the connector is a Rust-side change,
  not an audiograph C change.
- Tool resolution: `effect_compile.rs:250` and `effects/conv_reverb.rs:405`
  both do `current_dir().join("tools/DGenLisp")`, valid only because every
  entry point calls `paths::enter_sequencer_dir()` first. The committed
  `tools/DGenLisp` binary (3.1 MB, tracked in git) is from Jul 6 —
  **pre-Phase-2**; dylibs it produces still link Accelerate and carry stale
  absolute staging install-names.
- Cache (`dylib_cache.rs`): content-addressed
  (`CACHE_SCHEMA_VERSION = 1`, key = schema + kind + sampleRate + voices +
  effective-source sha + tool-binary sha + asset fingerprints) at
  `workspace_root()/.eseq/dgenlisp-cache/`; atomic staging→rename publication
  exists (line 237-292); metadata revalidation on read exists (line 468-506).
  Missing: ABI/toolchain/target/policy in the key, per-key compile lock
  (racing threads double-compile and double-publish), refcounted leases
  (`try_lease_artifact:338` is exclusive despite the doc comment), staging
  sweep, eviction (cache is 515 MB in this checkout).
- No `AppPaths` anywhere; `paths.rs` is a cargo-workspace locator only.

## 2. Design decisions (resolved)

1. **Toolchain hand-off is per-subprocess-invocation.** ESeq stages the dgen
   toolchain on disk and passes its root to the `DGenLisp` subprocess. dgen
   grows an explicit `--toolchain-root <dir>` CLI flag (host-selected root
   per the parent spec's Compiler Invocation Contract); the existing
   `DGEN_TOOLCHAIN_STAGE_ROOT` env var remains as the dev fallback. ESeq
   always passes the flag; it never relies on the subprocess inheriting env.
2. **Host services live in ESeq as a C file ported from dgen's reference
   implementation.** New `crates/sequencer/audiograph/dgen_host_services.c`
   (+ header) is a near-verbatim port of dgen's
   `Sources/DGenHostSupport/DGenHostSupport.c`, compiled by `build.rs`, with
   `-framework Accelerate` added to the app link (the *app* may link
   Accelerate; only generated dylibs may not). It exposes
   `const DGenHostServicesV1 *eseq_dgen_host_services_v1(void)` returning a
   process-lifetime static. The struct definition comes from a vendored copy
   of `dgen_runtime.h`'s ABI section (see decision 6).
3. **State layout v2: single memory span.** Header slots 0–9 keep their
   meaning (slot identity, total_memory_slots, canary, input count, enabled,
   host sample rate, fn-pointer chunks 6..10); the write span is deleted.
   `dgen_total_state_slots` becomes `HEADER_SLOTS + total_memory_slots +
   DGEN_STATE_REDZONE_SLOTS`. `dgenlisp_wrapper_process` builds a
   `DGenProcessContextV1` on the stack from state slot 5 per call (4 fields,
   negligible), fetches the static host-services pointer, and calls the v1
   signature. Param and tensor writes keep landing at
   `HEADER_SLOTS + cell_offset` — unchanged mechanism, since
   `dgen_set_param_value_v1` is a stub by design.
4. **Binary audit is reimplemented in Rust.** New
   `lisp_host/dgen/dgen_audit.rs` using the `object` crate parses the
   Mach-O directly: arm64 dylib filetype, `LC_BUILD_VERSION` minos 11.0,
   exported symbols exactly `{_dgen_process_v1, _dgen_set_param_value_v1}`,
   undefined symbols ⊆ the libSystem allowlist, load commands exactly
   `/usr/lib/libSystem.B.dylib` (+ own `LC_ID_DYLIB`), no `LC_RPATH`, no
   developer/workspace/tmp/user path strings, file-size limit. The two
   allowlist files are vendored from `toolchain/abi/` alongside the staged
   toolchain and read at audit time (not compiled in), so a toolchain update
   updates the audit contract atomically. dgen's shell-script audit stays as
   the cross-check in dgen's own proof harness; ESeq's production path never
   shells out to `nm`/`otool`.
5. **`AppPaths` is a small module, dev-mode first.** It answers: DGenLisp
   helper path, toolchain root, allowlist dir, effects/instruments source
   roots, cache root, scratch/staging root. In dev mode (this milestone) it
   resolves against the workspace exactly as today; the release arm
   (`Contents/MacOS`, `Application Support`, `Caches`) is stubbed with the
   final shapes but not exercised until Phase 5. All call sites go through
   it — the two `current_dir().join("tools/DGenLisp")` duplicates, the
   `EFFECTS_DIR`/`INSTRUMENTS_DIR` constants, `output_dir()`/`ir_prep_dir()`
   temp dirs, and the `current_dir()` fallback in
   `fingerprint_source_assets`.
6. **The ABI header is vendored, hash-pinned.** ESeq vendors the ABI-struct
   subset of `dgen_runtime.h` (context + host-services structs + the two
   export prototypes) as `audiograph/dgen_abi_v1.h`, with the source header's
   sha256 recorded in a comment and checked by a test against the staged
   toolchain's `include/dgen_runtime.h` ABI section. ESeq consumes the ABI;
   it does not redefine it (parent spec, Repository Ownership).
7. **Cache schema bumps to 2 with no migration.** Pre-ABI artifacts are
   abandoned per the parent spec's non-goal; the old `dylibs/` tier is simply
   ignored (and may be deleted by the sweep). New layout inserts the tier
   `<schema>/arm64-apple-macos/` between the cache root and `dylibs/`.
8. **Install-name is fixed at link time, in dgen.** DONE (D1, branch
   `toolchain-root-flag`): the `@rpath/<module>.dylib` install-name flags
   already existed in the policy; the actual gap was enforcement, and the
   dgen audit script now rejects absolute-path install names. ESeq's Rust
   audit (E5) must enforce the same rule.

## 3. Work slices

Each slice is independently landable and testable. D-slices are in the dgen
repo; E-slices in ESeq. Order: D1 → D2 → (E1 ∥ E5) → E2 → E3 → E4 → E6 → E7.
E3 and E4 are one functional unit (nothing plays until both land) but review
separately.

### D1 — dgen: explicit toolchain root + policy hygiene

- Add `--toolchain-root <dir>` to `Sources/DGenLisp/main.swift`; thread into
  `DGenToolchainPolicy.compileInvocation` as a parameter. Env var remains
  fallback; flag wins.
- Add the install-name link flag (decision 8) to both embedded and dev
  invocations.
- Replace the `/usr/bin/clang --version` cache-signature probe
  (`Runtime.swift:177`) with the staged `VERSION.json` content hash; in the
  embedded path the system clang must not be consulted for anything.
- Fix stale "requires Accelerate" wording in
  `Compilation/CompilationPipeline.swift:265-274`.

Exit: `DGenLisp compile --toolchain-root <staged> …` produces an audited
dylib with `env -i` (no PATH, no DEVELOPER_DIR); its install name is not an
absolute path; `otool -L` shows only libSystem.

### D2 — dgen: staged distribution is self-describing

- Ensure the toolchain archive includes `VERSION.json`, `SIZE.txt`,
  `abi/exports-v1.txt`, `abi/libsystem-symbols-v1.txt`, and
  `include/dgen_runtime.h` at stable relative paths (they exist in
  `toolchain/`; verify `build-toolchain.sh` stages all of them into the
  archive, add any missing).
- Document the staged layout in `docs/toolchain-hermetic-proof.md` or a new
  `toolchain/LAYOUT.md` as the contract ESeq consumes.

Exit: untarring the archive yields everything ESeq's E2/E5 slices read, with
no reference back into the dgen source tree.

### E1 — ESeq: `AppPaths`

- New `crates/sequencer/src/app_paths.rs` (module per repo style) with the
  query surface from decision 5, constructed once at startup and threaded
  where practical; a process-wide accessor is acceptable for this milestone
  given ~30 call sites, but the constructor must be explicit (no lazy
  `current_dir()` capture).
- Replace `dgenlisp_tool_path()` (`effect_compile.rs:250`),
  `conv_reverb.rs:405` `tool_path()`, `output_dir()`, `ir_prep_dir()`, the
  `EFFECTS_DIR`/`INSTRUMENTS_DIR` constants' resolution, and the
  `fingerprint_source_assets` cwd fallback.
- `enter_sequencer_dir()` stays for now (other subsystems depend on cwd),
  but nothing in the dgen compile path may depend on cwd afterward — enforce
  with a test that compiles from a foreign cwd.

Exit: with cwd set to `/`, an instrument compile via the cache manager
succeeds using only `AppPaths`-resolved locations.

### E2 — ESeq: vendor the new toolchain + DGenLisp

- Extend `rebuild_dgenlisp_tool.sh` to also stage the toolchain: untar the
  dgen archive into `crates/sequencer/tools/dgen-toolchain/` (gitignored —
  147 MB does not belong in git; the script records the archive sha256 it
  staged into a small committed `content/dgen-toolchain.lock` file and fails
  when they diverge).
- Rebuild `tools/DGenLisp` from dgen main (post-D1) and commit it as today.
- ESeq's subprocess spawn (`effect_compile.rs:415-455`) adds
  `--toolchain-root <AppPaths.toolchain_root()>` unconditionally. There is
  no fallback to system clang from ESeq: a missing/incomplete staged
  toolchain is a hard, user-visible compile error (parent spec, Locked
  Principle 1). Same change in `conv_reverb.rs` partition-ir spawn and
  `bin/instrument_probe.rs`.

Exit: fresh clone + `rebuild_dgenlisp_tool.sh` + probe run compiles through
the embedded toolchain; renaming `/usr/bin/clang` away (or `env -i` spawn
test) does not break it.

### E3 — ESeq: ABI v1 migration

All in `lisp_host/dgen/`, coordinated single change:

- `dgen_ffi.rs`: `DGenProcessFn` becomes the v1 signature; add
  `#[repr(C)] DGenProcessContextV1` and `DGenHostServicesV1` bindings
  matching `dgen_abi_v1.h`; single-span state layout (decision 3);
  `dgenlisp_wrapper_process` builds context from slot 5 and passes
  `eseq_dgen_host_services_v1()`; `dgenlisp_init` drops the write-span copy;
  delete `dgen_write_buffer_ptr` and the second span from
  `dgen_total_state_slots`.
- `dgen_manifest.rs`: require `processAbi` to name the v1 ABI; `load_dylib`
  resolves `dgen_process_v1` (keep resolving `dgen_set_param_value_v1` so
  the export audit and a future non-stub implementation stay honest).
- `instrument_compile.rs` offline render and `bin/instrument_probe.rs`
  updated to the same call shape.
- Grep-audit every use of `dgen_write_buffer_ptr` / `memory_write` /
  `dgen_buffer_span_slots` and the initial-state message builder for span
  assumptions.

Exit: unit tests on state layout math; a compiled v1 instrument produces
nonzero, finite, deterministic audio through the probe.

### E4 — ESeq: host-services connector

- `audiograph/dgen_abi_v1.h` (vendored ABI, decision 6) and
  `audiograph/dgen_host_services.c` (port of dgen's `DGenHostSupport.c`);
  `build.rs` compiles it and links `-framework Accelerate` **on Apple targets
  only**. Off Apple the same four callbacks come from the portable
  `audiograph/dgen_fft.c` (eseq-linux.9), which reproduces `vDSP_fft_zip`'s
  unscaled both-directions convention and natural bin ordering and
  `vDSP_zvma`'s non-conjugated product. The tests in
  `lisp_host/dgen/dgen_host_services_tests.rs` pin whichever backend the
  platform ships against a naive double-precision DFT, so running them on both
  hosts is what makes the two backends comparable. The table layout is
  untouched, so already-generated dylibs stay valid.
- FFT setup creation allocates; the table's `fft_setup_create_fn` is called
  lazily from generated code inside the audio callback on first use. Match
  dgen's reference behavior for this milestone, but pre-warm where possible:
  after manifest load, if the manifest declares spectral ops, invoke the
  setup path once from the control thread during audition so steady-state
  callbacks never allocate. Record this as the same accepted risk dgen's
  reference host carries.

Exit: the spectral fixture class (port one of dgen's
`toolchain/fixtures/*.lisp`, e.g. the spectral effect) renders through the
ESeq probe with output matching the dgen reference harness within 2e-5.

### E5 — ESeq: Rust binary audit

- `dgen_audit.rs` per decision 4 (`object` crate dev-dependency → real
  dependency). Runs inside `compile_new_artifact` after subprocess exit,
  before manifest load; failure = compile failure, staging removed, nothing
  published.
- **Companion dgen change (found during D1 verification):** DGenLisp runs
  its shell-script audit inline on every compile (`Compiler.swift:85`),
  which shells out to `nm`/`otool`/`file`/`strings` — a Command Line Tools
  dependency in the production path — and `DGenBinaryAudit` locates the
  script via `#filePath` (a build-machine path; fails on any other
  machine). Add a `--skip-inline-audit` flag (or make the audit conditional
  on an explicit `--audit-tool <path>`), re-vendor DGenLisp, and have ESeq
  pass the skip flag; ESeq's Rust audit replaces it on the exact published
  bytes. The dgen script remains dev-time proof tooling only.
- Test fixtures: a good dylib (from E2), plus deliberately bad ones —
  x86_64 build, extra export, Accelerate load command (the old committed
  tool produces these for free), absolute install name.

Exit: audit passes current-good and rejects each bad fixture with a distinct
structured error.

### E6 — ESeq: cache identity + concurrency

- `CACHE_SCHEMA_VERSION = 2`; insert the `<schema>/arm64-apple-macos/` path
  tier; extend key material with: staged `VERSION.json` sha256 (covers
  distribution/ABI/policy/llvm), target triple, minimum macOS, and the
  vendored ABI header hash. Tool-binary sha stays.
- Per-key compile lock: a `Mutex`-guarded in-flight map (key → `Condvar`/
  `Arc<Once>`-style latch) so the second requester waits and leases the
  first's artifact instead of double-compiling.
- Make leases refcounted as the module doc already claims: increment on
  repeat `try_lease_artifact` hits instead of returning `None`.
- Startup sweep: delete staging dirs with no live lease; ignore/quarantine
  metadata that fails to parse; delete schema-1 tier opportunistically.

Exit: unit tests — key changes for each new input; two threads racing one
key produce one artifact directory and two live leases; sweep removes
orphaned staging but never a leased artifact.

### E7 — Integration proof + factory revalidation

- Integration test (ignored-by-default, run explicitly like the toolchain
  proofs): compile a representative instrument and effect with the
  subprocess spawned under a neutered environment (empty `PATH`,
  `DEVELOPER_DIR=`), audit, load, render, assert finite nonzero audio.
- Run every factory/curated DGen instrument and effect through the embedded
  path via the existing probe harness (parent spec "Instrument validation");
  fix or flag regressions. This is the step most likely to surface real
  codegen gaps (spectral instruments exercising all four host services,
  polyphonic voice memory, tensor init data).
- Update `AGENTS.md`/`crates/sequencer/README.md` compile-workflow notes.

Exit: parent-spec acceptance criteria 2, 3, 4, 5, 10 hold in the dev
layout; criteria 1, 8 (clean machine, signed bundle) remain open for
Phase 5.

## 4. Explicitly deferred (tracked, not in this milestone)

- `.app` bundle layout, signing, entitlements, notarization, clean-machine
  acceptance (parent Phase 5). The `AppPaths` release arm exists but is
  unexercised.
- Single compilation service unifying the ~30 `compile_and_load*` call
  sites, structured diagnostics for agent retries, centralized cancellation
  (parent Phase 4). The generation-counter cancellation in
  `ui/edit_sessions.rs` stays as-is.
- Cache eviction/GC policy (the sweep only handles staging + old schema).
- ABI-mismatch *reporting* from inside generated code (guards currently
  no-op safely); needs a dgen-side channel, tracked there.
- x86_64 / universal anything.

## 5. Risks and open questions

1. **The old ABI's write-span existed for `restrict` aliasing.** v1 generated
   code manages aliasing internally with one span. There is no host-write
   race to worry about: all ESeq param/tensor/data writes go through the
   audiograph-owned queue, which applies them at the block boundary before
   the block is processed — never mid-block. What remains is mechanical:
   re-point the helpers (`queue_tensor_write`, param writes, the
   initial-state sparse-pair applier) at the single span's offsets and
   verify with the p-lock/param-sweep probe cases.
2. **`dgen_set_param_value_v1` is a stub.** ESeq's direct memory writes are
   the real param path; if dgen later makes the export functional the two
   mechanisms could fight. Record in the vendored header comment; revisit at
   ABI v2.
3. **Voice memory layout.** ESeq compiles instruments with `--voices 12`;
   confirm the v1 single-span layout's voice stride matches what
   `instrument_compile.rs` allocates (manifest `totalMemorySlots` semantics
   under voices) — the surveys did not verify this seam.
4. **FFT-setup allocation on the audio thread** (E4) — accepted, mitigated
   by pre-warm, must be listed in release notes per the parent spec's
   realtime requirements if any path can still hit it live.
5. **147 MB staged toolchain in every checkout/worktree.** Acceptable for
   dev; Phase 5 decides the distribution story. Worktree users: the stage
   dir is per-checkout; consider honoring `ESEQ_DGEN_TOOLCHAIN_ROOT` to
   share one stage across worktrees (dev-only override, per parent spec).
6. **`object`-crate audit fidelity vs `otool`.** Cross-check both audits on
   the same fixtures in E5's tests (dgen's script remains available in the
   dgen repo) before trusting the Rust audit alone.
7. **`VERSION.json` bookkeeping (from D2):** `dgen_compiler_version` is
   written as `git rev-parse HEAD` by the build script but the shipped file
   says `phase2-abi-v1` — a transient sha in a cache-key input churns the
   key on every dgen commit. Decide on a stable, intentionally-bumped
   version string before E6 wires `VERSION.json`'s hash into ESeq's cache
   key; also decide whether `distribution_version` bumps when the staged
   layout gains files (it did in D2 without a bump).

## 6. Acceptance for this spec

From a fresh checkout on a machine where the production compile path cannot
reach `/usr/bin/clang` (spawn-time `env -i` in tests), with the staged
toolchain present:

1. Instrument and effect compiles go through `tools/DGenLisp
   --toolchain-root …` and the embedded clang/lld only.
2. Published dylibs pass the Rust audit: arm64, v1 exports only, libSystem
   as the only load dependency, allowlisted undefineds, no absolute paths.
3. Audio renders correctly through the v1 ABI with ESeq's Accelerate-backed
   `DGenHostServicesV1`, including a spectral fixture within 2e-5 of the
   dgen reference harness.
4. Cache: schema-2 tier, toolchain fingerprint in the key, no same-key
   double-compiles, refcounted leases, staging sweep.
5. Every curated factory DGen instrument/effect passes probe validation on
   the embedded path.
