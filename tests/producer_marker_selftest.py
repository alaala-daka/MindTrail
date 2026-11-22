"""Isolated success and controlled failure checks for fixture-only markers."""
import json
import os
from pathlib import Path
import queue
import sqlite3
import subprocess
import threading

from mindtrail_real_agent import build_fixture, probe_producer, register_run
from mindtrail_real_agent_job import OwnedJob


def main():
    repo = Path(__file__).resolve().parent.parent
    tools = repo / '.tools'
    artifact = build_fixture(repo)
    base_env = {key: os.environ[key] for key in ('SystemRoot', 'WINDIR', 'PATH', 'TEMP', 'TMP') if key in os.environ}
    request = {'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {'protocolVersion': '2025-11-25', 'capabilities': {}, 'clientInfo': {'name': 'marker-selftest', 'version': '1'}}}
    data = json.dumps(request).encode() + b'\n'
    def setup():
        run = register_run(tools, tools / 'mcp-stage0-copy/src-tauri')
        probe_producer(artifact.executable, run['state'], base_env)
        env = {**base_env, 'MINDTRAIL_TEST_ROOT': str(run['state']), 'MINDTRAIL_TEST_PROCESS_MARKERS': '1'}
        return run, env
    def markers(run):
        return {name for name in ('opened', 'open-path-security', 'open-path-other', 'open-lock', 'open-storage') if (run['state'] / ('producer-' + name)).is_file()}
    def run_to_exit(run, env, name):
        with OwnedJob([str(artifact.executable), 'mcp', '--queue', name], cwd=run['work'], env=env) as job:
            job.process.communicate(data, timeout=25)
            return job.process.returncode
    success, env = setup()
    success_ok = run_to_exit(success, env, 'success') == 0 and markers(success) == {'opened'}
    lock_run, env = setup()
    first_env = {key: value for key, value in env.items() if key != 'MINDTRAIL_TEST_PROCESS_MARKERS'}
    with OwnedJob([str(artifact.executable), 'mcp', '--queue', 'locked'], cwd=lock_run['work'], env=first_env) as first:
        first.process.stdin.write(data); first.process.stdin.flush()
        output = queue.Queue()
        threading.Thread(target=lambda: output.put(first.process.stdout.readline()), daemon=True).start()
        ready = json.loads(output.get(timeout=15)).get('id') == 1
        second_exit = run_to_exit(lock_run, env, 'locked')
        lock_ok = ready and second_exit != 0 and markers(lock_run) == {'open-lock'}
    path_run, env = setup()
    target = path_run['fixture'] / 'junction-target'; target.mkdir()
    junction = path_run['state'] / 'producers/mcp/unsafe'
    ps_env = {**base_env, 'MARKER_JUNCTION': str(junction), 'MARKER_TARGET': str(target)}
    made = subprocess.run(['powershell.exe', '-NoProfile', '-NonInteractive', '-Command',
                           'New-Item -ItemType Junction -Path $env:MARKER_JUNCTION -Target $env:MARKER_TARGET | Out-Null'],
                          env=ps_env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15)
    path_ok = made.returncode == 0 and run_to_exit(path_run, env, 'unsafe') != 0 and markers(path_run) == {'open-path-security'}
    path_stage_ok = all((path_run['state'] / ('producer-' + name)).is_file() for name in ('stage-queue', 'stage-leaf', 'kind-permission'))
    other_run, env = setup()
    (other_run['state'] / 'producers/mcp/not-directory').write_bytes(b'synthetic')
    other_status = run_to_exit(other_run, env, 'not-directory')
    other_markers = markers(other_run)
    other_ok = other_status != 0 and other_markers == {'open-path-security'}
    acl_run, env = setup()
    acl_path = acl_run['state'] / 'producers/mcp/unsafe-acl'; acl_path.mkdir()
    grant = subprocess.run(['icacls.exe', str(acl_path), '/grant', '*S-1-1-0:(OI)(CI)(RX)'],
                           env=base_env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15)
    try:
        acl_ok = grant.returncode == 0 and run_to_exit(acl_run, env, 'unsafe-acl') != 0 and markers(acl_run) == {'open-path-security'}
    finally:
        restore = subprocess.run(['icacls.exe', str(acl_path), '/remove:g', '*S-1-1-0'],
                                 env=base_env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15)
        if restore.returncode != 0:
            raise ValueError('acl_restore_failed')
    storage_run, env = setup()
    db = storage_run['state'] / 'producers/mcp/probe/queue.sqlite3'
    with sqlite3.connect(db) as conn:
        conn.execute('PRAGMA user_version=999')
    storage_ok = run_to_exit(storage_run, env, 'probe') != 0 and markers(storage_run) == {'open-storage'}
    result = {'success': success_ok, 'lock': lock_ok, 'path_reparse': path_ok, 'path_stage': path_stage_ok, 'path_acl': acl_ok, 'path_wrong_type': other_ok, 'storage': storage_ok}
    if not other_ok:
        result.update({'other_nonzero': other_status != 0, 'other_marked_security': 'open-path-security' in other_markers,
                       'other_marked_other': 'open-path-other' in other_markers})
    print(json.dumps(result, sort_keys=True))
    return 0 if all(result.values()) else 1


if __name__ == '__main__':
    try:
        raise SystemExit(main())
    except Exception:
        print(json.dumps({'selftest_failed': True}))
        raise SystemExit(1) from None
