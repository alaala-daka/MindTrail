"""Exercise the live prefix workbench check using only synthetic loopback events."""
import json
from pathlib import Path
import shutil
import socket
import urllib.request

from mindtrail_real_agent import NoRedirect, PUBLIC_CODES, build_fixture, prepare_run, public_evidence
from mindtrail_real_agent_desktop import build_desktop, DesktopProcess, PreviewProcess, Workbench


def main():
    repo = Path(__file__).resolve().parent.parent
    evidence = {}
    try:
        fixture = build_fixture(repo)
        desktop_artifact = build_desktop(repo)
        node = shutil.which('node.exe')
        if not node:
            raise ValueError('node_missing')
        run = prepare_run(repo, fixture)
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            debug_port = sock.getsockname()[1]
        preview = PreviewProcess(repo, Path(node))
        try:
            desktop = DesktopProcess(desktop_artifact, run, repo, debug_port=debug_port)
            try:
                endpoint = desktop.endpoint()
                title, capability = 'synthetic prefix', 'synthetic-capability'
                events = [
                    {'v': 1, 'session': 'synthetic-session', 'seq': 1, 'ts': '2024-01-01T00:00:00Z',
                     'type': 'task.start', 'payload': {'title': title, 'goal': 'synthetic', 'project': 'synthetic'}},
                    {'v': 1, 'session': 'synthetic-session', 'seq': 2, 'ts': '2024-01-01T00:00:00Z',
                     'type': 'capability.intent', 'payload': {'id': capability, 'title': 'synthetic'}},
                ]
                opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
                request = urllib.request.Request('http://127.0.0.1:' + str(endpoint['port']) + '/v1/events',
                    data=json.dumps(events).encode('utf-8'), method='POST',
                    headers={'Authorization': 'Bearer ' + endpoint['token'], 'Content-Type': 'application/json'})
                with opener.open(request, timeout=5) as response:
                    evidence['synthetic_post_ok'] = response.status == 200
                page = Workbench(debug_port, desktop.job)
                try:
                    page.expect_prefix(title, capability, desktop.job.process, evidence=evidence)
                    evidence['synthetic_prefix_ok'] = True
                finally:
                    page.close()
            finally:
                desktop.close()
        finally:
            preview.close()
    except Exception as error:
        evidence['code'] = str(error) if isinstance(error, ValueError) and str(error) in PUBLIC_CODES else 'scenario_failed'
    result = public_evidence(evidence)
    result.update({key: evidence.get(key) is True for key in ('synthetic_post_ok', 'synthetic_prefix_ok')})
    print(json.dumps(result, sort_keys=True))


if __name__ == '__main__':
    main()
