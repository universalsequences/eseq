#!/bin/sh
# Launch the isolated development checkout with an explicitly selected compiler.
set -eu
HEAT_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
: "${ESEQ_DGENLISP_TOOL:?Set ESEQ_DGENLISP_TOOL to the locally fixed Heat compiler}"
: "${ESEQ_DGEN_TOOLCHAIN_ROOT:?Set ESEQ_DGEN_TOOLCHAIN_ROOT to the pinned toolchain}"
if [ ! -x "$ESEQ_DGENLISP_TOOL" ]; then
    echo "Compiler is not executable: $ESEQ_DGENLISP_TOOL" >&2
    exit 1
fi
mkdir -p "$HEAT_ROOT/.local/instruments"
heat_link() {
    heat_target=$1
    heat_destination=$2
    if [ -e "$heat_destination" ] || [ -L "$heat_destination" ]; then
        if [ ! -L "$heat_destination" ] || [ "$(readlink "$heat_destination")" != "$heat_target" ]; then
            echo "Existing library entry will not be replaced: $heat_destination" >&2
            exit 1
        fi
    else
        ln -s "$heat_target" "$heat_destination"
    fi
}
heat_link ../../tools/heat/instrument "$HEAT_ROOT/.local/instruments/Heat Development"
heat_link ../../tools/heat/instrument.presets "$HEAT_ROOT/.local/instruments/Heat Development.presets"
cd "$HEAT_ROOT"
echo 'In the development app, add Heat Development from Instruments > Library.'
exec cargo run -p sequencer --bin metal_seq
