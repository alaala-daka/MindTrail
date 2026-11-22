import hashlib
import importlib.util
import os
from pathlib import Path
import tempfile
import unittest

ROOT=Path(__file__).resolve().parent.parent
spec=importlib.util.spec_from_file_location('release_tools',ROOT/'scripts/release_tools.py')
module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)

class ReleaseToolsTests(unittest.TestCase):
    def test_tool_pin_and_path_mismatch(self):
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            p=Path(tmp)/'tool.exe'; p.write_bytes(b'synthetic-tool'); pin=hashlib.sha256(p.read_bytes()).hexdigest()
            self.assertTrue(module.validate_tool(p,p,pin))
            for bad in ('0'*64,'invalid'):
                with self.assertRaises(ValueError): module.validate_tool(p,p,bad)
            with self.assertRaises(ValueError): module.validate_tool(p,Path(tmp)/'different.exe',pin)
            p.write_bytes(b'changed')
            with self.assertRaises(ValueError): module.validate_tool(p,p,pin)
    @unittest.skipUnless(os.name=='nt','Windows hardlink check')
    def test_hardlinked_tool_rejected(self):
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            p=Path(tmp)/'tool.exe'; p.write_bytes(b'synthetic'); pin=hashlib.sha256(p.read_bytes()).hexdigest(); link=Path(tmp)/'link.exe'
            os.link(p,link)
            try:
                with self.assertRaises(ValueError): module.validate_tool(p,p,pin)
                self.assertTrue(module.validate_tool(p,p,pin,allow_hardlinks=True))
            finally: link.unlink()

if __name__=='__main__': unittest.main()
