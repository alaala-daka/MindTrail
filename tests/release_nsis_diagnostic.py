"""One owned NSIS reproduction; only sanitized failure lines are returned."""
import hashlib
import json
import os
from pathlib import Path
import re
import sys
import uuid

ROOT=Path(__file__).resolve().parent.parent
sys.path.insert(0,str(ROOT/'scripts')); sys.path.insert(0,str(ROOT/'tests'))
from release_nsis_readiness import inspect_owned_toolset
from release_producer_build import capture_job
from release_transfer_prepared import transfer
from release_desktop_candidate import fingerprint_tree

RUN_ID='24e310e5772e442da309a81162d3c712'
PIN='1db5b3b80075d0c774074ea14b9303bab4791bfe055c57e30cb7bc160bbb520e'
SCRIPT_PIN='f9d182c8e66c79fead02c44cc3008f92ff904eaa8817d8a758b6b3621f6b6707'

def transform_script(text,outfile):
    if not Path(outfile).is_absolute() or any(x in str(outfile) for x in ('"','\r','\n','$')): raise ValueError('invalid_target')
    if not re.search(r'^\s*!define\s+UNINSTALLERSIGNCOMMAND\s+""\s*$',text,re.IGNORECASE|re.MULTILINE): raise ValueError('unsafe_script')
    text,removed=re.subn(r"^\s*!uninstfinalize\s+'\$\{UNINSTALLERSIGNCOMMAND\}'\s*$",'',text,flags=re.IGNORECASE|re.MULTILINE)
    if removed!=1: raise ValueError('unsafe_script')
    if re.search(r'^\s*!(?:system|execute|finalize|uninstfinalize)\b',text,re.IGNORECASE|re.MULTILINE): raise ValueError('unsafe_script')
    replacement='!define OUTFILE "'+str(outfile)+'"'
    text,count=re.subn(r'^\s*!define\s+OUTFILE\s+"[^"\r\n]*"\s*$',lambda _:replacement,text,flags=re.IGNORECASE|re.MULTILINE)
    if count!=1: raise ValueError('invalid_script')
    return text

def main():
    base=ROOT/'.tools/productization-release'; run=base/('run-'+RUN_ID)
    if (run/'owner').read_text(encoding='ascii')!=RUN_ID or not (run/'.release-quarantine').exists(): raise ValueError('invalid_run')
    pin_file=run/'nsis-toolset.sha256.json'; data=pin_file.read_bytes()
    if hashlib.sha256(data).hexdigest()!=PIN: raise ValueError('invalid_toolset')
    if inspect_owned_toolset(run,base,json.loads(data))!='verified_candidate': raise ValueError('invalid_toolset')
    source=run/'target/x86_64-pc-windows-msvc/release/nsis/x64'
    if fingerprint_tree(source)!=SCRIPT_PIN: raise ValueError('script_changed')
    for includes in (source,run/'target/.tauri/NSIS'):
        for path in includes.rglob('*.nsh'):
            if re.search(r'^\s*!(?:system|execute|finalize|uninstfinalize)\b',path.read_text(encoding='utf8',errors='replace'),re.IGNORECASE|re.MULTILINE): raise ValueError('unsafe_include')
    scratch=ROOT/'.tools'/('nsis-diagnostic-'+uuid.uuid4().hex); scratch.mkdir()
    temporary=scratch/'temp'; temporary.mkdir(); work=scratch/'work'
    transfer(source,work)
    if fingerprint_tree(source)!=SCRIPT_PIN or fingerprint_tree(work)!=SCRIPT_PIN: raise ValueError('script_changed')
    script=work/'installer.nsi'; text=transform_script(script.read_text(encoding='utf8'),scratch/'synthetic-installer.exe')
    script.write_text(text,encoding='utf8')
    windows=Path(os.environ['SystemRoot']); cwd=windows/'System32'
    env={'SystemRoot':str(windows),'WINDIR':str(windows),'PATH':str(cwd),'TEMP':str(temporary),'TMP':str(temporary),'USERPROFILE':str(scratch),'HOME':str(scratch)}
    exe=run/'target/.tauri/NSIS/makensis.exe'
    code,out,err=capture_job([str(exe),'-NOCONFIG','-INPUTCHARSET','UTF8','-OUTPUTCHARSET','UTF8','-V2',str(script)],cwd=work,env=env,timeout=120,temporary=temporary)
    if fingerprint_tree(source)!=SCRIPT_PIN or inspect_owned_toolset(run,base,json.loads(data))!='verified_candidate': raise ValueError('source_changed')
    # Only fixed error classes are emitted. Unknown lines remain undisclosed.
    content=(out+'\n'+err).lower()
    print('nsis_diagnostic_exit='+str(code))
    for tag,needle in (('script_error','error in script'),('cannot_open','can\'t open'),('file_not_found','file not found'),('access_denied','access is denied'),('plugin_failure','plugin not found'),('invalid_command','invalid command'),('output_failure','error opening output'),('utf8_failure','invalid utf-8')):
        print('nsis_'+tag+'='+str(needle in content).lower())
    print('synthetic_installer_created='+str((scratch/'synthetic-installer.exe').exists()).lower())
    return 0
if __name__=='__main__':
    try: sys.exit(main())
    except Exception as error:
        print('nsis_diagnostic_exception='+type(error).__name__)
        value=getattr(error,'winerror',None)
        if isinstance(value,int) and 0<=value<=65535: print('nsis_diagnostic_winerror='+str(value))
        fixed={'invalid_run','invalid_toolset','invalid_target','unsafe_script','unsafe_include','invalid_script','script_changed','source_changed','output_limit_exceeded','job_create_failed','job_setup_failed','job_assign_failed','job_resume_failed'}
        if str(error) in fixed: print('nsis_diagnostic_category='+str(error))
        sys.exit(1)
