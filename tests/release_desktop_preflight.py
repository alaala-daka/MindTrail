"""Read-only admission evidence for an owned desktop packaging stage."""
import hashlib
import json
import os
from pathlib import Path
import re
import sys

ROOT=Path(__file__).resolve().parent.parent
sys.path.insert(0,str(ROOT/'scripts'))
from release_environment import build_environment
from release_nsis_readiness import inspect_owned_toolset
from release_producer_build import source_check,validate_pe

PWSH_SHA256='2a6769a3330dc1084d0154bfde7d164ab750ca7f700f313f9d417c1be78b2233'
PWSH_PACKAGE='OpenAI.CodexPrimaryRuntime.v26-1119-907-0_26.1119.907.0_x64__3k8sg7r9htsxt'

def approved_pwsh(program_files,record):
    expected=Path(program_files)/'WindowsApps'/PWSH_PACKAGE/'dependencies/native/powershell/pwsh.exe'
    item=record['tools']['pwsh.exe']
    if Path(item['path'])!=expected or item['sha256']!=PWSH_SHA256: raise ValueError('pwsh_pin_mismatch')
    from release_tools import validate_tool
    validate_tool(expected,expected,PWSH_SHA256,allow_hardlinks=True)
    return expected

def main(run_id,verify=False):
    if not re.fullmatch('[0-9a-f]{32}',run_id): raise ValueError('invalid_run')
    base=ROOT/'.tools/productization-release'; run=base/('run-'+run_id)
    if run.is_symlink() or run.resolve().parent!=base.resolve() or (run/'owner').read_text(encoding='ascii')!=run_id or (run/'.release-quarantine').exists(): raise ValueError('invalid_run')
    windows=Path(os.environ['SystemRoot']); cwd=windows/'System32'; rust=ROOT/'.tools/rustup/toolchains/stable-x86_64-pc-windows-msvc/bin'
    env=build_environment(run,cwd,windows,[rust],rust/'rustc.exe')
    tool_record=json.loads((run/'producer-toolchain-inspection-v2.json').read_text(encoding='utf8'))
    from release_tools import validate_tool
    pwsh=approved_pwsh(os.environ['ProgramFiles'],tool_record)
    source_check(run,cwd,env,pwsh)
    record=json.loads((run/'producer-build.json').read_text(encoding='utf8'))
    binary=run/'target/x86_64-pc-windows-msvc/release/mindtrail-producer.exe'
    resource=run/'source/src-tauri/resources/producer/mindtrail-producer.exe'
    if validate_pe(binary)!=record['sha256'] or validate_pe(resource)!=record['sha256']: raise ValueError('producer_mismatch')
    nsis=json.loads((run/'nsis-toolset.sha256.json').read_text(encoding='utf8'))
    if inspect_owned_toolset(run,base,nsis)!='verified_candidate': raise ValueError('nsis_invalid')
    source=run/'source'; node=Path(os.environ['ProgramFiles'])/'nodejs/node.exe'
    tools={'node':node,'csc':windows/'Microsoft.NET/Framework64/v4.0.30319/csc.exe',
           'tauri_js':source/'node_modules/@tauri-apps/cli/tauri.js',
           'tauri_native':source/'node_modules/@tauri-apps/cli-win32-x64-msvc/cli.win32-x64-msvc.node'}
    output={}
    for name,path in tools.items():
        info=path.lstat()
        if not path.is_file() or path.is_symlink() or getattr(info,'st_file_attributes',0)&0x400 or (info.st_nlink!=1 and name!='csc') or not 0<info.st_size<=256*1024*1024:
            print('invalid_tool_role='+name); raise ValueError('invalid_tool')
        digest=hashlib.sha256(path.read_bytes()).hexdigest()
        validate_tool(path,path,digest,allow_hardlinks=(name=='csc'))
        output[name]={'path':str(path),'sha256':digest}
    inspection=run/'desktop-toolchain-inspection.json'
    if verify:
        info=inspection.lstat()
        if not inspection.is_file() or inspection.is_symlink() or info.st_nlink!=1 or getattr(info,'st_file_attributes',0)&0x400 or info.st_size>1048576: raise ValueError('invalid_record')
        with inspection.open('r',encoding='utf8') as f: prior=json.load(f)
        if prior!=output: raise ValueError('tool_changed')
    else:
        with inspection.open('x',encoding='utf8') as f: json.dump(output,f,sort_keys=True)
    print('desktop_preflight=passed'); return 0
if __name__=='__main__':
    try: sys.exit(main(sys.argv[1],sys.argv[2:]==['--verify']))
    except Exception as error:
        allowed={'invalid_run','producer_mismatch','nsis_invalid','invalid_tool','source_changed','unsafe_cargo_home','invalid_environment','unsafe_environment_path','quarantined_run','tool_file_invalid','tool_changed','tool_link','tool_path_invalid','invalid_cwd','ancestor_config_present','invalid_tool_path','pwsh_pin_mismatch'}
        print('desktop_error='+(str(error) if str(error) in allowed else type(error).__name__)); sys.exit(1)
