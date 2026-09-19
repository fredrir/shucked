import importlib.util
from pathlib import Path
import struct
import unittest

SPEC=importlib.util.spec_from_file_location('verify_pe',Path(__file__).resolve().parents[1]/'verify-pe.py')
pe=importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(pe)


class PortableExecutable(unittest.TestCase):
    def fixture(self):
        data=bytearray(1024)
        data[:2]=b'MZ'
        struct.pack_into('<I',data,0x3c,128)
        data[128:132]=b'PE\0\0'
        struct.pack_into('<HH',data,132,0x8664,1)
        struct.pack_into('<H',data,148,240)
        struct.pack_into('<H',data,152,0x20b)
        struct.pack_into('<I',data,260,16)
        struct.pack_into('<II',data,272,0x1000,40)
        struct.pack_into('<IIII',data,400,512,0x1000,512,512)
        struct.pack_into('<I',data,524,0x1080)
        data[640:653]=b'missing.dll\0\0'
        return data

    def test_imports_are_recovered_without_executing_binary(self):
        self.assertEqual(pe.imports(self.fixture()),{'missing.dll'})

    def test_wrong_architecture_is_rejected(self):
        data=self.fixture()
        struct.pack_into('<H',data,132,0xaa64)
        with self.assertRaisesRegex(ValueError,'expected x64'): pe.imports(data)

    def test_external_import_rva_is_rejected(self):
        data=self.fixture()
        struct.pack_into('<I',data,524,0xfffffff)
        with self.assertRaisesRegex(ValueError,'outside file'): pe.imports(data)


if __name__=='__main__': unittest.main()
