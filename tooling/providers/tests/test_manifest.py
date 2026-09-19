import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location('runtime_manifest', Path(__file__).resolve().parents[1] / 'runtime-manifest.py')
manifest = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(manifest)


class ArtifactManifest(unittest.TestCase):
    def fixture(self, root, tested=True):
        (root / 'bin').mkdir()
        (root / 'helpers/bin').mkdir(parents=True)
        (root / 'sources').mkdir()
        for name in ('bash', 'fish', 'zsh'): (root / 'bin' / name).write_bytes(b'fixture'); (root / 'bin' / name).chmod(0o755)
        for name in manifest.REQUIRED_HELPERS: (root / 'helpers/bin' / name).write_bytes(b'helper'); (root / 'helpers/bin' / name).chmod(0o755)
        source = root / 'sources/source.tar'
        source.write_bytes(b'corresponding source fixture')
        sources = [dict(name='fixture', version='1', license='MIT', archives=[dict(path='sources/source.tar', url='https://example.invalid/source.tar', sha256=manifest.sha256(source))])]
        return manifest.write(root, 'darwin-arm64', sources, [], tested=tested)

    def test_target_and_real_execution_receipt_are_required(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root, tested=False)
            with self.assertRaisesRegex(ValueError, 'have not passed'): manifest.validate(root, 'darwin-arm64')
            with self.assertRaisesRegex(ValueError, 'target mismatch'): manifest.validate(root, 'linux-arm64')

    def test_added_changed_and_missing_files_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            manifest.validate(root, 'darwin-arm64')
            extra = root / 'bin/untracked-program'
            extra.write_text('unexpected')
            with self.assertRaisesRegex(ValueError, 'inventory mismatch'): manifest.validate(root, 'darwin-arm64')
            extra.unlink()
            (root / 'sources/source.tar').unlink()
            with self.assertRaisesRegex(ValueError, 'inventory mismatch'): manifest.validate(root, 'darwin-arm64')

    def test_escape_symlinks_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            (root / 'outside').symlink_to(root.parent, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, 'escapes runtime'): manifest.validate(root, 'darwin-arm64')


if __name__ == '__main__': unittest.main()
