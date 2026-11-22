"""Static 2.12.4 NSIS download-trigger check, not tool authenticity approval."""
import hashlib
import os
from pathlib import Path
import stat
import re

REQUIRED=('makensis.exe','Bin/makensis.exe','Stubs/lzma-x86-unicode','Stubs/lzma_solid-x86-unicode',
          'Plugins/x86-unicode/additional/nsis_tauri_utils.dll','Include/MUI2.nsh','Include/FileFunc.nsh',
          'Include/x64.nsh','Include/nsDialogs.nsh','Include/WinMessages.nsh','Include/Win/COM.nsh',
          'Include/Win/Propkey.nsh','Include/Win/RestartManager.nsh','NSIS.exe')
HASHES={'NSIS.exe':'6d45d4e8ccf18fe1fdac9164058f356102a2ecf2',
        'Plugins/x86-unicode/additional/nsis_tauri_utils.dll':'75197fee3c6a814fe035788d1c34ead39349b860'}

def _identity(info): return (info.st_dev,info.st_ino,info.st_size,info.st_mtime_ns)
def inspect_nsis(root):
    """Caller must separately establish ownership and trust of the whole toolset."""
    root=Path(root)
    if not root.is_absolute(): return 'unsafe'
    try:
        for parent in (root,*root.parents):
            info=parent.lstat()
            if not stat.S_ISDIR(info.st_mode) or getattr(info,'st_file_attributes',0)&0x400: return 'unsafe'
        for name in REQUIRED:
            file=root/name
            for parent in file.parents:
                if parent==root: break
                info=parent.lstat()
                if not stat.S_ISDIR(info.st_mode) or getattr(info,'st_file_attributes',0)&0x400: return 'unsafe'
            info=file.lstat()
            if not stat.S_ISREG(info.st_mode) or info.st_nlink!=1 or getattr(info,'st_file_attributes',0)&0x400 or info.st_size<1 or info.st_size>64*1024*1024: return 'unsafe'
            if name in HASHES:
                digest=hashlib.sha1(usedforsecurity=False)
                with file.open('rb') as stream:
                    before=os.fstat(stream.fileno())
                    if _identity(info)!=_identity(before): return 'unsafe'
                    while True:
                        block=stream.read(1024*1024)
                        if not block: break
                        digest.update(block)
                    after=os.fstat(stream.fileno())
                if _identity(before)!=_identity(after) or _identity(after)!=_identity(file.lstat()): return 'unsafe'
                if digest.hexdigest()!=HASHES[name]: return 'hash_mismatch'
        return 'matches_upstream_precheck'
    except FileNotFoundError: return 'missing'
    except OSError: return 'unsafe'

def inspect_owned_toolset(run,expected_base,pins):
    """Verify the exact toolset closure against caller-reviewed SHA-256 pins.

    The caller must establish exclusive ownership of the run and pin provenance.
    """
    run=Path(run); base=Path(expected_base)
    try:
        if not run.is_absolute() or not base.is_absolute() or run.parent!=base or not re.fullmatch(r'run-[0-9a-f]{32}',run.name): return 'unsafe'
        for parent in (run,*run.parents):
            info=parent.lstat()
            if not stat.S_ISDIR(info.st_mode) or getattr(info,'st_file_attributes',0)&0x400: return 'unsafe'
        owner=run/'owner'; info=owner.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_nlink!=1 or getattr(info,'st_file_attributes',0)&0x400 or info.st_size!=32: return 'unsafe'
        with owner.open('rb') as f:
            before=os.fstat(f.fileno()); content=f.read(33); after=os.fstat(f.fileno())
        if _identity(info)!=_identity(before) or _identity(before)!=_identity(after) or _identity(after)!=_identity(owner.lstat()) or content!=run.name[4:].encode('ascii'): return 'unsafe'
        root=run/'target/.tauri/NSIS'
        if inspect_nsis(root)!='matches_upstream_precheck': return 'upstream_precheck_failed'
        if type(pins) is not dict or not pins: return 'unsafe'
        names=set(); actual=set()
        for name,pin in pins.items():
            if not isinstance(name,str) or '\\' in name or ':' in name or name.startswith('/') or any(p in ('','.','..') for p in name.split('/')) or not isinstance(pin,str) or not re.fullmatch('[0-9a-f]{64}',pin) or name.casefold() in names: return 'unsafe'
            names.add(name.casefold())
        def reject_walk_error(error): raise error
        for current,dirs,files in os.walk(root,followlinks=False,onerror=reject_walk_error):
            for name in dirs:
                info=(Path(current)/name).lstat()
                if not stat.S_ISDIR(info.st_mode) or getattr(info,'st_file_attributes',0)&0x400: return 'unsafe'
            for name in files:
                p=Path(current)/name; relative=p.relative_to(root).as_posix(); info=p.lstat()
                if relative not in pins or not stat.S_ISREG(info.st_mode) or info.st_nlink!=1 or getattr(info,'st_file_attributes',0)&0x400 or not 0<info.st_size<=64*1024*1024: return 'unsafe'
                digest=hashlib.sha256()
                with p.open('rb') as f:
                    before=os.fstat(f.fileno())
                    if _identity(info)!=_identity(before): return 'unsafe'
                    while True:
                        block=f.read(1024*1024)
                        if not block: break
                        digest.update(block)
                    after=os.fstat(f.fileno())
                if _identity(before)!=_identity(after) or _identity(after)!=_identity(p.lstat()) or digest.hexdigest()!=pins[relative]: return 'unsafe'
                actual.add(relative)
        return 'verified_candidate' if actual==set(pins) else 'unsafe'
    except (OSError,ValueError): return 'unsafe'
