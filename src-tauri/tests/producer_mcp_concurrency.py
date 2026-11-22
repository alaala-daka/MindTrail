import json
import os
import pathlib
import queue
import sqlite3
import subprocess
import sys
import threading
import time
import uuid

fixture = pathlib.Path(sys.argv[2]).resolve() / ("mcp-concurrent-" + uuid.uuid4().hex)
fixture.mkdir()
root = fixture / "state"
env = os.environ.copy()
env["MINDTRAIL_TEST_ROOT"] = str(root)
process = subprocess.Popen([str(pathlib.Path(sys.argv[1]).resolve()), "mcp", "--queue", "concurrent"],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, creationflags=subprocess.CREATE_NO_WINDOW)
responses = queue.Queue()
threading.Thread(target=lambda: [responses.put(json.loads(x)) for x in iter(process.stdout.readline, b"")], daemon=True).start()
def send(value):
    process.stdin.write(json.dumps(value).encode() + b"\n"); process.stdin.flush()
def wait_id(identifier):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        value = responses.get(timeout=max(0.01, deadline-time.monotonic()))
        if value.get("id") == identifier:
            return value
    raise AssertionError("missing response")
def call(identifier):
    return {"jsonrpc":"2.0","id":identifier,"method":"tools/call","params":{"name":"trace_task_start","arguments":{"session":f"s{identifier}","operation_id":f"o{identifier}","title":"T","goal":"G","project":"P"}}}
try:
    send({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"concurrency","version":"1"}}})
    assert wait_id(1)["result"]["protocolVersion"] == "2025-11-25"
    send({"jsonrpc":"2.0","method":"notifications/initialized"})
    for identifier in range(100,116): send(call(identifier))
    db = root / "producers" / "mcp" / "concurrent" / "queue.sqlite3"
    deadline=time.monotonic()+1.5
    while time.monotonic()<deadline:
        try:
            with sqlite3.connect(f"file:{db.as_posix()}?mode=ro", uri=True) as connection:
                count=connection.execute("SELECT count(*) FROM entries").fetchone()[0]
            if count==16: break
        except sqlite3.Error: pass
        time.sleep(.01)
    assert count==16, count
    send({"jsonrpc":"2.0","id":999,"method":"notifications/cancelled","params":{"requestId":100}})
    assert "error" in wait_id(999)
    send({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":100,"unexpected":True}})
    send({"jsonrpc":"2.0","id":3,"method":"ping"});assert wait_id(3)["result"]=={}
    send(call(100));assert wait_id(100)["error"]["code"]==-32600
    send(call(116))
    assert wait_id(116)["result"]["structuredContent"]["error"]["code"]=="rate_limited"
    for identifier in range(100,116): send({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":identifier}})
    send({"jsonrpc":"2.0","id":2,"method":"ping"});assert wait_id(2)["result"]=={}
    time.sleep(.1)
    replacement=call(100);replacement["params"]["arguments"]["session"]="new-session";replacement["params"]["arguments"]["operation_id"]="new-operation"
    send(replacement);answer=wait_id(100);assert answer["result"]["structuredContent"]["status"]=="queued";assert answer["result"]["structuredContent"]["session"]=="new-session"
    process.stdin.close();assert process.wait(timeout=8)==0
    print("PASS 16 durable operations, 17th limited, cancellation and recovered capacity; fixture:",fixture)
finally:
    if process.poll() is None:
        process.kill();process.wait(timeout=5)
