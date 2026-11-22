"""Prepare a reviewed upstream NSIS archive in one owned run."""
import hashlib
import json
import os
from pathlib import Path
import re
import sys

ROOT=Path(__file__).resolve().parent.parent
sys.path.insert(0,str(ROOT/'scripts'))
from release_launcher import run_job
from release_nsis_extract import extract_owned
from release_nsis_readiness import inspect_owned_toolset

def child(run,base):
    try:
        record=(run/'nsis-downloads.sha256').read_text(encoding='ascii').splitlines()
        if len(record)!=2: raise ValueError('invalid_record')
        pins=dict(line.split(':',1) for line in record)
        if set(pins)!={'nsis.zip','plugin.dll'} or any(not re.fullmatch('[0-9a-f]{64}',x) for x in pins.values()): raise ValueError('invalid_record')
        upstream={'nsis.zip':'db14f8af2ea346b786c6ae19343f61aaf5e09b8e','plugin.dll':'75197fee3c6a814fe035788d1c34ead39349b860'}
        for name in upstream:
            path=run/'nsis-downloads'/name; info=path.lstat()
            if path.is_symlink() or getattr(info,'st_file_attributes',0)&0x400 or info.st_nlink!=1 or not 0<info.st_size<=64*1024*1024: raise ValueError('invalid_download')
            data=path.read_bytes()
            if hashlib.sha1(data,usedforsecurity=False).hexdigest()!=upstream[name] or hashlib.sha256(data).hexdigest()!=pins[name]: raise ValueError('invalid_download')
        extract_owned(run,base,pins['nsis.zip'])
        target=run/'target/.tauri/NSIS'; plugin=target/'Plugins/x86-unicode/additional/nsis_tauri_utils.dll'
        plugin.parent.mkdir(parents=True,exist_ok=True)
        with plugin.open('xb') as out: out.write((run/'nsis-downloads/plugin.dll').read_bytes())
        hashes={}
        for path in target.rglob('*'):
            if path.is_symlink() or getattr(path.lstat(),'st_file_attributes',0)&0x400: raise ValueError('unsafe_toolset')
            if path.is_file(): hashes[path.relative_to(target).as_posix()]=hashlib.sha256(path.read_bytes()).hexdigest()
        if inspect_owned_toolset(run,base,hashes)!='verified_candidate': raise ValueError('toolset_invalid')
        with (run/'nsis-toolset.sha256.json').open('x',encoding='utf8') as out: json.dump(hashes,out,sort_keys=True)
    except BaseException:
        try:
            with (run/'.release-quarantine').open('x',encoding='ascii') as out: out.write('failed')
        except FileExistsError: pass
        raise

def main(run_id):
    if not re.fullmatch('[0-9a-f]{32}',run_id): raise ValueError('invalid_run')
    base=ROOT/'.tools/productization-release'; run=base/('run-'+run_id)
    if run.is_symlink() or run.resolve().parent!=base.resolve() or (run/'owner').read_text(encoding='ascii')!=run_id or (run/'.release-quarantine').exists(): raise ValueError('invalid_run')
    windows=Path(os.environ['SystemRoot']); cwd=windows/'System32'
    env={'SystemRoot':str(windows),'WINDIR':str(windows),'PATH':str(cwd),'TEMP':str(run/'temp'),'TMP':str(run/'temp'),'HOME':str(run/'empty-home'),'USERPROFILE':str(run/'empty-home')}
    code=run_job([sys.executable,'-I',str(Path(__file__).resolve()),'--child',run_id],cwd=cwd,env=env,timeout_seconds=60)
    print('nsis_prepare='+('passed' if code==0 else 'failed')); return code
if __name__=='__main__':
    try:
        if sys.argv[1]=='--child':
            if len(sys.argv)!=3 or not re.fullmatch('[0-9a-f]{32}',sys.argv[2]): raise ValueError('invalid_run')
            base=ROOT/'.tools/productization-release'; run=base/('run-'+sys.argv[2])
            if run.is_symlink() or (run/'owner').read_text(encoding='ascii')!=sys.argv[2]: raise ValueError('invalid_run')
            child(run,base); sys.exit(0)
        sys.exit(main(sys.argv[1]))
    except TimeoutError: print('nsis_prepare=timeout'); sys.exit(1)
    except Exception: print('nsis_prepare=failed'); sys.exit(1)
