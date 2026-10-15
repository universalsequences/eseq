#!/bin/bash
# Builds the Beads Processor reference renderer.
#   EURORACK=/path/to/pichenettes/eurorack tools/beads/ref/build.sh
set -e
cd "$(dirname "$0")"
: "${EURORACK:?set EURORACK to a pichenettes/eurorack checkout}"
E="$EURORACK"
clang++ -O2 -std=c++11 -DTEST -I"$E" -o "${OUT:-bfxref}" bfxref.cc \
  "$E/beads/dsp/processor.cc" "$E/beads/dsp/trigger_generator.cc" \
  "$E/beads/dsp/delay_controller.cc" "$E/beads/dsp/delay_clock.cc" \
  "$E/beads/dsp/mu_law.cc" "$E/beads/resources.cc" \
  "$E/stmlib/utils/random.cc" "$E/stmlib/dsp/units.cc"
