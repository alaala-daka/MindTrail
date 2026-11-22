"""Candidate desktop/NSIS run for an independently reviewed owned stage."""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import sys

ROOT=Path(__file__).resolve().parent.parent
sys.path.insert(0,str(ROOT/'scripts')); sys.path.insert(0,str(ROOT/'tests'))
from release_desktop_preflight import main as desktop_preflight,approved_pwsh
from release_environment import build_environment
from release_launcher import run_job
from release_producer_build import capture_job,source_check,validate_pe
from release_desktop_diagnostics import capture_desktop_job
from release_tools import validate_tool
from release_nsis_readiness import inspect_owned_toolset

APPROVED_RUN='02bb4e8bbebd45da906552912bc6cb2a'
APPROVED_DIST='b4b73eb70ff1b44c3c7a1729eccbc4387db2411654904feea189d491d0c404ab'
RECORD_PINS={'producer-toolchain-inspection-v2.json':'1f2a139c0b4f388ae9e4c764cd934259232b1e450b8140e36b16478ad52d7c44',
             'desktop-toolchain-inspection.json':'e87bdf1c879b79c8ce828fd2d01637fa99bc84bde17a016b624221d11d06d889',
             'nsis-toolset.sha256.json':'1db5b3b80075d0c774074ea14b9303bab4791bfe055c57e30cb7bc160bbb520e',
             'source-inputs.json':'845e56571c5e8a154371fb84faba67ab0b09185a1e264fb097f57459b162d985',
             'producer-build.json':'4d2c9877f9698ef13f3feefe9f1e8e22a3b8fc2639e101eec92e71851608b004',
             'producer-resource.json':'8fa4c97e539ab7288d7ebd2d9a9650455eb36501dd0b4dcd93cf744a690acf1a'}

def read_pinned_json(path,expected):
    info=path.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_nlink!=1 or getattr(info,'st_file_attributes',0)&0x400 or info.st_size>1048576: raise ValueError('invalid_record')
    with path.open('rb') as f:
        before=os.fstat(f.fileno()); data=f.read(1048577); after=os.fstat(f.fileno())
    identity=lambda s:(s.st_dev,s.st_ino,s.st_size,s.st_mtime_ns)
    if identity(info)!=identity(before) or identity(before)!=identity(after) or identity(after)!=identity(path.lstat()) or hashlib.sha256(data).hexdigest()!=expected: raise ValueError('invalid_record')
    def unique(pairs):
        result={}
        for key,value in pairs:
            if key in result: raise ValueError('invalid_record')
            result[key]=value
        return result
    return json.loads(data,object_pairs_hook=unique)

def validate_built_pe(path):
    info=path.lstat()
    if not stat.S_ISREG(info.st_mode) or getattr(info,'st_file_attributes',0)&0x400 or info.st_nlink not in (1,2) or not 256<=info.st_size<=268435456: raise ValueError('desktop_invalid')
    data=path.read_bytes(); offset=int.from_bytes(data[60:64],'little')
    if data[:2]!=b'MZ' or offset+6>len(data) or data[offset:offset+4]!=b'PE\0\0' or data[offset+4:offset+6]!=b'\x64\x86' or any(x in data for x in (b'MINDTRAIL_TEST_ROOT',b'MINDTRAIL_TEST_OUTPIPE')): raise ValueError('desktop_invalid')
    if (info.st_dev,info.st_ino,info.st_size,info.st_mtime_ns)!=(path.stat().st_dev,path.stat().st_ino,path.stat().st_size,path.stat().st_mtime_ns): raise ValueError('desktop_invalid')
    if info.st_nlink==2:
        peer=path.parent/'deps'/path.name
        try: peer_info=peer.lstat()
        except OSError: raise ValueError('desktop_invalid')
        if not stat.S_ISREG(peer_info.st_mode) or getattr(peer_info,'st_file_attributes',0)&0x400 or (peer_info.st_dev,peer_info.st_ino)!=(info.st_dev,info.st_ino) or peer_info.st_nlink!=2: raise ValueError('desktop_invalid')
    return hashlib.sha256(data).hexdigest()

def validate_installer(path):
    info=path.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_nlink!=1 or getattr(info,'st_file_attributes',0)&0x400 or not 256<=info.st_size<=536870912: raise ValueError('installer_invalid')
    with path.open('rb') as f:
        before=os.fstat(f.fileno()); header=f.read(64)
        if len(header)!=64 or header[:2]!=b'MZ': raise ValueError('installer_invalid')
        offset=int.from_bytes(header[60:64],'little')
        if offset>info.st_size-6: raise ValueError('installer_invalid')
        f.seek(offset); signature=f.read(6); after=os.fstat(f.fileno())
    if signature[:4]!=b'PE\0\0' or signature[4:6] not in (b'\x4c\x01',b'\x64\x86') or (info.st_dev,info.st_ino,info.st_size,info.st_mtime_ns)!=(after.st_dev,after.st_ino,after.st_size,after.st_mtime_ns) or (before.st_dev,before.st_ino)!=(info.st_dev,info.st_ino): raise ValueError('installer_invalid')

def validate_nsis_script(text,allowed_sources,producer_hash,desktop_path):
    lines=[line.strip() for line in text.splitlines() if line.strip() and not line.lstrip().startswith(';')]
    def section(name):
        starts=[i for i,line in enumerate(lines) if re.fullmatch(r'Section\s+'+name,line,re.IGNORECASE)]
        if len(starts)!=1: raise ValueError('package_static_failed')
        ends=[i for i in range(starts[0]+1,len(lines)) if re.fullmatch(r'SectionEnd',lines[i],re.IGNORECASE)]
        if not ends: raise ValueError('package_static_failed')
        return lines[starts[0]+1:ends[0]]
    def define(name):
        values=[match.group(1) for line in lines for match in [re.fullmatch(r'!define\s+'+re.escape(name)+r'\s+"([^"]*)"',line,re.IGNORECASE)] if match]
        if len(values)!=1: raise ValueError('package_static_failed')
        return values[0]
    install=section('Install'); uninstall=section('Uninstall')
    if define('MAINBINARYNAME')!='mindtrail' or define('INSTALLMODE')!='currentUser': raise ValueError('package_static_failed')
    if 'StrCpy $INSTDIR "$LOCALAPPDATA\\${PRODUCTNAME}"' not in text: raise ValueError('package_static_failed')
    expected=os.path.normcase(os.path.normpath(str(desktop_path)))
    desktop=Path(define('MAINBINARYSRCPATH'))
    if not desktop.is_absolute() or os.path.normcase(os.path.normpath(str(desktop)))!=expected: raise ValueError('package_static_failed')
    validate_built_pe(desktop)
    if sum(bool(re.fullmatch(r'File\s+"\$\{MAINBINARYSRCPATH\}"',line,re.IGNORECASE)) for line in install)!=1: raise ValueError('package_static_failed')
    if sum(bool(re.fullmatch(r'CreateDirectory\s+"\$INSTDIR[\\/]producer"',line,re.IGNORECASE)) for line in install)!=1: raise ValueError('package_static_failed')
    if sum(bool(re.fullmatch(r'WriteUninstaller\s+"\$INSTDIR[\\/]uninstall\.exe"',line,re.IGNORECASE)) for line in install)!=1: raise ValueError('package_static_failed')
    matchers=[]
    pattern=re.compile(r'^File\s+/a\s+"/oname=producer[\\/]mindtrail-producer\.exe"\s+"([^"\r\n]+)"$',re.IGNORECASE)
    for i,line in enumerate(install):
        match=pattern.fullmatch(line)
        if match: matchers.append((i,match.group(1)))
    if len(matchers)!=1: raise ValueError('package_static_failed')
    index,source=matchers[0]; outpaths=[line for line in install[:index] if re.match(r'^SetOutPath\b',line,re.IGNORECASE)]
    if not outpaths or not re.fullmatch(r'SetOutPath\s+"?\$INSTDIR"?',outpaths[-1],re.IGNORECASE): raise ValueError('package_static_failed')
    source_path=Path(source)
    if not source_path.is_absolute() or not any(os.path.normcase(os.path.normpath(str(source_path)))==os.path.normcase(os.path.normpath(str(p))) for p in allowed_sources): raise ValueError('package_static_failed')
    if validate_pe(source_path)!=producer_hash: raise ValueError('package_static_failed')
    deletes=(r'Delete\s+"\$INSTDIR[\\/]\$\{MAINBINARYNAME\}\.exe"',r'Delete\s+"\$INSTDIR[\\/]producer[\\/]mindtrail-producer\.exe"',r'Delete\s+"\$INSTDIR[\\/]uninstall\.exe"')
    if any(sum(bool(re.fullmatch(rule,line,re.IGNORECASE)) for line in uninstall)!=1 for rule in deletes): raise ValueError('package_static_failed')
    if sum(bool(re.fullmatch(r'RMDir\s+/REBOOTOK\s+"\$INSTDIR[\\/]producer"',line,re.IGNORECASE)) for line in uninstall)!=1: raise ValueError('package_static_failed')
    shortcuts=(r'CreateShortcut\s+"\$SMPROGRAMS[^\r\n]*\$\{PRODUCTNAME\}\.lnk"\s+"\$INSTDIR[\\/]\$\{MAINBINARYNAME\}\.exe"',r'CreateShortcut\s+"\$DESKTOP[^\r\n]*\$\{PRODUCTNAME\}\.lnk"\s+"\$INSTDIR[\\/]\$\{MAINBINARYNAME\}\.exe"')
    if any(not re.search(rule,text,re.IGNORECASE) for rule in shortcuts): raise ValueError('package_static_failed')
    return True

def fingerprint_tree(root):
    root=Path(root); digest=hashlib.sha256(); items=[]
    if not root.is_dir() or root.is_symlink() or getattr(root.lstat(),'st_file_attributes',0)&0x400: raise ValueError('invalid_dist')
    def fail(error): raise error
    for current,dirs,files in os.walk(root,followlinks=False,onerror=fail):
        for name in dirs:
            info=(Path(current)/name).lstat()
            if not stat.S_ISDIR(info.st_mode) or getattr(info,'st_file_attributes',0)&0x400: raise ValueError('invalid_dist')
        for name in files: items.append(Path(current)/name)
    if not items: raise ValueError('invalid_dist')
    for path in sorted(items,key=lambda p:p.relative_to(root).as_posix()):
        info=path.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_nlink!=1 or getattr(info,'st_file_attributes',0)&0x400 or info.st_size>64*1024*1024: raise ValueError('invalid_dist')
        name=path.relative_to(root).as_posix(); digest.update((name+'\0'+str(info.st_size)+'\0').encode())
        with path.open('rb') as f:
            before=os.fstat(f.fileno())
            if (before.st_dev,before.st_ino)!=(info.st_dev,info.st_ino): raise ValueError('invalid_dist')
            for block in iter(lambda:f.read(1024*1024),b''): digest.update(block)
            after=os.fstat(f.fileno())
        last=path.lstat()
        identity=lambda x:(x.st_dev,x.st_ino,x.st_size,x.st_mtime_ns)
        if getattr(last,'st_file_attributes',0)&0x400 or last.st_nlink!=1 or identity(info)!=identity(before) or identity(before)!=identity(after) or identity(after)!=identity(last): raise ValueError('invalid_dist')
    return digest.hexdigest()

def run_command(argv,cwd,env,timeout):
    if run_job([str(x) for x in argv],cwd=cwd,env=env,timeout_seconds=timeout)!=0: raise ValueError('command_failed')

def main(run_id):
    if run_id!=APPROVED_RUN: raise ValueError('invalid_run')
    run=ROOT/'.tools/productization-release'/('run-'+run_id)
    sealed={name:read_pinned_json(run/name,digest) for name,digest in RECORD_PINS.items()}
    if desktop_preflight(run_id,True)!=0: raise ValueError('preflight_failed')
    base=ROOT/'.tools/productization-release'; stage=run/'source'
    release=run/'target/x86_64-pc-windows-msvc/release'
    for old in (release/'mindtrail.exe',release/'bundle/nsis/MindTrail_0.1.0_x64-setup.exe',release/'nsis/x64/installer.nsi'):
        if os.path.lexists(old): raise ValueError('stale_desktop_artifact')
    dist_pin=(run/'frontend-dist.sha256').read_text(encoding='ascii').strip()
    if dist_pin!=APPROVED_DIST or fingerprint_tree(stage/'dist')!=APPROVED_DIST: raise ValueError('frontend_dist_changed')
    producer_digest=sealed['producer-build.json']['sha256']
    nsis_pins=sealed['nsis-toolset.sha256.json']
    windows=Path(os.environ['SystemRoot']); cwd=windows/'System32'; rust=ROOT/'.tools/rustup/toolchains/stable-x86_64-pc-windows-msvc/bin'
    node_dir=Path(os.environ['ProgramFiles'])/'nodejs'
    desktop=sealed['desktop-toolchain-inspection.json']; producer=sealed['producer-toolchain-inspection-v2.json']
    pwsh=approved_pwsh(os.environ['ProgramFiles'],producer)
    expected={'node':node_dir/'node.exe','csc':windows/'Microsoft.NET/Framework64/v4.0.30319/csc.exe',
              'tauri_js':stage/'node_modules/@tauri-apps/cli/tauri.js',
              'tauri_native':stage/'node_modules/@tauri-apps/cli-win32-x64-msvc/cli.win32-x64-msvc.node'}
    if set(desktop)!=set(expected): raise ValueError('invalid_toolchain')
    for name,item in desktop.items(): validate_tool(item['path'],expected[name],item['sha256'],allow_hardlinks=(name=='csc'))
    for name,item in producer['tools'].items(): validate_tool(item['path'],item['path'],item['sha256'],allow_hardlinks=(name=='cmd.exe' or name=='pwsh.exe'))
    env=build_environment(run,cwd,windows,[rust,node_dir],rust/'rustc.exe')
    for name in ('SystemDrive','ProgramFiles','ProgramFiles(x86)','ProgramW6432','ProgramData','PROCESSOR_ARCHITECTURE'):
        if name in os.environ: env[name]=os.environ[name]
    vcvars=Path(producer['tools']['vcvars64.bat']['path'])
    if any(x in str(vcvars) for x in ('"','\r','\n','&','|','<','>','%','!')): raise ValueError('invalid_msvc')
    command='"'+str(cwd/'cmd.exe')+'" /d /s /c ""'+str(vcvars)+'" >nul && set"'
    code,out,_=capture_job(command,cwd=cwd,env=env,timeout=120,temporary=run/'temp')
    if code!=0: raise ValueError('msvc_failed')
    selected={}
    for line in out.splitlines():
        key,sep,value=line.partition('=')
        if sep and key.upper() in ('PATH','INCLUDE','LIB','LIBPATH'): selected[key.upper()]=value
    if any(not selected.get(n) for n in ('PATH','INCLUDE','LIB')): raise ValueError('msvc_failed')
    for name in ('cl.exe','link.exe','rc.exe','mt.exe'):
        item=producer['tools'][name]; resolved=shutil.which(name,path=selected['PATH'])
        if not resolved: raise ValueError('msvc_failed')
        validate_tool(resolved,item['path'],item['sha256'])
    env.update(selected)
    guard_dir=run/'desktop-guards'; guard_dir.mkdir()
    csc=desktop['csc']['path']; guard=guard_dir/'cargo.exe'; wrapper=guard_dir/'cargo-build.exe'
    try:
        for source,dest in (('release-cargo-preflight.cs',guard),('release-cargo-wrapper.cs',wrapper)):
            run_command([csc,'/noconfig','/nologo','/reference:System.dll','/out:'+str(dest),stage/'scripts'/source],cwd,env,60)
        guard_hashes={p:hashlib.sha256(p.read_bytes()).hexdigest() for p in (guard,wrapper)}
        for p,digest in guard_hashes.items(): validate_tool(p,p,digest)
        env['PATH']=str(guard_dir)+os.pathsep+env['PATH']
        env['MINDTRAIL_CARGO_EXE']=str(rust/'cargo.exe'); env['MINDTRAIL_SOURCE_ROOT']=str(stage); env['MINDTRAIL_CARGO_CWD']=str(cwd)
        override={'build':{'beforeBuildCommand':None,'runner':{'cmd':str(wrapper),'cwd':str(cwd),'args':[]}}}
        args=[desktop['node']['path'],desktop['tauri_js']['path'],'build','--target','x86_64-pc-windows-msvc','--bundles','nsis','--no-sign','--ci','--config',json.dumps(override,separators=(',',':'))]
        diagnostic=capture_desktop_job([str(x) for x in args],cwd=stage,env=env,timeout=1800)
        with (run/'desktop-diagnostic.json').open('x',encoding='utf8') as f: json.dump(diagnostic,f,sort_keys=True)
        if diagnostic['timed_out'] or diagnostic['exit_code']!=0: raise ValueError('desktop_build_failed')
        source_check(run,cwd,env,pwsh)
        if fingerprint_tree(stage/'dist')!=dist_pin: raise ValueError('frontend_dist_changed')
        if inspect_owned_toolset(run,base,nsis_pins)!='verified_candidate': raise ValueError('nsis_changed')
        if validate_pe(release/'mindtrail-producer.exe')!=producer_digest or validate_pe(stage/'src-tauri/resources/producer/mindtrail-producer.exe')!=producer_digest: raise ValueError('producer_changed')
        for name,item in desktop.items(): validate_tool(item['path'],expected[name],item['sha256'],allow_hardlinks=(name=='csc'))
        for p,digest in guard_hashes.items(): validate_tool(p,p,digest)
        release=run/'target/x86_64-pc-windows-msvc/release'
        desktop_exe=release/'mindtrail.exe'; installer=release/'bundle/nsis/MindTrail_0.1.0_x64-setup.exe'; nsi=release/'nsis/x64/installer.nsi'
        validate_built_pe(desktop_exe)
        validate_installer(installer)
        script=nsi.read_text(encoding='utf8')
        validate_nsis_script(script,[stage/'src-tauri/resources/producer/mindtrail-producer.exe',release/'producer/mindtrail-producer.exe'],producer_digest,desktop_exe)
        provenance=sealed['source-inputs.json']
        artifacts={'desktop/MindTrail.exe':desktop_exe,'producer/mindtrail-producer.exe':release/'mindtrail-producer.exe','installer/MindTrail-setup.exe':installer,'nsis/installer.nsi':nsi}
        output=run/'release'; files=[]
        for name,source in artifacts.items():
            dest=output/name; dest.parent.mkdir(parents=True,exist_ok=True)
            with source.open('rb') as src,dest.open('xb') as dst: shutil.copyfileobj(src,dst)
            digest=hashlib.sha256(source.read_bytes()).hexdigest()
            if digest!=hashlib.sha256(dest.read_bytes()).hexdigest(): raise ValueError('artifact_copy_failed')
            if name=='desktop/MindTrail.exe': validate_pe(dest)
            if name=='installer/MindTrail-setup.exe': validate_installer(dest)
            files.append({'path':name,'size':dest.stat().st_size,'sha256':digest})
        manifest={'version':'0.1.0','target':'x86_64-pc-windows-msvc','source_commit':provenance['source_commit'],'source_dirty':provenance['source_dirty'],'source_digest':provenance['source_digest'],'files':files,'signing':'unsigned_candidate','checks':{'build':'passed','package_static':'passed','installation':'not_run','upgrade':'not_run','uninstall':'not_run','real_agent':'not_run','semantic_provider':'not_run'}}
        with (output/'manifest.json').open('x',encoding='utf8') as f: json.dump(manifest,f,sort_keys=True)
        run_command([pwsh,'-NoProfile','-File',stage/'scripts/verify-release.ps1','-ReleaseDirectory',output],cwd,env,60)
        source_check(run,cwd,env,pwsh)
        print('desktop_release=passed'); return 0
    except BaseException:
        try:
            with (run/'.release-quarantine').open('x',encoding='ascii') as f: f.write('failed')
        except FileExistsError: pass
        raise
if __name__=='__main__':
    try: sys.exit(main(sys.argv[1]))
    except TimeoutError: print('desktop_release=timeout'); sys.exit(1)
    except Exception: print('desktop_release=failed'); sys.exit(1)
