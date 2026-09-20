import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('verify_unix', Path(__file__).resolve().parents[1] / 'verify-unix.py')
verify = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(verify)


class UnixDependencies(unittest.TestCase):
    def verify_output(self, output):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            header = bytearray(64)
            header[:6] = b'\x7fELF\x02\x01'
            header[18:20] = (62).to_bytes(2, 'little')
            (root / 'engine').write_bytes(header)
            with patch.object(verify, 'DEST', root), patch.object(verify.platform, 'machine', return_value='x86_64'), patch.object(verify, 'elf_is_static', return_value=False), patch.object(verify.subprocess, 'check_output', return_value=output):
                verify.verify_links()

    def test_arch_system_loader_is_allowed(self):
        self.verify_output('/lib64/ld-linux-x86-64.so.2 => /usr/lib64/ld-linux-x86-64.so.2 (0x1234)\nlibc.so.6 => /usr/lib/libc.so.6 (0x5678)')

    def test_non_system_absolute_library_is_rejected(self):
        with self.assertRaisesRegex(ValueError, 'unbundled runtime dependency'):
            self.verify_output('/opt/custom/libsomething.so => /opt/custom/libsomething.so (0x1234)')


if __name__ == '__main__': unittest.main()
