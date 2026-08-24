#!/usr/bin/env bash
# Fetch the DGenLisp compiler distribution pinned in content/dgenlisp.lock.
#
# DGenLisp binaries are not tracked in git; the lock file is the committed,
# reviewable record of which published distribution each target uses. This
# script is the one explicit place the repository reaches the network for the
# compiler: builds and tests never download anything themselves, they hard-fail
# with a message naming this script when the binary is absent.
#
# Install layout (crates/sequencer/tools/, all of it gitignored):
#   format executable  DGenLisp-<target>              the pinned binary itself
#   format tar.gz      DGenLisp-<target>.dist/        the extracted distribution
#                      DGenLisp-<target>              relative symlink to the
#                                                     compiler inside .dist/
# Either way the compiler is invoked at DGenLisp-<target>, the same path
# AppPaths has always resolved (crates/sequencer/src/app_paths/mod.rs). The
# distribution resolves its own headers/ABI/audit files relative to the real
# binary location, so the symlink is transparent to it.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOCK_FILE="$ROOT_DIR/content/dgenlisp.lock"
TOOLS_DIR="$ROOT_DIR/crates/sequencer/tools"
FORCE=0
TARGET=""

usage() {
  cat <<EOF
Usage: $(basename "$0") [--target TARGET] [--force]

Downloads the DGenLisp distribution pinned in:
  $LOCK_FILE
verifies its sha256 against the pin (a mismatch is a hard error), and installs
it under crates/sequencer/tools/ (gitignored).

The target defaults to the current host. Targets are the stanza names in the
lock file (currently macos-arm64 and linux-x86_64).

Options:
  --target TARGET   Fetch for TARGET instead of the current host.
  --force           Re-download and reinstall even if the installed
                    distribution already matches the pin.
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

if [[ -z "$TARGET" ]]; then
  case "$(uname -s)-$(uname -m)" in
    Darwin-arm64) TARGET="macos-arm64" ;;
    Linux-x86_64) TARGET="linux-x86_64" ;;
    *)
      echo "error: no DGenLisp distribution is published for host $(uname -s)-$(uname -m)" >&2
      echo "Pass --target explicitly to fetch for another platform." >&2
      exit 1
      ;;
  esac
fi

if [[ ! -f "$LOCK_FILE" ]]; then
  echo "error: lock file not found: $LOCK_FILE" >&2
  exit 1
fi

lock_field() {
  awk -v target="$TARGET" -v key="$1" '
    $1 == "target" { in_target = ($2 == target); next }
    in_target && $1 == key { print $2; exit }
  ' "$LOCK_FILE"
}

VERSION="$(lock_field version)"
URL="$(lock_field url)"
SHA256="$(lock_field sha256)"
FORMAT="$(lock_field format)"
EXECUTABLE_PATH="$(lock_field executable_path)"

for field in VERSION URL SHA256 FORMAT EXECUTABLE_PATH; do
  if [[ -z "${!field}" ]]; then
    echo "error: $LOCK_FILE has no complete \`target $TARGET\` stanza" >&2
    echo "(need version, url, sha256, format and executable_path; $field is missing)" >&2
    exit 1
  fi
done

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

TOOL_PATH="$TOOLS_DIR/DGenLisp-$TARGET"
DIST_DIR="$TOOL_PATH.dist"
STAMP_FILE="$DIST_DIR/.dgenlisp-sha256"

installed_matches_pin() {
  case "$FORMAT" in
    executable)
      [[ -f "$TOOL_PATH" && ! -L "$TOOL_PATH" ]] || return 1
      [[ "$(sha256 "$TOOL_PATH")" == "$SHA256" ]]
      ;;
    tar.gz)
      # The pin hashes the archive, which is not kept after extraction; the
      # stamp records which archive the .dist tree came from.
      [[ -f "$STAMP_FILE" && -x "$DIST_DIR/$EXECUTABLE_PATH" ]] || return 1
      [[ "$(cat "$STAMP_FILE")" == "$SHA256" ]] || return 1
      [[ "$(readlink "$TOOL_PATH" 2>/dev/null)" == "DGenLisp-$TARGET.dist/$EXECUTABLE_PATH" ]]
      ;;
    *)
      return 1
      ;;
  esac
}

if [[ "$FORCE" -eq 0 ]] && installed_matches_pin; then
  echo "DGenLisp $VERSION for $TARGET is already installed and matches the lock:"
  echo "  $TOOL_PATH"
  exit 0
fi

if ! command -v curl >/dev/null 2>&1; then
  echo "error: curl is required to fetch the DGenLisp distribution" >&2
  exit 1
fi

DOWNLOAD="$TOOLS_DIR/.fetch-dgenlisp.$$"
STAGING="$DIST_DIR.staging.$$"
cleanup() {
  rm -f "$DOWNLOAD"
  rm -rf "$STAGING"
}
trap cleanup EXIT

echo "Fetching DGenLisp $VERSION for $TARGET..."
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

case "$FORMAT" in
  executable)
    chmod 0755 "$DOWNLOAD"
    mv -f "$DOWNLOAD" "$TOOL_PATH"
    ;;
  tar.gz)
    mkdir -p "$STAGING"
    tar -xzf "$DOWNLOAD" -C "$STAGING"
    if [[ ! -f "$STAGING/$EXECUTABLE_PATH" ]]; then
      echo "error: $LOCK_FILE pins executable_path \`$EXECUTABLE_PATH\` but it is" >&2
      echo "absent from the $VERSION archive" >&2
      exit 1
    fi
    chmod 0755 "$STAGING/$EXECUTABLE_PATH"
    printf '%s\n' "$SHA256" > "$STAGING/.dgenlisp-sha256"
    rm -rf "$DIST_DIR"
    mv "$STAGING" "$DIST_DIR"
    rm -f "$DOWNLOAD"
    ln -sfn "DGenLisp-$TARGET.dist/$EXECUTABLE_PATH" "$TOOL_PATH"
    ;;
  *)
    echo "error: $LOCK_FILE pins an unsupported format \`$FORMAT\` for target $TARGET" >&2
    exit 1
    ;;
esac

if [[ ! -x "$TOOL_PATH" ]]; then
  echo "error: installed compiler is not executable: $TOOL_PATH" >&2
  exit 1
fi

echo "Installed DGenLisp $VERSION for $TARGET:"
echo "  $TOOL_PATH"
