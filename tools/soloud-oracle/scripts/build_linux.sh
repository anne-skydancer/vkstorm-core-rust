#!/bin/bash
# Builds the SoLoud oracle and the asset-decode oracle on Linux.
#
#   build_linux.sh <soloud-src> <llaudio-dir> <ogg-src> <vorbis-src> <outdir> [gcc-version]
#
# <soloud-src>: SoLoud e82fd32c1f with the 3p-soloud patches applied (the
#   3p-soloud build's _deps/soloud_source-src); <llaudio-dir>: the viewer's
#   indra/llaudio (for the unmodified llwindgen.h); <ogg-src>/<vorbis-src>:
#   libogg 1.3.5 / libvorbis 1.3.7 sources as the 3p-ogg_vorbis package uses.
# SoLoud is built as the 3p package's CMake Release build would (-O3
# -DNDEBUG), null backend only (backends do not touch mixing); libogg and
# libvorbis with the package's Linux flags.
set -euo pipefail
SOLOUD=$1 LLAUDIO=$2 OGG=$3 VORBIS=$4 OUT=$5 V=${6:-14}
C="$(cd "$(dirname "$0")/../c" && pwd)"
mkdir -p "$OUT/obj"
for f in "$SOLOUD"/src/core/*.cpp "$SOLOUD"/src/audiosource/wav/soloud_wav.cpp \
         "$SOLOUD"/src/audiosource/wav/dr_impl.cpp "$SOLOUD"/src/backend/null/soloud_null.cpp; do
  g++-$V -O3 -DNDEBUG -DWITH_NULL=1 -std=c++11 -I"$SOLOUD/include" -c "$f" -o "$OUT/obj/$(basename "$f").o"
done
gcc-$V -O3 -DNDEBUG -I"$SOLOUD/include" -c "$SOLOUD/src/audiosource/wav/stb_vorbis.c" -o "$OUT/obj/stb_vorbis.o"
rm -f "$OUT/libsoloud.a"
ar rcs "$OUT/libsoloud.a" "$OUT"/obj/*.o
g++-$V -O2 -std=c++17 -DNDEBUG -I"$C/shim" -I"$LLAUDIO" -I"$SOLOUD/include" "$C/soloud_oracle.cpp" \
  -o "$OUT/soloud_oracle" "$OUT/libsoloud.a" -lpthread -ldl -lm

L="$OUT/clibs"
FL="-m64 -O3 -g -fPIC -DPIC -DNDEBUG"
common=(-DCMAKE_BUILD_TYPE=Release -DCMAKE_C_COMPILER=gcc-$V "-DCMAKE_C_FLAGS=$FL" -DCMAKE_POLICY_VERSION_MINIMUM=3.5
        -DBUILD_SHARED_LIBS=OFF "-DCMAKE_INSTALL_PREFIX=$L" -DCMAKE_INSTALL_LIBDIR=lib)
cmake -S "$OGG" -B "$OUT/ogg-build" "${common[@]}"
cmake --build "$OUT/ogg-build" -j && cmake --install "$OUT/ogg-build"
cmake -S "$VORBIS" -B "$OUT/vorbis-build" "${common[@]}" "-DCMAKE_PREFIX_PATH=$L"
cmake --build "$OUT/vorbis-build" -j && cmake --install "$OUT/vorbis-build"
gcc-$V -O2 -I"$L/include" "$C/asset_oracle.c" -o "$OUT/asset_oracle" \
  "$L/lib/libvorbisfile.a" "$L/lib/libvorbis.a" "$L/lib/libogg.a" -lm
