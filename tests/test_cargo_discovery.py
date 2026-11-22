"""Synthetic, offline Cargo config-discovery probe; performs no build."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT=Path(__file__).resolve().parent.parent
TOOLCHAIN=ROOT/'.tools/rustup/toolchains/stable-x86_64-pc-windows-msvc/bin'

class CargoDiscoveryTest(unittest.TestCase):
    def test_cwd_not_manifest_controls_discovery(self):
        system_root=Path(os.environ['SystemRoot'])
        temp_root=system_root/'Temp'
        for directory in (temp_root,system_root,system_root.anchor):
            for name in ('.cargo/config','.cargo/config.toml'):
                self.assertFalse((Path(directory)/name).exists(), 'unsafe_ancestor_config')
        with tempfile.TemporaryDirectory(prefix='mindtrail-cargo-discovery-',dir=temp_root) as tmp:
            base=Path(tmp); clean=base/'clean'; source=base/'source'; home=base/'cargo-home'
            self.assertEqual(base.resolve().parent,temp_root.resolve(),'unexpected_fixture_parent')
            self.assertTrue(base.name.startswith('mindtrail-cargo-discovery-'))
            for p in (clean,source/'src',source/'.cargo',home): p.mkdir(parents=True)
            (source/'Cargo.toml').write_text('[package]\nname="synthetic_probe"\nversion="0.0.0"\nedition="2021"\n',encoding='utf8')
            (source/'src/lib.rs').write_text('',encoding='utf8')
            (source/'.cargo/config.toml').write_text('[synthetic_invalid\n',encoding='utf8')
            env={'SystemRoot':str(system_root),'WINDIR':str(system_root),'PATH':str(system_root/'System32'),
                 'TEMP':str(base),'TMP':str(base),'USERPROFILE':str(base),'HOME':str(base),
                 'CARGO_HOME':str(home),'CARGO_NET_OFFLINE':'true','RUSTC':str(TOOLCHAIN/'rustc.exe')}
            command=[str(TOOLCHAIN/'cargo.exe'),'metadata','--no-deps','--offline','--format-version','1','--manifest-path',str(source/'Cargo.toml')]
            safe=subprocess.run(command,cwd=clean,env=env,capture_output=True,timeout=30)
            self.assertEqual(safe.returncode,0,'clean_cwd_failed')
            self.assertEqual(json.loads(safe.stdout)['packages'][0]['name'],'synthetic_probe')
            configured=subprocess.run(command,cwd=source,env=env,capture_output=True,timeout=30)
            self.assertNotEqual(configured.returncode,0,'synthetic_config_not_observed')
            self.assertIn(b'config.toml',configured.stderr,'synthetic_config_failure_not_identified')
            self.assertIn(b'TOML',configured.stderr,'synthetic_toml_failure_not_identified')

if __name__ == '__main__': unittest.main()
