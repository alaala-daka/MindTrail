"""Fetch public locked Cargo dependencies into a single owned stage."""
import os
from pathlib import Path
import re
import subprocess
import sys

ROOT=Path(__file__).resolve().parent.parent
sys.path.insert(0,str(ROOT/'scripts'))
from release_environment import check_cargo_cwd
from release_launcher import run_job

def main(run_id):
    if not re.fullmatch('[0-9a-f]{32}',run_id): raise ValueError('invalid_run')
    base=ROOT/'.tools/productization-release'; run=base/('run-'+run_id)
    if run.is_symlink() or run.resolve().parent!=base.resolve() or (run/'owner').read_text(encoding='ascii')!=run_id: raise ValueError('invalid_run')
    for name in ('source','target','temp','cargo-home','empty-home'):
        p=run/name
        if not p.is_dir() or p.is_symlink() or getattr(p.lstat(),'st_file_attributes',0)&0x400: raise ValueError('invalid_run')
    windows=Path(os.environ['SystemRoot']); cwd=windows/'System32'; check_cargo_cwd(cwd)
    tool=ROOT/'.tools/rustup/toolchains/stable-x86_64-pc-windows-msvc/bin'
    env={'SystemRoot':str(windows),'WINDIR':str(windows),'PATH':str(tool)+os.pathsep+str(cwd),
         'TEMP':str(run/'temp'),'TMP':str(run/'temp'),'USERPROFILE':str(run/'empty-home'),'HOME':str(run/'empty-home'),
         'CARGO_HOME':str(run/'cargo-home'),'CARGO_TARGET_DIR':str(run/'target'),
         'CARGO_REGISTRIES_CRATES_IO_PROTOCOL':'sparse','CARGO_HTTP_TIMEOUT':'15','CARGO_NET_RETRY':'0','RUSTC':str(tool/'rustc.exe')}
    code=run_job([str(tool/'cargo.exe'),'fetch','--locked','--manifest-path',str(run/'source/src-tauri/Cargo.toml')],cwd=cwd,env=env,timeout_seconds=180)
    print('cargo_fetch='+('passed' if code==0 else 'failed'))
    return code
if __name__=='__main__':
    try: sys.exit(main(sys.argv[1]))
    except TimeoutError: print('cargo_fetch=timeout'); sys.exit(1)
    except Exception: print('cargo_fetch=probe_failed'); sys.exit(1)
