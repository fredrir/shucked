#!/bin/sh
set -eu
prefix=${SHUCKED_PROVIDER_DEST:?}
build=${SHUCKED_PROVIDER_BUILD:?}
mkdir -p "$build"
tar -xf "$prefix/sources/ncurses-6.6.tar.gz" -C "$build"
cd "$build/ncurses-6.6"
./configure --prefix="$build/ncurses" --without-shared --without-debug --without-cxx --without-cxx-binding --without-ada --without-progs --without-tests --enable-widec
make -j "${SHUCKED_BUILD_JOBS:-2}"
make -C include install
make -C ncurses install
ln -sf libncursesw.a "$build/ncurses/lib/libncurses.a"
tar -xf "$prefix/sources/bash-5.3.tar.gz" -C "$build"
cd "$build/bash-5.3"
CPPFLAGS="-I$build/ncurses/include -I$build/ncurses/include/ncursesw" LDFLAGS="-L$build/ncurses/lib" ./configure --prefix="$prefix" --disable-nls --with-curses --enable-readline --enable-progcomp --without-bash-malloc
make -j "${SHUCKED_BUILD_JOBS:-2}"
make install
