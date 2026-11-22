import json, os, pathlib, queue, subprocess, sys, threading, uuid
fixture=pathlib.Path(sys.argv[2]).resolve()/("mcp-connections-"+uuid.uuid4().hex);fixture.mkdir();root=fixture/"state"
env=os.environ.copy();env["MINDTRAIL_TEST_ROOT"]=str(root);children=[]
def start(name):
    p=subprocess.Popen([str(pathlib.Path(sys.argv[1]).resolve()),"mcp","--queue",name],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=env,creationflags=subprocess.CREATE_NO_WINDOW)
    q=queue.Queue();threading.Thread(target=lambda:[q.put(json.loads(line)) for line in iter(p.stdout.readline,b"")],daemon=True).start();children.append(p);return p,q
def send(p,value):p.stdin.write(json.dumps(value).encode()+b"\n");p.stdin.flush()
try:
    pairs=[start("one"),start("two")]
    for p,_ in pairs:send(p,{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"client","version":"1"}}})
    for _,q in pairs:assert q.get(timeout=8)["result"]["protocolVersion"]=="2025-11-25"
    for number,(p,_) in enumerate(pairs):
        send(p,{"jsonrpc":"2.0","method":"notifications/initialized"})
        send(p,{"jsonrpc":"2.0","id":77,"method":"tools/call","params":{"name":"trace_task_start","arguments":{"session":f"session-{number}","operation_id":f"op-{number}","title":"T","goal":"G","project":"P"}}})
    for number,(_,q) in enumerate(pairs):
        response=q.get(timeout=8);assert response["id"]==77;assert response["result"]["structuredContent"]["session"]==f"session-{number}"
    for p,_ in pairs:p.stdin.close()
    for p,_ in pairs:assert p.wait(timeout=8)==0
    print("PASS separate connections retain separate operations with same request ID; fixture:",fixture)
finally:
    for p in children:
        if p.poll() is None:p.kill();p.wait(timeout=5)
