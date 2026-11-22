"""Candidate fixed-tool admission; it does not select a tool or launch one."""
import hashlib
import os
from pathlib import Path
import re
import stat

def validate_tool(path, expected_path, expected_sha256, *, allow_hardlinks=False):
    if not isinstance(expected_sha256,str) or not re.fullmatch('[0-9a-f]{64}',expected_sha256): raise ValueError('tool_pin_invalid')
    candidate=Path(path); expected=Path(expected_path)
    if not candidate.is_absolute() or not expected.is_absolute() or os.path.normcase(os.path.normpath(str(candidate)))!=os.path.normcase(os.path.normpath(str(expected))): raise ValueError('tool_path_invalid')
    for item in (candidate,*candidate.parents):
        info=item.lstat()
        if stat.S_ISLNK(info.st_mode) or getattr(info,'st_file_attributes',0)&0x400: raise ValueError('tool_link')
    before=candidate.stat()
    if not stat.S_ISREG(before.st_mode) or (before.st_nlink!=1 and not allow_hardlinks) or before.st_size==0 or before.st_size>256*1024*1024: raise ValueError('tool_file_invalid')
    digest=hashlib.sha256()
    with candidate.open('rb') as file:
        opened=os.fstat(file.fileno())
        if (opened.st_dev,opened.st_ino)!=(before.st_dev,before.st_ino): raise ValueError('tool_changed')
        while True:
            block=file.read(1024*1024)
            if not block: break
            digest.update(block)
        after=os.fstat(file.fileno())
    current=candidate.stat()
    identity=lambda s:(s.st_dev,s.st_ino,s.st_size,s.st_mtime_ns)
    if identity(before)!=identity(after) or identity(before)!=identity(current) or digest.hexdigest()!=expected_sha256: raise ValueError('tool_changed')
    return True
