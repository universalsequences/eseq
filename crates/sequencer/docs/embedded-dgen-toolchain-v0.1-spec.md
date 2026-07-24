# Embedded DGen Native Toolchain v0.1 Spec

Status: accepted direction / implementation not started  
Target release: ESeq v0.1  
Primary platform: Apple Silicon macOS 11 or newer

## Decision

ESeq v0.1 will ship a self-contained native DSP compiler. An agent or user can
author a DGenLisp instrument or effect inside ESeq, compile it to an optimized
Mach-O dynamic library, validate it, and insert it into the live audio graph
without installing Xcode or the Command Line Tools.

The production compilation path is:

```text
DGenLisp source
  -> DGen parser, validator, and DSP lowering
  -> generated, controlled C
  -> bundled upstream Clang
  -> bundled LLVM Mach-O linker (LLD)
  -> cached arm64 Mach-O dylib
  -> manifest and binary validation
  -> offline DSP audition
  -> control-thread publication
  -> audio-thread use
```

This deliberately follows the proven Gen-style model: a constrained DSP
language lowers to C and an embedded Clang/LLVM toolchain produces native DSP.
The system compiler is not part of the runtime contract.

The generated dylib remains the deployment unit in v0.1. An in-process LLVM
JIT is not required for this release.

## Product Requirement

Agent-authored native instruments and effects are a defining ESeq feature,
not an optional developer mode. A release is incomplete if a user can run the
factory library but cannot create a new compiled DGen instrument on a clean,
supported Mac.

The expected user flow is:

1. The user asks the in-app agent for an instrument or effect.
2. The agent supplies DGenLisp DSP and UI sources through the existing
   structured artifact tools.
3. ESeq validates and compiles the DSP away from the audio thread.
4. ESeq auditions the compiled artifact through the production host ABI.
5. On success, ESeq saves the source artifact and activates the compiled DSP.
6. On failure, ESeq keeps the last known-good DSP active and reports a useful
   compiler or validator error to the agent and user.

No terminal, Xcode installation, compiler selection, or manual dylib handling
is visible to the user.

## Goals

- Compile DGenLisp instruments and effects on a clean supported Mac with only
  `ESeq.app` installed.
- Preserve native DSP performance and the current C ABI integration model.
- Keep DGenLisp as the only user- or agent-authored compiler input.
- Produce deterministic, content-addressed, reusable compiled artifacts.
- Keep compilation, linking, validation, and artifact publication off the
  audio thread.
- Make compiler failures recoverable and non-destructive to the current
  project or active audio graph.
- Package, sign, and notarize the app and its compiler helpers correctly.
- Avoid redistributing the Apple Clang toolchain or requiring an Apple SDK at
  runtime.
- Reuse and strengthen the existing `DylibCacheManager`, lease model,
  manifest parsing, and instrument audition path.

## Non-Goals

- Accepting arbitrary C, C++, object files, linker flags, headers, or dynamic
  libraries from users or agents.
- Providing a general-purpose compiler or shell inside ESeq.
- Supporting third-party native plug-in ABIs through the DGen loader.
- Mac App Store distribution in v0.1.
- Intel macOS or universal generated dylibs in v0.1. Supporting x86_64 later
  requires an x86 DGen vector lowering, an x86 LLVM target, and architecture-
  specific cache entries.
- Cross-compiling DGen artifacts for another machine.
- Replacing the dylib pipeline with LLVM ORC JIT in v0.1.
- Preserving compatibility with development cache artifacts created before
  the embedded-toolchain ABI and cache schema.

## Locked Design Principles

1. **The app never depends on `/usr/bin/clang`.** Development may support an
   explicit override, but release builds always use the compiler inside the
   signed app bundle.
2. **DGenLisp is the trust boundary.** The agent cannot provide C, headers,
   compiler arguments, linker arguments, symbol names, or output paths.
3. **Generated C targets a closed DGen host ABI.** It does not target the
   macOS SDK as an application-development environment.
4. **Factory and user sources are durable; dylibs are caches.** Losing the
   compiled cache must only cause recompilation.
5. **Compilation is transactional.** A new artifact becomes visible only
   after compilation, binary inspection, manifest validation, and audition
   succeed.
6. **The last known-good artifact wins.** A failed edit or agent attempt never
   replaces working DSP.
7. **The audio thread never compiles, links, loads, allocates, waits for a
   compiler lock, or parses a manifest.**
8. **Release behavior is tested from a signed app bundle.** A workspace build
   is insufficient evidence for path, signing, or library-validation behavior.

## Bundle Layout

Executable code and non-code resources must occupy their standard macOS bundle
locations:

```text
ESeq.app/
  Contents/
    Info.plist
    MacOS/
      metal_seq
      DGenLisp
      dgen-clang
      ld64.lld
    Resources/
      dgen-toolchain/
        VERSION.json
        LICENSES/
          LLVM-LICENSE.txt
          THIRD-PARTY-NOTICES.txt
        include/
          dgen_runtime.h
          clang/<version>/include/...
      runtime/
        eseqlisp/init.lisp
        ui/...
      factory/...
```

`DGenLisp`, `dgen-clang`, and `ld64.lld` are nested executable code. Each is
built for arm64, stripped of development-only data, signed individually with
the release identity, and then sealed by the outer app signature. They are not
stored under `Contents/Resources`.

The compiler is an ESeq-private tool. Its filename, arguments, directory
layout, and presence are not a public command-line API.

## Writable Runtime Layout

The compiler never writes into the app bundle. Sources and user assets live in
Application Support; generated artifacts live in Caches:

```text
~/Library/Application Support/<bundle-id>/
  instruments/...
  effects/...
  samples/...
  projects/...

~/Library/Caches/<bundle-id>/
  dgen/
    <cache-schema>/
      arm64-apple-macos/
        dylibs/
          <cache-key>/
            <artifact-id>/
              source.lisp
              generated.c
              manifest.json
              metadata.json
              module.dylib
```

`generated.c` may be retained in the cache because it is valuable for crash
diagnostics and compiler bug reports. It contains generated implementation,
not an independently trusted input. A future privacy setting may remove it
after successful compilation.

Temporary compilation happens in a sibling staging directory. Successful
publication uses an atomic rename into the final artifact directory. Startup
may delete abandoned staging directories that have no live lease.

## Toolchain Composition

The release toolchain is built from upstream LLVM components rather than copied
from Xcode:

- Clang C frontend and driver support required by DGen.
- LLVM AArch64 target only for v0.1.
- LLVM optimization and object-emission libraries required by the selected
  build mode.
- LLD Mach-O linker.
- Clang builtin headers required by generated arm64 code.
- The Clang compiler-rt builtins archive (`libclang_rt.builtins`) for arm64,
  because Clang emits implicit calls to `memcpy`, `memset`, and compiler-rt
  helper routines even for freestanding-looking C.
- ESeq-owned DGen runtime headers and the DGen-owned libSystem link stub
  described below.

The build disables unused targets, examples, tests, documentation, static
analysis tools, debugger support, and unrelated LLVM command-line utilities.
Size reduction is allowed only through supported build configuration and
symbol stripping. Binary surgery or deleting files discovered through trial
and error is not an acceptable packaging strategy.

The release build records at least:

```json
{
  "distribution_version": 1,
  "dgen_abi_version": 1,
  "dgen_compiler_version": "...",
  "llvm_version": "...",
  "target": "arm64-apple-macos",
  "minimum_macos": "11.0",
  "clang_sha256": "...",
  "lld_sha256": "...",
  "runtime_headers_sha256": "..."
}
```

This file is part of the cache identity and release diagnostics.

## No Runtime Apple SDK Dependency

Simply bundling `clang` is insufficient if generated C includes arbitrary
Apple SDK headers or the linker expects the full SDK's text-based stubs. The
v0.1 toolchain must therefore compile against a deliberately small DGen
environment.

The constraint is link-time, not runtime. `libSystem` is present and loaded in
every macOS process, so **generated dylibs may — and do — depend on
`libSystem`**. What the toolchain must not require is an installed SDK,
Xcode, or Command Line Tools to *produce* that link. v0.1 therefore treats
libSystem as a deliberate, validated load dependency rather than engineering
it away.

This keeps hot-path DSP fast: allowlisted libm calls (`sinf`, `expf`, ...)
at genuinely scalar call sites remain direct calls that Clang can inline,
vectorize, or lower to builtins. Routing per-sample scalar math through a
host function-pointer table is explicitly rejected — the indirection defeats
vectorization on exactly the code that matters most.

Phase 1 inventory confirmed that current generated C uses Accelerate two
ways, with different call-site shapes that demand different treatment:

1. **Block-level FFT/vDSP operations** (`vDSP_create_fftsetup`,
   `vDSP_fft_zip`, `vDSP_zvma`): one call per buffer. These move behind the
   host service table; one indirect call per block is negligible.
2. **Legacy vecLib 4-lane vector math** (`vsinf`, `vcosf`, `vtanhf`, ...:
   `float32x4_t -> float32x4_t`): one external call per 4 samples, emitted
   inside the fused per-sample SIMD loop. These must NOT go through the host
   table — a per-4-lane indirect call is the per-sample indirection this
   spec rejects. Phase 2 must choose their permanent lowering in the
   DGen-owned runtime header. The candidates, gated on measurement, are:
   inline SLEEF-class polynomial vector implementations (also the future x86
   path); restructuring emission so transcendental math is batched into
   array-level calls that CAN use the table (only if loop fusion permits);
   or lane-wise scalar libm calls where measurement shows the cost is
   acceptable.

The current driver also always links `-framework Accelerate`, so every dylib
acquires an Accelerate load command regardless of source content; dropping
that unconditional flag is part of Phase 2.

The Phase 1 lane-wise libm shim over the 4-lane entry points is a link proof
only; it is not the Phase 2 contract, and its performance difference must be
measured and reported, not silently accepted.

Headers such as `Accelerate/Accelerate.h` and `mach/mach_time.h` disappear
from release-generated C under this contract.

### DGen Runtime Header

`dgen_runtime.h` owns all types, intrinsics wrappers, ABI structures, and host
service declarations visible to generated code. Generated source includes
only DGen-owned headers plus explicitly selected Clang builtin headers. The
DGen headers declare exact prototypes for the allowlisted libm symbols; no
SDK header is included.

The header must not expose filesystem, process, networking, Objective-C,
dynamic loading, or general libc APIs beyond the declared math allowlist.

Representative shape:

```c
typedef struct DGenHostServicesV1 DGenHostServicesV1;
typedef struct DGenProcessContextV1 DGenProcessContextV1;

/* Narrow table for block-level operations that would otherwise pull in
   Accelerate. Every entry is array-in/array-out over a whole buffer; the
   table never carries per-sample scalar math. */
typedef struct {
    unsigned int abi_version;
    unsigned int struct_size;
    void (*fft_forward_fn)(/* fixed ABI arguments */);
    void (*fft_inverse_fn)(/* fixed ABI arguments */);
    /* Additional block-level (array-in/array-out, once-per-buffer) entries
       only as the Phase 1 symbol inventory demands. 4-lane vecLib-style
       vector math is explicitly excluded — its lowering is decided in the
       DGen runtime header, not this table. */
} DGenHostServicesV1;

void dgen_process_v1(
    const float * const *inputs,
    float * const *outputs,
    unsigned int frame_count,
    void *state,
    const DGenProcessContextV1 *context,
    const DGenHostServicesV1 *host);
```

The exact function table is derived from measured DGen requirements. It must
remain narrow — block-level FFT/Accelerate-class services only; it is not a
generic native extension API and it never carries per-sample math.

Scalar and SIMD operations lower to compiler builtins, inline generated code,
or direct allowlisted libm calls. SIMD behavior is wrapped behind DGen-owned
inline functions so a later x86_64 backend does not leak into DSL semantics.

Profiling support belongs in the host or a separate compiler-instrumented
build. Release-generated DSP does not include `stdio`, Mach timing, logging,
or debug file APIs.

### Link Contract

Generated dylibs link against exactly one system library: `libSystem`. The
link is satisfied without an installed SDK by one of two mechanisms, decided
by the Phase 1 prototype:

1. A DGen-owned minimal text-based stub (`libSystem` `.tbd`) authored from the
   versioned symbol allowlist — written by ESeq, not copied from the SDK.
2. `-undefined dynamic_lookup` at link time, with the undefined-symbol audit
   below enforcing the same allowlist after linking.

Either way, the allowlist — allowlisted libm functions, the compiler-inserted
symbols Clang emits implicitly (`memcpy`, `memset`, `bzero`), and any
compiler-rt helpers not statically satisfied by the bundled
`libclang_rt.builtins` — is explicit, versioned, and enforced by post-link
inspection. "No undefined symbols at all" is not a goal; an *audited* symbol
surface is.

The generated dylib must not acquire undeclared framework or SDK dependencies.
After linking, ESeq inspects it before loading:

- Mach-O file type is a loadable arm64 dynamic library.
- CPU type is arm64.
- Deployment target is supported by the running app.
- Exported symbols exactly match the DGen ABI allowlist for the artifact kind.
- Load commands reference no library other than `libSystem` (in particular,
  no Accelerate or other framework).
- Undefined symbols match the explicit, versioned allowlist.
- Load commands contain no absolute developer, workspace, Xcode, temporary,
  or user paths.
- No `LC_RPATH` points outside the controlled runtime contract.
- File size and declared state sizes are within configured limits.
- The manifest ABI version matches the host ABI.

Falling back to an installed SDK or developer directory to satisfy the link is
not permitted.

## Compiler Invocation Contract

The host resolves all compiler paths through the production `AppPaths`
service. `DGenLisp` receives a toolchain root selected by the host; it does not
search `PATH`, call `xcrun`, inspect `DEVELOPER_DIR`, or fall back to
`/usr/bin/clang` in a release build.

The compiler process is launched directly with an argument vector. It is never
launched through a shell.

The host owns:

- Input source bytes.
- Artifact kind: instrument or effect.
- Sample rate and voice count.
- Canonical asset base.
- Staging and final output directories.
- Target triple and minimum OS version.
- Optimization profile.
- Compiler timeout and output-size limits.

The agent or DGenLisp source cannot alter these values.

Release flags are centralized and versioned. A representative policy is:

```text
-target arm64-apple-macos11
-O3
-ffast-math                 # only if DGen numerical semantics explicitly permit it
-fvisibility=hidden
-fno-exceptions
-fno-asynchronous-unwind-tables
-nostdinc
-isystem <bundled-clang-builtins>
-I <dgen-runtime-include>
```

The final flags must be validated against sound-quality, NaN/Inf sanitization,
debuggability, and deterministic-cache requirements. The current deprecated
`-Ofast` spelling must not be carried into the release toolchain.

Linker flags likewise come from one versioned policy owned by DGenLisp. No
source-level escape hatch may inject additional flags.

## Source And Asset Boundary

Only valid DGenLisp reaches code generation. Validation occurs before spawning
Clang and includes:

- Parse and AST depth/size limits.
- Known operator and form allowlists.
- Tensor, wavetable, and other asset-reference validation.
- Bounded declared memory, voices, parameters, outputs, and modulation nodes.
- Rejection of unsupported path forms.

Relative assets resolve from the saved instrument/effect directory. Factory
assets resolve from the immutable factory artifact directory. External assets
must be imported or copied into an ESeq-managed artifact before compilation;
compiled instruments do not retain arbitrary filesystem authority.

Asset paths are canonicalized and verified to remain within their approved
artifact root. Symlink traversal outside that root is rejected.

Generated C is written only by DGenLisp. The compiler helper reads the exact
generated file descriptor or canonical staging path supplied by the host. It
does not scan source directories or follow compiler-controlled include paths.

## Compilation Lifecycle

### Request

```rust
pub struct DGenCompileRequest {
    pub kind: DGenCompileKind,
    pub origin: DGenSourceOrigin,
    pub source: String,
    pub sample_rate: u32,
    pub voices: Option<u32>,
    pub asset_base: Option<PathBuf>,
}
```

The service derives all other fields from trusted configuration.

### Pipeline

```text
validate request and source
  -> materialize approved defmacro imports
  -> generate effective DGen source
  -> fingerprint source, assets, ABI, flags, and toolchain
  -> acquire cache-key compile lock
  -> reuse a validated free cached artifact if available
  -> otherwise create staging directory
  -> invoke DGenLisp and embedded compiler
  -> parse structured manifest
  -> inspect Mach-O and symbols
  -> load in validation worker context
  -> initialize through production host ABI
  -> run deterministic offline audition
  -> atomically publish artifact
  -> acquire lease
  -> notify control thread
```

Only one compilation for the same cache key runs at once. Different keys may
be compiled concurrently up to a small configured worker limit. v0.1 may use a
single compiler worker if measurements show agent latency remains acceptable.

Compiler stdout and stderr are bounded. Diagnostics are captured as structured
stage failures and sanitized before display. A compiler timeout terminates the
compiler process group and removes its unpublished staging directory.

### Publication And Hot Swap

A successful artifact is first loaded and auditioned outside the audio
callback. The control thread then prepares the initialized DSP state and
publishes it through the existing graph/state transition mechanism.

The old artifact and state remain leased until no audio callback can reference
them. Swapping should use the existing safe graph transition or a short
crossfade; it must not close a dylib that an in-flight callback could execute.

If compilation, validation, initialization, or audition fails:

- The active artifact is unchanged.
- The draft sources remain available for repair.
- The failed artifact is never entered as a reusable cache hit.
- The agent receives the precise failing stage and bounded diagnostic.
- The user receives a concise, actionable error.

## Cache Identity And Integrity

The current cache already fingerprints effective source, DGenLisp, assets,
sample rate, voice count, and compile kind. v0.1 extends the key with:

- Cache schema version.
- DGen source-language version.
- DGen host ABI version.
- DGen manifest version.
- Full embedded toolchain distribution fingerprint.
- Target triple and minimum macOS version.
- Code-generation and link policy version.
- Optimization/numerical-semantics profile.
- Architecture-specific lowering version.

Cache identity must not depend on absolute installation paths or timestamps.
Metadata may record them for diagnostics, but moving `ESeq.app` must not force
an otherwise unnecessary recompile.

Every cache hit repeats cheap integrity checks before loading:

- Metadata parses and matches the request.
- Source and asset fingerprints match.
- Dylib hash matches metadata.
- Manifest hash matches metadata.
- ABI and architecture match.
- Binary load-command and symbol audit passes.

Corrupt or stale entries are quarantined or ignored and recompiled. Cache
corruption never prevents the source artifact from opening.

## Agent Integration

The agent pipeline remains source-oriented:

```text
agent response
  -> structured DGenLisp artifact extraction
  -> DSL validation
  -> compilation service
  -> manifest/UI validation
  -> instrument_probe-equivalent audition
  -> save and activate
```

Agents never see or control the embedded compiler command. Compiler diagnostics
may be returned to the agent for repair, but the retry instruction continues
to require a complete DGenLisp artifact rather than C or linker changes.

The generated C may be exposed in an advanced diagnostic view for humans, but
it is read-only and cannot become an input to the production compilation path.

## Security Model And Accepted Risk

v0.1 accepts the product-level risk of loading locally generated native DSP.
This is necessary for ESeq's core capability. The risk is materially reduced
because users and agents provide DGenLisp rather than native code or C.

The trust chain is:

```text
untrusted agent/user intent
  -> constrained DGenLisp source
  -> trusted parser, validator, lowering, and C generator
  -> trusted bundled compiler/linker
  -> audited DGen ABI dylib
  -> ESeq process
```

The DSL boundary prevents ordinary access to filesystem, networking, process
creation, arbitrary memory operations, inline assembly, compiler extensions,
and linker features. These properties must be enforced by implementation, not
assumed from current examples.

Residual risks remain:

- A bug in the DGen parser, optimizer, C generator, host ABI, or compiler could
  turn valid or adversarial DSL into unsafe native behavior.
- Incorrect bounds or memory-size calculations could corrupt the host process.
- Path validation bugs could expose unintended assets during compilation.
- Denial of service remains possible through pathological source unless input,
  compilation, memory, and execution limits are enforced.
- Native DSP runs in the main process after loading; a successful escape from
  the DSL/codegen boundary inherits ESeq's process authority.

v0.1 does not require a separate DSP process or heavyweight sandbox. It does
require the low-complexity controls already described: closed inputs, bounded
resources, canonical asset roots, transactional compilation, binary audit,
offline audition, and no general compiler interface.

Crashes during audition should occur in an isolated probe/helper process if
the current host architecture can support that without duplicating the audio
engine. If it cannot be completed robustly for v0.1, in-process audition is an
explicit accepted risk and must be recorded in release notes; fake isolation
or partial signal-handler recovery is not acceptable.

## Code Signing And Hardened Runtime

The outer app and every bundled executable are signed with Developer ID and the
hardened runtime enabled. Generated dylibs cannot carry the developer's Team ID
because the user's machine does not possess the release signing identity.

The v0.1 app therefore uses:

```xml
<key>com.apple.security.cs.disable-library-validation</key>
<true/>
```

This entitlement is a deliberate product decision, not a packaging accident.
No broader executable-memory or debugger entitlement is added unless testing
proves it is required for the dylib design.

Generated dylibs should retain the valid ad-hoc/linker signature produced by
the embedded linker when applicable. The binary audit records signature state,
but the DGen ABI validation remains the relevant trust boundary.

The release process must:

1. Sign nested helpers individually with their intended identifiers.
2. Sign the outer app with hardened runtime and the reviewed entitlements.
3. Verify signatures without relying on `codesign --deep` for signing.
4. Submit the final archive for notarization.
5. Staple the ticket.
6. Compile, load, and run a new DGen instrument from the stapled app on a clean
   supported Mac with no Xcode or Command Line Tools installed.

Passing notarization alone is not evidence that runtime-generated dylibs load
correctly.

## Realtime And Reliability Requirements

- Compilation and loading never occur on the audio callback.
- The callback sees only a fully initialized immutable dispatch/state handle.
- No compiler or cache mutex is reachable from the callback.
- DSP entry points are fixed by ABI version and resolved before publication.
- Generated processing performs no allocation, file I/O, logging, locking, or
  Objective-C messaging on the callback.
- Buffer bounds and frame-count assumptions are validated at initialization
  and guarded in generated code where necessary.
- NaN and infinity containment remains enabled at graph boundaries.
- Declared state allocation is bounded before allocation and agrees with the
  manifest and generated ABI.
- A dylib remains loaded until every state instance and in-flight callback
  lease has been released.

## Development Mode

Development builds may support explicit overrides such as:

```text
ESEQ_DGEN_TOOLCHAIN_ROOT=/absolute/path/to/test-toolchain
```

Overrides must be opt-in, validated, and visibly logged. They must not silently
fall back from a missing bundled toolchain to Xcode or `/usr/bin/clang`.

Release builds ignore development overrides unless a separate, intentionally
unsigned developer distribution is produced. This prevents environment
variables from changing the compiler used by the signed application.

The existing `crates/sequencer/tools/DGenLisp` workspace path remains a
development source/tool location, not a production runtime path.

## Build And Packaging Pipeline

Add a reproducible release-toolchain build that:

1. Fetches a pinned LLVM source revision with verified checksum.
2. Builds only the required Clang, LLVM AArch64, and LLD Mach-O components.
3. Runs the LLVM/DGen license inventory step.
4. Installs into a staging prefix with no absolute build-machine references.
5. Strips supported debug symbols into a separately archived symbol package.
6. Produces `VERSION.json` and file hashes.
7. Runs a hermetic compile smoke test with Xcode paths unavailable.
8. Copies helpers and resources into the app bundle.
9. Signs nested code and the app in the correct order.
10. Runs bundle validation, notarization, and clean-machine acceptance tests.

The build fails if `strings`, Mach-O load commands, compiler search paths, or
test traces reveal references to the build workspace, an Xcode installation,
`/Library/Developer/CommandLineTools`, or `/usr/bin/clang` in the production
compile path.

Toolchain size is measured and reported for every release. Size optimization
is desirable, but correctness and a hermetic runtime take priority over an
arbitrary initial download-size target.

## Repository Ownership

The work splits across two repositories:

- **dgen repository** (`dgen-audio`, the Swift package that becomes the
  bundled `DGenLisp` helper) owns Phases 1 and 2 entirely: the pinned LLVM
  toolchain build, the C codegen contract, `dgen_runtime.h`, the
  `DGenHostServicesV1` definition, the libm symbol allowlist, the libSystem
  link stub, the link/flag policy, and the binary audit tooling.
- **ESeq repository** owns Phases 3 through 5: `AppPaths`, bundling, signing,
  notarization, entitlements, cache integration, hot swap, and the agent/UI
  lifecycle. ESeq consumes the ABI headers and audit tooling that dgen
  publishes; it does not redefine them.

The no-Accelerate constraint applies only to generated dylibs, not to dgen's
own test code. The dgen repository therefore ships a **reference host
harness**: a small test program that implements `DGenHostServicesV1` with its
own Accelerate-backed FFT wrappers, loads a compiled artifact, and drives
`dgen_process_v1`. This closes the full loop — compile, link, audit, load,
run, verify output — standalone, and doubles as executable documentation of
what ESeq must implement. Phases 1 and 2 are provable in the dgen repository
with no ESeq involvement.

## Implementation Phases

### Phase 1: Toolchain contract prototype

Owned by the dgen repository.

- Build pinned upstream Clang/LLVM/LLD for arm64.
- Compile a minimal DGen-generated C file without invoking Apple Clang.
- Inventory every header and external symbol required by representative
  instruments and effects.
- Prove Mach-O linking without an installed Apple SDK at runtime, and decide
  between the DGen-owned libSystem stub and `-undefined dynamic_lookup`.
- Record compressed and installed size.

Exit criterion: a prototype bundle of tools compiles and loads representative
DGen artifacts on a clean Mac without Command Line Tools.

### Phase 2: Closed DGen host ABI

Owned by the dgen repository.

- Introduce versioned runtime headers, the libm symbol allowlist, and the
  narrow FFT/Accelerate-class host service table.
- Remove SDK and diagnostic headers from generated release C; libm prototypes
  come from DGen-owned headers and remain direct calls.
- Author the DGen-owned libSystem link stub, or validate the
  `-undefined dynamic_lookup` alternative chosen in Phase 1.
- Route FFT/vDSP block operations through the host table, choose and
  implement the measured lowering for the 4-lane vecLib vector-math calls
  (inline vector implementations, batched array-level emission, or lane-wise
  libm), and stop passing `-framework Accelerate` at link time, so
  Accelerate is no longer a link dependency of generated code.
- Add export, undefined-symbol, load-command, and architecture auditing,
  including the implicit `memcpy`/`memset`/compiler-rt symbol surface.
- Add ABI mismatch diagnostics.
- Build the reference host harness (Accelerate-backed `DGenHostServicesV1`
  implementation) and validate representative artifacts end to end through it.

Exit criterion: representative generated dylibs depend only on `libSystem`,
pass the binary allowlist, have no undeclared runtime dependencies, and
produce correct audio through the reference harness with `DEVELOPER_DIR` and
Xcode paths unavailable.

### Phase 3: Production paths and cache

Owned by the ESeq repository (as are Phases 4 and 5).

- Add production `AppPaths` locations for helpers, toolchain resources, and
  cache data.
- Remove `current_dir()` and workspace-path assumptions from compilation.
- Extend cache keys and metadata with ABI/toolchain policy.
- Add atomic staging/publication, corruption recovery, and compiler locking.
- Preserve existing artifact leases through hot swap.

Exit criterion: moving the app bundle does not break compilation, and deleting
the cache causes a transparent rebuild from saved sources.

### Phase 4: Agent and UI lifecycle

- Route every agent, editor, factory, and project DGen compile through one
  compilation service.
- Preserve last known-good DSP on every failure path.
- Return structured, bounded diagnostics to agent retries.
- Show compile, validate, audition, success, and failure states in the UI.
- Ensure cancellation and project close cannot publish a stale result.

Exit criterion: an agent can create, repair, save, reload, and reactivate a new
instrument entirely inside the app.

### Phase 5: Signed release validation

- Add reviewed hardened-runtime entitlements.
- Sign helpers and app; notarize and staple the distribution.
- Exercise compilation and dylib loading from the stapled artifact.
- Run clean-machine, clean-user-account, moved-app, read-only-bundle, cache-
  deletion, and corrupted-cache scenarios.

Exit criterion: all v0.1 acceptance criteria pass from the exact distributable
artifact.

## Required Tests

Use narrow targets during development, following the repository test policy.
The final release acceptance workflow is separate from ordinary unit testing.

### Unit tests

- Toolchain path resolution never consults `PATH` or Xcode in release mode.
- Cache key changes for every ABI, toolchain, source, asset, target, sample-rate,
  voice-count, and policy input that changes generated behavior.
- Cache key is stable when the app bundle moves.
- Asset canonicalization rejects traversal and escaping symlinks.
- Generated compiler arguments cannot contain source-provided flags.
- Manifest limits reject oversized state, buffers, params, or symbols.
- Failed publication preserves the last known-good artifact.
- Stale asynchronous results cannot replace a newer edit.

### Compiler integration tests

- Compile a minimal effect and instrument with the embedded toolchain.
- Compile representative scalar, feedback, wavetable, tensor, convolution,
  modulation, and polyphonic instruments.
- Inspect architecture, load commands, exports, undefined symbols, signature,
  and deployment target.
- Load and initialize through the production host ABI.
- Confirm deterministic output within the defined floating-point tolerance.
- Confirm invalid DSL never launches Clang.
- Confirm compiler crash, timeout, malformed manifest, invalid Mach-O, and
  failed audition do not publish an artifact.

### Instrument validation

Every curated release instrument and effect is compiled through the embedded
toolchain. DGenLisp instruments use `instrument_probe` with instrument-specific
parameters where necessary. Signal checks cover finite output, minimum peak and
RMS where sound is expected, bounded amplitude, parameter changes, repeated
note lifecycle, and deterministic initialization.

### Release acceptance

- Supported clean Mac with neither Xcode nor Command Line Tools installed.
- Stapled Developer ID distribution, not a workspace executable.
- App launched from `/Applications` and from another valid user-selected path.
- App bundle mounted/read-only while compilation succeeds into user cache.
- New agent-generated instrument compiles, auditions, activates, saves, and
  reloads after restart.
- New custom effect follows the same lifecycle.
- Cache deletion triggers successful recompilation.
- Corrupt cache entry is ignored and rebuilt.
- Compiler helper missing or tampered produces a clear error and preserves the
  active project.
- Compile occurs during uninterrupted audio playback without callback stalls.
- Gatekeeper, hardened runtime, and library validation do not block the
  generated artifact under the reviewed entitlement set.

## Acceptance Criteria

ESeq v0.1 satisfies this spec when all of the following are true:

1. A clean supported Mac can create and run a new DGen instrument without
   Xcode, Command Line Tools, Homebrew, or network access.
2. The app invokes only its bundled, signed compiler and linker.
3. Agents and users can provide only DGenLisp and managed assets to the native
   compilation path.
4. Generated dylibs expose only the versioned DGen ABI and pass binary audit.
5. Compilation and validation never block the audio callback.
6. A failure at any stage leaves the last known-good DSP and project state
   intact.
7. Saved source artifacts survive cache deletion and app upgrades.
8. The signed and notarized release can compile and load generated dylibs with
   the documented entitlement set.
9. Every curated factory DGen instrument and effect passes the embedded
   toolchain and appropriate probe validation.
10. No production compile depends on an absolute repository, Xcode, SDK,
    Command Line Tools, or current-working-directory path.

## Follow-Up Work After v0.1

- Intel or universal app/toolchain support.
- LLVM ORC JIT to remove dylib/linker publication where it provides measurable
  latency or lifecycle benefits.
- A dedicated DSP helper process if stronger fault isolation becomes a product
  priority.
- Additional architecture-neutral SIMD lowering.
- Compiler-service incremental compilation.
- User-visible generated-code diagnostics and reproducible support bundles.
- Optional artifact signing or provenance receipts beyond cache hashes.
- Fuzzing of the DGen parser, lowering, manifest parser, host ABI, and generated
  buffer-boundary behavior.

These are not prerequisites for the v0.1 embedded native compilation promise.
