import hashlib
import os
import sys
import tempfile
from pathlib import Path
from unittest import mock
import unittest

ROOT=Path(__file__).resolve().parent.parent
sys.path.insert(0,str(ROOT/'scripts'))
import release_nsis_readiness as readiness

class NsisReadinessTests(unittest.TestCase):
    def test_owned_closure_and_tampering(self):
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            base=Path(tmp); run=base/('run-'+'a'*32); root=run/'target/.tauri/NSIS'; root.mkdir(parents=True); (run/'owner').write_text('a'*32)
            for name in readiness.REQUIRED:
                p=root/name; p.parent.mkdir(parents=True,exist_ok=True); p.write_bytes(b'synthetic')
            fake=hashlib.sha1(b'synthetic',usedforsecurity=False).hexdigest(); pins={name:hashlib.sha256(b'synthetic').hexdigest() for name in readiness.REQUIRED}
            with mock.patch.object(readiness,'HASHES',{name:fake for name in readiness.HASHES}):
                self.assertEqual(readiness.inspect_owned_toolset(run,base,pins),'verified_candidate')
                extra=root/'unexpected'; extra.write_bytes(b'x'); self.assertEqual(readiness.inspect_owned_toolset(run,base,pins),'unsafe'); extra.unlink()
                p=root/'makensis.exe'; p.write_bytes(b'changed'); self.assertEqual(readiness.inspect_owned_toolset(run,base,pins),'unsafe'); p.write_bytes(b'synthetic')
                def fail_walk(*args,**kwargs):
                    kwargs['onerror'](PermissionError('synthetic'))
                    return iter(())
                with mock.patch.object(readiness.os,'walk',side_effect=fail_walk):
                    self.assertEqual(readiness.inspect_owned_toolset(run,base,pins),'unsafe')
                (run/'owner').write_text('b'*32); self.assertEqual(readiness.inspect_owned_toolset(run,base,pins),'unsafe')
    @unittest.skipUnless(os.name=='nt','Windows hardlink')
    def test_hardlink_in_owned_toolset_rejected(self):
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            base=Path(tmp); run=base/('run-'+'a'*32); root=run/'target/.tauri/NSIS'; root.mkdir(parents=True); (run/'owner').write_text('a'*32)
            for name in readiness.REQUIRED:
                p=root/name; p.parent.mkdir(parents=True,exist_ok=True); p.write_bytes(b'synthetic')
            link=run/'synthetic-link'; os.link(root/'makensis.exe',link)
            try:
                self.assertEqual(readiness.inspect_nsis(root),'unsafe')
            finally: link.unlink()
    def test_missing_and_bad_hash_block(self):
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            root=Path(tmp); self.assertEqual(readiness.inspect_nsis(root),'missing')
            for name in readiness.REQUIRED:
                p=root/name; p.parent.mkdir(parents=True,exist_ok=True); p.write_bytes(b'synthetic')
            self.assertEqual(readiness.inspect_nsis(root),'hash_mismatch')
    def test_fake_upstream_hashes_only_test_own_fixture(self):
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            root=Path(tmp)
            for name in readiness.REQUIRED:
                p=root/name; p.parent.mkdir(parents=True,exist_ok=True); p.write_bytes(b'synthetic')
            fake=hashlib.sha1(b'synthetic',usedforsecurity=False).hexdigest()
            with mock.patch.object(readiness,'HASHES',{name:fake for name in readiness.HASHES}):
                self.assertEqual(readiness.inspect_nsis(root),'matches_upstream_precheck')
                (root/'Include/MUI2.nsh').unlink(); self.assertEqual(readiness.inspect_nsis(root),'missing')

if __name__=='__main__': unittest.main()
