import json, os, secrets, socket, subprocess, time, urllib.error, urllib.request
from pathlib import Path
from websockets.sync.client import connect

base=Path(__file__).resolve().parents[1]; project=base/'.tools/mcp-stage0-copy'; manifest=project/'src-tauri/Cargo.toml'; suffix=secrets.token_hex(16)
root=project/'src-tauri'/('MindTrailAppEvent-'+suffix);root.mkdir();marker=secrets.token_hex(32);(root/'fixture-marker').write_text(marker)
print('isolated fixture:',root,flush=True)
env=os.environ.copy();env['CARGO_HOME']=str(base/'.tools/cargo');env['RUSTUP_HOME']=str(base/'.tools/rustup');env['CARGO_TARGET_DIR']=str(base/'.tools/producer-stage0-target')
env.update(MINDTRAIL_UI_FIXTURE=suffix, MINDTRAIL_DESKTOP_ROOT=str(root/'state'), MINDTRAIL_DESKTOP_CONTROL=str(root), WEBVIEW2_USER_DATA_FOLDER=str(root/'webview'))
cargo=base/'.tools/cargo/bin/cargo.exe'; cmd=[str(cargo),'test','--manifest-path',str(manifest),'--locked','--offline','--features','producer-test-fixture','--lib','--test','mcp_desktop_writer','--no-run','--message-format=json']
build=subprocess.run(cmd,cwd=base,env=env,capture_output=True,text=True,encoding='utf-8',errors='replace',timeout=240);(root/'cargo.log').write_text(build.stderr,encoding='utf-8');assert build.returncode==0,'build failed; see cargo.log'
artifacts=[o for line in build.stdout.splitlines() if (o:=json.loads(line)).get('reason')=='compiler-artifact' and o.get('profile',{}).get('test') and o.get('executable')]
def target(name):
    found=[o['executable'] for o in artifacts if o['target']['name']==name];assert len(found)==1,(name,found);return found[0]
app=target('mindtrail_lib');writer=target('mcp_desktop_writer')
def wait(fn,label,proc=None,timeout=45):
    end=time.monotonic()+timeout
    while time.monotonic()<end:
        if proc and proc.poll() is not None:raise RuntimeError(f'{label}: process exited {proc.returncode}')
        try:
            x=fn()
            if x:return x
        except (OSError,ValueError):pass
        time.sleep(.1)
    raise TimeoutError(label)
def free_port():
    with socket.socket() as s:s.bind(('127.0.0.1',0));return s.getsockname()[1]
debug=free_port();env['WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS']=f'--remote-debugging-port={debug} --remote-allow-origins=*';url='http://localhost:1420/'
with (root/'frontend-build.log').open('w',encoding='utf-8') as build_log:frontend=subprocess.run(['npm.cmd','run','build'],cwd=base,stdout=build_log,stderr=subprocess.STDOUT,timeout=120)
assert frontend.returncode==0,'frontend build failed'
vite_marker=base/'dist'/('mindtrail-mcp-test-'+suffix+'.txt');vite_marker.write_text(marker);vite_log=(root/'vite.log').open('w',encoding='utf-8');vite=None;producer=None
class Page:
    def __init__(self,url):self.ws=connect(url,open_timeout=10);self.i=0;self.ws.send(json.dumps({'id':0,'method':'Runtime.runIfWaitingForDebugger'}))
    def eval(self,js):
        self.i+=1;self.ws.send(json.dumps({'id':self.i,'method':'Runtime.evaluate','params':{'expression':js,'returnByValue':True,'awaitPromise':True}}))
        while True:
            result=json.loads(self.ws.recv(timeout=10))
            if result.get('id')==self.i:
                if 'error' in result or 'exceptionDetails' in result.get('result',{}):raise RuntimeError('WebView evaluation failed')
                return result['result']['result'].get('value')
    def close(self):self.ws.close()
def graph(page):return page.eval("""(() => { const e=document.querySelector('.canvas');return {nodes:[...e.querySelectorAll('[data-node-id]')].map(n=>n.dataset.nodeId).sort(),edges:[...e.querySelectorAll('.react-flow__edge')].map(x=>({source:x.getAttribute('data-trail-from'),target:x.getAttribute('data-trail-to'),kind:x.getAttribute('data-trail-kind')})).sort((a,b)=>JSON.stringify(a).localeCompare(JSON.stringify(b)))} })()""")
def http(endpoint,path,token=None):
    request=urllib.request.Request(f"http://127.0.0.1:{endpoint['port']}{path}",headers={'Authorization':'Bearer '+(token or endpoint['token'])})
    with urllib.request.urlopen(request,timeout=5) as response:return json.load(response)
def run_app(first,old=None):
    for name in ('stop','exit-requested','shutdown-result'):(root/name).unlink(missing_ok=True)
    with (root/('first.log' if first else 'second.log')).open('w',encoding='utf-8') as log:
        proc=subprocess.Popen([app,'--exact','app_tests::desktop_ui_isolated_fixture','--nocapture'],env=env,stdout=log,stderr=subprocess.STDOUT);failure=None
        try:
            endpoint=wait(lambda:json.loads((root/'state/endpoint.json').read_text(encoding='utf-8')),'gateway',proc)
            assert endpoint['port']==(7391 if first else 7392)
            assert http(endpoint,'/v1/health')['store_id']==endpoint['store_id']
            if old:
                try:http(endpoint,'/v1/health',old);raise AssertionError('old token accepted')
                except urllib.error.HTTPError as e:assert e.code==401
            events=[];seq=0
            def prefix():
                nonlocal seq
                result=http(endpoint,f'/v1/sessions/mcp-desktop/events?since_seq={seq}&limit=1');items=result['events']
                if items:seq=items[-1]['seq'];events.extend(items)
                return seq==7
            wait(prefix,'MCP automatic recovery',proc,60);assert [x['seq'] for x in events]==list(range(1,8));assert events[2]['payload']['quote']=='中文证据' and events[2]['payload']['context']=='隔离上下文'
            target=wait(lambda:next((x for x in json.load(urllib.request.urlopen(f'http://127.0.0.1:{debug}/json/list',timeout=2)) if x.get('type')=='page' and x.get('url','').startswith(url)),None),'WebView',proc)
            page=Page(target['webSocketDebuggerUrl']);wait(lambda:page.eval('Boolean(window.__TAURI_INTERNALS__)'),'Tauri',proc)
            expr="[...document.querySelectorAll('.task-list button')].find(x=>x.textContent.includes('MCP桌面任务'))"
            try:wait(lambda:page.eval(f'Boolean({expr})'),'task row',proc,60)
            except TimeoutError:
                print('WebView diagnostic:',page.eval("JSON.stringify({url:location.href,body:document.body.innerText,ready:document.readyState,html:document.documentElement.outerHTML.slice(0,1000),resources:performance.getEntriesByType('resource').map(x=>({name:x.name,duration:x.duration}))})"),flush=True);raise
            assert page.eval(f'(()=>{{{expr}.click();return true}})()');wait(lambda:page.eval(f"({expr})?.textContent.includes('已完成 · 7')"),'done',proc)
            wait(lambda:len(graph(page)['edges'])==3,'hierarchy',proc);hier=graph(page);assert hier['nodes']==['api','cap','file','root'];assert {(e['source'],e['target'],e['kind']) for e in hier['edges']}=={('root','cap','contains'),('cap','api','contains'),('cap','file','contains')}
            assert page.eval("(()=>{document.querySelector('[data-node-id=api]').click();return true})()");facts=page.eval("document.querySelector('.facts').innerText");assert '中文证据' in facts and '隔离上下文' in facts
            assert page.eval("(()=>{[...document.querySelectorAll('.segmented button')].find(x=>x.textContent.includes('调用视图')).click();return true})()");wait(lambda:len(graph(page)['edges'])==1,'calls',proc);calls=graph(page);assert calls['nodes']==['api','file'] and {(e['source'],e['target'],e['kind']) for e in calls['edges']}=={('api','file','calls')}
            if not first:
                second="[...document.querySelectorAll('.task-list button')].find(x=>x.textContent.includes('离线第二任务'))";wait(lambda:page.eval(f"({second})?.textContent.includes('运行中 · 1')"),'second running',proc,40);assert page.eval(f'(()=>{{{second}.click();return true}})()');wait(lambda:page.eval("document.querySelector('[data-node-id=root]')?.textContent.includes('离线第二任务')"),'second root',proc);(root/'producer-second-finish').write_text('finish');wait(lambda:(root/'producer-second-done').is_file(),'explicit task.done',producer);wait(lambda:page.eval(f"({second})?.textContent.includes('已完成 · 2')"),'second done',proc,40);assert [x['type'] for x in http(endpoint,'/v1/sessions/mcp-second/events?since_seq=0&limit=2')['events']]==['task.start','task.done']
            page.close();return (hier,calls,facts,endpoint['store_id']),endpoint
        except BaseException as exc:failure=exc;raise
        finally:
            (root/'stop').write_text('stop')
            try:proc.wait(timeout=25)
            except subprocess.TimeoutExpired:proc.kill();proc.wait();print('isolated desktop forcibly stopped',flush=True)
            if failure is None:assert proc.returncode==0 and (root/'exit-requested').is_file() and (root/'shutdown-result').read_text()=='exited' and not (root/'state/endpoint.json').exists()
try:
    vite=subprocess.Popen(['node',str(base/'node_modules/vite/bin/vite.js'),'preview','--host','127.0.0.1','--port','1420','--strictPort'],cwd=base,stdout=vite_log,stderr=subprocess.STDOUT)
    wait(lambda:'Local:' in (root/'vite.log').read_text(encoding='utf-8'),'Vite',vite);wait(lambda:urllib.request.urlopen('http://127.0.0.1:1420/'+vite_marker.name,timeout=2).read().decode()==marker,'Vite identity',vite)
    producer_log=(root/'producer.log').open('w',encoding='utf-8');producer=subprocess.Popen([writer,'--exact','isolated_desktop_writer','--nocapture'],env=env,stdout=producer_log,stderr=subprocess.STDOUT)
    wait(lambda:(root/'producer-ready').is_file(),'offline MCP operations',producer,60)
    before,endpoint=run_app(True)
    (root/'producer-second-start').write_text('start');wait(lambda:(root/'producer-second-ready').is_file(),'second offline admission',producer,30)
    with socket.socket() as held:
        held.bind(('127.0.0.1',7391));held.listen(1);after,restarted=run_app(False,endpoint['token'])
    assert endpoint['store_id']==restarted['store_id']
    assert before==after and endpoint['token']!=restarted['token'];(root/'producer-stop').write_text('stop');assert producer.wait(timeout=15)==0
    print('PASS official MCP offline recovery, real WebView graphs/evidence, restart, port fallback and second running-to-done task',flush=True)
finally:
    (root/'producer-stop').write_text('stop')
    if producer and producer.poll() is None:
        try:producer.wait(timeout=10)
        except subprocess.TimeoutExpired:producer.terminate();producer.wait(timeout=10)
    if vite and vite.poll() is None:vite.terminate();vite.wait(timeout=10)
    vite_log.close();vite_marker.unlink(missing_ok=True)
