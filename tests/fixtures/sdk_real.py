import pathlib, sys
sys.path.insert(0, sys.argv[2])
from mindtrail_producer import MindTrailProducer
sdk=MindTrailProducer(str(pathlib.Path(sys.argv[1]).resolve()))
online=sys.argv[3]=="online"
event={"v":1,"session":"sdk-python","seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"Python","goal":"G","project":"P"}}
first=sdk.submit_events("python","python & | batch",[event])
assert first.get("status")==("committed" if online else "queued"),first
assert sdk.submit_events("python","python & | batch",[event])==first
if online:
    changed={**event,"payload":{**event["payload"],"title":"changed"}}
    assert sdk.submit_events("python","python & | batch",[changed])["error"]["code"]=="batch_conflict"
    assert sdk.flush("python")=={"committed":1,"pending":0,"blocked":0}
print("PASS Python SDK")
