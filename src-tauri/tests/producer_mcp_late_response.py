import json,os,pathlib,queue,sqlite3,subprocess,sys,threading,time,uuid
fixture=pathlib.Path(sys.argv[2]).resolve()/("mcp-late-"+uuid.uuid4().hex);fixture.mkdir();root=fixture/"state";gate=fixture/"gate";gate.mkdir()
env=os.environ.copy();env["MINDTRAIL_TEST_ROOT"]=str(root);env["MINDTRAIL_TEST_OUTPUT_GATE"]=str(gate)
p=subprocess.Popen([str(pathlib.Path(sys.argv[1]).resolve()),"mcp","--queue","late"],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=env,creationflags=subprocess.CREATE_NO_WINDOW)
responses=queue.Queue();threading.Thread(target=lambda:[responses.put(json.loads(line)) for line in iter(p.stdout.readline,b"")],daemon=True).start()
def send(value):p.stdin.write(json.dumps(value).encode()+b"\n");p.stdin.flush()
def call(identifier,session):return {"jsonrpc":"2.0","id":identifier,"method":"tools/call","params":{"name":"trace_task_start","arguments":{"session":session,"operation_id":session,"title":"T","goal":"G","project":"P"}}}
try:
    send({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"late","version":"1"}}});assert responses.get(timeout=8)["result"]["protocolVersion"]=="2025-11-25"
    send({"jsonrpc":"2.0","method":"notifications/initialized"});send(call(100,"old"))
    deadline=time.monotonic()+8
    while not (gate/"ready").exists():assert time.monotonic()<deadline;time.sleep(.01)
    send({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":100}})
    send(call(100,"new"))
    for i in range(15):send(call(200+i,f"new-{i}"))
    db=root/"producers"/"mcp"/"late"/"queue.sqlite3";deadline=time.monotonic()+1.5
    while time.monotonic()<deadline:
        with sqlite3.connect(f"file:{db.as_posix()}?mode=ro",uri=True) as connection:count=connection.execute("SELECT count(*) FROM entries").fetchone()[0]
        if count==17:break
        time.sleep(.01)
    assert count==17,count
    (gate/"release").write_text("release")
    send(call(500,"overflow"));result=responses.get(timeout=5);assert result["id"]==500,result;assert result["result"]["structuredContent"]["error"]["code"]=="rate_limited",result
    # The old response was dropped. The reused ID belongs to the new session.
    result=responses.get(timeout=5)
    while result.get("id")!=100:result=responses.get(timeout=5)
    assert result["result"]["structuredContent"]["session"]=="new",result
    p.stdin.close();assert p.wait(timeout=8)==0
    print("PASS delayed old response cannot release reused request capacity; fixture:",fixture)
finally:
    if p.poll() is None:p.kill();p.wait(timeout=5)
