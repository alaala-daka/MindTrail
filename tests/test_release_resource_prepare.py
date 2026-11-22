"""Isolated end-to-end producer resource preparation."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT=Path(__file__).resolve().parent.parent
class ResourcePreparationTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(dir=ROOT/'.tools'); self.root=Path(self.temp.name); self.run_id='a'*32
        output=subprocess.run(['pwsh','-NoProfile','-File',str(ROOT/'tests/fixtures/release-source-list.ps1'),'-Library',str(ROOT/'scripts/release-source.ps1'),'-Root',str(ROOT)],capture_output=True,text=True,check=True,timeout=30)
        files=json.loads(output.stdout)
        self.run=self.root/'.tools/productization-release'/('run-'+self.run_id)
        for name in files:
            for destination in (self.root/name,self.run/'source'/name):
                destination.parent.mkdir(parents=True,exist_ok=True); shutil.copyfile(ROOT/name,destination)
        (self.root/'tests').mkdir(exist_ok=True); shutil.copyfile(ROOT/'tests/release_resource_prepare.ps1',self.root/'tests/release_resource_prepare.ps1')
        (self.run/'owner').write_text(self.run_id,encoding='ascii')
        self.binary=self.run/'target/x86_64-pc-windows-msvc/release/mindtrail-producer.exe'; self.binary.parent.mkdir(parents=True)
        data=bytearray(512); data[:2]=b'MZ'; data[60:64]=(128).to_bytes(4,'little'); data[128:134]=b'PE\0\0\x64\x86'; self.binary.write_bytes(data)
        self.digest=hashlib.sha256(data).hexdigest()
        (self.run/'producer-build.json').write_text(json.dumps({'target':'x86_64-pc-windows-msvc','features':['producer'],'sha256':self.digest}))
    def tearDown(self): self.temp.cleanup()
    def prepare(self):
        return subprocess.run(['pwsh','-NoProfile','-File',str(self.root/'tests/release_resource_prepare.ps1'),'-RunId',self.run_id],capture_output=True,text=True,timeout=60)
    def test_prepares_only_matching_producer(self):
        result=self.prepare(); self.assertEqual(result.returncode,0,result.stdout)
        output=self.run/'source/src-tauri/resources/producer/mindtrail-producer.exe'
        self.assertEqual(hashlib.sha256(output.read_bytes()).hexdigest(),self.digest)
        self.assertNotEqual(self.prepare().returncode,0)
    def test_changed_binary_never_prepares_resource(self):
        self.binary.write_bytes(b'changed')
        self.assertNotEqual(self.prepare().returncode,0)
        self.assertFalse((self.run/'source/src-tauri/resources').exists())
    def transfer_fixture(self):
        destination_id='b'*32; dest=self.root/'.tools/productization-release'/('run-'+destination_id)
        shutil.copytree(self.run/'source',dest/'source'); (dest/'target').mkdir(); (dest/'owner').write_text(destination_id,encoding='ascii')
        (self.run/'producer-diagnostic.json').write_text(json.dumps({'exit_code':0,'timed_out':False}))
        (self.run/'producer-toolchain-inspection-v2.json').write_text('{}')
        text=(ROOT/'tests/release_transfer_producer.ps1').read_text(encoding='utf8').replace('02bb4e8bbebd45da906552912bc6cb2a',self.run_id).replace('240b2bb2b6967bc41f0960947b9ed8661eb0d61e05110ad9269352bded2b16ee',self.digest)
        script=self.root/'tests/release_transfer_producer.ps1'; script.write_text(text,encoding='utf8')
        return destination_id,dest,script
    def test_verified_producer_transfer(self):
        dest_id,dest,script=self.transfer_fixture()
        result=subprocess.run(['pwsh','-NoProfile','-File',str(script),'-DestinationRun',dest_id],capture_output=True,text=True,timeout=60)
        self.assertEqual(result.returncode,0,result.stdout)
        copied=dest/'target/x86_64-pc-windows-msvc/release/mindtrail-producer.exe'
        self.assertEqual(hashlib.sha256(copied.read_bytes()).hexdigest(),self.digest)
    def test_changed_producer_is_not_transferred(self):
        dest_id,dest,script=self.transfer_fixture(); self.binary.write_bytes(b'changed')
        result=subprocess.run(['pwsh','-NoProfile','-File',str(script),'-DestinationRun',dest_id],capture_output=True,text=True,timeout=60)
        self.assertNotEqual(result.returncode,0); self.assertFalse((dest/'target/x86_64-pc-windows-msvc/release/mindtrail-producer.exe').exists())

if __name__=='__main__': unittest.main()
