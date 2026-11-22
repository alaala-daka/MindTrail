"""Copy only this session's prepared caches and NSIS into a fresh owned run."""
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import sys

ROOT=Path(__file__).resolve().parent.parent
sys.path.insert(0,str(ROOT/'scripts'))
from release_nsis_readiness import inspect_owned_toolset

def owned(base,run_id):
    if not re.fullmatch('[0-9a-f]{32}',run_id): raise ValueError('invalid_run')
    run=base/('run-'+run_id)
    if run.is_symlink() or run.resolve().parent!=base.resolve() or (run/'owner').read_text(encoding='ascii')!=run_id or (run/'.release-quarantine').exists(): raise ValueError('invalid_run')
    for p in (run,*run.parents):
        info=p.lstat()
        if not stat.S_ISDIR(info.st_mode) or getattr(info,'st_file_attributes',0)&0x400: raise ValueError('invalid_run')
    return run

def transfer(source,destination):
    if not source.is_dir() or source.is_symlink() or destination.exists(): raise ValueError('invalid_tree')
    for parent in (source,*source.parents):
        info=parent.lstat()
        if not stat.S_ISDIR(info.st_mode) or getattr(info,'st_file_attributes',0)&0x400: raise ValueError('unsafe_tree')
    destination.mkdir(parents=True)
    count=0
    def fail(error): raise error
    for directory,dirs,files in os.walk(source,followlinks=False,onerror=fail):
        current=Path(directory); outdir=destination/current.relative_to(source)
        for name in dirs:
            p=current/name; info=p.lstat()
            if not stat.S_ISDIR(info.st_mode) or getattr(info,'st_file_attributes',0)&0x400: raise ValueError('unsafe_tree')
            (outdir/name).mkdir()
        for name in files:
            p=current/name; q=outdir/name; info=p.lstat()
            if not stat.S_ISREG(info.st_mode) or info.st_nlink!=1 or getattr(info,'st_file_attributes',0)&0x400 or info.st_size>256*1024*1024: raise ValueError('unsafe_file')
            digest=hashlib.sha256()
            with p.open('rb') as inp,q.open('xb') as out:
                before=os.fstat(inp.fileno())
                if (before.st_dev,before.st_ino)!=(info.st_dev,info.st_ino): raise ValueError('source_changed')
                for block in iter(lambda:inp.read(1024*1024),b''): digest.update(block); out.write(block)
                after=os.fstat(inp.fileno())
            identity=lambda x:(x.st_dev,x.st_ino,x.st_size,x.st_mtime_ns)
            if identity(info)!=identity(after) or identity(before)!=identity(after) or identity(after)!=identity(p.lstat()): raise ValueError('source_changed')
            check=hashlib.sha256()
            with q.open('rb') as copied:
                for block in iter(lambda:copied.read(1024*1024),b''): check.update(block)
            if check.digest()!=digest.digest(): raise ValueError('copy_mismatch')
            count+=1
    return count

def main(source_id,destination_id):
    base=ROOT/'.tools/productization-release'; source=owned(base,source_id); target=owned(base,destination_id)
    if source==target: raise ValueError('invalid_run')
    try:
        pins=json.loads((source/'nsis-toolset.sha256.json').read_text(encoding='utf8'))
        if inspect_owned_toolset(source,base,pins)!='verified_candidate': raise ValueError('source_nsis_invalid')
        counts=[]
        for name in ('cargo-home/registry/cache','cargo-home/registry/index','npm-cache/_cacache','target/.tauri/NSIS'):
            counts.append(transfer(source/name,target/name))
        if inspect_owned_toolset(target,base,pins)!='verified_candidate': raise ValueError('target_nsis_invalid')
        with (target/'nsis-toolset.sha256.json').open('x',encoding='utf8') as f: json.dump(pins,f,sort_keys=True)
        print('prepared_trees_copied='+str(len(counts))); print('copied_files='+str(sum(counts))); return 0
    except BaseException:
        try:
            with (target/'.release-quarantine').open('x',encoding='ascii') as f: f.write('failed')
        except FileExistsError: pass
        raise
if __name__=='__main__':
    try: sys.exit(main(sys.argv[1],sys.argv[2]))
    except Exception: print('prepared_transfer=failed'); sys.exit(1)
