"""Owned Windows process tree: assign before the suspended process can run."""
import ctypes
from ctypes import wintypes
import os
import subprocess
import socket


class BASIC_LIMIT(ctypes.Structure):
    _fields_ = [('PerProcessUserTimeLimit', ctypes.c_int64), ('PerJobUserTimeLimit', ctypes.c_int64),
                ('LimitFlags', wintypes.DWORD), ('MinimumWorkingSetSize', ctypes.c_size_t),
                ('MaximumWorkingSetSize', ctypes.c_size_t), ('ActiveProcessLimit', wintypes.DWORD),
                ('Affinity', ctypes.c_size_t), ('PriorityClass', wintypes.DWORD), ('SchedulingClass', wintypes.DWORD)]


class IO_COUNTERS(ctypes.Structure):
    _fields_ = [(name, ctypes.c_uint64) for name in ('ReadOperationCount', 'WriteOperationCount', 'OtherOperationCount', 'ReadTransferCount', 'WriteTransferCount', 'OtherTransferCount')]


class EXTENDED_LIMIT(ctypes.Structure):
    _fields_ = [('BasicLimitInformation', BASIC_LIMIT), ('IoInfo', IO_COUNTERS),
                ('ProcessMemoryLimit', ctypes.c_size_t), ('JobMemoryLimit', ctypes.c_size_t),
                ('PeakProcessMemoryUsed', ctypes.c_size_t), ('PeakJobMemoryUsed', ctypes.c_size_t)]


class OwnedJob:
    def __init__(self, args, *, cwd, env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE):
        if os.name != 'nt':
            raise OSError('windows_required')
        kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        kernel.CreateJobObjectW.argtypes = [ctypes.c_void_p, wintypes.LPCWSTR]
        kernel.CreateJobObjectW.restype = wintypes.HANDLE
        kernel.SetInformationJobObject.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD]
        kernel.SetInformationJobObject.restype = wintypes.BOOL
        kernel.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
        kernel.AssignProcessToJobObject.restype = wintypes.BOOL
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        kernel.CloseHandle.restype = wintypes.BOOL
        self.kernel = kernel
        self.handle = kernel.CreateJobObjectW(None, None)
        self.process = None
        if not self.handle:
            raise OSError('job_create_failed')
        try:
            limits = EXTENDED_LIMIT()
            limits.BasicLimitInformation.LimitFlags = 0x2000  # KILL_ON_JOB_CLOSE
            if not kernel.SetInformationJobObject(self.handle, 9, ctypes.byref(limits), ctypes.sizeof(limits)):
                raise OSError('job_setup_failed')
            self.process = subprocess.Popen(args, cwd=cwd, env=env, stdin=stdin, stdout=stdout, stderr=stderr,
                                            creationflags=0x00000004 | 0x08000000)
            if not kernel.AssignProcessToJobObject(self.handle, wintypes.HANDLE(int(self.process._handle))):
                raise OSError('job_assign_failed')
            ntdll = ctypes.WinDLL('ntdll')
            ntdll.NtResumeProcess.argtypes = [wintypes.HANDLE]
            ntdll.NtResumeProcess.restype = ctypes.c_long
            if ntdll.NtResumeProcess(wintypes.HANDLE(int(self.process._handle))) != 0:
                raise OSError('job_resume_failed')
        except BaseException:
            try:
                if self.process is not None:
                    try:
                        self.process.kill(); self.process.wait(timeout=10)
                    except (OSError, subprocess.SubprocessError):
                        pass
                    finally:
                        for pipe in (self.process.stdin, self.process.stdout, self.process.stderr):
                            if pipe:
                                try: pipe.close()
                                except OSError: pass
            finally:
                kernel.CloseHandle(self.handle)
                self.handle = None
            raise

    def close(self):
        if self.handle:
            if not self.kernel.CloseHandle(self.handle):
                raise OSError('job_close_failed')
            self.handle = None
        if self.process is not None:
            self.process.wait(timeout=10)
            for pipe in (self.process.stdin, self.process.stdout, self.process.stderr):
                if pipe:
                    pipe.close()

    def contains_active_pid(self, pid):
        """Check that the specified live process is in this owned job."""
        if not self.handle or type(pid) is not int or not 0 < pid < 2**32:
            return False
        kernel = self.kernel
        kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel.OpenProcess.restype = wintypes.HANDLE
        kernel.IsProcessInJob.argtypes = [wintypes.HANDLE, wintypes.HANDLE, ctypes.POINTER(wintypes.BOOL)]
        kernel.IsProcessInJob.restype = wintypes.BOOL
        kernel.GetExitCodeProcess.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
        kernel.GetExitCodeProcess.restype = wintypes.BOOL
        process = kernel.OpenProcess(0x1000, False, pid)
        if not process:
            return False
        try:
            member, code = wintypes.BOOL(), wintypes.DWORD()
            return bool(kernel.IsProcessInJob(process, self.handle, ctypes.byref(member)) and member.value
                        and kernel.GetExitCodeProcess(process, ctypes.byref(code)) and code.value == 259)
        finally:
            kernel.CloseHandle(process)

    def owns_listener(self, port):
        """Confirm that the exact IPv4 listener belongs to this job's tree."""
        if not self.handle or type(port) is not int or not 1 <= port <= 65535:
            return False
        query = self.kernel.QueryInformationJobObject
        query.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD, ctypes.POINTER(wintypes.DWORD)]
        query.restype = wintypes.BOOL
        size = 8192
        job_data = ctypes.create_string_buffer(size)
        returned = wintypes.DWORD()
        if not query(self.handle, 3, job_data, size, ctypes.byref(returned)):
            return False
        count = ctypes.c_uint32.from_buffer_copy(job_data.raw[4:8]).value
        if count > (size - 8) // ctypes.sizeof(ctypes.c_size_t):
            return False
        pids = {ctypes.c_size_t.from_buffer_copy(job_data.raw[8 + i * ctypes.sizeof(ctypes.c_size_t):]).value for i in range(count)}
        ip = ctypes.WinDLL('iphlpapi', use_last_error=True)
        get_table = ip.GetExtendedTcpTable
        get_table.argtypes = [ctypes.c_void_p, ctypes.POINTER(wintypes.DWORD), wintypes.BOOL, wintypes.ULONG, ctypes.c_int, wintypes.ULONG]
        get_table.restype = wintypes.ULONG
        length = wintypes.DWORD(0)
        if get_table(None, ctypes.byref(length), False, 2, 3, 0) != 122 or length.value > 8 * 1024 * 1024:
            return False
        table = ctypes.create_string_buffer(length.value)
        if get_table(table, ctypes.byref(length), False, 2, 3, 0) != 0:
            return False
        rows = ctypes.c_uint32.from_buffer_copy(table.raw[:4]).value
        if 4 + rows * 24 > length.value:
            return False
        for index in range(rows):
            values = (ctypes.c_uint32 * 6).from_buffer_copy(table.raw[4 + index * 24:4 + (index + 1) * 24])
            if values[0] == 2 and values[1] == 0x0100007f and socket.ntohs(values[2] & 0xffff) == port and values[5] in pids:
                return True
        return False

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()
