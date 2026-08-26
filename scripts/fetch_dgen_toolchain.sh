#!/usr/bin/env bash
# Fetch the hermetic DGen toolchain stage pinned in content/dgen-toolchain.lock.
#
# Sibling of scripts/fetch_dgenlisp.sh: that one installs the DGenLisp compiler,
# this one installs the clang/lld stage the compiler shells out to. A checkout
# needs BOTH before any DGen patch can be compiled; the compiler hard-fails
# naming this script when the stage is absent, and never falls back to a system
# compiler.
#
# Install layout (gitignored):
#   crates/sequencer/tools/dgen-toolchain/    the staged root itself, which is
#                                            what AppPaths passes as
#                                            --toolchain-root
#
# Stages are target-specific by construction. The archive is pinned per target,
# its sha256 is verified before anything is unpacked, and the staged
# VERSION.json must declare the host target -- a wrong-architecture stage is a
# hard error, not a warning, because the failure it would otherwise produce
# surfaces much later and much less legibly.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOCK_FILE="$ROOT_DIR/content/dgen-toolchain.lock"
TOOLS_DIR="$ROOT_DIR/crates/sequencer/tools"
DEST="$TOOLS_DIR/dgen-toolchain"
FORCE=0
TARGET=""

usage() {
  cat <<USAGE
Usage: $(basename "$0") [--target TARGET] [--force]

Downloads the hermetic DGen toolchain archive pinned in:
  $LOCK_FILE
verifies its sha256 against the pin (a mismatch is a hard error), and installs
the staged root at:
  $DEST

The target defaults to the current host. Targets are the target-qualified key
prefixes in the lock file (currently arm64-apple-macos and
x86_64-unknown-linux-gnu). Because the install path is not target-qualified,
only the host target can be installed; run this script on the target machine.

Options:
  --target TARGET   Fetch for TARGET. Must name the current host; it exists to
                    state the host target explicitly, not to cross-install.
  --force           Re-download and reinstall even if the installed stage
                    already matches the pin.
USAGE
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
    --force)
      FORCE=1
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

case "$(uname -s):$(uname -m)" in
  Darwin:arm64) HOST_TARGET="arm64-apple-macos" ;;
  Linux:x86_64) HOST_TARGET="x86_64-unknown-linux-gnu" ;;
  *) HOST_TARGET="" ;;
esac

if [[ -z "$TARGET" ]]; then
  if [[ -z "$HOST_TARGET" ]]; then
    echo "error: no DGen toolchain target is defined for $(uname -s)/$(uname -m)" >&2
    exit 1
  fi
  TARGET="$HOST_TARGET"
fi

# The stage installs to a single, non-target-qualified path, and installing
# replaces whatever is there. Cross-installing would therefore destroy the
# host's stage -- for arm64-apple-macos, one that has no published archive and
# can only be rebuilt from a local dgen-audio checkout -- and every subsequent
# DGen compile would hard-fail the VERSION.json target assertion. Refuse before
# anything is downloaded or removed.
if [[ "$TARGET" != "$HOST_TARGET" ]]; then
  echo "error: cannot install a $TARGET toolchain stage on ${HOST_TARGET:-this host ($(uname -s)/$(uname -m))}" >&2
  echo "The stage installs to the single host path:" >&2
  echo "  $DEST" >&2
  echo "so a foreign-target stage would overwrite the host's stage and break" >&2
  echo "every DGen compile. Run this script on the $TARGET machine instead." >&2
  echo "(scripts/fetch_dgenlisp.sh does install target-qualified and is safe to" >&2
  echo "run cross-target.)" >&2
  exit 1
fi

if [[ ! -f "$LOCK_FILE" ]]; then
  echo "error: DGen toolchain lock file not found: $LOCK_FILE" >&2
  exit 1
fi

lock_value() {
  awk -v key="target.$TARGET.$1" '$1 == key { print $2; exit }' "$LOCK_FILE"
}

URL="$(lock_value url)"
SHA256="$(lock_value archive_sha256)"
LLVM_VERSION="$(lock_value llvm_version)"

if [[ -z "$SHA256" ]]; then
  echo "error: no hermetic DGen toolchain is pinned for target $TARGET" >&2
  echo "  lock: $LOCK_FILE" >&2
  echo "DGen compiles cannot fall back to the system compiler." >&2
  exit 1
fi

# The pin hashes the archive, which is not kept after extraction; the stamp
# records which archive the installed stage came from. This check runs before
# the url check below so that a vendored target whose stage is already staged
# and matching succeeds idempotently instead of erroring on the missing url.
STAMP_FILE="$DEST/.dgen-toolchain-sha256"

if [[ "$FORCE" -eq 0 && -f "$STAMP_FILE" && "$(cat "$STAMP_FILE")" == "$SHA256" ]]; then
  echo "DGen toolchain $LLVM_VERSION for $TARGET is already installed and matches the lock:"
  echo "  $DEST"
  exit 0
fi

# A target may be pinned by identity without being published. That is not a
# broken lock file -- it is the vendor-locally route -- so say which one this
# target is on rather than reporting a missing field.
if [[ -z "$URL" ]]; then
  echo "error: target $TARGET is pinned in $LOCK_FILE but has no published url" >&2
  echo "This target is vendored from a local dgen-audio checkout instead:" >&2
  echo "  ./rebuild_dgenlisp_tool.sh" >&2
  echo "Publish a relocatable archive for $TARGET and add a" >&2
  echo "target.$TARGET.url entry to make it fetchable." >&2
  exit 1
fi

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

if ! command -v curl >/dev/null 2>&1; then
  echo "error: curl is required to fetch the DGen toolchain archive" >&2
  exit 1
fi

mkdir -p "$TOOLS_DIR"
DOWNLOAD="$TOOLS_DIR/.fetch-dgen-toolchain.$$"
STAGING="$DEST.staging.$$"
cleanup() {
  rm -f "$DOWNLOAD"
  rm -rf "$STAGING"
}
trap cleanup EXIT

echo "Fetching DGen toolchain $LLVM_VERSION for $TARGET..."
echo "  $URL"
if ! curl --fail --location --silent --show-error --output "$DOWNLOAD" "$URL"; then
  echo "error: download failed: $URL" >&2
  echo "(pinned in $LOCK_FILE)" >&2
  exit 1
fi

ACTUAL="$(sha256 "$DOWNLOAD")"
if [[ "$ACTUAL" != "$SHA256" ]]; then
  echo "error: sha256 mismatch for $URL" >&2
  echo "  pinned:     $SHA256  ($LOCK_FILE)" >&2
  echo "  downloaded: $ACTUAL" >&2
  echo "The download was discarded. If the pin is stale, update the lock file" >&2
  echo "deliberately and commit the change; never weaken this check." >&2
  exit 1
fi
echo "  sha256 verified: $ACTUAL"

mkdir -p "$STAGING"
tar -xzf "$DOWNLOAD" -C "$STAGING"

# The archive layout contract is dgen-audio's toolchain/LAYOUT.md: untarring
# yields a single top-level dgen-toolchain/ directory, which is the staged root.
STAGED_ROOT="$STAGING/dgen-toolchain"
if [[ ! -d "$STAGED_ROOT" ]]; then
  echo "error: archive did not contain a top-level dgen-toolchain/ directory" >&2
  echo "(see dgen-audio toolchain/LAYOUT.md)" >&2
  exit 1
fi

STAGED_TARGET="$(sed -n 's/.*"target": *"\([^"]*\)".*/\1/p' "$STAGED_ROOT/VERSION.json")"
if [[ "$STAGED_TARGET" != "$TARGET" ]]; then
  echo "error: archive targets ${STAGED_TARGET:-an unknown target (invalid VERSION.json)}," >&2
  echo "but $TARGET was requested. Toolchain stages are target-specific and" >&2
  echo "cannot be reused across architectures." >&2
  exit 1
fi

printf '%s\n' "$SHA256" > "$STAGED_ROOT/.dgen-toolchain-sha256"
rm -rf "$DEST"
mv "$STAGED_ROOT" "$DEST"

echo "Installed DGen toolchain:"
echo "  root: $DEST"
echo "  target: $STAGED_TARGET"
echo "  llvm_version: $LLVM_VERSION"
