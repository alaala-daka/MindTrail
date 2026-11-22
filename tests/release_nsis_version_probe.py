"""Read-only version query of the pinned, owned NSIS executable."""
import json
import hashlib
import os
from pathlib import Path
import re
import sys
import tempfile

ROOT=Path(__file__).resolve().parent.parent
sys.path.insert(0,str(ROOT/'scripts')); sys.path.insert(0,str(ROOT/'tests'))
from release_producer_build import capture_job
from release_nsis_readiness import inspect_owned_toolset

def main():
    base=ROOT/'.tools/productization-release'; run=base/'run-24e310e5772e442da309a81162d3c712'
    if (run/'owner').read_text(encoding='ascii')!='24e310e5772e442da309a81162d3c712' or not (run/'.release-quarantine').exists(): raise ValueError('invalid_run')
    data=(run/'nsis-toolset.sha256.json').read_bytes()
    if hashlib.sha256(data).hexdigest()!='1db5b3b80075d0c774074ea14b9303bab4791bfe055c57e30cb7bc160bbb520e': raise ValueError('invalid_toolset')
    pins=json.loads(data)
    if inspect_owned_toolset(run,base,pins)!='verified_candidate': raise ValueError('invalid_toolset')
    with tempfile.TemporaryDirectory(prefix='nsis-version-',dir=ROOT/'.tools') as raw:
        scratch=Path(raw)
        if scratch.resolve().parent!=(ROOT/'.tools').resolve() or not scratch.name.startswith('nsis-version-'): raise ValueError('invalid_scratch')
        windows=Path(os.environ['SystemRoot']); cwd=windows/'System32'
        env={'SystemRoot':str(windows),'WINDIR':str(windows),'PATH':str(cwd),'TEMP':str(scratch),'TMP':str(scratch),'USERPROFILE':str(scratch),'HOME':str(scratch)}
        exe=run/'target/.tauri/NSIS/makensis.exe'
        code,out,err=capture_job([str(exe),'-NOCONFIG','-VERSION'],cwd=scratch,env=env,timeout=30,temporary=scratch)
        value=out.strip(); print('version_probe_exit='+str(code)); print('version_shape_valid='+str(bool(re.fullmatch(r'v?[0-9]+\.[0-9]+(?:\.[0-9]+)?',value,re.I))).lower())
        if re.fullmatch(r'v?[0-9]+\.[0-9]+(?:\.[0-9]+)?',value,re.I): print('version='+value)
        print('stderr_empty='+str(not bool(err)).lower()); return code
if __name__=='__main__':
    try: sys.exit(main())
    except Exception as error:
        print('version_probe_exception='+type(error).__name__)
        value=getattr(error,'winerror',None)
        if isinstance(value,int) and 0<=value<=65535: print('version_probe_winerror='+str(value))
        fixed={'invalid_run','invalid_toolset','invalid_scratch','job_create_failed','job_setup_failed','job_assign_failed','job_resume_failed','output_limit_exceeded'}
        if str(error) in fixed: print('version_probe_category='+str(error))
        sys.exit(1)
