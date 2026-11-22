"""No-credential real desktop acceptance against the current source fixture.

Builds an isolated copy of the current source, starts an isolated Tauri WebView
fixture, injects synthetic gateway events and inspects the rendered UI through
CDP. The copy's source digest is checked against both the workspace and the
verified release stage; it never installs software and never mutates the
workspace fixtures or sealed release artifacts.
"""
import argparse
import hashlib
import http.server
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import socket
import stat
import subprocess
import sys
import threading
import time
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / 'tests'))
from mindtrail_real_agent_job import OwnedJob

STEPS = ('build_fixture', 'empty_state', 'connection_help', 'synthetic_events',
         'hierarchy_graph', 'calls_graph', 'task_replay', 'facts',
         'ai_not_configured', 'restart_identity', 'mock_generation', 'cancel_switch',
         'offline_error', 'shutdown')
EVENTS = [
    ('task.start', {'title': '桌面验收', 'goal': '验证桌面', 'project': 'isolated'}),
    ('capability.intent', {'id': 'cap', 'title': '能力'}),
    ('api.expose', {'id': 'api', 'capabilityId': 'cap', 'kind': 'http', 'signature': 'GET /desktop', 'quote': '桌面证据原话', 'context': '桌面隔离上下文'}),
    ('file.change', {'id': 'file', 'capabilityId': 'cap', 'path': 'src/desktop.ts', 'action': 'create', 'stats': {'add': 1, 'del': 0}}),
    ('call.edge', {'from': 'api', 'to': 'file', 'kind': 'calls'}),
    ('task.done', {'summary': '完成'}),
]


class StepError(ValueError):
    pass


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def sha256_file(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_files(root):
    text = (ROOT / 'scripts' / 'release-source.ps1').read_text(encoding='utf-8-sig')
    def listing(name):
        match = re.search(r'\$' + name + r'=@\(([^\n]*)\)', text)
        if not match:
            raise StepError('source_policy_missing')
        return re.findall(r"'([^']+)'", match.group(1))
    fixed, optional = listing('fixed'), listing('optional')
    directories, extensions = listing('directories'), listing('extensions')
    root = Path(root)
    info = root.lstat()
    if not stat.S_ISDIR(info.st_mode) or getattr(info, 'st_file_attributes', 0) & 0x400:
        raise StepError('invalid_source_root')
    result = []
    for name in fixed + optional:
        path = root / name
        if path.exists():
            item = path.lstat()
            if not stat.S_ISREG(item.st_mode) or item.st_nlink != 1 or getattr(item, 'st_file_attributes', 0) & 0x400:
                raise StepError('invalid_source_file')
            result.append(name)
        elif name not in optional:
            raise StepError('missing_source_file')
    for name in directories:
        directory = root / name
        if not directory.exists():
            if name in ('public', 'protocol', 'src-tauri/capabilities'):
                continue
            raise StepError('missing_source_directory')
        item = directory.lstat()
        if not stat.S_ISDIR(item.st_mode) or getattr(item, 'st_file_attributes', 0) & 0x400:
            raise StepError('invalid_source_directory')
        for current, dirs, files in os.walk(directory, followlinks=False):
            for child in dirs:
                entry = (Path(current) / child).lstat()
                if not stat.S_ISDIR(entry.st_mode) or getattr(entry, 'st_file_attributes', 0) & 0x400 or child.startswith('.'):
                    raise StepError('invalid_source_directory')
            for child in files:
                path = Path(current) / child
                entry = path.lstat()
                if not stat.S_ISREG(entry.st_mode) or entry.st_nlink != 1 or getattr(entry, 'st_file_attributes', 0) & 0x400:
                    raise StepError('invalid_source_file')
                relative = path.relative_to(root).as_posix()
                if child.startswith('.') or Path(child).suffix.lower() not in extensions or re.search(
                        r'(?i)(^|[._-])(secret|credential|private|token|apikey|api_key)([._-]|$)', child):
                    raise StepError('unapproved_source_file')
                result.append(relative)
    return sorted(result)


def source_digest(root):
    root = Path(root)
    digest = hashlib.sha256()
    for name in source_files(root):
        path = root / name
        before = path.lstat()
        data = path.read_bytes()
        after = path.lstat()
        if (before.st_nlink != 1 or before.st_size != len(data) or (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
                != (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns)):
            raise StepError('source_changed')
        digest.update((name + '\0' + str(len(data)) + '\0').encode())
        digest.update(data)
    return digest.hexdigest()


def copy_verified_source(stage, destination):
    stage, destination = Path(stage), Path(destination)
    if not stage.is_dir() or getattr(stage.lstat(), 'st_file_attributes', 0) & 0x400 or destination.exists():
        raise StepError('invalid_isolated_source')
    def ignore(current, names):
        current = Path(current)
        skipped = set()
        if current == stage and 'node_modules' not in names:
            raise StepError('node_modules_missing')
        if current == stage / 'src-tauri' and 'target' in names:
            skipped.add('target')
        return skipped
    shutil.copytree(stage, destination, ignore=ignore)
    fixtures = ROOT / 'src-tauri' / 'tests' / 'fixtures'
    if not fixtures.is_dir():
        raise StepError('test_fixtures_missing')
    shutil.copytree(fixtures, destination / 'src-tauri' / 'tests' / 'fixtures')
    for name in ('package.json', 'package-lock.json', 'src-tauri/Cargo.toml', 'src-tauri/Cargo.lock',
                 'src-tauri/resources/producer/mindtrail-producer.exe', 'node_modules/vite/bin/vite.js',
                 'src-tauri/tests/fixtures/semantic_tls/test-ca.der'):
        item = destination / name
        if not item.is_file() or getattr(item.lstat(), 'st_file_attributes', 0) & 0x400:
            raise StepError('isolated_source_incomplete')


def base_env():
    env = {key: os.environ[key] for key in ('SystemRoot', 'SystemDrive', 'WINDIR', 'PATH', 'ProgramFiles', 'ProgramFiles(x86)', 'TEMP', 'TMP', 'PROCESSOR_ARCHITECTURE') if key in os.environ}
    env['CARGO_HOME'] = str(ROOT / '.tools' / 'cargo')
    env['RUSTUP_HOME'] = str(ROOT / '.tools' / 'rustup')
    env['TEMP'] = env['TMP'] = str(ROOT / '.tools' / 'productization-desktop-temp')
    Path(env['TEMP']).mkdir(parents=True, exist_ok=True)
    return env


def run_bounded(argv, *, cwd, env, timeout, log_path):
    with open(log_path, 'wb') as log:
        result = subprocess.run(argv, cwd=cwd, env=env, stdout=log, stderr=subprocess.STDOUT, timeout=timeout)
    return result.returncode


def build_fixture(manifest, run_dir, env, source_root=None):
    manifest = Path(manifest)
    source_root = Path(source_root) if source_root is not None else manifest.parent.parent
    cargo = ROOT / '.tools' / 'cargo' / 'bin' / 'cargo.exe'
    if not cargo.is_file():
        raise StepError('cargo_missing')
    frontend = run_bounded(['npm.cmd', 'run', 'build'], cwd=source_root, env=env, timeout=300,
                           log_path=run_dir / 'frontend-build.log')
    if frontend:
        raise StepError('frontend_build_failed')
    dist = source_root / 'dist'
    index = dist / 'index.html'
    if not index.is_file():
        raise StepError('frontend_dist_missing')
    command = [str(cargo), 'test', '--manifest-path', str(manifest), '--locked', '--offline',
               '--lib', '--no-run', '--message-format=json']
    build_env = dict(env)
    build_env['CARGO_TARGET_DIR'] = str(Path(run_dir) / 'target')
    with open(run_dir / 'cargo-build.log', 'wb') as log:
        build = subprocess.run(command, cwd=source_root, env=build_env, stdout=subprocess.PIPE, stderr=log, timeout=1200)
    if build.returncode:
        raise StepError('cargo_build_failed')
    artifacts = []
    for line in build.stdout.decode('utf-8', 'replace').splitlines():
        try:
            item = json.loads(line)
        except ValueError:
            continue
        if (item.get('reason') == 'compiler-artifact' and item.get('profile', {}).get('test')
                and item.get('target', {}).get('name') in ('mindtrail_lib', 'mindtrail')
                and item.get('executable')):
            artifacts.append(item['executable'])
    if len(artifacts) != 1:
        raise StepError('fixture_artifact_ambiguous')
    executable = Path(artifacts[0])
    if not executable.is_absolute() or not executable.is_file():
        raise StepError('fixture_artifact_invalid')
    producer = source_root / 'src-tauri' / 'resources' / 'producer' / 'mindtrail-producer.exe'
    if not producer.is_file() or getattr(producer.lstat(), 'st_file_attributes', 0) & 0x400:
        raise StepError('producer_resource_missing')
    staged = executable.parent / 'producer' / 'mindtrail-producer.exe'
    staged.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(producer, staged)
    producer_digest = sha256_file(producer)
    if sha256_file(staged) != producer_digest:
        raise StepError('producer_stage_mismatch')
    return executable, sha256_file(index), producer_digest


class CdpPage:
    def __init__(self, url):
        from websockets.sync.client import connect
        self.ws = connect(url, open_timeout=10, proxy=None)
        self.seq = 0
        self.ws.send(json.dumps({'id': 0, 'method': 'Runtime.runIfWaitingForDebugger'}))

    def evaluate(self, expression):
        self.seq += 1
        self.ws.send(json.dumps({'id': self.seq, 'method': 'Runtime.evaluate', 'params': {'expression': expression, 'returnByValue': True, 'awaitPromise': True}}))
        deadline = time.monotonic() + 10
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise StepError('webview_timeout')
            message = json.loads(self.ws.recv(timeout=remaining))
            if message.get('id') == self.seq:
                result = message.get('result', {})
                if 'error' in message or 'exceptionDetails' in result:
                    raise StepError('webview_evaluation')
                return result.get('result', {}).get('value')

    def close(self):
        self.ws.close()


def wait_for(fn, process, timeout=30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise StepError('fixture_exited')
        try:
            value = fn()
        except (OSError, ValueError, StepError):
            value = None
        if value:
            return value
        time.sleep(0.1)
    raise StepError('ui_timeout')


def click(page, expression):
    for attempt in range(20):
        if page.evaluate(f'(() => {{ const el = {expression}; if (!el) return false; el.click(); return true }})()'):
            return
        if attempt < 19:
            time.sleep(0.1)
    raise StepError('ui_click_failed')


def graph(page):
    return page.evaluate("""(() => { const el = document.querySelector('.canvas'); return {nodes: [...el.querySelectorAll('[data-node-id]')].map(n => n.dataset.nodeId).sort(), edges: [...el.querySelectorAll('.react-flow__edge')].map(e => ({source:e.getAttribute('data-trail-from'), target:e.getAttribute('data-trail-to'), kind:e.getAttribute('data-trail-kind')}))} })()""")


def http_json(port, token, path, payload=None, method=None):
    data = None if payload is None else json.dumps(payload).encode()
    request = urllib.request.Request(f'http://127.0.0.1:{port}{path}', data=data, method=method or ('POST' if data else 'GET'),
                                     headers={'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'})
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(request, timeout=10) as response:
        return response.status


class Fixture:
    def __init__(self, executable, root, webview, debug_port, env, ai_env=None):
        self.root = root
        self.webview = webview
        webview.mkdir(parents=True, exist_ok=True)
        for stale in ('stop', 'exit-requested', 'shutdown-result'):
            (root / stale).unlink(missing_ok=True)
        (root / 'fixture-marker').write_text(root.name.split('-')[-1], encoding='ascii')
        desktop_env = dict(env)
        desktop_env.update(
            MINDTRAIL_UI_FIXTURE=root.name.split('-')[-1],
            MINDTRAIL_UI_DEV_URL='http://localhost:1420/',
            MINDTRAIL_UI_TIMEOUT_SECS='600',
            WEBVIEW2_USER_DATA_FOLDER=str(webview),
            WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=f'--remote-debugging-port={debug_port} --remote-allow-origins=*',
        )
        for name in ('MINDTRAIL_AI_BASE_URL', 'MINDTRAIL_AI_MODEL', 'MINDTRAIL_AI_API_KEY'):
            desktop_env.pop(name, None)
        if ai_env:
            desktop_env.update(ai_env)
        self.debug_port = debug_port
        self.job = OwnedJob([str(executable), '--exact', 'app_tests::desktop_ui_isolated_fixture', '--nocapture'],
                            cwd=root, env=desktop_env, stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def endpoint(self, timeout=45):
        path = self.root / 'state' / 'endpoint.json'
        return wait_for(lambda: json.loads(path.read_text(encoding='utf8')) if path.exists() else None,
                        self.job.process, timeout)

    def page(self, timeout=45):
        url = 'http://localhost:1420/'
        wait_for(lambda: urllib.request.urlopen(f'http://127.0.0.1:{self.debug_port}/json/version', timeout=1).status == 200,
                 self.job.process, timeout)
        def target():
            with urllib.request.urlopen(f'http://127.0.0.1:{self.debug_port}/json/list', timeout=1) as response:
                items = json.loads(response.read(1048577))
            found = [item for item in items if item.get('type') == 'page' and item.get('url', '').startswith(url)]
            return found[0] if len(found) == 1 else None
        item = wait_for(target, self.job.process, timeout)
        page = CdpPage(item['webSocketDebuggerUrl'])
        wait_for(lambda: page.evaluate('Boolean(window.__TAURI_INTERNALS__)'), self.job.process, 20)
        return page

    def close(self):
        (self.root / 'stop').write_text('stop', encoding='ascii')
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline and self.job.process.poll() is None:
            time.sleep(0.1)
        self.job.close()
        if (self.job.process.returncode != 0 or not (self.root / 'exit-requested').is_file()
                or (self.root / 'shutdown-result').read_text(encoding='utf-8') != 'exited'
                or (self.root / 'state' / 'endpoint.json').exists()):
            raise StepError('desktop_shutdown_failed')


class Preview:
    def __init__(self, port, env, run_dir, source_root=None):
        root = Path(source_root) if source_root is not None else ROOT
        self.marker_value = secrets.token_hex(16)
        self.marker = root / 'dist' / ('desktop-marker-' + self.marker_value + '.txt')
        self.marker.write_text(self.marker_value, encoding='ascii')
        log = open(run_dir / 'vite.log', 'wb')
        self.job = OwnedJob(['node', str(root / 'node_modules' / 'vite' / 'bin' / 'vite.js'),
                             'preview', '--host', '127.0.0.1', '--port', str(port), '--strictPort'],
                            cwd=root, env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT)
        self.log = log
        self.port = port
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            if self.job.process.poll() is not None:
                raise StepError('vite_exited')
            try:
                with opener.open(f'http://127.0.0.1:{port}/{self.marker.name}', timeout=1) as response:
                    if response.read(100).decode('ascii') == self.marker_value and self.job.owns_listener(port):
                        return
            except (OSError, ValueError):
                pass
            time.sleep(0.1)
        raise StepError('vite_timeout')

    def close(self):
        self.job.close()
        self.log.close()
        self.marker.unlink(missing_ok=True)


class MockProvider:
    """Loopback OpenAI-compatible server for the test-only opt-in path."""

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_POST(self):
            length = int(self.headers.get('Content-Length', '0'))
            body = self.rfile.read(min(max(length, 0), 1024 * 1024))
            provider = self.server.provider
            with provider.lock:
                provider.request_count += 1
                provider.request_bytes.append(len(body))
                mode = provider.mode
            if mode == 'slow':
                time.sleep(4)
            content = json.dumps({'sections': [{
                'title': '桌面解读',
                'text': '事实支持桌面证据。',
                'citations': [{'kind': 'quote', 'session_id': 'desktop', 'node_id': 'api', 'id': 'desktop:3'}],
                'related_node_ids': [],
            }]}, ensure_ascii=False)
            if mode == 'error':
                self.send_response(500)
                self.send_header('Content-Length', '0')
                self.end_headers()
                return
            payload = json.dumps({'choices': [{'finish_reason': 'stop', 'message': {
                'role': 'assistant', 'content': content}}]}, ensure_ascii=False).encode()
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(payload)))
            self.end_headers()
            try:
                self.wfile.write(payload)
            except (ConnectionAbortedError, ConnectionResetError, BrokenPipeError):
                pass

    def __init__(self):
        self.mode = 'normal'
        self.lock = threading.Lock()
        self.request_count = 0
        self.request_bytes = []
        self.httpd = http.server.ThreadingHTTPServer(('127.0.0.1', 0), self.Handler)
        self.httpd.daemon_threads = True
        self.httpd.provider = self
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)
        self.thread.start()
        self.stopped = False

    @property
    def url(self):
        return 'http://127.0.0.1:' + str(self.httpd.server_address[1]) + '/v1'

    def stop(self):
        if not self.stopped:
            self.stopped = True
            self.httpd.shutdown()
            self.httpd.server_close()
            self.thread.join(10)


def driver(manifest, source_root, run_dir, steps, source_evidence=None):
    env = base_env()
    executable, dist_digest, producer_digest = build_fixture(manifest, run_dir, env, source_root)
    steps.append({'name': 'build_fixture', 'status': 'passed',
                  'evidence': {'sha256': sha256_file(executable), 'dist_sha256': dist_digest,
                               'producer_sha256': producer_digest, **(source_evidence or {})}, 'limitation': ''})
    suffix = secrets.token_hex(16)
    fixture_root = Path(manifest).parent / ('MindTrailAppEvent-' + suffix)
    webview = fixture_root / 'webview'
    debug_port = free_port()
    preview = Preview(1420, env, run_dir, source_root)
    provider = None
    first = second = None
    try:
        first = Fixture(executable, fixture_root, webview, debug_port, env)
        endpoint = first.endpoint()
        page = first.page()
        try:
            wait_for(lambda: page.evaluate('document.body.innerText.includes("暂无真实任务")'), first.job.process, 30)
            steps.append({'name': 'empty_state', 'status': 'passed', 'evidence': {'tasks': 0}, 'limitation': ''})
            click(page, "[...document.querySelectorAll('button')].find(x => x.textContent.trim() === '连接帮助')")
            wait_for(lambda: page.evaluate('Boolean(document.querySelector(`[aria-label="连接帮助"]`))'), first.job.process, 20)
            wait_for(lambda: page.evaluate('!document.querySelector(`[aria-label="连接帮助"]`).innerText.includes("正在检查连接程序")'), first.job.process, 20)
            text = page.evaluate('document.querySelector(`[aria-label="连接帮助"]`).innerText')
            listed, hint_copy = '程序：' in text, '复制配置' in text
            if not listed or not hint_copy:
                raise StepError('connection_help_incomplete')
            steps.append({'name': 'connection_help', 'status': 'passed',
                          'evidence': {'producer_listed': True, 'producer_sha256': producer_digest,
                                       'path_staged': 'deps/producer/mindtrail-producer.exe', 'hint_copy': True},
                          'limitation': 'fixture stages the verified packaged producer at the runtime resource path'})
            health = http_json(endpoint['port'], endpoint['token'], '/v1/health')
            if health != 200:
                raise StepError('gateway_health_failed')
            body = [{'v': 1, 'session': 'desktop', 'seq': i + 1, 'ts': '2024-01-01T00:00:00Z', 'type': kind, 'payload': payload}
                    for i, (kind, payload) in enumerate(EVENTS)]
            if http_json(endpoint['port'], endpoint['token'], '/v1/events', body) != 200:
                raise StepError('event_post_failed')
            steps.append({'name': 'synthetic_events', 'status': 'passed', 'evidence': {'events': len(body)}, 'limitation': ''})
            task = "[...document.querySelectorAll('main.connection-home > button')].find(x => x.textContent.includes('桌面验收'))"
            wait_for(lambda: page.evaluate(f'Boolean({task})'), first.job.process, 45)
            click(page, task)
            wait_for(lambda: page.evaluate("document.querySelector('.task-list button.active')?.textContent.includes('已完成 · 6')"), first.job.process, 30)
            wait_for(lambda: len(graph(page)['edges']) == 3, first.job.process, 30)
            hierarchy = graph(page)
            if hierarchy['nodes'] != ['api', 'cap', 'file', 'root'] or {(e['source'], e['target'], e['kind']) for e in hierarchy['edges']} != {
                    ('root', 'cap', 'contains'), ('cap', 'api', 'contains'), ('cap', 'file', 'contains')}:
                raise StepError('hierarchy_mismatch')
            steps.append({'name': 'hierarchy_graph', 'status': 'passed', 'evidence': {'nodes': 4, 'edges': 3}, 'limitation': ''})
            click(page, "document.querySelector('[data-node-id=api]')")
            facts = page.evaluate('document.querySelector(".facts").innerText')
            if '桌面证据原话' not in facts or '桌面隔离上下文' not in facts:
                raise StepError('facts_missing')
            steps.append({'name': 'facts', 'status': 'passed', 'evidence': {'quote': True, 'context': True}, 'limitation': ''})
            click(page, "[...document.querySelectorAll('.segmented button')].find(x => x.textContent.includes('调用视图'))")
            wait_for(lambda: len(graph(page)['edges']) == 1, first.job.process, 20)
            calls = graph(page)
            if calls['nodes'] != ['api', 'file'] or {(e['source'], e['target'], e['kind']) for e in calls['edges']} != {('api', 'file', 'calls')}:
                raise StepError('calls_mismatch')
            steps.append({'name': 'calls_graph', 'status': 'passed', 'evidence': {'nodes': 2, 'edges': 1}, 'limitation': ''})
            append_items = [{'v': 1, 'session': 'desktop-append', 'operation_id': f'op-{i + 1}', 'ts': '2024-01-01T00:00:00Z', 'type': kind,
                             'payload': {k: ({'cap': 'cap2', 'api': 'api2', 'file': 'file2'}.get(v, v) if isinstance(v, str) else v) for k, v in payload.items()}}
                            for i, (kind, payload) in enumerate(EVENTS)]
            append_items[0]['payload']['title'] = '追加验收'
            append_items[2]['payload'].update(quote='追加原话', context='追加隔离上下文')
            if http_json(endpoint['port'], endpoint['token'], '/v1/append', append_items[:1]) != 200:
                raise StepError('append_failed')
            append_task = "[...document.querySelectorAll('.task-list button')].find(x => x.textContent.includes('追加验收'))"
            wait_for(lambda: page.evaluate(f'Boolean({append_task})'), first.job.process, 30)
            click(page, append_task)
            wait_for(lambda: page.evaluate(f'({append_task})?.textContent.includes("运行中 · 1")'), first.job.process, 30)
            click(page, "[...document.querySelectorAll('.segmented button')].find(x => x.textContent.includes('层级视图'))")
            if http_json(endpoint['port'], endpoint['token'], '/v1/append', append_items[1:]) != 200:
                raise StepError('append_failed')
            wait_for(lambda: page.evaluate(f'({append_task})?.textContent.includes("已完成 · 6")'), first.job.process, 30)
            wait_for(lambda: len(graph(page)['edges']) == 3, first.job.process, 20)
            replay = graph(page)
            if replay['nodes'] != ['api2', 'cap2', 'file2', 'root']:
                raise StepError('replay_mismatch')
            steps.append({'name': 'task_replay', 'status': 'passed', 'evidence': {'nodes': 4, 'edges': 3}, 'limitation': ''})
            click(page, "document.querySelector('[data-node-id=api2]')")
            click(page, "[...document.querySelectorAll('.facts button')].find(x => x.textContent.trim() === '打开解读')")
            wait_for(lambda: page.evaluate('Boolean(document.querySelector(`[aria-label="节点解读"]`))'), first.job.process, 20)
            wait_for(lambda: page.evaluate('document.querySelector(`[aria-label="节点解读"]`).innerText.includes("尚未配置 AI 服务。")'), first.job.process, 20)
            steps.append({'name': 'ai_not_configured', 'status': 'passed', 'evidence': {'message': 'not_configured'}, 'limitation': ''})
            page.evaluate('document.querySelector(`[aria-label="关闭解读"]`).click()')
        finally:
            page.close()
        first.close()
        first = None
        provider = MockProvider()
        second = Fixture(executable, fixture_root, webview, debug_port, env, ai_env={
            'MINDTRAIL_AI_BASE_URL': provider.url,
            'MINDTRAIL_AI_MODEL': 'mock-model',
            'MINDTRAIL_AI_API_KEY': 'mock-key',
            'MINDTRAIL_AI_ALLOW_LOOPBACK': '1',
        })
        page2 = None
        try:
            again = second.endpoint()
            stable = again.get('store_id') == endpoint.get('store_id') and again.get('token') != endpoint.get('token')
            steps.append({'name': 'restart_identity', 'status': 'passed' if stable else 'failed',
                          'evidence': {'store_stable': again.get('store_id') == endpoint.get('store_id'), 'token_rotated': again.get('token') != endpoint.get('token')}, 'limitation': ''})
            page2 = second.page()
            task2 = "[...document.querySelectorAll('main.connection-home > button')].find(x => x.textContent.includes('桌面验收'))"
            wait_for(lambda: page2.evaluate(f'Boolean({task2})'), second.job.process, 45)
            click(page2, task2)
            wait_for(lambda: page2.evaluate("Boolean(document.querySelector('[data-node-id=api]'))"), second.job.process, 30)
            click(page2, "document.querySelector('[data-node-id=api]')")
            click(page2, "[...document.querySelectorAll('.facts button')].find(x => x.textContent.trim() === '打开解读')")
            wait_for(lambda: page2.evaluate('document.querySelector(`[aria-label="AI 解读"]`)?.innerText.includes("准备解读")'), second.job.process, 20)
            click(page2, "[...document.querySelectorAll('[aria-label=\"AI 解读\"] button')].find(x => x.textContent.trim() === '准备解读')")
            wait_for(lambda: page2.evaluate('document.querySelector(`[aria-label="AI 解读"]`).innerText.includes("发送预览")'), second.job.process, 20)
            preview_text = page2.evaluate('document.querySelector(`[aria-label="AI 解读"]`).innerText')
            if 'openai_compatible / mock-model' not in preview_text or '1 条原话' not in preview_text:
                raise StepError('preview_scope_mismatch')
            click(page2, "[...document.querySelectorAll('[aria-label=\"AI 解读\"] button')].find(x => x.textContent.trim() === '发送并生成')")
            wait_for(lambda: page2.evaluate('document.querySelector(`[aria-label="AI 解读"]`).innerText.includes("AI 解读 · openai_compatible/mock-model")'), second.job.process, 30)
            result_text = page2.evaluate('document.querySelector(`[aria-label="AI 解读"]`).innerText')
            if '桌面解读' not in result_text or '原话' not in result_text:
                raise StepError('mock_generation_missing_citation')
            steps.append({'name': 'mock_generation', 'status': 'passed',
                          'evidence': {'requests': provider.request_count, 'citation': True, 'sections': 1}, 'limitation': ''})
            provider.mode = 'slow'
            click(page2, "document.querySelector('[data-node-id=file]')")
            click(page2, "[...document.querySelectorAll('.facts button')].find(x => x.textContent.trim() === '打开解读')")
            wait_for(lambda: page2.evaluate('document.querySelector(`[aria-label="AI 解读"]`)?.innerText.includes("准备解读")'), second.job.process, 20)
            click(page2, "[...document.querySelectorAll('[aria-label=\"AI 解读\"] button')].find(x => x.textContent.trim() === '准备解读')")
            wait_for(lambda: page2.evaluate('document.querySelector(`[aria-label="AI 解读"]`).innerText.includes("发送预览")'), second.job.process, 20)
            click(page2, "[...document.querySelectorAll('[aria-label=\"AI 解读\"] button')].find(x => x.textContent.trim() === '发送并生成')")
            wait_for(lambda: page2.evaluate('document.querySelector(`[aria-label="AI 解读"]`).innerText.includes("正在生成…")'), second.job.process, 20)
            click(page2, "[...document.querySelectorAll('[aria-label=\"AI 解读\"] button')].find(x => x.textContent.trim() === '取消生成')")
            wait_for(lambda: page2.evaluate('document.querySelector(`[aria-label="AI 解读"]`)?.innerText.includes("准备解读")'), second.job.process, 20)
            click(page2, "[...document.querySelectorAll('[aria-label=\"AI 解读\"] button')].find(x => x.textContent.trim() === '准备解读')")
            wait_for(lambda: page2.evaluate('document.querySelector(`[aria-label="AI 解读"]`).innerText.includes("发送预览")'), second.job.process, 20)
            click(page2, "[...document.querySelectorAll('[aria-label=\"AI 解读\"] button')].find(x => x.textContent.trim() === '发送并生成')")
            wait_for(lambda: page2.evaluate('document.querySelector(`[aria-label="AI 解读"]`).innerText.includes("正在生成…")'), second.job.process, 20)
            click(page2, "document.querySelector('[data-node-id=cap]')")
            click(page2, "[...document.querySelectorAll('.facts button')].find(x => x.textContent.trim() === '打开解读')")
            wait_for(lambda: page2.evaluate('document.querySelector(`[aria-label="节点解读"] .insight-header`)?.innerText.includes("能力")'), second.job.process, 20)
            time.sleep(5)
            if page2.evaluate('document.querySelector(`[aria-label="AI 解读"]`)?.innerText.includes("AI 解读 · ") ?? false'):
                raise StepError('stale_generation_displayed')
            steps.append({'name': 'cancel_switch', 'status': 'passed',
                          'evidence': {'cancelled': True, 'switch_discarded': True, 'requests': provider.request_count},
                          'limitation': ''})
            provider.mode = 'normal'
            provider.stop()
            wait_for(lambda: page2.evaluate('document.querySelector(`[aria-label="AI 解读"]`)?.innerText.includes("准备解读")'), second.job.process, 20)
            click(page2, "[...document.querySelectorAll('[aria-label=\"AI 解读\"] button')].find(x => x.textContent.trim() === '准备解读')")
            wait_for(lambda: page2.evaluate('document.querySelector(`[aria-label="AI 解读"]`).innerText.includes("发送预览")'), second.job.process, 20)
            click(page2, "[...document.querySelectorAll('[aria-label=\"AI 解读\"] button')].find(x => x.textContent.trim() === '发送并生成')")
            wait_for(lambda: page2.evaluate('document.querySelector(`[aria-label="AI 解读"]`).innerText.includes("无法连接 AI 服务，请重试。")'), second.job.process, 45)
            steps.append({'name': 'offline_error', 'status': 'passed', 'evidence': {'message': 'transport'}, 'limitation': ''})
            page2.close()
            page2 = None
            second.close()
            second = None
        finally:
            if page2 is not None:
                page2.close()
            if second is not None:
                second.close()
        steps.append({'name': 'shutdown', 'status': 'passed', 'evidence': {'exit_requested': True, 'shutdown': 'exited', 'discovery_withdrawn': True}, 'limitation': ''})
    finally:
        for item in (first, second):
            if item is not None:
                try:
                    item.close()
                except (OSError, StepError, subprocess.SubprocessError):
                    pass
        if provider is not None:
            provider.stop()
        preview.close()


def run(manifest, report_dir, source_run=None):
    if report_dir.exists():
        raise StepError('report_exists')
    manifest = Path(manifest).resolve()
    expected = (ROOT / 'src-tauri' / 'Cargo.toml').resolve()
    if manifest != expected or not manifest.is_file():
        raise StepError('manifest_must_be_current_source')
    run_dir = ROOT / '.tools' / 'productization-desktop' / ('run-' + secrets.token_hex(16))
    run_dir.mkdir(parents=True, exist_ok=False)
    source_root, isolated_manifest, source_evidence = ROOT, manifest, None
    if source_run is not None:
        if not re.fullmatch(r'[0-9a-f]{32}', source_run):
            raise StepError('invalid_source_run')
        release = ROOT / '.tools' / 'productization-release' / ('run-' + source_run)
        owner = release / 'owner'
        if (release.is_symlink() or not release.is_dir() or (release / '.release-quarantine').exists()
                or not owner.is_file() or owner.read_text(encoding='ascii') != source_run):
            raise StepError('invalid_source_run')
        record = release / 'source-inputs.json'
        info = record.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or getattr(info, 'st_file_attributes', 0) & 0x400:
            raise StepError('invalid_source_run')
        recorded = json.loads(record.read_text(encoding='utf8'))
        current_digest = source_digest(ROOT)
        stage_digest = source_digest(release / 'source')
        if current_digest != stage_digest or recorded.get('source_digest') != current_digest:
            raise StepError('source_changed')
        copy_verified_source(release / 'source', run_dir / 'source')
        if source_digest(run_dir / 'source') != current_digest:
            raise StepError('isolated_source_changed')
        source_root = run_dir / 'source'
        isolated_manifest = source_root / 'src-tauri' / 'Cargo.toml'
        source_evidence = {'isolated_source': True, 'source_digest': current_digest, 'release_run': source_run}
    steps = []
    result = 'passed'
    try:
        driver(isolated_manifest, source_root, run_dir, steps, source_evidence)
        done = {step['name'] for step in steps}
        missing = set(STEPS) - done
        if missing:
            raise StepError('missing_steps')
    except BaseException:
        result = 'failed'
        raise
    finally:
        report = {'schema': 1, 'kind': 'productization_desktop', 'manifest': {'path': manifest.name, 'sha256': sha256_file(manifest)},
                  'run': run_dir.name, 'steps': steps, 'result': result,
                  'limitations': ['mock provider is a loopback HTTP fixture enabled only in the cfg(test) binary',
                                  'real HTTPS provider and installation are separate acceptance stages']}
        validate_report(report)
        report_dir.mkdir(parents=True, exist_ok=False)
        (report_dir / 'result.json').write_text(json.dumps(report, indent=2, sort_keys=True), encoding='utf8')


def validate_report(report):
    if not isinstance(report, dict) or set(report) != {'schema', 'kind', 'manifest', 'run', 'steps', 'result', 'limitations'}:
        raise StepError('invalid_report')
    if report['schema'] != 1 or report['kind'] != 'productization_desktop' or report['result'] not in ('passed', 'failed'):
        raise StepError('invalid_report')
    if not isinstance(report['steps'], list) or any(
            not isinstance(step, dict) or set(step) != {'name', 'status', 'evidence', 'limitation'}
            or step['status'] not in ('passed', 'failed', 'not_run', 'blocked') for step in report['steps']):
        raise StepError('invalid_report')
    if any(step['name'] not in STEPS for step in report['steps']) or len({step['name'] for step in report['steps']}) != len(report['steps']):
        raise StepError('invalid_report')
    if report['result'] == 'passed':
        if {step['name'] for step in report['steps']} != set(STEPS):
            raise StepError('missing_steps')
        if any(step['status'] != 'passed' for step in report['steps']):
            raise StepError('failed_step')
    return True


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument('--manifest', required=True)
    parser.add_argument('--report', required=True)
    parser.add_argument('--source-run', help='verified release run used as an isolated source copy')
    args = parser.parse_args(argv)
    try:
        run(args.manifest, Path(args.report), args.source_run)
    except StepError as error:
        print('productization_desktop=' + str(error))
        return 1
    except subprocess.TimeoutExpired:
        print('productization_desktop=timeout')
        return 1
    except Exception:
        print('productization_desktop=failed')
        return 1
    print('productization_desktop=passed')
    return 0


if __name__ == '__main__':
    sys.exit(main())
