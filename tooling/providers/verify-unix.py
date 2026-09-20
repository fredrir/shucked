#!/usr/bin/env python3
"""Verify native engines, private helpers, source archives, and relocation."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import struct
import tarfile
import tempfile
import urllib.parse

ROOT = Path(__file__).resolve().parents[2]
DEST = Path(os.environ.get('SHUCKED_PROVIDER_DEST', ROOT / 'target/provider-runtime')).resolve()
SPEC = importlib.util.spec_from_file_location('runtime_manifest', Path(__file__).with_name('runtime-manifest.py'))
manifest = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(manifest)


def source_archive(spec, parent='sources'):
    filename = urllib.parse.urlparse(spec['url']).path.rsplit('/', 1)[-1]
    path = DEST / parent / filename
    if not path.is_file() or manifest.sha256(path) != spec['sha256']:
        raise ValueError(f'source checksum mismatch: {path}')
    return dict(url=spec['url'], sha256=spec['sha256'], path=path.relative_to(DEST).as_posix())


def sources(mode):
    metadata = json.loads(Path(__file__).with_name('runtime-sources.json').read_text())
    result = []
    if mode == 'homebrew':
        lock = json.loads(Path(__file__).with_name('runtime-lock.darwin-'+platform.machine()+'.json').read_text())
        for item in lock:
            spec = dict(url=item['urls']['stable']['url'], sha256=item['urls']['stable']['checksum'])
            result.append(dict(name=item['name'], version=item['installed'][0]['version'], license=item['license'],
                archives=[source_archive(archive, 'sources/'+item['name']) for archive in [spec, *item['patches']]]))
        zsh = metadata['zsh']
        result.append(dict(name='zsh', version=zsh['version'], license=zsh['license'], archives=[source_archive(zsh, 'sources/zsh')]))
        metadata = {name: metadata[name] for name in ('coreutils','findutils','grep','gnu-sed','gawk')}
    for name, item in metadata.items():
        result.append(dict(name=name, version=item['version'], license=item['license'],
            archives=[source_archive(spec) for spec in [item, *item.get('patches', [])]]))
    return result


def elf_is_static(path, header):
    endian='<' if header[5]==1 else '>'
    wide=header[4]==2
    phoff=struct.unpack_from(endian+('Q' if wide else 'I'),header,32 if wide else 28)[0]
    size,count=struct.unpack_from(endian+'HH',header,54 if wide else 42)
    length=path.stat().st_size
    if count>4096 or phoff+size*count>length: raise ValueError('invalid ELF program headers')
    with path.open('rb') as file:
        for index in range(count):
            file.seek(phoff+size*index)
            entry=file.read(size)
            kind=struct.unpack_from(endian+'I',entry)[0]
            if kind==3: return False
            if kind!=2: continue
            offset=struct.unpack_from(endian+('Q' if wide else 'I'),entry,8 if wide else 4)[0]
            dynamic_size=struct.unpack_from(endian+('Q' if wide else 'I'),entry,32 if wide else 16)[0]
            if offset+dynamic_size>length: raise ValueError('invalid ELF dynamic section')
            file.seek(offset)
            for _ in range(dynamic_size//(16 if wide else 8)):
                dynamic=file.read(16 if wide else 8)
                tag=struct.unpack_from(endian+('q' if wide else 'i'),dynamic)[0]
                if tag==0: break
                if tag==1: return False
    return True


def verify_links():
    for path in DEST.rglob('*'):
        if not path.is_file() or '/sources/' in str(path): continue
        with path.open('rb') as file: header = file.read(64)
        magic = header[:4]
        if magic == b'\x7fELF':
            expected={'x86_64':62,'aarch64':183,'arm64':183,'armv7l':40}[platform.machine()]
            if int.from_bytes(header[18:20], 'little' if header[5]==1 else 'big')!=expected:
                raise ValueError(f'ELF architecture mismatch: {path}')
            if elf_is_static(path,header): continue
            output = subprocess.check_output(['ldd', str(path)], text=True, stderr=subprocess.STDOUT)
            for line in output.splitlines():
                if '=>' not in line: continue
                library = line.split('=>')[0].strip()
                if library not in ('libc.so.6','libm.so.6','libdl.so.2','libpthread.so.0','librt.so.1','libgcc_s.so.1','libutil.so.1','libc.musl-aarch64.so.1','libc.musl-x86_64.so.1'):
                    raise ValueError(f'unbundled runtime dependency: {path}: {line}')
        elif magic in (b'\xcf\xfa\xed\xfe', b'\xce\xfa\xed\xfe'):
            expected={'x86_64':0x01000007,'arm64':0x0100000c}[platform.machine()]
            if int.from_bytes(header[4:8],'little')!=expected: raise ValueError(f'Mach-O architecture mismatch: {path}')
            dependencies = subprocess.check_output(['otool', '-L', str(path)], text=True).splitlines()[1:]
            if int.from_bytes(header[12:16], 'little') == 6: dependencies = dependencies[1:]
            for line in dependencies:
                dependency = line.strip().split(' (')[0]
                if dependency.startswith(('/usr/lib/', '/System/Library/')): continue
                if dependency.startswith('@loader_path/'):
                    resolved=path.parent/dependency.removeprefix('@loader_path/')
                elif dependency.startswith('@executable_path/'):
                    resolved=DEST/'bin'/dependency.removeprefix('@executable_path/')
                else: resolved=None
                if resolved is not None and resolved.is_file() and resolved.resolve().is_relative_to(DEST): continue
                raise ValueError(f'unbundled runtime dependency: {path}: {dependency}')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--mode', choices=['source','homebrew'], default='source')
    args = parser.parse_args()
    for name in ('bash','zsh','fish'):
        subprocess.run([str(DEST/'bin'/name), '--version'], check=True, stdout=subprocess.DEVNULL)
    subprocess.run([str(DEST/'bin/bash'), '--noprofile', '--norc', '-c', 'type compgen complete >/dev/null'], check=True)
    verify_links()
    for name in ('build-unix.sh','rebuild-bash.sh','build-helpers.sh','fetch-linux.py','verify-unix.py','runtime-manifest.py','runtime-sources.json','spdx-identifiers.json','build-container.sh'):
        shutil.copy2(Path(__file__).with_name(name), DEST/'sources'/name)
    vendor = DEST/'sources/fish-vendor'
    if vendor.is_dir():
        with tarfile.open(DEST/'sources/fish-vendor.tar.xz', 'w:xz') as archive: archive.add(vendor, arcname='vendor')
        shutil.rmtree(vendor)
    with tempfile.TemporaryDirectory(prefix='shucked-relocation-') as temporary:
        providers = Path(temporary)/'providers'
        providers.mkdir()
        # Full copy verifies relocatability, without the build prefix remaining in use.
        shutil.copytree(DEST, providers/'runtime', symlinks=False, ignore=shutil.ignore_patterns('sources'))
        (providers/'packs').symlink_to(ROOT/'tooling/providers/packs', target_is_directory=True)
        # Hide the original prefix so hard-coded fallback paths cannot mask a broken relocation.
        hidden=DEST/('.validation-'+str(os.getpid()))
        hidden.mkdir()
        moved=[]
        try:
            for entry in DEST.iterdir():
                if entry==hidden: continue
                entry.rename(hidden/entry.name)
                moved.append(entry.name)
            subprocess.run(['python3', str(ROOT/'tooling/providers/tests/test_workers.py')],
                env=dict(os.environ, SHUCKED_TEST_PROVIDER_ROOT=str(providers)), check=True)
        finally:
            for name in moved:
                if (DEST/name).exists(): raise ValueError('runtime entry recreated during validation; original kept at '+str(hidden/name))
                (hidden/name).rename(DEST/name)
            hidden.rmdir()

    arch = {'aarch64':'arm64','arm64':'arm64','x86_64':'x64','armv7l':'armhf'}[platform.machine()]
    family = 'darwin' if platform.system() == 'Darwin' else 'alpine' if Path('/etc/alpine-release').exists() else 'linux'
    minimum = {}
    if family == 'linux': minimum['glibc'] = platform.libc_ver()[1]
    manifest.write(DEST, family+'-'+arch, sources(args.mode), ['target OS libraries and compiler runtime'], tested=True, minimum=minimum)
    manifest.validate(DEST, family+'-'+arch)
    print('Validated runtime:', family+'-'+arch)


if __name__ == '__main__': main()
