#!/usr/bin/env python3
import hashlib
import json
import platform
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]
DEST = ROOT / 'target/provider-runtime'
for name in ('bash', 'zsh', 'fish'):
    binary = DEST / 'bin' / name
    subprocess.run([str(binary), '--version'], check=True)
    libraries = subprocess.check_output(['ldd', str(binary)], text=True)
    # Private builds must not depend on a host-installed shell library.
    for line in libraries.splitlines():
        if '=>' in line:
            name = line.split('=>')[0].strip()
            if name not in ('libc.so.6', 'libm.so.6', 'libdl.so.2', 'libpthread.so.0', 'librt.so.1', 'libgcc_s.so.1', 'libutil.so.1'):
                raise RuntimeError(f'Unbundled dependency: {line}')
family = 'alpine' if Path('/etc/alpine-release').exists() else 'linux'
architecture = {'x86_64': 'x64', 'aarch64': 'arm64', 'armv7l': 'armhf'}[platform.machine()]
manifest = dict(schemaVersion=1, platform='linux', architecture=platform.machine(), target=f'{family}-{architecture}', sources=[dict(name='bash', version='5.3'), dict(name='zsh', version='5.9'), dict(name='fish', version='4.9.3'), dict(name='ncurses', version='6.6')],
                systemDependencies=['target libc', 'compiler runtime', 'POSIX utilities in /usr/bin and /bin'], files=[])
for file in sorted(DEST.rglob('*')):
    if file.is_file() and file.name != 'manifest.json':
        manifest['files'].append(dict(path=str(file.relative_to(DEST)), sha256=hashlib.sha256(file.read_bytes()).hexdigest()))
(DEST / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
