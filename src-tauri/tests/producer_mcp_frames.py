import json, os, pathlib, queue, sqlite3, subprocess, sys, threading, uuid
fixture=pathlib.Path(sys.argv[2]).resolve()/("mcp-frames-"+uuid.uuid4().hex);fixture.mkdir()
root=fixture/"state";env=os.environ.copy();env["MINDTRAIL_TEST_ROOT"]=str(root)
p=subprocess.Popen([str(pathlib.Path(sys.argv[1]).resolve()),"mcp","--queue","frames"],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=env,creationflags=subprocess.CREATE_NO_WINDOW)
responses=queue.Queue();threading.Thread(target=lambda:[responses.put(json.loads(x)) for x in iter(p.stdout.readline,b"")],daemon=True).start()
def write(data): p.stdin.write(data);p.stdin.flush()
def result(): return responses.get(timeout=8)
def send(value): write(json.dumps(value,ensure_ascii=False).encode()+b"\n")
try:
    frame=json.dumps({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"frames","version":"1"}}}).encode()+b"\r\n"
    for start in range(0,len(frame),7): write(frame[start:start+7])
    assert result()["result"]["protocolVersion"]=="2025-11-25"
    send({"jsonrpc":"2.0","method":"notifications/initialized"})
    for invalid in [b'{"jsonrpc":"2.0","id":7,"\\u0069d":8,"method":"ping"}\n',b"\xff\n"]:
        write(invalid);assert result()["error"]["code"]==-32700
    send({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"trace_task_done","extra":1,"arguments":{"session":"s","operation_id":"o"}}})
    assert result()["error"]["code"]==-32600
    send({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"trace_task_start","arguments":{"session":"s","operation_id":"o","title":None,"goal":"G","project":"P"}}})
    assert result()["result"]["isError"] is True
    send({"jsonrpc":"2.0","id":4,"method":"ping"});assert result()["result"]=={}
    db=root/"producers"/"mcp"/"frames"/"queue.sqlite3"
    with sqlite3.connect(f"file:{db.as_posix()}?mode=ro",uri=True) as connection: assert connection.execute("SELECT count(*) FROM entries").fetchone()[0]==0
    exact=b'{"x":"'+b'a'*(2*1024*1024-8)+b'"}'
    assert len(exact)==2*1024*1024
    write(exact+b"\n");assert result()["error"]["code"]==-32600
    write(exact+b"\r\n");assert result()["error"]["code"]==-32600
    write(b" "*(2*1024*1024+1));assert result()["error"]["code"]==-32700
    assert p.wait(timeout=8)==0
    print("PASS fragmented CRLF, strict frames, exact/over limit and zero admissions; fixture:",fixture)
finally:
    if p.poll() is None: p.kill();p.wait(timeout=5)
