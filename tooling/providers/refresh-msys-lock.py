#!/usr/bin/env python3
"""Maintenance only: pin the MSYS package and corresponding-source closure."""
import concurrent.futures
import hashlib
import io
import json
from pathlib import Path
import re
import tarfile
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
CACHE = ROOT / 'target/provider-sources/msys'
BASE = 'https://repo.msys2.org'
SEEDS = ['libzstd', 'msys2-runtime', 'bash', 'zsh', 'fish', 'coreutils', 'findutils', 'grep', 'sed', 'gawk']


def download(url, expected=None):
    target = CACHE / url.rsplit('/', 1)[1]
    target.parent.mkdir(parents=True, exist_ok=True)
    if not target.is_file():
        with urllib.request.urlopen(url, timeout=90) as response, tempfile.NamedTemporaryFile(dir=target.parent, delete=False) as output:
            while chunk := response.read(1024 * 1024): output.write(chunk)
            temporary = Path(output.name)
        temporary.replace(target)
    digest = hashlib.file_digest(target.open('rb'), 'sha256').hexdigest()
    if expected and digest != expected: raise ValueError(f'checksum mismatch: {url}')
    return dict(url=url, sha256=digest, bytes=target.stat().st_size)


def main():
    data = urllib.request.urlopen(BASE + '/msys/x86_64/msys.db', timeout=60).read()
    packages, providers = {}, {}
    with tarfile.open(fileobj=io.BytesIO(data)) as archive:
        for member in archive.getmembers():
            if not member.name.endswith('/desc'): continue
            text = archive.extractfile(member).read().decode()
            fields = {key: value.strip().splitlines() for key, value in re.findall(r'%([^%]+)%\n(.*?)(?=\n%|\Z)', text, re.S)}
            name = fields['NAME'][0]
            packages[name] = fields
            for alias in fields.get('PROVIDES', []): providers[re.split('[<>=]', alias)[0]] = name
    chosen, pending = {}, SEEDS.copy()
    while pending:
        name = pending.pop()
        if name not in packages: name = providers[name]
        if name in chosen: continue
        item = packages[name]
        chosen[name] = item
        pending.extend(re.split('[<>=]', value)[0] for value in item.get('DEPENDS', []))

    def pin(item):
        name, version = item['NAME'][0], item['VERSION'][0]
        binary = download(BASE + '/msys/x86_64/' + item['FILENAME'][0], item['SHA256SUM'][0])
        source = download(BASE + '/msys/sources/' + item['BASE'][0] + '-' + version.split(':')[-1] + '.src.tar.zst')
        print(f'pinned {name} {version}', flush=True)
        return dict(name=name, version=version, license=' AND '.join(value.removeprefix('spdx:') for value in item.get('LICENSE', [])),
                    binary=binary, source=source, dependencies=item.get('DEPENDS', []))

    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as executor:
        entries = list(executor.map(pin, (chosen[name] for name in sorted(chosen))))
    lock = dict(schemaVersion=1, distributor=BASE, databaseSha256=hashlib.sha256(data).hexdigest(),
                binaryArchitecture='x86_64', targets={'win32-x64': 'Windows 10 1809+', 'win32-arm64': 'Windows 11 with x64 emulation'}, packages=entries)
    Path(__file__).with_name('runtime-lock.msys-x64.json').write_text(json.dumps(lock, indent=2) + '\n')


if __name__ == '__main__': main()
