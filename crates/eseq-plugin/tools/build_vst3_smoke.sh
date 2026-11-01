#!/bin/sh
# Build the VST3 smoke host against the VST3 SDK clap-wrapper vendors.
# Usage: crates/eseq-plugin/tools/build_vst3_smoke.sh [OUT]   (default: target/vst3_smoke)
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
out=${1:-$root/target/vst3_smoke}
sdk=$(ls -d "$HOME"/.cargo/registry/src/*/clap-wrapper-0.3.1/external/vst3sdk | head -1)
[ -d "$sdk" ] || { echo "VST3 SDK not found; run: cargo fetch -p eseq-plugin" >&2; exit 1; }
h="$sdk/public.sdk/source/vst/hosting"
clang++ -std=c++17 -fobjc-arc -O1 -w -DRELEASE=1 \
  -I"$sdk" -I"$sdk/public.sdk" -I"$sdk/pluginterfaces" \
  "$here/vst3_smoke.mm" \
  "$h/module.cpp" "$h/module_mac.mm" "$h/hostclasses.cpp" "$h/eventlist.cpp" \
  "$h/parameterchanges.cpp" "$h/processdata.cpp" "$h/pluginterfacesupport.cpp" \
  "$sdk/public.sdk/source/common/memorystream.cpp" "$sdk/public.sdk/source/common/commonstringconvert.cpp" \
  "$sdk/public.sdk/source/vst/utility/stringconvert.cpp" \
  "$sdk/public.sdk/source/vst/vstinitiids.cpp" \
  "$sdk/pluginterfaces/base/funknown.cpp" "$sdk/pluginterfaces/base/coreiids.cpp" \
  "$sdk/base/source/fobject.cpp" "$sdk/base/source/updatehandler.cpp" "$sdk/base/source/fdebug.cpp" \
  "$sdk/base/thread/source/flock.cpp" "$sdk/base/source/fstring.cpp" "$sdk/base/source/baseiids.cpp" \
  "$sdk/pluginterfaces/base/ustring.cpp" "$sdk/pluginterfaces/base/conststringtable.cpp" \
  -framework Cocoa -framework CoreFoundation -o "$out"
echo "$out"
