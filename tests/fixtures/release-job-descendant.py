import subprocess
import sys
import time
from pathlib import Path

if sys.argv[1:] == ['descendant']:
    time.sleep(60)
elif sys.argv[1:] == ['normal']:
    sys.exit(23)
elif len(sys.argv)==3 and sys.argv[1]=='signal':
    child=subprocess.Popen([sys.executable,__file__,'descendant'],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    Path(sys.argv[2]).write_text(str(child.pid),encoding='ascii')
    time.sleep(60)
else:
    child=subprocess.Popen([sys.executable,__file__,'descendant'],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    print(child.pid,flush=True)
    time.sleep(60)
