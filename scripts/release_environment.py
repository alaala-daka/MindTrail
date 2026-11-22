"""Candidate explicit child environment; ownership and tool pins must be checked by caller."""
import os
from pathlib import Path
import re
import stat

def _safe_item(path,directory):
    item=path.lstat()
    if getattr(item,'st_file_attributes',0)&0x400 or stat.S_ISLNK(item.st_mode): raise ValueError('unsafe_environment_path')
    if directory and not stat.S_ISDIR(item.st_mode): raise ValueError('unsafe_environment_path')
    if not directory and (not stat.S_ISREG(item.st_mode) or item.st_nlink!=1 or item.st_size!=0): raise ValueError('unsafe_environment_path')

def check_cargo_cwd(cwd):
    path=Path(cwd)
    if not path.is_absolute(): raise ValueError('invalid_cwd')
    for item in (path,*path.parents):
        _safe_item(item,True)
        cargo=item/'.cargo'
        if os.path.lexists(cargo):
            _safe_item(cargo,True)
            if any(os.path.lexists(cargo/name) for name in ('config','config.toml')): raise ValueError('ancestor_config_present')

def build_environment(run,cargo_cwd,system_root,tool_directories,rustc):
    run=Path(run); system_root=Path(system_root); rustc=Path(rustc)
    if not all(p.is_absolute() for p in (run,system_root,rustc)): raise ValueError('invalid_environment')
    if not re.fullmatch(r'run-[0-9a-f]{32}',run.name): raise ValueError('invalid_environment')
    _safe_item(run,True)
    if os.path.lexists(run/'.release-quarantine'): raise ValueError('quarantined_run')
    owner=run/'owner'; _safe_item_owner=owner.lstat()
    if not stat.S_ISREG(_safe_item_owner.st_mode) or _safe_item_owner.st_nlink!=1 or getattr(_safe_item_owner,'st_file_attributes',0)&0x400 or _safe_item_owner.st_size!=32 or owner.read_text(encoding='ascii')!=run.name[4:]: raise ValueError('invalid_environment')
    check_cargo_cwd(cargo_cwd)
    for name in ('source','target','temp','cargo-home','empty-home','npm-cache'):_safe_item(run/name,True)
    for name in ('npm-user.conf','npm-global.conf','git-global.conf'):_safe_item(run/name,False)
    for child in (run/'cargo-home').iterdir():
        if child.name=='registry': _safe_item(child,True)
        elif child.name in ('.global-cache','.package-cache','.package-cache-mutate'):
            item=child.lstat()
            if not stat.S_ISREG(item.st_mode) or item.st_nlink!=1 or getattr(item,'st_file_attributes',0)&0x400 or item.st_size>64*1024*1024: raise ValueError('unsafe_cargo_home')
        else: raise ValueError('unsafe_cargo_home')
    dirs=[Path(p) for p in tool_directories]
    if not dirs or any(not p.is_absolute() or ';' in str(p) for p in dirs): raise ValueError('invalid_tool_path')
    for p in dirs:_safe_item(p,True)
    system32=system_root/'System32'; _safe_item(system32,True)
    paths=dirs+[system32]
    return {'SystemRoot':str(system_root),'WINDIR':str(system_root),'ComSpec':str(system32/'cmd.exe'),
            'PATH':os.pathsep.join(map(str,paths)),'TEMP':str(run/'temp'),'TMP':str(run/'temp'),
            'USERPROFILE':str(run/'empty-home'),'HOME':str(run/'empty-home'),'CARGO_HOME':str(run/'cargo-home'),
            'CARGO_TARGET_DIR':str(run/'target'),'CARGO_NET_OFFLINE':'true','CARGO_INCREMENTAL':'0','RUSTC':str(rustc),
            'NPM_CONFIG_USERCONFIG':str(run/'npm-user.conf'),'NPM_CONFIG_GLOBALCONFIG':str(run/'npm-global.conf'),
            'NPM_CONFIG_CACHE':str(run/'npm-cache'),'NPM_CONFIG_OFFLINE':'true','GIT_CONFIG_NOSYSTEM':'1',
            'GIT_CONFIG_GLOBAL':str(run/'git-global.conf'),'GIT_TERMINAL_PROMPT':'0'}
