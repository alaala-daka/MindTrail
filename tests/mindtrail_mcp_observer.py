"""Synthetic stdio forwarder; records only fixed lifecycle markers."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import threading

from mindtrail_real_agent import safe_existing_path
from mindtrail_recovery_guard import classify_recovery_frame


def main():
    repo = Path(__file__).resolve().parent.parent
    root = Path(os.environ['MINDTRAIL_TEST_ROOT'])
    fixture = root.parent
    match = re.fullmatch(r'MindTrailAppEvent-([0-9a-f]{32})', fixture.name)
    expected_parent = repo / '.tools/mcp-stage0-copy/src-tauri'
    if (not match or fixture.parent != expected_parent or root != fixture / 'state'
            or not safe_existing_path(str(root), directory=True)
            or not safe_existing_path(str(fixture / 'owner'), directory=False)
            or (fixture / 'owner').read_text(encoding='ascii') != match.group(1)):
        return 2
    exe = Path(os.environ['MINDTRAIL_PROBE_EXE'])
    target = repo / '.tools/real-agent-fixture-target'
    if (not safe_existing_path(str(exe), directory=False) or not exe.resolve().is_relative_to(target.resolve())
            or hashlib.sha256(exe.read_bytes()).hexdigest() != os.environ['MINDTRAIL_PROBE_SHA256']):
        return 2
    recovery = os.environ.get('MINDTRAIL_RECOVERY_GUARD') == '1'
    def mark(name):
        try:
            with (root / ('observer-' + name)).open('x', encoding='ascii'):
                pass
        except FileExistsError:
            pass
    mark('started')
    env = {key: os.environ[key] for key in ('SystemRoot', 'WINDIR', 'PATH', 'TEMP', 'TMP') if key in os.environ}
    env['MINDTRAIL_TEST_ROOT'] = str(root)
    try:
        queue_name = 'run-' + match.group(1) if recovery else 'observer-business'
        child = subprocess.Popen([str(exe), 'mcp', '--queue', queue_name], env=env,
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                 creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
    except OSError:
        mark('spawn-failed')
        return 1
    mark('spawned')
    if recovery:
        with (root / 'observer-recovery-pid').open('x', encoding='ascii') as stream:
            stream.write(str(child.pid))
        mark('recovery-producer-started')
    ids = {}
    lock = threading.Lock()
    def forward(source, dest, inbound):
        try:
            while True:
                line = source.readline(1048577)
                if not line or len(line) > 1048576 or not line.endswith(b'\n'):
                    if recovery and inbound and line:
                        mark('recovery-violation')
                    return
                if recovery and inbound and classify_recovery_frame(line) != 'startup':
                    mark('recovery-violation')
                    return
                try:
                    obj = json.loads(line)
                    if isinstance(obj, dict):
                        with lock:
                            if inbound and obj.get('method') in ('initialize', 'tools/list') and 'id' in obj:
                                name = 'initialize' if obj['method'] == 'initialize' else 'tools'
                                ids[str(obj['id'])] = name
                                mark(name + '-request')
                            elif not inbound and 'id' in obj and str(obj['id']) in ids:
                                mark(ids[str(obj['id'])] + '-response')
                                if recovery and ids[str(obj['id'])] == 'tools' and isinstance(obj.get('result'), dict) and 'error' not in obj:
                                    mark('recovery-ready')
                except (ValueError, TypeError):
                    pass
                dest.write(line)
                dest.flush()
        except (OSError, ValueError):
            pass
        finally:
            if inbound:
                try: dest.close()
                except OSError: pass
    threading.Thread(target=forward, args=(sys.stdin.buffer, child.stdin, True), daemon=True).start()
    output = threading.Thread(target=forward, args=(child.stdout, sys.stdout.buffer, False), daemon=True)
    output.start()
    status = child.wait()
    if recovery:
        mark('recovery-producer-exited')
    output.join(timeout=5)
    mark('exited')
    return status


if __name__ == '__main__':
    try:
        raise SystemExit(main())
    except Exception:
        raise SystemExit(1) from None
