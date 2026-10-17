#!/bin/bash
# Builds ref/rref, the C++ reference renderer, from a pichenettes/eurorack
# checkout: EURORACK=/path/to/eurorack ref/build.sh
# ref/shim holds test copies: stmlib's Random (constant / file noise) and
# rings' plucker.h and string.cc reading the per-sample noise streams.
set -e
P=$(cd "$(dirname "$0")/.." && pwd)
: "${EURORACK:?set EURORACK to a pichenettes/eurorack checkout}"
clang++ -std=c++14 -O2 -DTEST -Wno-everything -I "$P/ref/shim" -I "$EURORACK" -o "$P/ref/rref" \
  "$P/ref/rref.cc" "$P/ref/shim/random_u.cc" "$P/ref/shim/string_noise.cc" \
  $(find "$EURORACK/rings/dsp" -name '*.cc' ! -name string.cc) \
  "$EURORACK/rings/resources.cc" "$EURORACK/stmlib/dsp/units.cc"
