import json, os, pathlib, subprocess, sys, threading, uuid
fixture=pathlib.Path(sys.argv[2]).resolve()/("mcp-backpressure-"+uuid.uuid4().hex);fixture.mkdir()
env=os.environ.copy();env["MINDTRAIL_TEST_ROOT"]=str(fixture/"state")
p=subprocess.Popen([str(pathlib.Path(sys.argv[1]).resolve()),"mcp","--queue","backpressure"],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=env,creationflags=subprocess.CREATE_NO_WINDOW)
try:
    hello={"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"pressure","version":"1"}}}
    p.stdin.write(json.dumps(hello).encode()+b"\n");p.stdin.flush();assert json.loads(p.stdout.readline())["result"]["protocolVersion"]=="2025-11-25"
    def flood():
        try:
            p.stdin.write(b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
            for i in range(10000):p.stdin.write(json.dumps({"jsonrpc":"2.0","id":i+10,"method":"ping"}).encode()+b"\n")
            p.stdin.close()
        except (BrokenPipeError,OSError,ValueError):pass
    thread=threading.Thread(target=flood,daemon=True);thread.start()
    p.wait(timeout=10);thread.join(timeout=1)
    assert p.returncode in (0,1),p.returncode
    print("PASS bounded exit with unread stdout; fixture:",fixture)
finally:
    if p.poll() is None:p.kill();p.wait(timeout=5)
