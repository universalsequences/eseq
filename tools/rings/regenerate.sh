#!/bin/bash
# Regenerates the Rings factory instrument from the original sources: the
# data tables in content/instruments/Synths/Rings/data, the generated Lisp
# sources, and the assembled dsp.lisp.
#   EURORACK=/path/to/pichenettes/eurorack tools/rings/regenerate.sh
set -e
cd "$(dirname "$0")"
: "${EURORACK:?set EURORACK to a pichenettes/eurorack checkout}"
export EURORACK
INSTRUMENT="$(cd ../.. && pwd)/content/instruments/Synths/Rings"
python3 tools/lut.py lut_sine lut_4_decades lut_svf_shift lut_stiffness lut_fm_frequency_quantizer >/dev/null
python3 tools/gen_tables.py >/dev/null
python3 tools/gen_modal.py >/dev/null
python3 tools/gen_strings.py >/dev/null
python3 tools/gen_peace.py >/dev/null
python3 tools/assemble.py "$INSTRUMENT" --data-subdir data
