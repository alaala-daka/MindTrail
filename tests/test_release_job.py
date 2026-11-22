"""Synthetic Windows job ownership and descendant cleanup."""
import ctypes
from ctypes import wintypes
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest

sys.path.insert(0,str(Path(__file__).resolve().parent))
from mindtrail_real_agent_job import OwnedJob
sys.path.insert(0,str(Path(__file__).resolve().parent.parent/'scripts'))
from release_launcher import run_job

@unittest.skipUnless(os.name=='nt','Windows job test')
class ReleaseJobTests(unittest.TestCase):
    def test_launcher_normal_exit(self):
        fixture=Path(__file__).resolve().parent/'fixtures/release-job-descendant.py'
        system=os.environ['SystemRoot']; env={'SystemRoot':system,'WINDIR':system,'PATH':str(Path(system)/'System32')}
        self.assertEqual(run_job([sys.executable,str(fixture),'normal'],cwd=str(fixture.parent),env=env,timeout_seconds=10),23)
    def test_launcher_timeout_terminates_descendant(self):
        fixture=Path(__file__).resolve().parent/'fixtures/release-job-descendant.py'; root=fixture.parents[1].parent
        system=os.environ['SystemRoot']; env={'SystemRoot':system,'WINDIR':system,'PATH':str(Path(system)/'System32')}
        with tempfile.TemporaryDirectory(dir=root/'.tools') as tmp:
            signal=Path(tmp)/'pid'; outcomes=[]
            def run():
                try: run_job([sys.executable,str(fixture),'signal',str(signal)],cwd=str(fixture.parent),env=env,timeout_seconds=3)
                except TimeoutError: outcomes.append('timeout')
            thread=threading.Thread(target=run); thread.start()
            deadline=time.monotonic()+2
            while not signal.exists() and time.monotonic()<deadline: time.sleep(.02)
            self.assertTrue(signal.exists(),'child_not_started')
            pid=int(signal.read_text()); kernel=ctypes.WinDLL('kernel32',use_last_error=True)
            kernel.OpenProcess.argtypes=[wintypes.DWORD,wintypes.BOOL,wintypes.DWORD]; kernel.OpenProcess.restype=wintypes.HANDLE
            kernel.WaitForSingleObject.argtypes=[wintypes.HANDLE,wintypes.DWORD]; kernel.WaitForSingleObject.restype=wintypes.DWORD
            kernel.CloseHandle.argtypes=[wintypes.HANDLE]; kernel.CloseHandle.restype=wintypes.BOOL
            handle=kernel.OpenProcess(0x00100000,False,pid)
            try:
                self.assertTrue(handle); thread.join(10); self.assertFalse(thread.is_alive())
                self.assertEqual(outcomes,['timeout']); self.assertEqual(kernel.WaitForSingleObject(handle,10000),0)
            finally:
                if handle: kernel.CloseHandle(handle)
    def test_owned_descendant_is_terminated_on_close(self):
        fixture=Path(__file__).resolve().parent/'fixtures/release-job-descendant.py'
        system=os.environ['SystemRoot']
        env={'SystemRoot':system,'WINDIR':system,'PATH':str(Path(system)/'System32')}
        job=OwnedJob([sys.executable,str(fixture)],cwd=str(fixture.parent),env=env,stdin=subprocess.DEVNULL)
        kernel=ctypes.WinDLL('kernel32',use_last_error=True)
        kernel.OpenProcess.argtypes=[wintypes.DWORD,wintypes.BOOL,wintypes.DWORD]; kernel.OpenProcess.restype=wintypes.HANDLE
        kernel.WaitForSingleObject.argtypes=[wintypes.HANDLE,wintypes.DWORD]; kernel.WaitForSingleObject.restype=wintypes.DWORD
        kernel.CloseHandle.argtypes=[wintypes.HANDLE]; kernel.CloseHandle.restype=wintypes.BOOL
        handle=None
        try:
            pid=int(job.process.stdout.readline())
            self.assertTrue(job.contains_active_pid(pid))
            handle=kernel.OpenProcess(0x00100000,False,pid)
            self.assertTrue(handle)
            job.close()
            self.assertEqual(kernel.WaitForSingleObject(handle,10000),0,'descendant_not_terminated')
        finally:
            if job.handle: job.close()
            if handle: kernel.CloseHandle(handle)

if __name__=='__main__': unittest.main()
