"""Fetch upstream NSIS archives into an owned stage; no extraction or execution."""
import hashlib
import os
from pathlib import Path
import re
import ssl
import sys
from urllib.parse import urlsplit
from urllib.request import HTTPSHandler, HTTPRedirectHandler, ProxyHandler, Request, build_opener

ROOT=Path(__file__).resolve().parent.parent
sys.path.insert(0,str(ROOT/'scripts'))
from release_launcher import run_job

PACKAGES=(('nsis.zip','https://github.com/tauri-apps/binary-releases/releases/download/nsis-3.13/nsis-3.13.zip','db14f8af2ea346b786c6ae19343f61aaf5e09b8e'),
          ('plugin.dll','https://github.com/tauri-apps/nsis-tauri-utils/releases/download/nsis_tauri_utils-v0.5.3/nsis_tauri_utils.dll','75197fee3c6a814fe035788d1c34ead39349b860'))

class Redirect(HTTPRedirectHandler):
    def redirect_request(self,req,fp,code,msg,headers,newurl):
        p=urlsplit(newurl)
        if p.scheme!='https' or p.hostname not in ('github.com','release-assets.githubusercontent.com','objects.githubusercontent.com') or p.username or p.password or p.port not in (None,443): raise ValueError('redirect_rejected')
        return super().redirect_request(req,fp,code,msg,headers,newurl)

def child(run):
    destination=run/'nsis-downloads'; destination.mkdir()
    opener=build_opener(ProxyHandler({}),HTTPSHandler(context=ssl.create_default_context()),Redirect())
    records=[]
    for name,url,expected in PACKAGES:
        target=destination/name; sha1=hashlib.sha1(usedforsecurity=False); sha256=hashlib.sha256(); size=0
        with opener.open(Request(url,headers={'Accept-Encoding':'identity','User-Agent':'MindTrail-release-probe/1'}),timeout=30) as response, target.open('xb') as out:
            if response.status!=200: raise ValueError('http_failure')
            while True:
                chunk=response.read(1024*1024)
                if not chunk: break
                size+=len(chunk)
                if size>64*1024*1024: raise ValueError('too_large')
                sha1.update(chunk); sha256.update(chunk); out.write(chunk)
        if not size or sha1.hexdigest()!=expected: raise ValueError('checksum_mismatch')
        records.append(name+':'+sha256.hexdigest())
    (run/'nsis-downloads.sha256').write_text('\n'.join(records)+'\n',encoding='ascii')

def main(run_id):
    if not re.fullmatch('[0-9a-f]{32}',run_id): raise ValueError('invalid_run')
    base=ROOT/'.tools/productization-release'; run=base/('run-'+run_id)
    if run.is_symlink() or run.resolve().parent!=base.resolve() or (run/'owner').read_text(encoding='ascii')!=run_id or (run/'nsis-downloads').exists(): raise ValueError('invalid_run')
    windows=Path(os.environ['SystemRoot']); cwd=windows/'System32'
    env={'SystemRoot':str(windows),'WINDIR':str(windows),'PATH':str(cwd),'TEMP':str(run/'temp'),'TMP':str(run/'temp'),'HOME':str(run/'empty-home'),'USERPROFILE':str(run/'empty-home')}
    code=run_job([sys.executable,'-I',str(Path(__file__).resolve()),'--child',run_id],cwd=cwd,env=env,timeout_seconds=180)
    print('nsis_fetch='+('passed' if code==0 else 'failed')); return code
if __name__=='__main__':
    try:
        if sys.argv[1]=='--child':
            if len(sys.argv)!=3 or not re.fullmatch('[0-9a-f]{32}',sys.argv[2]): raise ValueError('invalid_run')
            owned=ROOT/'.tools/productization-release'/('run-'+sys.argv[2])
            if owned.is_symlink() or (owned/'owner').read_text(encoding='ascii')!=sys.argv[2]: raise ValueError('invalid_run')
            child(owned); sys.exit(0)
        sys.exit(main(sys.argv[1]))
    except TimeoutError: print('nsis_fetch=timeout'); sys.exit(1)
    except Exception: print('nsis_fetch=failed'); sys.exit(1)
