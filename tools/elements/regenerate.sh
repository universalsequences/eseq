#!/bin/bash
# Regenerates the Elements factory instrument from the original sources:
# the data tables in content/instruments/Synths/Elements/data, the generated
# modules (src/resonator.lisp, src/ominous.lisp) and the assembled dsp.lisp.
#   EURORACK=/path/to/pichenettes/eurorack tools/elements/regenerate.sh
set -e
cd "$(dirname "$0")"
: "${EURORACK:?set EURORACK to a pichenettes/eurorack checkout}"
export EURORACK
INSTRUMENT="$(cd ../.. && pwd)/content/instruments/Synths/Elements"
python3 tools/lut.py lut_sine lut_approx_svf_gain lut_approx_svf_g lut_4_decades \
  lut_accent_gain_coarse lut_accent_gain_fine lut_stiffness lut_env_increments \
  lut_env_expo lut_env_quartic lut_midi_to_f_high lut_midi_to_f_low \
  lut_fm_frequency_quantizer lut_detune_quantizer lut_svf_shift \
  smp_sample_data smp_noise_sample >/dev/null
python3 tools/gen_resonator.py
python3 tools/gen_ominous.py
python3 tools/assemble.py "$INSTRUMENT" --data-subdir data
