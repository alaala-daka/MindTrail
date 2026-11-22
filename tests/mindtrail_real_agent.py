"""Fail-closed entry point for the isolated real-agent acceptance run."""
import argparse
import contextlib
import hashlib
import ipaddress
import json
import os
import os
import queue
import re
import shutil
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from pathlib import Path
from mindtrail_real_agent_job import OwnedJob
from mindtrail_real_agent_lock import CliLock


def safe_existing_path(value, *, directory):
    """Reject relative paths and links in every existing path component."""
    path = Path(value)
    if not path.is_absolute():
        return False
    try:
        for component in (path, *path.parents):
            st = component.lstat()
            if component.is_symlink() or (getattr(st, 'st_file_attributes', 0) & 0x400):
                return False
        resolved = path.resolve(strict=True)
        return resolved == path and (resolved.is_dir() if directory else resolved.is_file())
    except (OSError, RuntimeError, ValueError):
        return False


def validate_live_paths(codex_exe, codex_home, manifest, report, repo, default_home):
    """Validate before starting any process; return only fixed error codes."""
    for value, directory in ((codex_exe, False), (codex_home, True), (manifest, False)):
        if not safe_existing_path(value, directory=directory):
            return 'invalid_isolation_path'
    home = Path(codex_home).resolve(strict=True)
    repository = Path(repo).resolve(strict=True)
    default = Path(default_home).resolve(strict=False)
    if (home == default or home.is_relative_to(default) or default.is_relative_to(home)
            or home == repository or home.is_relative_to(repository) or repository.is_relative_to(home)):
        return 'invalid_isolation_path'
    expected_manifest = repository / '.tools' / 'mcp-stage0-copy' / 'src-tauri' / 'Cargo.toml'
    try:
        manifest_matches = Path(manifest).resolve(strict=True) == expected_manifest.resolve(strict=True)
    except OSError:
        manifest_matches = False
    if not manifest_matches:
        return 'invalid_isolation_path'
    output = Path(report)
    report_root = repository / '.tools' / 'real-agent-reports'
    if (not output.is_absolute() or not safe_existing_path(str(output.parent), directory=True)
            or output.parent != report_root or output.exists() or output.is_symlink()):
        return 'invalid_report_path'
    return None


def verify_isolated_cli(codex, repo):
    """Bind the private copy to the pinned, signature-verified CLI digest."""
    codex, repo = Path(codex), Path(repo).resolve(strict=True)
    if (not safe_existing_path(str(codex), directory=False) or codex.name.lower() != 'codex.exe'
            or codex.parent.parent != repo / '.tools'
            or not re.fullmatch(r'codex-isolated-[0-9a-f]{32}', codex.parent.name)):
        raise ValueError('unsupported_cli')
    with locked_cli_bundle(codex):
        pass
    return True


TOOLS = ('trace_task_start', 'trace_intent', 'trace_api', 'trace_file',
         'trace_call', 'trace_done', 'trace_task_done')


def parse_user_proxy(raw):
    """Return a credential-free HTTP proxy URL; never include input in errors."""
    try:
        if not isinstance(raw, str) or not raw or any(ord(c) < 33 or ord(c) > 126 for c in raw):
            raise ValueError()
        entries = {}
        for part in raw.split(';'):
            if '=' in part:
                key, value = part.split('=', 1)
                key = key.lower()
                if key not in ('http', 'https') or key in entries:
                    raise ValueError()
            else:
                key, value = 'default', part
                if key in entries:
                    raise ValueError()
            parsed = urllib.parse.urlsplit(value if '://' in value else 'http://' + value)
            if (parsed.scheme.lower() != 'http' or not parsed.hostname or not parsed.port
                    or parsed.username is not None or parsed.password is not None
                    or parsed.path or parsed.query or parsed.fragment):
                raise ValueError()
            authority = parsed.netloc
            if authority.startswith('['):
                match = re.fullmatch(r'\[([0-9A-Fa-f:.]+)\]:([0-9]+)', authority)
                if not match:
                    raise ValueError()
                ipaddress.IPv6Address(match.group(1))
            else:
                match = re.fullmatch(r'([A-Za-z0-9.-]+):([0-9]+)', authority)
                if not match:
                    raise ValueError()
                host = match.group(1)
                if all(c in '0123456789.' for c in host):
                    ipaddress.IPv4Address(host)
                elif len(host) > 253 or any(not re.fullmatch(r'[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?', label) for label in host.split('.')):
                    raise ValueError()
            entries[key] = 'http://' + parsed.netloc
        result = entries.get('https') or entries.get('default')
        if not result:
            raise ValueError()
        return result
    except (ValueError, TypeError):
        raise ValueError('invalid_user_proxy') from None


def read_user_proxy():
    import winreg
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, r'Software\Microsoft\Windows\CurrentVersion\Internet Settings') as key:
            enabled = winreg.QueryValueEx(key, 'ProxyEnable')[0]
            raw = winreg.QueryValueEx(key, 'ProxyServer')[0]
        if enabled != 1:
            raise ValueError()
        return parse_user_proxy(raw)
    except (OSError, ValueError):
        raise ValueError('invalid_user_proxy') from None
PINNED_CLI_SHA256 = '094d4db493de22025712cc5963854e7654b7c97f4cee795e3e986270a0f4f6b4'
PINNED_CLI_BUNDLE = {
    'codex.exe': PINNED_CLI_SHA256,
    'codex-command-runner.exe': '2d226628cda9dfc29e3483bd8017d32484714d79127b5fb53ce53aaf05499fae',
    'codex-windows-sandbox-setup.exe': '0ee6252c8ed191b2a3809ba63231e941425427fe1156f58cdda978fefe76c8a9',
    'codex-windows-sandbox-service.exe': 'b8a185d2964a24a666e8320f35518a9c676727f4418717265a559468eb925612',
    'codex-code-mode-host.exe': 'ea5052efa8e4659f5eb2c6199090da75df65bbec4b08955e925eef60a64bbd33',
    'rg.exe': 'aa3a65c65acf29456302b8ebe1ad5b77659d6139e149f149031ec7c99331c417',
}


@contextlib.contextmanager
def locked_cli_bundle(codex):
    with contextlib.ExitStack() as stack:
        for name, digest in PINNED_CLI_BUNDLE.items():
            stack.enter_context(CliLock(Path(codex).parent / name, digest))
        if {item.name.lower() for item in Path(codex).parent.iterdir()} != set(PINNED_CLI_BUNDLE):
            raise ValueError('invalid_cli_bundle')
        yield
SYNTHETIC_VOLUME = Path('D:/')
PUBLIC_CODES = frozenset(('scenario_failed', 'preflight_failed', 'automatic_recovery_unverified',
                          'turn_failed', 'turn_auth_blocked', 'turn_network_blocked', 'turn_quota_blocked', 'turn_rate_limited',
                          'turn_permission_blocked', 'turn_handshake_failed', 'turn_timeout',
                          'turn_cli_arguments', 'turn_cli_configuration', 'turn_cli_model', 'turn_cli_sandbox',
                          'unexpected_instructions', 'invalid_turn_path', 'incomplete_calls', 'incomplete_turn',
                          'invalid_receipt', 'unexpected_tool', 'unpaired_item', 'item_mismatch',
                          'invalid_result', 'tool_error', 'output_limit', 'scenario_timeout',
                          'webview_timeout', 'webview_evaluation', 'desktop_exited', 'task_select_failed'))
DIAGNOSTIC_TERMS = {'diag_argument': ('argument', 'option', 'flag'), 'diag_config': ('config', 'toml'),
                    'diag_sandbox': ('sandbox',), 'diag_permission': ('permission', 'access denied', 'denied'),
                    'diag_network': ('network', 'connection', 'transport', 'request', 'stream'),
                    'diag_auth': ('auth', 'login', 'log in', 'credential'), 'diag_mcp': ('mcp',),
                    'diag_model': ('model',), 'diag_file': ('file', 'path', 'directory', 'os error'),
                    'diag_policy': ('policy', 'approval', 'trust'), 'diag_timeout': ('timeout', 'timed out'),
                    'diag_os_not_found': ('os error 2', 'system cannot find the file', '找不到指定的文件'),
                    'diag_os_access_denied': ('os error 5', 'access is denied', '拒绝访问'),
                    'diag_connection_refused': ('connection refused', 'os error 10061'),
                    'diag_spawn_stage': ('failed to spawn', 'failed spawning', 'spawn failed', 'could not spawn'),
                    'diag_handshake_stage': ('handshake', 'handshaking'),
                    'diag_initialize_stage': ('initialize', 'initialization'),
                    'diag_transport_closed': ('connection closed', 'transport closed', 'broken pipe', 'end of file'),
                    'diag_deadline_stage': ('timed out', 'timeout', 'deadline has elapsed'),
                    'diag_fixture_queue_io': ('queue fixture i/o',),
                    'diag_fixture_acl': ('directory dacl', 'ancestor ace', 'unexpected principal'),
                    'diag_fixture_lock': ('queue_busy', 'sharing violation', 'lock violation'),
                    'diag_fixture_arguments': ('invalid_arguments',),
                    'diag_fixture_queue_unavailable': ('queue_unavailable',),
                    'diag_dynamic_loader': ('0xc0000135', '0xc000007b', 'dll not found', 'entry point not found'),
                    'diag_control_stderr': ('mindtrail_synthetic_stderr_marker',)}
TOOL_ERROR_TERMS = {'tool_error_timeout': ('timed out', 'timeout', 'deadline'),
                    'tool_error_transport': ('transport', 'connection closed', 'broken pipe'),
                    'tool_error_invalid_arguments': ('invalid argument', 'invalid params', 'invalid input'),
                    'tool_error_unavailable': ('unavailable', 'not connected', 'connection refused'),
                    'tool_error_permission': ('permission denied', 'access denied'),
                    'tool_error_internal': ('internal error',),
                    'tool_error_mcp_protocol_phrase': ('mcp error',),
                    'tool_error_call_failed_phrase': ('tool call failed',),
                    'tool_error_server_disconnected_phrase': ('server disconnected',),
                    'tool_error_serialize_phrase': ('failed to serialize',),
                    'tool_error_deserialize_phrase': ('failed to deserialize',),
                    'tool_error_unknown_tool_phrase': ('unknown tool',),
                    'tool_error_producer_failure_phrase': ('the operation could not be completed.',),
                    'tool_error_gateway_rejection_phrase': ('the gateway rejected this operation.',)}
MCP_ERROR_CODES = ('invalid_arguments', 'unknown_tool', 'busy', 'queue_unavailable', 'queue_busy',
                   'queue_corrupt', 'cancelled', 'timeout', 'rate_limited', 'task_closed', 'seq_conflict')


FAILURE_CLASSES = ('rate_limited', 'quota', 'auth', 'permission')
FAILURE_CODES = {'rate_limit_exceeded': 'rate_limited', 'rate_limit_reached': 'rate_limited',
                 'insufficient_quota': 'quota', 'usage_limit_reached': 'quota',
                 'invalid_api_key': 'auth', 'authentication_error': 'auth',
                 'permission_denied': 'permission'}
FAILURE_STATUS = {429: 'rate_limited', 401: 'auth', 403: 'permission'}
FAILURE_HINTS = {'rate_limited': ('rate limit', 'http 429', 'http status 429', 'status 429'),
                 'quota': ('quota exceeded', 'usage limit', 'insufficient quota'),
                 'auth': ('unauthorized', 'authentication failed', 'not authenticated'),
                 'permission': ('permission denied', 'access denied', 'approval required'),
                 'arguments': ('unexpected argument', 'unexpected value', 'invalid argument', 'unrecognized option'),
                 'configuration': ('failed to load config', 'config error', 'invalid configuration', 'config.toml', 'unknown field'),
                 'sandbox': ('sandbox setup failed', 'failed to initialize sandbox', 'sandbox is unavailable', 'windows sandbox'),
                 'handshake': ('mcp startup failed', 'mcp client failed', 'mcp handshake failed'),
                 'network': ('connection refused', 'connection error', 'network error', 'error sending request', 'dns error'),
                 'model': ('model not found', 'model is not supported', 'model is unavailable', 'unsupported model')}


def unique_json_pairs(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError('duplicate_key')
        result[key] = value
    return result


def classify_cli_failure(raw, stderr=b'', *, diagnostics=None):
    """Classify only explicit structured errors; text matches remain clues."""
    found = set()
    for source, data in (('stdout', raw), ('stderr', stderr)):
        structured = set()
        hint_text = []
        for line in data.splitlines():
            try:
                event = json.loads(line, object_pairs_hook=unique_json_pairs)
            except (ValueError, UnicodeError, RecursionError):
                hint_text.append(line.decode('utf-8', errors='replace').lower())
                continue
            if not isinstance(event, dict) or event.get('type') not in ('error', 'turn.failed'):
                continue
            error = event.get('error')
            if event.get('type') == 'error' and not isinstance(error, dict):
                error = event
            if not isinstance(error, dict):
                continue
            for entry in (error, error.get('error')):
                if not isinstance(entry, dict):
                    continue
                code = entry.get('code')
                if isinstance(code, str) and code in FAILURE_CODES:
                    structured.add(FAILURE_CODES[code])
                else:
                    for key in ('status', 'status_code', 'http_status'):
                        status = entry.get(key)
                        if type(status) is int and status in FAILURE_STATUS:
                            structured.add(FAILURE_STATUS[status])
                message = entry.get('message')
                if isinstance(message, str):
                    hint_text.append(message.lower())
        found.update(structured)
        if diagnostics is not None:
            for category in FAILURE_CLASSES:
                diagnostics[f'failure_structured_{source}_{category}'] = category in structured
            for category in FAILURE_HINTS:
                diagnostics[f'failure_hint_{source}_{category}'] = any(term in line for line in hint_text for term in FAILURE_HINTS[category])
    if diagnostics is not None:
        diagnostics['failure_structured_conflict'] = len(found) > 1
    if len(found) != 1:
        return 'turn_failed'
    return {'rate_limited': 'turn_rate_limited', 'quota': 'turn_quota_blocked',
            'auth': 'turn_auth_blocked', 'permission': 'turn_permission_blocked'}[next(iter(found))]


def public_evidence(results):
    """Build evidence from booleans and fixed names, never copy raw output."""
    allowed = ('configuration', 'handshake', 'online_prefix', 'online_done',
               'offline_receipts', 'restart', 'automatic_recovery', 'retry', 'desktop',
               'diag_stdout', 'diag_stderr', 'diag_thread_started', 'diag_turn_started',
               'diag_model_event', 'diag_tool_call', 'diag_turn_completed', 'diag_turn_failed',
               'diag_mcp_started', 'diag_mcp_completed', 'diag_mcp_error',
               'diag_command_started', 'diag_command_completed', 'diag_command_failed',
               'diag_file_change_started', 'diag_file_change_completed', 'diag_file_change_failed',
               'failure_structured_conflict', *(f'failure_structured_{source}_{category}'
                   for source in ('stdout', 'stderr') for category in FAILURE_CLASSES),
               *(f'failure_hint_{source}_{category}' for source in ('stdout', 'stderr') for category in FAILURE_HINTS),
               *(f'call_started_{name}' for name in TOOLS), *(f'call_completed_{name}' for name in TOOLS),
               *DIAGNOSTIC_TERMS.keys(),
               'tool_error_object', 'tool_error_string', 'tool_error_other',
               'tool_error_has_message', 'tool_error_has_code', 'tool_error_has_data',
               'tool_error_message_string', 'tool_error_message_empty', 'tool_error_message_nonstring',
               'tool_error_code_invalid_request', 'tool_error_code_method_not_found',
               'tool_error_code_invalid_params', 'tool_error_code_internal', 'tool_error_code_other',
               *TOOL_ERROR_TERMS.keys(), *(f'tool_error_{name}' for name in TOOLS),
               *(f'tool_error_mcp_{name}' for name in MCP_ERROR_CODES),
               'stage_online_calls_done', 'stage_online_read_started', 'stage_online_read_done',
               'stage_online_projection_done', 'stage_online_ui_started', 'stage_online_ui_done',
               'stage_ui_find_started', 'stage_ui_find_done', 'stage_ui_status_done',
               'stage_ui_click_done', 'stage_ui_node_started', 'stage_ui_node_done',
               'stage_complete_select_started', 'stage_complete_select_done', 'stage_complete_hierarchy_done',
               'stage_complete_node_done', 'stage_complete_facts_done', 'stage_complete_calls_done',
               'recovery_process_started', 'recovery_events_seven_while_alive', 'recovery_process_exited_before_events',
               'recovery_process_alive_after_observation', 'recovery_cancelled', 'recovery_worker_finished',
               'recovery_turn_no_tools', 'recovery_guard_no_calls', 'recovery_job_reaped',
               'stage_code_started', 'stage_code_done', 'exception_type_error', 'exception_key_error',
               'exception_index_error', 'exception_attribute_error', 'exception_timeout',
               'exception_os_error', 'exception_runtime_error', 'exception_assertion_error',
               'exception_http_error', 'exception_url_error', 'exception_other')
    result = {key: results[key] for key in allowed if type(results.get(key)) is bool}
    if results.get('status') in ('passed', 'failed', 'blocked'):
        result['status'] = results['status']
    if results.get('code') in PUBLIC_CODES:
        result['code'] = results['code']
    if isinstance(results.get('cli_version'), str) and re.fullmatch(r'codex-cli [0-9A-Za-z.\-]+', results['cli_version']):
        result['cli_version'] = results['cli_version']
    return result


def write_report(path, report_root, evidence):
    """Create a new report exclusively and serialize only allowed booleans."""
    path, report_root = Path(path), Path(report_root)
    if (not path.is_absolute() or not safe_existing_path(str(report_root), directory=True)
            or path.parent != report_root or path.is_symlink()):
        raise ValueError('invalid_report_path')
    data = (json.dumps(public_evidence(evidence), ensure_ascii=True) + '\n').encode('ascii')
    try:
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, 'O_BINARY', 0), 0o600)
    except OSError:
        raise ValueError('report_create_failed') from None
    with os.fdopen(descriptor, 'wb') as stream:
        stream.write(data)


def parse_jsonl(data, *, max_line=1048576, max_total=8388608):
    if not isinstance(data, bytes) or len(data) > max_total:
        raise ValueError('output_limit')
    if data and not data.endswith(b'\n'):
        raise ValueError('partial_line')
    records = []
    for line in data.splitlines():
        if not line or len(line) > max_line:
            raise ValueError('invalid_line')
        try:
            item = json.loads(line, object_pairs_hook=unique_json_pairs)
        except (ValueError, UnicodeDecodeError):
            raise ValueError('invalid_json') from None
        if not isinstance(item, dict):
            raise ValueError('invalid_item')
        records.append(item)
    return records


def select_producer_artifact(lines, target_dir):
    """Accept exactly one Cargo-reported executable under the selected target."""
    if not safe_existing_path(str(target_dir), directory=True):
        raise ValueError('unsafe_target')
    target = Path(target_dir).resolve(strict=True)
    found = []
    for line in lines:
        try:
            item = json.loads(line)
        except (ValueError, TypeError):
            continue
        if (item.get('reason') == 'compiler-artifact'
                and item.get('target', {}).get('name') == 'mindtrail-producer'
                and item.get('executable')):
            path = Path(item['executable'])
            if not safe_existing_path(str(path), directory=False):
                raise ValueError('unsafe_artifact')
            resolved = path.resolve(strict=True)
            if not resolved.is_relative_to(target):
                raise ValueError('unsafe_artifact')
            found.append(resolved)
    if len(found) != 1:
        raise ValueError('artifact_count')
    return found[0]


def select_desktop_artifact(lines, target_dir):
    if not safe_existing_path(str(target_dir), directory=True):
        raise ValueError('unsafe_target')
    target = Path(target_dir).resolve(strict=True)
    found = []
    for line in lines:
        try:
            item = json.loads(line)
        except (ValueError, TypeError):
            continue
        if (item.get('reason') == 'compiler-artifact' and item.get('target', {}).get('name') == 'mindtrail_lib'
                and item.get('profile', {}).get('test') is True and item.get('executable')):
            path = Path(item['executable'])
            if not safe_existing_path(str(path), directory=False) or not path.resolve(strict=True).is_relative_to(target):
                raise ValueError('unsafe_artifact')
            found.append(path.resolve(strict=True))
    if len(found) != 1:
        raise ValueError('artifact_count')
    return found[0]


def fixture_environment(env, state_root):
    result = {key: env[key] for key in ('SystemRoot', 'WINDIR', 'PATH', 'TEMP', 'TMP') if key in env}
    result['MINDTRAIL_TEST_ROOT'] = str(state_root)
    return result


def bounded_stdout(command, *, cwd, env, timeout, limit):
    job = OwnedJob(command, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                   stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    proc = job.process
    chunks = []
    exceeded = threading.Event()
    def drain():
        size = 0
        while True:
            chunk = proc.stdout.read(8192)
            if not chunk:
                return
            size += len(chunk)
            if size > limit:
                exceeded.set()
                try:
                    proc.kill()
                except OSError:
                    pass
                return
            chunks.append(chunk)
    reader = threading.Thread(target=drain, daemon=True)
    reader.start()
    try:
        try:
            status = proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            proc.kill(); proc.wait(timeout=10)
            raise ValueError('process_timeout') from None
        reader.join(timeout=10)
        if reader.is_alive() or exceeded.is_set() or status != 0:
            raise ValueError('process_failed')
        return b''.join(chunks)
    finally:
        job.close()
        reader.join(timeout=10)


def record_turn_diagnostics(diagnostics, stdout, stderr):
    """Project bounded in-memory output to fixed booleans only."""
    diagnostics['diag_stdout'], diagnostics['diag_stderr'] = bool(stdout), bool(stderr)
    text = (stdout + b'\n' + stderr).decode('utf-8', errors='replace').lower()
    diagnostics.update({name: any(term in text for term in terms) for name, terms in DIAGNOSTIC_TERMS.items()})
    flags = dict.fromkeys(('diag_thread_started', 'diag_turn_started', 'diag_model_event', 'diag_tool_call', 'diag_turn_completed', 'diag_turn_failed', 'diag_mcp_started', 'diag_mcp_completed', 'diag_mcp_error', 'diag_command_started', 'diag_command_completed', 'diag_command_failed', 'diag_file_change_started', 'diag_file_change_completed', 'diag_file_change_failed'), False)
    for line in stdout.splitlines(keepends=True):
        if not line.endswith(b'\n'):
            continue
        try:
            event = json.loads(line)
        except (ValueError, UnicodeError, RecursionError):
            continue
        if not isinstance(event, dict):
            continue
        kind = event.get('type')
        if kind == 'thread.started': flags['diag_thread_started'] = True
        if kind == 'turn.started': flags['diag_turn_started'] = True
        if kind == 'turn.completed': flags['diag_turn_completed'] = True
        if kind == 'turn.failed': flags['diag_turn_failed'] = True
        item = event.get('item')
        if kind in ('item.started', 'item.updated', 'item.completed') and isinstance(item, dict):
            if item.get('type') in ('agent_message', 'reasoning'): flags['diag_model_event'] = True
            if item.get('type') == 'mcp_tool_call':
                flags['diag_tool_call'] = True
                if kind == 'item.started': flags['diag_mcp_started'] = True
                if kind == 'item.completed':
                    flags['diag_mcp_completed'] = True
                    if item.get('error') is not None: flags['diag_mcp_error'] = True
            for item_type, prefix in (('command_execution', 'diag_command_'), ('file_change', 'diag_file_change_')):
                if item.get('type') == item_type:
                    if kind == 'item.started': flags[prefix + 'started'] = True
                    if kind == 'item.completed':
                        flags[prefix + 'completed'] = True
                        if item.get('status') in ('failed', 'declined'): flags[prefix + 'failed'] = True
    diagnostics.update(flags)


def run_owned_turn(command, *, cwd, env, prompt, timeout=120, limit=8388608, started=None, cancel=None, diagnostics=None, started_job=False):
    """Bound both streams and destroy the owned process tree on every exit."""
    if not isinstance(prompt, bytes) or len(prompt) > 1048576:
        raise ValueError('invalid_prompt')
    with OwnedJob(command, cwd=cwd, env=env) as job:
        proc = job.process
        if started is not None:
            started.put((proc, job) if started_job else proc)
        if cancel is not None and cancel.is_set():
            raise ValueError('turn_cancelled')
        results = [bytearray(), bytearray()]
        exceeded = threading.Event()
        output_lock = threading.Lock()
        total = [0]
        def drain(pipe, output):
            while True:
                chunk = pipe.read(8192)
                if not chunk:
                    return
                with output_lock:
                    total[0] += len(chunk)
                    if total[0] > limit:
                        exceeded.set()
                        return
                    output.extend(chunk)
        threads = [threading.Thread(target=drain, args=(proc.stdout, results[0]), daemon=True),
                   threading.Thread(target=drain, args=(proc.stderr, results[1]), daemon=True)]
        write_failed = threading.Event()
        def write_prompt():
            try:
                proc.stdin.write(prompt)
                proc.stdin.close()
            except (BrokenPipeError, OSError, ValueError):
                write_failed.set()
        threads.append(threading.Thread(target=write_prompt, daemon=True))
        for thread in threads:
            thread.start()
        timed_out = False
        try:
            import time
            end = time.monotonic() + timeout
            while proc.poll() is None:
                if exceeded.is_set():
                    raise ValueError('output_limit')
                if time.monotonic() >= end:
                    timed_out = True
                    raise ValueError('turn_timeout')
                time.sleep(.05)
            for thread in threads:
                thread.join(timeout=2)
            if any(thread.is_alive() for thread in threads) or exceeded.is_set() or write_failed.is_set():
                raise ValueError('output_limit')
            if proc.returncode != 0:
                if diagnostics is not None:
                    record_turn_diagnostics(diagnostics, bytes(results[0]), bytes(results[1]))
                raise ValueError(classify_cli_failure(bytes(results[0]), bytes(results[1]), diagnostics=diagnostics))
            if diagnostics is not None:
                record_turn_diagnostics(diagnostics, bytes(results[0]), bytes(results[1]))
            records = parse_jsonl(bytes(results[0]))
            if any(isinstance(record, dict) and record.get('type') in ('turn.failed', 'error') for record in records):
                raise ValueError(classify_cli_failure(bytes(results[0]), bytes(results[1]), diagnostics=diagnostics))
            return records
        finally:
            # Closing the job first releases any pipe held by a descendant.
            job.close()
            for thread in threads:
                thread.join(timeout=5)
            if timed_out and diagnostics is not None:
                with output_lock:
                    record_turn_diagnostics(diagnostics, bytes(results[0]), bytes(results[1]))


_BUILD_AUTH = object()


class BuildArtifact:
    def __init__(self, executable, digest, key):
        if key is not _BUILD_AUTH:
            raise ValueError('invalid_artifact')
        self.executable = Path(executable)
        self.digest = digest


def model_turn(codex, codex_home, model, artifact, run, repo,
               prompt, session, operation_ids, expected, started=None, cancel=None, timeout=120, diagnostics=None, proxy=None, recovery_guard=False):
    """Run one previously authorized, preflighted synthetic CLI turn."""
    if not isinstance(artifact, BuildArtifact):
        raise ValueError('invalid_artifact')
    producer = artifact.executable
    state, work, fixture = Path(run['state']), Path(run['work']), Path(run['fixture'])
    repo = Path(repo).resolve(strict=True)
    for path, directory in ((codex, False), (codex_home, True), (producer, False), (state, True), (work, True)):
        if not safe_existing_path(str(path), directory=directory):
            raise ValueError('invalid_turn_path')
    ident = run['id']
    if not re.fullmatch(r'[0-9a-f]{32}', ident):
        raise ValueError('invalid_turn_identity')
    expected_run = repo / '.tools' / 'real-agent-runs' / ('run-' + ident)
    expected_fixture = repo / '.tools' / 'mcp-stage0-copy' / 'src-tauri' / ('MindTrailAppEvent-' + ident)
    work_root = SYNTHETIC_VOLUME / ('MindTrailSynthetic-' + ident)
    if not producer.resolve(strict=True).is_relative_to((repo / '.tools' / 'real-agent-fixture-target').resolve(strict=True)):
        raise ValueError('invalid_artifact')
    if (work != work_root / 'work' or fixture != expected_fixture or state != fixture / 'state'
            or run['queue'] != 'run-' + ident):
        raise ValueError('invalid_turn_path')
    home = Path(codex_home).resolve(strict=True)
    default = Path(os.environ.get('USERPROFILE', str(Path.home()))) / '.codex'
    if home == repo or home.is_relative_to(repo) or repo.is_relative_to(home) or home == default or home.is_relative_to(default) or default.is_relative_to(home):
        raise ValueError('invalid_turn_path')
    for name in ('AGENTS.md', 'AGENTS.override.md'):
        path = home / name
        if path.exists() or path.is_symlink():
            raise ValueError('unexpected_instructions')
    for marker in (expected_run / 'owner', fixture / 'owner', fixture / 'fixture-marker', work_root / 'owner', work / '.mindtrail-work-owner'):
        if not safe_existing_path(str(marker), directory=False) or marker.read_text(encoding='ascii') != ident:
            raise ValueError('invalid_turn_path')
    for directory in (work, work_root, SYNTHETIC_VOLUME):
        if any((directory / name).exists() or (directory / name).is_symlink() for name in ('AGENTS.md', 'AGENTS.override.md', '.codex', '.git')):
            raise ValueError('unexpected_instructions')
    if any(item.name not in {'.mindtrail-work-owner', 'index.mjs', 'cli.mjs'} or not safe_existing_path(str(item), directory=False) for item in work.iterdir()):
        raise ValueError('unexpected_work_file')
    producer_bytes = Path(producer).read_bytes()
    if hashlib.sha256(producer_bytes).hexdigest() != artifact.digest or b'MINDTRAIL_TEST_ROOT' not in producer_bytes:
        raise ValueError('fixture_marker_missing')
    if not re.fullmatch(r'[A-Za-z0-9_-]{1,128}', session):
        raise ValueError('invalid_turn_identity')
    env = {key: os.environ[key] for key in ('SystemRoot', 'WINDIR', 'PATH', 'TEMP', 'TMP') if key in os.environ}
    env['CODEX_HOME'] = str(codex_home)
    if proxy is not None:
        env['HTTPS_PROXY'] = proxy
        env['NO_PROXY'] = 'localhost,127.0.0.1,::1'
    command = codex_arguments(codex, work, model, producer, state, run['queue'], isolate_proxy=proxy is not None, approve_tools=True)
    guard_lock = contextlib.ExitStack()
    if recovery_guard:
        if expected or started is None:
            raise ValueError('invalid_turn_path')
        python = Path(sys.executable).resolve(strict=True)
        observer = repo / 'tests' / 'mindtrail_mcp_observer.py'
        guard = repo / 'tests' / 'mindtrail_recovery_guard.py'
        try:
            for path in (python, observer, guard, producer):
                if not safe_existing_path(str(path), directory=False):
                    raise ValueError('invalid_turn_path')
                digest = artifact.digest if path == producer else hashlib.sha256(path.read_bytes()).hexdigest()
                guard_lock.enter_context(CliLock(path, digest))
        except BaseException:
            guard_lock.close()
            raise
        for i, value in enumerate(command):
            if value.startswith('mcp_servers.mindtrail.command='):
                command[i] = 'mcp_servers.mindtrail.command=' + json.dumps(python.as_posix())
            if value.startswith('mcp_servers.mindtrail.args='):
                command[i] = 'mcp_servers.mindtrail.args=' + json.dumps([observer.as_posix()])
        for key, value in (('MINDTRAIL_PROBE_EXE', producer.as_posix()), ('MINDTRAIL_PROBE_SHA256', artifact.digest), ('MINDTRAIL_RECOVERY_GUARD', '1')):
            command[-1:-1] = ['-c', 'mcp_servers.mindtrail.env.' + key + '=' + json.dumps(value)]
    with guard_lock, locked_cli_bundle(codex), CliLock(work_root / 'owner', hashlib.sha256(ident.encode('ascii')).hexdigest()), CliLock(work / '.mindtrail-work-owner', hashlib.sha256(ident.encode('ascii')).hexdigest()):
        if run.get('mxc') is True:
            from mindtrail_cli_local_probe import add_synthetic_systemdrive, apply_mxc_probe_config, get_windows_directory, require_non_elevated
            require_non_elevated()
            with CliLock(expected_run / 'owner', hashlib.sha256(ident.encode('ascii')).hexdigest()):
                env = make_live_mxc_environment(env, expected_run, ident, get_windows_directory())
            command = apply_mxc_probe_config(command)
        for directory in (work, work_root, SYNTHETIC_VOLUME):
            if any((directory / name).exists() or (directory / name).is_symlink() for name in ('AGENTS.md', 'AGENTS.override.md', '.codex', '.git')):
                raise ValueError('unexpected_instructions')
        records = run_owned_turn(command, cwd=work, env=env, prompt=prompt.encode('utf-8'), started=started, cancel=cancel, timeout=timeout, diagnostics=diagnostics, started_job=recovery_guard)
    return validate_calls(records, session, operation_ids, expected, diagnostics=diagnostics)


def make_live_mxc_environment(env, run_root, ident, windows_directory):
    """Create fresh per-turn process-only directories under this run's owner."""
    from mindtrail_cli_local_probe import add_synthetic_systemdrive
    run_root = Path(run_root)
    if (not re.fullmatch('[0-9a-f]{32}', ident) or run_root.name != 'run-' + ident
            or not safe_existing_path(str(run_root), directory=True)
            or not safe_existing_path(str(run_root / 'owner'), directory=False)
            or (run_root / 'owner').read_text(encoding='ascii') != ident):
        raise ValueError('invalid_turn_path')
    child = run_root / ('mxc-turn-' + uuid.uuid4().hex)
    child.mkdir()
    temp, local = child / 'tmp', child / 'localappdata'
    temp.mkdir(); local.mkdir()
    if not all(safe_existing_path(str(path), directory=True) for path in (child, temp, local)):
        raise ValueError('invalid_turn_path')
    result = add_synthetic_systemdrive(env, windows_directory)
    result.update(TEMP=str(temp), TMP=str(temp), LOCALAPPDATA=str(local))
    return result


def recovery_guard_ready(state, job):
    """Check fixed observer markers and a live producer in the CLI-owned job."""
    state = Path(state)
    ready = state / 'observer-recovery-ready'
    pidfile = state / 'observer-recovery-pid'
    if any((state / name).exists() or (state / name).is_symlink() for name in ('observer-recovery-violation', 'observer-recovery-producer-exited')):
        return False
    if not safe_existing_path(str(ready), directory=False) or not safe_existing_path(str(pidfile), directory=False):
        return False
    if pidfile.stat().st_size > 10:
        return False
    text = pidfile.read_text(encoding='ascii')
    return bool(re.fullmatch('[1-9][0-9]{0,9}', text) and job.contains_active_pid(int(text)))


def register_run(run_parent, manifest_parent, work_volume=None):
    """Create disjoint, uniquely named test paths without adopting existing state."""
    run_parent, manifest_parent = Path(run_parent), Path(manifest_parent)
    if not safe_existing_path(str(run_parent), directory=True) or not safe_existing_path(str(manifest_parent), directory=True):
        raise ValueError('unsafe_run_parent')
    if work_volume is not None and not safe_existing_path(str(work_volume), directory=True):
        raise ValueError('unsafe_work_volume')
    ident = uuid.uuid4().hex
    run = run_parent / ('run-' + ident)
    fixture = manifest_parent / ('MindTrailAppEvent-' + ident)
    run.mkdir()
    try:
        fixture.mkdir()
        work_root = run if work_volume is None else Path(work_volume) / ('MindTrailSynthetic-' + ident)
        if work_volume is not None:
            work_root.mkdir()
            with (work_root / 'owner').open('x', encoding='ascii') as marker:
                marker.write(ident)
        (work_root / 'work').mkdir()
        if work_volume is not None:
            with (work_root / 'work' / '.mindtrail-work-owner').open('x', encoding='ascii') as marker:
                marker.write(ident)
        (run / 'report').mkdir()
        (fixture / 'webview').mkdir()
        (run / 'owner').write_text(ident, encoding='ascii')
        (fixture / 'owner').write_text(ident, encoding='ascii')
        (fixture / 'fixture-marker').write_text(ident, encoding='ascii')
    except BaseException:
        # Retain partially created paths for explicit inspection; do not adopt
        # or recursively remove any pre-existing directory.
        raise
    return {'id': ident, 'work': work_root / 'work', 'report': run / 'report',
            'fixture': fixture, 'state': fixture / 'state', 'webview': fixture / 'webview',
            'queue': 'run-' + ident}


def prepare_run(repo, artifact):
    """Create one run and initialize its separate probe queue in the same state."""
    repo = Path(repo).resolve(strict=True)
    if not isinstance(artifact, BuildArtifact) or not safe_existing_path(str(artifact.executable), directory=False):
        raise ValueError('invalid_artifact')
    if (not artifact.executable.resolve(strict=True).is_relative_to((repo / '.tools/real-agent-fixture-target').resolve(strict=True))
            or hashlib.sha256(artifact.executable.read_bytes()).hexdigest() != artifact.digest):
        raise ValueError('invalid_artifact')
    parent = repo / '.tools' / 'real-agent-runs'
    if not parent.exists():
        parent.mkdir()
        with (parent / 'owner').open('x', encoding='ascii') as marker:
            marker.write('mindtrail-real-agent-runs-v1')
    if (not safe_existing_path(str(parent), directory=True)
            or not safe_existing_path(str(parent / 'owner'), directory=False)
            or (parent / 'owner').read_text(encoding='ascii') != 'mindtrail-real-agent-runs-v1'):
        raise ValueError('unsafe_run_parent')
    fixture_parent = repo / '.tools' / 'mcp-stage0-copy' / 'src-tauri'
    if not safe_existing_path(str(SYNTHETIC_VOLUME), directory=True):
        raise ValueError('unsafe_work_volume')
    for directory in (SYNTHETIC_VOLUME,):
        if any((directory / name).exists() or (directory / name).is_symlink() for name in ('AGENTS.md', 'AGENTS.override.md', '.codex', '.git')):
            raise ValueError('unexpected_instructions')
    run = register_run(parent, fixture_parent, SYNTHETIC_VOLUME)
    env = {key: os.environ[key] for key in ('SystemRoot', 'WINDIR', 'PATH', 'TEMP', 'TMP') if key in os.environ}
    probe_producer(artifact.executable, run['state'], env)
    return run


def static_config_probe(codex, producer, root, env, *, isolate_proxy=False):
    """Observe only the names of configured servers in an empty CLI home."""
    home = root / ('codex-probe-' + uuid.uuid4().hex)
    home.mkdir()
    child_env = {key: env[key] for key in ('SystemRoot', 'WINDIR', 'PATH', 'TEMP', 'TMP') if key in env}
    child_env['CODEX_HOME'] = str(home)
    command = [str(codex)]
    for key, value in (('command', producer.as_posix()), ('args', ['mcp', '--queue', 'probe']),
                       ('env.MINDTRAIL_TEST_ROOT', str(home / 'unused-state')),
                       ('enabled_tools', list(TOOLS)), ('required', True)):
        command.extend(['-c', 'mcp_servers.mindtrail.' + key + '=' + json.dumps(value)])
    proxy_env = {'HTTP_PROXY': '', 'HTTPS_PROXY': '', 'ALL_PROXY': '', 'NO_PROXY': '*'} if isolate_proxy else {}
    for key, value in proxy_env.items():
        command.extend(['-c', 'mcp_servers.mindtrail.env.' + key + '=' + json.dumps(value)])
    command.extend(['mcp', 'list', '--json'])
    data = json.loads(bounded_stdout(command, cwd=home, env=child_env, timeout=30, limit=1048576))
    if not isinstance(data, list) or len(data) != 1 or not isinstance(data[0], dict) or data[0].get('name') != 'mindtrail':
        raise ValueError('unexpected_servers')
    server = data[0]
    transport = server.get('transport')
    if (server.get('enabled') is not True or not isinstance(transport, dict)
            or transport.get('type') != 'stdio' or transport.get('command') != producer.as_posix()
            or transport.get('args') != ['mcp', '--queue', 'probe']
            or transport.get('env') != {'MINDTRAIL_TEST_ROOT': str(home / 'unused-state'), **proxy_env}):
        raise ValueError('unexpected_config')
    return True


def static_cli_probe(codex, root, env):
    home = root / ('codex-help-' + uuid.uuid4().hex)
    home.mkdir()
    child_env = {key: env[key] for key in ('SystemRoot', 'WINDIR', 'PATH', 'TEMP', 'TMP') if key in env}
    child_env['CODEX_HOME'] = str(home)
    version = bounded_stdout([str(codex), '--version'], cwd=home, env=child_env, timeout=15, limit=4096).decode('utf-8').strip()
    if not re.fullmatch(r'codex-cli [0-9A-Za-z.\-]+', version):
        raise ValueError('unsupported_cli')
    top = bounded_stdout([str(codex), '--help'], cwd=home, env=child_env, timeout=15, limit=1048576)
    execute = bounded_stdout([str(codex), 'exec', '--help'], cwd=home, env=child_env, timeout=15, limit=1048576)
    if not all(flag in top for flag in (b'--strict-config', b'--config', b'--sandbox', b'--cd')) or not all(flag in execute for flag in (b'--json', b'--ephemeral', b'--ignore-user-config', b'--skip-git-repo-check')):
        raise ValueError('unsupported_cli')
    return version


def build_fixture(repo):
    """Build and bind the current Cargo-reported fixture to its digest."""
    repo = Path(repo).resolve(strict=True)
    tools = repo / '.tools'
    source = repo / 'src-tauri'
    copy = tools / 'mcp-stage0-copy' / 'src-tauri'
    target = tools / 'real-agent-fixture-target'
    cargo = tools / 'cargo' / 'bin' / 'cargo.exe'
    for path in (tools, source, copy, target, cargo.parent.parent, tools / 'rustup'):
        if not safe_existing_path(str(path), directory=True):
            raise ValueError('local_prerequisite')
    if not safe_existing_path(str(cargo), directory=False):
        raise ValueError('local_prerequisite')
    files = [Path('Cargo.toml'), Path('Cargo.lock'), Path('build.rs'), Path('tauri.conf.json')]
    original_rs = {p.relative_to(source) for p in (source / 'src').rglob('*.rs')}
    copied_rs = {p.relative_to(copy) for p in (copy / 'src').rglob('*.rs')}
    if original_rs != copied_rs:
        raise ValueError('source_mismatch')
    resources = set()
    for dirname in ('icons', 'capabilities'):
        for base in (source, copy):
            if not safe_existing_path(str(base / dirname), directory=True):
                raise ValueError('source_mismatch')
        original = {p.relative_to(source) for p in (source / dirname).rglob('*') if p.is_file()}
        copied = {p.relative_to(copy) for p in (copy / dirname).rglob('*') if p.is_file()}
        if original != copied:
            raise ValueError('source_mismatch')
        resources.update(original)
    for rel in files + sorted(original_rs | resources):
        a, b = source / rel, copy / rel
        if not safe_existing_path(str(a), directory=False) or not safe_existing_path(str(b), directory=False) or a.read_bytes() != b.read_bytes():
            raise ValueError('source_mismatch')
    env = {key: os.environ[key] for key in ('SystemRoot', 'SystemDrive', 'WINDIR', 'PATH', 'TEMP', 'TMP') if key in os.environ}
    env['CARGO_HOME'] = str(tools / 'cargo')
    env['RUSTUP_HOME'] = str(tools / 'rustup')
    command = [str(cargo), 'build', '--offline', '--locked', '--manifest-path', str(copy / 'Cargo.toml'),
               '--target-dir', str(target), '--features', 'producer-test-fixture', '--bin', 'mindtrail-producer', '--message-format=json']
    output = bounded_stdout(command, cwd=repo, env=env, timeout=600, limit=32 * 1024 * 1024)
    exe = select_producer_artifact(output.decode('utf-8').splitlines(), target)
    return BuildArtifact(exe, hashlib.sha256(exe.read_bytes()).hexdigest(), _BUILD_AUTH)


def local_probe(repo):
    """Build from the checked isolated copy and probe it, without credentials."""
    repo = Path(repo).resolve(strict=True)
    tools = repo / '.tools'
    copy = tools / 'mcp-stage0-copy' / 'src-tauri'
    artifact = build_fixture(repo)
    exe = artifact.executable
    env = {key: os.environ[key] for key in ('SystemRoot', 'WINDIR', 'PATH', 'TEMP', 'TMP') if key in os.environ}
    fixture = copy / ('MindTrailRealAgentProbe-' + uuid.uuid4().hex)
    fixture.mkdir()
    fixture_ok = probe_producer(exe, fixture / 'state', env)
    codex = shutil.which('codex.exe')
    if not codex:
        raise ValueError('codex_missing')
    static_cli_probe(Path(codex), tools, env)
    config_ok = static_config_probe(Path(codex), exe, tools, env)
    synthetic = run_owned_turn([sys.executable, '-c', 'import sys;sys.stdin.buffer.read();print("{}")'],
                               cwd=fixture, env=env, prompt=b'isolated', timeout=10)
    if synthetic != [{}]:
        raise ValueError('turn_probe_failed')
    return fixture_ok and config_ok


def codex_arguments(executable, work_dir, model, producer, state_root, queue_name, *, isolate_proxy=False, approve_tools=False):
    """Construct per-process overrides; no command interpreter is involved."""
    args = [str(executable), '-C', str(work_dir), '-s', 'workspace-write', '-m', model,
            '--strict-config', 'exec', '--json', '--ephemeral', '--ignore-user-config',
            '--skip-git-repo-check']
    for key, value in (
        ('command', Path(producer).as_posix()),
        ('args', ['mcp', '--queue', queue_name]),
        ('env.MINDTRAIL_TEST_ROOT', str(Path(state_root))),
        ('enabled_tools', list(TOOLS)),
        ('required', True),
    ):
        args.extend(['-c', 'mcp_servers.mindtrail.' + key + '=' + json.dumps(value, ensure_ascii=False)])
    if isolate_proxy:
        for key, value in (('HTTP_PROXY', ''), ('HTTPS_PROXY', ''), ('ALL_PROXY', ''), ('NO_PROXY', '*')):
            args.extend(['-c', 'mcp_servers.mindtrail.env.' + key + '=' + json.dumps(value)])
    if approve_tools:
        for name in TOOLS:
            args.extend(['-c', 'mcp_servers.mindtrail.tools.' + name + '.approval_mode="approve"'])
    args.append('-')
    return args


def probe_producer(executable, state_root, env):
    """Probe only a caller-verified fixture binary in a new isolated state."""
    executable, state_root = Path(executable), Path(state_root)
    if not safe_existing_path(str(executable), directory=False) or not safe_existing_path(str(state_root.parent), directory=True) or not state_root.is_absolute():
        raise ValueError('unsafe_probe_path')
    if state_root.exists() or state_root.is_symlink():
        raise ValueError('probe_state_exists')
    # Defense in depth only: callers must also prove this is the current
    # producer-test-fixture artifact from the Cargo invocation they own.
    if b'MINDTRAIL_TEST_ROOT' not in executable.read_bytes():
        raise ValueError('fixture_marker_missing')
    child_env = fixture_environment(env, state_root)
    output = queue.Queue(maxsize=3)
    proc = subprocess.Popen([str(executable), 'mcp', '--queue', 'probe'], stdin=subprocess.PIPE,
                            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=child_env,
                            creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
    def read_output():
        try:
            while True:
                line = proc.stdout.readline(1048577)
                if not line:
                    break
                output.put_nowait(line)
        except queue.Full:
            pass
        finally:
            try:
                output.put_nowait(None)
            except queue.Full:
                pass
    reader = threading.Thread(target=read_output, daemon=True)
    reader.start()
    def send(value):
        proc.stdin.write(json.dumps(value, separators=(',', ':')).encode() + b'\n')
        proc.stdin.flush()
    def response(ident):
        line = output.get(timeout=15)
        if line is None:
            raise ValueError('probe_exited')
        if len(line) > 1048576 or not line.endswith(b'\n'):
            raise ValueError('probe_frame_size')
        obj = json.loads(line)
        if obj.get('id') != ident or 'error' in obj or not isinstance(obj.get('result'), dict):
            raise ValueError('probe_response')
        return obj['result']
    try:
        send({'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {'protocolVersion': '2025-11-25', 'capabilities': {}, 'clientInfo': {'name': 'isolated-probe', 'version': '1'}}})
        response(1)
        send({'jsonrpc': '2.0', 'method': 'notifications/initialized'})
        send({'jsonrpc': '2.0', 'id': 2, 'method': 'tools/list'})
        tools = response(2).get('tools')
        if not isinstance(tools, list) or {item.get('name') for item in tools} != set(TOOLS) or len(tools) != len(TOOLS):
            raise ValueError('probe_tools')
        proc.stdin.close()
        if proc.wait(timeout=15) != 0:
            raise ValueError('probe_exit')
        db = state_root / 'producers' / 'mcp' / 'probe' / 'queue.sqlite3'
        if not safe_existing_path(str(db), directory=False):
            raise ValueError('probe_queue_missing')
        return True
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait(timeout=10)
        if proc.stdin and not proc.stdin.closed:
            proc.stdin.close()
        reader.join(timeout=10)
        proc.stdout.close()


def record_tool_error(diagnostics, tool, error):
    """Project a tool error onto a fixed set of booleans without retaining it."""
    diagnostics['tool_error_object'] = isinstance(error, dict)
    diagnostics['tool_error_string'] = isinstance(error, str)
    diagnostics['tool_error_other'] = not isinstance(error, (dict, str))
    diagnostics['tool_error_has_message'] = isinstance(error, dict) and 'message' in error
    diagnostics['tool_error_has_code'] = isinstance(error, dict) and 'code' in error
    diagnostics['tool_error_has_data'] = isinstance(error, dict) and 'data' in error
    for name in TOOLS:
        diagnostics['tool_error_' + name] = tool == name
    code = error.get('code') if isinstance(error, dict) else None
    code_names = {-32600: 'tool_error_code_invalid_request', -32601: 'tool_error_code_method_not_found',
                  -32602: 'tool_error_code_invalid_params', -32603: 'tool_error_code_internal'}
    for value, name in code_names.items():
        diagnostics[name] = type(code) is int and code == value
    diagnostics['tool_error_code_other'] = type(code) is int and code not in code_names
    message = error.get('message') if isinstance(error, dict) else error
    diagnostics['tool_error_message_string'] = isinstance(message, str)
    diagnostics['tool_error_message_empty'] = isinstance(message, str) and not message
    diagnostics['tool_error_message_nonstring'] = diagnostics['tool_error_has_message'] and not isinstance(message, str)
    text = message.lower() if isinstance(message, str) else ''
    diagnostics.update({name: any(term in text for term in terms) for name, terms in TOOL_ERROR_TERMS.items()})
    data = error.get('data') if isinstance(error, dict) else None
    nested = data.get('error') if isinstance(data, dict) else None
    candidates = (code, message, data.get('code') if isinstance(data, dict) else None,
                  nested.get('code') if isinstance(nested, dict) else None)
    for known in MCP_ERROR_CODES:
        diagnostics['tool_error_mcp_' + known] = any(isinstance(v, str) and v == known for v in candidates)


def record_exception_type(evidence, error):
    names = {'TypeError': 'exception_type_error', 'KeyError': 'exception_key_error',
             'IndexError': 'exception_index_error', 'AttributeError': 'exception_attribute_error',
             'TimeoutError': 'exception_timeout', 'OSError': 'exception_os_error',
             'RuntimeError': 'exception_runtime_error', 'AssertionError': 'exception_assertion_error',
             'HTTPError': 'exception_http_error', 'URLError': 'exception_url_error'}
    evidence[names.get(type(error).__name__, 'exception_other')] = True


def validate_calls(records, session, operation_ids, expected=TOOLS, *, diagnostics=None):
    """Validate paired Codex MCP items; final text is never evidence."""
    if diagnostics is not None:
        for name in TOOLS:
            for event_type, prefix in (('item.started', 'call_started_'), ('item.completed', 'call_completed_')):
                diagnostics[prefix + name] = any(isinstance(record, dict) and record.get('type') == event_type
                    and isinstance(record.get('item'), dict) and record['item'].get('type') == 'mcp_tool_call'
                    and record['item'].get('server') == 'mindtrail' and record['item'].get('tool') == name
                    for record in records)
    if any(isinstance(record, dict) and record.get('type') in ('turn.failed', 'error') for record in records):
        raise ValueError('turn_failed')
    pending = {}
    seen = set()
    calls = []
    for record in records:
        if record.get('type') not in ('item.started', 'item.completed'):
            continue
        item = record.get('item')
        if not isinstance(item, dict) or item.get('type') != 'mcp_tool_call':
            continue
        ident = item.get('id')
        if not isinstance(ident, str) or not ident:
            raise ValueError('invalid_item')
        if record['type'] == 'item.started':
            if ident in seen or ident in pending:
                raise ValueError('duplicate_item')
            if pending:
                raise ValueError('overlapping_calls')
            if item.get('server') != 'mindtrail' or item.get('tool') not in TOOLS:
                raise ValueError('unexpected_tool')
            args = item.get('arguments')
            if not isinstance(args, dict):
                raise ValueError('invalid_arguments')
            pending[ident] = item
        else:
            if ident not in pending:
                raise ValueError('unpaired_item')
            started = pending.pop(ident)
            seen.add(ident)
            if item.get('server') != started['server'] or item.get('tool') != started['tool'] or item.get('arguments') != started['arguments']:
                raise ValueError('item_mismatch')
            if item.get('error') is not None:
                if diagnostics is not None:
                    record_tool_error(diagnostics, started['tool'], item['error'])
                raise ValueError('tool_error')
            result = item.get('result')
            if not isinstance(result, dict):
                raise ValueError('invalid_result')
            error_keys = [key for key in ('isError', 'is_error') if key in result]
            if len(error_keys) > 1 or (error_keys and result[error_keys[0]] is not False):
                raise ValueError('invalid_result')
            keys = [key for key in ('structuredContent', 'structured_content') if key in result]
            if len(keys) != 1:
                raise ValueError('invalid_result')
            result = result[keys[0]]
            if not isinstance(result, dict):
                raise ValueError('invalid_result')
            status = result.get('status')
            seq = result.get('seq')
            if status == 'committed':
                if type(seq) is not int or seq < 1:
                    raise ValueError('invalid_receipt')
            elif status == 'queued':
                if 'seq' in result:
                    raise ValueError('invalid_receipt')
            else:
                raise ValueError('invalid_receipt')
            index = len(calls)
            args = started['arguments']
            if index >= len(expected) or started['tool'] != expected[index] or args.get('session') != session or args.get('operation_id') != operation_ids[index]:
                raise ValueError('unexpected_call')
            if result.get('session') != session or result.get('operation_id') != operation_ids[index]:
                raise ValueError('invalid_receipt')
            calls.append((started['tool'], args, result))
    if pending or len(calls) != len(expected):
        raise ValueError('incomplete_calls')
    completions = [i for i, record in enumerate(records) if isinstance(record, dict) and record.get('type') == 'turn.completed']
    item_positions = [i for i, record in enumerate(records) if isinstance(record, dict) and record.get('type') in ('item.started', 'item.completed')]
    if len(completions) != 1 or (item_positions and completions[0] < item_positions[-1]):
        raise ValueError('incomplete_turn')
    return calls


def validate_call_facts(calls):
    """Check relationships against the synthetic work, not model prose."""
    if len(calls) != len(TOOLS) or [item[0] for item in calls] != list(TOOLS):
        raise ValueError('incomplete_calls')
    start, intent, api, file, edge, done, _ = (item[1] for item in calls)
    capability, api_id, file_id = intent.get('id'), api.get('id'), file.get('id')
    if not all(isinstance(value, str) and value for value in (capability, api_id, file_id)) or len({capability, api_id, file_id, 'root'}) != 4:
        raise ValueError('invalid_nodes')
    if (not all(isinstance(start.get(key), str) and start[key] for key in ('title', 'goal', 'project'))
            or api.get('kind') != 'fn' or api.get('signature') != 'double(value)' or api.get('capability') != capability
            or file.get('path') != 'cli.mjs' or file.get('action') != 'create' or file.get('capability') != capability
            or edge.get('from') != file_id or edge.get('to') != api_id or edge.get('kind') != 'calls'
            or done.get('id') != capability or done.get('quote') != '命令入口调用 double'
            or done.get('context') != '已用正负输入验证输出'):
        raise ValueError('fact_mismatch')
    return capability, api_id, file_id


def validate_event_projection(calls, events):
    kinds = ('task.start', 'capability.intent', 'api.expose', 'file.change', 'call.edge', 'capability.done', 'task.done')
    if not calls or len(calls) > len(kinds) or len(events) != len(calls) or [item[0] for item in calls] != list(TOOLS[:len(calls)]):
        raise ValueError('event_count')
    for index, ((name, args, receipt), event) in enumerate(zip(calls, events), start=1):
        payload = {('capabilityId' if key == 'capability' else key): value for key, value in args.items()
                   if key not in ('session', 'operation_id', 'add', 'del')}
        if name == 'trace_file':
            payload['stats'] = {'add': args.get('add', 0), 'del': args.get('del', 0)}
        if name == 'trace_call' and 'kind' not in payload:
            payload['kind'] = 'calls'
        if (receipt.get('status') == 'committed' and receipt.get('seq') != index
                or event.get('v') != 1 or event.get('session') != args.get('session')
                or event.get('seq') != index or event.get('type') != kinds[index - 1]
                or event.get('payload') != payload or not isinstance(event.get('ts'), str)):
            raise ValueError('event_mismatch')
    return True


def validate_pages(pages):
    """Check a complete sequence of authenticated one-item pages."""
    events = []
    last_watermark = None
    for index, page in enumerate(pages):
        if not isinstance(page, dict) or not isinstance(page.get('events'), list):
            raise ValueError('invalid_page')
        items = page['events']
        watermark = page.get('last_seq')
        if type(watermark) is not int or watermark < 0 or (last_watermark is not None and watermark < last_watermark):
            raise ValueError('invalid_watermark')
        if len(items) > 1 or (not items and index != len(pages) - 1):
            raise ValueError('invalid_page')
        if items:
            item = items[0]
            if not isinstance(item, dict) or type(item.get('seq')) is not int or item['seq'] != len(events) + 1 or item['seq'] > watermark:
                raise ValueError('invalid_sequence')
            events.append(item)
        elif watermark != len(events):
            raise ValueError('incomplete_prefix')
        last_watermark = watermark
    if not pages or pages[-1]['events']:
        raise ValueError('incomplete_prefix')
    return events


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def read_events(endpoint, session):
    """Authenticated read only from the validated IPv4 loopback endpoint."""
    if not isinstance(endpoint, dict) or type(endpoint.get('port')) is not int or not 1 <= endpoint['port'] <= 65535:
        raise ValueError('invalid_endpoint')
    if not isinstance(endpoint.get('token'), str) or not endpoint['token'] or not isinstance(endpoint.get('store_id'), str) or not endpoint['store_id']:
        raise ValueError('invalid_endpoint')
    if not re.fullmatch(r'[A-Za-z0-9_-]{1,128}', session):
        raise ValueError('invalid_session')
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    base = f"http://127.0.0.1:{endpoint['port']}"
    def get(path):
        request = urllib.request.Request(base + path, headers={'Authorization': 'Bearer ' + endpoint['token']})
        with opener.open(request, timeout=5) as response:
            data = response.read(1048577)
        if len(data) > 1048576:
            raise ValueError('oversized_response')
        return json.loads(data)
    health = get('/v1/health')
    if health.get('version') != 1 or health.get('store_id') != endpoint['store_id']:
        raise ValueError('store_mismatch')
    pages = []
    for _ in range(10001):
        page = get(f'/v1/sessions/{session}/events?since_seq={len(pages)}&limit=1')
        pages.append(page)
        if not page.get('events'):
            events = validate_pages(pages)
            if any(event.get('session') != session for event in events):
                raise ValueError('session_mismatch')
            return events
    raise ValueError('too_many_events')


def validate_synthetic_sources(work_dir):
    """Recognize only the small ESM forms used in this synthetic exercise."""
    work = Path(work_dir)
    if not safe_existing_path(str(work), directory=True):
        raise ValueError('unsafe_work_dir')
    texts = []
    for name in ('index.mjs', 'cli.mjs'):
        path = work / name
        if not safe_existing_path(str(path), directory=False) or path.stat().st_size > 4096:
            raise ValueError('unsafe_source')
        # An initial UTF-8 BOM is permitted by JavaScript; retain fail-closed
        # matching for every remaining character, including a second BOM.
        texts.append(path.read_text(encoding='utf-8').removeprefix('\ufeff'))
    # Fail closed for unknown syntax: never execute arbitrary model output.
    index = re.fullmatch(r'\s*export\s+function\s+double\s*\(\s*value\s*\)\s*\{\s*return\s+value\s*\*\s*2\s*;?\s*\}\s*', texts[0])
    cli = re.fullmatch(r'''\s*import\s*\{\s*double\s*\}\s*from\s*['"]\./index\.mjs['"]\s*;?\s*console\.log\s*\(\s*JSON\.stringify\s*\(\s*double\s*\(\s*Number\s*\(\s*process\.argv\[2\]\s*\)\s*\)\s*\)\s*\)\s*;?\s*''', texts[1])
    if not index or not cli:
        raise ValueError('unsupported_synthetic_source')
    return True


def verify_synthetic_execution(work_dir, node, deadline=None):
    work = Path(work_dir)
    if not safe_existing_path(str(node), directory=False) or not validate_synthetic_sources(work):
        raise ValueError('invalid_synthetic_work')
    files = [work / 'index.mjs', work / 'cli.mjs']
    hashes = [hashlib.sha256(path.read_bytes()).digest() for path in files]
    env = {key: os.environ[key] for key in ('SystemRoot', 'WINDIR', 'PATH', 'TEMP', 'TMP') if key in os.environ}
    for argument, expected in (('4', b'8'), ('-3', b'-6')):
        output = bounded_stdout([str(node), str(files[1]), argument], cwd=work, env=env, timeout=time_left(deadline, 10) if deadline is not None else 10, limit=1024)
        if output.strip() != expected:
            raise ValueError('synthetic_output_mismatch')
    if any(not safe_existing_path(str(path), directory=False) or hashlib.sha256(path.read_bytes()).digest() != expected for path, expected in zip(files, hashes)):
        raise ValueError('synthetic_source_changed')
    return True


def wait_events(endpoint, session, count, timeout=60):
    import time
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            events = read_events(endpoint, session)
            if len(events) == count:
                return events
            if len(events) > count:
                raise ValueError('unexpected_event_count')
        except urllib.error.HTTPError as error:
            try:
                if error.code != 404:
                    raise ValueError('gateway_read_failed') from None
            finally:
                error.close()
        time.sleep(.2)
    raise ValueError('gateway_wait_timeout')


def time_left(deadline, cap):
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise ValueError('scenario_timeout')
    return min(cap, remaining)


def online_scenario(codex, home, model, artifact, run, repo, desktop, page, node, evidence, deadline, proxy=None):
    """Two real turns only; caller must have completed all preflight and review."""
    ident = run['id']
    session = 'online-' + ident
    title = 'MindTrail synthetic ' + ident
    operation_ids = [ident + '-' + suffix for suffix in ('start', 'intent', 'api', 'file', 'call', 'done', 'task-done')]
    work = run['work']
    if any(item.name != '.mindtrail-work-owner' for item in work.iterdir()):
        raise ValueError('work_not_empty')
    endpoint = desktop.endpoint(timeout=time_left(deadline, 45))
    first_prompt = (f'本次为隔离合成任务。session={session}。按顺序仅调用 trace_task_start 和 trace_intent，'
                    f'operation_id 分别是 {operation_ids[0]} 和 {operation_ids[1]}。'
                    f'task.start 的 title={title}，goal=验证合成调用，project=synthetic。'
                    '自行选择能力 id 并保留。先不要创建任何文件；两次调用成功后结束本回合。')
    first = model_turn(codex, home, model, artifact, run, repo, first_prompt, session, operation_ids[:2], TOOLS[:2], timeout=time_left(deadline, 120), diagnostics=evidence, proxy=proxy)
    evidence['stage_online_calls_done'] = True
    if first[0][1].get('title') != title or first[0][1].get('goal') != '验证合成调用' or first[0][1].get('project') != 'synthetic':
        raise ValueError('task_mismatch')
    capability = first[1][1].get('id')
    if not isinstance(capability, str) or not capability or any(item.name != '.mindtrail-work-owner' for item in work.iterdir()):
        raise ValueError('prefix_mismatch')
    evidence['stage_online_read_started'] = True
    prefix = wait_events(endpoint, session, 2, timeout=time_left(deadline, 60))
    evidence['stage_online_read_done'] = True
    validate_event_projection(first, prefix)
    evidence['stage_online_projection_done'] = True
    evidence['stage_online_ui_started'] = True
    page.expect_prefix(title, capability, desktop.job.process, evidence=evidence)
    evidence['stage_online_ui_done'] = True
    evidence['online_prefix'] = True
    later = (f'继续同一合成任务，session={session}，能力 id={capability}。'
             '创建 index.mjs，内容采用 export function double(value) { return value * 2; }；'
             '创建 cli.mjs，采用 import { double } from "./index.mjs"; console.log(JSON.stringify(double(Number(process.argv[2]))));。'
             '验证 node cli.mjs 4 输出 8、node cli.mjs -3 输出 -6。验证成功后依次调用 '
             'trace_api、trace_file、trace_call、trace_done、trace_task_done，operation_id 依次为 '
             + ', '.join(operation_ids[2:]) + '。API signature=double(value)、kind=fn；文件路径 cli.mjs、action=create；'
             '两者均关联该能力。调用边为文件节点指向 API 节点，kind=calls。'
             'trace_done 的 quote=命令入口调用 double，context=已用正负输入验证输出。'
             '每次等待回执，明确错误即停止，不得虚构完成。')
    rest = model_turn(codex, home, model, artifact, run, repo, later, session, operation_ids[2:], TOOLS[2:], timeout=time_left(deadline, 120), diagnostics=evidence, proxy=proxy)
    calls = first + rest
    cap, api, file = validate_call_facts(calls)
    evidence['stage_code_started'] = True
    verify_synthetic_execution(work, node, deadline)
    evidence['stage_code_done'] = True
    events = wait_events(endpoint, session, 7, timeout=time_left(deadline, 60))
    validate_event_projection(calls, events)
    page.expect_complete(title, cap, api, file, desktop.job.process, evidence=evidence)
    evidence['online_done'] = True
    evidence['desktop'] = True
    return {'online_prefix': True, 'online_done': True, 'desktop': True}


def offline_scenario(codex, home, model, artifact, run, repo, desktop_artifact, debug_port, node, evidence, deadline, proxy=None):
    """Offline admission and same-queue recovery; caller owns the preview."""
    from mindtrail_real_agent_desktop import DesktopProcess, Workbench
    ident = run['id']; session = 'offline-' + ident; title = 'MindTrail offline ' + ident
    ids = [ident + '-' + suffix for suffix in ('start', 'intent', 'api', 'file', 'call', 'done', 'task-done')]
    first_desktop = DesktopProcess(desktop_artifact, run, repo, debug_port=debug_port)
    try:
        old = first_desktop.endpoint(timeout=time_left(deadline, 45))
    finally:
        first_desktop.close()
    import socket
    with socket.socket() as probe:
        probe.settimeout(1)
        if probe.connect_ex(('127.0.0.1', old['port'])) == 0:
            raise ValueError('old_gateway_still_listening')
    prompt = (f'隔离合成任务 session={session}。按顺序调用 trace_task_start、trace_intent，'
              f'operation_id 分别为 {ids[0]}、{ids[1]}。标题={title}，goal=验证合成调用，project=synthetic。'
              '自行选择能力、API 和文件 id 并维持引用。创建 index.mjs，内容采用 '
              'export function double(value) { return value * 2; }；创建 cli.mjs，内容采用 '
              'import { double } from "./index.mjs"; console.log(JSON.stringify(double(Number(process.argv[2]))));。'
              '验证 node cli.mjs 4 输出 8、node cli.mjs -3 输出 -6。然后依次调用 '
              'trace_api、trace_file、trace_call、trace_done、trace_task_done，operation_id 分别为 '
              + ', '.join(ids[2:]) + '。API kind=fn、signature=double(value)，文件 path=cli.mjs、action=create，'
              '两者关联能力，调用边从文件指向 API、kind=calls。trace_done 的 quote=命令入口调用 double，'
              'context=已用正负输入验证输出。每次等待回执；明确错误时停止，不要伪造完成。')
    if any(item.name != '.mindtrail-work-owner' for item in run['work'].iterdir()):
        raise ValueError('work_not_empty')
    calls = model_turn(codex, home, model, artifact, run, repo, prompt, session, ids, TOOLS, timeout=time_left(deadline, 120), diagnostics=evidence, proxy=proxy)
    if any(receipt.get('status') != 'queued' for _, _, receipt in calls):
        raise ValueError('offline_not_queued')
    evidence['offline_receipts'] = True
    if calls[0][1].get('title') != title or calls[0][1].get('goal') != '验证合成调用' or calls[0][1].get('project') != 'synthetic':
        raise ValueError('task_mismatch')
    cap, api, file = validate_call_facts(calls)
    verify_synthetic_execution(run['work'], node, deadline)
    queue_file = run['state'] / 'producers' / 'mcp' / run['queue'] / 'queue.sqlite3'
    if not safe_existing_path(str(queue_file), directory=False):
        raise ValueError('queue_missing')
    desktop = DesktopProcess(desktop_artifact, run, repo, debug_port=debug_port)
    try:
        endpoint = desktop.endpoint(timeout=time_left(deadline, 45))
        if old['store_id'] != endpoint['store_id'] or old['token'] == endpoint['token']:
            raise ValueError('desktop_identity_mismatch')
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
        request = urllib.request.Request(f"http://127.0.0.1:{endpoint['port']}/v1/health", headers={'Authorization': 'Bearer ' + old['token']})
        try:
            opener.open(request, timeout=5).close()
        except urllib.error.HTTPError as error:
            try:
                if error.code != 401:
                    raise ValueError('old_token_check_failed') from None
            finally:
                error.close()
        else:
            raise ValueError('old_token_accepted')
        started, finished = queue.Queue(), queue.Queue()
        cancellation = threading.Event()
        def keepalive():
            try:
                value = model_turn(codex, home, model, artifact, run, repo, '只保持连接一小段时间，禁止调用任何 trace 工具，然后结束。', session, [], (), started=started, cancel=cancellation, timeout=time_left(deadline, 120), diagnostics=evidence, proxy=proxy, recovery_guard=True)
                finished.put((True, value))
            except BaseException:
                finished.put((False, None))
        worker = threading.Thread(target=keepalive, daemon=True); worker.start()
        try:
            process, recovery_job = started.get(timeout=20)
            evidence['recovery_process_started'] = True
        except queue.Empty:
            cancellation.set()
            worker.join(timeout=10)
            raise ValueError('recovery_start_timeout') from None
        automatic = False
        cancelled = False
        try:
            import time
            recovery_deadline = time.monotonic() + time_left(deadline, 60)
            while time.monotonic() < recovery_deadline and process.poll() is None:
                try:
                    events = read_events(endpoint, session)
                    if len(events) == 7 and process.poll() is None and recovery_guard_ready(run['state'], recovery_job):
                        validate_event_projection(calls, events)
                        if process.poll() is not None or not recovery_guard_ready(run['state'], recovery_job):
                            break
                        evidence['recovery_events_seven_while_alive'] = True
                        automatic = True; break
                except urllib.error.HTTPError as error:
                    try:
                        if error.code != 404: raise ValueError('gateway_read_failed') from None
                    finally:
                        error.close()
                time.sleep(.2)
            if automatic:
                worker.join(timeout=10)
        finally:
            still_alive = process.poll() is None
            evidence['recovery_process_exited_before_events'] = not automatic and not still_alive
            evidence['recovery_process_alive_after_observation'] = still_alive
            if still_alive:
                cancelled = True
                evidence['recovery_cancelled'] = True
                try:
                    process.terminate()
                except OSError:
                    pass
            worker.join(timeout=15)
        if worker.is_alive():
            raise ValueError('recovery_turn_stuck')
        evidence['recovery_worker_finished'] = True
        no_tools, _ = finished.get_nowait()
        evidence['recovery_turn_no_tools'] = no_tools
        if not no_tools and not cancelled:
            raise ValueError('recovery_turn_failed')
        if process.poll() is None or recovery_job.handle is not None:
            raise ValueError('recovery_turn_stuck')
        evidence['recovery_job_reaped'] = True
        violation = run['state'] / 'observer-recovery-violation'
        if violation.exists() or violation.is_symlink():
            raise ValueError('recovery_turn_failed')
        if automatic:
            evidence['recovery_guard_no_calls'] = True
        retry_prompt = ('只重试这一次既有工具调用，参数必须逐字相同，不调用其他 trace：'
                        + json.dumps({'tool': 'trace_task_start', 'arguments': calls[0][1]}, ensure_ascii=False))
        retry = model_turn(codex, home, model, artifact, run, repo, retry_prompt, session, ids[:1], TOOLS[:1], timeout=time_left(deadline, 120), diagnostics=evidence, proxy=proxy)
        if retry[0][1] != calls[0][1] or retry[0][2].get('status') != 'committed' or retry[0][2].get('seq') != 1:
            raise ValueError('retry_mismatch')
        events = wait_events(endpoint, session, 7, timeout=time_left(deadline, 60))
        validate_event_projection(calls, events)
        evidence['restart'] = True
        evidence['automatic_recovery'] = automatic
        if read_events(endpoint, session) != events:
            raise ValueError('retry_changed_events')
        evidence['retry'] = True
        page = Workbench(debug_port, desktop.job, timeout=time_left(deadline, 45))
        try:
            page.deadline = deadline
            page.expect_complete(title, cap, api, file, desktop.job.process, evidence=evidence)
        finally:
            page.close()
        return {'offline_receipts': True, 'restart': True, 'automatic_recovery': automatic, 'retry': True, 'desktop': True}
    finally:
        desktop.close()


def execute_live(args, repo):
    """Prepared live orchestration; main intentionally leaves it gated for review."""
    import socket
    from mindtrail_real_agent_desktop import build_desktop, DesktopProcess, PreviewProcess, Workbench
    repo = Path(repo).resolve(strict=True)
    default = Path(os.environ.get('USERPROFILE', str(Path.home()))) / '.codex'
    error = validate_live_paths(args.codex_exe, args.codex_home, args.manifest, args.report, repo, default)
    if error:
        raise ValueError(error)
    evidence = {}
    model_stage = False
    try:
        if args.use_mxc:
            from mindtrail_cli_local_probe import require_non_elevated
            require_non_elevated()
        proxy = read_user_proxy() if args.use_user_proxy else None
        artifact = build_fixture(repo)
        env = {key: os.environ[key] for key in ('SystemRoot', 'WINDIR', 'PATH', 'TEMP', 'TMP') if key in os.environ}
        verify_isolated_cli(args.codex_exe, repo)
        with locked_cli_bundle(Path(args.codex_exe)):
            evidence['cli_version'] = static_cli_probe(Path(args.codex_exe), repo / '.tools', env)
            static_config_probe(Path(args.codex_exe), artifact.executable, repo / '.tools', env, isolate_proxy=proxy is not None)
        evidence['configuration'] = True
        desktop_artifact = build_desktop(repo)
        node = shutil.which('node.exe')
        if not node:
            raise ValueError('node_missing')
        online = prepare_run(repo, artifact)
        evidence['handshake'] = True
        offline = prepare_run(repo, artifact)
        if args.use_mxc:
            online['mxc'] = offline['mxc'] = True
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0)); debug_port = sock.getsockname()[1]
        preview = PreviewProcess(repo, Path(node))
        deadline = time.monotonic() + 480
        try:
            desktop = DesktopProcess(desktop_artifact, online, repo, debug_port=debug_port)
            try:
                desktop.endpoint(timeout=time_left(deadline, 45))
                page = Workbench(debug_port, desktop.job, timeout=time_left(deadline, 45))
                try:
                    page.deadline = deadline
                    model_stage = True
                    online_scenario(Path(args.codex_exe), Path(args.codex_home), args.model, artifact, online, repo, desktop, page, Path(node), evidence, deadline, proxy=proxy)
                finally:
                    page.close()
            finally:
                desktop.close()
            offline_scenario(Path(args.codex_exe), Path(args.codex_home), args.model, artifact, offline, repo, desktop_artifact, debug_port, Path(node), evidence, deadline, proxy=proxy)
        finally:
            preview.close()
        required = ('configuration', 'handshake', 'online_prefix', 'online_done', 'offline_receipts', 'restart', 'automatic_recovery', 'retry', 'desktop')
        evidence['status'] = 'passed' if all(evidence.get(key) is True for key in required) else 'failed'
        if evidence['status'] != 'passed':
            evidence['code'] = 'automatic_recovery_unverified'
        return evidence
    except Exception as error:
        evidence['status'] = 'failed' if model_stage else 'blocked'
        if not isinstance(error, ValueError):
            record_exception_type(evidence, error)
        detail = str(error) if isinstance(error, ValueError) else ''
        evidence['code'] = detail if detail in PUBLIC_CODES else ('scenario_failed' if model_stage else 'preflight_failed')
        raise
    finally:
        write_report(Path(args.report), repo / '.tools' / 'real-agent-reports', evidence)


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument('--live', action='store_true')
    parser.add_argument('--use-user-proxy', action='store_true')
    parser.add_argument('--use-mxc', action='store_true')
    parser.add_argument('--codex-exe')
    parser.add_argument('--codex-home')
    parser.add_argument('--model')
    parser.add_argument('--manifest')
    parser.add_argument('--report')
    args = parser.parse_args(argv)
    # This entry is deliberately closed until all isolation checks exist.
    if args.live:
        if not all((args.codex_exe, args.codex_home, args.model, args.manifest, args.report)):
            code = 'missing_isolation_arguments'
        else:
            repo = Path(__file__).resolve().parent.parent
            default_home = Path(os.environ.get('USERPROFILE', str(Path.home()))) / '.codex'
            code = validate_live_paths(args.codex_exe, args.codex_home, args.manifest, args.report, repo, default_home)
            if code is None:
                try:
                    evidence = execute_live(args, repo)
                except Exception:
                    print(json.dumps({'status': 'failed', 'code': 'scenario_failed'}))
                    return 1
                status = evidence.get('status')
                print(json.dumps({'status': status if status in ('passed', 'failed') else 'failed'}))
                return 0 if status == 'passed' else 1
        print(json.dumps({'status': 'blocked', 'code': code}))
        return 2
    try:
        ok = local_probe(Path(__file__).resolve().parent.parent)
        from mindtrail_real_agent_desktop import local_desktop_probe
        desktop_ok = local_desktop_probe(Path(__file__).resolve().parent.parent)
    except (OSError, ValueError, UnicodeError, subprocess.SubprocessError, queue.Empty):
        print(json.dumps({'status': 'blocked', 'code': 'local_probe_failed'}))
        return 2
    print(json.dumps({'status': 'blocked', 'code': 'local_acceptance_incomplete', 'fixture_probe': bool(ok), 'desktop_probe': bool(desktop_ok)}))
    return 2


if __name__ == '__main__':
    sys.exit(main())
