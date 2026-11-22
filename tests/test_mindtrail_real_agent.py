import json
import ctypes
import hashlib
import contextlib
import io
import http.server
import os
import subprocess
import sys
import tempfile
import threading
import time
import queue
import shutil
import socket
import unittest
import urllib.error
from unittest.mock import patch
from pathlib import Path


SCRIPT = Path(__file__).with_name('mindtrail_real_agent.py')
sys.path.insert(0, str(SCRIPT.parent))
from mindtrail_real_agent import bounded_stdout, classify_cli_failure, codex_arguments, fixture_environment, model_turn, offline_scenario, online_scenario, parse_jsonl, public_evidence, read_events, register_run, run_owned_turn, safe_existing_path, select_producer_artifact, probe_producer, validate_call_facts, validate_calls, validate_event_projection, validate_live_paths, validate_pages, validate_synthetic_sources, verify_isolated_cli, verify_synthetic_execution, write_report
import mindtrail_real_agent_desktop
from mindtrail_real_agent_desktop import DesktopProcess, PreviewProcess, Workbench, parse_endpoint, validate_graphs
from mindtrail_real_agent_job import OwnedJob
from mindtrail_real_agent_lock import CliLock
import mindtrail_real_agent_job
import mindtrail_real_agent


class ClosedEntryTests(unittest.TestCase):
    def test_cli_failure_classification_never_copies_diagnostics(self):
        self.assertEqual(classify_cli_failure(b'authentication failed SECRET_SENTINEL'), 'turn_failed')
        self.assertEqual(classify_cli_failure(b'unexpected argument SECRET_SENTINEL'), 'turn_failed')
        self.assertEqual(classify_cli_failure(b'windows sandbox SECRET_SENTINEL'), 'turn_failed')
        self.assertEqual(classify_cli_failure(b'unknown SECRET_SENTINEL'), 'turn_failed')

    def test_cli_failure_evidence_requires_error_structure(self):
        def event(error):
            return (json.dumps({'type': 'turn.failed', 'error': error}) + '\n').encode()
        diag = {}
        self.assertEqual(classify_cli_failure(b'HTTP 429 SECRET_SENTINEL', diagnostics=diag), 'turn_failed')
        self.assertTrue(diag['failure_hint_stdout_rate_limited'])
        self.assertFalse(diag['failure_structured_stdout_rate_limited'])
        diag = {}
        self.assertEqual(classify_cli_failure(event({'status_code': 429, 'message': 'SECRET_SENTINEL'}), diagnostics=diag), 'turn_rate_limited')
        self.assertTrue(diag['failure_structured_stdout_rate_limited'])
        self.assertFalse(diag['failure_structured_stderr_rate_limited'])
        self.assertNotIn('SECRET_SENTINEL', json.dumps(public_evidence(diag)))
        self.assertEqual(classify_cli_failure(event({'status': 429, 'code': 'insufficient_quota'})), 'turn_quota_blocked')
        diag = {}
        self.assertEqual(classify_cli_failure(event({'code': 'invalid_api_key'}), event({'status': 429}), diagnostics=diag), 'turn_failed')
        self.assertTrue(diag['failure_structured_conflict'])
        self.assertTrue(diag['failure_structured_stdout_auth'] and diag['failure_structured_stderr_rate_limited'])
        unrelated = (json.dumps({'type': 'item.completed', 'item': {'text': 'quota exceeded SECRET_SENTINEL', 'status': 429}}) + '\n').encode()
        diag = {}
        self.assertEqual(classify_cli_failure(unrelated, diagnostics=diag), 'turn_failed')
        self.assertFalse(diag['failure_hint_stdout_quota'])
        duplicate = b'{"type":"turn.failed","error":{"status":429,"status":401,"message":"SECRET_SENTINEL"}}\n'
        diag = {}
        self.assertEqual(classify_cli_failure(duplicate, diagnostics=diag), 'turn_failed')
        self.assertFalse(diag['failure_structured_stdout_rate_limited'] or diag['failure_structured_stdout_auth'])
        self.assertNotIn('SECRET_SENTINEL', json.dumps(public_evidence(diag)))
        diag = {}
        self.assertEqual(classify_cli_failure(b'not a rate limit', diagnostics=diag), 'turn_failed')
        self.assertTrue(diag['failure_hint_stdout_rate_limited'])
        diag = {}
        self.assertEqual(classify_cli_failure(b'', b'windows sandbox SECRET_SENTINEL', diagnostics=diag), 'turn_failed')
        self.assertTrue(diag['failure_hint_stderr_sandbox'])

    @unittest.skipUnless(os.name == 'nt', 'Windows file sharing test')
    def test_cli_lock_blocks_replacement_until_closed(self):
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent.parent / '.tools') as tmp:
            root = Path(tmp).resolve(); binary = root / 'codex.exe'; binary.write_bytes(b'original')
            replacement = root / 'replacement.exe'; replacement.write_bytes(b'replacement')
            with CliLock(binary, hashlib.sha256(b'original').hexdigest()):
                with self.assertRaises(OSError): os.replace(replacement, binary)
                moved = root.with_name(root.name + '-renamed')
                try:
                    os.rename(root, moved)
                except OSError:
                    pass
                else:
                    os.rename(moved, root)
                    self.fail('locked parent directory was renamed')
                self.assertEqual(binary.read_bytes(), b'original')
            os.replace(replacement, binary)
            with self.assertRaisesRegex(ValueError, 'cli_digest_mismatch'):
                with CliLock(binary, hashlib.sha256(b'original').hexdigest()): pass


    @unittest.skipUnless(os.name == 'nt', 'Windows directory sharing test')
    def test_work_directory_lock_prevents_rename(self):
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent.parent / '.tools') as tmp:
            root = Path(tmp).resolve(); work = root / 'work'; work.mkdir(); moved = root / 'moved'
            owner = work / '.mindtrail-work-owner'; owner.write_bytes(b'synthetic')
            with CliLock(owner, hashlib.sha256(b'synthetic').hexdigest()):
                with self.assertRaises(OSError): os.rename(work, moved)
                (work / 'synthetic.txt').write_text('fixture')
            os.rename(work, moved)

    def test_cli_copy_must_match_installed_binary_and_expected_directory(self):
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent.parent / '.tools') as tmp:
            repo = Path(tmp).resolve(); isolated = repo / '.tools' / ('codex-isolated-' + 'a' * 32); isolated.mkdir(parents=True)
            copied = isolated / 'codex.exe'; copied.write_bytes(b'synthetic')
            installed = repo / 'installed.exe'; installed.write_bytes(b'synthetic')
            with patch.object(mindtrail_real_agent, 'PINNED_CLI_BUNDLE', {'codex.exe': hashlib.sha256(b'synthetic').hexdigest()}):
                self.assertTrue(verify_isolated_cli(copied, repo))
                copied.write_bytes(b'different')
                with self.assertRaises(ValueError): verify_isolated_cli(copied, repo)
                with self.assertRaises(ValueError): verify_isolated_cli(installed, repo)

    def test_bundle_rejects_extra_missing_and_modified_sidecar(self):
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent.parent / '.tools') as tmp:
            repo = Path(tmp).resolve(); bundle = repo / '.tools' / ('codex-isolated-' + 'a' * 32); bundle.mkdir(parents=True)
            exe = bundle / 'codex.exe'; exe.write_bytes(b'cli')
            sidecar = bundle / 'helper.exe'; sidecar.write_bytes(b'helper')
            pins = {'codex.exe': hashlib.sha256(b'cli').hexdigest(), 'helper.exe': hashlib.sha256(b'helper').hexdigest()}
            with patch.object(mindtrail_real_agent, 'PINNED_CLI_BUNDLE', pins):
                self.assertTrue(verify_isolated_cli(exe, repo))
                extra = bundle / 'extra.dll'; extra.write_bytes(b'extra')
                with self.assertRaises(ValueError): verify_isolated_cli(exe, repo)
                extra.unlink(); sidecar.write_bytes(b'changed')
                with self.assertRaises(ValueError): verify_isolated_cli(exe, repo)
                sidecar.unlink()
                with self.assertRaises(ValueError): verify_isolated_cli(exe, repo)

    def test_webview_connection_never_uses_proxy(self):
        target = [{'type': 'page', 'url': 'http://localhost:1420/', 'webSocketDebuggerUrl': 'ws://127.0.0.1:12345/devtools/page/synthetic'}]
        response = unittest.mock.MagicMock(); response.__enter__.return_value.read.return_value = json.dumps(target).encode()
        opener = unittest.mock.Mock(); opener.open.return_value = response
        ws = unittest.mock.Mock(); ws.recv.return_value = json.dumps({'id': 1, 'result': {'result': {'value': True}}})
        job = unittest.mock.Mock(); job.process.poll.return_value = None; job.owns_listener.return_value = True
        with patch('urllib.request.build_opener', return_value=opener), patch('websockets.sync.client.connect', return_value=ws) as connect:
            page = Workbench(12345, job); page.close()
        self.assertIsNone(connect.call_args.kwargs['proxy'])

    def test_offline_orchestration_requires_queued_and_same_retry(self):
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent.parent / '.tools') as tmp:
            root = Path(tmp).resolve(); work = root / 'work'; work.mkdir(); state = root / 'state'; state.mkdir()
            ident = 'a' * 32; session = 'offline-' + ident; qname = 'run-' + ident
            queue_file = state / 'producers' / 'mcp' / qname / 'queue.sqlite3'; queue_file.parent.mkdir(parents=True); queue_file.write_bytes(b'fixture')
            names = list(mindtrail_real_agent.TOOLS); ids = [ident + '-' + suffix for suffix in ('start','intent','api','file','call','done','task-done')]
            values = [{'title': 'MindTrail offline ' + ident, 'goal': '验证合成调用', 'project': 'synthetic'}, {'id': 'c', 'title': 'C'}, {'id': 'a', 'kind': 'fn', 'signature': 'double(value)', 'capability': 'c'}, {'id': 'f', 'path': 'cli.mjs', 'action': 'create', 'capability': 'c'}, {'from': 'f', 'to': 'a', 'kind': 'calls'}, {'id': 'c', 'quote': '命令入口调用 double', 'context': '已用正负输入验证输出'}, {}]
            calls = [(name, {'session': session, 'operation_id': ids[i], **value}, {'status': 'queued'}) for i, (name, value) in enumerate(zip(names, values))]
            kinds = ['task.start','capability.intent','api.expose','file.change','call.edge','capability.done','task.done']; payloads = [dict(v) for v in values]
            payloads[2]['capabilityId'] = payloads[2].pop('capability'); payloads[3]['capabilityId'] = payloads[3].pop('capability'); payloads[3]['stats'] = {'add': 0, 'del': 0}
            events = [{'v': 1, 'session': session, 'seq': i + 1, 'ts': 'synthetic', 'type': kind, 'payload': payloads[i]} for i, kind in enumerate(kinds)]
            first = unittest.mock.Mock(); first.endpoint.return_value = {'store_id': 's', 'token': 'old', 'port': 1234}
            second = unittest.mock.Mock(); second.endpoint.return_value = {'store_id': 's', 'token': 'new', 'port': 1235}
            proc = unittest.mock.Mock(); proc.poll.side_effect = [None, None, None, 0, 0]
            recovery_job = unittest.mock.Mock(); recovery_job.handle = None
            def fake_turn(*args, **kwargs):
                expected = args[9]
                if not expected:
                    kwargs['started'].put((proc, recovery_job)); return []
                if len(expected) == 7: return calls
                return [(names[0], calls[0][1], {'status': 'committed', 'seq': 1})]
            opener = unittest.mock.Mock(); opener.open.side_effect = urllib.error.HTTPError('http://127.0.0.1', 401, 'synthetic', {}, None)
            page = unittest.mock.Mock(); evidence = {}
            sock = unittest.mock.MagicMock(); sock.__enter__.return_value.connect_ex.return_value = 1
            with patch('mindtrail_real_agent_desktop.DesktopProcess', side_effect=[first, second]), patch('mindtrail_real_agent_desktop.Workbench', return_value=page), patch.object(mindtrail_real_agent, 'model_turn', side_effect=fake_turn), patch.object(mindtrail_real_agent, 'recovery_guard_ready', return_value=True), patch.object(mindtrail_real_agent, 'verify_synthetic_execution', return_value=True), patch.object(mindtrail_real_agent, 'read_events', return_value=events), patch.object(mindtrail_real_agent, 'wait_events', return_value=events), patch.object(mindtrail_real_agent.urllib.request, 'build_opener', return_value=opener), patch('socket.socket', return_value=sock):
                offline_scenario('cli', 'home', 'model', object(), {'id': ident, 'work': work, 'state': state, 'queue': qname}, 'repo', object(), 12345, 'node', evidence, time.monotonic()+30)
            self.assertTrue(evidence['offline_receipts'] and evidence['restart'] and evidence['retry'])
            self.assertTrue(evidence['recovery_events_seven_while_alive'] and evidence['automatic_recovery'])
            self.assertFalse(evidence['recovery_process_alive_after_observation'])
            second.close.assert_called_once()
            # Seeing seven events is not enough for this verifier when it must
            # cancel the still-running keepalive process afterward.
            proc.poll.side_effect = [None, None, None, None, 0]
            again = {}
            with patch('mindtrail_real_agent_desktop.DesktopProcess', side_effect=[first, second]), patch('mindtrail_real_agent_desktop.Workbench', return_value=page), patch.object(mindtrail_real_agent, 'model_turn', side_effect=fake_turn), patch.object(mindtrail_real_agent, 'recovery_guard_ready', return_value=True), patch.object(mindtrail_real_agent, 'verify_synthetic_execution', return_value=True), patch.object(mindtrail_real_agent, 'read_events', return_value=events), patch.object(mindtrail_real_agent, 'wait_events', return_value=events), patch.object(mindtrail_real_agent.urllib.request, 'build_opener', return_value=opener), patch('socket.socket', return_value=sock):
                offline_scenario('cli', 'home', 'model', object(), {'id': ident, 'work': work, 'state': state, 'queue': qname}, 'repo', object(), 12345, 'node', again, time.monotonic()+30)
            self.assertTrue(again['recovery_events_seven_while_alive'] and again['recovery_cancelled'] and again['retry'])
            self.assertTrue(again['automatic_recovery'])
            bad_events = [dict(event) for event in events]
            bad_events[2]['type'] = 'task.done'
            proc.poll.side_effect = [None, None, 0]
            fail = {}; turn_mock = unittest.mock.Mock(side_effect=fake_turn)
            with patch('mindtrail_real_agent_desktop.DesktopProcess', side_effect=[first, second]), patch('mindtrail_real_agent_desktop.Workbench', return_value=page), patch.object(mindtrail_real_agent, 'model_turn', turn_mock), patch.object(mindtrail_real_agent, 'recovery_guard_ready', return_value=True), patch.object(mindtrail_real_agent, 'verify_synthetic_execution', return_value=True), patch.object(mindtrail_real_agent, 'read_events', return_value=bad_events), patch.object(mindtrail_real_agent, 'wait_events', return_value=events), patch.object(mindtrail_real_agent.urllib.request, 'build_opener', return_value=opener), patch('socket.socket', return_value=sock):
                with self.assertRaisesRegex(ValueError, '^event_mismatch$'):
                    offline_scenario('cli', 'home', 'model', object(), {'id': ident, 'work': work, 'state': state, 'queue': qname}, 'repo', object(), 12345, 'node', fail, time.monotonic()+30)
            self.assertEqual(turn_mock.call_count, 2)
            self.assertNotIn('recovery_events_seven_while_alive', fail)
            proc.poll.side_effect = [None, None, None, None, 0]
            recovery_job.handle = object(); fail = {}; turn_mock.reset_mock()
            with patch('mindtrail_real_agent_desktop.DesktopProcess', side_effect=[first, second]), patch('mindtrail_real_agent_desktop.Workbench', return_value=page), patch.object(mindtrail_real_agent, 'model_turn', turn_mock), patch.object(mindtrail_real_agent, 'recovery_guard_ready', return_value=True), patch.object(mindtrail_real_agent, 'verify_synthetic_execution', return_value=True), patch.object(mindtrail_real_agent, 'read_events', return_value=events), patch.object(mindtrail_real_agent, 'wait_events', return_value=events), patch.object(mindtrail_real_agent.urllib.request, 'build_opener', return_value=opener), patch('socket.socket', return_value=sock):
                with self.assertRaisesRegex(ValueError, '^recovery_turn_stuck$'):
                    offline_scenario('cli', 'home', 'model', object(), {'id': ident, 'work': work, 'state': state, 'queue': qname}, 'repo', object(), 12345, 'node', fail, time.monotonic()+30)
            self.assertEqual(turn_mock.call_count, 2)

    def test_live_entry_calls_only_the_preflighted_runner(self):
        args = ['--live', '--codex-exe', 'x', '--codex-home', 'x', '--model', 'x', '--manifest', 'x', '--report', 'x']
        with patch.object(mindtrail_real_agent, 'validate_live_paths', return_value=None), patch.object(mindtrail_real_agent, 'execute_live', return_value={'status': 'passed'}) as live, contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(mindtrail_real_agent.main(args), 0)
            live.assert_called_once()
        with patch.object(mindtrail_real_agent, 'validate_live_paths', return_value='invalid_isolation_path'), patch.object(mindtrail_real_agent, 'execute_live') as live, contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(mindtrail_real_agent.main(args), 2)
            live.assert_not_called()

    def test_online_orchestration_requires_prefix_before_completion(self):
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent.parent / '.tools') as tmp:
            work = Path(tmp).resolve(); ident = 'a' * 32; session = 'online-' + ident
            names = list(mindtrail_real_agent.TOOLS)
            values = [{'title': 'MindTrail synthetic ' + ident, 'goal': '验证合成调用', 'project': 'synthetic'}, {'id': 'c', 'title': 'C'}, {'id': 'a', 'kind': 'fn', 'signature': 'double(value)', 'capability': 'c'}, {'id': 'f', 'path': 'cli.mjs', 'action': 'create', 'capability': 'c'}, {'from': 'f', 'to': 'a', 'kind': 'calls'}, {'id': 'c', 'quote': '命令入口调用 double', 'context': '已用正负输入验证输出'}, {}]
            ids = [ident + '-' + name for name in ('start','intent','api','file','call','done','task-done')]
            calls = [(name, {'session': session, 'operation_id': ids[i], **value}, {'status': 'committed', 'seq': i + 1}) for i, (name, value) in enumerate(zip(names, values))]
            kinds = ['task.start','capability.intent','api.expose','file.change','call.edge','capability.done','task.done']
            payloads = [dict(v) for v in values]; payloads[2]['capabilityId'] = payloads[2].pop('capability'); payloads[3]['capabilityId'] = payloads[3].pop('capability'); payloads[3]['stats'] = {'add': 0, 'del': 0}
            events = [{'v': 1, 'session': session, 'seq': i + 1, 'ts': 'synthetic', 'type': kind, 'payload': payloads[i]} for i, kind in enumerate(kinds)]
            page = unittest.mock.Mock(); desktop = unittest.mock.Mock(); desktop.endpoint.return_value = {}
            evidence = {}
            with patch.object(mindtrail_real_agent, 'model_turn', side_effect=[calls[:2], calls[2:]]), patch.object(mindtrail_real_agent, 'wait_events', side_effect=[events[:2], events]), patch.object(mindtrail_real_agent, 'verify_synthetic_execution', return_value=True):
                online_scenario('cli', 'home', 'model', object(), {'id': ident, 'work': work}, 'repo', desktop, page, 'node', evidence, time.monotonic() + 30)
            page.expect_prefix.assert_called_once(); page.expect_complete.assert_called_once()
            self.assertTrue(evidence['online_prefix'] and evidence['online_done'])
            self.assertTrue(all(evidence[k] for k in ('stage_online_calls_done', 'stage_online_read_started', 'stage_online_read_done', 'stage_online_projection_done', 'stage_online_ui_started', 'stage_online_ui_done', 'stage_code_started', 'stage_code_done')))
            failed = {}
            with patch.object(mindtrail_real_agent, 'model_turn', return_value=calls[:2]), patch.object(mindtrail_real_agent, 'wait_events', side_effect=TypeError('SECRET_SENTINEL')):
                with self.assertRaises(TypeError):
                    online_scenario('cli', 'home', 'model', object(), {'id': ident, 'work': work}, 'repo', desktop, page, 'node', failed, time.monotonic() + 30)
            self.assertTrue(failed['stage_online_read_started'])
            self.assertNotIn('stage_online_read_done', failed)
            mindtrail_real_agent.record_exception_type(failed, TypeError('SECRET_SENTINEL'))
            self.assertTrue(failed['exception_type_error'])
            self.assertNotIn('SECRET_SENTINEL', json.dumps(public_evidence(failed)))
            self.assertEqual(public_evidence({'code': 'webview_timeout'})['code'], 'webview_timeout')
            self.assertNotIn('code', public_evidence({'code': 'SECRET_SENTINEL'}))

    def test_complete_ui_failure_reports_only_fixed_stage(self):
        from mindtrail_real_agent_desktop import Workbench
        page = object.__new__(Workbench)
        page.select_task = unittest.mock.Mock(side_effect=ValueError('SECRET_SENTINEL'))
        evidence = {}
        with self.assertRaises(ValueError):
            page.expect_complete('title', 'cap', 'api', 'file', None, evidence=evidence)
        self.assertEqual(evidence, {'stage_complete_select_started': True})
        self.assertNotIn('SECRET_SENTINEL', json.dumps(mindtrail_real_agent.public_evidence(evidence)))

    def test_recovery_guard_rejects_every_tool_call_and_ambiguous_frame(self):
        from mindtrail_recovery_guard import classify_recovery_frame
        def encode(obj): return (json.dumps(obj) + '\n').encode()
        for frame in ({'jsonrpc':'2.0', 'id':1, 'method':'initialize'},
                      {'jsonrpc':'2.0', 'method':'notifications/initialized'},
                      {'jsonrpc':'2.0', 'id':'2', 'method':'tools/list'}):
            self.assertEqual(classify_recovery_frame(encode(frame)), 'startup')
        for frame in (b'{"jsonrpc":"2.0","method":"tools/call","id":1}\n',
                      b'{"jsonrpc":"2.0","method":"tools/list","method":"tools/call","id":1}\n',
                      b'{"jsonrpc":"2.0","method":"tools/list","id":1}',
                      b'{"jsonrpc":"2.0","method":"tools/list","id":true}\n',
                      b'{"jsonrpc":"2.0","method":"SECRET_SENTINEL"}\n'):
            self.assertEqual(classify_recovery_frame(frame), 'reject')

    def test_recovery_guard_requires_ready_live_job_member_and_no_violation(self):
        from mindtrail_real_agent import recovery_guard_ready
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent.parent / '.tools') as tmp:
            root = Path(tmp).resolve(); job = unittest.mock.Mock()
            job.contains_active_pid.return_value = True
            self.assertFalse(recovery_guard_ready(root, job))
            (root / 'observer-recovery-ready').write_text('', encoding='ascii')
            pid = root / 'observer-recovery-pid'; pid.write_text('123', encoding='ascii')
            self.assertTrue(recovery_guard_ready(root, job))
            job.contains_active_pid.assert_called_once_with(123)
            job.contains_active_pid.return_value = False
            self.assertFalse(recovery_guard_ready(root, job))
            job.contains_active_pid.return_value = True
            (root / 'observer-recovery-violation').write_text('', encoding='ascii')
            self.assertFalse(recovery_guard_ready(root, job))

    def test_prefix_requires_planned_capability(self):
        from mindtrail_real_agent_desktop import Workbench
        page = Workbench.__new__(Workbench)
        page.select_task = unittest.mock.Mock()
        page.evaluate = unittest.mock.Mock(return_value=True)
        page.wait = lambda fn, process: fn()
        evidence = {}
        page.expect_prefix('synthetic', 'cap', object(), evidence=evidence)
        self.assertIn('em.planned', page.evaluate.call_args.args[0])
        self.assertTrue(evidence['stage_ui_node_started'] and evidence['stage_ui_node_done'])

    @unittest.skipUnless(os.name == 'nt', 'Windows job test')
    def test_job_listener_ownership_rejects_unrelated_listener(self):
        env = {key: os.environ[key] for key in ('SYSTEMROOT', 'WINDIR', 'PATH', 'TEMP', 'TMP') if key in os.environ}
        code = 'import socket,time;s=socket.socket();s.bind(("127.0.0.1",0));s.listen();print(s.getsockname()[1],flush=True);time.sleep(30)'
        with socket.socket() as unrelated:
            unrelated.bind(('127.0.0.1', 0)); unrelated.listen()
            job = OwnedJob([sys.executable, '-c', code], cwd=SCRIPT.parent, env=env)
            lines = queue.Queue()
            threading.Thread(target=lambda: lines.put(job.process.stdout.readline()), daemon=True).start()
            try:
                owned_port = int(lines.get(timeout=10))
                self.assertTrue(job.owns_listener(owned_port))
                self.assertFalse(job.owns_listener(unrelated.getsockname()[1]))
                self.assertTrue(job.contains_active_pid(job.process.pid))
                self.assertFalse(job.contains_active_pid(os.getpid()))
            finally:
                job.close()
            self.assertFalse(job.contains_active_pid(job.process.pid))

    def test_report_is_exclusive_and_redacted(self):
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent.parent / '.tools') as tmp:
            root = Path(tmp).resolve(); report = root / 'report.json'
            write_report(report, root, {'handshake': True, 'stderr': 'SECRET_SENTINEL'})
            self.assertEqual(json.loads(report.read_text()), {'handshake': True})
            with self.assertRaises(ValueError): write_report(report, root, {'handshake': False})
            self.assertEqual(json.loads(report.read_text()), {'handshake': True})

    @unittest.skipUnless(os.name == 'nt', 'Windows job test')
    def test_job_assignment_failure_closes_handle_even_if_kill_fails(self):
        kernel = unittest.mock.MagicMock()
        kernel.CreateJobObjectW.return_value = 123
        kernel.SetInformationJobObject.return_value = True
        kernel.AssignProcessToJobObject.return_value = False
        child = unittest.mock.Mock()
        child._handle = 456
        child.kill.side_effect = OSError('synthetic')
        child.stdin = unittest.mock.Mock(); child.stdout = unittest.mock.Mock(); child.stderr = unittest.mock.Mock()
        with patch.object(mindtrail_real_agent_job.ctypes, 'WinDLL', return_value=kernel), patch.object(mindtrail_real_agent_job.subprocess, 'Popen', return_value=child):
            with self.assertRaisesRegex(OSError, 'job_assign_failed'):
                OwnedJob(['synthetic'], cwd='.', env={})
        kernel.CloseHandle.assert_called_once_with(123)
        child.stdout.close.assert_called_once()

    def test_desktop_rejects_foreign_webview_before_process_start(self):
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent.parent / '.tools') as tmp:
            repo = Path(tmp).resolve(); runs = repo / '.tools/real-agent-runs'; runs.mkdir(parents=True)
            fixtures = repo / '.tools/mcp-stage0-copy/src-tauri'; fixtures.mkdir(parents=True)
            target = repo / '.tools/producer-stage0-target'; target.mkdir()
            exe = target / 'test.exe'; exe.write_bytes(b'synthetic')
            artifact = mindtrail_real_agent_desktop.DesktopArtifact(exe, hashlib.sha256(exe.read_bytes()).hexdigest(), mindtrail_real_agent_desktop._DESKTOP_BUILD_AUTH)
            fixture_run = register_run(runs, fixtures)
            with patch('mindtrail_real_agent_desktop.OwnedJob') as launch:
                with self.assertRaises(ValueError):
                    DesktopProcess(artifact, {**fixture_run, 'webview': repo}, repo, debug_port=12345)
                launch.assert_not_called()

    def test_endpoint_strict_shape_and_no_duplicate_keys(self):
        valid = {'port': 1234, 'version': 1, 'token': 'a' * 64, 'store_id': 'b' * 64}
        self.assertEqual(parse_endpoint(json.dumps(valid).encode()), valid)
        for raw in (b'{"port":1,"port":2}', json.dumps({**valid, 'token': 'x'}).encode(), json.dumps({**valid, 'extra': 1}).encode(), b'x' * 4097):
            with self.subTest(raw=raw[:30]), self.assertRaises(ValueError):
                parse_endpoint(raw)

    def test_desktop_stop_is_exclusive_and_job_always_closed(self):
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent.parent / '.tools') as tmp:
            root = Path(tmp).resolve(); (root / 'stop').write_text('do not replace')
            desktop = DesktopProcess.__new__(DesktopProcess); desktop.fixture = root
            desktop.job = unittest.mock.Mock()
            with self.assertRaises(FileExistsError): desktop.close()
            desktop.job.close.assert_called_once()
            self.assertEqual((root / 'stop').read_text(), 'do not replace')

    def test_preview_cleanup_runs_even_when_job_close_fails(self):
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent.parent / '.tools') as tmp:
            marker = Path(tmp) / 'marker'; marker.write_text('synthetic')
            preview = PreviewProcess.__new__(PreviewProcess); preview.marker = marker; preview.marker_value = 'synthetic'; preview.job = unittest.mock.Mock()
            preview.job.close.side_effect = OSError('synthetic')
            with self.assertRaises(OSError): preview.close()
            self.assertFalse(marker.exists())

    def test_model_turn_uses_isolated_environment_and_verifies_items(self):
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent.parent / '.tools') as tmp:
            root = Path(tmp).resolve()
            repo = root / 'repo'; repo.mkdir()
            cli = root / 'cli.exe'; cli.write_bytes(b'fixture')
            target = repo / '.tools/real-agent-fixture-target'; target.mkdir(parents=True)
            producer = target / 'producer.exe'; producer.write_bytes(b'MINDTRAIL_TEST_ROOT')
            home = root / 'home'; home.mkdir()
            runs = repo / '.tools/real-agent-runs'; runs.mkdir()
            fixtures = repo / '.tools/mcp-stage0-copy/src-tauri'; fixtures.mkdir(parents=True)
            volume = root / 'volume'; volume.mkdir()
            fixture_run = register_run(runs, fixtures, volume); fixture_run['state'].mkdir()
            artifact = mindtrail_real_agent.BuildArtifact(producer, hashlib.sha256(producer.read_bytes()).hexdigest(), mindtrail_real_agent._BUILD_AUTH)
            item = {'type': 'mcp_tool_call', 'id': 'i', 'server': 'mindtrail', 'tool': 'trace_task_start', 'arguments': {'session': 's', 'operation_id': 'op'}}
            records = [{'type': 'item.started', 'item': item}, {'type': 'item.completed', 'item': {**item, 'result': {'structuredContent': {'status': 'queued', 'session': 's', 'operation_id': 'op'}}}}, {'type': 'turn.completed'}]
            with patch.dict(os.environ, {'OPENAI_API_KEY': 'SECRET_SENTINEL'}), patch.object(mindtrail_real_agent, 'SYNTHETIC_VOLUME', volume), patch.object(mindtrail_real_agent, 'run_owned_turn', return_value=records) as run, patch.object(mindtrail_real_agent, 'CliLock'), patch.object(mindtrail_real_agent, 'locked_cli_bundle', return_value=contextlib.nullcontext()):
                calls = model_turn(cli, home, 'model', artifact, fixture_run, repo, 'synthetic', 's', ['op'], ('trace_task_start',))
                self.assertEqual(len(calls), 1)
                self.assertNotIn('SECRET_SENTINEL', repr(run.call_args.kwargs['env']))
                run.reset_mock()
                with self.assertRaises(ValueError):
                    model_turn(cli, home, 'model', artifact, {**fixture_run, 'work': repo}, repo, 'synthetic', 's', ['op'], ('trace_task_start',))
                run.assert_not_called()
                instructions = home / 'AGENTS.md'; instructions.write_text('SECRET_SENTINEL')
                with self.assertRaisesRegex(ValueError, 'unexpected_instructions'):
                    model_turn(cli, home, 'model', artifact, fixture_run, repo, 'synthetic', 's', ['op'], ('trace_task_start',))
                instructions.unlink()
                run.assert_not_called()
                (volume / '.git').mkdir()
                with self.assertRaisesRegex(ValueError, 'unexpected_instructions'):
                    model_turn(cli, home, 'model', artifact, fixture_run, repo, 'synthetic', 's', ['op'], ('trace_task_start',))
                (volume / '.git').rmdir()
                run.assert_not_called()
                @contextlib.contextmanager
                def inject_marker(path, digest):
                    if Path(path).name == '.mindtrail-work-owner':
                        (volume / '.codex').mkdir()
                    yield
                with patch.object(mindtrail_real_agent, 'CliLock', side_effect=inject_marker):
                    with self.assertRaisesRegex(ValueError, 'unexpected_instructions'):
                        model_turn(cli, home, 'model', artifact, fixture_run, repo, 'synthetic', 's', ['op'], ('trace_task_start',))
                (volume / '.codex').rmdir()
                run.assert_not_called()
                producer.write_bytes(b'MINDTRAIL_TEST_ROOT changed')
                with self.assertRaises(ValueError):
                    model_turn(cli, home, 'model', artifact, fixture_run, repo, 'synthetic', 's', ['op'], ('trace_task_start',))
                run.assert_not_called()

    def test_events_match_normalized_calls_and_receipts(self):
        names = ['trace_task_start', 'trace_intent', 'trace_api', 'trace_file', 'trace_call', 'trace_done', 'trace_task_done']
        kinds = ['task.start', 'capability.intent', 'api.expose', 'file.change', 'call.edge', 'capability.done', 'task.done']
        args = [{'session': 's', 'operation_id': str(i)} for i in range(7)]
        args[3].update({'capability': 'cap', 'add': 2})
        calls = [(name, arg, {'status': 'committed', 'seq': i + 1}) for i, (name, arg) in enumerate(zip(names, args))]
        events = [{'v': 1, 'session': 's', 'seq': i + 1, 'ts': 'synthetic', 'type': kind, 'payload': {}} for i, kind in enumerate(kinds)]
        events[3]['payload'] = {'capabilityId': 'cap', 'stats': {'add': 2, 'del': 0}}
        events[4]['payload'] = {'kind': 'calls'}
        self.assertTrue(validate_event_projection(calls, events))
        events[3]['payload']['stats']['add'] = 3
        with self.assertRaises(ValueError):
            validate_event_projection(calls, events)

    def test_synthetic_call_relationships_and_evidence(self):
        names = ['trace_task_start', 'trace_intent', 'trace_api', 'trace_file', 'trace_call', 'trace_done', 'trace_task_done']
        args = [{'title': 't', 'goal': 'g', 'project': 'p'}, {'id': 'c'}, {'id': 'a', 'kind': 'fn', 'signature': 'double(value)', 'capability': 'c'}, {'id': 'f', 'path': 'cli.mjs', 'action': 'create', 'capability': 'c'}, {'from': 'f', 'to': 'a', 'kind': 'calls'}, {'id': 'c', 'quote': '命令入口调用 double', 'context': '已用正负输入验证输出'}, {}]
        calls = list(zip(names, args, [{}] * 7))
        self.assertEqual(validate_call_facts(calls), ('c', 'a', 'f'))
        calls[4] = ('trace_call', {'from': 'a', 'to': 'f', 'kind': 'calls'}, {})
        with self.assertRaises(ValueError):
            validate_call_facts(calls)

    @unittest.skipUnless(os.name == 'nt', 'Windows job test')
    def test_owned_turn_parses_output_and_times_out_without_exposing_stderr(self):
        env = {key: os.environ[key] for key in ('SYSTEMROOT', 'WINDIR', 'PATH', 'TEMP', 'TMP') if key in os.environ}
        command = [sys.executable, '-c', 'import sys;sys.stdin.buffer.read();print("{}")']
        self.assertEqual(run_owned_turn(command, cwd=SCRIPT.parent, env=env, prompt=b'synthetic', timeout=5), [{}])
        diag = {}
        command = [sys.executable, '-c', 'print(\'{"type":"turn.failed","error":{"status":429,"message":"SECRET_SENTINEL"}}\')']
        with self.assertRaisesRegex(ValueError, '^turn_rate_limited$'):
            run_owned_turn(command, cwd=SCRIPT.parent, env=env, prompt=b'', timeout=5, diagnostics=diag)
        self.assertTrue(diag['failure_structured_stdout_rate_limited'])
        self.assertNotIn('SECRET_SENTINEL', json.dumps(public_evidence(diag)))
        command = [sys.executable, '-c', 'import sys,time;print("SECRET_SENTINEL",file=sys.stderr,flush=True);time.sleep(30)']
        with self.assertRaisesRegex(ValueError, 'turn_timeout') as error:
            run_owned_turn(command, cwd=SCRIPT.parent, env=env, prompt=b'', timeout=.3)
        self.assertNotIn('SECRET_SENTINEL', str(error.exception))
        diagnostic = {}
        script = ('import sys,time; '
                  'print(\'{"type":"thread.started","secret":"SECRET_SENTINEL"}\', flush=True); '
                  'print(\'{"type":"turn.started"}\', flush=True); '
                  'print(\'{"type":"item.completed","item":{"type":"mcp_tool_call"}}\', flush=True); '
                  'print("network auth SECRET_SENTINEL", file=sys.stderr, flush=True); time.sleep(30)')
        with self.assertRaisesRegex(ValueError, 'turn_timeout'):
            run_owned_turn([sys.executable, '-c', script], cwd=SCRIPT.parent, env=env, prompt=b'', timeout=1, diagnostics=diagnostic)
        self.assertTrue(all(diagnostic[k] for k in ('diag_thread_started', 'diag_turn_started', 'diag_tool_call', 'diag_network', 'diag_auth')))
        self.assertFalse(diagnostic['diag_model_event'])
        self.assertNotIn('SECRET_SENTINEL', json.dumps(public_evidence(diagnostic)))
        with self.assertRaisesRegex(ValueError, 'turn_timeout'):
            run_owned_turn(command, cwd=SCRIPT.parent, env=env, prompt=b'x' * 1000000, timeout=.3)
        diagnostic = {}
        command = [sys.executable, '-c', 'import sys;print("SECRET_SENTINEL",file=sys.stderr);sys.exit(1)']
        with self.assertRaisesRegex(ValueError, 'turn_failed'):
            run_owned_turn(command, cwd=SCRIPT.parent, env=env, prompt=b'', timeout=5, diagnostics=diagnostic)
        self.assertTrue(diagnostic['diag_stderr'])
        self.assertNotIn('SECRET_SENTINEL', json.dumps(public_evidence(diagnostic)))
        diagnostic = {}
        command = [sys.executable, '-c', 'import sys;print("handshaking connection closed SECRET_SENTINEL",file=sys.stderr);sys.exit(1)']
        with self.assertRaises(ValueError):
            run_owned_turn(command, cwd=SCRIPT.parent, env=env, prompt=b'', timeout=5, diagnostics=diagnostic)
        self.assertTrue(diagnostic['diag_handshake_stage'] and diagnostic['diag_transport_closed'])
        self.assertNotIn('SECRET_SENTINEL', json.dumps(public_evidence(diagnostic)))
        diagnostic = {}
        command = [sys.executable, '-c', 'import sys;print("queue fixture I/O: directory DACL SECRET_SENTINEL",file=sys.stderr);sys.exit(1)']
        with self.assertRaises(ValueError):
            run_owned_turn(command, cwd=SCRIPT.parent, env=env, prompt=b'', timeout=5, diagnostics=diagnostic)
        self.assertTrue(diagnostic['diag_fixture_queue_io'] and diagnostic['diag_fixture_acl'])
        self.assertNotIn('SECRET_SENTINEL', json.dumps(public_evidence(diagnostic)))

    @unittest.skipUnless(os.name == 'nt', 'Windows job test')
    def test_job_close_terminates_owned_child_tree(self):
        env = {key: os.environ[key] for key in ('SYSTEMROOT', 'WINDIR', 'PATH', 'TEMP', 'TMP') if key in os.environ}
        code = 'import subprocess,sys,time; p=subprocess.Popen([sys.executable,"-c","import time;time.sleep(30)"],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL); print(p.pid,flush=True); time.sleep(30)'
        job = OwnedJob([sys.executable, '-c', code], cwd=SCRIPT.parent, env=env)
        lines = queue.Queue()
        threading.Thread(target=lambda: lines.put(job.process.stdout.readline()), daemon=True).start()
        handle = None
        kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        kernel.OpenProcess.argtypes = [ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
        kernel.OpenProcess.restype = ctypes.c_void_p
        kernel.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
        kernel.WaitForSingleObject.restype = ctypes.c_ulong
        kernel.CloseHandle.argtypes = [ctypes.c_void_p]
        try:
            pid = int(lines.get(timeout=10))
            handle = kernel.OpenProcess(0x00100000, False, pid)
            self.assertTrue(handle)
            self.assertEqual(kernel.WaitForSingleObject(handle, 0), 258)
            job.close()
            self.assertEqual(kernel.WaitForSingleObject(handle, 5000), 0)
        finally:
            job.close()
            if handle:
                kernel.CloseHandle(handle)

    def test_output_limit_stops_owned_process(self):
        env = {key: os.environ[key] for key in ('SYSTEMROOT', 'WINDIR', 'PATH', 'TEMP', 'TMP') if key in os.environ}
        with self.assertRaises(ValueError):
            bounded_stdout([sys.executable, '-c', 'print("x" * 100000)'], cwd=SCRIPT.parent, env=env, timeout=5, limit=100)

    def test_default_entry_runs_probe_but_does_not_claim_full_acceptance(self):
        with patch.object(mindtrail_real_agent, 'local_probe', return_value=True) as probe, patch('mindtrail_real_agent_desktop.local_desktop_probe', return_value=True) as desktop, contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(mindtrail_real_agent.main([]), 2)
            probe.assert_called_once()
            desktop.assert_called_once()

    def test_graphs_require_expected_direction_and_structure(self):
        def edge(source, target, kind):
            return {'source': source, 'target': target, 'kind': kind}
        hierarchy = {'nodes': ['root', 'cap', 'api', 'file'], 'edges': [edge('root', 'cap', 'contains'), edge('cap', 'api', 'contains'), edge('cap', 'file', 'contains')]}
        calls = {'nodes': ['api', 'file'], 'edges': [edge('file', 'api', 'calls')]}
        self.assertTrue(validate_graphs(hierarchy, calls, 'cap', 'api', 'file'))
        with self.assertRaises(ValueError):
            validate_graphs(hierarchy, {'nodes': calls['nodes'], 'edges': [edge('api', 'file', 'calls')]}, 'cap', 'api', 'file')

    def test_jsonl_limits_and_partial_frames_fail_closed(self):
        self.assertEqual(parse_jsonl(b'{"type":"ok"}\n'), [{'type': 'ok'}])
        for value in (b'{"type":"ok"}', b'{}\n\n', b'[]\n', b'\xff\n', b'{"a":1,"a":2}\n'):
            with self.subTest(value=value), self.assertRaises(ValueError):
                parse_jsonl(value)
        with self.assertRaises(ValueError):
            parse_jsonl(b'{}\n', max_total=2)

    def test_evidence_allowlist_excludes_raw_credentials_and_errors(self):
        data = public_evidence({'handshake': True, 'desktop': 'SECRET_SENTINEL', 'stderr': 'SECRET_SENTINEL', 'arguments': {'token': 'SECRET_SENTINEL'}})
        self.assertEqual(data, {'handshake': True})
        self.assertNotIn('SECRET_SENTINEL', json.dumps(data))

    def test_authenticated_loopback_reads_without_proxy(self):
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args): pass
            def do_GET(self):
                if self.headers.get('Authorization') != 'Bearer synthetic':
                    self.send_error(401); return
                if self.path == '/v1/health':
                    body = {'version': 1, 'store_id': 'synthetic-store'}
                elif 'since_seq=0' in self.path:
                    body = {'last_seq': 1, 'events': [{'seq': 1, 'session': 'synthetic'}]}
                else:
                    body = {'last_seq': 1, 'events': []}
                data = json.dumps(body).encode()
                self.send_response(200); self.end_headers(); self.wfile.write(data)
        server = http.server.HTTPServer(('127.0.0.1', 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True); thread.start()
        try:
            endpoint = {'port': server.server_port, 'store_id': 'synthetic-store', 'token': 'synthetic'}
            self.assertEqual(read_events(endpoint, 'synthetic'), [{'seq': 1, 'session': 'synthetic'}])
            with self.assertRaises(ValueError):
                read_events({**endpoint, 'store_id': 'other'}, 'synthetic')
        finally:
            server.shutdown(); thread.join(); server.server_close()

    def test_runs_have_disjoint_work_state_and_queue(self):
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent.parent / '.tools') as tmp:
            root = Path(tmp).resolve()
            runs = root / 'runs'; runs.mkdir()
            fixtures = root / 'fixtures'; fixtures.mkdir()
            first = register_run(runs, fixtures); second = register_run(runs, fixtures)
            self.assertNotEqual(first['id'], second['id'])
            self.assertNotEqual(first['queue'], second['queue'])
            self.assertFalse(first['state'].exists())
            self.assertNotEqual(first['work'], first['state'].parent)
            self.assertTrue(first['webview'].is_dir())

    def test_configuration_is_one_argument_per_override(self):
        args = codex_arguments('cli.exe', 'work', 'model', 'producer.exe', 'state', 'queue"; injected')
        self.assertEqual(args[-1], '-')
        overrides = [args[i + 1] for i, arg in enumerate(args[:-1]) if arg == '-c']
        self.assertEqual(len(overrides), 5)
        raw = next(item.split('=', 1)[1] for item in overrides if '.args=' in item)
        self.assertEqual(json.loads(raw), ['mcp', '--queue', 'queue"; injected'])

    def test_turn_diagnostics_only_report_fixed_execution_flags(self):
        events = [
            {'type': 'item.started', 'item': {'type': 'command_execution', 'command': 'SECRET_SENTINEL'}},
            {'type': 'item.completed', 'item': {'type': 'command_execution', 'status': 'failed', 'output': 'SECRET_SENTINEL'}},
            {'type': 'item.completed', 'item': {'type': 'file_change', 'status': 'declined', 'path': 'SECRET_SENTINEL'}},
            {'type': 'turn.completed'},
        ]
        diagnostics = {}
        raw = b''.join((json.dumps(event) + '\n').encode() for event in events)
        mindtrail_real_agent.record_turn_diagnostics(diagnostics, raw, b'')
        result = public_evidence(diagnostics)
        self.assertTrue(all(result[key] for key in ('diag_command_started', 'diag_command_completed', 'diag_command_failed', 'diag_file_change_completed', 'diag_file_change_failed', 'diag_turn_completed')))
        self.assertNotIn('SECRET_SENTINEL', json.dumps(result))

    def test_approval_overrides_only_cover_fixture_tools(self):
        args = codex_arguments('cli', 'work', 'model', 'producer', 'state', 'queue', approve_tools=True)
        overrides = [args[i + 1] for i, arg in enumerate(args[:-1]) if arg == '-c']
        approvals = [item for item in overrides if 'approval_mode' in item]
        self.assertEqual(set(approvals), {'mcp_servers.mindtrail.tools.' + name + '.approval_mode="approve"' for name in mindtrail_real_agent.TOOLS})
        self.assertEqual(len(approvals), 7)
        default = codex_arguments('cli', 'work', 'model', 'producer', 'state', 'queue')
        self.assertFalse(any('approval_mode' in item for item in default))

    def test_proxy_validation_and_mcp_isolation(self):
        parse = mindtrail_real_agent.parse_user_proxy
        self.assertEqual(parse('https=example.test:8080'), 'http://example.test:8080')
        self.assertEqual(parse('[::1]:8080'), 'http://[::1]:8080')
        for value in ('http://user:secret@example.test:8080', 'socks5://example.test:8080',
                      'example.test:8080/path', 'example.test', 'https=example.test:8080;socks=host:123',
                      'http://example.test:8080?token=secret', 'example.test:8080\nsecret',
                      'example.test\\host:8080', 'example%2etest:8080', '127.1:8080',
                      '-host.test:8080', 'host..test:8080', '[fe80::1%25x]:8080'):
            with self.assertRaisesRegex(ValueError, '^invalid_user_proxy$') as error:
                parse(value)
            self.assertNotIn('secret', str(error.exception))
        args = codex_arguments('cli', 'work', 'model', 'producer', 'state', 'queue', isolate_proxy=True)
        overrides = [args[i + 1] for i, arg in enumerate(args[:-1]) if arg == '-c']
        env = {key.rsplit('.', 1)[-1]: json.loads(value) for key, value in
               (item.split('=', 1) for item in overrides) if '.env.' in key}
        self.assertEqual({k: env[k] for k in ('HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'NO_PROXY')},
                         {'HTTP_PROXY': '', 'HTTPS_PROXY': '', 'ALL_PROXY': '', 'NO_PROXY': '*'})

    def test_synthetic_provider_emits_only_fixed_tool_call(self):
        from mindtrail_synthetic_provider import synthetic_response
        from mindtrail_cli_local_probe import observe_second_request, SAFE_COMMAND_WORDS
        request = {'tools': [{'type': 'namespace', 'name': 'mcp__mindtrail', 'tools': [{'name': 'trace_task_start'}]}], 'secret': 'SECRET_SENTINEL'}
        result = synthetic_response(request, 'session', 'operation', True)
        self.assertIn(b'function_call', result)
        self.assertIn(b'response.function_call_arguments.delta', result)
        self.assertIn(b'response.function_call_arguments.done', result)
        self.assertNotIn(b'SECRET_SENTINEL', result)
        command = synthetic_response({'tools': [{'name': 'exec_command'}]}, 'session', 'operation', True, command=True)
        self.assertIn(b'exec_command', command)
        self.assertIn(b'synthetic-command.txt', command)
        read_only = synthetic_response({'tools': [{'name': 'exec_command'}]}, 'session', 'operation', True, command=True, command_shape='read-only')
        self.assertNotIn(b'synthetic-command.txt', read_only)
        first = synthetic_response({'tools': [{'name': 'exec_command'}]}, 'session', 'operation', True, command=True, command_shape='write-execute', call_index=1)
        second = synthetic_response({'tools': [{'name': 'exec_command'}]}, 'session', 'operation', True, command=True, command_shape='write-execute', call_index=2)
        self.assertIn(b'Set-Content', first)
        self.assertIn(b'node synthetic-command.js', second)
        self.assertIn(b'call_synthetic_2', second)
        with self.assertRaisesRegex(ValueError, '^invalid_command_shape$'):
            synthetic_response({'tools': [{'name': 'exec_command'}]}, 'session', 'operation', True, command=True, command_shape='write-execute', call_index=3)
        with self.assertRaisesRegex(ValueError, '^invalid_command_shape$'):
            synthetic_response({'tools': [{'name': 'exec_command'}]}, 'session', 'operation', True, command=True, command_shape='other')
        with self.assertRaisesRegex(ValueError, '^synthetic_tool_missing$'):
            synthetic_response({'tools': []}, 'session', 'operation', True, command=True)
        with self.assertRaisesRegex(ValueError, '^synthetic_tool_missing$'):
            synthetic_response({'tools': []}, 'session', 'operation', True)
        with self.assertRaisesRegex(ValueError, '^synthetic_tool_missing$'):
            synthetic_response({'tools': request['tools'] * 2}, 'session', 'operation', True)
        with self.assertRaisesRegex(ValueError, '^synthetic_tool_missing$'):
            synthetic_response({'tools': [{'name': 'other_trace_task_start'}]}, 'session', 'operation', True)
        observed = {'second_has_function_output': False}
        observe_second_request(observed, 2, {'input': [{'type': 'function_call_output', 'call_id': 'call_synthetic'}]})
        observe_second_request(observed, 3, {'input': []})
        self.assertTrue(observed['second_has_function_output'])
        observed = {'second_has_function_output': False}
        observe_second_request(observed, 2, {'input': [{'type': 'function_call_output', 'call_id': 'call_synthetic', 'output': 'sandbox failed SECRET_SENTINEL'}]})
        self.assertTrue(observed['command_output_hint_sandbox'] and observed['command_output_hint_tool_error'])
        self.assertNotIn('SECRET_SENTINEL', json.dumps(observed))
        observed = {'second_has_function_output': False}
        observe_second_request(observed, 2, {'input': [{'type': 'function_call_output', 'call_id': 'call_synthetic', 'output': 'approval required by policy, but AskForApproval is set to Never SECRET_SENTINEL'}]})
        self.assertTrue(observed['command_output_phrase_policy_never'])
        self.assertFalse(observed['command_output_phrase_rejected_user'])
        self.assertNotIn('SECRET_SENTINEL', json.dumps(observed))
        self.assertTrue(set(observed['command_output_fixed_tokens']).issubset(SAFE_COMMAND_WORDS))
        observed = {'second_has_function_output': False}
        observe_second_request(observed, 2, {'input': [{'type': 'function_call_output', 'call_id': 'call_synthetic', 'output': '`echo` rejected: blocked by policy `SECRET_SENTINEL`'}]})
        self.assertTrue(observed['command_policy_template_match'] and observed['command_policy_subject_echo'])
        self.assertFalse(observed['command_policy_rule_known'])
        self.assertNotIn('SECRET_SENTINEL', json.dumps(observed))
        observed = {'second_has_function_output': False}
        observe_second_request(observed, 2, {'input': [{'type': 'function_call_output', 'call_id': 'call_synthetic', 'output': 'os error 1260 0x80070005 SECRET_SENTINEL'}]})
        self.assertTrue(observed['command_os_error_1260'] and observed['command_hresult_0x80070005'])
        self.assertFalse(observed['command_os_error_5'])
        self.assertNotIn('SECRET_SENTINEL', json.dumps(observed))
        observed = {'second_has_function_output': False}
        observe_second_request(observed, 2, {'input': [{'type': 'function_call_output', 'call_id': 'call_synthetic', 'output': 'os error 11260 0x180070005'}]})
        self.assertFalse(observed['command_os_error_1260'] or observed['command_hresult_0x80070005'])

    def test_owned_sandbox_log_selection_is_fixed_and_bounded(self):
        from mindtrail_cli_local_probe import observe_sandbox_selection
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent.parent / '.tools') as tmp:
            root = Path(tmp).resolve()
            home = root / ('codex-local-probe-' + 'a' * 32)
            home.mkdir()
            ident = 'b' * 32
            marker = home / '.mindtrail-probe-owner'
            marker.write_text(ident, encoding='ascii')
            empty = observe_sandbox_selection(home, root, ident)
            self.assertFalse(empty['selection_log_present'])
            folder = home / '.sandbox'; folder.mkdir()
            log = folder / 'sandbox.synthetic.log'
            log.write_text('SECRET_SENTINEL\nWindows sandbox selection: elevated\nWindows sandbox selection: mxc-unknown\n', encoding='ascii')
            result = observe_sandbox_selection(home, root, ident)
            self.assertTrue(result['selection_log_present'] and result['selection_elevated'])
            self.assertFalse(result['selection_mxc'])
            self.assertNotIn('SECRET_SENTINEL', json.dumps(result))
            marker.write_text('c' * 32, encoding='ascii')
            with self.assertRaisesRegex(ValueError, '^invalid_probe_home$'):
                observe_sandbox_selection(home, root, ident)

    def test_mxc_probe_requires_fixed_narrow_scope(self):
        from mindtrail_cli_local_probe import validate_mxc_probe, validate_mxc_variant, apply_mxc_probe_config
        allowed = dict(requested=True, order='direct', simulate_command=True, shape='read-only',
                       observe=True, proxy=False, approve=False, responses=1)
        self.assertTrue(validate_mxc_probe(**allowed))
        self.assertTrue(validate_mxc_probe(**{**allowed, 'shape': 'write-execute', 'responses': 2}))
        self.assertTrue(validate_mxc_variant(True, False, 'read-only'))
        self.assertTrue(validate_mxc_variant(False, True, 'write-execute'))
        for values in ((True, False, 'write-execute'), (True, True, 'write-execute'), (False, True, 'read-only')):
            with self.assertRaisesRegex(ValueError, '^invalid_probe_mode$'):
                validate_mxc_variant(*values)
        for field, value in (('order', 'control'), ('simulate_command', False), ('shape', 'write'),
                             ('observe', False), ('proxy', True), ('approve', True), ('responses', 2)):
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, '^invalid_probe_mode$'):
                validate_mxc_probe(**{**allowed, field: value})
        with self.assertRaisesRegex(ValueError, '^invalid_probe_mode$'):
            validate_mxc_probe(**{**allowed, 'shape': 'write-execute', 'responses': 1})
        args = codex_arguments('cli', 'work', 'model', 'producer', 'state', 'queue')
        configured = apply_mxc_probe_config(args)
        self.assertEqual(configured[1:3], ['-a', 'never'])
        self.assertEqual(configured.count('windows.sandbox="mxc"'), 1)
        self.assertIn('workspace-write', configured)
        self.assertNotIn('windows.sandbox="mxc"', args)
        for malformed in (['cli', '-s', 'danger-full-access', '--ignore-user-config', '--ephemeral', '-'],
                          args[:-1] + ['-c', 'windows.sandbox="elevated"', '-']):
            with self.assertRaisesRegex(ValueError, '^invalid_probe_mode$'):
                apply_mxc_probe_config(malformed)

    def test_mxc_error_projection_only_uses_fixed_synthetic_categories(self):
        from mindtrail_cli_local_probe import MXC_STATIC_ERRORS, observe_second_request, project_synthetic_mxc_error
        for category, phrase in MXC_STATIC_ERRORS.items():
            result = project_synthetic_mxc_error('synthetic SECRET_SENTINEL: ' + phrase + ' SECRET_SENTINEL')
            self.assertTrue(result['mxc_error_' + category])
            self.assertNotIn('SECRET_SENTINEL', json.dumps(result))
        unknown = project_synthetic_mxc_error('requires SECRET_SENTINEL')
        self.assertFalse(unknown['mxc_error_any_known'])
        self.assertNotIn('SECRET_SENTINEL', json.dumps(unknown))
        second = {'mxc_probe': False, 'second_has_function_output': False}
        observe_second_request(second, 2, {'input': [{'type': 'function_call_output', 'call_id': 'call_synthetic', 'output': 'native MXC is unavailable on this executor'}]})
        self.assertNotIn('mxc_error_native_unavailable_executor', second)
        second['mxc_probe'] = True
        observe_second_request(second, 2, {'input': [{'type': 'function_call_output', 'call_id': 'call_synthetic', 'output': 'native MXC is unavailable on this executor SECRET_SENTINEL'}]})
        self.assertTrue(second['mxc_error_native_unavailable_executor'])
        self.assertNotIn('SECRET_SENTINEL', json.dumps(second))
        self.assertFalse(project_synthetic_mxc_error('MXC requires nonempty Unicode host SystemDrive_EXTRA')['mxc_error_missing_host_systemdrive'])

    def test_mxc_systemdrive_only_uses_validated_windows_root(self):
        from mindtrail_cli_local_probe import add_synthetic_systemdrive
        env = {'SystemRoot': r'C:\Windows', 'TEMP': 'synthetic'}
        result = add_synthetic_systemdrive(env, r'C:\Windows')
        self.assertEqual(result['SystemDrive'], 'C:')
        self.assertNotIn('LOCALAPPDATA', result)
        self.assertNotIn('SystemDrive', env)
        for candidate, actual in (({'SystemRoot': 'relative'}, r'C:\Windows'),
                                  ({'SystemRoot': r'\\server\share\Windows'}, r'\\server\share\Windows'),
                                  ({'SystemRoot': r'D:\Windows'}, r'C:\Windows'),
                                  ({'SystemRoot': r'C:\Windows', 'localappdata': 'secret'}, r'C:\Windows'),
                                  ({'SystemRoot': r'C:\Windows', 'systemdrive': 'D:'}, r'C:\Windows')):
            with self.assertRaisesRegex(ValueError, '^invalid_probe_mode$'):
                add_synthetic_systemdrive(candidate, actual)

    def test_synthetic_error_item_and_command_sequence(self):
        from mindtrail_cli_local_probe import project_synthetic_item_errors, validate_synthetic_command_sequence
        def event(kind, ident, code=None):
            item = {'type': 'command_execution', 'id': ident}
            if code is not None: item['exit_code'] = code
            return {'type': kind, 'item': item}
        records = [event('item.started', 'one'), event('item.completed', 'one', 0),
                   event('item.started', 'two'), event('item.completed', 'two', 0)]
        self.assertTrue(validate_synthetic_command_sequence(records, 2))
        self.assertFalse(validate_synthetic_command_sequence(records[:-1], 2))
        self.assertFalse(validate_synthetic_command_sequence(records + [event('item.completed', 'two', 0)], 2))
        self.assertFalse(validate_synthetic_command_sequence([event('item.started', 'one'), event('item.completed', 'one', 1)], 1))
        warning = {'type': 'item.completed', 'item': {'type': 'error', 'message': 'model rerouted: SECRET_SENTINEL'}}
        projection = project_synthetic_item_errors([warning] + records)
        self.assertTrue(projection['synthetic_error_item_present'] and projection['synthetic_error_item_before_command'])
        self.assertTrue(projection['synthetic_error_item_phrase_model_rerouted'])
        self.assertFalse(projection['synthetic_error_item_after_command'])
        self.assertNotIn('SECRET_SENTINEL', json.dumps(projection))

    def test_write_execute_requires_paired_fixed_commands_and_final_turn(self):
        from mindtrail_cli_local_probe import validate_synthetic_write_execute, project_synthetic_write_execute
        import copy
        commands = ["Set-Content -LiteralPath synthetic-command.js -Value 'console.log(42)' -Encoding ascii", 'node synthetic-command.js']
        records = []
        for i, cmd in enumerate(commands):
            for kind in ('item.started', 'item.completed'):
                item = {'type': 'command_execution', 'id': str(i), 'command': cmd}
                if kind == 'item.completed':
                    item.update(exit_code=0, aggregated_output='42' if i == 1 else '')
                records.append({'type': kind, 'item': item})
        records.append({'type': 'turn.completed'})
        self.assertTrue(validate_synthetic_write_execute(records))
        projected = project_synthetic_write_execute(records)
        self.assertTrue(projected['synthetic_second_exit_zero'] and projected['synthetic_second_output_42'])
        changed = copy.deepcopy(records); changed[1]['item']['aggregated_output'] = '42'; changed[3]['item']['aggregated_output'] = 'SECRET_SENTINEL'
        self.assertFalse(project_synthetic_write_execute(changed)['synthetic_second_output_42'])
        self.assertNotIn('SECRET_SENTINEL', json.dumps(project_synthetic_write_execute(changed)))
        changed[3]['item']['command'] = 'powershell.exe -Command node synthetic-command.js'
        self.assertTrue(project_synthetic_write_execute(changed)['synthetic_second_powershell_wrapper_exact'])
        self.assertTrue(project_synthetic_write_execute(changed)['synthetic_second_contains_fixed_command'])
        changed[3]['item']['command'] += ' extra'
        self.assertFalse(project_synthetic_write_execute(changed)['synthetic_second_powershell_wrapper_exact'])
        self.assertTrue(project_synthetic_write_execute([{'type': 'item.completed', 'item': {'type': 'error', 'message': 'model rerouted: one -> two (Reason)'}}])['synthetic_error_reroute_template'])
        warning = {'type': 'item.completed', 'item': {'type': 'error', 'message': 'Model metadata for `synthetic-model` not found. Defaulting to fallback metadata; this can degrade performance and cause issues.'}}
        self.assertTrue(project_synthetic_write_execute([warning])['synthetic_error_fixed_model_metadata_warning'])
        warning['item']['message'] += ' SECRET_SENTINEL'
        projection = project_synthetic_write_execute([warning])
        self.assertFalse(projection['synthetic_error_fixed_model_metadata_warning'])
        self.assertNotIn('SECRET_SENTINEL', json.dumps(projection))
        variants = []
        changed = copy.deepcopy(records); changed[1]['item']['aggregated_output'] = '42'; changed[3]['item']['aggregated_output'] = ''; variants.append(changed)
        changed = copy.deepcopy(records); changed[3]['item']['id'] = '0'; variants.append(changed)
        changed = copy.deepcopy(records); changed[2]['item']['command'] = 'other'; variants.append(changed)
        variants.extend((records[:-1], records + [{'type': 'turn.completed'}], records + [None]))
        for candidate in variants:
            self.assertFalse(validate_synthetic_write_execute(candidate))

    def test_synthetic_wrapper_diagnostic_rejects_extra_commands_and_warnings(self):
        import copy, shlex
        from mindtrail_cli_local_probe import validate_synthetic_wrapped_write_execute
        root = r'C:\Windows'
        shell = root + r'\System32\WindowsPowerShell\v1.0\powershell.exe'
        commands = ["Set-Content -LiteralPath synthetic-command.js -Value 'console.log(42)' -Encoding ascii", 'node synthetic-command.js']
        records = []
        for i, command in enumerate(commands):
            for kind in ('item.started', 'item.completed'):
                item = {'type': 'command_execution', 'id': str(i), 'command': shlex.join([shell, '-Command', command])}
                if kind == 'item.completed':
                    item.update(exit_code=0, aggregated_output='42' if i else '')
                records.append({'type': kind, 'item': item})
        records.append({'type': 'turn.completed'})
        self.assertTrue(validate_synthetic_wrapped_write_execute(records, root))
        warning = {'type': 'item.completed', 'item': {'type': 'error', 'message': 'Model metadata for `synthetic-model` not found. Defaulting to fallback metadata; this can degrade performance and cause issues.'}}
        self.assertTrue(validate_synthetic_wrapped_write_execute([warning] + records, root))
        variants = []
        for argv in ([shell, '-Command', commands[1] + '; echo extra'], [shell, '-Command', commands[1], 'extra'], [r'C:\evil\powershell.exe', '-Command', commands[1]]):
            changed = copy.deepcopy(records); changed[3]['item']['command'] = shlex.join(argv); variants.append(changed)
        changed = copy.deepcopy(warning); changed['item']['message'] += ' extra'; variants.append([changed] + records)
        variants.extend(([warning, warning] + records, records[:-1], records + [{'type': 'turn.completed'}], [{'type': 'error'}] + records))
        for candidate in variants:
            self.assertFalse(validate_synthetic_wrapped_write_execute(candidate, root))

    def test_live_mxc_environment_is_per_turn_and_rejects_forged_owner(self):
        from mindtrail_real_agent import make_live_mxc_environment
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent.parent / '.tools') as tmp:
            ident = 'a' * 32
            root = Path(tmp) / ('run-' + ident); root.mkdir()
            (root / 'owner').write_text(ident, encoding='ascii')
            env = {'SystemRoot': r'C:\Windows', 'CODEX_HOME': 'synthetic', 'HTTPS_PROXY': 'synthetic-proxy'}
            first = make_live_mxc_environment(env, root, ident, r'C:\Windows')
            second = make_live_mxc_environment(env, root, ident, r'C:\Windows')
            self.assertNotEqual(first['TEMP'], second['TEMP'])
            self.assertEqual(first['TEMP'], first['TMP'])
            self.assertEqual(first['HTTPS_PROXY'], 'synthetic-proxy')
            self.assertTrue(Path(first['LOCALAPPDATA']).is_relative_to(root))
            for bad, owner, windows in (({**env, 'LOCALAPPDATA': 'sentinel'}, ident, r'C:\Windows'),
                                        (env, 'b' * 32, r'C:\Windows'), (env, ident, r'D:\Windows')):
                with self.assertRaises(ValueError):
                    make_live_mxc_environment(bad, root, owner, windows)

    @unittest.skipUnless(os.name == 'nt', 'Windows owner-lock test')
    def test_mxc_localappdata_is_fresh_run_bound_and_audited(self):
        from mindtrail_cli_local_probe import add_synthetic_localappdata, audit_synthetic_localappdata
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent.parent / '.tools') as tmp:
            root = Path(tmp).resolve()
            home = root / ('codex-local-probe-' + 'a' * 32); home.mkdir()
            (home / 'tmp').mkdir()
            ident = 'b' * 32
            (home / '.mindtrail-probe-owner').write_text(ident, encoding='ascii')
            env = {'SystemRoot': r'C:\Windows', 'SystemDrive': 'C:', 'CODEX_HOME': str(home),
                   'TEMP': str(home / 'tmp'), 'TMP': str(home / 'tmp')}
            for candidate, owner in (({**env, 'LOCALAPPDATA': 'SECRET_SENTINEL'}, ident),
                                     ({**env, 'TEMP': str(root)}, ident),
                                     ({**env, 'CODEX_HOME': str(root)}, ident),
                                     ({**env, 'SystemDrive': 'D:'}, ident),
                                     (env, 'c' * 32)):
                with self.assertRaises((ValueError, OSError)):
                    add_synthetic_localappdata(candidate, home, root, owner)
                self.assertFalse((home / 'localappdata').exists())
            child = add_synthetic_localappdata(env, home, root, ident)
            self.assertEqual(child['LOCALAPPDATA'], str(home / 'localappdata'))
            self.assertNotIn('LOCALAPPDATA', env)
            with self.assertRaises(OSError):
                add_synthetic_localappdata(env, home, root, ident)
            (home / 'localappdata' / 'nested').mkdir()
            (home / 'localappdata' / 'nested' / 'fixture').write_text('SECRET_SENTINEL', encoding='ascii')
            audit = audit_synthetic_localappdata(home, root, ident)
            self.assertEqual((audit['local_audit_files'], audit['local_audit_dirs']), (1, 1))
            self.assertTrue(audit['local_audit_complete'])
            self.assertNotIn('SECRET_SENTINEL', json.dumps(audit))
            with self.assertRaises((ValueError, OSError)):
                audit_synthetic_localappdata(home, root, 'c' * 32)

    @unittest.skipUnless(os.name == 'nt', 'Windows native path test')
    def test_configuration_preserves_native_state_path(self):
        state = Path(r'C:\isolated\state')
        args = codex_arguments('cli.exe', 'work', 'model', 'producer.exe', state, 'queue')
        overrides = [args[i + 1] for i, arg in enumerate(args[:-1]) if arg == '-c']
        raw = next(item.split('=', 1)[1] for item in overrides if '.env.MINDTRAIL_TEST_ROOT=' in item)
        self.assertEqual(json.loads(raw), str(state))
        self.assertNotIn('/', json.loads(raw))

    @unittest.skipUnless(os.name == 'nt', 'Windows junction test')
    def test_junction_in_parent_chain_is_rejected(self):
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent.parent / '.tools') as tmp:
            root = Path(tmp).resolve()
            target = root / 'target'; target.mkdir()
            (target / 'child').mkdir()
            junction = root / 'link'
            env = {key: os.environ[key] for key in ('SYSTEMROOT', 'WINDIR', 'PATH', 'TEMP', 'TMP') if key in os.environ}
            env.update(REAL_AGENT_TEST_LINK=str(junction), REAL_AGENT_TEST_TARGET=str(target))
            result = subprocess.run(['powershell.exe', '-NoProfile', '-Command', 'New-Item -ItemType Junction -Path $env:REAL_AGENT_TEST_LINK -Target $env:REAL_AGENT_TEST_TARGET | Out-Null'], env=env, capture_output=True, timeout=10)
            self.assertEqual(result.returncode, 0)
            try:
                self.assertFalse(safe_existing_path(str(junction), directory=True))
                self.assertFalse(safe_existing_path(str(junction / 'child'), directory=True))
            finally:
                os.rmdir(junction)

    def test_fixture_environment_excludes_authentication_sentinel(self):
        env = fixture_environment({'PATH': 'tools', 'CODEX_HOME': 'SECRET_SENTINEL', 'OPENAI_API_KEY': 'SECRET_SENTINEL'}, 'synthetic')
        self.assertEqual(env, {'PATH': 'tools', 'MINDTRAIL_TEST_ROOT': 'synthetic'})
        self.assertNotIn('SECRET_SENTINEL', repr(env))

    def test_probe_rejects_non_fixture_without_starting_it(self):
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent.parent / '.tools') as tmp:
            root = Path(tmp).resolve()
            binary = root / 'not-a-fixture.exe'
            binary.write_bytes(b'not an executable')
            with self.assertRaisesRegex(ValueError, 'fixture_marker_missing'):
                probe_producer(binary, root / 'state', {})
            self.assertFalse((root / 'state').exists())

    def test_synthetic_source_rejects_extra_code_before_execution(self):
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent.parent / '.tools') as tmp:
            root = Path(tmp).resolve()
            (root / 'index.mjs').write_text('export function double(value) { return value * 2; }')
            cli = root / 'cli.mjs'
            cli.write_text('import { double } from "./index.mjs"; console.log(JSON.stringify(double(Number(process.argv[2]))));')
            self.assertTrue(validate_synthetic_sources(root))
            self.assertTrue(verify_synthetic_execution(root, Path(shutil.which('node.exe'))))
            index = root / 'index.mjs'
            clean_index, clean_cli = index.read_text(), cli.read_text()
            index.write_text('\ufeff' + clean_index, encoding='utf-8')
            cli.write_text('\ufeff' + clean_cli, encoding='utf-8')
            self.assertTrue(validate_synthetic_sources(root))
            self.assertTrue(verify_synthetic_execution(root, Path(shutil.which('node.exe'))))
            for bad in ('\ufeff\ufeff' + clean_cli, '\ufeff' + clean_cli + '\nprocess.exit(0)', '\ufeff' + clean_cli + '\u200b'):
                cli.write_text(bad, encoding='utf-8')
                with self.assertRaises(ValueError): validate_synthetic_sources(root)
            cli.write_text(clean_cli, encoding='utf-8')
            cli.write_text(cli.read_text() + '\nprocess.exit(0)')
            with self.assertRaises(ValueError):
                validate_synthetic_sources(root)

    def test_pagination_requires_contiguous_complete_prefix(self):
        pages = [{'events': [{'seq': 1}], 'last_seq': 2}, {'events': [{'seq': 2}], 'last_seq': 2}, {'events': [], 'last_seq': 2}]
        self.assertEqual(len(validate_pages(pages)), 2)
        for invalid in ([pages[0], {'events': [], 'last_seq': 2}],
                        [pages[0], {'events': [{'seq': 1}], 'last_seq': 2}, pages[-1]],
                        [{'events': [{'seq': 2}], 'last_seq': 2}, pages[-1]],
                        [pages[0]]):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                validate_pages(invalid)

    def test_identity_rejects_default_and_repository_homes(self):
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent.parent / '.tools') as tmp:
            root = Path(tmp).resolve()
            repo = root / 'repo'; repo.mkdir()
            default = root / 'default'; default.mkdir()
            isolated = root / 'isolated'; isolated.mkdir()
            binary = root / 'cli.exe'; binary.write_bytes(b'fixture')
            manifest = repo / '.tools/mcp-stage0-copy/src-tauri/Cargo.toml'; manifest.parent.mkdir(parents=True); manifest.write_text('fixture')
            report = repo / '.tools/real-agent-reports/report.json'; report.parent.mkdir()
            check = lambda home: validate_live_paths(str(binary), str(home), str(manifest), str(report), repo, default)
            self.assertIsNone(check(isolated))
            self.assertEqual(check(default), 'invalid_isolation_path')
            self.assertEqual(check(repo), 'invalid_isolation_path')
            self.assertEqual(validate_live_paths(str(binary), str(isolated), str(repo / 'Cargo.toml'), str(report), repo, default), 'invalid_isolation_path')
            self.assertEqual(validate_live_paths(str(binary), str(isolated), str(manifest), str(isolated / 'report.json'), repo, default), 'invalid_report_path')
            report.write_text('existing')
            self.assertEqual(check(isolated), 'invalid_report_path')

    def test_select_only_cargo_reported_artifact_inside_target(self):
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent.parent / '.tools') as tmp:
            root = Path(tmp).resolve()
            target = root / 'target'
            target.mkdir()
            binary = target / 'fixture.exe'
            binary.write_bytes(b'synthetic')
            outside = root / 'other.exe'
            outside.write_bytes(b'synthetic')
            def artifact(path):
                return json.dumps({'reason': 'compiler-artifact', 'target': {'name': 'mindtrail-producer'}, 'executable': str(path)})
            self.assertEqual(select_producer_artifact([artifact(binary)], target), binary)
            for lines in ([], [artifact(binary), artifact(binary)], [artifact(outside)]):
                with self.subTest(lines=lines), self.assertRaises(ValueError):
                    select_producer_artifact(lines, target)

    def test_tool_items_require_matching_invocation_and_receipt(self):
        item = {'type': 'mcp_tool_call', 'id': 'i1', 'server': 'mindtrail', 'tool': 'trace_task_start', 'arguments': {'session': 'synthetic', 'operation_id': 'start'}}
        start = {'type': 'item.started', 'item': item}
        finish = {'type': 'item.completed', 'item': {**item, 'result': {'structuredContent': {'status': 'committed', 'seq': 1, 'session': 'synthetic', 'operation_id': 'start'}}}}
        done = {'type': 'turn.completed'}
        self.assertEqual(len(validate_calls([start, finish, done], 'synthetic', ['start'], ('trace_task_start',))), 1)
        snake = {'type': 'item.completed', 'item': {**item, 'result': {'structured_content': {'status': 'queued', 'session': 'synthetic', 'operation_id': 'start'}}}}
        self.assertEqual(validate_calls([start, snake, done], 'synthetic', ['start'], ('trace_task_start',))[0][2]['status'], 'queued')
        for records, category in (([start, finish], 'incomplete_turn'),
                                  ([start, finish, done, {'type': 'turn.failed'}], 'turn_failed'),
                                  ([start, finish, {'type': 'error', 'message': 'SECRET_SENTINEL'}, done], 'turn_failed'),
                                  ([start, finish, done, done], 'incomplete_turn'),
                                  ([done, start, finish], 'incomplete_turn')):
            with self.assertRaisesRegex(ValueError, '^' + category + '$'):
                validate_calls(records, 'synthetic', ['start'], ('trace_task_start',))
        incomplete_diagnostic = {}
        with self.assertRaisesRegex(ValueError, '^incomplete_calls$'):
            validate_calls([], 'synthetic', ['start'], ('trace_task_start',), diagnostics=incomplete_diagnostic)
        self.assertFalse(incomplete_diagnostic['call_started_trace_task_start'])
        self.assertFalse(incomplete_diagnostic['call_completed_trace_task_start'])
        partial_diagnostic = {}
        with self.assertRaises(ValueError):
            validate_calls([start], 'synthetic', ['start'], ('trace_task_start',), diagnostics=partial_diagnostic)
        self.assertTrue(partial_diagnostic['call_started_trace_task_start'])
        self.assertFalse(partial_diagnostic['call_completed_trace_task_start'])
        for key in ('isError', 'is_error'):
            valid = {'type': 'item.completed', 'item': {**item, 'result': {**snake['item']['result'], key: False}}}
            self.assertEqual(len(validate_calls([start, valid, done], 'synthetic', ['start'], ('trace_task_start',))), 1)
            for value in (True, None, 0, 'false'):
                bad = {'type': 'item.completed', 'item': {**item, 'result': {**snake['item']['result'], key: value}}}
                with self.assertRaisesRegex(ValueError, '^invalid_result$'):
                    validate_calls([start, bad], 'synthetic', ['start'], ('trace_task_start',))
        conflict = {'type': 'item.completed', 'item': {**item, 'result': {**snake['item']['result'], 'isError': False, 'is_error': False}}}
        with self.assertRaisesRegex(ValueError, '^invalid_result$'):
            validate_calls([start, conflict], 'synthetic', ['start'], ('trace_task_start',))
        for records in ([{'type': 'item.completed', 'item': {'type': 'agent_message', 'text': 'called tool'}}],
                        [finish], [start, start, finish],
                        [start, {'type': 'item.completed', 'item': {**finish['item'], 'server': 'other'}}],
                        [start, {'type': 'item.completed', 'item': {**finish['item'], 'result': {'structuredContent': {'status': 'queued', 'seq': 1}}}}],
                        [start, {'type': 'item.completed', 'item': {**finish['item'], 'result': {'structuredContent': {'status': 'committed'}}}}],
                        [start, {'type': 'item.completed', 'item': {**item, 'result': {'structuredContent': {'status': 'queued'}, 'structured_content': {'status': 'queued'}}}}],
                        [start, {'type': 'item.completed', 'item': {**item, 'result': {'structured_content': {'status': 'queued', 'session': 'other', 'operation_id': 'start'}}}}],
                        [start, {'type': 'item.completed', 'item': {**item, 'result': {'structured_content': {'status': 'queued', 'seq': 1}}}}]):
            with self.subTest(records=records), self.assertRaises(ValueError):
                validate_calls(records, 'synthetic', ['start'], ('trace_task_start',))

        for error in ({'code': -32602, 'message': 'invalid params SECRET_SENTINEL', 'data': 'SECRET_SENTINEL'},
                      {'message': '', 'data': {'error': {'code': 'queue_unavailable'}}},
                      {'message': None, 'code': 'SECRET_SENTINEL'},
                      {'message': 'Failed to deserialize SECRET_SENTINEL'},
                      'transport closed SECRET_SENTINEL', ['SECRET_SENTINEL']):
            diagnostic = {}
            failed = {'type': 'item.completed', 'item': {**item, 'error': error}}
            with self.assertRaisesRegex(ValueError, '^tool_error$'):
                validate_calls([start, failed], 'synthetic', ['start'], ('trace_task_start',), diagnostics=diagnostic)
            public = public_evidence(diagnostic)
            self.assertTrue(public['tool_error_trace_task_start'])
            self.assertNotIn('SECRET_SENTINEL', json.dumps(public))
            if isinstance(error, dict) and error.get('code') == -32602:
                self.assertTrue(public['tool_error_object'] and public['tool_error_code_invalid_params'] and public['tool_error_invalid_arguments'])
                self.assertTrue(public['tool_error_has_code'] and public['tool_error_has_message'] and public['tool_error_has_data'])
            elif isinstance(error, dict) and error.get('message') == '':
                self.assertTrue(public['tool_error_message_empty'] and public['tool_error_mcp_queue_unavailable'])
            elif isinstance(error, dict) and isinstance(error.get('message'), str):
                self.assertTrue(public['tool_error_deserialize_phrase'])
            elif isinstance(error, dict):
                self.assertTrue(public['tool_error_message_nonstring'])
            elif isinstance(error, str):
                self.assertTrue(public['tool_error_string'] and public['tool_error_transport'])
            else:
                self.assertTrue(public['tool_error_other'])

    def test_paths_reject_relative_missing_and_wrong_type(self):
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent.parent / '.tools') as tmp:
            root = Path(tmp).resolve()
            file = root / 'synthetic.txt'
            file.write_text('fixture')
            self.assertTrue(safe_existing_path(str(root), directory=True))
            self.assertTrue(safe_existing_path(str(file), directory=False))
            self.assertFalse(safe_existing_path('relative', directory=True))
            self.assertFalse(safe_existing_path(str(root / 'missing'), directory=True))
            self.assertFalse(safe_existing_path(str(file), directory=True))

    def test_missing_live_arguments_stop_without_echoing_input(self):
        sentinel = 'SECRET_SENTINEL'
        run = subprocess.run([sys.executable, str(SCRIPT), '--live', '--model', sentinel], capture_output=True, text=True, timeout=10)
        self.assertEqual(run.returncode, 2)
        self.assertEqual(json.loads(run.stdout)['code'], 'missing_isolation_arguments')
        self.assertNotIn(sentinel, run.stdout + run.stderr)

    def test_complete_live_arguments_still_fail_closed(self):
        run = subprocess.run([sys.executable, str(SCRIPT), '--live', '--codex-exe', 'x', '--codex-home', 'x', '--model', 'x', '--manifest', 'x', '--report', 'x'], capture_output=True, text=True, timeout=10)
        self.assertEqual(run.returncode, 2)
        self.assertEqual(json.loads(run.stdout)['code'], 'invalid_isolation_path')


if __name__ == '__main__':
    unittest.main()
