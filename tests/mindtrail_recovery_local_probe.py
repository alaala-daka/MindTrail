"""Empty-home real CLI and synthetic provider test of the recovery guard."""
import argparse
import contextlib
import hashlib
import http.server
import json
import os
from pathlib import Path
import queue
import sqlite3
import sys
import threading
import time
import uuid

from mindtrail_real_agent import (CliLock, build_fixture, codex_arguments, locked_cli_bundle,
                                  prepare_run, recovery_guard_ready, run_owned_turn, safe_existing_path)
from mindtrail_synthetic_provider import synthetic_response


def main(attempt):
    repo = Path(__file__).resolve().parent.parent
    home = repo / '.tools' / ('codex-recovery-probe-' + uuid.uuid4().hex)
    if not safe_existing_path(str(home.parent), directory=True):
        raise ValueError('invalid_probe')
    home.mkdir()
    artifact = build_fixture(repo)
    run = prepare_run(repo, artifact)
    cli = repo / '.tools/codex-isolated-f271b77dac234b2a86590f4f695dfbce/codex.exe'
    observer, guard, python = repo / 'tests/mindtrail_mcp_observer.py', repo / 'tests/mindtrail_recovery_guard.py', Path(sys.executable).resolve(strict=True)
    release, got_request = threading.Event(), threading.Event()
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_): pass
        def do_POST(self):
            try:
                size = int(self.headers.get('Content-Length', '0'))
                if not 0 < size <= 1048576: raise ValueError('size')
                request = json.loads(self.rfile.read(size)); got_request.set()
                if not release.wait(20): raise ValueError('timeout')
                payload = synthetic_response(request, 'synthetic-session', 'synthetic-operation', attempt and self.server.calls == 0)
                self.server.calls += 1
                self.send_response(200); self.send_header('Content-Type', 'text/event-stream'); self.send_header('Content-Length', str(len(payload))); self.end_headers(); self.wfile.write(payload)
            except (ValueError, OSError, TypeError):
                pass
    class Server(http.server.HTTPServer):
        calls = 0
        def get_request(self):
            connection, address = super().get_request(); connection.settimeout(5); return connection, address
    server = Server(('127.0.0.1', 0), Handler)
    server_thread = threading.Thread(target=server.serve_forever, daemon=True); server_thread.start()
    result = {}; started = queue.Queue(); finished = queue.Queue(); process = None
    try:
        args = codex_arguments(cli, run['work'], 'synthetic-model', artifact.executable, run['state'], run['queue'])
        for index, value in enumerate(args):
            if value.startswith('mcp_servers.mindtrail.command='): args[index] = 'mcp_servers.mindtrail.command=' + json.dumps(python.as_posix())
            if value.startswith('mcp_servers.mindtrail.args='): args[index] = 'mcp_servers.mindtrail.args=' + json.dumps([observer.as_posix()])
        overrides = [('model_provider', 'synthetic'), ('model_providers.synthetic.name', 'synthetic'),
                     ('model_providers.synthetic.base_url', f'http://127.0.0.1:{server.server_port}/v1'),
                     ('model_providers.synthetic.env_key', 'MINDTRAIL_SYNTHETIC_KEY'), ('model_providers.synthetic.wire_api', 'responses'),
                     ('model_providers.synthetic.requires_openai_auth', False), ('model_providers.synthetic.supports_websockets', False),
                     ('mcp_servers.mindtrail.env.MINDTRAIL_PROBE_EXE', artifact.executable.as_posix()),
                     ('mcp_servers.mindtrail.env.MINDTRAIL_PROBE_SHA256', artifact.digest),
                     ('mcp_servers.mindtrail.env.MINDTRAIL_RECOVERY_GUARD', '1')]
        if attempt: overrides.append(('mcp_servers.mindtrail.tools.trace_task_start.approval_mode', 'approve'))
        for key, value in overrides: args[-1:-1] = ['-c', key + '=' + json.dumps(value)]
        env = {key: os.environ[key] for key in ('SystemRoot', 'WINDIR', 'PATH', 'TEMP', 'TMP') if key in os.environ}
        env.update(CODEX_HOME=str(home), MINDTRAIL_SYNTHETIC_KEY='synthetic-only')
        def worker():
            try:
                with contextlib.ExitStack() as stack:
                    stack.enter_context(locked_cli_bundle(cli))
                    for path in (python, observer, guard, artifact.executable):
                        stack.enter_context(CliLock(path, hashlib.sha256(path.read_bytes()).hexdigest()))
                    run_owned_turn(args, cwd=run['work'], env=env, prompt=b'Synthetic only.', started=started, started_job=True, timeout=45)
                finished.put(True)
            except BaseException:
                finished.put(False)
        thread = threading.Thread(target=worker, daemon=True); thread.start()
        try:
            process, job = started.get(timeout=20)
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline and process.poll() is None and not recovery_guard_ready(run['state'], job): time.sleep(.05)
            result['cli_and_producer_alive'] = process.poll() is None and recovery_guard_ready(run['state'], job)
            result['provider_request'] = got_request.wait(10)
        finally:
            release.set()
        thread.join(timeout=55)
        if thread.is_alive():
            if process is not None and process.poll() is None: process.terminate()
            thread.join(timeout=15)
        result['worker_exited'] = not thread.is_alive()
        result['cli_exited'] = process is not None and process.poll() is not None
        result['guard_violation'] = (run['state'] / 'observer-recovery-violation').is_file()
        db = run['state'] / 'producers/mcp' / run['queue'] / 'queue.sqlite3'
        if db.is_file():
            with sqlite3.connect(db.resolve().as_uri() + '?mode=ro', uri=True) as conn: result['queue_entries'] = conn.execute('select count(*) from entries').fetchone()[0]
        return result
    finally:
        release.set(); server.shutdown(); server_thread.join(timeout=10); server.server_close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(); parser.add_argument('--attempt-tool', action='store_true'); options = parser.parse_args()
    try: print(json.dumps(main(options.attempt_tool), sort_keys=True))
    except Exception: print(json.dumps({'probe_failed': True})); raise SystemExit(1) from None
