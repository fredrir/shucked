#!/usr/bin/env python3
"""Artifact inventory, source provenance, and SPDX SBOM for managed runtimes."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import time

REQUIRED_HELPERS = ('awk', 'basename', 'cat', 'cut', 'dirname', 'find', 'grep', 'head', 'ls', 'readlink', 'sed', 'sort', 'tail', 'tr', 'uniq', 'wc', 'xargs')


def sha256(path):
    with path.open('rb') as file: return hashlib.file_digest(file, 'sha256').hexdigest()


def files(root):
    result = []
    for path in sorted(root.rglob('*')):
        if path.is_symlink() and not path.resolve().is_relative_to(root.resolve()):
            raise ValueError(f'artifact symlink escapes runtime: {path}')
        if path.is_file() and path != root / 'manifest.json':
            result.append(dict(path=path.relative_to(root).as_posix(), sha256=sha256(path)))
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
    for source in manifest['sources']:
        for archive in source.get('archives', []):
            path = root / archive['path']
            if not path.is_file() or sha256(path) != archive['sha256']:
                raise ValueError('corresponding source archive missing or changed')
    return manifest


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
    packages = []
    relationships = []
    for index, source in enumerate(sources):
        identifier = f'SPDXRef-Package-{index}'
        packages.append(dict(name=source['name'], SPDXID=identifier, versionInfo=source['version'],
            downloadLocation=source['archives'][0]['url'], filesAnalyzed=False,
            licenseConcluded='NOASSERTION', licenseDeclared=source['license'], copyrightText='NOASSERTION',
            checksums=[dict(algorithm='SHA256', checksumValue=source['archives'][0]['sha256'])]))
        relationships.append(dict(spdxElementId='SPDXRef-DOCUMENT', relationshipType='DESCRIBES', relatedSpdxElement=identifier))
    identity = hashlib.sha256(json.dumps(sources, sort_keys=True).encode()).hexdigest()
    sbom = dict(spdxVersion='SPDX-2.3', dataLicense='CC0-1.0', SPDXID='SPDXRef-DOCUMENT', name='Shucked managed providers '+target,
        documentNamespace='https://shucked.dev/spdx/providers/'+target+'/'+identity,
        creationInfo=dict(created='2025-07-01T00:00:00Z', creators=['Tool: shucked-provider-builder']), packages=packages, relationships=relationships)
    (root / 'sbom.spdx.json').write_text(json.dumps(sbom, indent=2) + '\n')
    manifest = dict(schemaVersion=2, platform='linux' if target.startswith(('linux-', 'alpine-')) else target.split('-')[0],
        architecture='x86_64' if windows else platform.machine(), target=target, sources=sources, helperNames=helper_names,
        minimumHost=minimum or {}, systemDependencies=system_dependencies,
        validation=dict(workers='passed' if tested else 'not-run', host=platform.platform(), inputs=worker_inputs()), files=files(root))
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
