import os
import json
from pathlib import Path
import sys
import tempfile
import unittest

ROOT=Path(__file__).resolve().parent.parent
sys.path.insert(0,str(ROOT/'scripts'))
from release_producer_build import capture_job,capture_build_job,producer_environment,validate_pe

class ProducerCandidateTests(unittest.TestCase):
    def test_pe_validation_rejects_invalid_and_fixture_binary(self):
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            p=Path(tmp)/'fake.exe'; data=bytearray(512); data[:2]=b'MZ'; data[60:64]=(128).to_bytes(4,'little'); data[128:134]=b'PE\0\0\x64\x86'; p.write_bytes(data)
            self.assertEqual(len(validate_pe(p)),64)
            data[132:134]=b'\x4c\x01'; p.write_bytes(data)
            with self.assertRaises(ValueError): validate_pe(p)
            data[132:134]=b'\x64\x86'; data[200:200+len(b'MINDTRAIL_TEST_ROOT')]=b'MINDTRAIL_TEST_ROOT'; p.write_bytes(data)
            with self.assertRaises(ValueError): validate_pe(p)
    def test_capture_output_is_bounded(self):
        windows=Path(os.environ['SystemRoot']); env={'SystemRoot':str(windows),'WINDIR':str(windows),'PATH':str(windows/'System32')}
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            code,out,err=capture_job([sys.executable,'-I','-c','print("synthetic")'],cwd=tmp,env=env,timeout=10,temporary=tmp)
            self.assertEqual(code,0); self.assertEqual(out.strip(),'synthetic'); self.assertEqual(err,'')
            with self.assertRaisesRegex(ValueError,'output_limit_exceeded'):
                capture_job([sys.executable,'-I','-c','import sys; sys.stdout.write("x"*2000000)'],cwd=tmp,env=env,timeout=10,temporary=tmp)
    def test_failure_diagnostics_preserve_only_fixed_evidence(self):
        windows=Path(os.environ['SystemRoot']); env={'SystemRoot':str(windows),'WINDIR':str(windows),'PATH':str(windows/'System32')}
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            code='import sys; sys.stderr.write("SYNTHETIC_SECRET_SENTINEL error[E0432] permission denied LNK1104"); sys.exit(37)'
            result=capture_build_job([sys.executable,'-I','-c',code],cwd=tmp,env=env,timeout=10)
            self.assertEqual(result['exit_code'],37); self.assertFalse(result['timed_out'])
            self.assertTrue(result['flags']['access_denied']); self.assertEqual(result['codes'],['E0432','LNK1104'])
            self.assertNotIn('SYNTHETIC_SECRET_SENTINEL',json.dumps(result))
    def test_failure_diagnostics_timeout(self):
        windows=Path(os.environ['SystemRoot']); env={'SystemRoot':str(windows),'WINDIR':str(windows),'PATH':str(windows/'System32')}
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            result=capture_build_job([sys.executable,'-I','-c','import time; time.sleep(30)'],cwd=tmp,env=env,timeout=.5)
            self.assertTrue(result['timed_out']); self.assertIsNone(result['exit_code'])
    def test_producer_override_does_not_modify_desktop_environment(self):
        desktop={'PATH':'synthetic'}; producer=producer_environment(desktop)
        self.assertNotIn('TAURI_CONFIG',desktop)
        self.assertEqual(json.loads(producer['TAURI_CONFIG']),{'bundle':{'resources':None}})
        config=json.loads((ROOT/'src-tauri/tauri.conf.json').read_text(encoding='utf8'))
        self.assertEqual(config['bundle']['resources'],{'resources/producer/mindtrail-producer.exe':'producer/mindtrail-producer.exe'})
    def test_missing_resource_classification_does_not_save_path(self):
        windows=Path(os.environ['SystemRoot']); env={'SystemRoot':str(windows),'WINDIR':str(windows),'PATH':str(windows/'System32')}
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            code='import sys; sys.stderr.write("resource path `SYNTHETIC_SECRET/mindtrail-producer.exe` doesn\'t exist"); sys.exit(101)'
            result=capture_build_job([sys.executable,'-I','-c',code],cwd=tmp,env=env,timeout=10)
            self.assertTrue(result['flags']['producer_resource_missing']); self.assertNotIn('SYNTHETIC_SECRET',json.dumps(result))

if __name__=='__main__': unittest.main()
