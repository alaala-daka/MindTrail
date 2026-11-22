"""Prepare npm dependencies only in an owned stage, with scripts disabled."""
import os
import hashlib
from pathlib import Path
import re
import shutil
import sys

ROOT=Path(__file__).resolve().parent.parent
sys.path.insert(0,str(ROOT/'scripts'))
from release_launcher import run_job

def main(run_id,offline=False,install_offline=False):
    if not re.fullmatch('[0-9a-f]{32}',run_id): raise ValueError('invalid_run')
    base=ROOT/'.tools/productization-release'; run=base/('run-'+run_id)
    if run.is_symlink() or run.resolve().parent!=base.resolve() or (run/'owner').read_text(encoding='ascii')!=run_id: raise ValueError('invalid_run')
    for name in ('source','temp','npm-cache','empty-home'):
        p=run/name
        if not p.is_dir() or p.is_symlink() or getattr(p.lstat(),'st_file_attributes',0)&0x400: raise ValueError('invalid_run')
    for name in ('npm-user.conf','npm-global.conf'):
        p=run/name
        if p.is_symlink() or not p.is_file() or p.stat().st_size!=0: raise ValueError('invalid_config')
    if (run/'source/.npmrc').exists(): raise ValueError('invalid_source')
    if offline:
        work=run/'npm-offline-check'
        work.mkdir()
        for name in ('package.json','package-lock.json'):
            original=run/'source'/name; target=work/name
            shutil.copyfile(original,target)
            if hashlib.sha256(original.read_bytes()).digest()!=hashlib.sha256(target.read_bytes()).digest(): raise ValueError('copy_mismatch')
    else:
        work=run/'source'
        if (work/'node_modules').exists(): raise ValueError('invalid_source')
    windows=Path(os.environ['SystemRoot']); node_dir=Path(os.environ['ProgramFiles'])/'nodejs'
    node=node_dir/'node.exe'; cli=node_dir/'node_modules/npm/bin/npm-cli.js'
    env={'SystemRoot':str(windows),'WINDIR':str(windows),'ComSpec':str(windows/'System32/cmd.exe'),
         'PATH':str(node_dir)+os.pathsep+str(windows/'System32'),'TEMP':str(run/'temp'),'TMP':str(run/'temp'),
         'USERPROFILE':str(run/'empty-home'),'HOME':str(run/'empty-home'),
         'NPM_CONFIG_USERCONFIG':str(run/'npm-user.conf'),'NPM_CONFIG_GLOBALCONFIG':str(run/'npm-global.conf'),
         'NPM_CONFIG_CACHE':str(run/'npm-cache'),'NPM_CONFIG_REGISTRY':'https://registry.npmjs.org/',
         'NPM_CONFIG_REPLACE_REGISTRY_HOST':'always','NPM_CONFIG_FETCH_RETRIES':'0'}
    args=[str(node),str(cli),'ci','--ignore-scripts','--no-audit','--no-fund']
    if offline or install_offline: args.append('--offline')
    code=run_job(args,cwd=work,env=env,timeout_seconds=180)
    print(('npm_offline=' if offline or install_offline else 'npm_prepare=')+('passed' if code==0 else 'failed'))
    return code
if __name__=='__main__':
    try: sys.exit(main(sys.argv[1],sys.argv[2:]==['--offline'],sys.argv[2:]==['--install-offline']))
    except TimeoutError: print('npm_prepare=timeout'); sys.exit(1)
    except Exception: print('npm_prepare=probe_failed'); sys.exit(1)
