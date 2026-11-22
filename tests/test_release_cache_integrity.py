import hashlib
import importlib.util
import os
from pathlib import Path
import tempfile
import unittest

ROOT=Path(__file__).resolve().parent.parent
spec=importlib.util.spec_from_file_location('release_cache_integrity',ROOT/'tests/release_cache_integrity.py')
module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)

class CacheIntegrityTests(unittest.TestCase):
    def test_digest_and_replacement_checks(self):
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            path=Path(tmp)/'package'; path.write_bytes(b'synthetic')
            digest=hashlib.sha256(b'synthetic').digest()
            self.assertTrue(module.match_file(path,'sha256',digest))
            self.assertFalse(module.match_file(path,'sha256',bytes(32)))
    @unittest.skipUnless(os.name=='nt','Windows hardlink')
    def test_hardlinked_cache_file_rejected(self):
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            p=Path(tmp)/'package'; p.write_bytes(b'synthetic'); link=Path(tmp)/'link'; os.link(p,link)
            try: self.assertFalse(module.match_file(p,'sha256',hashlib.sha256(b'synthetic').digest()))
            finally: link.unlink()
    @unittest.skipUnless(os.name=='nt','Windows symbolic link')
    def test_linked_ancestor_rejected(self):
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            root=Path(tmp); actual=root/'actual'; actual.mkdir(); (actual/'package').write_bytes(b'synthetic'); link=root/'linked'
            try: os.symlink(actual,link,target_is_directory=True)
            except OSError: self.skipTest('symbolic links unavailable')
            self.assertFalse(module.match_file(link/'package','sha256',hashlib.sha256(b'synthetic').digest()))

if __name__=='__main__': unittest.main()
