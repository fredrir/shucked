import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('vendor', ROOT / 'vendor.py')
vendor = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(vendor)


class CompletionPacks(unittest.TestCase):
    def test_named_commands_have_upstream_definitions(self):
        registry = json.loads((ROOT / 'packs/registry.json').read_text())
        for name in ('brew', 'pacman', 'apt', 'paru', 'docker', 'eza', 'ls'):
            with self.subTest(command=name):
                entries = registry['commands'][name]
                self.assertTrue(entries)
                self.assertTrue(all((ROOT / 'packs' / entry['path']).is_file() for entry in entries))
        self.assertEqual(registry['commands']['brew'][0]['source'], 'brew')
        self.assertEqual(registry['commands']['docker'][0]['source'], 'oh-my-zsh')

    def test_pack_files_and_registry_match_the_integrity_manifest(self):
        manifest = json.loads((ROOT / 'packs/manifest.json').read_text())
        paths = set()
        for source in manifest['sources']:
            for item in source['files']:
                with self.subTest(path=item['path']):
                    self.assertNotIn(item['path'], paths)
                    paths.add(item['path'])
                    self.assertEqual(hashlib.sha256((ROOT / 'packs' / item['path']).read_bytes()).hexdigest(), item['sha256'])
        for item in manifest['generated']:
            self.assertEqual(hashlib.sha256((ROOT / 'packs' / item['path']).read_bytes()).hexdigest(), item['sha256'])
        self.assertEqual(vendor.registry(manifest['sources']), json.loads((ROOT / 'packs/registry.json').read_text()))

    def test_registry_ignores_stubs_and_preserves_aliases_and_priority(self):
        with tempfile.TemporaryDirectory() as folder, patch.object(vendor, 'DEST', Path(folder)):
            files = {
                'fish/share/completions/demo.fish': '# Install the upstream definition.\n',
                'fish/share/completions/other.fish': 'complete -c other -l example\n',
                'zsh-extra/_demo': '#compdef demo alias=demo # notes are not commands\n_arguments\n',
                'zsh/Completion/_demo': '#compdef demo\n_arguments\n',
            }
            for name, contents in files.items():
                target = Path(folder) / name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(contents)
            registry = vendor.registry([dict(name='fixture', files=[dict(path=name) for name in files])])
            self.assertEqual(set(registry['commands']), {'demo', 'alias', 'other'})
            self.assertEqual(len(registry['commands']['demo']), 2)
            self.assertEqual(registry['commands']['demo'][0]['path'], 'zsh-extra/_demo')

    def test_omz_pack_excludes_startup_and_plugin_bootstrap(self):
        self.assertTrue(vendor.selected('oh-my-zsh', 'plugins/example/_example'))
        self.assertFalse(vendor.selected('oh-my-zsh', 'oh-my-zsh.sh'))
        self.assertFalse(vendor.selected('oh-my-zsh', 'plugins/example/example.plugin.zsh'))


if __name__ == '__main__':
    unittest.main()
