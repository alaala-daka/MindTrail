"""Check lock-derived cache entry existence only; do not read cache payloads."""
import base64
import json
from pathlib import Path
import re
import tomllib

ROOT=Path(__file__).resolve().parent.parent
def main():
    lock=tomllib.loads((ROOT/'src-tauri/Cargo.lock').read_text(encoding='utf8'))
    registry_root=ROOT/'.tools/cargo/registry/cache'
    registry_dirs=[p for p in registry_root.iterdir() if p.is_dir() and not p.is_symlink()]
    cargo=[p for p in lock['package'] if p.get('source','').startswith('registry+')]
    if any(p['source']!='registry+https://github.com/rust-lang/crates.io-index' or not re.fullmatch('[A-Za-z0-9_-]+',p['name']) or not re.fullmatch('[A-Za-z0-9.+_-]+',p['version']) for p in cargo): raise ValueError('unapproved_source')
    present=sum(any((registry/(p['name']+'-'+p['version']+'.crate')).is_file() for registry in registry_dirs) for p in cargo)
    npm=json.loads((ROOT/'package-lock.json').read_text(encoding='utf8'))
    packages=[v for v in npm['packages'].values() if isinstance(v,dict) and v.get('resolved')]
    content=ROOT/'.tools/npm-cache/_cacache/content-v2/sha512'
    hashes=[]
    for p in packages:
        integrity=p.get('integrity','')
        if not integrity.startswith('sha512-'): raise ValueError('unapproved_integrity')
        digest=base64.b64decode(integrity[7:],validate=True)
        if len(digest)!=64: raise ValueError('unapproved_integrity')
        h=digest.hex(); hashes.append(content/h[:2]/h[2:4]/h[4:])
    npm_present=sum(p.is_file() for p in hashes)
    print('cargo_lock_entries='+str(len(cargo)))
    print('cargo_cache_directory_count='+str(len(registry_dirs)))
    print('cargo_cache_entries_present='+str(present))
    print('npm_lock_entries='+str(len(packages)))
    print('npm_cache_entries_present='+str(npm_present))
if __name__=='__main__':
    try: main()
    except Exception: print('cache_availability=failed'); raise SystemExit(1)
