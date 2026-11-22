"""Build the owned stage's frontend after checking current frontend inputs."""
import hashlib
import os
from pathlib import Path
import re
import sys

ROOT=Path(__file__).resolve().parent.parent
sys.path.insert(0,str(ROOT/'scripts'))
from release_launcher import run_job

def inputs(root):
    result={}
    for name in ('package.json','package-lock.json','index.html','vite.config.ts','tsconfig.json','tsconfig.node.json'):
        p=root/name
        if p.is_symlink() or not p.is_file(): raise ValueError('invalid_input')
        result[name]=hashlib.sha256(p.read_bytes()).digest()
    optional=root/'tsconfig.app.json'
    if optional.exists():
        if optional.is_symlink() or not optional.is_file(): raise ValueError('invalid_input')
        result['tsconfig.app.json']=hashlib.sha256(optional.read_bytes()).digest()
    for directory in ('src','public','protocol'):
        rootdir=root/directory
        if not rootdir.exists(): continue
        for p in rootdir.rglob('*'):
            if p.is_symlink() or getattr(p.lstat(),'st_file_attributes',0)&0x400: raise ValueError('invalid_input')
            if p.is_file(): result[p.relative_to(root).as_posix()]=hashlib.sha256(p.read_bytes()).digest()
    return result

def main(run_id):
    if not re.fullmatch('[0-9a-f]{32}',run_id): raise ValueError('invalid_run')
    base=ROOT/'.tools/productization-release'; run=base/('run-'+run_id)
    if run.is_symlink() or run.resolve().parent!=base.resolve() or (run/'owner').read_text(encoding='ascii')!=run_id or (run/'.release-quarantine').exists(): raise ValueError('invalid_run')
    stage=run/'source'; before=inputs(ROOT)
    if before!=inputs(stage): raise ValueError('frontend_source_changed')
    windows=Path(os.environ['SystemRoot']); node_dir=Path(os.environ['ProgramFiles'])/'nodejs'
    env={'SystemRoot':str(windows),'WINDIR':str(windows),'ComSpec':str(windows/'System32/cmd.exe'),
         'PATH':str(node_dir)+os.pathsep+str(windows/'System32'),'TEMP':str(run/'temp'),'TMP':str(run/'temp'),
         'USERPROFILE':str(run/'empty-home'),'HOME':str(run/'empty-home'),
         'NPM_CONFIG_USERCONFIG':str(run/'npm-user.conf'),'NPM_CONFIG_GLOBALCONFIG':str(run/'npm-global.conf'),
         'NPM_CONFIG_CACHE':str(run/'npm-cache'),'NPM_CONFIG_OFFLINE':'true'}
    code=run_job([str(node_dir/'node.exe'),str(node_dir/'node_modules/npm/bin/npm-cli.js'),'run','build'],cwd=stage,env=env,timeout_seconds=180)
    if before!=inputs(ROOT) or before!=inputs(stage): raise ValueError('frontend_source_changed')
    print('frontend_build='+('passed' if code==0 else 'failed')); return code
if __name__=='__main__':
    try: sys.exit(main(sys.argv[1]))
    except TimeoutError: print('frontend_build=timeout'); sys.exit(1)
    except Exception: print('frontend_build=probe_failed'); sys.exit(1)
