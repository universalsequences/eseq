# Publishing a Linux DGenLisp release

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
