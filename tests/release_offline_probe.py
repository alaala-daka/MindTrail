"""Read-only, offline dependency-resolution probe for an owned source stage."""
import os
from pathlib import Path
import re
import subprocess
import sys

ROOT=Path(__file__).resolve().parent.parent
def main(run_id):
    if not re.fullmatch('[0-9a-f]{32}',run_id): raise ValueError('invalid_run')
    base=ROOT/'.tools/productization-release'; run=base/('run-'+run_id)
    if run.is_symlink() or run.resolve().parent!=base.resolve() or (run/'owner').read_text(encoding='ascii')!=run_id: raise ValueError('invalid_run')
    for name in ('source','target','temp','cargo-home'):
        p=run/name
        if p.is_symlink() or not p.is_dir(): raise ValueError('invalid_run')
    windows=Path(os.environ['SystemRoot']); cwd=windows/'System32'
    for p in (cwd,windows,Path(windows.anchor)):
        if any((p/n).exists() for n in ('.cargo/config','.cargo/config.toml')): raise ValueError('cwd_config_present')
    tool=ROOT/'.tools/rustup/toolchains/stable-x86_64-pc-windows-msvc/bin'
    env={'SystemRoot':str(windows),'WINDIR':str(windows),'PATH':str(tool)+os.pathsep+str(cwd),
         'TEMP':str(run/'temp'),'TMP':str(run/'temp'),'USERPROFILE':str(run/'empty-home'),'HOME':str(run/'empty-home'),
         'CARGO_HOME':str(run/'cargo-home'),'CARGO_TARGET_DIR':str(run/'target'),'CARGO_NET_OFFLINE':'true',
         'RUSTC':str(tool/'rustc.exe')}
    result=subprocess.run([str(tool/'cargo.exe'),'metadata','--format-version','1','--locked','--offline','--manifest-path',str(run/'source/src-tauri/Cargo.toml')],cwd=cwd,env=env,capture_output=True,timeout=60)
    if result.returncode==0: print('offline_metadata=passed')
    elif b'no matching package named' in result.stderr.lower(): print('offline_metadata=dependency_missing')
    else: print('offline_metadata=failed')
    return result.returncode
if __name__=='__main__':
    try: sys.exit(main(sys.argv[1]))
    except Exception: print('offline_metadata=probe_failed'); sys.exit(1)
