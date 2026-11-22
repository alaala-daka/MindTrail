"""Derive public archive candidates from lock entries; no network operations."""
import base64
import hashlib
import os
from pathlib import Path
import re
import ssl
import tempfile
from urllib.parse import quote, unquote, urlsplit
from urllib.request import HTTPSHandler, HTTPRedirectHandler, ProxyHandler, Request, build_opener

_CRATE=re.compile(r'^[A-Za-z0-9_-]+$')
_VERSION=re.compile(r'^[0-9A-Za-z][0-9A-Za-z.+_-]*$')
_NPM=re.compile(r'^(?:@[a-z0-9][a-z0-9._-]*/)?[a-z0-9][a-z0-9._-]*$')

def cargo_candidate(entry):
    name=entry.get('name',''); version=entry.get('version',''); checksum=entry.get('checksum','')
    if entry.get('source')!='registry+https://github.com/rust-lang/crates.io-index' or not _CRATE.fullmatch(name) or not _VERSION.fullmatch(version) or not re.fullmatch('[0-9a-f]{64}',checksum): raise ValueError('invalid_cargo_entry')
    return {'url':'https://static.crates.io/crates/'+name+'/'+name+'-'+version+'.crate','algorithm':'sha256','digest':checksum}

def npm_candidate(path,entry):
    if 'node_modules/' not in path: raise ValueError('invalid_npm_entry')
    name=path.rsplit('node_modules/',1)[1]; version=entry.get('version',''); integrity=entry.get('integrity','')
    resolved=urlsplit(entry.get('resolved',''))
    if not _NPM.fullmatch(name) or not _VERSION.fullmatch(version) or resolved.scheme!='https' or not resolved.hostname or resolved.username or resolved.password or not integrity.startswith('sha512-'): raise ValueError('invalid_npm_entry')
    try: digest=base64.b64decode(integrity[7:],validate=True)
    except Exception: raise ValueError('invalid_npm_entry') from None
    if len(digest)!=64: raise ValueError('invalid_npm_entry')
    filename=name.rsplit('/',1)[-1]+'-'+version+'.tgz'
    return {'url':'https://registry.npmjs.org/'+quote(name,safe='@/')+'/-/'+quote(filename,safe='.-_+'),'algorithm':'sha512','digest':digest.hex()}

def _validate_stage(destination,algorithm,expected,run_root,max_bytes):
    if os.name!='nt' or algorithm not in ('sha256','sha512') or not re.fullmatch('[0-9a-f]{'+str(hashlib.new(algorithm).digest_size*2)+'}',expected) or not 0<max_bytes<=64*1024*1024: raise ValueError('invalid_archive')
    dest=Path(destination); parent=dest.parent; root=Path(run_root)
    if not root.is_absolute() or root.is_symlink() or getattr(root.lstat(),'st_file_attributes',0)&0x400 or not parent.resolve().is_relative_to(root.resolve()): raise ValueError('invalid_destination')
    quarantine=root/'.release-quarantine'
    if not dest.is_absolute() or not parent.is_dir() or parent.is_symlink() or getattr(parent.lstat(),'st_file_attributes',0)&0x400 or dest.exists() or os.path.lexists(quarantine): raise ValueError('invalid_destination')
    return dest,parent,quarantine

def stage_verified(stream,destination,algorithm,expected,run_root,max_bytes=64*1024*1024):
    """Stage a stream under an owned parent; the caller supplies and validates the stream."""
    dest,parent,quarantine=_validate_stage(destination,algorithm,expected,run_root,max_bytes)
    parent_before=parent.stat()
    temp=None
    try:
        digest=hashlib.new(algorithm); size=0
        with tempfile.NamedTemporaryFile(prefix='.release-package-',dir=parent,delete=False) as output:
            temp=Path(output.name)
            while True:
                block=stream.read(1024*1024)
                if not block: break
                size+=len(block)
                if size>max_bytes: raise ValueError('archive_too_large')
                digest.update(block); output.write(block)
            output.flush(); os.fsync(output.fileno())
        if size==0 or digest.hexdigest()!=expected: raise ValueError('archive_digest_mismatch')
        # Windows rename fails if destination already exists.
        os.rename(temp,dest); temp=None
        try:
            parent_after=parent.stat()
            if (parent_before.st_dev,parent_before.st_ino)!=(parent_after.st_dev,parent_after.st_ino): raise ValueError('destination_changed')
            info=dest.lstat()
            if getattr(info,'st_file_attributes',0)&0x400 or info.st_nlink!=1: raise ValueError('destination_changed')
            check=hashlib.new(algorithm); total=0
            with dest.open('rb') as staged:
                before=os.fstat(staged.fileno())
                if (before.st_dev,before.st_ino)!=(info.st_dev,info.st_ino): raise ValueError('destination_changed')
                while True:
                    block=staged.read(1024*1024)
                    if not block: break
                    total+=len(block)
                    if total>max_bytes: raise ValueError('destination_changed')
                    check.update(block)
                after=os.fstat(staged.fileno())
            last=dest.lstat()
            identity=lambda s:(s.st_dev,s.st_ino,s.st_size,s.st_mtime_ns)
            if identity(info)!=identity(before) or identity(before)!=identity(after) or identity(after)!=identity(last) or total!=size or check.hexdigest()!=expected: raise ValueError('destination_changed')
        except BaseException:
            try:
                with open(quarantine,'x',encoding='ascii') as marker: marker.write('failed')
            except FileExistsError: pass
            raise
    finally:
        if temp is not None: temp.unlink(missing_ok=True)

class _NoRedirect(HTTPRedirectHandler):
    def redirect_request(self,*args,**kwargs): return None

def download_public(plan,destination,run_root):
    """Direct HTTPS only; caller must provide an isolated environment and owned parent."""
    url=plan.get('url',''); parsed=urlsplit(url)
    if parsed.scheme!='https' or parsed.hostname not in ('static.crates.io','registry.npmjs.org') or parsed.username or parsed.password or parsed.port not in (None,443) or parsed.query or parsed.fragment: raise ValueError('invalid_archive_url')
    path=unquote(parsed.path)
    if '\\' in path or any(part in ('.','..') for part in path.split('/')) or not path.startswith('/'): raise ValueError('invalid_archive_url')
    if parsed.hostname=='static.crates.io' and not path.startswith('/crates/'): raise ValueError('invalid_archive_url')
    _validate_stage(destination,plan.get('algorithm',''),plan.get('digest',''),run_root,64*1024*1024)
    opener=build_opener(ProxyHandler({}),HTTPSHandler(context=ssl.create_default_context()),_NoRedirect())
    request=Request(url,headers={'Accept-Encoding':'identity','User-Agent':'MindTrail-release-probe/1'})
    with opener.open(request,timeout=30) as response:
        if response.status!=200: raise ValueError('archive_http_error')
        stage_verified(response,destination,plan.get('algorithm',''),plan.get('digest',''),run_root)
