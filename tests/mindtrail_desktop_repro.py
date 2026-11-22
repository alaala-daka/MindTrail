"""Unauthenticated fixed-event desktop reproduction; output fixed booleans only."""
import json
import os
from pathlib import Path
import queue
import shutil
import socket
import subprocess
import threading

from mindtrail_real_agent import TOOLS, build_fixture, fixture_environment, prepare_run, wait_events, validate_event_projection
from mindtrail_real_agent_desktop import build_desktop, DesktopProcess, PreviewProcess, Workbench
from mindtrail_real_agent_job import OwnedJob

RESULT = {}


def main():
    repo = Path(__file__).resolve().parent.parent
    result = RESULT
    artifact = build_fixture(repo)
    desktop_artifact = build_desktop(repo)
    run = prepare_run(repo, artifact)
    ident = run['id']
    session, title = 'synthetic-' + ident, 'MindTrail synthetic ' + ident
    values = [{'title': title, 'goal': '验证合成调用', 'project': 'synthetic'},
              {'id': 'cap', 'title': 'C'},
              {'id': 'api', 'kind': 'fn', 'signature': 'double(value)', 'capability': 'cap'},
              {'id': 'file', 'path': 'cli.mjs', 'action': 'create', 'capability': 'cap'},
              {'from': 'file', 'to': 'api', 'kind': 'calls'},
              {'id': 'cap', 'quote': '命令入口调用 double', 'context': '已用正负输入验证输出'}, {}]
    node = shutil.which('node.exe')
    if not node:
        raise ValueError('node_missing')
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0)); port = sock.getsockname()[1]
    preview = PreviewProcess(repo, Path(node))
    try:
        desktop = DesktopProcess(desktop_artifact, run, repo, debug_port=port)
        try:
            endpoint = desktop.endpoint()
            page = Workbench(port, desktop.job)
            try:
                env = {k: os.environ[k] for k in ('SystemRoot', 'WINDIR', 'PATH', 'TEMP', 'TMP') if k in os.environ}
                env = fixture_environment(env, run['state'])
                job = OwnedJob([str(artifact.executable), 'mcp', '--queue', run['queue']], cwd=run['work'], env=env, stderr=subprocess.DEVNULL)
                responses = queue.Queue(maxsize=16)
                def reader():
                    for line in job.process.stdout:
                        if len(line) > 1048576:
                            break
                        responses.put_nowait(line)
                thread = threading.Thread(target=reader, daemon=True); thread.start()
                def send(value):
                    job.process.stdin.write(json.dumps(value, ensure_ascii=False).encode() + b'\n'); job.process.stdin.flush()
                def receive(ident):
                    response = json.loads(responses.get(timeout=20))
                    if response.get('id') != ident or 'error' in response or not isinstance(response.get('result'), dict):
                        raise ValueError('response_failed')
                    return response['result']
                try:
                    send({'jsonrpc':'2.0','id':1,'method':'initialize','params':{'protocolVersion':'2025-11-25','capabilities':{},'clientInfo':{'name':'synthetic','version':'1'}}}); receive(1)
                    send({'jsonrpc':'2.0','method':'notifications/initialized'})
                    calls = []
                    for index, (tool, value) in enumerate(zip(TOOLS, values)):
                        args = {'session': session, 'operation_id': ident + '-' + str(index), **value}
                        send({'jsonrpc':'2.0','id':index+2,'method':'tools/call','params':{'name':tool,'arguments':args}})
                        response = receive(index+2)
                        if response.get('isError') is True or response.get('is_error') is True:
                            raise ValueError('tool_failed')
                        calls.append((tool, args, {}))
                        if index == 1:
                            validate_event_projection(calls, wait_events(endpoint, session, 2))
                            page.expect_prefix(title, 'cap', desktop.job.process)
                            result['prefix'] = True
                    validate_event_projection(calls, wait_events(endpoint, session, 7))
                    result['seven_events'] = True
                    try:
                        page.expect_complete(title, 'cap', 'api', 'file', desktop.job.process, evidence=result)
                    except ValueError:
                        graph = page.graph()
                        result['graph_node_count'] = len(graph['nodes'])
                        result['graph_edge_count'] = len(graph['edges'])
                        for name in ('root', 'cap', 'api', 'file', 'unassigned'):
                            result['graph_has_' + name] = name in graph['nodes']
                        result['hierarchy_selected'] = bool(page.evaluate("[...document.querySelectorAll('.segmented button')].some(x=>x.textContent.includes('层级视图') && x.getAttribute('aria-pressed')==='true')"))
                        result['handle_count'] = page.evaluate("document.querySelectorAll('.react-flow__handle').length")
                        result['node_boxes_nonzero'] = bool(page.evaluate("[...document.querySelectorAll('.react-flow__node')].every(n=>n.getBoundingClientRect().width>0 && n.getBoundingClientRect().height>0)"))
                        result['canvas_box_nonzero'] = bool(page.evaluate("(()=>{const e=document.querySelector('.react-flow');return !!e && e.getBoundingClientRect().width>0 && e.getBoundingClientRect().height>0})()"))
                        raise
                    result['complete'] = True
                finally:
                    job.close(); thread.join(timeout=10)
            finally:
                page.close()
        finally:
            desktop.close()
    finally:
        preview.close()
    return result


if __name__ == '__main__':
    try:
        print(json.dumps(main(), sort_keys=True))
    except Exception:
        RESULT['repro_failed'] = True
        print(json.dumps(RESULT, sort_keys=True))
        raise SystemExit(1) from None
