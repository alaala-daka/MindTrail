"""Synthetic release-manifest checks; never installs a package."""
import hashlib
import http.server
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import threading
import unittest
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
VERIFY = ROOT / 'scripts/verify-release.ps1'
FILES = ('desktop/MindTrail.exe', 'producer/mindtrail-producer.exe', 'installer/MindTrail-setup.exe', 'nsis/installer.nsi')
PWSH_PINNED = Path(r'C:\Program Files\WindowsApps\OpenAI.CodexPrimaryRuntime.v26-1119-907-0_26.1119.907.0_x64__3k8sg7r9htsxt\dependencies\native\powershell\pwsh.exe')


def pwsh():
    for candidate in (os.environ.get('MINDTRAIL_PWSH'), str(PWSH_PINNED), shutil.which('pwsh')):
        if candidate and Path(candidate).is_file():
            return candidate
    raise unittest.SkipTest('PowerShell 7 is not available')


class ReleaseManifestTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=ROOT / '.tools')
        self.root = Path(self.temp.name).resolve()
        items = []
        for name in FILES:
            path = self.root / name; path.parent.mkdir(parents=True, exist_ok=True); path.write_bytes(('synthetic:' + name).encode())
            payload = path.read_bytes(); items.append({'path':name,'size':len(payload),'sha256':hashlib.sha256(payload).hexdigest()})
        self.manifest = {'version':'0.1.0','target':'x86_64-pc-windows-msvc','source_commit':'a'*40,'source_dirty':True,'source_digest':'b'*64,'files':items,'signing':'unsigned_candidate','checks':{'build':'passed','package_static':'passed','installation':'not_run','upgrade':'not_run','uninstall':'not_run','real_agent':'not_run','semantic_provider':'not_run'}}
    def tearDown(self): self.temp.cleanup()
    def verify(self):
        (self.root/'manifest.json').write_text(json.dumps(self.manifest),encoding='utf8')
        return self.run_verify()
    def run_verify(self):
        return subprocess.run([pwsh(),'-NoProfile','-File',str(VERIFY),'-ReleaseDirectory',str(self.root)],stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=20).returncode
    def test_valid_static_candidate(self): self.assertEqual(self.verify(),0)
    def test_missing_producer_or_installer(self):
        for name in (FILES[1],FILES[2]):
            path=self.root/name; data=path.read_bytes(); path.unlink()
            self.assertNotEqual(self.verify(),0); path.write_bytes(data)
    def test_bad_hash_size_and_duplicate(self):
        for field,value in (('sha256','0'*64),('size',0)):
            old=self.manifest['files'][0][field]; self.manifest['files'][0][field]=value
            self.assertNotEqual(self.verify(),0); self.manifest['files'][0][field]=old
        for duplicate in ('desktop/MindTrail.exe','DESKTOP/mindtrail.exe'):
            self.manifest['files'][1]=dict(self.manifest['files'][0],path=duplicate)
            (self.root/'manifest.json').write_text(json.dumps(self.manifest),encoding='utf8')
            result=subprocess.run([pwsh(),'-NoProfile','-File',str(VERIFY),'-ReleaseDirectory',str(self.root)],capture_output=True,text=True,timeout=20)
            self.assertNotEqual(result.returncode,0)
            self.assertIn('release_duplicate_path',result.stdout)
    @unittest.skipUnless(os.name == 'nt', 'Windows hardlink test')
    def test_hardlinks_to_manifest_and_payload_are_rejected(self):
        self.assertEqual(self.verify(),0)
        for name in ('manifest.json',FILES[0]):
            link=self.root/'hardlink-synthetic'
            os.link(self.root/name,link)
            try: self.assertNotEqual(self.run_verify(),0)
            finally: link.unlink()
    def test_unsafe_paths(self):
        for name in ('../outside','..\\outside','C:\\outside','/absolute','desktop/../x'):
            old=self.manifest['files'][0]['path']; self.manifest['files'][0]['path']=name
            self.assertNotEqual(self.verify(),0); self.manifest['files'][0]['path']=old
    def test_unverified_or_misrepresented_metadata(self):
        for field,value in (('version','unknown'),('target','unknown'),('source_digest','bad'),('source_dirty',None),('signing','signed')):
            old=self.manifest[field]; self.manifest[field]=value
            self.assertNotEqual(self.verify(),0); self.manifest[field]=old
        self.manifest['checks']['installation']='passed'; self.assertNotEqual(self.verify(),0)
    def test_duplicate_json_keys_are_rejected(self):
        payload=json.dumps(self.manifest)
        (self.root/'manifest.json').write_text(payload.replace('"version": "0.1.0"','"version": "bad", "version": "0.1.0"'),encoding='utf8')
        self.assertNotEqual(self.run_verify(),0)
    @unittest.skipUnless(os.name == 'nt', 'Windows symbolic link test')
    def test_linked_payload_is_rejected(self):
        target=self.root/FILES[0]; saved=target.read_bytes(); other=self.root/'other.bin'; other.write_bytes(saved); target.unlink()
        try:
            os.symlink(other,target)
        except OSError:
            self.skipTest('symbolic links unavailable')
        self.assertNotEqual(self.verify(),0)


class NsisArtifactTests(unittest.TestCase):
    RELEASE = ROOT / '.tools/productization-release/run-02bb4e8bbebd45da906552912bc6cb2a/release'

    def setUp(self):
        if not (self.RELEASE / 'manifest.json').is_file():
            self.skipTest('sealed candidate is unavailable')
        self.manifest = json.loads((self.RELEASE / 'manifest.json').read_text(encoding='utf8'))
        self.script = (self.RELEASE / 'nsis/installer.nsi').read_text(encoding='utf-8-sig')
        self.files = {item['path']: item for item in self.manifest['files']}

    def test_script_installs_desktop_producer_uninstaller_and_shortcuts(self):
        self.assertIn('!define MAINBINARYNAME "mindtrail"', self.script)
        self.assertIn('!define INSTALLMODE "currentUser"', self.script)
        self.assertIn('StrCpy $INSTDIR "$LOCALAPPDATA\\${PRODUCTNAME}"', self.script)
        self.assertIn('File "${MAINBINARYSRCPATH}"', self.script)
        self.assertIn('File /a "/oname=producer\\mindtrail-producer.exe"', self.script)
        self.assertIn('WriteUninstaller "$INSTDIR\\uninstall.exe"', self.script)
        self.assertIn('CreateShortcut "$SMPROGRAMS', self.script)
        self.assertIn('CreateShortcut "$DESKTOP', self.script)
        self.assertIn('Delete "$INSTDIR\\${MAINBINARYNAME}.exe"', self.script)
        self.assertIn('Delete "$INSTDIR\\producer\\mindtrail-producer.exe"', self.script)
        self.assertIn('Delete "$INSTDIR\\uninstall.exe"', self.script)
        self.assertEqual(len(re.findall(r'^\s*File\s+"\$\{MAINBINARYSRCPATH\}"$', self.script, re.MULTILINE)), 1)
        self.assertEqual(len(re.findall(r'^\s*WriteUninstaller\s+"\$INSTDIR\\uninstall\.exe"$', self.script, re.MULTILINE)), 1)

    def test_script_sources_match_manifest(self):
        match = re.search(r'!define MAINBINARYSRCPATH "([^"]+)"', self.script)
        self.assertIsNotNone(match)
        desktop = Path(match.group(1))
        self.assertTrue(desktop.is_absolute() and desktop.is_file())
        self.assertEqual(hashlib.sha256(desktop.read_bytes()).hexdigest(), self.files['desktop/MindTrail.exe']['sha256'])
        producer = re.search(r'^\s*File\s+/a\s+"/oname=producer[\\/]mindtrail-producer\.exe"\s+"([^"]+)"$', self.script, re.MULTILINE)
        self.assertIsNotNone(producer)
        producer_path = Path(producer.group(1))
        self.assertTrue(producer_path.is_absolute() and producer_path.is_file())
        self.assertEqual(hashlib.sha256(producer_path.read_bytes()).hexdigest(), self.files['producer/mindtrail-producer.exe']['sha256'])


class NsisValidatorTests(unittest.TestCase):
    RELEASE = ROOT / '.tools/productization-release/run-02bb4e8bbebd45da906552912bc6cb2a/release'

    def setUp(self):
        if not (self.RELEASE / 'manifest.json').is_file():
            self.skipTest('sealed candidate is unavailable')
        path = ROOT / 'tests/release_desktop_candidate.py'
        spec = importlib.util.spec_from_file_location('release_desktop_candidate', path)
        self.candidate = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.candidate)
        self.script = (self.RELEASE / 'nsis/installer.nsi').read_text(encoding='utf-8-sig')
        manifest = json.loads((self.RELEASE / 'manifest.json').read_text(encoding='utf8'))
        self.producer_hash = next(item['sha256'] for item in manifest['files'] if item['path'] == 'producer/mindtrail-producer.exe')
        self.desktop = Path(re.search(r'!define MAINBINARYSRCPATH "([^"]+)"', self.script).group(1))
        self.producer = Path(re.search(r'^\s*File\s+/a\s+"/oname=producer[\\/]mindtrail-producer\.exe"\s+"([^"]+)"$', self.script, re.MULTILINE).group(1))

    def validate(self, script):
        return self.candidate.validate_nsis_script(script, [self.producer], self.producer_hash, self.desktop)

    def test_sealed_script_passes(self):
        self.assertTrue(self.validate(self.script))

    def test_desktop_uninstaller_install_mode_and_shortcuts_are_required(self):
        variants = (
            self.script.replace('File "${MAINBINARYSRCPATH}"', 'File "${MAINBINARYSRCPATH}.bak"'),
            self.script.replace('WriteUninstaller "$INSTDIR\\uninstall.exe"', 'WriteUninstaller "$INSTDIR\\other.exe"'),
            self.script.replace('!define INSTALLMODE "currentUser"', '!define INSTALLMODE "perMachine"'),
            self.script.replace('CreateShortcut "$DESKTOP\\${PRODUCTNAME}.lnk"', 'CreateShortcut "$DESKTOP\\Other.lnk"'),
            self.script.replace('Delete "$INSTDIR\\${MAINBINARYNAME}.exe"', 'Delete "$INSTDIR\\other.exe"'),
        )
        for script in variants:
            with self.assertRaises(ValueError):
                self.validate(script)


class ProducerCopyTests(unittest.TestCase):
    def test_failed_build_never_replaces_existing_resource(self):
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            root=Path(tmp); old=root/'resource.exe'; old.write_bytes(b'existing')
            stale=root/'stale.exe'; pe=bytearray(256); pe[:2]=b'MZ'; pe[60:64]=(128).to_bytes(4,'little'); pe[128:132]=b'PE\0\0'; pe[132:134]=(0x8664).to_bytes(2,'little'); stale.write_bytes(pe)
            cmd=[pwsh(),'-NoProfile','-File',str(ROOT/'tests/fixtures/release-copy-probe.ps1'),'-Library',str(ROOT/'scripts/release-lib.ps1'),'-BuiltPath',str(stale),'-Destination',str(old)]
            self.assertNotEqual(subprocess.run(cmd+['-BuildExitCode','1'],capture_output=True,timeout=20).returncode,0)
            self.assertEqual(old.read_bytes(),b'existing')
            self.assertEqual(subprocess.run(cmd+['-BuildExitCode','0'],capture_output=True,timeout=20).returncode,0)
            self.assertEqual(old.read_bytes(),bytes(pe))
            pe[132:134]=(0x14c).to_bytes(2,'little'); stale.write_bytes(pe)
            self.assertNotEqual(subprocess.run(cmd+['-BuildExitCode','0'],capture_output=True,timeout=20).returncode,0)
            self.assertNotEqual(old.read_bytes(),bytes(pe))
            pe[132:134]=(0x8664).to_bytes(2,'little'); pe[160:179]=b'MINDTRAIL_TEST_ROOT'; stale.write_bytes(pe)
            self.assertNotEqual(subprocess.run(cmd+['-BuildExitCode','0'],capture_output=True,timeout=20).returncode,0)
            stale.unlink()
            self.assertNotEqual(subprocess.run(cmd+['-BuildExitCode','0'],capture_output=True,timeout=20).returncode,0)


def load_tool(path, name):
    import importlib.util
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class DesktopReportTests(unittest.TestCase):
    def setUp(self):
        self.module = load_tool(ROOT / 'tests/productization-desktop.py', 'productization_desktop')
        self.report = {
            'schema': 1, 'kind': 'productization_desktop',
            'manifest': {'path': 'Cargo.toml', 'sha256': 'a' * 64},
            'run': 'run-synthetic',
            'steps': [{'name': name, 'status': 'passed', 'evidence': {}, 'limitation': ''}
                      for name in self.module.STEPS],
            'result': 'passed', 'limitations': [],
        }

    def test_all_passed_steps_can_pass(self):
        self.assertTrue(self.module.validate_report(self.report))

    def test_failed_step_cannot_be_reported_as_passed(self):
        self.report['steps'][0]['status'] = 'failed'
        with self.assertRaises(self.module.StepError):
            self.module.validate_report(self.report)


class InstallerReportTests(unittest.TestCase):
    def setUp(self):
        self.module = load_tool(ROOT / 'tests/installer-acceptance.py', 'installer_acceptance')
        self.steps = [{'name': name, 'status': 'passed', 'evidence': None, 'limitation': ''} for name in self.module.STEPS]
        by_name = {step['name']: step for step in self.steps}
        by_name['installed_files']['evidence'] = {'producer': {'sha256': 'a' * 64}, 'desktop': {'sha256': 'b' * 64}}
        by_name['upgrade']['evidence'] = {'old_version': '0.0.9', 'new_version': '0.1.0'}
        by_name['uninstall_data_retained']['evidence'] = {'retained': True}
        self.isolation = {'environment_id': 'vm-1', 'hypervisor': 'hyperv', 'snapshot_id': 'snap-1',
                          'snapshot_sha256': 'c' * 64, 'restore_tested': True, 'network_blocked': True,
                          'preinstalled_webview2': True, 'windows_11_x64': True}
        self.report = {'schema': 1, 'kind': 'installer_acceptance',
                       'environment': {'id': 'vm-1', 'isolation': self.isolation},
                       'installer': {'path': 'MindTrail-setup.exe', 'size': 10, 'sha256': 'd' * 64},
                       'steps': self.steps, 'result': 'passed'}
    def test_valid_report(self):
        self.assertTrue(self.module.validate_report(self.report))
    def test_manifest_installer_path_prefix_is_accepted(self):
        manifest_installer = {'path': 'installer/MindTrail-setup.exe', 'size': 10, 'sha256': 'd' * 64}
        self.assertTrue(self.module.validate_report(self.report, manifest_installer=manifest_installer))
        manifest_installer['sha256'] = 'e' * 64
        with self.assertRaises(self.module.ReportError):
            self.module.validate_report(self.report, manifest_installer=manifest_installer)
        manifest_installer['sha256'] = 'd' * 64; manifest_installer['size'] = 11
        with self.assertRaises(self.module.ReportError):
            self.module.validate_report(self.report, manifest_installer=manifest_installer)
    def test_manifest_installer_basename_mismatch_is_rejected(self):
        with self.assertRaises(self.module.ReportError):
            self.module.validate_report(self.report, manifest_installer={
                'path': 'installer/other.exe', 'size': 10, 'sha256': 'd' * 64})
    def test_missing_step_cannot_pass(self):
        self.report['steps'] = self.report['steps'][:-1]
        with self.assertRaises(self.module.ReportError): self.module.validate_report(self.report)
    def test_missing_isolation_cannot_pass(self):
        self.report['environment']['isolation'] = None
        with self.assertRaises(self.module.ReportError): self.module.validate_report(self.report)
    def test_missing_installed_producer_cannot_pass(self):
        {step['name']: step for step in self.steps}['installed_files']['evidence'] = {'desktop': {'sha256': 'b' * 64}}
        with self.assertRaises(self.module.ReportError): self.module.validate_report(self.report)
    def test_unchanged_upgrade_version_cannot_pass(self):
        {step['name']: step for step in self.steps}['upgrade']['evidence'] = {'old_version': '0.1.0', 'new_version': '0.1.0'}
        with self.assertRaises(self.module.ReportError): self.module.validate_report(self.report)
    def test_raw_or_unknown_fields_are_rejected(self):
        self.steps[0]['evidence'] = {'prompt': 'secret'}
        with self.assertRaises(self.module.ReportError): self.module.validate_report(self.report)
        self.steps[0]['evidence'] = None
        self.report['raw_response'] = 'secret'
        with self.assertRaises(self.module.ReportError): self.module.validate_report(self.report)


class SemanticLiveTests(unittest.TestCase):
    def setUp(self):
        self.module = load_tool(ROOT / 'tests/semantic-live.py', 'semantic_live')
        self.temp = tempfile.TemporaryDirectory(dir=ROOT / '.tools')
        self.report_dir = Path(self.temp.name) / 'report'
        self.env = {key: os.environ.get(key) for key in ('MINDTRAIL_AI_BASE_URL', 'MINDTRAIL_AI_MODEL', 'MINDTRAIL_AI_API_KEY')}
        for key in self.env: os.environ.pop(key, None)
        self.calls = []
        original = self.module.run_live
        def fake(manifest, report, config):
            self.calls.append(config)
            report.update({'provider': 'openai_compatible', 'model': config['model'], 'endpoint_origin': config['endpoint_origin'],
                           'prompt_version': 'v1', 'input_counts': {'events': 5, 'evidence': 1}, 'prefix_seq': 5,
                           'request_count': 1, 'result_category': 'passed', 'citations_valid': True,
                           'desktop_display': True})
            report['checks'] = {name: 'passed' for name in self.module.CHECK_KEYS}
            return report
        self.module.run_live = fake
        self.restore = original
    def tearDown(self):
        self.module.run_live = self.restore
        for key, value in self.env.items():
            if value is None: os.environ.pop(key, None)
            else: os.environ[key] = value
        self.temp.cleanup()
    def args(self, *extra): return ['--desktop-fixture-manifest', str(ROOT / 'src-tauri/Cargo.toml'), '--report', str(self.report_dir), *extra]
    def install_live_fakes(self, request_count=1, events_after=b'synthetic-events',
                           task_after='真实语义验收 · 已完成 · 5', citation=True,
                           preview_events=5, preview_evidence=1):
        module = self.module
        state = {'sent': False, 'citation': citation, 'task_reads': 0}
        calls = {'events': 0}
        class FakePage:
            def evaluate(self, expression):
                if 'JSON.parse' in expression:
                    return {'events': [{}] * preview_events, 'evidence': [{}] * preview_evidence}
                if 'document.activeElement' in expression:
                    return state['citation'] is True
                if "=== '发送并生成'" in expression:
                    state['sent'] = True
                    return True
                if '=== "原话"' in expression:
                    if not state['citation']: return False
                    state['citation'] = True
                    return True
                if expression.lstrip().startswith('['):
                    return True
                if '?.textContent' in expression:
                    state['task_reads'] += 1
                    return task_after if state['task_reads'] >= 3 else '真实语义验收 · 已完成 · 5'
                if '.innerText' in expression:
                    if state['sent']:
                        return 'AI 解读 · openai_compatible/model · 前缀 5 原话'
                    return ('发送预览 服务：openai_compatible / model · https://provider.invalid '
                            '任务：live · 节点：api · 前缀：5 5 条事件 · 1 条原话 · 99 字节')
                return True
            def close(self): pass
        class FakeJob:
            process = object()
        class FakeFixture:
            def __init__(self, *args, **kwargs): self.job = FakeJob()
            def endpoint(self): return {'port': 1, 'token': 'synthetic'}
            def page(self): return FakePage()
            def close(self): pass
        class FakePreview:
            def __init__(self, *args, **kwargs): pass
            def close(self): pass
        class FakeProxy:
            def __init__(self, upstream):
                self.url = 'http://127.0.0.1:1/v1'
                self.requests = [{'started': 0.0, 'status': 200, 'duration': 0.001}] * request_count
            def close(self): pass
        class FakeDesktop:
            StepError = Exception
            Preview = FakePreview
            Fixture = FakeFixture
            def base_env(self): return {}
            def build_fixture(self, *args, **kwargs): return ('fixture', 'dist', 'producer')
            def free_port(self): return 1
            def http_json(self, *args, **kwargs): return 200
            def wait_for(self, fn, process, timeout=30): return fn()
            def click(self, page, expression): return page.evaluate(expression)
            def graph(self, page): return {'nodes': [], 'edges': [1, 2, 3]}
        def events(*args, **kwargs):
            calls['events'] += 1
            return b'synthetic-events' if calls['events'] == 1 else events_after
        module.ROOT = Path(self.temp.name)
        module.load_desktop_module = lambda: FakeDesktop()
        module.CountingProxy = FakeProxy
        module.gateway_events_bytes = events
        module.run_live = self.restore
        return state
    def test_without_live_does_not_call_provider(self):
        self.assertEqual(self.module.main(self.args()), 2)
        self.assertEqual(self.calls, [])
        self.assertEqual(json.loads((self.report_dir / 'result.json').read_text())['result_category'], 'live_not_requested')
    def test_missing_credentials_does_not_call_provider(self):
        self.assertEqual(self.module.main(self.args('--live')), 2)
        self.assertEqual(self.calls, [])
        self.assertEqual(json.loads((self.report_dir / 'result.json').read_text())['result_category'], 'blocked_missing_credentials')
    def test_disallowed_url_does_not_call_provider(self):
        os.environ.update(MINDTRAIL_AI_BASE_URL='http://127.0.0.1/v1', MINDTRAIL_AI_MODEL='m', MINDTRAIL_AI_API_KEY='k')
        self.assertEqual(self.module.main(self.args('--live')), 2)
        self.assertEqual(self.calls, [])
    def test_existing_report_is_refused(self):
        self.report_dir.mkdir()
        self.assertEqual(self.module.main(self.args('--live')), 1)
        self.assertEqual(self.calls, [])
    def test_live_with_credentials_runs_once(self):
        os.environ.update(MINDTRAIL_AI_BASE_URL='https://provider.invalid/v1', MINDTRAIL_AI_MODEL='model', MINDTRAIL_AI_API_KEY='synthetic')
        self.assertEqual(self.module.main(self.args('--live')), 0)
        self.assertEqual(len(self.calls), 1)
        self.assertEqual(self.calls[0]['endpoint_origin'], 'https://provider.invalid')

    def test_live_runner_failure_is_not_mislabeled_as_missing_dependency(self):
        os.environ.update(MINDTRAIL_AI_BASE_URL='https://provider.invalid/v1', MINDTRAIL_AI_MODEL='model', MINDTRAIL_AI_API_KEY='synthetic')
        def fail(manifest, report, config): raise RuntimeError('synthetic live failure')
        self.module.run_live = fail
        self.assertEqual(self.module.main(self.args('--live')), 2)
        self.assertEqual(json.loads((self.report_dir / 'result.json').read_text())['result_category'], 'harness_failure')

    def test_live_runner_passed_with_failed_check_is_harness_failure(self):
        os.environ.update(MINDTRAIL_AI_BASE_URL='https://provider.invalid/v1', MINDTRAIL_AI_MODEL='model', MINDTRAIL_AI_API_KEY='synthetic')
        def inconsistent(manifest, report, config):
            report.update({'provider': 'openai_compatible', 'model': 'model',
                           'endpoint_origin': 'https://provider.invalid', 'prompt_version': 'mindtrail-insight-v1',
                           'input_counts': {'events': 5, 'evidence': 1}, 'prefix_seq': 5,
                           'request_count': 1, 'result_category': 'passed', 'citations_valid': True,
                           'desktop_display': True})
            report['checks'] = {name: 'passed' for name in self.module.CHECK_KEYS}
            report['checks']['no_retry'] = 'failed'
            return report
        self.module.run_live = inconsistent
        self.assertEqual(self.module.main(self.args('--live')), 2)
        self.assertEqual(json.loads((self.report_dir / 'result.json').read_text())['result_category'], 'harness_failure')

    def test_run_live_measures_wiring_without_real_desktop(self):
        self.install_live_fakes()
        module = self.module
        report = module.base_report(ROOT / 'src-tauri/Cargo.toml', 'blocked_missing_dependency', [])
        config = {'base_url': 'https://provider.invalid/v1', 'model': 'model',
                  'key': 'synthetic', 'endpoint_origin': 'https://provider.invalid'}
        result = module.run_live(ROOT / 'src-tauri/Cargo.toml', report, config)
        module.validate_report(result)
        self.assertEqual(result['result_category'], 'passed')
        self.assertEqual(result['prefix_seq'], 5)
        self.assertEqual(result['input_counts'], {'events': 5, 'evidence': 1})
        self.assertEqual(result['request_count'], 1)
        self.assertTrue(result['citations_valid'])
        self.assertTrue(all(value == 'passed' for value in result['checks'].values()))

    def test_run_live_changed_facts_or_task_blocks_success(self):
        self.install_live_fakes(events_after=b'changed-events', task_after='真实语义验收 · 已完成 · 6')
        module = self.module
        report = module.base_report(ROOT / 'src-tauri/Cargo.toml', 'blocked_missing_dependency', [])
        config = {'base_url': 'https://provider.invalid/v1', 'model': 'model',
                  'key': 'synthetic', 'endpoint_origin': 'https://provider.invalid'}
        result = module.run_live(ROOT / 'src-tauri/Cargo.toml', report, config)
        self.assertEqual(result['checks']['facts_unchanged'], 'failed')
        self.assertEqual(result['checks']['task_state_unchanged'], 'failed')
        with self.assertRaises(ValueError): module.validate_report(result)

    def test_run_live_preview_scope_must_match_posted_events(self):
        self.install_live_fakes(preview_events=4)
        module = self.module
        report = module.base_report(ROOT / 'src-tauri/Cargo.toml', 'blocked_missing_dependency', [])
        config = {'base_url': 'https://provider.invalid/v1', 'model': 'model',
                  'key': 'synthetic', 'endpoint_origin': 'https://provider.invalid'}
        with self.assertRaises(Exception):
            module.run_live(ROOT / 'src-tauri/Cargo.toml', report, config)

    def test_run_live_retry_blocks_success(self):
        self.install_live_fakes(request_count=2)
        module = self.module
        report = module.base_report(ROOT / 'src-tauri/Cargo.toml', 'blocked_missing_dependency', [])
        config = {'base_url': 'https://provider.invalid/v1', 'model': 'model',
                  'key': 'synthetic', 'endpoint_origin': 'https://provider.invalid'}
        result = module.run_live(ROOT / 'src-tauri/Cargo.toml', report, config)
        self.assertEqual(result['checks']['single_request'], 'failed')
        self.assertEqual(result['checks']['no_retry'], 'failed')
        with self.assertRaises(ValueError): module.validate_report(result)

    def test_run_live_unrevealed_citation_blocks_success(self):
        self.install_live_fakes(citation=False)
        module = self.module
        report = module.base_report(ROOT / 'src-tauri/Cargo.toml', 'blocked_missing_dependency', [])
        config = {'base_url': 'https://provider.invalid/v1', 'model': 'model',
                  'key': 'synthetic', 'endpoint_origin': 'https://provider.invalid'}
        result = module.run_live(ROOT / 'src-tauri/Cargo.toml', report, config)
        self.assertEqual(result['result_category'], 'invalid_output')
        self.assertFalse(result['citations_valid'])
        self.assertEqual(result['checks']['citations_same_prefix'], 'failed')

    def test_passed_report_requires_measured_checks(self):
        base = self.module.base_report(ROOT / 'src-tauri/Cargo.toml', 'live_not_requested', [])
        base.update({'provider': 'openai_compatible', 'model': 'model',
                     'endpoint_origin': 'https://provider.invalid', 'prompt_version': 'mindtrail-insight-v1',
                     'input_counts': {'events': 5, 'evidence': 1}, 'prefix_seq': 5,
                     'request_count': 1, 'result_category': 'passed', 'citations_valid': True,
                     'desktop_display': True})
        base['checks'] = {name: 'passed' for name in self.module.CHECK_KEYS}
        self.assertTrue(self.module.validate_report(base))
        base['checks']['no_retry'] = 'failed'
        with self.assertRaises(ValueError): self.module.validate_report(base)

    def test_report_rejects_raw_or_unknown_fields(self):
        base = self.module.base_report(ROOT / 'src-tauri/Cargo.toml', 'live_not_requested', [])
        self.assertTrue(self.module.validate_report(base))
        raw = dict(base); raw['raw_response'] = 'secret'
        with self.assertRaises(ValueError): self.module.validate_report(raw)
        unknown = json.loads(json.dumps(base)); unknown['checks']['raw_text'] = 'secret'
        with self.assertRaises(ValueError): self.module.validate_report(unknown)

    def test_prompt_version_matches_rust_source(self):
        source = (ROOT / 'src-tauri/src/semantic/service.rs').read_text(encoding='utf8')
        match = re.search(r'PROMPT_VERSION: &str = "([^"]+)"', source)
        self.assertIsNotNone(match)
        self.assertEqual(self.module.PROMPT_VERSION, match.group(1))

    def test_counting_proxy_forwards_and_counts_one_request(self):
        module = self.module
        class Upstream(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass
            def do_POST(self):
                self.rfile.read(int(self.headers.get('Content-Length', '0')))
                self.server.count += 1
                payload = b'{"ok":true}'
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)
        upstream = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Upstream)
        upstream.count = 0
        thread = threading.Thread(target=upstream.serve_forever, daemon=True)
        thread.start()
        proxy = module.CountingProxy(f'http://127.0.0.1:{upstream.server_address[1]}/v1')
        try:
            request = urllib.request.Request(proxy.url + '/chat/completions', data=b'{"x":1}',
                                             headers={'Content-Type': 'application/json', 'Authorization': 'Bearer synthetic'},
                                             method='POST')
            with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=10) as response:
                self.assertEqual(response.status, 200)
            self.assertEqual(upstream.count, 1)
            self.assertEqual(len(proxy.requests), 1)
            self.assertEqual(proxy.requests[0]['status'], 200)
        finally:
            proxy.close()
            upstream.shutdown()
            upstream.server_close()


if __name__ == '__main__': unittest.main()
