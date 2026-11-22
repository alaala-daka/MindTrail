"""Fail-closed argument policy; this module does not start a build."""
import ntpath
from pathlib import PureWindowsPath

TARGET='x86_64-pc-windows-msvc'

def cargo_arguments(source_root, arguments):
    if not isinstance(source_root,str) or not source_root or '\x00' in source_root or '"' in source_root:
        raise ValueError('invalid_source_root')
    path=PureWindowsPath(source_root)
    if not path.is_absolute() or path.drive.startswith('\\\\') or any(x in ('.','..') for x in path.parts):
        raise ValueError('invalid_source_root')
    manifest=ntpath.join(str(path),'src-tauri','Cargo.toml')
    if type(arguments) is not list or not arguments or arguments[0]!='build':
        raise ValueError('invalid_cargo_arguments')
    result=['build','--manifest-path',manifest,'--locked','--offline']
    seen=set(); i=1
    while i<len(arguments):
        arg=arguments[i]
        if arg in ('--release','--bins') and arg not in seen:
            seen.add(arg); result.append(arg); i+=1
        elif arg=='--target' and arg not in seen and i+1<len(arguments) and arguments[i+1]==TARGET:
            seen.add(arg); result.extend((arg,TARGET)); i+=2
        elif arg=='--features' and arg not in seen and i+1<len(arguments) and arguments[i+1]=='tauri/custom-protocol':
            seen.add(arg); result.extend((arg,'tauri/custom-protocol')); i+=2
        else:
            raise ValueError('invalid_cargo_arguments')
    if '--release' not in seen or '--target' not in seen:
        raise ValueError('invalid_cargo_arguments')
    return result
