#!/usr/bin/env python3
"""Stage pinned MSYS runtimes. Release requires a separate target-host smoke test."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import urllib.request

ROOT = Path(__file__).resolve().parents[2]


def load_manifest_module():
    spec = importlib.util.spec_from_file_location('runtime_manifest', Path(__file__).with_name('runtime-manifest.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def fetch(spec, cache):
    path = cache / spec['url'].rsplit('/', 1)[1]
    if not path.is_file():
        with urllib.request.urlopen(spec['url'], timeout=90) as response, path.open('wb') as output:
            shutil.copyfileobj(response, output)
    with path.open('rb') as source: digest = hashlib.file_digest(source, 'sha256').hexdigest()
    if digest != spec['sha256']: raise ValueError(f'checksum mismatch: {path.name}')
    return path


def extract(archive, destination):
    # Python 3.14 supplies zstd and safe extraction on every build host.
    with tarfile.open(archive, 'r:zst') as source:
        members = [item for item in source.getmembers() if not item.name.startswith('.')]
        source.extractall(destination, members=members, filter='data')


def fish_vendor(dest, lock, cache):
    package=next(item for item in lock['packages'] if item['name']=='fish')
    source=fetch(package['source'],cache)
    work=ROOT/'target/provider-msys-fish-source'
    work.mkdir(parents=True,exist_ok=True)
    with tarfile.open(source,'r:zst') as archive:
        member=next(item for item in archive.getmembers() if item.name.endswith('/fish-4.9.3.tar.xz'))
        upstream=archive.extractfile(member).read()
    expected=json.loads(Path(__file__).with_name('runtime-sources.json').read_text())['fish']['sha256']
    if hashlib.sha256(upstream).hexdigest()!=expected: raise ValueError('MSYS Fish upstream source differs from pinned vendor input')
    archive_path=work/'fish-4.9.3.tar.xz'
    archive_path.write_bytes(upstream)
    with tarfile.open(archive_path) as archive: archive.extractall(work,filter='data')
    vendor=dest/'sources/fish-vendor'
    config=subprocess.check_output(['cargo','vendor','--locked','--manifest-path',str(work/'fish-4.9.3/Cargo.toml'),str(vendor)],text=True)
    (dest/'sources/fish-cargo-config.toml').write_text(config.replace(str(vendor),'vendor'))
    with tarfile.open(dest/'sources/fish-vendor.tar.xz','w:xz') as archive: archive.add(vendor,arcname='vendor')
    shutil.rmtree(vendor)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--target', choices=['win32-x64', 'win32-arm64'], required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    dest = args.output.resolve()
    if (dest / 'manifest.json').exists(): raise ValueError('output already contains a runtime; use a fresh staging directory')
    cache = ROOT / 'target/provider-sources/msys'
    cache.mkdir(parents=True, exist_ok=True)
    lock = json.loads(Path(__file__).with_name('runtime-lock.msys-x64.json').read_text())
    (dest / 'msys').mkdir(parents=True, exist_ok=True)
    (dest / 'sources').mkdir(exist_ok=True)
    sources = []
    for package in lock['packages']:
        binary = fetch(package['binary'], cache)
        archive = fetch(package['source'], cache)
        extract(binary, dest / 'msys')
        shutil.copy2(archive, dest / 'sources' / archive.name)
        sources.append(dict(name=package['name'], version=package['version'], license=package['license'],
            archives=[dict(**package['source'], path='sources/' + archive.name)], binary=package['binary']))
    for name in ('bundle-msys.py','runtime-manifest.py','runtime-sources.json','spdx-identifiers.json'):
        shutil.copy2(Path(__file__).with_name(name), dest/'sources'/name)
    shutil.copy2(Path(__file__).with_name('runtime-lock.msys-x64.json'), dest / 'sources/runtime-lock.msys-x64.json')
    # The 32-bit process-inspection helper is unrelated to managed completion workers.
    (dest/'msys/usr/libexec/getprocaddr32.exe').unlink(missing_ok=True)
    fish_vendor(dest, lock, cache)
    subprocess.run(['python3', str(Path(__file__).with_name('verify-pe.py')), str(dest)], check=True)
    shutil.copy2(Path(__file__).with_name('verify-pe.py'), dest/'sources/verify-pe.py')
    minimum = dict(windowsBuild=22000 if args.target.endswith('arm64') else 17763,
                   binaryArchitecture='x64', emulation='Windows x64' if args.target.endswith('arm64') else None)
    load_manifest_module().write(dest, args.target, sources, ['Windows system DLLs'], tested=False, minimum=minimum)
    print('Staged pinned MSYS runtime. Target-host worker validation remains required before VSIX packaging.')


if __name__ == '__main__': main()
