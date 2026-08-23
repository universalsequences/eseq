#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DGEN_REPO="${DGEN_REPO:-$HOME/code/dgen-audio}"
PRODUCT_NAME="DGenLisp"
SOURCE_BIN="$DGEN_REPO/.build/release/$PRODUCT_NAME"
TOOLCHAIN_DEST="$ROOT_DIR/crates/sequencer/tools/dgen-toolchain"
LOCK_FILE="$ROOT_DIR/content/dgen-toolchain.lock"
ALLOW_UNCHANGED=0
UPDATE_LOCK=0
TARGET=""

usage() {
  cat <<EOF
Usage: $(basename "$0") [--target TARGET] [--allow-unchanged] [--update-lock]

Rebuilds the release DGenLisp binary from:
  $DGEN_REPO

Installs it under crates/sequencer/tools using the selected target suffix.
The target defaults to the current host. Supported targets are:
  macos-arm64
  linux-x86_64

Also stages the hermetic dgen toolchain archive
(\$DGEN_REPO/.toolchain/dgen-toolchain-*.tar.gz) into:
  $TOOLCHAIN_DEST        (gitignored, ~147 MB)
and records the vendored archive's identity in the committed lock file:
  $LOCK_FILE

Environment:
  DGEN_REPO=/path/to/dgen-audio
                            Override the Swift dgen repo path.

Options:
  --target TARGET           Select the host target and destination suffix.
  --allow-unchanged         Copy the release artifact even if its timestamp did
                            not advance during this run.
  --update-lock             Accept a toolchain archive whose sha256 differs
                            from the committed lock file and rewrite the lock.
                            Without this flag a divergent archive is an error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --target)
      if [[ $# -lt 2 ]]; then
        echo "error: --target requires a value" >&2
        exit 2
      fi
      TARGET="$2"
      shift 2
      ;;
    --allow-unchanged)
      ALLOW_UNCHANGED=1
      shift
      ;;
    --update-lock)
      UPDATE_LOCK=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) HOST_TARGET="macos-arm64" ;;
  Linux-x86_64) HOST_TARGET="linux-x86_64" ;;
  *)
    echo "error: unsupported build host: $(uname -s)-$(uname -m)" >&2
    exit 1
    ;;
esac

TARGET="${TARGET:-$HOST_TARGET}"
if [[ "$TARGET" != "$HOST_TARGET" ]]; then
  echo "error: target $TARGET cannot be built on host $HOST_TARGET" >&2
  echo "Build each binary on its matching host to avoid mislabeled artifacts." >&2
  exit 1
fi

DEST_BIN="$ROOT_DIR/crates/sequencer/tools/$PRODUCT_NAME-$TARGET"
BUILD_ARGS=(-c release --product "$PRODUCT_NAME")
case "$TARGET" in
  macos-arm64)
    STRIP_ARGS=(-x)
    ;;
  linux-x86_64)
    BUILD_ARGS+=(
      -Xswiftc -static-stdlib
      -Xswiftc -Xclang-linker
      -Xswiftc -static-libgcc
    )
    STRIP_ARGS=(--strip-all)
    ;;
  *)
    echo "error: unsupported target: $TARGET" >&2
    exit 1
    ;;
esac

mtime() {
  local path="$1"
  if [[ ! -e "$path" ]]; then
    echo 0
  elif [[ "$(uname -s)" == "Darwin" ]]; then
    stat -f '%m' "$path"
  else
    stat -c '%Y' "$path"
  fi
}

describe_file() {
  local path="$1"
  if [[ ! -e "$path" ]]; then
    echo "missing $path"
  elif [[ "$(uname -s)" == "Darwin" ]]; then
    stat -f '%Sm %z %N' "$path"
  else
    stat -c '%y %s %n' "$path"
  fi
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

if [[ ! -d "$DGEN_REPO" ]]; then
  echo "error: dgen repo not found: $DGEN_REPO" >&2
  exit 1
fi

if [[ ! -f "$DGEN_REPO/Package.swift" ]]; then
  echo "error: Package.swift not found in dgen repo: $DGEN_REPO" >&2
  exit 1
fi

if [[ ! -d "$(dirname "$DEST_BIN")" ]]; then
  echo "error: destination directory not found: $(dirname "$DEST_BIN")" >&2
  exit 1
fi

SOURCE_MTIME_BEFORE="$(mtime "$SOURCE_BIN")"
SOURCE_DESC_BEFORE="$(describe_file "$SOURCE_BIN")"
DEST_DESC_BEFORE="$(describe_file "$DEST_BIN")"

echo "Release artifact before build:"
echo "  $SOURCE_DESC_BEFORE"
echo
echo "Installed tool before copy:"
echo "  $DEST_DESC_BEFORE"
echo
echo "Building $PRODUCT_NAME release binary in $DGEN_REPO..."
(
  cd "$DGEN_REPO"
  swift build "${BUILD_ARGS[@]}"
)

if [[ ! -x "$SOURCE_BIN" ]]; then
  echo "error: release binary not found or not executable after build: $SOURCE_BIN" >&2
  exit 1
fi

SOURCE_MTIME_AFTER="$(mtime "$SOURCE_BIN")"
if [[ "$ALLOW_UNCHANGED" -eq 0 && "$SOURCE_MTIME_AFTER" -le "$SOURCE_MTIME_BEFORE" ]]; then
  echo "error: release artifact timestamp did not advance during build" >&2
  echo "before: $SOURCE_DESC_BEFORE" >&2
  echo "after:  $(describe_file "$SOURCE_BIN")" >&2
  echo "Use --allow-unchanged only when intentionally copying an existing artifact." >&2
  exit 1
fi

echo
echo "Release artifact after build:"
echo "  $(describe_file "$SOURCE_BIN")"

SOURCE_HASH="$(sha256 "$SOURCE_BIN")"
echo "  sha256 $SOURCE_HASH"
echo
echo "Installing to $DEST_BIN..."
install -m 0755 "$SOURCE_BIN" "$DEST_BIN"

if [[ ! -x "$DEST_BIN" ]]; then
  echo "error: installed binary is not executable: $DEST_BIN" >&2
  exit 1
fi

COPIED_HASH="$(sha256 "$DEST_BIN")"
if [[ "$SOURCE_HASH" != "$COPIED_HASH" ]]; then
  echo "error: installed binary checksum does not match release artifact" >&2
  echo "source: $SOURCE_HASH $SOURCE_BIN" >&2
  echo "dest:   $COPIED_HASH $DEST_BIN" >&2
  exit 1
fi

strip "${STRIP_ARGS[@]}" "$DEST_BIN"
DEST_HASH="$(sha256 "$DEST_BIN")"
echo "Installed and stripped tool:"
echo "  $(describe_file "$DEST_BIN")"
echo "  sha256 $DEST_HASH"

# ── Stage the hermetic toolchain ──
# The archive layout contract is $DGEN_REPO/toolchain/LAYOUT.md: untarring
# yields a single top-level dgen-toolchain/ directory, which is the root
# passed to `DGenLisp compile --toolchain-root`.

echo
echo "Locating toolchain archive in $DGEN_REPO/.toolchain..."
ARCHIVES=("$DGEN_REPO"/.toolchain/dgen-toolchain-*.tar.gz)
if [[ ${#ARCHIVES[@]} -eq 0 || ! -e "${ARCHIVES[0]}" ]]; then
  echo "error: no toolchain archive found: $DGEN_REPO/.toolchain/dgen-toolchain-*.tar.gz" >&2
  echo "Build one in the dgen repo first (scripts/build-toolchain.sh)." >&2
  exit 1
fi
if [[ ${#ARCHIVES[@]} -gt 1 ]]; then
  echo "error: multiple toolchain archives found; remove all but one:" >&2
  printf '  %s\n' "${ARCHIVES[@]}" >&2
  exit 1
fi
ARCHIVE="${ARCHIVES[0]}"
ARCHIVE_NAME="$(basename "$ARCHIVE")"
echo "  $(describe_file "$ARCHIVE")"

ARCHIVE_HASH="$(sha256 "$ARCHIVE")"
echo "  sha256 $ARCHIVE_HASH"

# The lock file is the committed, reviewable record of which toolchain is
# vendored. A divergent archive sha requires an explicit --update-lock.
if [[ -f "$LOCK_FILE" ]]; then
  LOCKED_HASH="$(awk '$1 == "archive_sha256" {print $2}' "$LOCK_FILE")"
  if [[ -n "$LOCKED_HASH" && "$LOCKED_HASH" != "$ARCHIVE_HASH" ]]; then
    if [[ "$UPDATE_LOCK" -eq 0 ]]; then
      echo "error: toolchain archive sha256 differs from the committed lock file" >&2
      echo "  locked:  $LOCKED_HASH  ($LOCK_FILE)" >&2
      echo "  archive: $ARCHIVE_HASH  ($ARCHIVE)" >&2
      echo "Re-run with --update-lock to vendor the new toolchain (and commit the lock change)." >&2
      exit 1
    fi
    echo "Lock update requested (--update-lock):"
    echo "  old sha256: $LOCKED_HASH"
    echo "  new sha256: $ARCHIVE_HASH"
  fi
fi

echo
echo "Staging toolchain into $TOOLCHAIN_DEST..."
STAGING_TMP="$TOOLCHAIN_DEST.staging.$$"
rm -rf "$STAGING_TMP"
mkdir -p "$STAGING_TMP"
tar -xzf "$ARCHIVE" -C "$STAGING_TMP"

STAGED_ROOT="$STAGING_TMP/dgen-toolchain"
if [[ ! -d "$STAGED_ROOT" ]]; then
  echo "error: archive did not contain a top-level dgen-toolchain/ directory (see toolchain/LAYOUT.md)" >&2
  rm -rf "$STAGING_TMP"
  exit 1
fi

# Required staged files per LAYOUT.md; an incomplete stage is useless (the
# compiler preflight-checks it and hard-errors), so fail here instead.
REQUIRED_FILES=(
  "VERSION.json"
  "abi/exports-v1.txt"
  "abi/libsystem-symbols-v1.txt"
  "include/dgen_runtime.h"
  "bin/dgen-clang"
  "bin/ld64.lld"
  "lib/clang/20/lib/darwin/libclang_rt.builtins.a"
  "lib/libSystem.tbd"
)
for rel in "${REQUIRED_FILES[@]}"; do
  if [[ ! -f "$STAGED_ROOT/$rel" ]]; then
    echo "error: staged toolchain is missing required file: $rel" >&2
    rm -rf "$STAGING_TMP"
    exit 1
  fi
done

LLVM_VERSION="$(sed -n 's/.*"llvm_version": *"\([^"]*\)".*/\1/p' "$STAGED_ROOT/VERSION.json")"
DGEN_ABI_VERSION="$(sed -n 's/.*"dgen_abi_version": *\([0-9][0-9]*\).*/\1/p' "$STAGED_ROOT/VERSION.json")"
if [[ -z "$LLVM_VERSION" || -z "$DGEN_ABI_VERSION" ]]; then
  echo "error: could not read llvm_version / dgen_abi_version from staged VERSION.json" >&2
  rm -rf "$STAGING_TMP"
  exit 1
fi

# Replace any prior stage atomically: untarred to a temp sibling above, now
# swap the verified root into place.
rm -rf "$TOOLCHAIN_DEST"
mv "$STAGED_ROOT" "$TOOLCHAIN_DEST"
rm -rf "$STAGING_TMP"

cat > "$LOCK_FILE" <<EOF
# Vendored dgen toolchain (staged by rebuild_dgenlisp_tool.sh into
# tools/dgen-toolchain/, which is gitignored). This lock file is committed
# and is the reviewable record of which toolchain archive is vendored.
archive $ARCHIVE_NAME
archive_sha256 $ARCHIVE_HASH
llvm_version $LLVM_VERSION
dgen_abi_version $DGEN_ABI_VERSION
EOF

echo "Staged toolchain:"
echo "  root: $TOOLCHAIN_DEST"
echo "  llvm_version: $LLVM_VERSION"
echo "  dgen_abi_version: $DGEN_ABI_VERSION"
echo
echo "Lock file ($LOCK_FILE):"
sed 's/^/  /' "$LOCK_FILE"
echo
echo "Done."
