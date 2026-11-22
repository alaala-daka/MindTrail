import json
import os
from pathlib import Path
import sys
import tempfile
import unittest

ROOT=Path(__file__).resolve().parent.parent
sys.path.insert(0,str(ROOT/'tests')); sys.path.insert(0,str(ROOT/'scripts'))
from release_desktop_diagnostics import capture_desktop_job

class DesktopDiagnosticsTests(unittest.TestCase):
    def setUp(self):
        windows=Path(os.environ['SystemRoot']); self.env={'SystemRoot':str(windows),'WINDIR':str(windows),'PATH':str(windows/'System32')}
    def test_failure_categories_have_sources_and_no_raw_output(self):
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            code='import sys; print("Error in script SYNTHETIC_SECRET"); sys.stderr.write("Failed to bundle app with makensis error[E0432] SYNTHETIC_SECRET"); sys.exit(37)'
            result=capture_desktop_job([sys.executable,'-I','-c',code],cwd=tmp,env=self.env,timeout=10)
            self.assertEqual(result['exit_code'],37); self.assertTrue(result['stdout']['nsis_script_error']); self.assertFalse(result['stderr']['nsis_script_error'])
            self.assertTrue(result['stderr']['makensis_failed']); self.assertFalse(result['stdout']['makensis_failed']); self.assertEqual(result['codes'],['E0432'])
            self.assertNotIn('SYNTHETIC_SECRET',json.dumps(result))
    def test_timeout_is_not_success(self):
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            result=capture_desktop_job([sys.executable,'-I','-c','import time; time.sleep(30)'],cwd=tmp,env=self.env,timeout=.5)
            self.assertTrue(result['timed_out']); self.assertIsNone(result['exit_code'])

if __name__=='__main__': unittest.main()
