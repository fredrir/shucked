#!/bin/sh
# Run on the target libc/architecture; no cross-target assumptions.
set -eu
case "$(uname -s)" in Linux|Darwin) ;; *) echo 'Unix target host required' >&2; exit 1;; esac
repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
build="${SHUCKED_PROVIDER_BUILD:-$repo/target/provider-linux-build}"
prefix="${SHUCKED_PROVIDER_DEST:-$repo/target/provider-runtime}"
mkdir -p "$build" "$prefix/sources" "$prefix/bin"
export SHUCKED_PROVIDER_BUILD="$build" SHUCKED_PROVIDER_DEST="$prefix"
export CARGO_BUILD_JOBS="${SHUCKED_BUILD_JOBS:-2}"
python3 "$repo/tooling/providers/fetch-linux.py"
tar -xf "$prefix/sources/zsh-5.9.tar.xz" -C "$build"
tar -xf "$prefix/sources/fish-4.9.3.tar.xz" -C "$build"
"$repo/tooling/providers/rebuild-bash.sh"
cd "$build/zsh-5.9"
CFLAGS="-O2 -Wno-error=implicit-int -Wno-error=implicit-function-declaration -Wno-error=incompatible-pointer-types" CPPFLAGS="-I$build/ncurses/include -I$build/ncurses/include/ncursesw" LDFLAGS="-L$build/ncurses/lib" ./configure --prefix="$prefix" --disable-dynamic --enable-multibyte --with-tcsetpgrp=yes --with-term-lib=ncursesw
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
"$repo/tooling/providers/build-helpers.sh"
python3 "$repo/tooling/providers/verify-unix.py" --mode source
