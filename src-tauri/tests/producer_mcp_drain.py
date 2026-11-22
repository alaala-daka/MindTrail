import json,os,pathlib,queue,subprocess,sys,threading,time,uuid
parent=pathlib.Path(sys.argv[2]).resolve()
for release in (True,False):
    fixture=parent/("mcp-drain-"+uuid.uuid4().hex);fixture.mkdir();gate=fixture/"gate";gate.mkdir();env=os.environ.copy();env["MINDTRAIL_TEST_ROOT"]=str(fixture/"state");env["MINDTRAIL_TEST_OUTPUT_GATE"]=str(gate)
    p=subprocess.Popen([str(pathlib.Path(sys.argv[1]).resolve()),"mcp","--queue","drain"],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=env,creationflags=subprocess.CREATE_NO_WINDOW)
    responses=queue.Queue();threading.Thread(target=lambda:[responses.put(json.loads(line)) for line in iter(p.stdout.readline,b"")],daemon=True).start()
    def send(value):p.stdin.write(json.dumps(value).encode()+b"\n");p.stdin.flush()
    def receive():
        try:return responses.get(timeout=8)
        except queue.Empty:
            details=p.stderr.read().decode(errors="replace") if p.poll() is not None else "producer still running"
            raise AssertionError(f"response timeout: {details}")
    try:
        send({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"drain","version":"1"}}});assert receive()["result"]["protocolVersion"]=="2025-11-25"
        send({"jsonrpc":"2.0","method":"notifications/initialized"});send({"jsonrpc":"2.0","id":100,"method":"tools/call","params":{"name":"trace_task_start","arguments":{"session":"s","operation_id":"o","title":"T","goal":"G","project":"P"}}})
        deadline=time.monotonic()+8
        while not (gate/"ready").exists():assert time.monotonic()<deadline;time.sleep(.01)
        p.stdin.close();time.sleep(.2);assert p.poll() is None,"EOF exited before draining the queued response"
        if release:
            (gate/"release").write_text("release");response=responses.get(timeout=5);assert response["id"]==100 and response["result"]["structuredContent"]["status"]=="queued";assert p.wait(timeout=5)==0
        else:
            assert p.wait(timeout=5)!=0;assert b"output_unavailable" in p.stderr.read()
        print("PASS EOF drain", "released" if release else "bounded timeout", "fixture:",fixture)
    finally:
        if p.poll() is None:p.kill();p.wait(timeout=5)
