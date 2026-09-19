#!/bin/sh
set -eu
repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
build="$repo/target/provider-build"
mkdir -p "$build"
curl --fail --location --max-time 60 https://www.zsh.org/pub/old/zsh-5.9.tar.xz -o "$build/zsh-5.9.tar.xz"
python3 - "$build/zsh-5.9.tar.xz" <<'PY'
import hashlib,sys
assert hashlib.sha256(open(sys.argv[1], 'rb').read()).hexdigest() == '9b8d1ecedd5b5e81fbf1918e876752a7dd948e05c1a0dba10ab863842d45acd5', 'Zsh source checksum mismatch'
PY
tar -xf "$build/zsh-5.9.tar.xz" -C "$build"
cd "$build/zsh-5.9"
CFLAGS="-O2 -Wno-error=implicit-int -Wno-error=implicit-function-declaration" ./configure --prefix="$build/runtime" --disable-dynamic --enable-multibyte --with-tcsetpgrp=yes
python3 - <<'PYMOD'
from pathlib import Path
import re
p = Path('config.modules')
p.write_text(re.sub(r'(name=zsh/(?:system|zpty|zselect) .*?)link=no', r'\1link=static', p.read_text()))
PYMOD
make -j "${SHUCKED_BUILD_JOBS:-4}"
make install.bin
