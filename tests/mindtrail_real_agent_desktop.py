"""Read-only checks for an isolated workbench snapshot."""
import json
import hashlib
import os
from pathlib import Path
import queue
import re
import shutil
import socket
import subprocess
import threading
import time
import urllib.request
import urllib.parse
import uuid

from mindtrail_real_agent_job import OwnedJob


def parse_endpoint(raw):
    if not isinstance(raw, bytes) or len(raw) > 4096:
        raise ValueError('invalid_endpoint')
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ValueError('invalid_endpoint')
            result[key] = value
        return result
    try:
        data = json.loads(raw, object_pairs_hook=pairs)
    except (ValueError, UnicodeError):
        raise ValueError('invalid_endpoint') from None
    if (not isinstance(data, dict) or set(data) != {'port', 'token', 'version', 'store_id'}
            or type(data['port']) is not int or not 1 <= data['port'] <= 65535
            or type(data['version']) is not int or data['version'] != 1
            or not isinstance(data['store_id'], str) or not re.fullmatch('[0-9a-f]{64}', data['store_id'])
            or not isinstance(data['token'], str) or not re.fullmatch('[0-9a-f]{64}', data['token'])):
        raise ValueError('invalid_endpoint')
    return data


class PreviewProcess:
    def __init__(self, repo, node):
        from mindtrail_real_agent import safe_existing_path
        repo, node = Path(repo), Path(node)
        dist, vite = repo / 'dist', repo / 'node_modules' / 'vite' / 'bin' / 'vite.js'
        if not safe_existing_path(str(repo), directory=True) or not safe_existing_path(str(dist), directory=True) or not safe_existing_path(str(node), directory=False) or not safe_existing_path(str(vite), directory=False):
            raise ValueError('invalid_preview')
        marker = uuid.uuid4().hex
        self.marker_value = marker
        self.marker = dist / ('mindtrail-agent-' + marker + '.txt')
        with self.marker.open('x', encoding='ascii') as stream:
            stream.write(marker)
        env = {key: os.environ[key] for key in ('SystemRoot', 'WINDIR', 'PATH', 'TEMP', 'TMP') if key in os.environ}
        self.job = None
        try:
            self.job = OwnedJob([str(node), str(vite), 'preview', '--host', '127.0.0.1', '--port', '1420', '--strictPort'],
                                cwd=repo, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
            lines = queue.Queue(maxsize=64)
            proc = self.job.process
            def read_lines():
                while True:
                    try:
                        line = proc.stdout.readline(4097)
                    except (OSError, ValueError):
                        return
                    if not line:
                        return
                    try:
                        lines.put_nowait(line)
                    except queue.Full:
                        return
            reader = threading.Thread(target=read_lines, daemon=True)
            reader.start()
            from mindtrail_real_agent import NoRedirect
            opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
            deadline = time.monotonic() + 45
            bound = False
            while time.monotonic() < deadline:
                if self.job.process.poll() is not None:
                    raise ValueError('preview_exited')
                try:
                    line = re.sub(rb'\x1b\[[0-9;]*m', b'', lines.get_nowait())
                    bound = bound or b'Local:' in line
                except queue.Empty:
                    pass
                if not bound:
                    time.sleep(.1)
                    continue
                try:
                    with opener.open('http://127.0.0.1:1420/' + self.marker.name, timeout=1) as response:
                        if response.read(100).decode('ascii') == marker and self.job.process.poll() is None and self.job.owns_listener(1420):
                            return
                except (OSError, ValueError):
                    pass
                time.sleep(.1)
            raise ValueError('preview_timeout')
        except BaseException:
            try:
                self.close()
            except (OSError, ValueError, subprocess.SubprocessError):
                pass
            raise

    def close(self):
        try:
            if self.job:
                self.job.close()
                self.job = None
        finally:
            if self.marker.is_symlink() or (self.marker.exists() and self.marker.read_text(encoding='ascii') != self.marker_value):
                raise ValueError('unsafe_preview_marker')
            self.marker.unlink(missing_ok=True)


_DESKTOP_BUILD_AUTH = object()


class DesktopArtifact:
    def __init__(self, executable, digest, key):
        if key is not _DESKTOP_BUILD_AUTH:
            raise ValueError('invalid_desktop_artifact')
        self.executable = Path(executable)
        self.digest = digest


class DesktopProcess:
    """An owned fixture window; the caller must verify its preview and exe."""
    def __init__(self, artifact, run, repo, *, debug_port):
        from mindtrail_real_agent import safe_existing_path
        if not isinstance(artifact, DesktopArtifact):
            raise ValueError('invalid_desktop_artifact')
        executable = artifact.executable
        repo = Path(repo).resolve(strict=True)
        if not safe_existing_path(str(executable), directory=False):
            raise ValueError('invalid_desktop_artifact')
        fixture, state = Path(run['fixture']), Path(run['state'])
        ident = run['id']
        webview = Path(run['webview'])
        expected = repo / '.tools' / 'mcp-stage0-copy' / 'src-tauri' / ('MindTrailAppEvent-' + ident)
        if (not re.fullmatch('[0-9a-f]{32}', ident) or fixture != expected
                or not executable.resolve(strict=True).is_relative_to((repo / '.tools/producer-stage0-target').resolve(strict=True))
                or hashlib.sha256(executable.read_bytes()).hexdigest() != artifact.digest
                or not safe_existing_path(str(fixture), directory=True)
                or state != fixture / 'state'
                or (state.exists() and not safe_existing_path(str(state), directory=True)) or state.is_symlink()
                or not safe_existing_path(str(fixture / 'fixture-marker'), directory=False)
                or (fixture / 'fixture-marker').read_text(encoding='ascii') != ident
                or not safe_existing_path(str(fixture / 'owner'), directory=False)
                or (fixture / 'owner').read_text(encoding='ascii') != ident
                or webview != fixture / 'webview' or not safe_existing_path(str(webview), directory=True)
                or type(debug_port) is not int or not 1 <= debug_port <= 65535):
            raise ValueError('invalid_desktop_fixture')
        if (state / 'endpoint.json').exists():
            raise ValueError('desktop_already_running')
        for name in ('stop', 'exit-requested', 'shutdown-result'):
            path = fixture / name
            if path.is_symlink() or (path.exists() and not safe_existing_path(str(path), directory=False)):
                raise ValueError('unsafe_desktop_marker')
            path.unlink(missing_ok=True)
        env = {key: os.environ[key] for key in ('SystemRoot', 'WINDIR', 'PATH', 'TEMP', 'TMP') if key in os.environ}
        env.update(MINDTRAIL_UI_FIXTURE=ident, MINDTRAIL_UI_TIMEOUT_SECS='600',
                   WEBVIEW2_USER_DATA_FOLDER=str(run['webview']),
                   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=f'--remote-debugging-port={debug_port} --remote-allow-origins=*')
        self.fixture = fixture
        self.state = state
        self.job = OwnedJob([str(executable), '--exact', 'app_tests::desktop_ui_isolated_fixture', '--nocapture'],
                            cwd=fixture, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def endpoint(self, timeout=45):
        from mindtrail_real_agent import safe_existing_path
        deadline = time.monotonic() + timeout
        path = self.state / 'endpoint.json'
        while time.monotonic() < deadline:
            if self.job.process.poll() is not None:
                raise ValueError('desktop_exited')
            if path.exists():
                if not safe_existing_path(str(path), directory=False):
                    raise ValueError('unsafe_endpoint')
                with path.open('rb') as stream:
                    return parse_endpoint(stream.read(4097))
            time.sleep(.1)
        raise ValueError('desktop_timeout')

    def close(self):
        try:
            with (self.fixture / 'stop').open('x', encoding='ascii') as marker:
                marker.write('stop')
            self.job.process.wait(timeout=25)
            if (self.job.process.returncode != 0 or not (self.fixture / 'exit-requested').is_file()
                    or (self.fixture / 'shutdown-result').read_text(encoding='utf-8') != 'exited'
                    or (self.state / 'endpoint.json').exists()):
                raise ValueError('desktop_shutdown_failed')
        finally:
            self.job.close()


class Workbench:
    """Read one local fixture WebView; never expose the discovery token."""
    def __init__(self, debug_port, job, timeout=45):
        from websockets.sync.client import connect
        from mindtrail_real_agent import NoRedirect
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
        deadline = time.monotonic() + timeout
        target = None
        while time.monotonic() < deadline:
            if job.process.poll() is not None:
                raise ValueError('desktop_exited')
            if not job.owns_listener(debug_port):
                time.sleep(.1)
                continue
            try:
                with opener.open(f'http://127.0.0.1:{debug_port}/json/list', timeout=1) as response:
                    raw = response.read(1048577)
                if len(raw) > 1048576:
                    raise ValueError('debug_response_limit')
                data = json.loads(raw)
                candidates = [item for item in data if item.get('type') == 'page'
                              and item.get('url', '').startswith('http://localhost:1420/')]
                if len(candidates) == 1:
                    target = candidates[0]; break
            except (OSError, json.JSONDecodeError):
                pass
            time.sleep(.1)
        if target is None:
            raise ValueError('webview_timeout')
        ws = urllib.parse.urlsplit(target.get('webSocketDebuggerUrl', ''))
        if ws.scheme != 'ws' or ws.hostname not in ('127.0.0.1', 'localhost') or ws.port != debug_port or not ws.path.startswith('/devtools/page/') or ws.username or ws.password:
            raise ValueError('invalid_debug_target')
        self.ws = connect(ws.geturl(), open_timeout=10, proxy=None)
        self.index = 0
        self.deadline = None
        self.ws.send(json.dumps({'id': 0, 'method': 'Runtime.runIfWaitingForDebugger'}))
        try:
            self.wait(lambda: self.evaluate('Boolean(window.__TAURI_INTERNALS__)'), job.process, timeout)
        except BaseException:
            self.close()
            raise

    def evaluate(self, expression):
        self.index += 1
        self.ws.send(json.dumps({'id': self.index, 'method': 'Runtime.evaluate', 'params': {'expression': expression, 'returnByValue': True, 'awaitPromise': True}}))
        while True:
            remaining = 10 if self.deadline is None else min(10, self.deadline - time.monotonic())
            if remaining <= 0:
                raise ValueError('webview_timeout')
            message = json.loads(self.ws.recv(timeout=remaining))
            if message.get('id') == self.index:
                result = message.get('result', {})
                if 'error' in message or 'exceptionDetails' in result:
                    raise ValueError('webview_evaluation')
                return result.get('result', {}).get('value')

    def wait(self, fn, process, timeout=45):
        deadline = time.monotonic() + timeout
        if self.deadline is not None:
            deadline = min(deadline, self.deadline)
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise ValueError('desktop_exited')
            value = fn()
            if value:
                return value
            time.sleep(.1)
        raise ValueError('webview_timeout')

    def close(self):
        self.ws.close()

    def select_task(self, title, status, process, evidence=None):
        title_js = json.dumps(title, ensure_ascii=True)
        expression = f"[...document.querySelectorAll('.task-list button')].filter(x=>x.textContent.includes({title_js}))"
        if evidence is not None: evidence['stage_ui_find_started'] = True
        self.wait(lambda: self.evaluate(f'{expression}.length===1'), process)
        if evidence is not None: evidence['stage_ui_find_done'] = True
        self.wait(lambda: self.evaluate(f'{expression}[0].textContent.includes({json.dumps(status)})'), process)
        if evidence is not None: evidence['stage_ui_status_done'] = True
        if not self.evaluate(f'(()=>{{{expression}[0].click();return true}})()'):
            raise ValueError('task_select_failed')
        if evidence is not None: evidence['stage_ui_click_done'] = True

    def graph(self):
        return self.evaluate("""(() => { const e=document.querySelector('.canvas');return {nodes:[...e.querySelectorAll('[data-node-id]')].map(n=>n.dataset.nodeId),edges:[...e.querySelectorAll('.react-flow__edge')].map(x=>({source:x.getAttribute('data-trail-from'),target:x.getAttribute('data-trail-to'),kind:x.getAttribute('data-trail-kind')}))} })()""")

    def expect_prefix(self, title, capability, process, evidence=None):
        self.select_task(title, '运行中 · 2', process, evidence=evidence)
        selector = json.dumps(capability)
        if evidence is not None: evidence['stage_ui_node_started'] = True
        self.wait(lambda: self.evaluate(f'Boolean(document.querySelector(`[data-node-id="${{CSS.escape({selector})}}"] em.planned`))'), process)
        if evidence is not None: evidence['stage_ui_node_done'] = True

    def expect_complete(self, title, capability, api, file, process, evidence=None):
        if evidence is not None: evidence['stage_complete_select_started'] = True
        self.select_task(title, '已完成 · 7', process)
        if evidence is not None: evidence['stage_complete_select_done'] = True
        self.wait(lambda: len(self.graph()['edges']) == 3, process)
        if evidence is not None: evidence['stage_complete_hierarchy_done'] = True
        hierarchy = self.graph()
        node = json.dumps(capability)
        self.wait(lambda: self.evaluate(f'Boolean(document.querySelector(`[data-node-id="${{CSS.escape({node})}}"] em.done`))'), process)
        if evidence is not None: evidence['stage_complete_node_done'] = True
        if not self.evaluate(f'(()=>{{document.querySelector(`[data-node-id="${{CSS.escape({node})}}"]`).click();return true}})()'):
            raise ValueError('node_select_failed')
        self.wait(lambda: self.evaluate("document.querySelector('.facts')?.innerText.includes('命令入口调用 double') && document.querySelector('.facts')?.innerText.includes('已用正负输入验证输出')"), process)
        if evidence is not None: evidence['stage_complete_facts_done'] = True
        self.evaluate("(()=>{[...document.querySelectorAll('.segmented button')].find(x=>x.textContent.includes('调用视图')).click();return true})()")
        self.wait(lambda: len(self.graph()['edges']) == 1, process)
        if evidence is not None: evidence['stage_complete_calls_done'] = True
        calls = self.graph()
        validate_graphs(hierarchy, calls, capability, api, file)
        return True


def build_desktop(repo):
    """Build the current isolated desktop target and bind its digest."""
    from mindtrail_real_agent import bounded_stdout, select_desktop_artifact
    repo = Path(repo).resolve(strict=True)
    tools = repo / '.tools'
    copy = tools / 'mcp-stage0-copy' / 'src-tauri'
    target = tools / 'producer-stage0-target'
    cargo = tools / 'cargo' / 'bin' / 'cargo.exe'
    node = shutil.which('node.exe')
    if not node:
        raise ValueError('node_missing')
    env = {key: os.environ[key] for key in ('SystemRoot', 'SystemDrive', 'WINDIR', 'PATH', 'TEMP', 'TMP') if key in os.environ}
    env.update(CARGO_HOME=str(tools / 'cargo'), RUSTUP_HOME=str(tools / 'rustup'))
    bounded_stdout([node, str(repo / 'node_modules/typescript/bin/tsc'), '--noEmit'], cwd=repo, env=env, timeout=120, limit=1048576)
    bounded_stdout([node, str(repo / 'node_modules/vite/bin/vite.js'), 'build'], cwd=repo, env=env, timeout=120, limit=1048576)
    output = bounded_stdout([str(cargo), 'test', '--offline', '--locked', '--manifest-path', str(copy / 'Cargo.toml'),
                             '--target-dir', str(target), '--features', 'producer-test-fixture', '--lib', '--no-run', '--message-format=json'],
                            cwd=repo, env=env, timeout=600, limit=32 * 1024 * 1024)
    executable = select_desktop_artifact(output.decode('utf-8').splitlines(), target)
    return DesktopArtifact(executable, hashlib.sha256(executable.read_bytes()).hexdigest(), _DESKTOP_BUILD_AUTH)


def local_desktop_probe(repo):
    """Build this source copy and exercise only an empty isolated desktop."""
    from mindtrail_real_agent import register_run
    repo = Path(repo).resolve(strict=True)
    tools = repo / '.tools'
    copy = tools / 'mcp-stage0-copy' / 'src-tauri'
    node = shutil.which('node.exe')
    if not node:
        raise ValueError('node_missing')
    artifact = build_desktop(repo)
    run = register_run(tools, copy)
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        debug_port = sock.getsockname()[1]
    preview = PreviewProcess(repo, Path(node))
    try:
        first = DesktopProcess(artifact, run, repo, debug_port=debug_port)
        try:
            before = first.endpoint()
            page = Workbench(debug_port, first.job)
            try:
                page.wait(lambda: page.evaluate('Boolean(document.querySelector("#root")?.children.length)'), first.job.process)
            finally:
                page.close()
        finally:
            first.close()
        second = DesktopProcess(artifact, run, repo, debug_port=debug_port)
        try:
            after = second.endpoint()
            if before.get('store_id') != after.get('store_id') or before.get('token') == after.get('token'):
                raise ValueError('desktop_identity_mismatch')
        finally:
            second.close()
    finally:
        preview.close()
    return True


def validate_graphs(hierarchy, calls, capability, api, file):
    def edges(graph):
        return {(edge['source'], edge['target'], edge['kind']) for edge in graph['edges']}
    if len({capability, api, file, 'root'}) != 4:
        raise ValueError('duplicate_node')
    if (set(hierarchy['nodes']) != {'root', capability, api, file}
            or len(hierarchy['nodes']) != 4
            or len(hierarchy['edges']) != 3
            or edges(hierarchy) != {('root', capability, 'contains'), (capability, api, 'contains'), (capability, file, 'contains')}):
        raise ValueError('hierarchy_mismatch')
    if (set(calls['nodes']) != {api, file} or len(calls['nodes']) != 2
            or len(calls['edges']) != 1 or edges(calls) != {(file, api, 'calls')}):
        raise ValueError('calls_mismatch')
    return True
