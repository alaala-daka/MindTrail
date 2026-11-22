"""Bounded live semantic acceptance; one request, no retry, no raw content.

Without --live, or without the three explicitly configured environment
variables, this tool records a blocked result and sends no network request.
"""
import argparse
import hashlib
import http.client
import http.server
import importlib.util
import json
import os
from pathlib import Path
import re
import sys
import threading
import time
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
PROMPT_VERSION = 'mindtrail-insight-v1'
REPORT_KEYS = {'schema', 'kind', 'manifest', 'provider', 'model', 'endpoint_origin',
               'prompt_version', 'input_counts', 'prefix_seq', 'request_count',
               'result_category', 'citations_valid', 'desktop_display', 'checks', 'limitations'}
CHECK_KEYS = {'config_accepted', 'single_request', 'timeout_30s', 'no_retry',
              'citations_same_prefix', 'facts_unchanged', 'task_state_unchanged'}
RESULTS = {'passed', 'unauthorized', 'rate_limited', 'timeout', 'transport',
           'provider_unavailable', 'invalid_output', 'invalid_reference',
           'insufficient_evidence', 'blocked_missing_credentials', 'live_not_requested',
           'blocked_missing_dependency', 'harness_failure'}


def sha256_file(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def gateway_events_bytes(port, token, session='live'):
    request = urllib.request.Request(f'http://127.0.0.1:{port}/v1/sessions/{urllib.parse.quote(session, safe="")}/events',
                                     headers={'Authorization': 'Bearer ' + token})
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(request, timeout=10) as response:
        if response.status != 200:
            raise ValueError('events_read_failed')
        return response.read()


class CountingProxy:
    """Loopback relay that forwards the single generation to the configured HTTPS provider."""

    def __init__(self, upstream):
        parts = urllib.parse.urlsplit(upstream)
        self.upstream = upstream
        self.secure = parts.scheme == 'https'
        self.host = parts.hostname
        self.port = parts.port or (443 if self.secure else 80)
        self.requests = []
        self.lock = threading.Lock()
        proxy = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = 'HTTP/1.1'

            def log_message(self, *_args):
                pass

            def do_POST(self):
                length = int(self.headers.get('Content-Length', '0'))
                body = self.rfile.read(min(max(length, 0), 512 * 1024))
                started = time.monotonic()
                with proxy.lock:
                    proxy.requests.append({'started': started, 'status': None, 'duration': None})
                    entry = proxy.requests[-1]
                status, payload, content_type = 502, b'', 'application/json'
                connection = None
                try:
                    cls = http.client.HTTPSConnection if proxy.secure else http.client.HTTPConnection
                    connection = cls(proxy.host, proxy.port, timeout=35)
                    headers = {key: value for key, value in self.headers.items()
                               if key.lower() in ('authorization', 'content-type', 'accept')}
                    connection.request('POST', self.path, body=body, headers=headers)
                    response = connection.getresponse()
                    status = response.status
                    content_type = response.getheader('Content-Type', 'application/json')
                    payload = response.read(128 * 1024 + 1)
                except Exception:
                    pass
                finally:
                    if connection is not None:
                        connection.close()
                    with proxy.lock:
                        entry['status'] = status
                        entry['duration'] = time.monotonic() - started
                self.send_response(status)
                self.send_header('Content-Type', content_type)
                self.send_header('Content-Length', str(len(payload)))
                self.send_header('Connection', 'close')
                self.end_headers()
                self.close_connection = True
                try:
                    self.wfile.write(payload)
                except (ConnectionAbortedError, ConnectionResetError, BrokenPipeError):
                    pass

        self.httpd = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.httpd.daemon_threads = True
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)
        self.thread.start()

    @property
    def url(self):
        path = urllib.parse.urlsplit(self.upstream).path.rstrip('/')
        return f'http://127.0.0.1:{self.httpd.server_address[1]}{path}'

    def close(self):
        self.httpd.shutdown()
        self.httpd.server_close()
        self.thread.join(10)


def validate_origin(raw):
    if not isinstance(raw, str) or not raw or len(raw) > 2048 or any(c.isspace() or c == '\\' or ord(c) < 32 for c in raw):
        raise ValueError('invalid_endpoint')
    parts = urllib.parse.urlsplit(raw)
    if (parts.scheme != 'https' or not parts.hostname or parts.username or parts.password
            or parts.query or parts.fragment or parts.port not in (None, 443)):
        raise ValueError('invalid_endpoint')
    return raw.rstrip('/')


def credentials(environ):
    base, model, key = (environ.get('MINDTRAIL_AI_BASE_URL'), environ.get('MINDTRAIL_AI_MODEL'), environ.get('MINDTRAIL_AI_API_KEY'))
    if not base and not model and not key:
        return None, 'missing'
    if not base or not model or not key:
        return None, 'incomplete'
    if len(model) > 128 or len(key) > 4096 or any(c in key for c in '\r\n'):
        return None, 'invalid'
    try:
        base = validate_origin(base)
    except ValueError:
        return None, 'invalid'
    parts = urllib.parse.urlsplit(base)
    return {'base_url': base, 'model': model, 'key': key,
            'endpoint_origin': f'{parts.scheme}://{parts.netloc}'}, None


def validate_report(report):
    if not isinstance(report, dict) or set(report) != REPORT_KEYS:
        raise ValueError('invalid_report')
    if report['schema'] != 1 or report['kind'] != 'semantic_live':
        raise ValueError('invalid_report')
    if not isinstance(report['manifest'], dict) or set(report['manifest']) != {'path', 'sha256'}:
        raise ValueError('invalid_report')
    if not isinstance(report['checks'], dict) or set(report['checks']) != CHECK_KEYS:
        raise ValueError('invalid_report')
    if any(report['checks'][name] not in ('passed', 'failed', 'not_run') for name in CHECK_KEYS):
        raise ValueError('invalid_report')
    if report['result_category'] not in RESULTS:
        raise ValueError('invalid_report')
    if report['result_category'] == 'passed':
        if any(report['checks'][name] != 'passed' for name in CHECK_KEYS):
            raise ValueError('invalid_report')
        if report['request_count'] != 1 or not isinstance(report['prefix_seq'], int):
            raise ValueError('invalid_report')
        if report['citations_valid'] is not True or report['desktop_display'] is not True:
            raise ValueError('invalid_report')
    for name in ('provider', 'model', 'endpoint_origin', 'prompt_version'):
        if report[name] is not None and not isinstance(report[name], str):
            raise ValueError('invalid_report')
    if not isinstance(report['input_counts'], dict) or set(report['input_counts']) != {'events', 'evidence'}:
        raise ValueError('invalid_report')
    for name in ('prefix_seq', 'request_count'):
        if report[name] is not None and type(report[name]) is not int:
            raise ValueError('invalid_report')
    for name in ('citations_valid', 'desktop_display'):
        if report[name] is not None and not isinstance(report[name], bool):
            raise ValueError('invalid_report')
    if not isinstance(report['limitations'], list) or len(report['limitations']) > 8 or any(not isinstance(item, str) or len(item) > 300 for item in report['limitations']):
        raise ValueError('invalid_report')
    return True


def write_report(report_dir, report):
    validate_report(report)
    report_dir.mkdir(parents=True, exist_ok=False)
    (report_dir / 'result.json').write_text(json.dumps(report, indent=2, sort_keys=True), encoding='utf8')


def base_report(manifest, category, limitations):
    return {
        'schema': 1, 'kind': 'semantic_live',
        'manifest': {'path': manifest.name, 'sha256': sha256_file(manifest)},
        'provider': None, 'model': None, 'endpoint_origin': None, 'prompt_version': None,
        'input_counts': {'events': 0, 'evidence': 0}, 'prefix_seq': None, 'request_count': 0,
        'result_category': category, 'citations_valid': None, 'desktop_display': None,
        'checks': {name: 'not_run' for name in CHECK_KEYS}, 'limitations': limitations,
    }


def load_desktop_module():
    path = ROOT / 'tests' / 'productization-desktop.py'
    spec = importlib.util.spec_from_file_location('productization_desktop', path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def run_live(manifest, report, config):
    """Drive one real desktop generation; failures map to fixed categories."""
    desktop = load_desktop_module()
    run_dir = ROOT / '.tools' / 'semantic-live' / ('run-' + os.urandom(16).hex())
    run_dir.mkdir(parents=True, exist_ok=False)
    env = desktop.base_env()
    executable, _, _ = desktop.build_fixture(manifest, run_dir, env)
    proxy = CountingProxy(config['base_url'])
    suffix = os.urandom(16).hex()
    fixture_root = manifest.parent / ('MindTrailAppEvent-' + suffix)
    webview = fixture_root / 'webview'
    preview = None
    fixture = None
    try:
        preview = desktop.Preview(1420, env, run_dir)
        fixture = desktop.Fixture(executable, fixture_root, webview, desktop.free_port(), env, ai_env={
            'MINDTRAIL_AI_BASE_URL': proxy.url,
            'MINDTRAIL_AI_MODEL': config['model'],
            'MINDTRAIL_AI_API_KEY': config['key'],
            'MINDTRAIL_AI_ALLOW_LOOPBACK': '1',
        })
        endpoint = fixture.endpoint()
        page = fixture.page()
        try:
            events = [
                {'v': 1, 'session': 'live', 'seq': 1, 'ts': '2024-01-01T00:00:00Z', 'type': 'task.start',
                 'payload': {'title': '真实语义验收', 'goal': '一次生成', 'project': 'isolated'}},
                {'v': 1, 'session': 'live', 'seq': 2, 'ts': '2024-01-01T00:00:01Z', 'type': 'capability.intent',
                 'payload': {'id': 'cap', 'title': '能力'}},
                {'v': 1, 'session': 'live', 'seq': 3, 'ts': '2024-01-01T00:00:02Z', 'type': 'api.expose',
                 'payload': {'id': 'api', 'capabilityId': 'cap', 'kind': 'http', 'signature': 'GET /live',
                             'quote': '真实语义原话', 'context': '隔离上下文'}},
                {'v': 1, 'session': 'live', 'seq': 4, 'ts': '2024-01-01T00:00:03Z', 'type': 'file.change',
                 'payload': {'id': 'file', 'capabilityId': 'cap', 'path': 'src/live.ts', 'action': 'create',
                             'stats': {'add': 1, 'del': 0}}},
                {'v': 1, 'session': 'live', 'seq': 5, 'ts': '2024-01-01T00:00:04Z', 'type': 'task.done',
                 'payload': {'summary': '完成'}},
            ]
            if desktop.http_json(endpoint['port'], endpoint['token'], '/v1/events', events) != 200:
                raise desktop.StepError('event_post_failed')
            report['input_counts'] = {'events': 5, 'evidence': 1}
            report['prefix_seq'] = 5
            events_before = gateway_events_bytes(endpoint['port'], endpoint['token'])
            task = "[...document.querySelectorAll('main.connection-home > button')].find(x => x.textContent.includes('真实语义验收'))"
            desktop.wait_for(lambda: page.evaluate(f'Boolean({task})'), fixture.job.process, 45)
            desktop.wait_for(lambda: page.evaluate(f'({task})?.textContent.includes("已完成 · 5")'), fixture.job.process, 30)
            task_before = page.evaluate(f'({task})?.textContent')
            desktop.click(page, task)
            desktop.wait_for(lambda: len(desktop.graph(page)['edges']) == 3, fixture.job.process, 30)
            desktop.click(page, "document.querySelector('[data-node-id=api]')")
            desktop.click(page, "[...document.querySelectorAll('.facts button')].find(x => x.textContent.trim() === '打开解读')")
            drawer = 'document.querySelector(`[aria-label="节点解读"]`)'
            desktop.wait_for(lambda: page.evaluate(f'Boolean({drawer})'), fixture.job.process, 20)
            desktop.wait_for(lambda: page.evaluate(f'Boolean({drawer}?.innerText.includes("准备解读"))'), fixture.job.process, 20)
            desktop.click(page, f"[...{drawer}.querySelectorAll('button')].find(x => x.textContent.trim() === '准备解读')")
            desktop.wait_for(lambda: page.evaluate(f'Boolean({drawer}?.innerText.includes("发送预览"))'), fixture.job.process, 20)
            preview_text = page.evaluate(f'{drawer}.innerText')
            preview_payload = page.evaluate(f'JSON.parse({drawer}.querySelector("pre").textContent)')
            if not isinstance(preview_payload, dict) or not isinstance(preview_payload.get('events'), list) or not isinstance(preview_payload.get('evidence'), list):
                raise desktop.StepError('preview_payload_missing')
            counts = {'events': len(preview_payload['events']), 'evidence': len(preview_payload['evidence'])}
            expected_counts = {'events': len(events),
                               'evidence': sum(1 for event in events if event.get('payload', {}).get('quote'))}
            prefix_match = re.search(r'前缀：(\d+)', preview_text)
            if not prefix_match:
                raise desktop.StepError('preview_prefix_missing')
            prefix = int(prefix_match.group(1))
            expected_prefix = max(event['seq'] for event in events)
            if (counts != expected_counts or prefix != expected_prefix
                    or f"服务：openai_compatible / {config['model']}" not in preview_text
                    or f"{counts['events']} 条事件 · {counts['evidence']} 条原话" not in preview_text):
                raise desktop.StepError('preview_scope_mismatch')
            report['input_counts'] = counts
            report['prefix_seq'] = prefix
            report['prompt_version'] = PROMPT_VERSION
            started = time.monotonic()
            desktop.click(page, f"[...{drawer}.querySelectorAll('button')].find(x => x.textContent.trim() === '发送并生成')")
            desktop.wait_for(lambda: page.evaluate(f'Boolean({drawer}?.innerText.includes("AI 解读 · ")) || Boolean({drawer}?.innerText.match(/服务未授权|服务请求受限|请求超时|无法连接 AI 服务|服务返回格式不兼容|解读引用无效|当前范围没有足够的原话证据/))'),
                             fixture.job.process, 45)
            text = page.evaluate(f'{drawer}.innerText')
            categories = {
                '服务未授权，请检查配置。': 'unauthorized', '服务请求受限，请稍后重试。': 'rate_limited',
                '请求超时，请重试。': 'timeout', '无法连接 AI 服务，请重试。': 'transport',
                'AI 服务暂不可用，请重试。': 'provider_unavailable', '服务返回格式不兼容，请更换兼容模型后重试。': 'invalid_output',
                '解读引用无效，请重新生成。': 'invalid_reference', '当前范围没有足够的原话证据。': 'insufficient_evidence',
            }
            category = next((value for message, value in categories.items() if message in text), None)
            elapsed = time.monotonic() - started
            requests = list(proxy.requests)
            report['request_count'] = len(requests)
            report['checks']['config_accepted'] = 'passed' if len(requests) == 1 else 'failed'
            report['checks']['single_request'] = 'passed' if len(requests) == 1 else 'failed'
            report['checks']['no_retry'] = 'passed' if len(requests) == 1 else 'failed'
            report['checks']['timeout_30s'] = 'passed' if len(requests) == 1 and requests[0]['status'] is not None and elapsed <= 35 else 'failed'
            try:
                events_after = gateway_events_bytes(endpoint['port'], endpoint['token'])
                report['checks']['facts_unchanged'] = 'passed' if events_after == events_before else 'failed'
            except Exception:
                report['checks']['facts_unchanged'] = 'failed'
            task_after = page.evaluate(f'({task})?.textContent')
            report['checks']['task_state_unchanged'] = 'passed' if task_after == task_before else 'failed'
            if 'AI 解读 · ' in text:
                report['result_category'] = 'passed'
                expected_header = f"AI 解读 · openai_compatible/{config['model']} · 前缀 {report['prefix_seq']}"
                report['desktop_display'] = expected_header in text
                citation_selector = f'[...{drawer}.querySelectorAll("button")].find(x => x.textContent.trim() === "原话")'
                citation = page.evaluate(f'Boolean({citation_selector})')
                revealed = False
                if citation:
                    desktop.click(page, citation_selector)
                    try:
                        revealed = bool(desktop.wait_for(
                            lambda: page.evaluate('Boolean(document.activeElement?.dataset.evidenceKind === "quote" && document.activeElement?.querySelector("pre")?.textContent === "真实语义原话")'),
                            fixture.job.process, 20))
                    except desktop.StepError:
                        revealed = False
                report['citations_valid'] = bool(revealed)
                report['checks']['citations_same_prefix'] = 'passed' if revealed else 'failed'
                if not report['desktop_display'] or not revealed:
                    report['result_category'] = 'invalid_output'
            elif category:
                report['result_category'] = category
                report['desktop_display'] = True
                report['checks']['citations_same_prefix'] = 'not_run'
            else:
                report['result_category'] = 'invalid_output'
                report['desktop_display'] = False
            report['provider'] = 'openai_compatible'
            report['model'] = config['model']
            report['endpoint_origin'] = config['endpoint_origin']
            report['limitations'] = list(report['limitations']) + [
                'timeout_30s observes completion within 35s, not the client 30s timer',
                'single_request and no_retry share the same relay count',
            ]
            page.evaluate(f'{drawer}.querySelector(`[aria-label="关闭解读"]`).click()')
        finally:
            page.close()
        fixture.close()
        fixture = None
    finally:
        if fixture is not None:
            try:
                fixture.close()
            except Exception:
                pass
        try:
            if preview is not None:
                preview.close()
        finally:
            proxy.close()
    return report


def main(argv=None, live_runner=None):
    parser = argparse.ArgumentParser()
    parser.add_argument('--desktop-fixture-manifest', required=True)
    parser.add_argument('--report', required=True)
    parser.add_argument('--live', action='store_true')
    args = parser.parse_args(argv)
    report_dir = Path(args.report)
    if report_dir.exists():
        print('semantic_live=refused_existing_report')
        return 1
    manifest = Path(args.desktop_fixture_manifest).resolve()
    expected = (ROOT / 'src-tauri' / 'Cargo.toml').resolve()
    if manifest != expected or not manifest.is_file():
        print('semantic_live=invalid_manifest')
        return 1
    limitations = ['one request only; no automatic retry', 'raw prompt and response are never retained']
    if not args.live:
        write_report(report_dir, base_report(manifest, 'live_not_requested', limitations + ['--live not supplied']))
        print('semantic_live=live_not_requested')
        return 2
    config, reason = credentials(os.environ)
    if config is None:
        write_report(report_dir, base_report(manifest, 'blocked_missing_credentials', limitations + ['credential state: ' + reason]))
        print('semantic_live=blocked_missing_credentials')
        return 2
    report = base_report(manifest, 'blocked_missing_dependency', limitations + ['live desktop chain did not run'])
    runner = live_runner or run_live
    try:
        report = runner(manifest, report, config)
        validate_report(report)
    except Exception:
        write_report(report_dir, base_report(manifest, 'harness_failure', limitations + ['live desktop chain failed before a reportable result']))
        print('semantic_live=harness_failure')
        return 2
    write_report(report_dir, report)
    passed = report['result_category'] == 'passed' and all(value == 'passed' for value in report['checks'].values())
    print('semantic_live=' + ('passed' if passed else report['result_category']))
    return 0 if passed else 2


if __name__ == '__main__':
    sys.exit(main())
