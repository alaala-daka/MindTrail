"""Process environment admission using synthetic inputs only."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT=Path(__file__).resolve().parent.parent
class ReleaseEnvironmentTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory(dir=ROOT/'.tools'); self.root=Path(self.tmp.name)
        for name in ('source','temp','cargo-home','empty-home','npm-cache','target','rustup'):
            (self.root/name).mkdir()
        for name in ('npm-user.conf','npm-global.conf','git-global.conf'): (self.root/name).touch()
    def tearDown(self): self.tmp.cleanup()
    def probe(self, *args):
        return subprocess.run(['pwsh','-NoProfile','-File',str(ROOT/'tests/fixtures/release-environment-probe.ps1'),'-Library',str(ROOT/'scripts/release-environment.ps1'),'-SourceLibrary',str(ROOT/'scripts/release-source.ps1'),'-Root',str(self.root),*args],capture_output=True,text=True,timeout=30)
    def test_child_environment_is_allowlisted(self):
        result=self.probe(); self.assertEqual(result.returncode,0,result.stdout); self.assertNotIn('SYNTHETIC_PRIVATE_SENTINEL',result.stdout+result.stderr)
    def test_ancestor_config_blocks(self):
        for name in ('.npmrc','.cargo/config','.cargo/config.toml'):
            p=self.root/name; p.parent.mkdir(parents=True,exist_ok=True); p.write_text('SYNTHETIC_PRIVATE_SENTINEL')
            result=self.probe(); self.assertNotEqual(result.returncode,0); self.assertNotIn('SYNTHETIC_PRIVATE_SENTINEL',result.stdout+result.stderr)
            p.unlink()
    def test_nonempty_config_blocks(self):
        (self.root/'npm-user.conf').write_text('SYNTHETIC_PRIVATE_SENTINEL')
        self.assertNotEqual(self.probe().returncode,0)
    def test_cargo_credentials_and_config_block(self):
        for name in ('credentials.toml','config.toml'):
            path=self.root/'cargo-home'/name; path.write_text('SYNTHETIC_PRIVATE_SENTINEL')
            result=self.probe(); self.assertNotEqual(result.returncode,0); self.assertNotIn('SYNTHETIC_PRIVATE_SENTINEL',result.stdout+result.stderr)
            path.unlink()
    def test_build_entrypoints_remain_blocked(self):
        for name in ('prepare-producer.ps1','release-build-hook.ps1'):
            result=subprocess.run(['pwsh','-NoProfile','-File',str(ROOT/'scripts'/name)],capture_output=True,text=True,timeout=15)
            self.assertNotEqual(result.returncode,0)
            self.assertIn('release_environment_not_admitted',result.stderr)
    def test_direct_child_timeout_is_reaped(self):
        result=self.probe('-TimeoutTest'); self.assertEqual(result.returncode,0,result.stdout); self.assertIn('timeout_reaped',result.stdout)

if __name__ == '__main__': unittest.main()
