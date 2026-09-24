import os
from pathlib import Path
import subprocess
import signal
import shutil
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[3]
WORKERS = ROOT / 'crates/shucked-lsp/src/handlers/completion'
PROVIDERS = Path(os.environ['SHUCKED_TEST_PROVIDER_ROOT']) if 'SHUCKED_TEST_PROVIDER_ROOT' in os.environ else None
RUNTIME = PROVIDERS / 'runtime' if PROVIDERS else ROOT / 'target/provider-runtime'
PACKS = PROVIDERS / 'packs' if PROVIDERS else ROOT / 'tooling/providers/packs'


class ManagedWorkers(unittest.TestCase):
    def test_installed_zsh_provider_can_match_prefixes_with_regular_expressions(self):
        with tempfile.TemporaryDirectory() as folder:
            home = Path(folder)
            definitions = home / 'completions'
            definitions.mkdir()
            (definitions / '_regex_fixture').write_text(
                '#compdef regex-fixture\n[[ $PREFIX =~ "^--" ]] && compadd -- --regex\n')
            with patch.dict(os.environ, SHUCKED_COMPLETION_PATHS=str(definitions)):
                candidates = self.complete('zsh', ['regex-fixture', '--'], home)
            self.assertIn('--regex', candidates)

    def complete(self, shell, words, home):
        return self.complete_many(shell, [words], home)[0]

    def complete_many(self, shell, requests, home):
        executable = RUNTIME / 'bin' / shell
        if not executable.is_file():
            self.skipTest(f'{shell} runtime has not been built')
        providers = home / 'providers'
        providers.mkdir(exist_ok=True)
        if not (providers / 'packs').exists():
            (providers / 'packs').symlink_to(PACKS, target_is_directory=True)
        env = dict(os.environ, HOME=str(home), XDG_CONFIG_HOME=str(home / '.config'),
                   ZDOTDIR=str(home), SHUCKED_PROVIDER_ROOT=str(providers),
                   PATH=self.worker_path(home), TERM='dumb', SHELL='/missing/user/shell')
        env.pop('BASH_ENV', None)
        env.pop('ENV', None)
        payload = bytearray()
        if shell == 'zsh':
            import shlex
            env.update(SHUCKED_NATIVE_SHELL=str(executable),
                       SHUCKED_NATIVE_SCRIPT=(WORKERS / 'zsh_worker.zsh').read_text(),
                       SHUCKED_NATIVE_PERSONAL='0')
            args = [str(executable), '-f', '-c', (WORKERS / 'zsh_supervisor.zsh').read_text()]
            for words in requests:
                buffer = ' '.join(shlex.quote(word) if word else '' for word in words)
                payload.extend(('\0'.join([str(home), env['PATH'], buffer, str(len(buffer))]) + '\0').encode())
        elif shell == 'bash':
            args = [str(executable), '--noprofile', '--norc', str(WORKERS / 'bash_worker.bash')]
        else:
            args = [str(executable), '--no-config', '--private', str(WORKERS / 'fish_worker.fish')]
        if shell != 'zsh':
            for words in requests:
                payload.extend(('\0'.join([str(home), env['PATH'], '', str(len(words)), *words]) + '\0').encode())
        process = subprocess.Popen(args, env=env, cwd=home, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, start_new_session=True)
        try:
            output, errors = process.communicate(input=bytes(payload), timeout=8)
        except subprocess.TimeoutExpired as error:
            fields = (error.output or b'').split(b'\0')
            if shell == 'zsh' and len(fields) >= 2 and fields[0] == b'P' and fields[1].isdigit():
                try: os.killpg(int(fields[1]), signal.SIGKILL)
                except ProcessLookupError: pass
            try: os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError: pass
            process.communicate()
            raise
        result = subprocess.CompletedProcess(args, process.returncode, output, errors)
        self.assertEqual(result.returncode, 0, result.stderr.decode(errors='replace'))
        fields = result.stdout.split(b'\0')
        results, current, index = [], [], 0
        while index < len(fields) - 1:
            tag = fields[index]
            if tag == b'P':
                index += 2
            elif tag in (b'C', b'Q', b'M', b'B'):
                current.append(fields[index + 1].decode())
                index += 5 if tag in (b'C', b'Q') else 3
            elif tag == b'E':
                results.append(current)
                current = []
                index += 1
            else:
                self.fail(f'Unexpected worker field: {tag!r}')
        self.assertEqual(len(results), len(requests), result.stdout)
        return results

    def worker_path(self, home):
        target = home / 'target-bin'
        target.mkdir(exist_ok=True)
        if not (RUNTIME / 'helpers/bin/grep').is_file():
            return os.pathsep.join([str(target), '/usr/bin', '/bin'])
        git = shutil.which('git')
        if git and not (target / 'git').exists():
            (target / 'git').symlink_to(git)
        return os.pathsep.join(map(str, [target, RUNTIME / 'helpers/bin', RUNTIME / 'bin']))

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

    def test_upstream_definitions_complete_commands_flags_and_values(self):
        cases = [
            ('zsh', ['brew', ''], 'install'),
            ('zsh', ['brew', 'install', '--'], '--formula'),
            ('zsh', ['apt', ''], 'install'),
            ('zsh', ['paru', '-'], '-S'),
            ('zsh', ['docker', ''], 'container'),
            ('zsh', ['docker', 'container', ''], 'ls'),
            ('zsh', ['eza', '-'], '--long'),
            ('zsh', ['eza', '--color='], '--color=always'),
            ('fish', ['pacman', '-'], '-S'),
        ]
        with tempfile.TemporaryDirectory() as folder:
            home = Path(folder)
            target = home / 'target-bin'
            target.mkdir()
            for name in ('brew', 'apt', 'paru', 'docker', 'eza', 'pacman'):
                command = target / name
                command.write_text('#!/bin/sh\nexit 0\n')
                command.chmod(0o755)
            (target / 'docker').write_text("#!/bin/sh\n[ $# -eq 0 ] || exit 0\nprintf 'Commands:\\n  container  Container operations\\n  image  Image operations\\n'\n")
            for shell, words, expected in cases:
                with self.subTest(shell=shell, words=words):
                    candidates = self.complete(shell, words, home)
                    self.assertIn(expected, [candidate.rstrip() for candidate in candidates])

    def test_one_worker_handles_multiple_completion_contexts(self):
        with tempfile.TemporaryDirectory() as folder:
            home = Path(folder)
            for shell in ('zsh', 'bash', 'fish'):
                with self.subTest(shell=shell):
                    candidates = self.complete_many(shell, [['git', 'chec'], ['git', 'sta']], home)
                    self.assertIn('checkout', [candidate.rstrip() for candidate in candidates[0]])
                    self.assertIn('status', [candidate.rstrip() for candidate in candidates[1]])


if __name__ == '__main__':
    unittest.main()
