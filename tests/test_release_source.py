"""Synthetic source admission checks; no installation or build."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
PROBE = ROOT / 'tests/fixtures/release-source-probe.ps1'
LIB = ROOT / 'scripts/release-source.ps1'
FIXED = ('package.json','package-lock.json','index.html','vite.config.ts','tsconfig.json','tsconfig.node.json',
         'src-tauri/Cargo.toml','src-tauri/Cargo.lock','src-tauri/build.rs','src-tauri/tauri.conf.json',
         'scripts/release-lib.ps1','scripts/release-source.ps1','scripts/release-source-verify.ps1','scripts/release-environment.ps1','scripts/release-cargo-wrapper.cs','scripts/release-cargo-preflight.cs','scripts/release_cargo_policy.py','scripts/release_dependencies.py','scripts/release_environment.py','scripts/release_job.py','scripts/release_launcher.py','scripts/release_nsis_extract.py','scripts/release_nsis_readiness.py','scripts/release_producer_build.py','scripts/release_tools.py','scripts/prepare-producer.ps1',
         'scripts/release-build-hook.ps1','scripts/prepare-release-stage.ps1','scripts/verify-release.ps1',
         'src/main.ts','src-tauri/src/lib.rs','src-tauri/icons/icon.png')

class SourceAdmissionTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir=ROOT / '.tools')
        self.root = Path(self.tmp.name)
        for name in FIXED:
            p = self.root / name
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_bytes(b'synthetic')
    def tearDown(self): self.tmp.cleanup()
    def run_probe(self, *args):
        return subprocess.run(['pwsh','-NoProfile','-File',str(PROBE),'-Library',str(LIB),'-Root',str(self.root),*args],capture_output=True,text=True,timeout=30)
    def test_excluded_configs_and_included_changes(self):
        before = self.run_probe(); self.assertEqual(before.returncode, 0)
        for name in ('.npmrc','.cargo/config.toml','.env','node_modules/package.json'):
            p=self.root/name; p.parent.mkdir(parents=True,exist_ok=True); p.write_text('SYNTHETIC_PRIVATE_SENTINEL')
        after = self.run_probe(); self.assertEqual(after.returncode,0)
        self.assertEqual(before.stdout,after.stdout); self.assertNotIn('SYNTHETIC_PRIVATE_SENTINEL',after.stdout+after.stderr)
        (self.root/'src/new.ts').write_text('synthetic change')
        changed=self.run_probe(); self.assertEqual(changed.returncode,0); self.assertNotEqual(before.stdout,changed.stdout)
    def test_required_reference_and_unapproved_inputs(self):
        (self.root/'tsconfig.node.json').unlink(); self.assertNotEqual(self.run_probe().returncode,0)
        (self.root/'tsconfig.node.json').write_text('synthetic')
        for name in ('src/.env','src/secret.json','src/unknown.bin','src/.cargo/config.json'):
            p=self.root/name; p.parent.mkdir(parents=True,exist_ok=True); p.write_text('SYNTHETIC_PRIVATE_SENTINEL')
            result=self.run_probe(); self.assertNotEqual(result.returncode,0); self.assertNotIn('SYNTHETIC_PRIVATE_SENTINEL',result.stdout+result.stderr)
            p.unlink()
            if p.parent.name == '.cargo': p.parent.rmdir()
    @unittest.skipUnless(os.name == 'nt','Windows hardlink check')
    def test_hardlinked_source_rejected(self):
        link=self.root/'other'; os.link(self.root/'src/main.ts',link)
        try: self.assertNotEqual(self.run_probe().returncode,0)
        finally: link.unlink()
    def test_snapshot_mismatch_rejected(self): self.assertNotEqual(self.run_probe('-Mismatch').returncode,0)

if __name__ == '__main__': unittest.main()
