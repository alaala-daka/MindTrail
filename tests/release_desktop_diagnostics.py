"""Bounded fixed-category Tauri diagnostics; raw output is never retained."""
import re
import subprocess
import threading
from release_job import OwnedJob

PHRASES={'makensis_failed':(b'failed to bundle app with makensis',b'failed to run makensis'),
         'nsis_script_error':(b'error in script',b'error: resolving install directory'),
         'nsis_tool_missing':(b'makensis.exe not found',b'failed to run makensis.exe'),
         'nsis_download_failure':(b'failed to download',b'error downloading'),
         'access_denied':(b'access is denied',b'permission denied'),
         'resource_missing':(b'resource path',b'resource not found'),
         'custom_build_failure':(b'failed to run custom build command',),
         'compile_failure':(b'could not compile',),
         'linker_failure':(b'linking with',b'link.exe failed'),
         'tauri_failure':(b'failed to bundle',b'failed to build app')}

def capture_desktop_job(argv,*,cwd,env,timeout):
    result={'exit_code':None,'timed_out':False,'stdout':{k:False for k in PHRASES},'stderr':{k:False for k in PHRASES}}
    codes=set(); lock=threading.Lock(); job=OwnedJob(argv,cwd=cwd,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    def read(pipe,channel):
        tail=b''
        while True:
            block=pipe.read(65536)
            if not block: break
            data=(tail+block).lower()
            for name,phrases in PHRASES.items():
                if any(phrase in data for phrase in phrases): result[channel][name]=True
            with lock:
                for match in re.findall(rb'error\[e([0-9]{4})\]|\blnk([0-9]{4})\b',data):
                    if len(codes)<8: codes.add(('E'+match[0].decode()) if match[0] else ('LNK'+match[1].decode()))
            tail=data[-4096:]
    threads=[threading.Thread(target=read,args=(job.process.stdout,'stdout')),threading.Thread(target=read,args=(job.process.stderr,'stderr'))]
    for thread in threads: thread.start()
    try:
        try: result['exit_code']=job.process.wait(timeout=timeout)
        except subprocess.TimeoutExpired: result['timed_out']=True
    finally:
        job.close()
        for thread in threads: thread.join(timeout=5)
        for pipe in (job.process.stdout,job.process.stderr): pipe.close()
    if any(thread.is_alive() for thread in threads): raise ValueError('diagnostic_cleanup_failed')
    result['codes']=sorted(codes)
    return result
