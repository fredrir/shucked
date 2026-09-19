#!/usr/bin/env python3
"""Artifact inventory, source provenance, and SPDX SBOM for managed runtimes."""
import argparse
from functools import cache
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import tarfile
import tomllib

REQUIRED_HELPERS = ('awk', 'basename', 'cat', 'cut', 'dirname', 'find', 'grep', 'head', 'ls', 'readlink', 'sed', 'sort', 'tail', 'tr', 'uniq', 'wc', 'xargs')


def sha256(path):
    with path.open('rb') as file: return hashlib.file_digest(file, 'sha256').hexdigest()


def files(root):
    result = []
    for path in sorted(root.rglob('*')):
        if path.is_symlink() and not path.resolve().is_relative_to(root.resolve()):
            raise ValueError(f'artifact symlink escapes runtime: {path}')
        if path.is_file() and path != root / 'manifest.json':
            result.append(dict(path=path.relative_to(root).as_posix(), sha256=sha256(path), executable=bool(path.stat().st_mode & 0o111)))
    result.sort(key=lambda item: item['path'])
    return result


def worker_inputs():
    repo=Path(__file__).resolve().parents[2]
    workers=repo/'crates/shucked-lsp/src/handlers/completion'
    return dict(packManifest=sha256(repo/'tooling/providers/packs/manifest.json'), workers={
        name: sha256(workers/name) for name in ('bash_worker.bash','fish_worker.fish','zsh_worker.zsh','zsh_supervisor.zsh')})


def validate(root, target, require_tested=True):
    manifest = json.loads((root / 'manifest.json').read_text())
    if manifest.get('schemaVersion') != 2: raise ValueError('runtime schema mismatch')
    if manifest.get('target') != target: raise ValueError('runtime target mismatch')
    if manifest.get('files') != files(root): raise ValueError('runtime file inventory mismatch')
    if require_tested and manifest.get('validation', {}).get('workers') != 'passed':
        raise ValueError('runtime workers have not passed on this target')
    if manifest.get('validation', {}).get('inputs') != worker_inputs(): raise ValueError('runtime tested worker inputs changed')
    if not manifest.get('sources'): raise ValueError('runtime source provenance missing')
    if not (root / 'sbom.spdx.json').is_file(): raise ValueError('runtime SBOM missing')
    names = manifest.get('helperNames', [])
    if not set(REQUIRED_HELPERS).issubset(names): raise ValueError('private helper suite incomplete')
    if not target.startswith('win32-'):
        for directory, names in [('bin', ('bash','zsh','fish')), ('helpers/bin', REQUIRED_HELPERS)]:
            for name in names:
                path=root/directory/name
                if not path.is_file() or not path.stat().st_mode & 0o111: raise ValueError('runtime executable permission missing')
    for source in manifest['sources']:
        for archive in source.get('archives', []):
            path = root / archive['path']
            if not path.is_file() or sha256(path) != archive['sha256']:
                raise ValueError('corresponding source archive missing or changed')
    return manifest


@cache
def spdx_declared(value):
    identifiers=json.loads(Path(__file__).with_name('spdx-identifiers.json').read_text())
    licenses=set(identifiers['licenses'])
    exceptions=set(identifiers['exceptions'])
    tokens=re.findall(r'[A-Za-z0-9.+-]+|[()]',value)
    if ''.join(tokens) != re.sub(r'\s+','',value): return 'NOASSERTION'
    position=0
    def atom(depth=0):
        nonlocal position
        if depth>32 or position>=len(tokens): raise ValueError()
        token=tokens[position]
        position+=1
        if token=='(':
            expression(depth+1)
            if position>=len(tokens) or tokens[position]!=')': raise ValueError()
            position+=1
        else:
            if token not in licenses: raise ValueError()
            if position<len(tokens) and tokens[position]=='WITH':
                position+=1
                if position>=len(tokens) or tokens[position] not in exceptions: raise ValueError()
                position+=1
    def expression(depth):
        nonlocal position
        atom(depth)
        while position<len(tokens) and tokens[position] in ('AND','OR'):
            position+=1
            atom(depth)
    try:
        expression(0)
        return value if position==len(tokens) else 'NOASSERTION'
    except ValueError: return 'NOASSERTION'


def write(root, target, sources, system_dependencies, tested=False, minimum=None):
    windows = target.startswith('win32-')
    bin_dir = root / ('msys/usr/bin' if windows else 'bin')
    helper_dir = bin_dir if windows else root / 'helpers/bin'
    extension = '.exe' if windows else ''
    for name in ('bash', 'zsh', 'fish'):
        if not (bin_dir / (name + extension)).is_file(): raise ValueError(f'engine missing: {name}')
    helper_names = sorted({path.name.removesuffix('.exe') for path in helper_dir.iterdir() if path.is_file()})
    if not set(REQUIRED_HELPERS).issubset(helper_names): raise ValueError('private helper suite incomplete')
    for item in sources:
        if not item.get('license') or not item.get('archives'): raise ValueError('source license/archive metadata missing')
    fish_sources=[source for source in sources if source['name']=='fish']
    vendor=next((path for path in (root/'sources/fish-vendor.tar.xz',root/'sources/fish/vendor.tar.xz') if path.is_file()),None)
    if fish_sources and vendor is None: raise ValueError('Fish corresponding Rust dependency sources missing')
    if vendor:
        for source in fish_sources:
            source['archives'].append(dict(url='NOASSERTION',sha256=sha256(vendor),path=vendor.relative_to(root).as_posix()))
    packages = []
    relationships = []
    for index, source in enumerate(sources):
        identifier = f'SPDXRef-Package-{index}'
        packages.append(dict(name=source['name'], SPDXID=identifier, versionInfo=source['version'],
            downloadLocation=source['archives'][0]['url'], filesAnalyzed=False,
            licenseConcluded='NOASSERTION', licenseDeclared=spdx_declared(source['license']), licenseComments='Upstream metadata: '+source['license'], copyrightText='NOASSERTION',
            checksums=[dict(algorithm='SHA256', checksumValue=source['archives'][0]['sha256'])]))
        relationships.append(dict(spdxElementId='SPDXRef-DOCUMENT', relationshipType='DESCRIBES', relatedSpdxElement=identifier))
    if vendor:
        with tarfile.open(vendor) as archive:
            for item in archive.getmembers():
                if item.name.count('/')!=2 or not item.name.endswith('/Cargo.toml'): continue
                package=tomllib.loads(archive.extractfile(item).read().decode()).get('package',{})
                if not package.get('name') or not isinstance(package.get('version'),str): raise ValueError('Rust vendor package identity missing')
                identifier=f'SPDXRef-RustPackage-{len(packages)}'
                declared=package.get('license','NOASSERTION').replace(' / ', ' OR ').replace('/', ' OR ')
                packages.append(dict(name=package['name'],SPDXID=identifier,versionInfo=package['version'],downloadLocation='NOASSERTION',filesAnalyzed=False,licenseConcluded='NOASSERTION',licenseDeclared=spdx_declared(declared),copyrightText='NOASSERTION'))
                relationships.append(dict(spdxElementId='SPDXRef-DOCUMENT',relationshipType='DESCRIBES',relatedSpdxElement=identifier))
    identity = hashlib.sha256(json.dumps(sources, sort_keys=True).encode()).hexdigest()
    sbom = dict(spdxVersion='SPDX-2.3', dataLicense='CC0-1.0', SPDXID='SPDXRef-DOCUMENT', name='Shucked managed providers '+target,
        documentNamespace='https://shucked.dev/spdx/providers/'+target+'/'+identity,
        creationInfo=dict(created='2025-07-01T00:00:00Z', creators=['Tool: shucked-provider-builder']), packages=packages, relationships=relationships)
    (root / 'sbom.spdx.json').write_text(json.dumps(sbom, indent=2) + '\n')
    manifest = dict(schemaVersion=2, platform='linux' if target.startswith(('linux-', 'alpine-')) else target.split('-')[0],
        architecture='x86_64' if windows else platform.machine(), target=target, sources=sources, helperNames=helper_names,
        minimumHost=minimum or {}, systemDependencies=system_dependencies,
        validation=dict(workers='passed' if tested else 'not-run', host=platform.platform(), execution=os.environ.get('SHUCKED_PROVIDER_EXECUTION','native'), inputs=worker_inputs()), files=files(root))
    (root / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    return manifest


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('root', type=Path)
    parser.add_argument('--target', required=True)
    parser.add_argument('--allow-untested', action='store_true')
    args = parser.parse_args()
    result = validate(args.root, args.target, not args.allow_untested)
    print(f"verified {result['target']}: {len(result['files'])} files")
