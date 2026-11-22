import json, os, secrets, socket, subprocess, time, urllib.error, urllib.request
from pathlib import Path
from websockets.sync.client import connect

base = Path(__file__).resolve().parents[1]
manifest = Path(os.environ.get('MINDTRAIL_TEST_MANIFEST', base / 'src-tauri' / 'Cargo.toml')).resolve()
suffix = secrets.token_hex(16)
root = manifest.parent / ('MindTrailAppEvent-' + suffix)
root.mkdir(); marker = secrets.token_hex(32); (root / 'fixture-marker').write_text(marker)
print('isolated fixture:', root, flush=True)

def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0)); return sock.getsockname()[1]

def wait_for(fn, description, proc=None, timeout=30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if proc and proc.poll() is not None: raise RuntimeError(f'{description}: process exited {proc.returncode}')
        try:
            value = fn()
            if value: return value
        except (OSError, ValueError): pass
        time.sleep(.1)
    raise TimeoutError(f'{description}: timed out')

env = os.environ.copy()
env['CARGO_HOME'] = str(base / '.tools' / 'cargo'); env['RUSTUP_HOME'] = str(base / '.tools' / 'rustup')
cargo = base / '.tools' / 'cargo' / 'bin' / 'cargo.exe'
command = [str(cargo), 'test', '--manifest-path', str(manifest), '--locked', '--offline', '--lib', '--no-run', '--message-format=json']
build = subprocess.run(command, cwd=base, env=env, capture_output=True, text=True, encoding='utf-8', errors='replace')
(root/'cargo.log').write_text(build.stderr, encoding='utf-8')
if build.returncode: raise RuntimeError(f'Cargo build failed; see {root / "cargo.log"}')
artifacts = [o for line in build.stdout.splitlines() if (o := json.loads(line)).get('reason') == 'compiler-artifact' and o.get('profile', {}).get('test') and o.get('target', {}).get('name') == 'mindtrail_lib' and o.get('executable')]
if len(artifacts) != 1: raise RuntimeError(f'Expected one current test executable, got {len(artifacts)}')
exe = artifacts[0]['executable']
port, debug_port = 1420, free_port()
while port == debug_port: debug_port = free_port()
url = f'http://localhost:{port}/'
env.update(MINDTRAIL_UI_FIXTURE=suffix, MINDTRAIL_UI_DEV_URL=url, WEBVIEW2_USER_DATA_FOLDER=str(root/'webview'), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=f'--remote-debugging-port={debug_port} --remote-allow-origins=*')
with (root/'frontend-build.log').open('w', encoding='utf-8') as frontend_log:
    frontend = subprocess.run(['npm.cmd', 'run', 'build'], cwd=base, stdout=frontend_log, stderr=subprocess.STDOUT, timeout=120)
if frontend.returncode: raise RuntimeError(f'Frontend build failed; see {root / "frontend-build.log"}')
vite_marker = base / 'dist' / ('mindtrail-test-' + suffix + '.txt'); vite_marker.write_text(marker)
vite_log = (root/'vite.log').open('w', encoding='utf-8')
vite = subprocess.Popen(['node', str(base/'node_modules'/'vite'/'bin'/'vite.js'), 'preview', '--host', '127.0.0.1', '--port', str(port), '--strictPort'], cwd=base, stdout=vite_log, stderr=subprocess.STDOUT)
events = [
 ('task.start', {'title':'桌面真实验证','goal':'验证重放','project':'isolated'}),
 ('capability.intent', {'id':'cap','title':'能力'}),
 ('api.expose', {'id':'api','capabilityId':'cap','kind':'http','signature':'GET /actual','quote':'桌面真实证据','context':'隔离上下文'}),
 ('file.change', {'id':'file','capabilityId':'cap','path':'src/actual.ts','action':'create','stats':{'add':1,'del':0}}),
 ('call.edge', {'from':'api','to':'file','kind':'calls'}),
 ('task.done', {'summary':'完成'})]
body = json.dumps([{'v':1,'session':'actual','seq':i+1,'ts':'2024-01-01T00:00:00Z','type':kind,'payload':payload} for i,(kind,payload) in enumerate(events)]).encode()
append_events = [(kind, {key: ({'cap':'cap2','api':'api2','file':'file2'}.get(value, value) if isinstance(value, str) else value) for key,value in {**payload, **({'title':'追加真实验证'} if kind == 'task.start' else {}), **({'quote':'追加独有证据','context':'追加上下文'} if kind == 'api.expose' else {})}.items()}) for kind,payload in events]
append_items = [{'v':1,'session':'actual-append','operation_id':f'op-{i+1}','ts':'2024-01-01T00:00:00Z','type':kind,'payload':payload} for i,(kind,payload) in enumerate(append_events)]
append_body = json.dumps(append_items[:1]).encode()
append_rest = json.dumps(append_items[1:]).encode()

class Page:
    def __init__(self, url):
        self.ws = connect(url, open_timeout=10); self.seq = 0
        self.ws.send(json.dumps({'id':0,'method':'Runtime.runIfWaitingForDebugger'}))
    def evaluate(self, expression):
        self.seq += 1
        self.ws.send(json.dumps({'id':self.seq,'method':'Runtime.evaluate','params':{'expression':expression,'returnByValue':True,'awaitPromise':True}}))
        while True:
            result = json.loads(self.ws.recv(timeout=10))
            if result.get('id') == self.seq:
                if 'error' in result or 'exceptionDetails' in result.get('result', {}): raise RuntimeError(f'WebView evaluation failed: {result}')
                return result['result']['result'].get('value')
    def close(self): self.ws.close()

def graph(page):
    return page.evaluate("""(() => { const el = document.querySelector('.canvas'); return {nodes: [...el.querySelectorAll('[data-node-id]')].map(n => n.dataset.nodeId).sort(), edges: [...el.querySelectorAll('.react-flow__edge')].map(e => ({id:e.getAttribute('data-id'), source:e.getAttribute('data-trail-from'), target:e.getAttribute('data-trail-to'), kind:e.getAttribute('data-trail-kind'), text:e.textContent.trim()})).sort((a,b) => (a.id || '').localeCompare(b.id || ''))} })()""")

def click(page, expression):
    assert page.evaluate(f'(() => {{ const el = {expression}; if (!el) return false; el.click(); return true }})()')

def run_once(first, old_token=None, occupied_port=None):
    stop = root/'stop'; stop.unlink(missing_ok=True); (root/'shutdown-result').unlink(missing_ok=True); (root/'exit-requested').unlink(missing_ok=True)
    with (root/('first.log' if first else 'second.log')).open('w',encoding='utf-8') as log:
        proc = subprocess.Popen([exe,'--exact','app_tests::desktop_ui_isolated_fixture','--nocapture'],env=env,stdout=log,stderr=subprocess.STDOUT)
        failure = None; result = None
        try:
            endpoint = wait_for(lambda: json.loads((root/'state'/'endpoint.json').read_text(encoding='utf-8')), 'gateway startup', proc)
            if first: assert endpoint['port'] == 7391, 'preferred port 7391 was unavailable'
            else: assert occupied_port == 7391 and endpoint['port'] == 7392, 'expected fallback 7391 to 7392'
            print(f"gateway {'first' if first else 'restarted'} port: {endpoint['port']}", flush=True)
            health = f"http://127.0.0.1:{endpoint['port']}/v1/health"
            with urllib.request.urlopen(urllib.request.Request(health, headers={'Authorization':'Bearer '+endpoint['token']}), timeout=5) as response: assert response.status == 200
            if old_token:
                try: urllib.request.urlopen(urllib.request.Request(health, headers={'Authorization':'Bearer '+old_token}), timeout=5); raise AssertionError('old token accepted')
                except urllib.error.HTTPError as error: assert error.code == 401
            if first:
                req = urllib.request.Request(f"http://127.0.0.1:{endpoint['port']}/v1/events",data=body,headers={'Authorization':'Bearer '+endpoint['token'],'Content-Type':'application/json'},method='POST')
                with urllib.request.urlopen(req,timeout=5) as response: assert response.status == 200
                req = urllib.request.Request(f"http://127.0.0.1:{endpoint['port']}/v1/append",data=append_body,headers={'Authorization':'Bearer '+endpoint['token'],'Content-Type':'application/json'},method='POST')
                with urllib.request.urlopen(req,timeout=5) as response: assert response.status == 200
            wait_for(lambda: urllib.request.urlopen(f'http://127.0.0.1:{debug_port}/json/version',timeout=1).status == 200, 'WebView CDP startup', proc)
            def find_target():
                targets = json.load(urllib.request.urlopen(f'http://127.0.0.1:{debug_port}/json/list',timeout=1))
                return next((t for t in targets if t.get('type') == 'page' and t.get('url','').startswith(url)),None)
            target = wait_for(find_target, 'isolated WebView navigation', proc)
            page = Page(target['webSocketDebuggerUrl'])
            wait_for(lambda: page.evaluate('Boolean(window.__TAURI_INTERNALS__)'), 'Tauri bridge initialization', proc, 20)
            task_expr = "[...document.querySelectorAll('.task-list button')].find(x => x.textContent.includes('桌面真实验证'))"
            try: wait_for(lambda: page.evaluate(f'Boolean({task_expr})'), 'task list render', proc, 60)
            except TimeoutError:
                print('WebView diagnostic:', page.evaluate("JSON.stringify({url:location.href, body:document.body.innerText, html:document.documentElement.outerHTML.slice(0,2000), resources:performance.getEntriesByType('resource').map(x=>({name:x.name,duration:x.duration})), ready:document.readyState})"), flush=True); raise
            click(page, task_expr)
            wait_for(lambda: page.evaluate(f"({task_expr})?.textContent.includes('已完成 · 6')"), 'task replay', proc, 30)
            click(page, "document.querySelector('[data-node-id=api]')")
            facts = page.evaluate("document.querySelector('.facts').innerText")
            assert '桌面真实证据' in facts and '隔离上下文' in facts, facts
            wait_for(lambda: len(graph(page)['edges']) == 3, 'hierarchy edges render', proc, 20)
            hierarchy = graph(page)
            assert hierarchy['nodes'] == ['api','cap','file','root'], hierarchy
            assert {(edge['source'],edge['target'],edge['kind']) for edge in hierarchy['edges']} == {('root','cap','contains'),('cap','api','contains'),('cap','file','contains')}, hierarchy
            click(page, "[...document.querySelectorAll('.segmented button')].find(x => x.textContent.includes('调用视图'))")
            wait_for(lambda: page.evaluate("Boolean(document.querySelector('[data-node-id=file]'))"), 'call graph render', proc)
            wait_for(lambda: len(graph(page)['edges']) == 1, 'call edge render', proc, 20)
            calls = graph(page)
            assert calls['nodes'] == ['api','file'], calls
            assert len(calls['edges']) == 1 and calls['edges'][0]['id'] == 'call:5' and (calls['edges'][0]['source'],calls['edges'][0]['target'],calls['edges'][0]['kind']) == ('api','file','calls') and '调用' in calls['edges'][0]['text'], calls
            original_status = page.evaluate(f'({task_expr}).innerText')
            append_expr = "[...document.querySelectorAll('.task-list button')].find(x => x.textContent.includes('追加真实验证'))"
            wait_for(lambda: page.evaluate(f'Boolean({append_expr})'), 'append task listing', proc, 20)
            click(page, append_expr)
            if first:
                wait_for(lambda: page.evaluate(f"({append_expr})?.textContent.includes('运行中 · 1')"), 'initial append selection', proc, 30)
                wait_for(lambda: page.evaluate("document.querySelector('.badge')?.textContent === '真实任务' && document.querySelector('[data-node-id=root]')?.textContent.includes('追加真实验证') && document.querySelector('.task-list button.active')?.textContent.includes('追加真实验证')"), 'selected append prefix rendered', proc, 20)
                assert graph(page)['nodes'] == ['root']
                req = urllib.request.Request(f"http://127.0.0.1:{endpoint['port']}/v1/append",data=append_rest,headers={'Authorization':'Bearer '+endpoint['token'],'Content-Type':'application/json'},method='POST')
                with urllib.request.urlopen(req,timeout=5) as response: assert response.status == 200
            wait_for(lambda: page.evaluate(f"({append_expr})?.textContent.includes('已完成 · 6')"), 'append task replay', proc, 30)
            wait_for(lambda: page.evaluate("Boolean(document.querySelector('[data-node-id=api2]'))"), 'append hierarchy render', proc, 20)
            wait_for(lambda: len(graph(page)['edges']) == 3, 'append hierarchy edges', proc, 20)
            append_hierarchy = graph(page)
            assert append_hierarchy['nodes'] == ['api2','cap2','file2','root'], append_hierarchy
            assert {(e['source'],e['target'],e['kind']) for e in append_hierarchy['edges']} == {('root','cap2','contains'),('cap2','api2','contains'),('cap2','file2','contains')}, append_hierarchy
            click(page, "document.querySelector('[data-node-id=api2]')")
            appended_facts = page.evaluate("document.querySelector('.facts').innerText")
            assert '追加独有证据' in appended_facts and '追加上下文' in appended_facts, appended_facts
            click(page, "[...document.querySelectorAll('.segmented button')].find(x => x.textContent.includes('调用视图'))")
            wait_for(lambda: len(graph(page)['edges']) == 1, 'append call edge', proc, 20)
            append_calls = graph(page)
            assert append_calls['nodes'] == ['api2','file2'], append_calls
            assert {(e['source'],e['target'],e['kind']) for e in append_calls['edges']} == {('api2','file2','calls')}, append_calls
            result = (hierarchy, calls, facts, original_status, append_hierarchy, append_calls, appended_facts, page.evaluate(f'({append_expr}).innerText'))
            page.close()
        except BaseException as error:
            failure = error
        finally:
            stop.write_text('stop')
            try: proc.wait(timeout=25)
            except subprocess.TimeoutExpired:
                proc.kill(); proc.wait(); print('window close timed out; process terminated',flush=True)
                if failure is None: failure = RuntimeError('window close timed out')
            if proc.returncode != 0 and failure is None: failure = RuntimeError(f'fixture exited {proc.returncode}; see {log.name}')
            if failure is None and (not (root/'exit-requested').is_file() or (root/'shutdown-result').read_text() != 'exited' or (root/'state'/'endpoint.json').exists()): failure = RuntimeError('production exit or discovery withdrawal failed')
        if failure: raise failure
        return result, endpoint['store_id'], endpoint['token'], endpoint['port']
try:
    wait_for(lambda: 'Local:' in (root/'vite.log').read_text(encoding='utf-8'), 'dedicated Vite bind', vite)
    marker_url = url + vite_marker.name
    wait_for(lambda: urllib.request.urlopen(marker_url,timeout=2).read().decode() == marker, 'dedicated Vite ownership', vite)
    if vite.poll() is not None: raise RuntimeError('dedicated Vite exited during ownership verification')
    before, store1, token1, first_port = run_once(True)
    with socket.socket() as occupied:
        occupied.bind(('127.0.0.1',first_port)); occupied.listen(1)
        after, store2, token2, second_port = run_once(False,token1,first_port)
    assert before == after and store1 == store2 and token1 != token2 and first_port != second_port
    print('PASS desktop explicit and append HTTP, port fallback, both graphs, evidence, production close, old token rejection and restart replay',flush=True)
finally:
    vite.terminate()
    try: vite.wait(timeout=10)
    except subprocess.TimeoutExpired: vite.kill(); vite.wait()
    vite_log.close()
    vite_marker.unlink(missing_ok=True)













