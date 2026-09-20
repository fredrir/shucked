#!/bin/sh
set -eu
repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
build="${SHUCKED_PROVIDER_BUILD:-$repo/target/provider-helper-build}"
prefix="${SHUCKED_PROVIDER_DEST:-$repo/target/provider-runtime}"
export SHUCKED_PROVIDER_DEST="$prefix"
export SOURCE_DATE_EPOCH=1751328000
mkdir -p "$build" "$prefix/helpers/bin"
python3 "$repo/tooling/providers/fetch-linux.py"
python3 - "$repo" "$build" "$prefix" <<'PY'
import hashlib,json,os,shutil,subprocess,sys
from pathlib import Path
repo,build,prefix=map(Path,sys.argv[1:])
metadata=json.loads((repo/'tooling/providers/runtime-sources.json').read_text())
for name in os.environ.get('SHUCKED_HELPER_COMPONENTS','coreutils,findutils,grep,gnu-sed,gawk').split(','):
    item=metadata[name]
    filename=item['url'].rsplit('/',1)[1]
    archive=prefix/'sources'/filename
    source=build/filename.removesuffix('.tar.xz').removesuffix('.tar.gz')
    stamp=source/'.shucked-installed'
    stamp_value=str(prefix)+hashlib.sha256((repo/'tooling/providers/build-helpers.sh').read_bytes()+json.dumps(item,sort_keys=True).encode()).hexdigest()
    if stamp.is_file() and stamp.read_text()==stamp_value: continue
    source_identity=hashlib.sha256(json.dumps(item,sort_keys=True).encode()).hexdigest()
    source_stamp=source/'.shucked-source-input'
    if source.is_dir() and (not source_stamp.is_file() or source_stamp.read_text()!=source_identity):
        shutil.rmtree(source)
    if not source.is_dir():
        subprocess.run(['tar','-xf',str(archive),'-C',str(build)],check=True)
        for patch in item.get('patches',[]):
            patchfile=prefix/'sources'/patch['url'].split('?',1)[0].rsplit('/',1)[1]
            subprocess.run(['patch','-'+patch['strip'],'-i',str(patchfile)],cwd=source,check=True)
        source_stamp.write_text(source_identity)
    if (source/'Makefile').is_file():
        subprocess.run(['make','clean'],cwd=source,check=True,stdout=subprocess.DEVNULL)
    flags=['--prefix='+str(prefix/'helpers'),'--disable-nls','--disable-dependency-tracking','--without-selinux']
    flags+= {'coreutils':['--without-libgmp','--without-libcap','--without-selinux','--without-openssl'],
             'grep':['--disable-perl-regexp'],
             'gawk':['--disable-mpfr','--without-readline','--disable-extensions']}.get(name,[])
    subprocess.run(['./configure',*flags],cwd=source,check=True)
    subprocess.run(['make','MAKEINFO=true','-j',os.environ.get('SHUCKED_BUILD_JOBS','4')],cwd=source,check=True)
    subprocess.run(['make','MAKEINFO=true','install-exec'],cwd=source,check=True)
    license_dir=prefix/'sources'/'licenses'/name
    license_dir.mkdir(parents=True,exist_ok=True)
    for filename in ['COPYING','COPYING.LESSER','LICENSE']:
        if (source/filename).is_file():
            import shutil
            shutil.copy2(source/filename,license_dir/filename)
    stamp.write_text(stamp_value)
PY

python3 - "$prefix" <<'PYRELOCATE'
from pathlib import Path
import shutil,sys
root=Path(sys.argv[1])/'helpers'
for name,flag in [('egrep','-E'),('fgrep','-F')]:
    path=root/'bin'/name
    path.write_text('#!/bin/sh\nexec "${0%/*}/grep" '+flag+' "$@"\n')
    path.chmod(0o755)
library=root/'libexec/coreutils/libstdbuf.so'
if library.is_file(): shutil.copy2(library,root/'bin/libstdbuf.so')
PYRELOCATE
