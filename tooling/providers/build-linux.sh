#!/bin/sh
# Run on the target libc/architecture; no cross-target assumptions.
set -eu
[ "$(uname -s)" = Linux ] || { echo 'Linux target host required' >&2; exit 1; }
repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
build="$repo/target/provider-linux-build"
prefix="$repo/target/provider-runtime"
mkdir -p "$build" "$prefix/sources" "$prefix/bin"
export SHUCKED_PROVIDER_BUILD="$build"
python3 "$repo/tooling/providers/fetch-linux.py"
tar -xf "$prefix/sources/bash-5.3.tar.gz" -C "$build"
tar -xf "$prefix/sources/zsh-5.9.tar.xz" -C "$build"
tar -xf "$prefix/sources/fish-4.9.3.tar.xz" -C "$build"
tar -xf "$prefix/sources/ncurses-6.6.tar.gz" -C "$build"
cd "$build/ncurses-6.6"
./configure --prefix="$build/ncurses" --without-shared --without-cxx --without-cxx-binding --without-ada --without-progs --without-tests --enable-widec
make -j "${SHUCKED_BUILD_JOBS:-4}"
make install
cd "$build/bash-5.3"
./configure --prefix="$prefix" --disable-nls --disable-readline --without-bash-malloc
make -j "${SHUCKED_BUILD_JOBS:-4}"
make install
cd "$build/zsh-5.9"
CPPFLAGS="-I$build/ncurses/include/ncursesw" LDFLAGS="-L$build/ncurses/lib" ./configure --prefix="$prefix" --disable-dynamic --enable-multibyte --with-tcsetpgrp=yes --with-term-lib=ncursesw
python3 - <<'PYMOD'
from pathlib import Path
import re
p = Path('config.modules')
p.write_text(re.sub(r'(name=zsh/(?:system|zpty|zselect) .*?)link=no', r'\1link=static', p.read_text()))
PYMOD
make -j "${SHUCKED_BUILD_JOBS:-4}"
make install.bin
cd "$build/fish-4.9.3"
cargo vendor --locked "$prefix/sources/fish-vendor" > "$prefix/sources/fish-cargo-config.toml"
cmake -S . -B build -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$prefix" -DFISH_USE_SYSTEM_PCRE2=OFF -DBUILD_TESTING=OFF -DBUILD_DOCUMENTATION=OFF
cmake --build build --parallel "${SHUCKED_BUILD_JOBS:-4}"
cmake --install build
python3 "$repo/tooling/providers/verify-linux.py"
