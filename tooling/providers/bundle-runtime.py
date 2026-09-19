#!/usr/bin/env python3
"""Create a relocatable macOS shell runtime with complete corresponding sources."""
import concurrent.futures
import hashlib
import json
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
DEST = ROOT / 'target/provider-runtime'

def run(*args):
    return subprocess.check_output(args, text=True).strip()

def download(url, dest, expected):
    if dest.is_file() and hashlib.sha256(dest.read_bytes()).hexdigest() == expected:
        return
    try:
        data = urllib.request.urlopen(url, timeout=60).read()
    except Exception as error:
        if url.startswith('https://ftpmirror.gnu.org/'):
            data = urllib.request.urlopen(url.replace('https://ftpmirror.gnu.org/', 'https://ftp.gnu.org/gnu/'), timeout=60).read()
        else:
            raise RuntimeError(f'source unavailable: {url}') from error
    if hashlib.sha256(data).hexdigest() != expected:
        raise ValueError(f'source checksum mismatch: {url}')
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_bytes(data)

def main():
    if platform.system() != 'Darwin':
        raise SystemExit('This recipe supports macOS only; Linux and Windows runtime builds are not yet supplied.')
    (DEST / 'bin').mkdir(parents=True, exist_ok=True)
    (DEST / 'lib').mkdir(exist_ok=True)
    pending = [(Path(run('brew', '--prefix', name)) / 'bin' / name, DEST / 'bin' / name) for name in ('bash', 'fish')]
    pending.append((ROOT / 'target/provider-build/runtime/bin/zsh', DEST / 'bin/zsh'))
    copied = set()
    formulae = set()
    while pending:
        source, target = pending.pop()
        if target in copied:
            continue
        copied.add(target)
        shutil.copy2(source, target)
        target.chmod(0o755)
        for line in run('otool', '-L', str(source)).splitlines()[1:]:
            dependency = line.strip().split(' (')[0]
            if not dependency.startswith(('/opt/homebrew/', '/usr/local/')):
                continue
            original = Path(dependency)
            formulae.add(original.resolve().parts[original.resolve().parts.index('Cellar') + 1])
            destination = DEST / 'lib' / original.name
            pending.append((original, destination))
            new = ('@loader_path/' if target.parent.name == 'lib' else '@executable_path/../lib/') + original.name
            subprocess.run(['install_name_tool', '-change', dependency, new, str(target)], check=True, capture_output=True)
        if target.suffix == '.dylib':
            subprocess.run(['install_name_tool', '-id', '@loader_path/' + target.name, str(target)], check=True, capture_output=True)
        subprocess.run(['codesign', '--force', '--sign', '-', str(target)], check=True, capture_output=True)
    fish_share = Path(run('brew', '--prefix', 'fish')) / 'share/fish'
    shutil.copytree(fish_share, DEST / 'share/fish', dirs_exist_ok=True, ignore=shutil.ignore_patterns('*shellcheck*'))
    lock = Path(__file__).with_name(f'runtime-lock.darwin-{platform.machine()}.json')
    metadata = json.loads(lock.read_text())
    if {item['name'] for item in metadata} != formulae | {'bash', 'fish'}:
        raise RuntimeError('runtime dependency closure differs from pinned lock')
    sources = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=6) as executor:
        jobs = []
        for item in metadata:
            stable = item['urls']['stable']
            source_dir = DEST / 'sources' / item['name']
            source_dir.mkdir(parents=True, exist_ok=True)
            (source_dir / 'homebrew.json').write_text(json.dumps(item, indent=2) + '\n')
            installed = Path(run('brew', '--prefix', item['name'])).resolve()
            if installed.name != item['installed'][0]['version']:
                raise RuntimeError(f'installed runtime version differs from lock: {installed}')
            formula = installed / '.brew' / (item['name'] + '.rb')
            if formula.is_file():
                shutil.copy2(formula, source_dir / formula.name)
            for license_file in installed.glob('*COPY*'):
                if license_file.is_file(): shutil.copy2(license_file, source_dir / license_file.name)
            for spec in [dict(url=stable['url'], sha256=stable['checksum']), *item['patches']]:
                filename = Path(urllib.parse.urlparse(spec['url']).path).name
                jobs.append(executor.submit(download, spec['url'], source_dir / filename, spec['sha256']))
            sources.append(dict(name=item['name'], version=item['installed'][0]['version'], license=item['license'], source=stable['url'], sha256=stable['checksum']))
        for job in jobs:
            job.result()
    fish_sources = DEST / 'sources/fish'
    if not (fish_sources / 'vendor.tar.xz').is_file():
        work = ROOT / 'target/provider-fish-source'
        work.mkdir(parents=True, exist_ok=True)
        archive = next(fish_sources.glob('fish-*.tar.xz'))
        subprocess.run(['tar', '-xf', str(archive), '-C', str(work)], check=True)
        manifest_path = work / archive.name.removesuffix('.tar.xz') / 'Cargo.toml'
        vendor = fish_sources / 'vendor'
        config = run('cargo', 'vendor', '--locked', '--manifest-path', str(manifest_path), str(vendor))
        (fish_sources / 'cargo-config.toml').write_text(config.replace(str(vendor), 'vendor') + '\n')
        with tarfile.open(fish_sources / 'vendor.tar.xz', 'w:xz') as archive_file:
            archive_file.add(vendor, arcname='vendor')
        shutil.rmtree(vendor)
    zsh_archive = ROOT / 'target/provider-build/zsh-5.9.tar.xz'
    (DEST / 'sources/zsh').mkdir(parents=True, exist_ok=True)
    shutil.copy2(zsh_archive, DEST / 'sources/zsh/zsh-5.9.tar.xz')
    sources.append(dict(name='zsh', version='5.9', license='Zsh', source='https://www.zsh.org/pub/old/zsh-5.9.tar.xz', sha256=hashlib.sha256(zsh_archive.read_bytes()).hexdigest()))
    manifest = dict(schemaVersion=1, platform='darwin', architecture=platform.machine(), sources=sources,
                    systemDependencies=['macOS system libraries', '/usr/bin and /bin POSIX utilities'], files=[])
    for file in sorted(DEST.rglob('*')):
        if file.is_file() and file.name != 'manifest.json':
            manifest['files'].append(dict(path=str(file.relative_to(DEST)), sha256=hashlib.sha256(file.read_bytes()).hexdigest()))
    (DEST / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')

if __name__ == '__main__':
    main()
