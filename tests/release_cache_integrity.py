"""Hash only lock-derived package archive candidates; no extraction or credential reads."""
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import tomllib

ROOT=Path(__file__).resolve().parent.parent
def match_file(path,algorithm,expected):
    try:
        ancestors=[]
        for parent in path.parents:
            parent_info=parent.lstat()
            if not stat.S_ISDIR(parent_info.st_mode) or getattr(parent_info,'st_file_attributes',0)&0x400: return False
            ancestors.append((parent,parent_info.st_dev,parent_info.st_ino))
        info=path.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_nlink!=1 or getattr(info,'st_file_attributes',0)&0x400 or info.st_size>64*1024*1024: return False
        digest=hashlib.new(algorithm)
        with path.open('rb') as f:
            before=os.fstat(f.fileno())
            if (before.st_dev,before.st_ino)!=(info.st_dev,info.st_ino): return False
            while True:
                block=f.read(1024*1024)
                if not block: break
                digest.update(block)
            after=os.fstat(f.fileno())
        current=path.lstat()
        if getattr(current,'st_file_attributes',0)&0x400 or current.st_nlink!=1: return False
        for parent,dev,ino in ancestors:
            now=parent.lstat()
            if getattr(now,'st_file_attributes',0)&0x400 or (now.st_dev,now.st_ino)!=(dev,ino): return False
        identity=lambda s:(s.st_dev,s.st_ino,s.st_size,s.st_mtime_ns)
        return identity(info)==identity(before)==identity(after)==identity(current) and digest.digest()==expected
    except (FileNotFoundError,PermissionError,OSError): return False

def main():
    lock=tomllib.loads((ROOT/'src-tauri/Cargo.lock').read_text(encoding='utf8'))
    dirs=[p for p in (ROOT/'.tools/cargo/registry/cache').iterdir() if p.is_dir() and not p.is_symlink()]
    packages=[p for p in lock['package'] if p.get('source','').startswith('registry+')]
    verified=0
    for p in packages:
        if p['source']!='registry+https://github.com/rust-lang/crates.io-index' or not re.fullmatch('[A-Za-z0-9_-]+',p['name']) or not re.fullmatch('[A-Za-z0-9.+_-]+',p['version']) or not re.fullmatch('[0-9a-f]{64}',p.get('checksum','')): raise ValueError('invalid_lock')
        name=p['name']+'-'+p['version']+'.crate'; expected=bytes.fromhex(p['checksum'])
        verified+=any(match_file(d/name,'sha256',expected) for d in dirs)
    npm=json.loads((ROOT/'package-lock.json').read_text(encoding='utf8'))
    pkgs=[v for v in npm['packages'].values() if isinstance(v,dict) and v.get('resolved')]
    content=ROOT/'.tools/npm-cache/_cacache/content-v2/sha512'; npm_verified=0
    for p in pkgs:
        integrity=p.get('integrity','')
        if not integrity.startswith('sha512-'): raise ValueError('invalid_lock')
        digest=base64.b64decode(integrity[7:],validate=True)
        if len(digest)!=64: raise ValueError('invalid_lock')
        h=digest.hex(); npm_verified+=match_file(content/h[:2]/h[2:4]/h[4:],'sha512',digest)
    print('cargo_checksum_verified='+str(verified)+'/'+str(len(packages)))
    print('npm_checksum_verified='+str(npm_verified)+'/'+str(len(pkgs)))
if __name__=='__main__':
    try: main()
    except Exception: print('cache_integrity=failed'); raise SystemExit(1)
