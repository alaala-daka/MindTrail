import json
import os
import pathlib
import queue
import subprocess
import sys
import threading
import uuid

exe = pathlib.Path(sys.argv[1]).resolve()
fixture_parent = pathlib.Path(sys.argv[2]).resolve()
fixture = fixture_parent / ("mcp-smoke-" + uuid.uuid4().hex)
fixture.mkdir()
env = os.environ.copy()
env["MINDTRAIL_TEST_ROOT"] = str(fixture / "state")
process = subprocess.Popen([str(exe), "mcp", "--queue", "smoke"], stdin=subprocess.PIPE,
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env,
                           creationflags=subprocess.CREATE_NO_WINDOW)
lines = queue.Queue()
threading.Thread(target=lambda: [lines.put(line) for line in iter(process.stdout.readline, b"")], daemon=True).start()

def send(value):
    process.stdin.write(json.dumps(value, ensure_ascii=False).encode() + b"\n")
    process.stdin.flush()

def reply():
    try:
        return json.loads(lines.get(timeout=8))
    except queue.Empty:
        details = process.stderr.read().decode(errors="replace") if process.poll() is not None else "still running"
        raise AssertionError(f"producer response timeout: {details}")

try:
    send({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"smoke","version":"1"}}})
    initial = reply()
    assert initial["result"]["protocolVersion"] == "2025-11-25", initial
    assert "tools" in initial["result"]["capabilities"], initial
    send({"jsonrpc":"2.0","method":"notifications/initialized"})
    send({"jsonrpc":"2.0","id":2,"method":"ping"})
    assert reply()["result"] == {}
    send({"jsonrpc":"2.0","id":3,"method":"tools/list"})
    assert len(reply()["result"]["tools"]) == 7
    call={"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"trace_task_start","arguments":{"session":"smoke","operation_id":"op","title":"标题","goal":"goal","project":"project"}}}
    send(call)
    result=reply()
    assert result["result"]["structuredContent"]["status"] == "queued", result
    call["id"]=5
    send(call)
    assert reply()["result"]["structuredContent"]["status"] == "queued"
    process.stdin.close()
    assert process.wait(timeout=8) == 0, process.stderr.read().decode(errors="replace")
    print("PASS isolated subprocess initialize, ping, seven tools and durable queued retry; fixture:", fixture)
finally:
    if process.poll() is None:
        process.kill()
        process.wait(timeout=5)
