# Publishing DGenLisp releases

## Linux x86_64

This is the complete Linux build, verification, publication, and eseq re-pin
recipe for DGenLisp. Run it from an x86_64 Linux host with Docker, `binutils`,
`jq`, `gh`, and an authenticated GitHub CLI. The upstream checkout is
[`universalsequences/dgen-audio`](https://github.com/universalsequences/dgen-audio).

The release binary is built in Ubuntu 22.04 so its glibc requirement remains at
or below 2.35. Do not replace the image with the host Swift toolchain: an Arch
host build inherits the host's newer glibc. Do not replace the image with
`swift:6.3-jammy` or a `-slim` image either; the former is not an exact compiler
pin and the latter omits required build tools.

## 1. Choose the source and version

Start with clean dgen-audio and eseq checkouts. Set absolute paths and choose a
new version:

```sh
export DGEN_ROOT=/absolute/path/to/dgen-audio
export ESEQ_ROOT=/absolute/path/to/eseq
export VERSION=0.1.3
export TAG="dgenlisp-v$VERSION"
export IMAGE=swift:6.3.3-jammy

cd "$DGEN_ROOT"
test -z "$(git status --porcelain)" || {
  echo "error: dgen-audio checkout is dirty" >&2
  exit 1
}
export UPSTREAM_COMMIT="$(git rev-parse HEAD)"
```

The source must include executable-relative runtime-resource resolution and the
Linux staged-toolchain support introduced by dgen-audio commits `ae25e9d` and
`ab969ed`, respectively.

## 2. Build in the container

```sh
docker info >/dev/null
docker run --rm \
  --user "$(id -u):$(id -g)" \
  -e HOME=/tmp \
  -v "$DGEN_ROOT:/src" \
  -w /src \
  "$IMAGE" \
  swift build -c release --static-swift-stdlib \
    --product DGenLisp \
    --scratch-path /src/.build-container
```

Every unusual option is intentional:

- `--user` prevents root-owned build products in the host checkout.
- **`-e HOME=/tmp` is required.** With `--user` alone, this image leaves
  `HOME=/`; SwiftPM then fails to write `/.swiftpm` and Clang's
  `/.cache/clang/ModuleCache`. The resulting `could not build C module
  'SwiftShims'` diagnostic looks like a toolchain failure but is a permissions
  failure.
- `.build-container` keeps container and host Swift artifacts separate. It must
  remain ignored by dgen-audio.
- `--static-swift-stdlib` prevents runtime dependencies on Swift, Foundation,
  dispatch, and Foundation ICU shared libraries.

Record the immutable image digest for the release notes:

```sh
export IMAGE_ID="$(docker image inspect "$IMAGE" --format '{{index .RepoDigests 0}}')"
printf 'source: %s\nimage:  %s\n' "$UPSTREAM_COMMIT" "$IMAGE_ID"
```

## 3. Assemble target-qualified assets

The standalone archive is the asset eseq pins. It includes resources which the
compiler resolves relative to its executable; shipping only the executable
requires callers to provide those resources explicitly.

```sh
cd "$DGEN_ROOT"
export BUILD="$DGEN_ROOT/.build-container/x86_64-unknown-linux-gnu/release/DGenLisp"
export DIST_NAME=dgenlisp-linux-x86_64
export DIST="$DGEN_ROOT/out/$DIST_NAME"
export BARE="$DGEN_ROOT/out/DGenLisp-linux-x86_64"
export ARCHIVE="$BARE.tar.gz"

test -x "$BUILD"
rm -rf "$DIST"
mkdir -p "$DIST/scripts" "$DIST/toolchain/include" "$DIST/toolchain/abi"
install -m 0755 "$BUILD" "$DIST/DGenLisp"
strip "$DIST/DGenLisp"
install -m 0755 scripts/audit-dgen-elf-so.sh "$DIST/scripts/"
install -m 0644 toolchain/include/*.h "$DIST/toolchain/include/"
install -m 0644 toolchain/abi/*.txt "$DIST/toolchain/abi/"
cp "$DIST/DGenLisp" "$BARE"
tar -C "$DGEN_ROOT/out" -czf "$ARCHIVE" "$DIST_NAME"
```

Both published filenames contain `linux-x86_64`. Never publish an unqualified
asset named only `DGenLisp`.

## 4. Run all pre-publication gates

Fetch eseq's pinned staged compiler toolchain if it is not already installed:

```sh
cd "$ESEQ_ROOT"
./scripts/fetch_dgen_toolchain.sh
export TOOLCHAIN="$ESEQ_ROOT/crates/sequencer/tools/dgen-toolchain"
```

### glibc floor

The maximum imported GLIBC symbol must be no newer than 2.35:

```sh
export MAX_GLIBC="$({ objdump -T "$BARE" || exit; } |
  grep -oE 'GLIBC_[0-9.]+' | sed 's/^GLIBC_//' | sort -Vu | tail -1)"
test -n "$MAX_GLIBC"
test "$(printf '2.35\n%s\n' "$MAX_GLIBC" | sort -Vu | tail -1)" = 2.35 || {
  echo "error: DGenLisp requires GLIBC_$MAX_GLIBC (maximum is GLIBC_2.35)" >&2
  exit 1
}
printf 'maximum required GLIBC symbol: GLIBC_%s\n' "$MAX_GLIBC"
```

### Dynamic dependencies

`ldd` may report the kernel-provided `linux-vdso.so.1`. Apart from that pseudo
library, the only permitted dependencies are libc, libm, libstdc++, libgcc_s,
and the ELF loader:

```sh
ldd "$BARE"
diff -u \
  <(printf '%s\n' \
    ld-linux-x86-64.so.2 libc.so.6 libgcc_s.so.1 libm.so.6 \
    libstdc++.so.6 linux-vdso.so.1 | sort) \
  <(ldd "$BARE" | awk '
      $1 ~ /^\// { name=$1; sub(".*/", "", name); print name; next }
      $1 ~ /\.so/ { print $1 }
    ' | sort -u)
```

This gate must show no `libswift*`, `libFoundation*`, `libdispatch`, or
`lib_FoundationICU` dependency.

### Functional standalone smoke test

Compile a real patch with the packaged compiler, not the build-tree binary.
The result must contain nonempty generated C, manifest, and shared-object files,
and must advertise the host ABI expected by eseq:

```sh
export SMOKE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/dgenlisp-release.XXXXXX")"
trap 'rm -rf "$SMOKE_DIR"' EXIT
"$DIST/DGenLisp" compile \
  "$DGEN_ROOT/toolchain/fixtures/scalar-synth.lisp" \
  --output "$SMOKE_DIR" \
  --name release-smoke \
  --toolchain-root "$TOOLCHAIN" \
  >"$SMOKE_DIR/stdout.json"

test -s "$SMOKE_DIR/release-smoke.c"
test -s "$SMOKE_DIR/release-smoke.json"
test -s "$SMOKE_DIR/release-smoke.so"
test "$(jq -r .processAbi "$SMOKE_DIR/release-smoke.json")" = dgen-host-abi-v1
```

Do not publish if any gate fails.

## 5. Publish safely

Create the release as a draft so an interrupted upload cannot expose a partial
release. Include the source commit, image digest, measured glibc floor,
dependency result, and smoke-test result in the notes.

```sh
cd "$DGEN_ROOT"
gh release create "$TAG" "$BARE" "$ARCHIVE" \
  --repo universalsequences/dgen-audio \
  --target "$UPSTREAM_COMMIT" \
  --title "DGenLisp v$VERSION" \
  --draft \
  --notes "Built from $UPSTREAM_COMMIT in $IMAGE ($IMAGE_ID) with --static-swift-stdlib. Maximum required symbol GLIBC_$MAX_GLIBC. ldd contains only libc, libm, libstdc++, libgcc_s and the loader. The standalone archive compiled toolchain/fixtures/scalar-synth.lisp to nonempty .c/.json/.so outputs with processAbi dgen-host-abi-v1."
```

A slow connection may time out while uploading the roughly 64 MB bare binary.
`gh release create` can then leave a draft with only some assets. This is safe:
nothing is public. Inspect and recover the same draft instead of creating a
second release:

```sh
gh release view "$TAG" --repo universalsequences/dgen-audio \
  --json isDraft,assets,url
gh release upload "$TAG" "$BARE" "$ARCHIVE" \
  --repo universalsequences/dgen-audio --clobber
```

After both target-qualified assets are present, publish and verify anonymous
access:

```sh
gh release edit "$TAG" --repo universalsequences/dgen-audio --draft=false
curl --fail --location --output /dev/null \
  "https://github.com/universalsequences/dgen-audio/releases/download/$TAG/$(basename "$ARCHIVE")"
sha256sum "$BARE" "$ARCHIVE"
```

The repository and asset must be publicly downloadable; otherwise fresh eseq
clones and CI cannot use the unauthenticated fetch script.

A macOS artifact must be built and verified on macOS. If one is published, name
it `DGenLisp-macos-arm64`, record its independent provenance, and re-pin its
lock stanza independently. Never imply that the Linux container produced it.

## 6. Re-pin eseq

Only after the public download succeeds, update the `linux-x86_64` stanza in
[`content/dgenlisp.lock`](../content/dgenlisp.lock):

- `version`: `$TAG`
- `url`: the target-qualified `.tar.gz` release URL tested above
- `sha256`: `sha256sum "$ARCHIVE"`
- `format`: `tar.gz`
- `executable_path`: `dgenlisp-linux-x86_64/DGenLisp`
- `upstream_commit`: `$UPSTREAM_COMMIT`
- `glibc_floor`: `$MAX_GLIBC`
- `container_image`: `$IMAGE`

Do not re-pin the bare binary: the archive is the standalone distribution with
its runtime headers and audit policy. Leave other target stanzas unchanged.
Then verify the public asset through eseq's normal installation path. The two
lock files use different target vocabularies on purpose — `dgenlisp.lock` names
targets `linux-x86_64`/`macos-arm64`, `dgen-toolchain.lock` uses rustc-style
triples — so the two fetch scripts take different `--target` values. They also differ in
where they install: `fetch_dgenlisp.sh` installs target-qualified and is safe to
run cross-target from any machine, while `fetch_dgen_toolchain.sh` installs the
stage at the single host path `crates/sequencer/tools/dgen-toolchain` and
therefore refuses any target but the host's — so run this verification on the
Linux box:

```sh
cd "$ESEQ_ROOT"
./scripts/fetch_dgenlisp.sh --target linux-x86_64 --force
./scripts/fetch_dgen_toolchain.sh --target x86_64-unknown-linux-gnu

export INSTALLED="$ESEQ_ROOT/crates/sequencer/tools/DGenLisp-linux-x86_64"
export VERIFY_DIR="$(mktemp -d "${TMPDIR:-/tmp}/dgenlisp-repin.XXXXXX")"
trap 'rm -rf "$VERIFY_DIR"' EXIT
"$INSTALLED" compile \
  "$DGEN_ROOT/toolchain/fixtures/scalar-synth.lisp" \
  --output "$VERIFY_DIR" --name repin-smoke \
  --toolchain-root "$ESEQ_ROOT/crates/sequencer/tools/dgen-toolchain" \
  >"$VERIFY_DIR/stdout.json"
test -s "$VERIFY_DIR/repin-smoke.c"
test -s "$VERIFY_DIR/repin-smoke.json"
test -s "$VERIFY_DIR/repin-smoke.so"
test "$(jq -r .processAbi "$VERIFY_DIR/repin-smoke.json")" = dgen-host-abi-v1
```

Commit the lock-file change separately from upstream DGenLisp source changes so
the reviewed pin remains an explicit supply-chain update.

## macOS arm64

Run this workflow on an Apple Silicon Mac with Xcode, the Xcode command-line
tools, `jq`, `gh`, and an authenticated GitHub CLI. The macOS and Linux
artifacts have independent provenance and independent stanzas in
[`content/dgenlisp.lock`](../content/dgenlisp.lock). Publishing one target does
not require changing the other target's pin.

The DGenLisp distribution and the staged compiler toolchain are different
artifacts:

- the small DGenLisp archive built below contains the compiler, its inline
  binary-audit script, runtime headers, and ABI allowlists;
- `--toolchain-root` selects the much larger target-specific Clang/lld stage
  which actually compiles generated C.

The stage's complete, stable layout is defined by dgen-audio's
[`toolchain/LAYOUT.md`](https://github.com/universalsequences/dgen-audio/blob/main/toolchain/LAYOUT.md).
Do not copy that entire stage into the DGenLisp archive. Eseq installs the
compiler distribution according to the contract in
[`scripts/fetch_dgenlisp.sh`](../scripts/fetch_dgenlisp.sh) and stages the
compiler toolchain separately.

### 1. Choose clean, published source and a version

Set absolute paths and a new version. The source commit must already be pushed:
a release whose recorded commit exists only in a local checkout is not
reproducible provenance.

```sh
export DGEN_ROOT=/absolute/path/to/dgen-audio
export ESEQ_ROOT=/absolute/path/to/eseq
export VERSION=0.1.5
export TAG="dgenlisp-v$VERSION"

cd "$DGEN_ROOT"
test "$(uname -s):$(uname -m)" = Darwin:arm64
test -z "$(git status --porcelain)" || {
  echo "error: dgen-audio checkout is dirty" >&2
  exit 1
}
git fetch origin
export UPSTREAM_COMMIT="$(git rev-parse HEAD)"
git merge-base --is-ancestor "$UPSTREAM_COMMIT" origin/main || {
  echo "error: $UPSTREAM_COMMIT has not been pushed to origin/main" >&2
  exit 1
}
printf 'source: %s\n' "$UPSTREAM_COMMIT"
```

The source must resolve runtime resources relative to the *real* executable,
including through a symlink. This is required because `fetch_dgenlisp.sh`
installs a tar distribution under `DGenLisp-macos-arm64.dist/` and exposes it
through a `DGenLisp-macos-arm64` symlink. Dgen-audio commit `701fd6b` introduced
the required symlink resolution and its regression test; do not publish older
source as a tar distribution.

### 2. Build the release compiler

Build natively for arm64. Use SwiftPM's reported binary directory rather than
hard-coding an SDK- or Swift-version-dependent `.build` path.

```sh
cd "$DGEN_ROOT"
swift build -c release --product DGenLisp
export BUILD_DIR="$(swift build -c release --show-bin-path)"
export BUILD="$BUILD_DIR/DGenLisp"
test -x "$BUILD"
file "$BUILD"
```

`file` must report a Mach-O arm64 executable. Record the Swift and Xcode
versions in the release notes:

```sh
swift --version
xcodebuild -version
```

### 3. Assemble the target-qualified archive

The archive must untar to one target-qualified top-level directory. Keep the
directory name lowercase and the release asset target-qualified. On the default
case-insensitive macOS filesystem, a sibling bare asset named
`DGenLisp-macos-arm64` collides with the `dgenlisp-macos-arm64/` directory, so
the standalone tarball is the canonical macOS asset.

Strip before signing because stripping changes the executable. Ad-hoc signing
is sufficient for this command-line development tool; verify the resulting
signature before continuing.

```sh
cd "$DGEN_ROOT"
export DIST_NAME=dgenlisp-macos-arm64
export DIST="$DGEN_ROOT/out/$DIST_NAME"
export ARCHIVE="$DGEN_ROOT/out/DGenLisp-macos-arm64.tar.gz"

rm -rf "$DIST" "$ARCHIVE"
mkdir -p "$DIST/scripts" "$DIST/toolchain/include" "$DIST/toolchain/abi"
install -m 0755 "$BUILD" "$DIST/DGenLisp"
strip -x "$DIST/DGenLisp"
codesign --force --sign - "$DIST/DGenLisp"
install -m 0755 scripts/audit-dgen-dylib.sh "$DIST/scripts/"
install -m 0644 toolchain/include/*.h "$DIST/toolchain/include/"
install -m 0644 toolchain/abi/*.txt "$DIST/toolchain/abi/"
tar -C "$DGEN_ROOT/out" -czf "$ARCHIVE" "$DIST_NAME"

file "$DIST/DGenLisp"
codesign --verify --strict --verbose=2 "$DIST/DGenLisp"
tar -tzf "$ARCHIVE"
export ARCHIVE_SHA256="$(shasum -a 256 "$ARCHIVE" | awk '{print $1}')"
printf 'archive sha256: %s\n' "$ARCHIVE_SHA256"
```

The executable-relative files are not optional. Without
`scripts/audit-dgen-dylib.sh`, compilation fails at inline audit. Without the
runtime headers and ABI files, the compiler distribution is not standalone.

### 4. Stage the pinned arm64 compiler toolchain

Eseq's arm64 toolchain is currently vendored from a local dgen-audio archive,
not downloaded: its stanza in
[`content/dgen-toolchain.lock`](../content/dgen-toolchain.lock) deliberately has
no URL. If the exact archive pinned there is absent, build it natively according
to `toolchain/LAYOUT.md`:

```sh
cd "$DGEN_ROOT"
scripts/build-toolchain.sh
```

Then let eseq verify the archive hash and stage it at the production path:

```sh
cd "$ESEQ_ROOT"
DGEN_REPO="$DGEN_ROOT" ./rebuild_dgenlisp_tool.sh
export TOOLCHAIN="$ESEQ_ROOT/crates/sequencer/tools/dgen-toolchain"
test -f "$TOOLCHAIN/VERSION.json"
test "$(jq -r .target "$TOOLCHAIN/VERSION.json")" = arm64-apple-macos
```

`rebuild_dgenlisp_tool.sh` does not build DGenLisp and does not accept a
nearby toolchain archive: it stages only the archive identity pinned by eseq.
A hash mismatch is a release blocker, not permission to weaken the check.

### 5. Run all pre-publication gates

Use the packaged compiler under `$DIST`, never the build-tree binary. The first
smoke verifies ordinary compilation, the expected host ABI, and the packaged
inline-audit resources.

```sh
export SMOKE_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/dgenlisp-macos-release.XXXXXX")"
trap 'rm -rf "$SMOKE_ROOT"' EXIT
mkdir -p "$SMOKE_ROOT/scalar"

"$DIST/DGenLisp" compile \
  "$DGEN_ROOT/toolchain/fixtures/scalar-synth.lisp" \
  --output "$SMOKE_ROOT/scalar" \
  --name release-smoke \
  --toolchain-root "$TOOLCHAIN" \
  >"$SMOKE_ROOT/scalar/stdout.json"

test -s "$SMOKE_ROOT/scalar/release-smoke.c"
test -s "$SMOKE_ROOT/scalar/release-smoke.json"
test -s "$SMOKE_ROOT/scalar/release-smoke.dylib"
test "$(jq -r .processAbi "$SMOKE_ROOT/scalar/release-smoke.json")" = dgen-host-abi-v1
```

Also compile a grouped-parameter fixture. This protects the resolver,
modulation lowering, and host-facing manifest identity shipped by the compiler,
not merely the ability to start the executable:

```sh
cat >"$SMOKE_ROOT/namespaced.lisp" <<'LISP'
(def mod1 (in 5 @name mod1 @modulator 1))
(param attack @group op1
  @default 0.25 @min 0 @max 1 @mod true @mod-mode additive)
(param attack @group op2
  @default 0.75 @min 0 @max 1 @mod true @mod-mode additive)
(out (+ (mod op1.attack) op2.attack~) 1)
LISP
mkdir -p "$SMOKE_ROOT/namespaced"

"$DIST/DGenLisp" compile \
  "$SMOKE_ROOT/namespaced.lisp" \
  --output "$SMOKE_ROOT/namespaced" \
  --name namespaced \
  --toolchain-root "$TOOLCHAIN" \
  >"$SMOKE_ROOT/namespaced/stdout.json"

test -s "$SMOKE_ROOT/namespaced/namespaced.dylib"
jq -e '
  [.params[] | select(.displayName == "attack") |
    {name, displayName, group}] | sort_by(.name) == [
      {name: "op1.attack", displayName: "attack", group: "op1"},
      {name: "op2.attack", displayName: "attack", group: "op2"}
    ]
' "$SMOKE_ROOT/namespaced/namespaced.json" >/dev/null
jq -e '
  [.modDestinations[].name] | sort == ["op1.attack", "op2.attack"]
' "$SMOKE_ROOT/namespaced/namespaced.json" >/dev/null
```

Do not publish if any gate fails.

### 6. Publish safely

If this is the first target for a new version, create a draft. Always use the
full commit hash for `--target`; GitHub may reject an abbreviated hash.

```sh
cd "$DGEN_ROOT"
gh release create "$TAG" "$ARCHIVE" \
  --repo universalsequences/dgen-audio \
  --target "$UPSTREAM_COMMIT" \
  --title "DGenLisp v$VERSION" \
  --draft \
  --notes "macOS arm64 DGenLisp built from $UPSTREAM_COMMIT with $(swift --version | head -1). The target-qualified archive is stripped, ad-hoc signed, and passed scalar plus grouped-parameter standalone compile smokes against eseq's pinned arm64 DGen toolchain."
```

If another platform already created `$TAG`, verify that it targets the same
source commit and upload to that release instead of creating another tag:

```sh
test "$(gh release view "$TAG" \
  --repo universalsequences/dgen-audio \
  --json targetCommitish --jq .targetCommitish)" = "$UPSTREAM_COMMIT"
gh release upload "$TAG" "$ARCHIVE" \
  --repo universalsequences/dgen-audio
```

Do not use `--clobber` when adding macOS to an already-public release. An
existing asset with the same name is a conflict to investigate, not something
to overwrite.

For a new draft, inspect its assets before publishing. Recover an interrupted
upload on the same draft; do not create a second release:

```sh
gh release view "$TAG" --repo universalsequences/dgen-audio \
  --json isDraft,assets,targetCommitish,url
gh release upload "$TAG" "$ARCHIVE" \
  --repo universalsequences/dgen-audio --clobber
gh release edit "$TAG" --repo universalsequences/dgen-audio --draft=false
```

Verify the exact public bytes anonymously before changing eseq's lock:

```sh
export PUBLIC_DIR="$(mktemp -d "${TMPDIR:-/tmp}/dgenlisp-macos-public.XXXXXX")"
export PUBLIC_ARCHIVE="$PUBLIC_DIR/DGenLisp-macos-arm64.tar.gz"
curl --fail --location --silent --show-error \
  --output "$PUBLIC_ARCHIVE" \
  "https://github.com/universalsequences/dgen-audio/releases/download/$TAG/DGenLisp-macos-arm64.tar.gz"
export PUBLIC_SHA256="$(shasum -a 256 "$PUBLIC_ARCHIVE" | awk '{print $1}')"
test "$PUBLIC_SHA256" = "$ARCHIVE_SHA256"
printf 'public sha256: %s\n' "$PUBLIC_SHA256"
```

Published assets are immutable supply-chain inputs. If a public archive is
wrong, publish a corrected *new version* and re-pin; never silently replace an
asset already named by a committed lock file. A failed draft upload may be
replaced before publication.

### 7. Re-pin and verify through eseq's installed symlink

Only after the anonymous download passes, update the `macos-arm64` stanza in
`content/dgenlisp.lock`:

- `version`: `$TAG`
- `url`: the public `DGenLisp-macos-arm64.tar.gz` URL tested above
- `sha256`: `$PUBLIC_SHA256`
- `format`: `tar.gz`
- `executable_path`: `dgenlisp-macos-arm64/DGenLisp`
- `upstream_commit`: `$UPSTREAM_COMMIT`

Leave the `linux-x86_64` stanza unchanged. Then install with the production
fetcher and prove that the compiler finds resources through the installed
symlink, with no resource-path overrides masking a packaging error:

```sh
cd "$ESEQ_ROOT"
./scripts/fetch_dgenlisp.sh --target macos-arm64 --force
export INSTALLED="$ESEQ_ROOT/crates/sequencer/tools/DGenLisp-macos-arm64"
test -L "$INSTALLED"

export VERIFY_DIR="$(mktemp -d "${TMPDIR:-/tmp}/dgenlisp-macos-repin.XXXXXX")"
mkdir -p "$VERIFY_DIR/out"
cat >"$VERIFY_DIR/namespaced.lisp" <<'LISP'
(def mod1 (in 5 @name mod1 @modulator 1))
(param attack @group op1
  @default 0.25 @min 0 @max 1 @mod true @mod-mode additive)
(param attack @group op2
  @default 0.75 @min 0 @max 1 @mod true @mod-mode additive)
(out (+ (mod op1.attack) op2.attack~) 1)
LISP

env -u DGEN_RUNTIME_INCLUDE -u DGEN_BINARY_AUDIT_TOOL \
  "$INSTALLED" compile \
  "$VERIFY_DIR/namespaced.lisp" \
  --output "$VERIFY_DIR/out" \
  --name namespaced \
  --toolchain-root "$TOOLCHAIN" \
  >"$VERIFY_DIR/out/stdout.json"

test -s "$VERIFY_DIR/out/namespaced.c"
test -s "$VERIFY_DIR/out/namespaced.json"
test -s "$VERIFY_DIR/out/namespaced.dylib"
jq -e '
  [.params[] | select(.displayName == "attack") | .name] |
    sort == ["op1.attack", "op2.attack"]
' "$VERIFY_DIR/out/namespaced.json" >/dev/null
jq -e '
  [.modDestinations[].name] | sort == ["op1.attack", "op2.attack"]
' "$VERIFY_DIR/out/namespaced.json" >/dev/null
```

This final symlink-based compile is mandatory. A direct `$DIST/DGenLisp`
compile does not exercise the install seam and would not have caught the broken
v0.1.3 package, which looked valid until it searched for
`scripts/audit-dgen-dylib.sh` beside the symlink.

Review the lock diff, commit it separately from dgen-audio source changes, and
remove local release output when it is no longer needed:

```sh
cd "$ESEQ_ROOT"
git diff -- content/dgenlisp.lock
git diff --check
git add content/dgenlisp.lock
git commit -m "chore(dgenlisp): pin macOS compiler $TAG"

rm -rf "$DGEN_ROOT/out"
rm -rf "$PUBLIC_DIR"
rm -rf "$VERIFY_DIR"
```
