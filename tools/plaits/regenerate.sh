#!/bin/bash
# Regenerates the Plaits factory instrument from the original sources:
# the data tables in content/instruments/Synths/Plaits/data, the generated
# engine files, and the assembled dsp.lisp.
#   EURORACK=/path/to/pichenettes/eurorack tools/plaits/regenerate.sh
set -e
cd "$(dirname "$0")"
: "${EURORACK:?set EURORACK to a pichenettes/eurorack checkout}"
export EURORACK
INSTRUMENT="$(cd ../.. && pwd)/content/instruments/Synths/Plaits"
python3 tools/lut.py lut_fm_frequency_quantizer lut_fold lut_fold_2 lut_sine \
  lut_stiffness lut_svf_shift lut_lpc_excitation_pulse >/dev/null
python3 tools/lut_ws.py >/dev/null
python3 tools/wav_extract.py >/dev/null
python3 tools/speech_data.py >/dev/null
python3 tools/gen_sixop.py >/dev/null      # also writes the six-op patch data
python3 tools/gen_modal.py >/dev/null
python3 tools/gen_additive.py >/dev/null
python3 tools/gen_chordbank_lisp.py >/dev/null
python3 tools/assemble.py "$INSTRUMENT" --data-subdir data
