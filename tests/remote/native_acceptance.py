#!/usr/bin/env python3
"""Validate shipped completion engines against a real Arch host's package database."""
import argparse
import asyncio
import json
from pathlib import Path
import sys
import time
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tests.lsp.client import LspClient
from tests.remote.acceptance import Transport


class NativeClient(LspClient):
    def __init__(self, transport, binary, root, providers):
        super().__init__(binary)
        self.transport, self.root, self.providers = transport, root, providers

    async def start(self):
        command = self.transport.command([
            '/usr/bin/env', 'PATH=/usr/bin:/bin', f'HOME={self.root}',
            f'ZDOTDIR={self.root}', f'XDG_CONFIG_HOME={self.root}/.config',
            f'SHUCKED_PROVIDER_ROOT={self.providers}', self.binary_path, 'server',
        ])
        self.proc = await asyncio.create_subprocess_exec(*command, stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
        self._read_task = asyncio.create_task(self._read_loop())


async def run(args):
    transport = Transport(json.loads(args.command), args.ssh)
    packages = set(transport.run(['/usr/bin/pacman', '-Slq']).splitlines())
    assert args.package in packages, 'Expected package is absent from the target repository database'
    root = '/tmp/shucked-native-' + uuid.uuid4().hex
    transport.run(['/usr/bin/python3', '-c', '''from pathlib import Path
import sys
p=Path(sys.argv[1]); p.mkdir(); (p/'.config/fish').mkdir(parents=True)
for name in ['.zshenv','.zshrc','.bashrc','.bash_profile','.bash_completion','.config/fish/config.fish']:
 (p/name).write_text('touch '+str(p/'startup-ran')+'; exit 1\\n')
''', root])
    checks = []
    started = time.monotonic()
    client = NativeClient(transport, args.binary, root, args.provider_root)
    try:
        await client.start()
        await client.initialize(root_uri='file://' + root, initialization_options={'nativeExecutionAllowed': True})
        await client.initialized()
        for shell in ['bash', 'zsh', 'fish']:
            uri = f'file://{root}/script.{shell}'
            line = 'pacman -S ' + args.package[:-1]
            await client.open_document(uri, text=line, language_id=shell)
            result = await client.completion(uri, 0, len(line))
            items = result.get('items', []) if isinstance(result, dict) else result or []
            candidate = next((item for item in items if item['label'] == args.package), None)
            assert candidate is not None, (shell, items)
            assert candidate.get('textEdit', {}).get('newText', candidate.get('insertText')) == args.package, candidate
            checks.append(shell + ': real pacman repository package and insertion')
            # A bundled engine must also contribute tool-specific flags through LSP.
            line = 'git checkout --'
            transport.run(['/usr/bin/git', '-C', root, 'init', '--quiet'])
            await client.change_document(uri, text=line, version=2)
            result = await client.completion(uri, 0, len(line))
            items = result.get('items', []) if isinstance(result, dict) else result or []
            expected = '--track' if shell == 'fish' else '--detach'
            candidate = next((item for item in items if item['label'] == expected), None)
            assert candidate is not None, (shell, items)
            assert candidate.get('textEdit', {}).get('newText', candidate.get('insertText')) == expected, candidate
            checks.append(shell + ': bundled Git flags and insertion')
        transport.run(['/usr/bin/python3', '-c', 'from pathlib import Path; import sys; assert not (Path(sys.argv[1])/"startup-ran").exists()', root])
        print(json.dumps({'transport': args.name, 'host': transport.run(['uname', '-sm']).strip(),
            'checks': checks + ['personal startup files were not executed'],
            'elapsedSeconds': round(time.monotonic() - started, 3)}, indent=2))
    finally:
        if client.proc is not None:
            await client.shutdown_and_exit()
        transport.run(['/usr/bin/python3', '-c', 'import shutil,sys; shutil.rmtree(sys.argv[1])', root])


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--command', required=True, help='JSON transport argv')
    parser.add_argument('--ssh', action='store_true')
    parser.add_argument('--binary', required=True)
    parser.add_argument('--provider-root', required=True, help='Target directory containing packs/ and runtime/')
    parser.add_argument('--package', default='pacman')
    parser.add_argument('--name', default='Arch SSH')
    asyncio.run(run(parser.parse_args()))
