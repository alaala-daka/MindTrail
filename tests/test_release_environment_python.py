import os
import hashlib
from pathlib import Path
import sys
import tempfile
import unittest

ROOT=Path(__file__).resolve().parent.parent
sys.path.insert(0,str(ROOT/'scripts'))
from release_environment import build_environment, check_cargo_cwd
from release_launcher import run_pinned_job, run_isolated_python

class ChildEnvironmentTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(dir=ROOT/'.tools'); base=Path(self.temp.name)
        self.run=base/('run-'+'a'*32); self.run.mkdir(); (self.run/'owner').write_text('a'*32,encoding='ascii')
        for name in ('source','target','temp','cargo-home','empty-home','npm-cache','tools'): (self.run/name).mkdir()
        for name in ('npm-user.conf','npm-global.conf','git-global.conf'): (self.run/name).touch()
        self.system=Path(os.environ['SystemRoot'])
    def tearDown(self): self.temp.cleanup()
    def build(self): return build_environment(self.run,self.system/'System32',self.system,[self.run/'tools'],self.run/'tools/rustc.exe')
    def test_no_ambient_secrets_or_options(self):
        os.environ['SYNTHETIC_SECRET']='SYNTHETIC_PRIVATE_SENTINEL'
        try:
            env=self.build(); self.assertNotIn('SYNTHETIC_SECRET',env); self.assertNotIn('SYNTHETIC_PRIVATE_SENTINEL',str(env))
            self.assertEqual(env['CARGO_NET_OFFLINE'],'true'); self.assertEqual(env['NPM_CONFIG_OFFLINE'],'true')
        finally: del os.environ['SYNTHETIC_SECRET']
    def test_config_and_cargo_credentials_rejected(self):
        for path in (self.run/'npm-user.conf',self.run/'cargo-home/credentials.toml'):
            path.write_text('SYNTHETIC_PRIVATE_SENTINEL')
            with self.assertRaises(ValueError): self.build()
            if path.parent.name=='cargo-home': path.unlink()
            else: path.write_bytes(b'')
    def test_generated_cargo_cache_files_are_allowed(self):
        for name in ('.global-cache','.package-cache','.package-cache-mutate'):
            (self.run/'cargo-home'/name).write_bytes(b'synthetic')
        self.assertTrue(self.build())
    def test_owner_must_match(self):
        (self.run/'owner').write_text('b'*32)
        with self.assertRaises(ValueError): self.build()
    def test_quarantined_run_cannot_launch(self):
        (self.run/'.release-quarantine').write_text('failed')
        with self.assertRaisesRegex(ValueError,'quarantined_run'): self.build()
    def test_cwd_config_rejected_before_searching_ancestors(self):
        cargo=self.run/'source/.cargo'; cargo.mkdir(); (cargo/'config.toml').write_text('synthetic')
        with self.assertRaisesRegex(ValueError,'ancestor_config_present'): check_cargo_cwd(self.run/'source')
    def test_pinned_job_checks_tool_before_launch(self):
        tool=Path(sys.executable); digest=hashlib.sha256(tool.read_bytes()).hexdigest()
        kwargs=dict(expected_executable=tool,run=self.run,cargo_cwd=self.system/'System32',system_root=self.system,tool_directories=[self.run/'tools'],rustc=self.run/'tools/rustc.exe',timeout_seconds=10)
        self.assertEqual(run_pinned_job([str(tool),'-c','import sys; sys.exit(23)'],expected_sha256=digest,**kwargs),23)
        with self.assertRaises(ValueError): run_pinned_job([str(tool),'-c','raise RuntimeError()'],expected_sha256='0'*64,**kwargs)
    def test_isolated_python_does_not_import_script_directory(self):
        tool=Path(sys.executable); digest=hashlib.sha256(tool.read_bytes()).hexdigest(); source=self.run/'source'
        (source/'sentinel_untrusted.py').write_text('raise RuntimeError()',encoding='ascii')
        script=source/'probe.py'; script.write_text('import importlib.util,sys\nsys.exit(23 if importlib.util.find_spec("sentinel_untrusted") is None else 99)\n',encoding='ascii')
        kwargs=dict(expected_executable=tool,expected_sha256=digest,run=self.run,cargo_cwd=self.system/'System32',system_root=self.system,tool_directories=[self.run/'tools'],rustc=self.run/'tools/rustc.exe',timeout_seconds=10)
        self.assertEqual(run_isolated_python(tool,script,[],**kwargs),23)

if __name__=='__main__': unittest.main()
