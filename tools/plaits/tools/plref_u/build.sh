#!/bin/bash
# Builds tools/plref_u/plref_u: ref/plref with a PLREF_U constant-noise override.
set -e
P=$(cd "$(dirname "$0")/../.." && pwd)
SRCS=$(find "$EURORACK/plaits/dsp" -name '*.cc'; echo "$EURORACK/plaits/resources.cc" "$EURORACK/stmlib/dsp/units.cc" "$EURORACK/stmlib/dsp/atan.cc")
clang++ -std=c++14 -O2 -DTEST -Wno-everything -I "$P/tools/plref_u" -I "$EURORACK" \
  $SRCS "$P/tools/plref_u/random_u.cc" "$P/ref/plref.cc" -o "$P/tools/plref_u/plref_u"
