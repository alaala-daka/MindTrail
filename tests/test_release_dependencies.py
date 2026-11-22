import base64
import hashlib
import importlib.util
import io
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock

ROOT=Path(__file__).resolve().parent.parent
spec=importlib.util.spec_from_file_location('release_dependencies',ROOT/'scripts/release_dependencies.py')
module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)

class ReleaseDependenciesTests(unittest.TestCase):
    def test_cargo_is_pinned_to_public_archive_and_digest(self):
        p=module.cargo_candidate({'name':'sample','version':'1.2.3','source':'registry+https://github.com/rust-lang/crates.io-index','checksum':'a'*64})
        self.assertEqual(p['url'],'https://static.crates.io/crates/sample/sample-1.2.3.crate'); self.assertEqual(p['digest'],'a'*64)
        with self.assertRaises(ValueError): module.cargo_candidate({'name':'../bad','version':'1','checksum':'a'*64,'source':'registry+https://github.com/rust-lang/crates.io-index'})
    def test_npm_derives_canonical_candidate_with_integrity(self):
        entry={'version':'1.2.3','resolved':'https://mirror.example/package','integrity':'sha512-'+base64.b64encode(bytes(64)).decode()}
        result=module.npm_candidate('node_modules/@scope/sample',entry)
        self.assertEqual(result['url'],'https://registry.npmjs.org/@scope/sample/-/sample-1.2.3.tgz'); self.assertEqual(result['digest'],'00'*64)
        for key,value in (('resolved','https://user:secret@example.com/x'),('integrity','sha512-invalid'),('version','../x')):
            bad=dict(entry); bad[key]=value
            with self.assertRaises(ValueError): module.npm_candidate('node_modules/sample',bad)
        for path in ('node_modules/../bad','node_modules/.','node_modules/..','node_modules/@scope/..','node_modules/@../sample','node_modules/@./sample'):
            with self.subTest(path=path),self.assertRaises(ValueError): module.npm_candidate(path,entry)
    @unittest.skipUnless(os.name=='nt','Windows exclusive rename')
    def test_staging_requires_matching_hash_and_never_overwrites(self):
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            dest=Path(tmp)/'package'; data=b'synthetic-archive'; digest=hashlib.sha256(data).hexdigest()
            for blob,pin,limit in ((data,'0'*64,100),(data,digest,2),(b'',hashlib.sha256(b'').hexdigest(),100)):
                with self.assertRaises(ValueError): module.stage_verified(io.BytesIO(blob),dest,'sha256',pin,Path(tmp),limit)
                self.assertFalse(dest.exists()); self.assertEqual(list(Path(tmp).iterdir()),[])
            module.stage_verified(io.BytesIO(data),dest,'sha256',digest,Path(tmp))
            with self.assertRaises(ValueError): module.stage_verified(io.BytesIO(b'other'),dest,'sha256',digest,Path(tmp))
            self.assertEqual(dest.read_bytes(),data)
    @unittest.skipUnless(os.name=='nt','Windows exclusive rename')
    def test_staging_rehash_detects_tampering(self):
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            dest=Path(tmp)/'package'; data=b'synthetic-archive'; digest=hashlib.sha256(data).hexdigest(); original=module.os.rename
            def tamper(source,target):
                original(source,target); Path(target).write_bytes(b'tampered')
            with mock.patch.object(module.os,'rename',side_effect=tamper):
                with self.assertRaisesRegex(ValueError,'destination_changed'): module.stage_verified(io.BytesIO(data),dest,'sha256',digest,Path(tmp))
            self.assertTrue((Path(tmp)/'.release-quarantine').is_file())
            with self.assertRaisesRegex(ValueError,'invalid_destination'): module.stage_verified(io.BytesIO(data),Path(tmp)/'other','sha256',digest,Path(tmp))
    @unittest.skipUnless(os.name=='nt','Windows exclusive rename')
    def test_download_candidate_is_host_limited_and_hash_checked_without_network(self):
        class FakeResponse(io.BytesIO):
            status=200
        class FakeOpener:
            def open(self,request,timeout):
                self.asserted_timeout=timeout
                return FakeResponse(b'synthetic-archive')
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp, mock.patch.object(module,'build_opener',return_value=FakeOpener()) as opener:
            dest=Path(tmp)/'package'; digest=hashlib.sha256(b'synthetic-archive').hexdigest()
            plan={'url':'https://static.crates.io/crates/sample/sample-1.0.crate','algorithm':'sha256','digest':digest}
            module.download_public(plan,dest,Path(tmp)); self.assertEqual(dest.read_bytes(),b'synthetic-archive')
            for url in ('http://static.crates.io/crates/x','https://user:secret@static.crates.io/crates/x','https://attacker.example/x','https://static.crates.io/crates/%2e%2e/x','https://static.crates.io/crates/x?token=x'):
                with self.subTest(url=url),self.assertRaises(ValueError): module.download_public(dict(plan,url=url),Path(tmp)/'other',Path(tmp))
            self.assertEqual(opener.call_count,1)
    @unittest.skipUnless(os.name=='nt','Windows exclusive rename')
    def test_rejected_stage_does_not_open_network(self):
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp, mock.patch.object(module,'build_opener') as opener:
            root=Path(tmp); dest=root/'package'; plan={'url':'https://static.crates.io/crates/x/x-1.crate','algorithm':'sha256','digest':'a'*64}
            for bad in (dict(plan,algorithm='md5'),dict(plan,digest='bad')):
                with self.assertRaises(ValueError): module.download_public(bad,dest,root)
            dest.write_text('synthetic')
            with self.assertRaises(ValueError): module.download_public(plan,dest,root)
            dest.unlink(); (root/'.release-quarantine').write_text('failed')
            with self.assertRaises(ValueError): module.download_public(plan,dest,root)
            opener.assert_not_called()

if __name__=='__main__': unittest.main()
