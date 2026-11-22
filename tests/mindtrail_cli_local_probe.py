"""Observe CLI MCP startup with a synthetic loopback provider and empty home.

The provider is configured to loopback; this does not prove OS-level egress
isolation for the CLI. No real credentials or raw CLI output are retained.
"""
import hashlib
import argparse
import contextlib
import ctypes
import http.server
import json
import ntpath
import os
from pathlib import Path
import re
import shlex
import sqlite3
import threading
import uuid

from mindtrail_real_agent import (CliLock, PUBLIC_CODES, bounded_stdout, build_fixture, codex_arguments,
                                  locked_cli_bundle, prepare_run, public_evidence, run_owned_turn, safe_existing_path, validate_calls)
from mindtrail_synthetic_provider import synthetic_response

SYNTHETIC_ERROR_WORDS = ('failed', 'call', 'mcp', 'invalid', 'tool', 'argument', 'schema',
                         'deserialize', 'timed', 'error', 'server', 'closed', 'channel', 'response',
                         'process', 'connection', 'request', 'found', 'parse', 'initialize', 'required',
                         'session', 'protocol', 'cancel', 'json', 'result', 'execution', 'task',
                         'operation', 'expected', 'unsupported', 'read', 'write', 'utf', 'denied',
                         'unknown', 'not', 'missing', 'no', 'empty', 'unexpected')
SYNTHETIC_ERROR_PHRASES = {'blocked_app_configuration': 'MCP tool call blocked by app configuration',
                           'approval_never': 'MCP tool call requires approval, but approval policy is never',
                           'cancelled_approval': 'MCP tool call cancelled before approval',
                           'interactive_approval_unavailable': 'MCP tool call cancelled: interactive approval is unavailable in exec mode'}
COMMAND_OUTPUT_HINTS = {'approval': ('approval', 'not approved', 'rejected'),
                        'sandbox': ('sandbox',), 'permission': ('permission denied', 'access denied'),
                        'unavailable': ('unavailable', 'not available', 'not supported'),
                        'tool_error': ('error', 'failed')}
COMMAND_APPROVAL_PHRASES = {
    'policy_never': 'approval required by policy, but AskForApproval is set to Never',
    'policy_granular_sandbox': 'approval required by policy, but AskForApproval::Granular.sandbox_approval is false',
    'policy_granular_rules': 'approval required by policy rule, but AskForApproval::Granular.rules is false',
    'rejected_user': 'rejected by user',
    'rejected_configuration': 'rejected by configuration',
    'policy_is': 'approval policy is',
    'reject_command': 'reject command',
    'command_rejected': 'command rejected',
    'escalation_disallowed': 'you cannot ask for escalated permissions if the approval policy is',
    'blocked_by_policy': 'blocked by policy',
    'policy_forbids_prefix': 'policy forbids commands starting with',
    'unsafe_style_commands': 'commands are not permitted. Use a safer approach',
    'exec_handler_failed': 'exec_command failed',
    'create_process': 'CreateProcess',
    'process_rejected': 'Rejected(',
}
POLICY_NAMES = ('exec_command', 'echo', 'cmd', 'cmd.exe', 'powershell', 'powershell.exe',
                'pwsh', 'pwsh.exe', 'bash', 'bash.exe', 'sandbox', 'execpolicy')
SYNTHETIC_OS_ERRORS = (5, 1260, 4551)
SYNTHETIC_HRESULTS = ('0x80070005', '0x800704ec', '0x800711c7')
MXC_STATIC_ERRORS = {
    'native_unavailable_executor': 'native MXC is unavailable on this executor',
    'native_unavailable_windows': 'native MXC is unavailable on this Windows build',
    'native_deny_unsupported': 'this Windows build cannot enforce native MXC deny paths',
    'managed_requires_elevated': 'managed networking requires the elevated Windows sandbox backend',
    'managed_requires_local_proxy': 'managed networking requires an executor-local proxy',
    'managed_requires_proxy_ports': 'MXC managed networking requires dedicated proxy ports',
    'managed_loopback_unsupported': 'MXC cannot enforce managed networking with allow_local_binding=false',
    'absolute_policy_cwd': 'MXC requires an absolute policy working directory',
    'absolute_command_cwd': 'MXC requires an absolute command working directory',
    'unicode_policy_cwd': 'MXC requires a Unicode policy working directory',
    'unicode_command_cwd': 'MXC requires a Unicode command working directory',
    'unicode_policy_paths': 'MXC requires Unicode filesystem policy paths',
    'unicode_env_names': 'MXC requires Unicode environment variable names',
    'unicode_env_values': 'MXC requires Unicode environment variable values',
    'nonempty_unicode_host': 'MXC requires nonempty Unicode host ',
    'missing_host_systemdrive': 'MXC requires nonempty Unicode host SystemDrive',
    'missing_host_localappdata': 'MXC requires nonempty Unicode host LOCALAPPDATA',
    'deny_parent_missing': 'MXC currently requires deny paths to have existing parent directories.',
    'invalid_launcher_request': 'invalid MXC launcher request',
    'empty_command': 'MXC command must not be empty',
    'symbolic_path_unsupported': 'MXC policy contains an unresolved symbolic filesystem path',
    'unsupported_learning_mode': 'MXC native launch does not support permissiveLearningMode',
    'missing_session_launcher': 'a native Windows sandbox requires its session launcher',
    'runtime_read_roots_required': 'runtime read access requires explicit workspace roots',
}
SYNTHETIC_ITEM_ERROR_PHRASES = {
    'untrusted_project': 'Untrusted project-local configuration is disabled',
    'model_rerouted': 'model rerouted:',
    'rate_limit': 'rate limit',
    'deprecation': 'deprecated',
    'configuration': 'configuration',
    'sandbox': 'sandbox',
    'network': 'network',
    'approval': 'approval',
    'provider': 'provider',
    'model': 'model',
}


def project_synthetic_mxc_error(output):
    """Match only source-derived phrases; caller supplies only synthetic output."""
    text = output.lower() if isinstance(output, str) else ''
    result = {}
    for name, phrase in MXC_STATIC_ERRORS.items():
        if name in ('missing_host_systemdrive', 'missing_host_localappdata'):
            result['mxc_error_' + name] = bool(re.search(re.escape(phrase.lower()) + r'(?![a-z0-9_])', text))
        else:
            result['mxc_error_' + name] = phrase.lower() in text
    result['mxc_error_any_known'] = any(result.values())
    return result


def project_synthetic_item_errors(records):
    """Summarize only fixed error-item structure and relative command order."""
    errors = [(i, record['item']) for i, record in enumerate(records)
              if isinstance(record, dict) and record.get('type') == 'item.completed'
              and isinstance(record.get('item'), dict) and record['item'].get('type') == 'error']
    commands = [i for i, record in enumerate(records) if isinstance(record, dict)
                and record.get('type') == 'item.started' and isinstance(record.get('item'), dict)
                and record['item'].get('type') == 'command_execution']
    result = {'synthetic_error_item_present': bool(errors),
              'synthetic_error_item_message_string': any(isinstance(item.get('message'), str) for _, item in errors),
              'synthetic_error_item_before_command': bool(commands) and any(i < commands[0] for i, _ in errors),
              'synthetic_error_item_after_command': bool(commands) and any(i > commands[0] for i, _ in errors)}
    for name, phrase in SYNTHETIC_ITEM_ERROR_PHRASES.items():
        result['synthetic_error_item_phrase_' + name] = any(isinstance(item.get('message'), str) and phrase.lower() in item['message'].lower() for _, item in errors)
    return result


def validate_synthetic_command_sequence(records, count):
    """Require distinct ordered start/completion pairs with zero exit codes."""
    pending = set()
    completed = set()
    for record in records:
        if not isinstance(record, dict) or record.get('type') not in ('item.started', 'item.completed'):
            continue
        item = record.get('item')
        if not isinstance(item, dict) or item.get('type') != 'command_execution':
            continue
        ident = item.get('id')
        if not isinstance(ident, str) or not ident:
            return False
        if record['type'] == 'item.started':
            if pending or ident in completed:
                return False
            pending.add(ident)
        else:
            if ident not in pending or type(item.get('exit_code')) is not int or item['exit_code'] != 0:
                return False
            pending.remove(ident)
            completed.add(ident)
    return not pending and len(completed) == count


def validate_synthetic_write_execute(records):
    """Require the two fixed commands, paired results and a final completed turn."""
    expected = ("Set-Content -LiteralPath synthetic-command.js -Value 'console.log(42)' -Encoding ascii",
                'node synthetic-command.js')
    if not validate_synthetic_command_sequence(records, 2):
        return False
    if not records or not isinstance(records[-1], dict) or records[-1].get('type') != 'turn.completed':
        return False
    if sum(isinstance(r, dict) and r.get('type') == 'turn.completed' for r in records) != 1:
        return False
    commands = [r for r in records if isinstance(r, dict) and r.get('type') in ('item.started', 'item.completed')
                and isinstance(r.get('item'), dict) and r['item'].get('type') == 'command_execution']
    if len(commands) != 4:
        return False
    for index, command in enumerate(expected):
        start, end = commands[index * 2:index * 2 + 2]
        if start['type'] != 'item.started' or end['type'] != 'item.completed':
            return False
        if start['item'].get('command') != command or end['item'].get('command') != command:
            return False
    output = commands[3]['item'].get('aggregated_output')
    return isinstance(output, str) and output.strip() == '42'


def validate_synthetic_wrapped_write_execute(records, windows_directory):
    """Validate only the synthetic CLI event display, not executable identity."""
    if not isinstance(windows_directory, str) or not ntpath.isabs(windows_directory):
        return False
    if not validate_synthetic_command_sequence(records, 2) or not records or not isinstance(records[-1], dict) or records[-1].get('type') != 'turn.completed':
        return False
    if sum(isinstance(r, dict) and r.get('type') == 'turn.completed' for r in records) != 1:
        return False
    if any(isinstance(r, dict) and r.get('type') in ('error', 'turn.failed') for r in records):
        return False
    warning = 'Model metadata for `synthetic-model` not found. Defaulting to fallback metadata; this can degrade performance and cause issues.'
    errors = [r['item'] for r in records if isinstance(r, dict) and r.get('type') == 'item.completed'
              and isinstance(r.get('item'), dict) and r['item'].get('type') == 'error']
    if len(errors) > 1 or any(e.get('message') != warning for e in errors):
        return False
    commands = [r for r in records if isinstance(r, dict) and r.get('type') in ('item.started', 'item.completed')
                and isinstance(r.get('item'), dict) and r['item'].get('type') == 'command_execution']
    if len(commands) != 4:
        return False
    expected = ("Set-Content -LiteralPath synthetic-command.js -Value 'console.log(42)' -Encoding ascii", 'node synthetic-command.js')
    executable = ntpath.normcase(ntpath.normpath(ntpath.join(windows_directory, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe')))
    for index, command in enumerate(expected):
        start, end = commands[index * 2:index * 2 + 2]
        if start['type'] != 'item.started' or end['type'] != 'item.completed':
            return False
        for event in (start, end):
            try:
                argv = shlex.split(event['item'].get('command', ''), posix=True)
            except (TypeError, ValueError):
                return False
            if len(argv) != 3 or ntpath.normcase(ntpath.normpath(argv[0])) != executable or argv[1:] != ['-Command', command]:
                return False
    output = commands[3]['item'].get('aggregated_output')
    return isinstance(output, str) and output.strip() == '42'


def project_synthetic_write_execute(records):
    """Project fixed structural clues only; never return event text."""
    completed = [r['item'] for r in records if isinstance(r, dict) and r.get('type') == 'item.completed'
                 and isinstance(r.get('item'), dict) and r['item'].get('type') == 'command_execution']
    commands = ("Set-Content -LiteralPath synthetic-command.js -Value 'console.log(42)' -Encoding ascii", 'node synthetic-command.js')
    result = {}
    for index, label in enumerate(('first', 'second')):
        item = completed[index] if len(completed) > index else {}
        command = item.get('command')
        result['synthetic_' + label + '_command_string'] = isinstance(command, str)
        result['synthetic_' + label + '_command_exact'] = command == commands[index]
        result['synthetic_' + label + '_powershell_wrapper_exact'] = command in (
            'powershell -Command ' + commands[index], 'powershell.exe -Command ' + commands[index])
        lower = command.lower() if isinstance(command, str) else ''
        for token, spelling in (('powershell_exe', 'powershell.exe'), ('pwsh_exe', 'pwsh.exe'),
                                ('no_profile', '-noprofile'), ('command_flag', '-command'),
                                ('encoded_command', '-encodedcommand')):
            result['synthetic_' + label + '_has_' + token] = spelling in lower
        result['synthetic_' + label + '_contains_fixed_command'] = isinstance(command, str) and commands[index] in command
    second = completed[1] if len(completed) > 1 else {}
    result['synthetic_second_exit_zero'] = type(second.get('exit_code')) is int and second['exit_code'] == 0
    output = second.get('aggregated_output')
    result['synthetic_second_output_42'] = isinstance(output, str) and output.strip() == '42'
    errors = [r['item'] for r in records if isinstance(r, dict) and r.get('type') == 'item.completed'
              and isinstance(r.get('item'), dict) and r['item'].get('type') == 'error']
    result['synthetic_error_reroute_template'] = any(isinstance(e.get('message'), str) and re.fullmatch(r'model rerouted: .+ -> .+ \(.+\)', e['message']) is not None for e in errors)
    fallback = 'Model metadata for `synthetic-model` not found. Defaulting to fallback metadata; this can degrade performance and cause issues.'
    result['synthetic_error_fixed_model_metadata_warning'] = any(e.get('message') == fallback for e in errors)
    return result


def observe_sandbox_selection(home, tools_root, ident):
    """Read bounded fixed-pattern logs only after validating this run's home marker."""
    home, tools_root = Path(home), Path(tools_root)
    if (not re.fullmatch('[0-9a-f]{32}', ident) or home.parent != tools_root
            or not re.fullmatch('codex-local-probe-[0-9a-f]{32}', home.name)
            or not safe_existing_path(str(tools_root), directory=True)
            or not safe_existing_path(str(home), directory=True)):
        raise ValueError('invalid_probe_home')
    marker = home / '.mindtrail-probe-owner'
    if not safe_existing_path(str(marker), directory=False) or marker.stat().st_size != 32 or marker.read_text(encoding='ascii') != ident:
        raise ValueError('invalid_probe_home')
    result = {'selection_log_present': False, 'selection_log_safe': True,
              **{'selection_' + value: False for value in ('mxc', 'elevated', 'unelevated', 'disabled')}}
    directory = home / '.sandbox'
    if not directory.exists():
        return result
    if not safe_existing_path(str(directory), directory=True):
        raise ValueError('invalid_probe_log')
    logs = list(directory.glob('sandbox.*.log'))
    if len(logs) > 16:
        raise ValueError('invalid_probe_log')
    result['selection_log_present'] = bool(logs)
    for path in logs:
        if not safe_existing_path(str(path), directory=False) or path.stat().st_nlink != 1 or path.stat().st_size > 4 * 1024 * 1024:
            raise ValueError('invalid_probe_log')
        with path.open('rb') as stream:
            total = 0
            while True:
                line = stream.readline(8193)
                if not line:
                    break
                total += len(line)
                if len(line) > 8192 or total > 4 * 1024 * 1024:
                    raise ValueError('invalid_probe_log')
                match = re.search(rb'Windows sandbox selection: (mxc|elevated|unelevated|disabled)(?![A-Za-z0-9_-])', line)
                if match:
                    result['selection_' + match.group(1).decode('ascii')] = True
    return result


def validate_mxc_probe(*, requested, order, simulate_command, shape, observe, proxy, approve, responses):
    """Only admit a single fixed read-only synthetic comparison."""
    if requested and (order != 'direct' or not simulate_command or shape not in ('read-only', 'write-execute')
                      or not observe or proxy or approve or responses != (2 if shape == 'write-execute' else 1)):
        raise ValueError('invalid_probe_mode')
    return requested


def validate_mxc_variant(readonly, write_execute, shape):
    if (readonly and write_execute) or (readonly and shape != 'read-only') or (write_execute and shape != 'write-execute'):
        raise ValueError('invalid_probe_mode')
    return readonly or write_execute


def add_synthetic_systemdrive(env, windows_root):
    """Add only the drive derived from the existing absolute SystemRoot."""
    root = env.get('SystemRoot')
    if (not isinstance(root, str) or not isinstance(windows_root, str) or not ntpath.isabs(root)
            or ntpath.normcase(ntpath.normpath(root)) != ntpath.normcase(ntpath.normpath(windows_root))):
        raise ValueError('invalid_probe_mode')
    drive, tail = ntpath.splitdrive(root)
    if not re.fullmatch('[A-Za-z]:', drive) or not tail.startswith('\\'):
        raise ValueError('invalid_probe_mode')
    if any(key.lower() in ('localappdata', 'systemdrive') for key in env):
        raise ValueError('invalid_probe_mode')
    result = dict(env)
    result['SystemDrive'] = drive
    return result


def add_synthetic_localappdata(env, home, tools_root, ident):
    """Create a fresh run-bound local app data root without consulting the user profile."""
    home, tools_root = Path(home), Path(tools_root)
    if (not re.fullmatch('[0-9a-f]{32}', ident) or home.parent != tools_root
            or not re.fullmatch('codex-local-probe-[0-9a-f]{32}', home.name)
            or not safe_existing_path(str(tools_root), directory=True)
            or not safe_existing_path(str(home), directory=True)
            or env.get('CODEX_HOME') != str(home)
            or env.get('TEMP') != str(home / 'tmp') or env.get('TMP') != str(home / 'tmp')
            or not safe_existing_path(str(home / 'tmp'), directory=True)
            or any(key.lower() == 'localappdata' for key in env)):
        raise ValueError('invalid_probe_home')
    root = env.get('SystemRoot')
    drive = env.get('SystemDrive')
    if not isinstance(root, str) or not ntpath.isabs(root) or not isinstance(drive, str) or ntpath.splitdrive(root)[0].lower() != drive.lower() or not re.fullmatch('[A-Za-z]:', drive):
        raise ValueError('invalid_probe_home')
    marker = home / '.mindtrail-probe-owner'
    digest = hashlib.sha256(ident.encode('ascii')).hexdigest()
    with CliLock(marker, digest):
        if not safe_existing_path(str(marker), directory=False) or marker.stat().st_size != 32 or marker.read_text(encoding='ascii') != ident:
            raise ValueError('invalid_probe_home')
        local = home / 'localappdata'
        local.mkdir()
        if not safe_existing_path(str(local), directory=True):
            raise ValueError('invalid_probe_home')
        result = dict(env)
        result['LOCALAPPDATA'] = str(local)
        return result


def audit_synthetic_localappdata(home, tools_root, ident):
    """Count only metadata under the fresh owned directory; never follow links."""
    home, tools_root = Path(home), Path(tools_root)
    if (not re.fullmatch('[0-9a-f]{32}', ident) or home.parent != tools_root
            or not re.fullmatch('codex-local-probe-[0-9a-f]{32}', home.name)
            or not safe_existing_path(str(home), directory=True)):
        raise ValueError('invalid_probe_home')
    marker = home / '.mindtrail-probe-owner'
    digest = hashlib.sha256(ident.encode('ascii')).hexdigest()
    result = {'local_audit_complete': False, 'local_audit_files': 0, 'local_audit_dirs': 0,
              'local_audit_reparse': 0, 'local_audit_other': 0}
    with CliLock(marker, digest):
        if not safe_existing_path(str(marker), directory=False) or marker.stat().st_size != 32 or marker.read_text(encoding='ascii') != ident:
            raise ValueError('invalid_probe_home')
        local = home / 'localappdata'
        if not safe_existing_path(str(local), directory=True):
            raise ValueError('invalid_probe_home')
        pending = [(local, 0)]
        while pending:
            directory, depth = pending.pop()
            if depth > 32 or not safe_existing_path(str(directory), directory=True):
                raise ValueError('invalid_probe_home')
            with os.scandir(directory) as entries:
                for entry in entries:
                    if sum(result[k] for k in ('local_audit_files', 'local_audit_dirs', 'local_audit_reparse', 'local_audit_other')) >= 10000:
                        raise ValueError('invalid_probe_home')
                    info = entry.stat(follow_symlinks=False)
                    if entry.is_symlink() or getattr(info, 'st_file_attributes', 0) & 0x400:
                        result['local_audit_reparse'] += 1
                    elif entry.is_dir(follow_symlinks=False):
                        result['local_audit_dirs'] += 1
                        pending.append((Path(entry.path), depth + 1))
                    elif entry.is_file(follow_symlinks=False):
                        result['local_audit_files'] += 1
                    else:
                        result['local_audit_other'] += 1
    result['local_audit_complete'] = True
    return result


def get_windows_directory():
    kernel = ctypes.WinDLL('kernel32', use_last_error=True, winmode=0x800)
    kernel.GetWindowsDirectoryW.argtypes = [ctypes.POINTER(ctypes.c_wchar), ctypes.c_uint32]
    kernel.GetWindowsDirectoryW.restype = ctypes.c_uint32
    buffer = ctypes.create_unicode_buffer(32768)
    length = kernel.GetWindowsDirectoryW(buffer, len(buffer))
    if not length or length >= len(buffer):
        raise ValueError('invalid_probe_mode')
    return buffer.value


def apply_mxc_probe_config(args):
    if (not args or args[-1] != '-' or args.count('-s') != 1
            or args[args.index('-s') + 1:args.index('-s') + 2] != ['workspace-write']
            or '--ignore-user-config' not in args or '--ephemeral' not in args
            or '-a' in args or any('windows.sandbox=' in arg for arg in args)):
        raise ValueError('invalid_probe_mode')
    result = list(args)
    result[1:1] = ['-a', 'never']
    result[-1:-1] = ['-c', 'windows.sandbox="mxc"']
    return result


def require_non_elevated():
    if os.name != 'nt':
        raise ValueError('invalid_probe_mode')
    kernel = ctypes.WinDLL('kernel32', use_last_error=True, winmode=0x800)
    advapi = ctypes.WinDLL('advapi32', use_last_error=True, winmode=0x800)
    kernel.GetCurrentProcess.restype = ctypes.c_void_p
    kernel.CloseHandle.argtypes = [ctypes.c_void_p]
    kernel.CloseHandle.restype = ctypes.c_int
    advapi.OpenProcessToken.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.POINTER(ctypes.c_void_p)]
    advapi.OpenProcessToken.restype = ctypes.c_int
    advapi.GetTokenInformation.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_void_p, ctypes.c_uint32, ctypes.POINTER(ctypes.c_uint32)]
    advapi.GetTokenInformation.restype = ctypes.c_int
    token = ctypes.c_void_p()
    try:
        elevation, length = ctypes.c_uint32(), ctypes.c_uint32()
        if (not advapi.OpenProcessToken(kernel.GetCurrentProcess(), 8, ctypes.byref(token))
                or not advapi.GetTokenInformation(token, 20, ctypes.byref(elevation), ctypes.sizeof(elevation), ctypes.byref(length))
                or length.value != ctypes.sizeof(elevation) or elevation.value != 0):
            raise ValueError('invalid_probe_mode')
    finally:
        if token.value:
            kernel.CloseHandle(token)
SAFE_COMMAND_WORDS = frozenset(('approval', 'approve', 'required', 'requires', 'request', 'policy',
                                'never', 'set', 'to', 'not', 'no', 'because', 'the', 'is', 'for',
                                'command', 'execution', 'sandbox', 'permission', 'user', 'denied',
                                'rejected', 'failed', 'error', 'available', 'unavailable', 'cannot',
                                'disabled', 'mode', 'in', 'with', 'and', 'on', 'this', 'operation',
                                'untrusted', 'workspace', 'write', 'read', 'auto', 'review'))

def observe_second_request(synthetic, request_number, request):
    if request_number == 2 and isinstance(request, dict) and isinstance(request.get('input'), list):
        synthetic['second_has_function_output'] = any(isinstance(item, dict) and item.get('type') == 'function_call_output' and item.get('call_id') == 'call_synthetic' for item in request['input'])
        outputs = [item.get('output') for item in request['input'] if isinstance(item, dict) and item.get('type') == 'function_call_output' and item.get('call_id') == 'call_synthetic']
        synthetic['command_output_string'] = len(outputs) == 1 and isinstance(outputs[0], str)
        text = outputs[0].lower() if synthetic['command_output_string'] else ''
        if synthetic.get('mxc_probe') is True:
            synthetic.update(project_synthetic_mxc_error(outputs[0] if synthetic['command_output_string'] else None))
        synthetic['command_output_fixed_tokens'] = [token for token in re.findall(r'[a-z]+', text) if token in SAFE_COMMAND_WORDS][:80]
        for category, terms in COMMAND_OUTPUT_HINTS.items():
            synthetic['command_output_hint_' + category] = any(term in text for term in terms)
        for category, phrase in COMMAND_APPROVAL_PHRASES.items():
            synthetic['command_output_phrase_' + category] = phrase.lower() in text
        for code in SYNTHETIC_OS_ERRORS:
            synthetic['command_os_error_' + str(code)] = bool(re.search(r'\bos error\s+' + str(code) + r'\b', text))
        for code in SYNTHETIC_HRESULTS:
            synthetic['command_hresult_' + code] = bool(re.search(r'(?<![0-9a-f])' + code + r'(?![0-9a-f])', text))
        match = re.search(r'`([^`\r\n]{1,256})` rejected: blocked by policy `([^`\r\n]{1,256})`', text)
        synthetic['command_policy_template_match'] = bool(match)
        if match:
            for index, label in enumerate(('subject', 'rule'), 1):
                synthetic['command_policy_' + label + '_known'] = match.group(index) in POLICY_NAMES
                for name in POLICY_NAMES:
                    safe_name = name.replace('.', '_')
                    synthetic['command_policy_' + label + '_' + safe_name] = match.group(index) == name
        for kind in ('message', 'function_call', 'function_call_output', 'mcp_call', 'mcp_call_output'):
            synthetic['second_input_' + kind] = sum(isinstance(item, dict) and item.get('type') == kind for item in request['input'])


def observe_tools(request):
    offered = request.get('tools') if isinstance(request, dict) else None
    tools = offered if isinstance(offered, list) else []
    result = {'tools_count': len(tools)}
    for kind in ('function', 'namespace', 'custom', 'mcp'):
        result['tools_type_' + kind] = sum(isinstance(item, dict) and item.get('type') == kind for item in tools)
    result['tools_nested_list'] = sum(isinstance(item, dict) and isinstance(item.get('tools'), list) for item in tools)
    result['tools_nested_function'] = sum(isinstance(item, dict) and isinstance(item.get('function'), dict) for item in tools)
    result['tools_target_top'] = any(isinstance(item, dict) and item.get('name') in ('mcp__mindtrail__trace_task_start', 'trace_task_start') for item in tools)
    result['tools_target_nested'] = any(isinstance(item, dict) and isinstance(item.get('tools'), list) and any(isinstance(child, dict) and child.get('name') == 'trace_task_start' for child in item['tools']) for item in tools)
    for candidate in ('shell', 'shell_command', 'exec_command', 'apply_patch', 'write_stdin', 'write_file', 'edit_file', 'patch', 'apply_diff', 'python'):
        result['tools_top_' + candidate] = any(isinstance(item, dict) and item.get('name') == candidate for item in tools)
        result['tools_nested_' + candidate] = any(isinstance(item, dict) and isinstance(item.get('tools'), list) and any(isinstance(child, dict) and child.get('name') == candidate for child in item['tools']) for item in tools)
    exec_tools = [item for item in tools if isinstance(item, dict) and item.get('name') == 'exec_command']
    if len(exec_tools) == 1:
        schema = exec_tools[0].get('parameters')
        result['exec_schema_object'] = isinstance(schema, dict)
        if isinstance(schema, dict):
            props, required = schema.get('properties'), schema.get('required')
            for field in ('cmd', 'command', 'workdir', 'yield_time_ms'):
                result['exec_property_' + field] = isinstance(props, dict) and field in props
                result['exec_required_' + field] = isinstance(required, list) and field in required
    result['tools_mindtrail_namespace'] = any(isinstance(item, dict) and item.get('type') == 'namespace' and item.get('name') == 'mindtrail' for item in tools)
    for candidate in ('mcp__mindtrail', 'mcp_mindtrail', 'functions', 'mcp'):
        result['tools_namespace_' + candidate] = any(isinstance(item, dict) and item.get('type') == 'namespace' and item.get('name') == candidate and isinstance(item.get('tools'), list) and any(isinstance(child, dict) and child.get('name') == 'trace_task_start' for child in item['tools']) for item in tools)
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--order', choices=('forward', 'reverse', 'direct', 'control'), default='forward')
    parser.add_argument('--check-proxy-isolation', action='store_true')
    parser.add_argument('--simulate-tool', action='store_true')
    parser.add_argument('--simulate-command', action='store_true')
    parser.add_argument('--command-shape', choices=('write', 'read-only', 'write-execute'), default='write')
    parser.add_argument('--synthetic-call-responses', type=int, choices=(1, 2), default=1)
    parser.add_argument('--approve-synthetic-start', action='store_true')
    parser.add_argument('--observe-sandbox-selection', action='store_true')
    parser.add_argument('--mxc-readonly-probe', action='store_true')
    parser.add_argument('--mxc-write-execute-probe', action='store_true')
    parser.add_argument('--mxc-systemdrive-probe', action='store_true')
    parser.add_argument('--mxc-isolated-localappdata-probe', action='store_true')
    options = parser.parse_args()
    order = options.order
    check_proxy = options.check_proxy_isolation
    simulate_command = options.simulate_command
    simulate_tool = options.simulate_tool or simulate_command
    if options.simulate_tool and simulate_command:
        raise ValueError('invalid_probe_mode')
    if not simulate_command and options.command_shape != 'write':
        raise ValueError('invalid_probe_mode')
    if check_proxy and order != 'control':
        raise ValueError('invalid_probe_mode')
    if simulate_tool and (order != 'direct' or check_proxy):
        raise ValueError('invalid_probe_mode')
    if not simulate_tool and options.synthetic_call_responses != 1:
        raise ValueError('invalid_probe_mode')
    if options.approve_synthetic_start and (not simulate_tool or simulate_command):
        raise ValueError('invalid_probe_mode')
    if options.observe_sandbox_selection and (not simulate_command or options.command_shape not in ('read-only', 'write-execute')):
        raise ValueError('invalid_probe_mode')
    mxc_requested = validate_mxc_variant(options.mxc_readonly_probe, options.mxc_write_execute_probe, options.command_shape)
    mxc_probe = validate_mxc_probe(requested=mxc_requested, order=order,
        simulate_command=simulate_command, shape=options.command_shape, observe=options.observe_sandbox_selection,
        proxy=check_proxy, approve=options.approve_synthetic_start, responses=options.synthetic_call_responses)
    if options.mxc_systemdrive_probe and not mxc_probe:
        raise ValueError('invalid_probe_mode')
    if options.mxc_isolated_localappdata_probe and not options.mxc_systemdrive_probe:
        raise ValueError('invalid_probe_mode')
    if options.command_shape == 'write-execute' and not (mxc_probe and options.mxc_systemdrive_probe and options.mxc_isolated_localappdata_probe):
        raise ValueError('invalid_probe_mode')
    if mxc_probe:
        require_non_elevated()
    repo = Path(__file__).resolve().parent.parent
    cli = repo / '.tools/codex-isolated-f271b77dac234b2a86590f4f695dfbce/codex.exe'
    result = {}
    requests = [0]
    synthetic = {'session': 'synthetic-session', 'operation': 'synthetic-operation', 'provider_valid': False,
                 'second_has_function_output': False, 'mxc_probe': mxc_probe}

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass
        def do_POST(self):
            requests[0] += 1
            if simulate_tool:
                try:
                    length = int(self.headers.get('Content-Length', '0'))
                    if not 0 < length <= 1048576:
                        raise ValueError('invalid_request')
                    request = json.loads(self.rfile.read(length))
                    if requests[0] <= 2 and isinstance(request, dict):
                        offered = request.get('tools')
                        synthetic['request_' + str(requests[0]) + '_offers_tool'] = isinstance(offered, list) and any(isinstance(item, dict) and item.get('name') == 'mcp__mindtrail__trace_task_start' for item in offered)
                        synthetic.update({'request_' + str(requests[0]) + '_' + key: value for key, value in observe_tools(request).items()})
                    observe_second_request(synthetic, requests[0], request)
                    emit_call = requests[0] <= options.synthetic_call_responses
                    payload = synthetic_response(request, synthetic['session'], synthetic['operation'], emit_call, command=simulate_command, command_shape=options.command_shape, call_index=requests[0])
                    if emit_call:
                        synthetic['call_responses_sent'] = synthetic.get('call_responses_sent', 0) + 1
                    synthetic['provider_valid'] = True
                    self.send_response(200)
                    self.send_header('Content-Type', 'text/event-stream')
                    self.send_header('Content-Length', str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                    return
                except (ValueError, TypeError, OSError):
                    pass
            self.send_response(503)
            self.send_header('Content-Length', '0')
            self.end_headers()
        def do_GET(self):
            requests[0] += 1
            self.send_response(503)
            self.send_header('Content-Length', '0')
            self.end_headers()

    class BoundedServer(http.server.HTTPServer):
        def get_request(self):
            connection, address = super().get_request()
            connection.settimeout(5)
            return connection, address

    server = BoundedServer(('127.0.0.1', 0), Handler)
    server_thread = threading.Thread(target=server.serve_forever, daemon=True)
    server_thread.start()
    try:
        home = None
        if mxc_probe:
            home = repo / '.tools' / ('codex-local-probe-' + uuid.uuid4().hex)
            if not safe_existing_path(str(home.parent), directory=True):
                raise ValueError('invalid_probe_home')
            home.mkdir()
            temp = home / 'tmp'
            temp.mkdir()
            if not safe_existing_path(str(temp), directory=True):
                raise ValueError('invalid_probe_home')
            # This changes only this short-lived probe process and its children.
            os.environ['TEMP'] = os.environ['TMP'] = str(temp)
        artifact = build_fixture(repo)
        control = None
        control_digest = None
        if order == 'control':
            control = artifact.executable.parent / ('mindtrail-mcp-control-' + uuid.uuid4().hex + '.exe')
            build_env = {key: os.environ[key] for key in ('SystemRoot', 'SystemDrive', 'WINDIR', 'PATH', 'TEMP', 'TMP') if key in os.environ}
            build_env.update(CARGO_HOME=str(repo / '.tools/cargo'), RUSTUP_HOME=str(repo / '.tools/rustup'))
            bounded_stdout([str(repo / '.tools/cargo/bin/rustc.exe'), '--edition=2024', str(repo / 'tests/fixtures/mindtrail_mcp_control.rs'), '-o', str(control)], cwd=repo, env=build_env, timeout=120, limit=1048576)
            control_digest = hashlib.sha256(control.read_bytes()).hexdigest()
        shared = prepare_run(repo, artifact) if order != 'reverse' else None
        if home is None:
            home = repo / '.tools' / ('codex-local-probe-' + uuid.uuid4().hex)
            if not safe_existing_path(str(home.parent), directory=True):
                raise ValueError('invalid_probe_home')
            home.mkdir()
        if options.observe_sandbox_selection:
            with (home / '.mindtrail-probe-owner').open('x', encoding='ascii') as marker:
                marker.write(shared['id'])
        env = {key: os.environ[key] for key in ('SystemRoot', 'WINDIR', 'PATH', 'TEMP', 'TMP') if key in os.environ}
        env.update(CODEX_HOME=str(home), MINDTRAIL_SYNTHETIC_KEY='synthetic-only')
        if options.mxc_systemdrive_probe:
            env = add_synthetic_systemdrive(env, get_windows_directory())
        if options.mxc_isolated_localappdata_probe:
            env = add_synthetic_localappdata(env, home, repo / '.tools', shared['id'])
        if check_proxy:
            env.update(HTTPS_PROXY='http://127.0.0.1:9', NO_PROXY='localhost,127.0.0.1,::1')
        modes = {'forward': ('direct', 'observer'), 'reverse': ('observer', 'direct'), 'direct': ('direct',), 'control': ('control',)}[order]
        for mode in modes:
            run = shared if shared is not None else prepare_run(repo, artifact)
            probe_queue = (run['state'] / 'producers/mcp/probe/queue.sqlite3').is_file()
            if any(item.name != '.mindtrail-work-owner' for item in run['work'].iterdir()):
                raise ValueError('unexpected_work_file')
            result = {'order': order, 'mode': mode, 'provider_hit': False, 'business_queue': False, 'probe_queue': probe_queue, 'cli_completed': False, 'egress_restricted': False, 'provider_requests_before': requests[0]}
            if mxc_probe:
                result['mxc_probe_temp_isolated'] = env['TEMP'] == env['TMP'] == str(home / 'tmp')
                result['mxc_probe_non_elevated'] = True
                result['mxc_probe_host_systemdrive_present'] = bool(env.get('SystemDrive'))
                result['mxc_probe_host_localappdata_present'] = any(key.lower() == 'localappdata' for key in env)
                result['mxc_probe_localappdata_isolated'] = env.get('LOCALAPPDATA') == str(home / 'localappdata')
            result['business_queue_before'] = (run['state'] / 'producers/mcp/observer-business/queue.sqlite3').is_file()
            args = codex_arguments(cli, run['work'], 'synthetic-model', artifact.executable, run['state'], 'observer-business', isolate_proxy=check_proxy)
            if mode == 'control':
                for index, argument in enumerate(args):
                    if argument.startswith('mcp_servers.mindtrail.command='):
                        args[index] = 'mcp_servers.mindtrail.command=' + json.dumps(control.as_posix())
            if mode == 'observer':
                for index, argument in enumerate(args):
                    if argument.startswith('mcp_servers.mindtrail.command='):
                        args[index] = 'mcp_servers.mindtrail.command=' + json.dumps(Path(os.sys.executable).as_posix())
                    elif argument.startswith('mcp_servers.mindtrail.args='):
                        args[index] = 'mcp_servers.mindtrail.args=' + json.dumps([(repo / 'tests/mindtrail_mcp_observer.py').as_posix()])
            overrides = [
                ('model_provider', 'synthetic'),
                ('model_providers.synthetic.name', 'synthetic'),
                ('model_providers.synthetic.base_url', f'http://127.0.0.1:{server.server_port}/v1'),
                ('model_providers.synthetic.env_key', 'MINDTRAIL_SYNTHETIC_KEY'),
                ('model_providers.synthetic.wire_api', 'responses'),
                ('model_providers.synthetic.requires_openai_auth', False),
                ('model_providers.synthetic.supports_websockets', False),
            ]
            if mode == 'direct':
                overrides.append(('mcp_servers.mindtrail.env.MINDTRAIL_TEST_PROCESS_MARKERS', '1'))
            if mode == 'observer':
                overrides.extend((('mcp_servers.mindtrail.env.MINDTRAIL_PROBE_EXE', artifact.executable.as_posix()),
                                  ('mcp_servers.mindtrail.env.MINDTRAIL_PROBE_SHA256', artifact.digest)))
            for key, value in overrides:
                args[-1:-1] = ['-c', key + '=' + json.dumps(value)]
            if options.approve_synthetic_start:
                args[-1:-1] = ['-c', 'mcp_servers.mindtrail.tools.trace_task_start.approval_mode="approve"']
            if mxc_probe:
                args = apply_mxc_probe_config(args)
            owner = run['work'].parent / 'owner'
            work_owner = run['work'] / '.mindtrail-work-owner'
            digest = hashlib.sha256(run['id'].encode('ascii')).hexdigest()
            diagnostic = {}
            try:
                extra_lock = CliLock(control, control_digest) if control is not None else contextlib.nullcontext()
                home_lock = CliLock(home / '.mindtrail-probe-owner', digest) if options.observe_sandbox_selection else contextlib.nullcontext()
                artifact_lock = CliLock(artifact.executable, artifact.digest) if mxc_probe else contextlib.nullcontext()
                with locked_cli_bundle(cli), CliLock(owner, digest), CliLock(work_owner, digest), extra_lock, home_lock, artifact_lock:
                    for directory in (run['work'], run['work'].parent, run['work'].parent.parent):
                        if any((directory / name).exists() or (directory / name).is_symlink() for name in ('AGENTS.md', 'AGENTS.override.md', '.codex', '.git')):
                            raise ValueError('unexpected_instructions')
                    records = run_owned_turn(args, cwd=run['work'], env=env, prompt=b'Synthetic local probe only.',
                                             timeout=45, diagnostics=diagnostic)
                    result['cli_completed'] = True
                    if simulate_tool:
                        for event_type in ('item.started', 'item.completed', 'turn.failed', 'turn.completed'):
                            result['synthetic_' + event_type.replace('.', '_') + '_count'] = sum(isinstance(record, dict) and record.get('type') == event_type for record in records)
                        known_types = ('mcp_tool_call', 'agent_message', 'command_execution', 'reasoning', 'error', 'web_search', 'file_change', 'todo_list')
                        for item_type in known_types:
                            result['synthetic_item_' + item_type + '_count'] = sum(isinstance(record, dict) and isinstance(record.get('item'), dict) and record['item'].get('type') == item_type for record in records)
                        if simulate_command:
                            result['synthetic_command_exit_zero'] = any(isinstance(record, dict) and record.get('type') == 'item.completed' and isinstance(record.get('item'), dict) and record['item'].get('type') == 'command_execution' and type(record['item'].get('exit_code')) is int and record['item'].get('exit_code') == 0 for record in records)
                            result.update(project_synthetic_item_errors(records))
                            result['synthetic_command_sequence_ok'] = validate_synthetic_command_sequence(records, 2 if options.command_shape == 'write-execute' else 1)
                            if options.command_shape == 'write-execute':
                                result.update(project_synthetic_write_execute(records))
                                result['synthetic_write_execute_records_ok'] = validate_synthetic_write_execute(records)
                                result['synthetic_wrapped_records_diagnostic'] = validate_synthetic_wrapped_write_execute(records, get_windows_directory())
                                result['synthetic_no_error_items'] = (not result['synthetic_error_item_present'] and not any(isinstance(record, dict) and record.get('type') in ('error', 'turn.failed') for record in records))
                        result['synthetic_item_other_count'] = sum(isinstance(record, dict) and isinstance(record.get('item'), dict) and record['item'].get('type') not in known_types for record in records)
                        try:
                            if simulate_command:
                                raise ValueError('synthetic_command_only')
                            calls = validate_calls(records, synthetic['session'], [synthetic['operation']], ('trace_task_start',), diagnostics=diagnostic)
                            result['synthetic_receipt_queued'] = calls[0][2].get('status') == 'queued'
                        except ValueError as error:
                            if not simulate_command:
                                result['synthetic_call_category'] = str(error) if str(error) in PUBLIC_CODES else 'probe_failed'
                            if str(error) == 'invalid_result':
                                for record in records:
                                    item = record.get('item') if isinstance(record, dict) and record.get('type') == 'item.completed' else None
                                    if not isinstance(item, dict) or item.get('type') != 'mcp_tool_call':
                                        continue
                                    payload = item.get('result')
                                    result['synthetic_result_object'] = isinstance(payload, dict)
                                    if isinstance(payload, dict):
                                        for key in ('content', 'structuredContent', 'structured_content', 'isError', 'is_error'):
                                            result['synthetic_result_has_' + key] = key in payload
                                        content = payload.get('content')
                                        result['synthetic_result_content_list'] = isinstance(content, list)
                                        result['synthetic_result_text_blocks'] = sum(isinstance(block, dict) and block.get('type') == 'text' and isinstance(block.get('text'), str) for block in content) if isinstance(content, list) else 0
                                        structured = payload.get('structured_content')
                                        result['synthetic_result_structured_object'] = isinstance(structured, dict)
                                        if isinstance(structured, dict):
                                            result['synthetic_result_status_queued'] = structured.get('status') == 'queued'
                                            result['synthetic_result_identity_matches'] = structured.get('session') == synthetic['session'] and structured.get('operation_id') == synthetic['operation']
                            if str(error) == 'tool_error':
                                for record in records:
                                    item = record.get('item') if isinstance(record, dict) else None
                                    problem = item.get('error') if isinstance(item, dict) else None
                                    message = problem.get('message') if isinstance(problem, dict) else None
                                    if isinstance(message, str):
                                        result.update({'synthetic_error_word_' + word: word in message.lower() for word in SYNTHETIC_ERROR_WORDS})
                                        result.update({'synthetic_error_phrase_' + name: phrase in message for name, phrase in SYNTHETIC_ERROR_PHRASES.items()})
            except ValueError as error:
                result['exit_category'] = str(error) if str(error) in PUBLIC_CODES else 'probe_failed'
            if options.observe_sandbox_selection:
                try:
                    with CliLock(home / '.mindtrail-probe-owner', digest):
                        result.update(observe_sandbox_selection(home, repo / '.tools', run['id']))
                except (ValueError, OSError, UnicodeError):
                    result['selection_log_safe'] = False
            if options.mxc_isolated_localappdata_probe:
                try:
                    result.update(audit_synthetic_localappdata(home, repo / '.tools', run['id']))
                except (ValueError, OSError, UnicodeError):
                    result['local_audit_complete'] = False
            result['business_queue'] = (run['state'] / 'producers/mcp/observer-business/queue.sqlite3').is_file()
            result['provider_requests_after'] = requests[0]
            result['provider_hit'] = result['provider_requests_after'] > result['provider_requests_before']
            if simulate_tool:
                result['synthetic_provider_valid'] = synthetic['provider_valid']
                result['synthetic_second_has_function_output'] = synthetic['second_has_function_output']
                if simulate_command:
                    marker = run['work'] / ('synthetic-command.js' if options.command_shape == 'write-execute' else 'synthetic-command.txt')
                    result['synthetic_command_file_exists'] = marker.is_file() and not marker.is_symlink()
                    expected_marker = b'console.log(42)' if options.command_shape == 'write-execute' else b'synthetic'
                    result['synthetic_command_file_matches'] = result['synthetic_command_file_exists'] and marker.stat().st_size <= 32 and marker.read_bytes().strip() == expected_marker
                    if options.command_shape == 'write-execute':
                        result['synthetic_write_execute_passed'] = all(result.get(key) is True for key in ('cli_completed', 'synthetic_no_error_items', 'synthetic_command_sequence_ok', 'synthetic_write_execute_records_ok', 'synthetic_command_file_matches'))
                result.update({'synthetic_' + key: value for key, value in synthetic.items() if key.startswith('second_input_') and type(value) is int})
                if simulate_command:
                    result.update({'synthetic_' + key: value for key, value in synthetic.items() if key.startswith(('command_output_', 'command_policy_', 'command_os_error_', 'command_hresult_')) and type(value) is bool})
                    if mxc_probe:
                        result.update({'synthetic_' + key: value for key, value in synthetic.items() if key.startswith('mxc_error_') and type(value) is bool})
                    result['synthetic_command_output_fixed_tokens'] = synthetic.get('command_output_fixed_tokens', [])
                for key in ('request_1_offers_tool', 'request_2_offers_tool', 'call_responses_sent'):
                    if key in synthetic:
                        result['synthetic_' + key] = synthetic[key]
                result.update({'synthetic_' + key: value for key, value in synthetic.items() if key.startswith(('request_1_tools_', 'request_2_tools_', 'request_1_exec_', 'request_2_exec_')) and type(value) in (bool, int)})
                queue_path = run['state'] / 'producers/mcp/observer-business/queue.sqlite3'
                if queue_path.is_file():
                    with sqlite3.connect(queue_path.resolve().as_uri() + '?mode=ro', uri=True) as db:
                        result['synthetic_queue_entries'] = db.execute('SELECT COUNT(*) FROM entries').fetchone()[0]
            for name in ('started', 'spawned', 'spawn-failed', 'initialize-request', 'initialize-response', 'tools-request', 'tools-response', 'exited'):
                result['observer_' + name.replace('-', '_')] = (run['state'] / ('observer-' + name)).is_file()
            for name in ('started', 'initialize-request', 'initialize-response', 'tools-request', 'tools-response', 'exited'):
                result['control_' + name.replace('-', '_')] = (run['state'] / ('control-' + name)).is_file()
            result['control_proxy_isolated'] = (run['state'] / 'control-proxy-isolated').is_file()
            for name in ('main', 'args', 'before-open', 'opened', 'open-path-security', 'open-path-other', 'open-lock', 'open-storage',
                         'stage-root', 'stage-producers', 'stage-mode', 'stage-queue', 'stage-preflight', 'stage-parent-chain', 'stage-leaf',
                         'os-2', 'os-3', 'os-5', 'os-6', 'os-32', 'os-33', 'os-87', 'os-123', 'os-183', 'os-267', 'os-995', 'os-1314', 'os-1332', 'os-other', 'os-unavailable',
                         'kind-not-found', 'kind-invalid-input', 'kind-permission', 'kind-other'):
                result['producer_' + name.replace('-', '_')] = (run['state'] / ('producer-' + name)).is_file()
            result.update({key: value for key, value in diagnostic.items() if key.startswith('diag_') and type(value) is bool})
            result.update({key: value for key, value in public_evidence(diagnostic).items() if key.startswith('tool_error_') and type(value) is bool})
            print(json.dumps(result, sort_keys=True))
    finally:
        server.shutdown()
        server_thread.join(timeout=10)
        server.server_close()


if __name__ == '__main__':
    try:
        main()
    except Exception:
        print(json.dumps({'probe_failed': True}))
        raise SystemExit(1) from None
