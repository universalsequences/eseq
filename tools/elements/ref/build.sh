#!/bin/bash
# Builds ref/elref, the C++ reference renderer, and ref/elbench, the C++ CPU
# benchmark (-O3, for tools/bench.py), from a pichenettes/eurorack checkout:
# EURORACK=/path/to/eurorack ref/build.sh
set -e
P=$(cd "$(dirname "$0")/.." && pwd)
: "${EURORACK:?set EURORACK to a pichenettes/eurorack checkout}"
clang++ -std=c++14 -O2 -DTEST -Wno-everything -I "$P/ref/override" -I "$EURORACK" -o "$P/ref/elref" "$P/ref/elref.cc" \
  "$EURORACK/elements/dsp/part.cc" "$EURORACK/elements/dsp/voice.cc" "$EURORACK/elements/dsp/exciter.cc" \
  "$EURORACK/elements/dsp/resonator.cc" "$EURORACK/elements/dsp/string.cc" "$EURORACK/elements/dsp/tube.cc" \
  "$EURORACK/elements/dsp/multistage_envelope.cc" "$EURORACK/elements/dsp/ominous_voice.cc" \
  "$EURORACK/elements/resources.cc" "$EURORACK/stmlib/dsp/units.cc" "$P/ref/random_det.cc"
clang++ -std=c++14 -O3 -DTEST -Wno-everything -I "$EURORACK" -o "$P/ref/elbench" "$P/ref/elbench.cc" \
  "$EURORACK/elements/dsp/part.cc" "$EURORACK/elements/dsp/voice.cc" "$EURORACK/elements/dsp/exciter.cc" \
  "$EURORACK/elements/dsp/resonator.cc" "$EURORACK/elements/dsp/string.cc" "$EURORACK/elements/dsp/tube.cc" \
  "$EURORACK/elements/dsp/multistage_envelope.cc" "$EURORACK/elements/dsp/ominous_voice.cc" \
  "$EURORACK/elements/resources.cc" "$EURORACK/stmlib/dsp/units.cc" "$EURORACK/stmlib/utils/random.cc"
