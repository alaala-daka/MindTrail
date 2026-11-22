"""Windows path identity and immutable read lock for the isolated CLI."""
import ctypes
from ctypes import wintypes
import hashlib
import msvcrt
import os
from pathlib import Path


class FILE_ATTRIBUTE_TAG_INFO(ctypes.Structure):
    _fields_ = [('attributes', wintypes.DWORD), ('tag', wintypes.DWORD)]


class CliLock:
    def __init__(self, path, expected):
        self.path = Path(path)
        self.expected = expected
        self.handles = []
        self.stream = None

    def __enter__(self):
        kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, ctypes.c_void_p, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
        kernel.CreateFileW.restype = wintypes.HANDLE
        kernel.GetFileInformationByHandleEx.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD]
        kernel.GetFileInformationByHandleEx.restype = wintypes.BOOL
        kernel.GetFinalPathNameByHandleW.argtypes = [wintypes.HANDLE, wintypes.LPWSTR, wintypes.DWORD, wintypes.DWORD]
        kernel.GetFinalPathNameByHandleW.restype = wintypes.DWORD
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        kernel.CloseHandle.restype = wintypes.BOOL
        self.kernel = kernel
        try:
            # No DELETE sharing prevents path components being renamed while
            # allowing ordinary reads and writes elsewhere in each directory.
            for component in reversed((self.path.parent, *self.path.parent.parents)):
                handle = kernel.CreateFileW(str(component), 0, 3, None, 3, 0x02200000, None)
                if handle == ctypes.c_void_p(-1).value:
                    raise ValueError('cli_lock_failed')
                self.handles.append(handle)
                self._verify_handle(handle, component)
            handle = kernel.CreateFileW(str(self.path), 0x80000000, 1, None, 3, 0x00200000, None)
            if handle == ctypes.c_void_p(-1).value:
                raise ValueError('cli_lock_failed')
            self.handles.append(handle)
            self._verify_handle(handle, self.path)
            fd = msvcrt.open_osfhandle(handle, os.O_RDONLY | os.O_BINARY)
            self.handles.pop()
            self.stream = os.fdopen(fd, 'rb')
            digest = hashlib.sha256()
            while block := self.stream.read(1048576):
                digest.update(block)
            if digest.hexdigest() != self.expected:
                raise ValueError('cli_digest_mismatch')
            return self
        except BaseException:
            self.close()
            raise

    def _verify_handle(self, handle, expected):
        info = FILE_ATTRIBUTE_TAG_INFO()
        if not self.kernel.GetFileInformationByHandleEx(handle, 9, ctypes.byref(info), ctypes.sizeof(info)) or info.attributes & 0x400:
            raise ValueError('cli_reparse_path')
        buffer = ctypes.create_unicode_buffer(32768)
        length = self.kernel.GetFinalPathNameByHandleW(handle, buffer, len(buffer), 0)
        if not length or length >= len(buffer):
            raise ValueError('cli_lock_failed')
        actual = buffer.value
        if actual.startswith('\\\\?\\'):
            actual = actual[4:]
        if os.path.normcase(os.path.normpath(actual)) != os.path.normcase(os.path.normpath(str(expected))):
            raise ValueError('cli_path_changed')

    def close(self):
        if self.stream:
            self.stream.close()
            self.stream = None
        for handle in reversed(self.handles):
            self.kernel.CloseHandle(handle)
        self.handles.clear()

    def __exit__(self, *_):
        self.close()
