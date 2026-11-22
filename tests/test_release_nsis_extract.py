import sys
import hashlib
from pathlib import Path
import tempfile
import unittest
import zipfile

ROOT=Path(__file__).resolve().parent.parent
sys.path.insert(0,str(ROOT/'scripts'))
from release_nsis_extract import extract_nsis,extract_owned

class NsisExtractTests(unittest.TestCase):
    def test_extracts_owned_synthetic_archive(self):
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            base=Path(tmp); parent=base/'.tauri'; parent.mkdir(); z=base/'input.zip'
            with zipfile.ZipFile(z,'w') as out: out.writestr('nsis-3.13/Bin/file.exe',b'synthetic')
            extract_nsis(z,parent/'NSIS'); self.assertEqual((parent/'NSIS/Bin/file.exe').read_bytes(),b'synthetic')
            with self.assertRaises(ValueError): extract_nsis(z,parent/'NSIS')
    def test_rejects_traversal_duplicates_and_reserved(self):
        for names in (['nsis-3.13/../escape'],['nsis-3.13/A','nsis-3.13/a'],['nsis-3.13/CON'],['nsis-3.13/bad?.exe'],['other/file'],['nsis-3.13/a\\b']):
            with self.subTest(names=names),tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
                base=Path(tmp); parent=base/'.tauri'; parent.mkdir(); z=base/'input.zip'
                with zipfile.ZipFile(z,'w') as out:
                    for name in names: out.writestr(name.replace('\\','_'),b'synthetic')
                if any('\\' in name for name in names): z.write_bytes(z.read_bytes().replace(b'a_b',b'a\\b'))
                with self.assertRaises(ValueError): extract_nsis(z,parent/'NSIS')
                self.assertFalse((parent/'NSIS').exists())
    def test_owned_run_and_digest_binding(self):
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            base=Path(tmp); run=base/('run-'+'a'*32); (run/'target').mkdir(parents=True); (run/'nsis-downloads').mkdir(); (run/'owner').write_text('a'*32)
            archive=run/'nsis-downloads/nsis.zip'
            with zipfile.ZipFile(archive,'w') as out: out.writestr('nsis-3.13/bin/file',b'synthetic')
            digest=hashlib.sha256(archive.read_bytes()).hexdigest()
            extract_owned(run,base,digest); self.assertEqual((run/'target/.tauri/NSIS/bin/file').read_bytes(),b'synthetic')
    def test_bad_digest_quarantines_run(self):
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            base=Path(tmp); run=base/('run-'+'a'*32); (run/'target').mkdir(parents=True); (run/'nsis-downloads').mkdir(); (run/'owner').write_text('a'*32)
            with zipfile.ZipFile(run/'nsis-downloads/nsis.zip','w') as out: out.writestr('nsis-3.13/file',b'synthetic')
            with self.assertRaises(ValueError): extract_owned(run,base,'0'*64)
            self.assertTrue((run/'.release-quarantine').exists())
    def test_crc_failure_quarantines_run(self):
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            base=Path(tmp); run=base/('run-'+'a'*32); (run/'target').mkdir(parents=True); (run/'nsis-downloads').mkdir(); (run/'owner').write_text('a'*32); archive=run/'nsis-downloads/nsis.zip'
            with zipfile.ZipFile(archive,'w') as out: out.writestr('nsis-3.13/file',b'synthetic')
            archive.write_bytes(archive.read_bytes().replace(b'synthetic',b'tampered!'))
            with self.assertRaises(zipfile.BadZipFile): extract_owned(run,base,hashlib.sha256(archive.read_bytes()).hexdigest())
            self.assertTrue((run/'.release-quarantine').exists())

if __name__=='__main__': unittest.main()
