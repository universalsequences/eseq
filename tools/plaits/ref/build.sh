#!/bin/bash
# Builds ref/plref, the C++ reference renderer, from a pichenettes/eurorack
# checkout: EURORACK=/path/to/eurorack ref/build.sh
set -e
P=$(cd "$(dirname "$0")/.." && pwd)
: "${EURORACK:?set EURORACK to a pichenettes/eurorack checkout}"
clang++ -std=c++14 -O2 -DTEST -Wno-everything -I "$EURORACK" -o "$P/ref/plref" "$P/ref/plref.cc" \
  $(find "$EURORACK/plaits/dsp" -name '*.cc') "$EURORACK/plaits/resources.cc" \
  "$EURORACK/stmlib/dsp/units.cc" "$EURORACK/stmlib/dsp/atan.cc" "$EURORACK/stmlib/utils/random.cc"
