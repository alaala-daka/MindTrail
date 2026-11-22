"""Extract an already verified NSIS archive into a newly owned target directory."""
import hashlib
import os
from pathlib import Path
import re
import stat
import zipfile

_RESERVED={'CON','PRN','AUX','NUL',*(f'COM{i}' for i in range(1,10)),*(f'LPT{i}' for i in range(1,10))}

def extract_nsis(archive,target):
    target=Path(target)
    if isinstance(archive,(str,Path)) and not Path(archive).is_absolute(): raise ValueError('invalid_archive')
    if not target.is_absolute() or target.exists(): raise ValueError('invalid_target')
    if target.name!='NSIS' or target.parent.name!='.tauri': raise ValueError('invalid_target')
    parent=target.parent
    if not parent.is_dir() or parent.is_symlink() or getattr(parent.lstat(),'st_file_attributes',0)&0x400: raise ValueError('invalid_target')
    with zipfile.ZipFile(archive) as source:
        entries=source.infolist()
        if not entries or len(entries)>5000 or sum(item.file_size for item in entries)>256*1024*1024: raise ValueError('invalid_archive')
        seen=set(); files=[]
        for item in entries:
            name=item.orig_filename
            mode=(item.external_attr>>16)&0o170000
            if not name.startswith('nsis-3.13/') or '\\' in name or mode not in (0,stat.S_IFREG,stat.S_IFDIR) or item.file_size>64*1024*1024: raise ValueError('invalid_archive')
            relative=name[len('nsis-3.13/'):].rstrip('/')
            if not relative: continue
            parts=relative.split('/')
            if any(not part or part in ('.','..') or re.search(r'[<>:"|?*\x00-\x1f]',part) or part.rstrip(' .')!=part or part.split('.')[0].upper() in _RESERVED or (hasattr(os.path,'isreserved') and os.path.isreserved(part)) for part in parts): raise ValueError('invalid_archive')
            key=relative.casefold()
            if key in seen: raise ValueError('invalid_archive')
            seen.add(key); files.append((item,parts))
        target.mkdir()
        for item,parts in files:
            path=target.joinpath(*parts)
            if item.is_dir(): path.mkdir(parents=True,exist_ok=True); continue
            path.parent.mkdir(parents=True,exist_ok=True)
            with source.open(item) as inp, path.open('xb') as out:
                remaining=item.file_size
                while remaining:
                    block=inp.read(min(1024*1024,remaining))
                    if not block: raise ValueError('invalid_archive')
                    out.write(block); remaining-=len(block)
                if inp.read(1): raise ValueError('invalid_archive')

def extract_owned(run,base,expected_sha256):
    """Caller must verify exclusive ownership and independently approve archive pin."""
    run=Path(run); base=Path(base)
    if not run.is_absolute() or not base.is_absolute() or run.parent!=base or not re.fullmatch(r'run-[0-9a-f]{32}',run.name) or not re.fullmatch('[0-9a-f]{64}',expected_sha256): raise ValueError('invalid_run')
    for parent in (run,*run.parents):
        info=parent.lstat()
        if not stat.S_ISDIR(info.st_mode) or getattr(info,'st_file_attributes',0)&0x400: raise ValueError('invalid_run')
    owner=run/'owner'; owner_info=owner.lstat()
    if not stat.S_ISREG(owner_info.st_mode) or owner_info.st_nlink!=1 or getattr(owner_info,'st_file_attributes',0)&0x400 or owner_info.st_size!=32: raise ValueError('invalid_run')
    with owner.open('rb') as f:
        if (os.fstat(f.fileno()).st_dev,os.fstat(f.fileno()).st_ino)!=(owner_info.st_dev,owner_info.st_ino) or f.read(33)!=run.name[4:].encode('ascii'): raise ValueError('invalid_run')
    if os.path.lexists(run/'.release-quarantine'): raise ValueError('quarantined_run')
    folder=run/'nsis-downloads'; info=folder.lstat()
    if not stat.S_ISDIR(info.st_mode) or getattr(info,'st_file_attributes',0)&0x400: raise ValueError('invalid_run')
    archive=folder/'nsis.zip'; info=archive.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_nlink!=1 or getattr(info,'st_file_attributes',0)&0x400 or not 0<info.st_size<=64*1024*1024: raise ValueError('invalid_archive')
    target_parent=run/'target'; ti=target_parent.lstat()
    if not stat.S_ISDIR(ti.st_mode) or getattr(ti,'st_file_attributes',0)&0x400: raise ValueError('invalid_run')
    local=target_parent/'.tauri'
    if os.path.lexists(local): raise ValueError('invalid_target')
    try:
        with archive.open('rb') as stream:
            before=os.fstat(stream.fileno())
            if (before.st_dev,before.st_ino,before.st_size,before.st_mtime_ns)!=(info.st_dev,info.st_ino,info.st_size,info.st_mtime_ns): raise ValueError('invalid_archive')
            digest=hashlib.sha256()
            for block in iter(lambda:stream.read(1024*1024),b''): digest.update(block)
            if digest.hexdigest()!=expected_sha256: raise ValueError('invalid_archive')
            stream.seek(0); local.mkdir(); extract_nsis(stream,local/'NSIS')
            after=os.fstat(stream.fileno()); last=archive.lstat()
            identity=lambda s:(s.st_dev,s.st_ino,s.st_size,s.st_mtime_ns)
            if identity(before)!=identity(after) or identity(after)!=identity(last): raise ValueError('invalid_archive')
    except BaseException:
        try:
            with (run/'.release-quarantine').open('x',encoding='ascii') as f: f.write('failed')
        except FileExistsError: pass
        raise
