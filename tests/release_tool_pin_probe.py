import json
from pathlib import Path
import sys

ROOT=Path(__file__).resolve().parent.parent
sys.path.insert(0,str(ROOT/'scripts'))
from release_tools import validate_tool

record=json.loads((ROOT/'.tools/productization-release'/('run-'+sys.argv[1])/'producer-toolchain-inspection-v2.json').read_text())
good=bad=0
for name,item in record['tools'].items():
    try: validate_tool(item['path'],item['path'],item['sha256']); good+=1
    except Exception: bad+=1; print('rejected_role='+name if name in ('cargo.exe','rustc.exe','vswhere.exe','vcvars64.bat','cmd.exe','pwsh.exe','cl.exe','link.exe','rc.exe','mt.exe') else 'rejected_role=unknown')
print('tool_pins_valid='+str(good)); print('tool_pins_rejected='+str(bad))
