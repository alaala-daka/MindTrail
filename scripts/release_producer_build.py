"""Candidate isolated MSVC producer build; run only after independent review."""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import threading
import time

ROOT=Path(__file__).resolve().parent.parent
sys.path.insert(0,str(ROOT/'scripts'))
from release_environment import check_cargo_cwd,build_environment
from release_launcher import run_job
from release_job import OwnedJob
from release_tools import validate_tool

def capture_job(argv,*,cwd,env,timeout,temporary):
    job=OwnedJob(argv,cwd=cwd,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    buffers=[bytearray(),bytearray()]; overflow=threading.Event()
    def read(pipe,index):
        while True:
            block=pipe.read(65536)
            if not block: break
            if len(buffers[index])+len(block)>1048576: overflow.set()
            else: buffers[index].extend(block)
    threads=[threading.Thread(target=read,args=(job.process.stdout,0)),threading.Thread(target=read,args=(job.process.stderr,1))]
    for thread in threads: thread.start()
    deadline=time.monotonic()+timeout; code=None
    try:
        while code is None:
            if overflow.is_set(): raise ValueError('output_limit_exceeded')
            if time.monotonic()>=deadline: raise TimeoutError('child_timeout')
            try: code=job.process.wait(timeout=.1)
            except subprocess.TimeoutExpired: pass
    finally:
        job.close()
        for thread in threads: thread.join(timeout=5)
        for pipe in (job.process.stdout,job.process.stderr): pipe.close()
    if overflow.is_set() or any(thread.is_alive() for thread in threads): raise ValueError('output_limit_exceeded')
    return code,buffers[0].decode(errors='replace'),buffers[1].decode(errors='replace')

def capture_build_job(argv,*,cwd,env,timeout):
    """Keep only fixed diagnostics and a bounded set of compiler codes in memory."""
    phrases={'access_denied':(b'access is denied',b'permission denied'),
             'linker_failure':(b'linking with',b'linker `',b'link.exe failed'),
             'missing_dependency':(b'no matching package named',b'failed to download'),
             'custom_build_failure':(b'failed to run custom build command',),
             'custom_build_mindtrail':(b'failed to run custom build command for `mindtrail v',),
             'missing_tool':(b'could not find',b'not found',b'not recognized'),
             'compile_failure':(b'could not compile',b'error: could not compile'),
             'resource_exhaustion':(b'out of memory',b'no space left on device')}
    flags={name:False for name in phrases}; codes=set(); job=OwnedJob(argv,cwd=cwd,env=env,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
    def read():
        tail=b''
        while True:
            block=job.process.stderr.read(65536)
            if not block: break
            data=(tail+block).lower()
            for name,words in phrases.items():
                if any(word in data for word in words): flags[name]=True
            if re.search(rb"resource path `[^`\r\n]{0,2048}mindtrail-producer\.exe` doesn't exist",data): flags['producer_resource_missing']=True
            for match in re.findall(rb'error\[e([0-9]{4})\]|\blnk([0-9]{4})\b',data):
                if len(codes)<8: codes.add(('E'+match[0].decode()) if match[0] else ('LNK'+match[1].decode()))
            tail=data[-4096:]
    flags['producer_resource_missing']=False
    thread=threading.Thread(target=read); thread.start(); timed_out=False
    try:
        try: code=job.process.wait(timeout=timeout)
        except subprocess.TimeoutExpired: timed_out=True; code=None
    finally:
        job.close(); thread.join(timeout=5); job.process.stderr.close()
    if thread.is_alive(): raise ValueError('diagnostic_cleanup_failed')
    return {'exit_code':code,'timed_out':timed_out,'flags':flags,'codes':sorted(codes)}

def source_check(run,cwd,env,powershell):
    code,out,_=capture_job([str(powershell),'-NoProfile','-File',str(ROOT/'scripts/release-source-verify.ps1'),'-Run',str(run)],cwd=cwd,env=env,timeout=60,temporary=run/'temp')
    if code!=0 or out.strip()!='source_matches=true': raise ValueError('source_changed')

def validate_pe(path):
    info=path.lstat()
    if not path.is_file() or path.is_symlink() or getattr(info,'st_file_attributes',0)&0x400 or info.st_nlink not in (1,2) or not 256<=info.st_size<=268435456: raise ValueError('producer_invalid')
    data=path.read_bytes(); offset=int.from_bytes(data[60:64],'little')
    if data[:2]!=b'MZ' or offset+6>len(data) or data[offset:offset+4]!=b'PE\0\0' or data[offset+4:offset+6]!=b'\x64\x86' or any(x in data for x in (b'MINDTRAIL_TEST_ROOT',b'MINDTRAIL_TEST_OUTPIPE')): raise ValueError('producer_invalid')
    if (info.st_dev,info.st_ino,info.st_size,info.st_mtime_ns)!=(path.stat().st_dev,path.stat().st_ino,path.stat().st_size,path.stat().st_mtime_ns): raise ValueError('producer_invalid')
    if info.st_nlink==2:
        peer=path.parent/'deps'/(path.stem.replace('-','_')+path.suffix)
        try: peer_info=peer.lstat()
        except OSError: raise ValueError('producer_invalid')
        if not stat.S_ISREG(peer_info.st_mode) or getattr(peer_info,'st_file_attributes',0)&0x400 or (peer_info.st_dev,peer_info.st_ino)!=(info.st_dev,info.st_ino) or peer_info.st_nlink!=2: raise ValueError('producer_invalid')
    return hashlib.sha256(data).hexdigest()

def producer_environment(base):
    """Only the producer build omits the resource that it creates."""
    result=dict(base)
    result['TAURI_CONFIG']='{"bundle":{"resources":null}}'
    return result

def main(run_id,inspect=False):
    if not re.fullmatch('[0-9a-f]{32}',run_id): raise ValueError('invalid_run')
    base=ROOT/'.tools/productization-release'; run=base/('run-'+run_id)
    if run.is_symlink() or run.resolve().parent!=base.resolve() or (run/'owner').read_text(encoding='ascii')!=run_id or (run/'.release-quarantine').exists(): raise ValueError('invalid_run')
    windows=Path(os.environ['SystemRoot']); cwd=windows/'System32'; check_cargo_cwd(cwd)
    rust=ROOT/'.tools/rustup/toolchains/stable-x86_64-pc-windows-msvc/bin'
    pwsh=Path(shutil.which('pwsh') or '')
    if not pwsh.is_absolute(): raise ValueError('invalid_tool_path')
    base_env=build_environment(run,cwd,windows,[rust],rust/'rustc.exe')
    system_env={'SystemRoot':str(windows),'WINDIR':str(windows),'ComSpec':str(cwd/'cmd.exe'),
                'PATH':str(cwd),'TEMP':str(run/'temp'),'TMP':str(run/'temp'),
                'USERPROFILE':str(run/'empty-home'),'HOME':str(run/'empty-home')}
    for name in ('SystemDrive','ProgramFiles','ProgramFiles(x86)','ProgramW6432','ProgramData','PROCESSOR_ARCHITECTURE'):
        if name in os.environ: system_env[name]=os.environ[name]
    vswhere=Path(os.environ['ProgramFiles(x86)'])/'Microsoft Visual Studio/Installer/vswhere.exe'
    record_path=run/'producer-toolchain-inspection-v2.json'; record_info=record_path.lstat()
    if not record_path.is_file() or record_path.is_symlink() or record_info.st_nlink!=1 or getattr(record_info,'st_file_attributes',0)&0x400 or record_info.st_size>1048576: raise ValueError('toolchain_changed')
    def unique(pairs):
        result={}
        for key,value in pairs:
            if key in result: raise ValueError('toolchain_changed')
            result[key]=value
        return result
    with record_path.open('r',encoding='utf8') as f: expected=json.load(f,object_pairs_hook=unique)
    if (record_info.st_dev,record_info.st_ino,record_info.st_size,record_info.st_mtime_ns)!=(record_path.stat().st_dev,record_path.stat().st_ino,record_path.stat().st_size,record_path.stat().st_mtime_ns): raise ValueError('toolchain_changed')
    bootstrap={'cargo.exe':rust/'cargo.exe','rustc.exe':rust/'rustc.exe','vswhere.exe':vswhere,'cmd.exe':cwd/'cmd.exe','pwsh.exe':pwsh}
    for name,path in bootstrap.items():
        item=expected['tools'][name]
        validate_tool(path,item['path'],item['sha256'],allow_hardlinks=(name=='cmd.exe'))
    discover_code,discover_out,_=capture_job([str(vswhere),'-latest','-products','*','-requires','Microsoft.VisualStudio.Component.VC.Tools.x86.x64','-property','installationPath'],cwd=cwd,env=system_env,timeout=30,temporary=run/'temp')
    if discover_code!=0: raise ValueError('msvc_discovery_failed')
    lines=discover_out.splitlines()
    if len(lines)!=1: raise ValueError('msvc_discovery_failed')
    vs=Path(lines[0]); vcvars=vs/'VC/Auxiliary/Build/vcvars64.bat'
    if not vs.is_absolute() or not vcvars.is_file() or any(x in str(vcvars) for x in ('"','\r','\n','&','|','<','>','%','!')): raise ValueError('invalid_msvc')
    item=expected['tools']['vcvars64.bat']; validate_tool(vcvars,item['path'],item['sha256'])
    command='"'+str(cwd/'cmd.exe')+'" /d /s /c ""'+str(vcvars)+'" >nul && set"'
    capture_code,capture_out,capture_err=capture_job(command,cwd=cwd,env=system_env,timeout=120,temporary=run/'temp')
    if capture_code!=0:
        output=(capture_out+'\n'+capture_err).lower()
        print('vcvars_exit='+str(capture_code))
        for label,phrase in (('access_denied','access is denied'),('not_found','not found'),('not_recognized','not recognized'),('vsdevcmd','vsdevcmd'),('missing_tools','tools could not be found'),('powershell','powershell'),('where','where'),('findstr','findstr'),('reg','reg.exe'),('vscmd','vscmd')):
            print('vcvars_'+label+'='+str(phrase in output).lower())
        raise ValueError('msvc_environment_failed')
    allowed={'PATH','INCLUDE','LIB','LIBPATH','VSCMD_ARG_TGT_ARCH','VCToolsVersion','WindowsSDKVersion'}
    selected={}
    for line in capture_out.splitlines():
        key,sep,value=line.partition('=')
        if sep and key.upper() in {x.upper() for x in allowed}: selected[key.upper()]=value
    if any(not selected.get(name) for name in ('PATH','INCLUDE','LIB')):
        print('vcvars_missing_required=true'); raise ValueError('msvc_environment_failed')
    tools={}
    for name in ('cl.exe','link.exe','rc.exe','mt.exe'):
        resolved=shutil.which(name,path=selected['PATH'])
        if not resolved: raise ValueError('msvc_tool_missing')
        p=Path(resolved); item=expected['tools'][name]; validate_tool(p,item['path'],item['sha256'])
        tools[name]={'path':str(p),'sha256':hashlib.sha256(p.read_bytes()).hexdigest()}
    env=dict(base_env); env.update({k:v for k,v in selected.items() if k in ('PATH','INCLUDE','LIB','LIBPATH')})
    env.update({'CARGO_HOME':str(run/'cargo-home'),'CARGO_TARGET_DIR':str(run/'target'),'CARGO_NET_OFFLINE':'true','RUSTC':str(rust/'rustc.exe')})
    version_code,version_out,_=capture_job([str(rust/'rustc.exe'),'--version'],cwd=cwd,env=env,timeout=20,temporary=run/'temp')
    if version_code!=0 or not re.fullmatch(r'rustc [0-9]+\.[0-9]+\.[0-9]+ \([0-9a-f]+ [0-9-]+\)',version_out.strip()): raise ValueError('rust_version_invalid')
    for name,path in (('cargo.exe',rust/'cargo.exe'),('rustc.exe',rust/'rustc.exe'),('vswhere.exe',vswhere),('vcvars64.bat',vcvars),('cmd.exe',cwd/'cmd.exe'),('pwsh.exe',pwsh)):
        tools[name]={'path':str(path),'sha256':hashlib.sha256(path.read_bytes()).hexdigest()}
    record={'rustc':version_out.strip(),'msvc_version':selected.get('VCTOOLSVERSION','unknown'),'windows_sdk_version':selected.get('WINDOWSSDKVERSION','unknown'),'tools':tools}
    source_check(run,cwd,env,pwsh)
    if inspect:
        with (run/'producer-toolchain-inspection-v3.json').open('x',encoding='utf8') as f: json.dump(record,f,sort_keys=True)
    else:
        if record!=expected: raise ValueError('toolchain_changed')
        for name,item in tools.items(): validate_tool(item['path'],item['path'],item['sha256'],allow_hardlinks=(name=='cmd.exe'))
    if inspect: print('producer_preflight=passed'); return 0
    output=run/'target/x86_64-pc-windows-msvc/release/mindtrail-producer.exe'
    if output.exists(): raise ValueError('stale_producer')
    args=[str(rust/'cargo.exe'),'build','--locked','--offline','--release','--target','x86_64-pc-windows-msvc','--features','producer','--bin','mindtrail-producer','--manifest-path',str(run/'source/src-tauri/Cargo.toml')]
    try:
        diagnostic=capture_build_job(args,cwd=cwd,env=producer_environment(env),timeout=1800)
        with (run/'producer-diagnostic.json').open('x',encoding='utf8') as f: json.dump(diagnostic,f,sort_keys=True)
        if diagnostic['timed_out'] or diagnostic['exit_code']!=0: raise ValueError('producer_build_failed')
        digest=validate_pe(output); source_check(run,cwd,env,pwsh)
        for name,item in tools.items(): validate_tool(item['path'],item['path'],item['sha256'],allow_hardlinks=(name=='cmd.exe'))
        with (run/'producer-build.json').open('x',encoding='utf8') as f: json.dump({'sha256':digest,'target':'x86_64-pc-windows-msvc','features':['producer']},f,sort_keys=True)
        print('producer_build=passed'); return 0
    except BaseException:
        try:
            with (run/'.release-quarantine').open('x',encoding='ascii') as f: f.write('failed')
        except FileExistsError: pass
        raise
if __name__=='__main__':
    try: sys.exit(main(sys.argv[1],sys.argv[2:]==['--inspect']))
    except TimeoutError: print('producer_build=timeout'); sys.exit(1)
    except Exception as error:
        fixed={'invalid_run','msvc_discovery_failed','invalid_msvc','msvc_environment_failed','msvc_tool_missing','rust_version_invalid','stale_producer','producer_missing','unsafe_environment_path','invalid_environment','unsafe_cargo_home','invalid_tool_path','quarantined_run','ancestor_config_present','invalid_cwd','source_changed','toolchain_changed','producer_invalid','producer_build_failed'}
        print('producer_error='+(str(error) if str(error) in fixed else type(error).__name__)); sys.exit(1)
