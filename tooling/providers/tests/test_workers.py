import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]
WORKERS = ROOT / 'crates/shucked-lsp/src/handlers/completion'
RUNTIME = ROOT / 'target/provider-runtime'
PACKS = ROOT / 'tooling/providers/packs'


class ManagedWorkers(unittest.TestCase):
    def complete(self, shell, words, home):
        executable = RUNTIME / 'bin' / shell
        if not executable.is_file():
            self.skipTest(f'{shell} runtime has not been built')
        providers = home / 'providers'
        providers.mkdir(exist_ok=True)
        if not (providers / 'packs').exists():
            (providers / 'packs').symlink_to(PACKS, target_is_directory=True)
        env = dict(os.environ, HOME=str(home), XDG_CONFIG_HOME=str(home / '.config'),
                   ZDOTDIR=str(home), SHUCKED_PROVIDER_ROOT=str(providers),
                   PATH='/usr/bin:/bin', TERM='dumb', SHELL='/missing/user/shell')
        env.pop('BASH_ENV', None)
        env.pop('ENV', None)
        if shell == 'zsh':
            import shlex
            env.update(SHUCKED_NATIVE_SHELL=str(executable),
                       SHUCKED_NATIVE_SCRIPT=(WORKERS / 'zsh_worker.zsh').read_text(),
                       SHUCKED_NATIVE_BUFFER=' '.join(map(shlex.quote, words)),
                       SHUCKED_NATIVE_PERSONAL='0')
            args = [str(executable), '-f', '-c', (WORKERS / 'zsh_supervisor.zsh').read_text()]
        elif shell == 'bash':
            args = [str(executable), '--noprofile', '--norc', str(WORKERS / 'bash_worker.bash'), *words]
        else:
            args = [str(executable), '--no-config', '--private', str(WORKERS / 'fish_worker.fish'), *words]
        result = subprocess.run(args, env=env, cwd=home, capture_output=True, timeout=8)
        self.assertEqual(result.returncode, 0, result.stderr.decode(errors='replace'))
        fields = result.stdout.split(b'\0')
        self.assertEqual(fields[0], b'P', result.stdout)
        self.assertEqual(fields[-2], b'E', result.stdout)
        return [fields[i + 1].decode() for i in range(2, len(fields) - 2, 3)]

    def test_bundled_shells_complete_arguments_with_hostile_personal_config(self):
        with tempfile.TemporaryDirectory() as folder:
            home = Path(folder)
            marker = home / 'startup-ran'
            for name in ('.zshenv', '.zshrc', '.bashrc', '.bash_profile', '.bash_completion'):
                (home / name).write_text(f'touch {marker}; exit 1\n')
            (home / '.config/fish').mkdir(parents=True)
            (home / '.config/fish/config.fish').write_text(f'touch {marker}; exit 1\n')
            for shell in ('zsh', 'bash', 'fish'):
                with self.subTest(shell=shell):
                    candidates = self.complete(shell, ['git', 'chec'], home)
                    self.assertIn('checkout', [candidate.rstrip() for candidate in candidates])
                    self.assertFalse(marker.exists())

    def test_editor_command_substitutions_are_data(self):
        with tempfile.TemporaryDirectory() as folder:
            home = Path(folder)
            marker = home / 'editor-ran'
            for shell in ('zsh', 'bash', 'fish'):
                with self.subTest(shell=shell):
                    self.complete(shell, ['git', f'$(touch {marker})', ''], home)
                    self.assertFalse(marker.exists())


if __name__ == '__main__':
    unittest.main()
