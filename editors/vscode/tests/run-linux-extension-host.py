#!/usr/bin/env python3
"""Package existing native Linux binaries and run an isolated installed-VSIX host."""
import argparse
import ctypes
import threading
import time
import zipfile
import hashlib
import json
import os
from pathlib import Path
import platform
import shlex
import shutil
import subprocess
import tempfile


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def run(command, **options):
    subprocess.run([str(value) for value in command], check=True, **options)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', type=Path, required=True, help='Native existing shucked binary; never rebuilt by this runner')
    parser.add_argument('--worker-server', type=Path, required=True, help='Native existing shucked-server binary')
    parser.add_argument('--runtime', type=Path, required=True, help='Native provider runtime with source archives')
    parser.add_argument('--node', type=Path, required=True)
    parser.add_argument('--code', type=Path, required=True, help='Native VS Code bin/code launcher')
    parser.add_argument('--reuse-vsix', type=Path, help='Run updated acceptance tests against an existing native artifact without rebuilding it')
    parser.add_argument('--source-revision', help='Exact source commit when testing an exported tree without Git metadata')
    parser.add_argument('--build-mode', choices=['debug', 'release'], required=True)
    arguments = parser.parse_args()
    if platform.system() != 'Linux':
        parser.error('Run inside the Linux target host/container; this does not start or restart a VM.')
    for command in ('xvfb-run', 'xauth', 'zsh', 'fish', 'git', 'ldd'):
        if not shutil.which(command):
            parser.error(f'{command} is required on the isolated target host')
    for name in ('server', 'worker_server', 'runtime', 'node', 'code'):
        value = getattr(arguments, name).resolve()
        if not value.exists():
            parser.error(f'Missing {name}: {value}')
        setattr(arguments, name, value)
    repository = Path(__file__).resolve().parents[3]
    source = repository / 'editors/vscode'
    target = 'linux-' + {'aarch64': 'arm64', 'x86_64': 'x64'}[platform.machine()]
    root = Path(tempfile.mkdtemp(prefix='shucked-linux-installed-'))
    print(f'Linux installed-extension evidence: {root}', flush=True)
    extension = root / 'extension'
    shutil.copytree(source, extension, ignore=shutil.ignore_patterns('bin', 'node_modules', '*.vsix', '__pycache__'))
    package_path = extension / 'package.json'
    package = json.loads(package_path.read_text())
    # The isolated acceptance copy consumes explicitly supplied native binaries.
    # Production packaging keeps its normal mandatory release-build prepublish step.
    package['scripts'].pop('vscode:prepublish', None)
    package_path.write_text(json.dumps(package, indent=2) + '\n')
    environment = dict(os.environ, PATH=str(arguments.node.parent) + os.pathsep + os.environ.get('PATH', ''))
    if arguments.reuse_vsix:
        vsix = arguments.reuse_vsix.resolve()
        with zipfile.ZipFile(vsix) as archive:
            if json.loads(archive.read('extension/bin/platform.json'))['target'] != target:
                parser.error('The existing VSIX does not match this native target')
            bundled_hashes = {}
            for name in ('shucked', 'shucked-server'):
                with archive.open('extension/bin/' + name) as stream:
                    bundled_hashes[name] = hashlib.file_digest(stream, 'sha256').hexdigest()
    else:
        binaries = extension / 'bin'
        binaries.mkdir()
        for name, binary in [('shucked', arguments.server), ('shucked-server', arguments.worker_server)]:
            shutil.copy2(binary, binaries / name)
            if arguments.build_mode == 'debug':
                # Preserve debug code behavior while keeping the acceptance VSIX bounded.
                run(['strip', '--strip-debug', binaries / name])
        (binaries / 'platform.json').write_text(json.dumps({'target': target}) + '\n')
        providers = binaries / 'providers'
        providers.mkdir()
        shutil.copytree(repository / 'tooling/providers/packs', providers / 'packs')
        shutil.copytree(arguments.runtime, providers / 'runtime', symlinks=False)
        run(['python3', repository / 'tooling/providers/verify-unix.py'], env=dict(environment, SHUCKED_PROVIDER_DEST=str(providers / 'runtime')))
        verification = '''
    import { verifyProviderRuntime } from './provider-artifacts.mjs';
    import { binaryPlatform, hostTarget } from './platform.mjs';
    import path from 'node:path';
    for (const name of ['shucked', 'shucked-server']) {
      const binary = binaryPlatform(path.resolve('bin', name));
      if (binary.platform !== process.platform || binary.arch !== process.arch) throw Error('Mismatched native binary: '+name);
    }
    verifyProviderRuntime(path.resolve('bin/providers/runtime'), hostTarget());
    '''
        verification = verification.replace("'./provider-artifacts.mjs'", json.dumps((source / 'provider-artifacts.mjs').as_uri()))
        run([arguments.node, '--input-type=module', '-e', verification], cwd=extension, env=environment)
        vsix = root / f'shucked-{target}-{package["version"]}-{arguments.build_mode}-acceptance.vsix'
        run([arguments.node, source / 'node_modules/@vscode/vsce/vsce', 'package', '--no-dependencies', '--target', target, '--out', vsix], cwd=extension, env=environment)
        bundled_hashes = {name: digest(binaries / name) for name in ('shucked', 'shucked-server')}
    wrapper = root / 'code-isolated'
    wrapper.write_text('#!/bin/sh\nexec ' + shlex.quote(str(arguments.code)) + ' --no-sandbox --disable-gpu --disable-dev-shm-usage "$@"\n')
    wrapper.chmod(0o700)
    receipt = {'target': target, 'buildMode': arguments.build_mode, 'reusedArtifact': bool(arguments.reuse_vsix), 'serverSha256': None if arguments.reuse_vsix else digest(arguments.server), 'workerServerSha256': None if arguments.reuse_vsix else digest(arguments.worker_server), 'bundledServerSha256': bundled_hashes['shucked'], 'bundledWorkerServerSha256': bundled_hashes['shucked-server'], 'vsixSha256': digest(vsix), 'vsix': str(vsix), 'sourceRevision': arguments.source_revision or subprocess.check_output(['git', '-C', repository, 'rev-parse', 'HEAD'], text=True).strip(), 'node': subprocess.check_output([arguments.node, '--version'], text=True).strip(), 'passed': False}
    receipt_path = root / 'receipt.json'
    receipt_path.write_text(json.dumps(receipt, indent=2) + '\n')
    # Electron's sandbox is disabled only inside this user-isolated test container.
    # The test owns descendant cleanup even when the container's PID1 is not an init.
    if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError('Could not enable isolated editor child reaping')
    def reap_orphans():
        while True:
            children = Path('/proc/self/task') / str(os.getpid()) / 'children'
            for pid in children.read_text().split():
                candidate = int(pid)
                if candidate == process.pid:
                    continue
                try:
                    os.waitpid(candidate, os.WNOHANG)
                except ChildProcessError:
                    pass
            time.sleep(0.05)
    process = subprocess.Popen(['xvfb-run', '--auto-servernum', '--server-args=-screen 0 1280x900x24', str(arguments.node), str(extension / 'tests/run-extension-host.mjs')], env=dict(environment, SHUCKED_TEST_VSIX=str(vsix), SHUCKED_CODE_COMMAND=str(wrapper)), stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    threading.Thread(target=reap_orphans, daemon=True).start()
    output, _ = process.communicate()
    result = subprocess.CompletedProcess(process.args, process.returncode, stdout=output)
    (root / 'host.log').write_text(result.stdout)
    print(result.stdout, end='', flush=True)
    for line in result.stdout.splitlines():
        if line.startswith('Extension Development Host evidence: '):
            host_report = Path(line.removeprefix('Extension Development Host evidence: ')) / 'result.json'
            if host_report.is_file():
                receipt['hostReport'] = json.loads(host_report.read_text())
    receipt['passed'] = result.returncode == 0 and receipt.get('hostReport', {}).get('passed') is True
    receipt_path.write_text(json.dumps(receipt, indent=2) + '\n')
    print(f'Linux installed-extension receipt: {receipt_path}', flush=True)
    if not receipt['passed']:
        raise SystemExit(result.returncode or 1)


if __name__ == '__main__':
    main()
