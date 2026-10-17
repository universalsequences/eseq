#!/bin/bash
# Builds tools/plref_speech/plref_speech: ref/plref where, with PLREF_U=<u>
# set, Random::GetSample() returns +1 (u > 0.5) or -1 (deterministic LPC
# noise sign) and GetFloat() returns u.
set -e
P=$(cd "$(dirname "$0")/../.." && pwd)
SRCS=$(find "$EURORACK/plaits/dsp" -name '*.cc'; echo "$EURORACK/plaits/resources.cc" "$EURORACK/stmlib/dsp/units.cc" "$EURORACK/stmlib/dsp/atan.cc")
clang++ -std=c++14 -O2 -DTEST -Wno-everything -I "$P/tools/plref_speech" -I "$EURORACK" \
  $SRCS "$P/tools/plref_speech/random_s.cc" "$P/ref/plref.cc" -o "$P/tools/plref_speech/plref_speech"
