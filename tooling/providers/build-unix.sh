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
p.write_text(re.sub(r'(name=zsh/(?:system|zpty|zselect|regex) .*?)link=no', r'\1link=static', p.read_text()))
PYMOD
make -j "${SHUCKED_BUILD_JOBS:-4}"
make install.bin
if [ -n "${SHUCKED_FISH_PREBUILT:-}" ] && [ -f "$SHUCKED_FISH_PREBUILT/source.json" ]; then
python3 - "$repo" "$prefix" "$SHUCKED_FISH_PREBUILT" <<'PYFISH'
import hashlib,json,platform,shutil,sys
from pathlib import Path
repo,prefix,prebuilt=map(Path,sys.argv[1:])
metadata=json.loads((prebuilt/'source.json').read_text())
source=json.loads((repo/'tooling/providers/runtime-sources.json').read_text())['fish']
arch={'x86_64':'x86_64','aarch64':'aarch64','arm64':'aarch64'}[platform.machine()]
family='musl' if Path('/etc/alpine-release').exists() else 'gnu'
expected=arch+'-unknown-linux-'+family
if platform.system()!='Linux' or metadata.get('target')!=expected or metadata.get('sourceSha256')!=source['sha256']:
    raise ValueError('prebuilt Fish source/target mismatch')
for name,key in [('bin/fish','binarySha256'),('sources/fish-vendor.tar.xz','vendorSha256')]:
    if hashlib.sha256((prebuilt/name).read_bytes()).hexdigest()!=metadata.get(key):
        raise ValueError('prebuilt Fish artifact checksum mismatch: '+name)
shutil.copy2(prebuilt/'bin/fish',prefix/'bin/fish')
shutil.copytree(prebuilt/'share/fish',prefix/'share/fish',dirs_exist_ok=True)
for name in ('fish-vendor.tar.xz','fish-cargo-config.toml'):
    shutil.copy2(prebuilt/'sources'/name,prefix/'sources'/name)
shutil.copy2(prebuilt/'source.json',prefix/'sources/fish-cross-build.json')
PYFISH
else
cd "$build/fish-4.9.3"
cargo vendor --locked "$prefix/sources/fish-vendor" > "$prefix/sources/fish-cargo-config.toml"
cmake -S . -B build -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$prefix" -DFISH_USE_SYSTEM_PCRE2=OFF -DBUILD_TESTING=OFF -DBUILD_DOCUMENTATION=OFF
cmake --build build --parallel "${SHUCKED_BUILD_JOBS:-4}"
cmake --install build
fi
"$repo/tooling/providers/build-helpers.sh"
python3 "$repo/tooling/providers/verify-unix.py" --mode source
