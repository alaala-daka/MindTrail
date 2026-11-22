"""Windows job wrapper for an owned, suspended process tree."""
import ctypes
from ctypes import wintypes
import os
import subprocess

class _Basic(ctypes.Structure):
    _fields_=[('PerProcessUserTimeLimit',ctypes.c_int64),('PerJobUserTimeLimit',ctypes.c_int64),('LimitFlags',wintypes.DWORD),('MinimumWorkingSetSize',ctypes.c_size_t),('MaximumWorkingSetSize',ctypes.c_size_t),('ActiveProcessLimit',wintypes.DWORD),('Affinity',ctypes.c_size_t),('PriorityClass',wintypes.DWORD),('SchedulingClass',wintypes.DWORD)]
class _Io(ctypes.Structure):
    _fields_=[(n,ctypes.c_uint64) for n in ('ReadOperationCount','WriteOperationCount','OtherOperationCount','ReadTransferCount','WriteTransferCount','OtherTransferCount')]
class _Extended(ctypes.Structure):
    _fields_=[('BasicLimitInformation',_Basic),('IoInfo',_Io),('ProcessMemoryLimit',ctypes.c_size_t),('JobMemoryLimit',ctypes.c_size_t),('PeakProcessMemoryUsed',ctypes.c_size_t),('PeakJobMemoryUsed',ctypes.c_size_t)]

class OwnedJob:
    def __init__(self,argv,*,cwd,env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL):
        if os.name!='nt': raise OSError('windows_required')
        k=ctypes.WinDLL('kernel32',use_last_error=True); self.k=k; self.process=None
        k.CreateJobObjectW.argtypes=[ctypes.c_void_p,wintypes.LPCWSTR]; k.CreateJobObjectW.restype=wintypes.HANDLE
        k.SetInformationJobObject.argtypes=[wintypes.HANDLE,ctypes.c_int,ctypes.c_void_p,wintypes.DWORD]; k.SetInformationJobObject.restype=wintypes.BOOL
        k.AssignProcessToJobObject.argtypes=[wintypes.HANDLE,wintypes.HANDLE]; k.AssignProcessToJobObject.restype=wintypes.BOOL
        k.CloseHandle.argtypes=[wintypes.HANDLE]; k.CloseHandle.restype=wintypes.BOOL
        self.handle=k.CreateJobObjectW(None,None)
        if not self.handle: raise OSError('job_create_failed')
        try:
            limits=_Extended(); limits.BasicLimitInformation.LimitFlags=0x2000
            if not k.SetInformationJobObject(self.handle,9,ctypes.byref(limits),ctypes.sizeof(limits)): raise OSError('job_setup_failed')
            self.process=subprocess.Popen(argv,cwd=cwd,env=env,stdin=subprocess.DEVNULL,stdout=stdout,stderr=stderr,creationflags=0x00000004|0x08000000)
            if not k.AssignProcessToJobObject(self.handle,wintypes.HANDLE(int(self.process._handle))): raise OSError('job_assign_failed')
            ntdll=ctypes.WinDLL('ntdll'); ntdll.NtResumeProcess.argtypes=[wintypes.HANDLE]; ntdll.NtResumeProcess.restype=ctypes.c_long
            if ntdll.NtResumeProcess(wintypes.HANDLE(int(self.process._handle)))!=0: raise OSError('job_resume_failed')
        except BaseException:
            try:
                if self.process:
                    try: self.process.kill(); self.process.wait(timeout=10)
                    except (OSError,subprocess.SubprocessError): pass
            finally:
                k.CloseHandle(self.handle); self.handle=None
            raise
    def close(self):
        if self.handle:
            if not self.k.CloseHandle(self.handle): raise OSError('job_close_failed')
            self.handle=None
        if self.process: self.process.wait(timeout=10)
