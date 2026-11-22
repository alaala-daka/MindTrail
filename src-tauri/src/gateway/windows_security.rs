use std::{io, mem, ptr};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    Security::{GetLengthSid, GetTokenInformation, IsValidSid, TokenUser, TOKEN_QUERY, TOKEN_USER},
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

struct Token(HANDLE);
impl Drop for Token {
    fn drop(&mut self) { unsafe { CloseHandle(self.0); } }
}

/// Returns an owned, suitably aligned copy of the current process user's SID.
pub(super) fn current_user_sid() -> io::Result<Vec<u32>> {
    unsafe {
        let mut handle = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle) == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = Token(handle);
        let mut size = 0;
        GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut size);
        if size < mem::size_of::<TOKEN_USER>() as u32 {
            return Err(io::Error::last_os_error());
        }
        let mut buffer = vec![0usize; (size as usize).div_ceil(mem::size_of::<usize>())];
        if GetTokenInformation(token.0, TokenUser, buffer.as_mut_ptr().cast(), size, &mut size) == 0 {
            return Err(io::Error::last_os_error());
        }
        let sid = (*(buffer.as_ptr().cast::<TOKEN_USER>())).User.Sid;
        if sid.is_null() || IsValidSid(sid) == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "process token contains an invalid user SID"));
        }
        let len = GetLengthSid(sid) as usize;
        let mut result = vec![0u32; len.div_ceil(mem::size_of::<u32>())];
        ptr::copy_nonoverlapping(sid.cast::<u8>(), result.as_mut_ptr().cast::<u8>(), len);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn current_sid_is_valid_and_nonempty() {
        let sid = current_user_sid().expect("read current user SID");
        assert!(!sid.is_empty());
        assert_ne!(unsafe { IsValidSid(sid.as_ptr() as *mut _) }, 0);
        assert!(unsafe { GetLengthSid(sid.as_ptr() as *mut _) } > 0);
    }
}

/// Checks the DACL of an already-open file-system object. The caller must hold
/// the handle open and ensure it refers to the intended directory.
pub(super) fn validate_directory_dacl(handle: HANDLE) -> io::Result<()> { validate_directory_dacl_snapshot(handle, false) }
fn validate_directory_dacl_snapshot(handle: HANDLE, require_inheritance: bool) -> io::Result<()> {
    use windows_sys::Win32::{Foundation::LocalFree, Security::{
        Authorization::{GetSecurityInfo, SE_FILE_OBJECT}, CreateWellKnownSid, EqualSid,
        GetAce, GetSecurityDescriptorControl, IsValidAcl, ACE_HEADER, ACCESS_ALLOWED_ACE,
        DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, INHERITED_ACE, SE_DACL_PROTECTED, SECURITY_MAX_SID_SIZE,
        WinLocalSystemSid,
    }};
    struct Descriptor(*mut core::ffi::c_void);
    impl Drop for Descriptor { fn drop(&mut self) { unsafe { LocalFree(self.0); } } }
    fn invalid(message: &'static str) -> io::Error { io::Error::new(io::ErrorKind::PermissionDenied, message) }
    unsafe {
        let user = current_user_sid()?;
        let mut system = [0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)];
        let mut system_size = SECURITY_MAX_SID_SIZE;
        if CreateWellKnownSid(WinLocalSystemSid, ptr::null_mut(), system.as_mut_ptr().cast(), &mut system_size) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut owner = ptr::null_mut();
        let mut acl = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        let status = GetSecurityInfo(handle, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION, &mut owner, ptr::null_mut(), &mut acl, ptr::null_mut(), &mut descriptor);
        if status != 0 { return Err(io::Error::from_raw_os_error(status as i32)); }
        let _descriptor = Descriptor(descriptor);
        if owner.is_null() || IsValidSid(owner) == 0 || (EqualSid(owner, user.as_ptr() as *mut _) == 0 && EqualSid(owner, system.as_mut_ptr().cast()) == 0) { return Err(invalid("directory owner is not trusted")); }
        let mut control = 0;
        let mut revision = 0;
        if GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) == 0 { return Err(io::Error::last_os_error()); }
        if control & SE_DACL_PROTECTED == 0 { return Err(invalid("directory DACL is not protected")); }
        if acl.is_null() { return Err(invalid("directory has a null DACL")); }
        if IsValidAcl(acl) == 0 { return Err(invalid("directory DACL is invalid")); }
        for index in 0..(*acl).AceCount as u32 {
            let mut ace = ptr::null_mut();
            if GetAce(acl, index, &mut ace) == 0 { return Err(io::Error::last_os_error()); }
            let header = &*(ace.cast::<ACE_HEADER>());
            if header.AceFlags as u32 & INHERITED_ACE != 0 { return Err(invalid("directory DACL contains an inherited ACE")); }
            // Only ordinary allow and deny ACEs are understood. Reject all other
            // types, including object and callback ACEs, rather than guessing.
            match header.AceType {
                0 | 1 => {
                    if (header.AceSize as usize) < mem::size_of::<ACCESS_ALLOWED_ACE>() { return Err(invalid("directory DACL contains a truncated ACE")); }
                    let sid = ptr::addr_of!((*(ace.cast::<ACCESS_ALLOWED_ACE>())).SidStart) as *mut core::ffi::c_void;
                    let sid_bytes = header.AceSize as usize - mem::offset_of!(ACCESS_ALLOWED_ACE, SidStart);
                    if sid_bytes < 8 || 8 + (*((sid as *const u8).add(1)) as usize) * 4 > sid_bytes || IsValidSid(sid) == 0 {
                        return Err(invalid("directory DACL contains an invalid SID"));
                    }
                    if header.AceType == 0 && EqualSid(sid, user.as_ptr() as *mut _) == 0 && EqualSid(sid, system.as_mut_ptr().cast()) == 0 {
                        return Err(invalid("directory DACL grants access to an unexpected principal"));
                    }
                }
                _ => return Err(invalid("directory DACL contains an unsupported ACE type")),
            }
        }
        if require_inheritance { validate_leaf_inheritance(acl)?; }
        Ok(())
    }
}

#[cfg(test)]
mod dacl_tests {
    use super::*;
    use std::{fs::{self, OpenOptions}, os::windows::{fs::OpenOptionsExt, io::AsRawHandle}, time::{SystemTime, UNIX_EPOCH}};
    use windows_sys::Win32::Security::{ACL, ACL_REVISION, AddAccessAllowedAce, AddAccessDeniedAce, InitializeAcl, CreateWellKnownSid, WinWorldSid, SECURITY_MAX_SID_SIZE, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, UNPROTECTED_DACL_SECURITY_INFORMATION, Authorization::{SetSecurityInfo, SE_FILE_OBJECT}};
    #[test]
    fn checks_protection_and_unexpected_allow() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("mindtrail-acl-test-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&path).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup { fn drop(&mut self) { let _ = fs::remove_dir(&self.0); } }
        let _cleanup = Cleanup(path.clone());
        let file = OpenOptions::new().access_mode(0x00020000 | 0x00040000).custom_flags(0x02000000).open(&path).unwrap();
        let handle = file.as_raw_handle() as HANDLE;
        unsafe {
            let user = current_user_sid().unwrap();
            let mut world = [0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)];
            let mut size = SECURITY_MAX_SID_SIZE;
            assert_ne!(CreateWellKnownSid(WinWorldSid, ptr::null_mut(), world.as_mut_ptr().cast(), &mut size), 0);
            let mut storage = [0u32; 128];
            let acl = storage.as_mut_ptr().cast::<ACL>();
            assert_ne!(InitializeAcl(acl, mem::size_of_val(&storage) as u32, ACL_REVISION), 0);
            assert_ne!(AddAccessAllowedAce(acl, ACL_REVISION, 0x1f01ff, user.as_ptr() as *mut _), 0);
            assert_ne!(AddAccessDeniedAce(acl, ACL_REVISION, 1, world.as_mut_ptr().cast()), 0);
            let set = |flags| SetSecurityInfo(handle, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | flags, ptr::null_mut(), ptr::null_mut(), acl, ptr::null());
            assert_eq!(set(PROTECTED_DACL_SECURITY_INFORMATION), 0);
            validate_directory_dacl(handle).unwrap();
            assert_eq!(SetSecurityInfo(handle, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(), ptr::null(), ptr::null()), 0);
            assert_eq!(validate_directory_dacl(handle).unwrap_err().to_string(), "directory has a null DACL");
            assert_eq!(set(UNPROTECTED_DACL_SECURITY_INFORMATION), 0);
            assert_eq!(validate_directory_dacl(handle).unwrap_err().to_string(), "directory DACL is not protected");
            assert_ne!(AddAccessAllowedAce(acl, ACL_REVISION, 1, world.as_mut_ptr().cast()), 0);
            assert_eq!(set(PROTECTED_DACL_SECURITY_INFORMATION), 0);
            assert_eq!(validate_directory_dacl(handle).unwrap_err().to_string(), "directory DACL grants access to an unexpected principal");
        }
        drop(file);
        fs::remove_dir(path).unwrap();
    }
}






fn validate_path_syntax(path: &std::path::Path) -> io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    fn bad(message: &'static str) -> io::Error { io::Error::new(io::ErrorKind::InvalidInput, message) }
    let raw: Vec<u16> = path.as_os_str().encode_wide().collect();
    if raw.len() < 4 || !((raw[0] >= 65 && raw[0] <= 90) || (raw[0] >= 97 && raw[0] <= 122)) || raw[1] != 58 || raw[2] != 92 {
        return Err(bad("path must be an absolute local drive path"));
    }
    for component in raw[3..].split(|&c| c == 92) {
        if component.is_empty() || component.last() == Some(&32) || component.last() == Some(&46) || component.iter().any(|&c| c < 32 || [34, 42, 47, 58, 60, 62, 63, 124, 126, 0xb9, 0xb2, 0xb3].contains(&c)) {
            return Err(bad("path contains an ambiguous component"));
        }
        let name = String::from_utf16_lossy(component).split('.').next().unwrap_or("").to_ascii_uppercase();
        if ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&name.as_str()) || (name.len() == 4 && (name.starts_with("COM") || name.starts_with("LPT")) && matches!(name.as_bytes()[3], b'1'..=b'9')) {
            return Err(bad("path contains a reserved device name"));
        }
    }
    Ok(raw)
}

/// Validates lexical path constraints and resolves its drive to a local volume.
/// This does not validate any directory, ancestor, reparse point, or ACL.
struct ResolvedVolume { root: std::path::PathBuf, path: std::path::PathBuf }

fn resolve_local_volume_path(path: &std::path::Path) -> io::Result<ResolvedVolume> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumeInformationW, GetVolumeNameForVolumeMountPointW, QueryDosDeviceW};
    fn bad(message: &'static str) -> io::Error { io::Error::new(io::ErrorKind::InvalidInput, message) }
    use windows_sys::Win32::System::{WindowsProgramming::DRIVE_FIXED, SystemServices::FILE_PERSISTENT_ACLS};
    fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }
    let raw = validate_path_syntax(path)?;
    let drive = String::from_utf16_lossy(&raw[..2]);
    let root = wide(&format!("{drive}\\"));
    unsafe {
        if GetDriveTypeW(root.as_ptr()) != DRIVE_FIXED { return Err(bad("path is not on a fixed local drive")); }
        let mut volume = [0u16; 64];
        if GetVolumeNameForVolumeMountPointW(root.as_ptr(), volume.as_mut_ptr(), volume.len() as u32) == 0 { return Err(io::Error::last_os_error()); }
        let end = volume.iter().position(|&c| c == 0).ok_or_else(|| bad("invalid volume name"))?;
        let volume_name = String::from_utf16(&volume[..end]).map_err(|_| bad("invalid volume name"))?;
        let volume_device = volume_name.strip_prefix("\\\\?\\").and_then(|s| s.strip_suffix('\\')).ok_or_else(|| bad("invalid volume GUID"))?;
        if !volume_device.starts_with("Volume{") || !volume_device.ends_with('}') { return Err(bad("invalid volume GUID")); }
        let mut drive_target = [0u16; 1024];
        let mut volume_target = [0u16; 1024];
        let drive_len = QueryDosDeviceW(wide(&drive).as_ptr(), drive_target.as_mut_ptr(), drive_target.len() as u32) as usize;
        if drive_len == 0 { return Err(io::Error::last_os_error()); }
        let volume_len = QueryDosDeviceW(wide(volume_device).as_ptr(), volume_target.as_mut_ptr(), volume_target.len() as u32) as usize;
        if volume_len == 0 { return Err(io::Error::last_os_error()); }
        fn first(v: &[u16], len: usize) -> io::Result<&[u16]> {
            let data = v.get(..len).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid DOS device result length"))?;
            let end = data.iter().position(|&c| c == 0).filter(|&i| i > 0).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unterminated or empty DOS device target"))?;
            Ok(&data[..end])
        }
        if first(&drive_target, drive_len)? != first(&volume_target, volume_len)? { return Err(bad("drive mapping does not match volume root")); }
        let mut flags = 0;
        let mut fs = [0u16; 32];
        if GetVolumeInformationW(volume.as_ptr(), ptr::null_mut(), 0, ptr::null_mut(), ptr::null_mut(), &mut flags, fs.as_mut_ptr(), fs.len() as u32) == 0 { return Err(io::Error::last_os_error()); }
        if first(&fs, fs.len())? != [78, 84, 70, 83] || flags & FILE_PERSISTENT_ACLS == 0 { return Err(bad("volume must be NTFS with persistent ACLs")); }
        let mut result = volume[..end].to_vec();
        result.extend_from_slice(&raw[3..]);
        Ok(ResolvedVolume { root: std::path::PathBuf::from(std::ffi::OsString::from_wide(&volume[..end])), path: std::path::PathBuf::from(std::ffi::OsString::from_wide(&result)) })
    }
}


pub(super) fn validate_local_volume_path(path: &std::path::Path) -> io::Result<std::path::PathBuf> {
    Ok(resolve_local_volume_path(path)?.path)
}

#[cfg(test)]
mod volume_tests {
    use super::*;
    #[test]
    fn lexical_rejections_and_workspace_volume() {
        for path in [r"relative\x", r"C:x", r"\\server\share\x", r"\\?\C:\x", r"C:\a\..\b", r"C:\a\", r"C:\a:b", r"C:\NUL.txt", r"C:\abc~1", r"C:\a ", r"C:\a//b"] {
            assert_eq!(validate_path_syntax(std::path::Path::new(path)).unwrap_err().kind(), io::ErrorKind::InvalidInput, "{path}");
        }
        for path in ["C:\\a\\\\b", "C:\\a.", "C:\\COM1.txt", "C:\\LPT9", "C:\\a\0b", "\\\\.\\C:\\x"] {
            assert_eq!(validate_path_syntax(std::path::Path::new(path)).unwrap_err().kind(), io::ErrorKind::InvalidInput, "{path:?}");
        }
        let unicode = std::path::Path::new("C:\\目录\\普通：文件");
        assert_eq!(validate_path_syntax(unicode).unwrap(), { use std::os::windows::ffi::OsStrExt; unicode.as_os_str().encode_wide().collect::<Vec<_>>() });
        let candidate = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("isolated-volume-test");
        let result = validate_local_volume_path(&candidate).unwrap();
        assert!(result.to_string_lossy().starts_with(r"\\?\Volume{"));
        assert!(result.ends_with("isolated-volume-test"));
    }
}

fn verify_root_identity(a: &windows_sys::Win32::Storage::FileSystem::FILE_ID_INFO, b: &windows_sys::Win32::Storage::FileSystem::FILE_ID_INFO) -> io::Result<()> {
    if a.VolumeSerialNumber != b.VolumeSerialNumber || a.FileId.Identifier != b.FileId.Identifier {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "drive root differs from volume root"));
    }
    Ok(())
}

/// Holds the verified volume root open while subsequent directory traversal runs.
pub(super) struct VerifiedVolumeRoot {
    handle: HANDLE,
    pub(super) path: std::path::PathBuf,
}
impl Drop for VerifiedVolumeRoot {
    fn drop(&mut self) { unsafe { CloseHandle(self.handle); } }
}

pub(super) fn open_verified_volume_root(path: &std::path::Path) -> io::Result<VerifiedVolumeRoot> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{Foundation::INVALID_HANDLE_VALUE, Storage::FileSystem::{ READ_CONTROL,
        CreateFileW, GetDriveTypeW, GetFileInformationByHandleEx, GetFileType,
        GetVolumeInformationByHandleW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TYPE_DISK,
        FileAttributeTagInfo, FileIdInfo, OPEN_EXISTING,
    }, System::{SystemServices::FILE_PERSISTENT_ACLS, WindowsProgramming::DRIVE_FIXED}};
    fn bad(message: &'static str) -> io::Error { io::Error::new(io::ErrorKind::PermissionDenied, message) }
    fn open(path: &[u16]) -> io::Result<VerifiedVolumeRoot> {
        unsafe {
            let h = CreateFileW(path.as_ptr(), FILE_READ_ATTRIBUTES | READ_CONTROL, FILE_SHARE_READ | FILE_SHARE_WRITE, ptr::null(), OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT, ptr::null_mut());
            if h == INVALID_HANDLE_VALUE { return Err(io::Error::last_os_error()); }
            Ok(VerifiedVolumeRoot { handle: h, path: std::path::PathBuf::new() })
        }
    }
    unsafe fn info<T: Default>(h: HANDLE, class: i32) -> io::Result<T> {
        let mut value = T::default();
        if unsafe { GetFileInformationByHandleEx(h, class, (&mut value as *mut T).cast(), mem::size_of::<T>() as u32) } == 0 { return Err(io::Error::last_os_error()); }
        Ok(value)
    }
    let resolved = resolve_local_volume_path(path)?;
    let mut root_wide: Vec<u16> = resolved.root.as_os_str().encode_wide().collect();
    root_wide.push(0);
    let mut drive_wide: Vec<u16> = path.as_os_str().encode_wide().take(3).collect();
    drive_wide.push(0);
    unsafe {
        if GetDriveTypeW(root_wide.as_ptr()) != DRIVE_FIXED { return Err(bad("volume root is not fixed")); }
        let mut root = open(&root_wide)?;
        check_volume_device_with(root.handle, query_volume_device)?;
        let drive_root = open(&drive_wide)?;
        if GetFileType(root.handle) != FILE_TYPE_DISK || GetFileType(drive_root.handle) != FILE_TYPE_DISK { return Err(bad("volume root is not a disk")); }
        let attributes: FILE_ATTRIBUTE_TAG_INFO = info(root.handle, FileAttributeTagInfo)?;
        if attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 || attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 { return Err(bad("volume root is not a non-reparse directory")); }
        let a: FILE_ID_INFO = info(root.handle, FileIdInfo)?;
        let b: FILE_ID_INFO = info(drive_root.handle, FileIdInfo)?;
        verify_root_identity(&a, &b)?;
        let mut flags = 0;
        let mut fs = [0u16; 32];
        if GetVolumeInformationByHandleW(root.handle, ptr::null_mut(), 0, ptr::null_mut(), ptr::null_mut(), &mut flags, fs.as_mut_ptr(), fs.len() as u32) == 0 { return Err(io::Error::last_os_error()); }
        if !fs.starts_with(&[78,84,70,83,0]) || flags & FILE_PERSISTENT_ACLS == 0 { return Err(bad("volume root is not NTFS with persistent ACLs")); }
        root.path = resolved.root;
        Ok(root)
    }
}



#[cfg(test)]
mod root_tests {
    use super::*;
    #[test]
    fn root_identity_mismatch_is_rejected() {
        use windows_sys::Win32::Storage::FileSystem::FILE_ID_INFO;
        let a = FILE_ID_INFO::default();
        let mut b = a;
        b.FileId.Identifier[15] = 1;
        assert_eq!(verify_root_identity(&a, &b).unwrap_err().to_string(), "drive root differs from volume root");
        b = a;
        b.VolumeSerialNumber = 1;
        assert!(verify_root_identity(&a, &b).is_err());
    }
    #[test]
    fn workspace_volume_root_is_held_and_invalid_path_rejected() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("root-test-not-created");
        let root = open_verified_volume_root(&path).unwrap();
        assert!(root.path.to_string_lossy().starts_with(r"\\?\Volume{"));
        assert!(!root.handle.is_null());
        eprintln!("volume device: {:?}", query_volume_device(root.handle).unwrap());
        assert!(open_verified_volume_root(std::path::Path::new(r"\\server\share\x")).is_err());
    }
}



fn check_volume_device_with<F>(handle: HANDLE, query: F) -> io::Result<()>
where F: FnOnce(HANDLE) -> io::Result<(u32, u32)> {
    use windows_sys::{Wdk::System::SystemServices::{FILE_REMOTE_DEVICE, FILE_REMOVABLE_MEDIA}, Win32::Storage::FileSystem::FILE_DEVICE_DISK};
    let (device_type, characteristics) = query(handle)?;
    if device_type != FILE_DEVICE_DISK || characteristics & (FILE_REMOTE_DEVICE | FILE_REMOVABLE_MEDIA) != 0 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "volume device is remote, removable, or not a disk"));
    }
    Ok(())
}

fn query_volume_device(handle: HANDLE) -> io::Result<(u32, u32)> {
    use windows_sys::{Wdk::{Storage::FileSystem::{NtQueryVolumeInformationFile, FileFsDeviceInformation}, System::SystemServices::FILE_FS_DEVICE_INFORMATION}, Win32::System::IO::IO_STATUS_BLOCK};
    let mut status_block = IO_STATUS_BLOCK::default();
    let mut device = FILE_FS_DEVICE_INFORMATION::default();
    let status = unsafe { NtQueryVolumeInformationFile(handle, &mut status_block, (&mut device as *mut FILE_FS_DEVICE_INFORMATION).cast(), mem::size_of_val(&device) as u32, FileFsDeviceInformation) };
    if status != 0 { return Err(io::Error::other(format!("volume device query failed: NTSTATUS 0x{:08x}", status as u32))); }
    if unsafe { status_block.Anonymous.Status } != 0 { return Err(io::Error::other(format!("volume device completion failed: NTSTATUS 0x{:08x}", unsafe { status_block.Anonymous.Status } as u32))); }
    if status_block.Information != mem::size_of_val(&device) { return Err(io::Error::new(io::ErrorKind::InvalidData, "volume device query returned an unexpected length")); }
    Ok((device.DeviceType, device.Characteristics))
}

#[cfg(test)]
mod device_tests {
    use super::*;
    use windows_sys::{Wdk::System::SystemServices::{FILE_REMOTE_DEVICE, FILE_REMOVABLE_MEDIA}, Win32::Storage::FileSystem::FILE_DEVICE_DISK};
    #[test]
    fn rejects_device_query_failures_and_unsafe_characteristics() {
        for value in [(0, 0), (FILE_DEVICE_DISK, FILE_REMOTE_DEVICE), (FILE_DEVICE_DISK, FILE_REMOVABLE_MEDIA)] {
            assert!(check_volume_device_with(ptr::null_mut(), |_| Ok(value)).is_err());
        }
        assert!(check_volume_device_with(ptr::null_mut(), |_| Err(io::Error::other("injected failure"))).is_err());
        assert!(check_volume_device_with(ptr::null_mut(), |_| Ok((FILE_DEVICE_DISK, 0))).is_ok());
    }
}




/// Preliminary traversal only: callers must separately validate ancestor ACLs.
pub(super) struct DirectoryChain {
    _root: VerifiedVolumeRoot,
    _directories: Vec<Token>,
    path: std::path::PathBuf,
}

pub(super) fn open_existing_directory_chain(path: &std::path::Path) -> io::Result<DirectoryChain> {
    open_directory_chain_with(path, |handle, _| validate_ancestor_acl(handle, false))
}

fn open_directory_chain_with<F>(path: &std::path::Path, mut check: F) -> io::Result<DirectoryChain>
where F: FnMut(HANDLE, bool) -> io::Result<()> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::{Foundation::INVALID_HANDLE_VALUE, Storage::FileSystem::{
        CreateFileW, GetFileInformationByHandleEx, GetFileType, GetFinalPathNameByHandleW,
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO,
        FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TYPE_DISK,
        FileAttributeTagInfo, FileIdInfo, OPEN_EXISTING, READ_CONTROL, VOLUME_NAME_GUID,
    }};
    let raw = validate_path_syntax(path)?;
    let root = open_verified_volume_root(path)?;
    let mut prefix: Vec<u16> = root.path.as_os_str().encode_wide().collect();
    let mut handles = Vec::new();
    unsafe {
        let mut root_id = FILE_ID_INFO::default();
        if GetFileInformationByHandleEx(root.handle, FileIdInfo, (&mut root_id as *mut FILE_ID_INFO).cast(), mem::size_of_val(&root_id) as u32) == 0 { return Err(io::Error::last_os_error()); }
        let components: Vec<&[u16]> = raw[3..].split(|&c| c == b'\\' as u16).collect();
        for (index, component) in components.iter().enumerate() {
            let parent = handles.last().map_or(root.handle, |h: &Token| h.0);
            check(parent, index + 1 == components.len()).map_err(|e| {
                let parent_name = if index == 0 { "volume root".to_owned() } else { std::ffi::OsString::from_wide(components[index - 1]).to_string_lossy().into_owned() };
                io::Error::new(e.kind(), format!("ancestor {parent_name}: {e}"))
            })?;
            if !prefix.ends_with(&[b'\\' as u16]) { prefix.push(b'\\' as u16); }
            prefix.extend_from_slice(component);
            let mut terminated = prefix.clone(); terminated.push(0);
            let handle = CreateFileW(terminated.as_ptr(), READ_CONTROL | FILE_READ_ATTRIBUTES, FILE_SHARE_READ | FILE_SHARE_WRITE, ptr::null(), OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT, ptr::null_mut());
            if handle == INVALID_HANDLE_VALUE { return Err(io::Error::last_os_error()); }
            let guard = Token(handle);
            let mut attrs = FILE_ATTRIBUTE_TAG_INFO::default();
            let mut id = FILE_ID_INFO::default();
            if GetFileType(handle) != FILE_TYPE_DISK { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "directory component is not a disk object")); }
            if GetFileInformationByHandleEx(handle, FileAttributeTagInfo, (&mut attrs as *mut FILE_ATTRIBUTE_TAG_INFO).cast(), mem::size_of_val(&attrs) as u32) == 0 || GetFileInformationByHandleEx(handle, FileIdInfo, (&mut id as *mut FILE_ID_INFO).cast(), mem::size_of_val(&id) as u32) == 0 { return Err(io::Error::last_os_error()); }
            if attrs.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 || attrs.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || id.VolumeSerialNumber != root_id.VolumeSerialNumber { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "directory component is not a non-reparse directory on the verified volume")); }
            let mut buffer = vec![0u16; 512];
            loop {
                let n = GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), buffer.len() as u32, VOLUME_NAME_GUID) as usize;
                if n == 0 { return Err(io::Error::last_os_error()); }
                if n < buffer.len() {
                    if buffer[..n] != prefix { return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("directory name cannot be verified: {}", std::ffi::OsString::from_wide(component).to_string_lossy()))); }
                    break;
                }
                if n > 32768 { return Err(io::Error::new(io::ErrorKind::InvalidData, "normalized path exceeds limit")); }
                buffer.resize(n + 1, 0);
            }
            handles.push(guard);
        }
    }
    Ok(DirectoryChain { _root: root, _directories: handles, path: std::path::PathBuf::from(std::ffi::OsString::from_wide(&prefix)) })
}

#[cfg(test)]
mod chain_tests {
    use super::*;
    use std::{fs, time::{SystemTime, UNIX_EPOCH}};
    fn unchecked(path: &std::path::Path) -> io::Result<DirectoryChain> { open_directory_chain_with(path, |_, _| Ok(())) }
    #[test]
    fn validates_existing_directory_components() {
        if std::env::var_os("MINDTRAIL_CHAIN_ISOLATED_TEST").is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "gateway::windows_security::chain_tests::validates_existing_directory_components", "--test-threads=1"])
                .env("MINDTRAIL_CHAIN_ISOLATED_TEST", "1").status().unwrap();
            assert!(status.success(), "isolated handle test failed");
            return;
        }
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailChain{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
        let _cleanup = Cleanup(base.clone());
        let child = base.join("中文目录");
        fs::create_dir(&child).unwrap();
        let file = base.join("plainfile"); fs::write(&file, b"sentinel").unwrap();
        use std::os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle};
        use windows_sys::Win32::{Foundation::GetHandleInformation, System::Threading::{GetCurrentProcess, GetProcessHandleCount}};
        let chain = unchecked(&child).unwrap();
        let opened: Vec<HANDLE> = std::iter::once(chain._root.handle).chain(chain._directories.iter().map(|h| h.0)).collect();
        assert!(opened.len() >= 3);
        for &handle in &opened { let mut flags = 0; assert_ne!(unsafe { GetHandleInformation(handle, &mut flags) }, 0); }
        drop(chain);
        for &handle in &opened { let mut flags = 0; assert_eq!(unsafe { GetHandleInformation(handle, &mut flags) }, 0); }
        let count = || { let mut n = 0; assert_ne!(unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut n) }, 0); n };
        let before = count();
        for _ in 0..10 { assert!(unchecked(&base.join("中文目录").join("missing")).is_err()); }
        assert_eq!(count(), before, "failed traversal leaked handles");
        assert!(unchecked(&file).is_err());
        assert!(unchecked(&base.join("PLAINFILE")).is_err());
        let case = base.join("CaseName"); fs::create_dir(&case).unwrap();
        assert!(unchecked(&base.join("casename")).is_err());
        let junction = base.join("junction");
        let output = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&junction).arg(&child).output().unwrap();
        assert!(output.status.success(), "junction fixture unavailable: {}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(unchecked(&junction).err().unwrap().to_string(), "directory component is not a non-reparse directory on the verified volume");
        fs::remove_dir(&junction).unwrap();
        use windows_sys::Win32::Storage::FileSystem::SetFileShortNameW;
        let long = base.join("Long Directory Name"); fs::create_dir(&long).unwrap();
        let handle = fs::OpenOptions::new().access_mode(0x00010000).custom_flags(0x02000000).open(&long).unwrap();
        let alias: Vec<u16> = std::ffi::OsStr::new("MDTRAL01").encode_wide().chain(Some(0)).collect();
        assert_ne!(unsafe { SetFileShortNameW(handle.as_raw_handle() as HANDLE, alias.as_ptr()) }, 0, "short-name fixture unavailable: {}", io::Error::last_os_error());
        drop(handle);
        let original = fs::OpenOptions::new().read(true).custom_flags(0x02000000).open(&long).unwrap();
        let alias_handle = fs::OpenOptions::new().read(true).custom_flags(0x02000000).open(base.join("MDTRAL01")).unwrap();
        use windows_sys::Win32::Storage::FileSystem::{FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx};
        let identity = |h: &fs::File| { let mut id = FILE_ID_INFO::default(); assert_ne!(unsafe { GetFileInformationByHandleEx(h.as_raw_handle() as HANDLE, FileIdInfo, (&mut id as *mut FILE_ID_INFO).cast(), mem::size_of_val(&id) as u32) }, 0); id };
        let a = identity(&original); let b = identity(&alias_handle);
        assert_eq!(a.VolumeSerialNumber, b.VolumeSerialNumber);
        assert_eq!(a.FileId.Identifier, b.FileId.Identifier);
        drop(original); drop(alias_handle);
        unchecked(&long).unwrap();
        assert_eq!(unchecked(&base.join("MDTRAL01")).err().unwrap().to_string(), "directory name cannot be verified: MDTRAL01");
    }
}

/// Evaluates one ordinary allow ACE on an ancestor directory. This is only a
/// policy primitive: callers must independently validate the SID, owner, ACL,
/// ACE type and structure, and all directory handles.
fn ancestor_allow_is_safe(trusted: bool, flags: u32, mut mask: u32, direct_parent: bool) -> io::Result<()> {
    use windows_sys::Win32::{Foundation::{GENERIC_ALL, GENERIC_EXECUTE, GENERIC_READ, GENERIC_WRITE}, Security::{CONTAINER_INHERIT_ACE, INHERITED_ACE, INHERIT_ONLY_ACE, NO_PROPAGATE_INHERIT_ACE, OBJECT_INHERIT_ACE}, Storage::FileSystem::{DELETE, FILE_ALL_ACCESS, FILE_APPEND_DATA, FILE_DELETE_CHILD, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA, WRITE_DAC, WRITE_OWNER}};
    let inheritance = OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE;
    let known = inheritance | INHERITED_ACE | INHERIT_ONLY_ACE | NO_PROPAGATE_INHERIT_ACE;
    if flags & !known != 0 || (flags & (INHERIT_ONLY_ACE | NO_PROPAGATE_INHERIT_ACE) != 0 && flags & inheritance == 0) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "unsupported ancestor ACE flags"));
    }
    if trusted || flags & INHERIT_ONLY_ACE != 0 { return Ok(()); }
    for (generic, expanded) in [(GENERIC_READ, FILE_GENERIC_READ), (GENERIC_WRITE, FILE_GENERIC_WRITE), (GENERIC_EXECUTE, FILE_GENERIC_EXECUTE), (GENERIC_ALL, FILE_ALL_ACCESS)] {
        if mask & generic != 0 { mask = (mask & !generic) | expanded; }
    }
    let mut dangerous = DELETE | FILE_DELETE_CHILD | WRITE_DAC | WRITE_OWNER | FILE_WRITE_ATTRIBUTES | FILE_WRITE_EA;
    // These bits mean adding files/subdirectories on a directory. Ancestors
    // may permit unrelated additions; the immediate parent must not.
    if direct_parent { dangerous |= FILE_WRITE_DATA | FILE_APPEND_DATA; }
    if mask & dangerous != 0 { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "ancestor ACE grants dangerous access")); }
    Ok(())
}

#[cfg(test)]
mod ancestor_policy_tests {
    use super::*;
    use windows_sys::Win32::{Foundation::{GENERIC_ALL, GENERIC_EXECUTE, GENERIC_READ, GENERIC_WRITE}, Security::{CONTAINER_INHERIT_ACE, INHERIT_ONLY_ACE, OBJECT_INHERIT_ACE, NO_PROPAGATE_INHERIT_ACE, INHERITED_ACE}, Storage::FileSystem::{DELETE, FILE_APPEND_DATA, FILE_DELETE_CHILD, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA, WRITE_DAC, WRITE_OWNER}};
    #[test]
    fn evaluates_ancestor_allow_masks_and_flags() {
        for mask in [DELETE, FILE_DELETE_CHILD, WRITE_DAC, WRITE_OWNER, FILE_WRITE_ATTRIBUTES, FILE_WRITE_EA, GENERIC_WRITE, GENERIC_ALL] {
            for parent in [false, true] { assert!(ancestor_allow_is_safe(false, 0, mask, parent).is_err(), "{mask:#x}"); }
            assert!(ancestor_allow_is_safe(true, 0, mask, true).is_ok());
            assert!(ancestor_allow_is_safe(false, INHERIT_ONLY_ACE | CONTAINER_INHERIT_ACE, mask, true).is_ok());
        }
        for mask in [FILE_WRITE_DATA, FILE_APPEND_DATA] {
            assert!(ancestor_allow_is_safe(false, 0, mask, false).is_ok());
            assert!(ancestor_allow_is_safe(false, 0, mask, true).is_err());
        }
        for mask in [0, GENERIC_READ, GENERIC_EXECUTE] { assert!(ancestor_allow_is_safe(false, 0, mask, true).is_ok()); }
        for flags in [OBJECT_INHERIT_ACE, CONTAINER_INHERIT_ACE | NO_PROPAGATE_INHERIT_ACE, INHERITED_ACE, OBJECT_INHERIT_ACE | INHERIT_ONLY_ACE] { assert!(ancestor_allow_is_safe(false, flags, GENERIC_READ, true).is_ok()); }
        for flags in [OBJECT_INHERIT_ACE, CONTAINER_INHERIT_ACE | NO_PROPAGATE_INHERIT_ACE, INHERITED_ACE] { assert!(ancestor_allow_is_safe(false, flags, DELETE, false).is_err()); }
        assert!(ancestor_allow_is_safe(false, OBJECT_INHERIT_ACE | INHERIT_ONLY_ACE, DELETE, true).is_ok());
        for flags in [0x20, INHERIT_ONLY_ACE, NO_PROPAGATE_INHERIT_ACE] { assert!(ancestor_allow_is_safe(true, flags, 0, false).is_err()); }
    }
}


unsafe fn ancestor_owner_is_trusted(owner: *mut core::ffi::c_void, user: *mut core::ffi::c_void, system: *mut core::ffi::c_void, administrators: *mut core::ffi::c_void) -> bool {
    use windows_sys::Win32::Security::EqualSid;
    // S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464.
    // Only the exact Windows Modules Installer service SID is trusted as owner.
    let trusted_installer: [u32; 8] = [0x00000601, 0x05000000, 80, 956008885, 3418522649, 1831038044, 1853292631, 2271478464];
    unsafe { EqualSid(owner, user) != 0 || EqualSid(owner, system) != 0 || EqualSid(owner, administrators) != 0 || EqualSid(owner, trusted_installer.as_ptr() as *mut _) != 0 }
}

/// Validates the complete ordinary ACE and returns its flags, mask and SID offset.
fn parse_ancestor_ace(bytes: &[u8]) -> io::Result<(u8, u32, usize)> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid ancestor ACE structure");
    if bytes.len() < 8 { return Err(invalid()); }
    validate_ancestor_ace_type(bytes[0])?;
    let len = u16::from_le_bytes([bytes[2], bytes[3]]) as usize;
    if len != bytes.len() || len < 16 { return Err(invalid()); }
    let sid = &bytes[8..];
    let sid_len = 8usize.checked_add((sid[1] as usize).checked_mul(4).ok_or_else(invalid)?).ok_or_else(invalid)?;
    if sid[0] != 1 || sid[1] > 15 || sid_len != sid.len() { return Err(invalid()); }
    let flags = bytes[1];
    ancestor_allow_is_safe(true, flags as u32, 0, false)?;
    Ok((flags, u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 8))
}

fn validate_ancestor_ace_type(ace_type: u8) -> io::Result<()> {
    if ace_type != 0 && ace_type != 1 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "ancestor DACL contains an unsupported ACE type"));
    }
    Ok(())
}

fn validate_creator_owner_flags(creator: bool, flags: u32) -> io::Result<()> {
    if creator && flags & windows_sys::Win32::Security::INHERIT_ONLY_ACE == 0 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "creator owner ACE is not inherit-only"));
    }
    Ok(())
}

/// Checks an already-open ancestor; does not validate object type or paths.
fn validate_ancestor_acl(handle: HANDLE, direct_parent: bool) -> io::Result<()> {
    use windows_sys::Win32::{Foundation::LocalFree, Security::{Authorization::{GetSecurityInfo, SE_FILE_OBJECT}, CreateWellKnownSid, EqualSid, GetAce, IsValidAcl, ACE_HEADER, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, SECURITY_MAX_SID_SIZE, WinLocalSystemSid, WinBuiltinAdministratorsSid, WinCreatorOwnerSid}};
    struct Descriptor(*mut core::ffi::c_void);
    impl Drop for Descriptor { fn drop(&mut self) { unsafe { LocalFree(self.0); } } }
    fn invalid(s: &'static str) -> io::Error { io::Error::new(io::ErrorKind::PermissionDenied, s) }
    unsafe {
        let user = current_user_sid()?;
        let mut known = [[0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)]; 3];
        for (sid, kind) in known.iter_mut().zip([WinLocalSystemSid, WinBuiltinAdministratorsSid, WinCreatorOwnerSid]) {
            let mut size = SECURITY_MAX_SID_SIZE;
            if CreateWellKnownSid(kind, ptr::null_mut(), sid.as_mut_ptr().cast(), &mut size) == 0 { return Err(io::Error::last_os_error()); }
        }
        let mut owner = ptr::null_mut(); let mut acl = ptr::null_mut(); let mut descriptor = ptr::null_mut();
        let status = GetSecurityInfo(handle, SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION, &mut owner, ptr::null_mut(), &mut acl, ptr::null_mut(), &mut descriptor);
        if status != 0 { return Err(io::Error::from_raw_os_error(status as i32)); }
        let _descriptor = Descriptor(descriptor);
        let trusted = |sid| EqualSid(sid, user.as_ptr() as *mut _) != 0 || EqualSid(sid, known[0].as_ptr() as *mut _) != 0 || EqualSid(sid, known[1].as_ptr() as *mut _) != 0;
        if owner.is_null() || IsValidSid(owner) == 0 { return Err(invalid("ancestor owner is not trusted")); }
        if !ancestor_owner_is_trusted(owner, user.as_ptr() as *mut _, known[0].as_ptr() as *mut _, known[1].as_ptr() as *mut _) { return Err(invalid("ancestor owner is not trusted")); }
        if acl.is_null() { return Err(invalid("ancestor has a null DACL")); }
        if IsValidAcl(acl) == 0 { return Err(invalid("ancestor DACL is invalid")); }
        for i in 0..(*acl).AceCount as u32 {
            let mut ace = ptr::null_mut();
            if GetAce(acl, i, &mut ace) == 0 { return Err(io::Error::last_os_error()); }
            let header = &*ace.cast::<ACE_HEADER>();
            let len = header.AceSize as usize;
            let bytes = std::slice::from_raw_parts(ace.cast::<u8>(), len);
            let (flags, parsed_mask, sid_offset) = parse_ancestor_ace(bytes)?;
            let sid = ace.cast::<u8>().add(sid_offset).cast::<core::ffi::c_void>();
            if IsValidSid(sid) == 0 { return Err(invalid("ancestor DACL contains an invalid SID")); }
            let creator = EqualSid(sid, known[2].as_ptr() as *mut _) != 0;
            validate_creator_owner_flags(creator, flags as u32)?;
            // Validate flags even on deny entries; deny entries never grant access.
            let mask = if header.AceType == 0 { parsed_mask } else { 0 };
            ancestor_allow_is_safe(trusted(sid), flags as u32, mask, direct_parent)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod ancestor_acl_tests {
    use super::*;
    use std::{fs::{self, OpenOptions}, os::windows::{fs::OpenOptionsExt, io::AsRawHandle}, time::{SystemTime, UNIX_EPOCH}};
    use windows_sys::Win32::{Foundation::GENERIC_READ, Security::{ACL, ACL_REVISION, AddAccessAllowedAce, AddAccessAllowedAceEx, AddAccessDeniedAce, AddAuditAccessAce, InitializeAcl, CreateWellKnownSid, WinWorldSid, WinLocalSystemSid, WinBuiltinAdministratorsSid, WinCreatorOwnerSid, SECURITY_MAX_SID_SIZE, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, INHERIT_ONLY_ACE, CONTAINER_INHERIT_ACE, Authorization::{SetSecurityInfo, SE_FILE_OBJECT}}, Storage::FileSystem::{FILE_DELETE_CHILD, WRITE_DAC, FILE_WRITE_DATA}};
    #[test]
    fn evaluates_real_ancestor_acl() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailAncestor{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&path).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { let _ = fs::remove_dir(&self.0); } }
        let _cleanup = Cleanup(path.clone());
        let file = OpenOptions::new().access_mode(0x00020000 | 0x00040000).custom_flags(0x02000000).open(&path).unwrap();
        let handle = file.as_raw_handle() as HANDLE;
        unsafe {
            let user = current_user_sid().unwrap();
            let mut world = [0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)]; let mut size = SECURITY_MAX_SID_SIZE;
            assert_ne!(CreateWellKnownSid(WinWorldSid, ptr::null_mut(), world.as_mut_ptr().cast(), &mut size), 0);
            assert_eq!(SetSecurityInfo(handle, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(), ptr::null(), ptr::null()), 0);
            assert_eq!(validate_ancestor_acl(handle, false).unwrap_err().to_string(), "ancestor has a null DACL");
            for (kind, flags, expected) in [(WinLocalSystemSid, 0, true), (WinBuiltinAdministratorsSid, 0, true), (WinCreatorOwnerSid, INHERIT_ONLY_ACE | CONTAINER_INHERIT_ACE, true)] {
                let mut principal = [0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)]; let mut length = SECURITY_MAX_SID_SIZE;
                assert_ne!(CreateWellKnownSid(kind, ptr::null_mut(), principal.as_mut_ptr().cast(), &mut length), 0);
                let mut storage = [0u32; 128]; let acl = storage.as_mut_ptr().cast::<ACL>();
                assert_ne!(InitializeAcl(acl, mem::size_of_val(&storage) as u32, ACL_REVISION), 0);
                assert_ne!(AddAccessAllowedAce(acl, ACL_REVISION, 0x1f01ff, user.as_ptr() as *mut _), 0);
                assert_ne!(AddAccessAllowedAceEx(acl, ACL_REVISION, flags, WRITE_DAC, principal.as_mut_ptr().cast()), 0);
                assert_eq!(SetSecurityInfo(handle, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(), acl, ptr::null()), 0);
                let result = validate_ancestor_acl(handle, true);
                assert_eq!(result.is_ok(), expected, "principal {kind}: {result:?}");
            }
            let mut system = [0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)];
            let mut admins = system;
            for (sid, kind) in [(&mut system, WinLocalSystemSid), (&mut admins, WinBuiltinAdministratorsSid)] {
                let mut length = SECURITY_MAX_SID_SIZE;
                assert_ne!(CreateWellKnownSid(kind, ptr::null_mut(), sid.as_mut_ptr().cast(), &mut length), 0);
            }
            let user_ptr = user.as_ptr() as *mut _; let sys_ptr = system.as_mut_ptr().cast(); let admin_ptr = admins.as_mut_ptr().cast();
            for owner in [user_ptr, sys_ptr, admin_ptr] { assert!(ancestor_owner_is_trusted(owner, user_ptr, sys_ptr, admin_ptr)); }
            assert!(!ancestor_owner_is_trusted(world.as_mut_ptr().cast(), user_ptr, sys_ptr, admin_ptr));
            let mut installer = [0x00000601u32, 0x05000000, 80, 956008885, 3418522649, 1831038044, 1853292631, 2271478464];
            assert_ne!(IsValidSid(installer.as_mut_ptr().cast()), 0);
            assert!(ancestor_owner_is_trusted(installer.as_mut_ptr().cast(), user_ptr, sys_ptr, admin_ptr));
            use windows_sys::Win32::{Foundation::LocalFree, Security::{Authorization::ConvertStringSidToSidW, EqualSid}};
            let text: Vec<u16> = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464".encode_utf16().chain(Some(0)).collect();
            let mut actual = ptr::null_mut();
            assert_ne!(ConvertStringSidToSidW(text.as_ptr(), &mut actual), 0);
            assert_ne!(EqualSid(actual, installer.as_mut_ptr().cast()), 0);
            LocalFree(actual);
            installer[7] ^= 1;
            assert!(!ancestor_owner_is_trusted(installer.as_mut_ptr().cast(), user_ptr, sys_ptr, admin_ptr));
            installer[7] ^= 1; installer[3] ^= 1;
            assert!(!ancestor_owner_is_trusted(installer.as_mut_ptr().cast(), user_ptr, sys_ptr, admin_ptr));
            {
                let mut storage = [0u32; 128]; let acl = storage.as_mut_ptr().cast::<ACL>();
                assert_ne!(InitializeAcl(acl, mem::size_of_val(&storage) as u32, ACL_REVISION), 0);
                assert_ne!(AddAuditAccessAce(acl, ACL_REVISION, GENERIC_READ, world.as_mut_ptr().cast(), 1, 0), 0);
                assert_ne!(AddAccessAllowedAce(acl, ACL_REVISION, 0x1f01ff, user.as_ptr() as *mut _), 0);
                let status = SetSecurityInfo(handle, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(), acl, ptr::null());
                if status == 0 {
                    assert_eq!(validate_ancestor_acl(handle, false).unwrap_err().to_string(), "ancestor DACL contains an unsupported ACE type");
                    eprintln!("audit ACE persisted and rejected");
                } else {
                    eprintln!("audit ACE not accepted in DACL (error {status}); testing parser type directly");
                    assert_eq!(validate_ancestor_ace_type(2).unwrap_err().to_string(), "ancestor DACL contains an unsupported ACE type");
                }
            }
            assert!(validate_creator_owner_flags(true, 0).is_err());
            assert!(validate_creator_owner_flags(true, INHERIT_ONLY_ACE).is_ok());
            {
                let mut storage = [0u32; 128]; let acl = storage.as_mut_ptr().cast::<ACL>();
                assert_ne!(InitializeAcl(acl, mem::size_of_val(&storage) as u32, ACL_REVISION), 0);
                assert_ne!(AddAccessDeniedAce(acl, ACL_REVISION, WRITE_DAC, world.as_mut_ptr().cast()), 0);
                assert_ne!(AddAccessAllowedAce(acl, ACL_REVISION, 0x1f01ff, user.as_ptr() as *mut _), 0);
                assert_eq!(SetSecurityInfo(handle, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(), acl, ptr::null()), 0);
                validate_ancestor_acl(handle, true).unwrap();
            }
            for (mask, flags, parent, expected) in [(GENERIC_READ, 0, true, true), (FILE_DELETE_CHILD, 0, false, false), (WRITE_DAC, 0, false, false), (FILE_WRITE_DATA, 0, false, true), (FILE_WRITE_DATA, 0, true, false), (WRITE_DAC, INHERIT_ONLY_ACE | CONTAINER_INHERIT_ACE, true, true)] {
                let mut storage = [0u32; 128]; let acl = storage.as_mut_ptr().cast::<ACL>();
                assert_ne!(InitializeAcl(acl, mem::size_of_val(&storage) as u32, ACL_REVISION), 0);
                assert_ne!(AddAccessAllowedAce(acl, ACL_REVISION, 0x1f01ff, user.as_ptr() as *mut _), 0);
                assert_ne!(AddAccessAllowedAceEx(acl, ACL_REVISION, flags, mask, world.as_mut_ptr().cast()), 0);
                assert_eq!(SetSecurityInfo(handle, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(), acl, ptr::null()), 0, "failed to set isolated ACL");
                let result = validate_ancestor_acl(handle, parent);
                assert_eq!(result.is_ok(), expected, "mask={mask:#x} flags={flags:#x}: {result:?}");
            }
        }
    }
}

#[cfg(test)]
mod default_home_probe {
    use super::*;
    use std::{fs::OpenOptions, os::windows::{fs::OpenOptionsExt, io::AsRawHandle}};
    #[test]
    fn report_home_acl_compatibility() {
        let path = std::env::var_os("USERPROFILE").expect("USERPROFILE unavailable");
        let file = OpenOptions::new().access_mode(0x00020000).custom_flags(0x02000000).open(path).unwrap();
        let ordinary = validate_ancestor_acl(file.as_raw_handle() as HANDLE, false);
        let direct = validate_ancestor_acl(file.as_raw_handle() as HANDLE, true);
        eprintln!("home ancestor={ordinary:?}; direct parent={direct:?}");
    }
}

#[cfg(test)]
mod ancestor_ace_bytes_tests {
    use super::*;
    #[test]
    fn rejects_truncated_sid_and_unsupported_flags() {
        let valid = [0u8, 0, 16, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 5];
        assert!(parse_ancestor_ace(&valid).is_ok());
        for len in 0..valid.len() { assert!(parse_ancestor_ace(&valid[..len]).is_err(), "length {len}"); }
        let mut bad = valid; bad[2] = 17; assert!(parse_ancestor_ace(&bad).is_err());
        bad = valid; bad[9] = 1; assert!(parse_ancestor_ace(&bad).is_err());
        bad = valid; bad[8] = 0; assert!(parse_ancestor_ace(&bad).is_err());
        bad = valid; bad[1] = 0x20; assert!(parse_ancestor_ace(&bad).is_err());
        bad = valid; bad[1] = 0x08; assert!(parse_ancestor_ace(&bad).is_err());
    }
}


#[cfg(test)]
mod integrated_ancestor_tests {
    use super::*;
    use std::{fs::{self, OpenOptions}, os::windows::{fs::OpenOptionsExt, io::AsRawHandle}, time::{SystemTime, UNIX_EPOCH}};
    use windows_sys::Win32::{Security::{ACL, ACL_REVISION, AddAccessAllowedAce, AddAccessAllowedAceEx, CONTAINER_INHERIT_ACE, OBJECT_INHERIT_ACE, InitializeAcl, CreateWellKnownSid, WinWorldSid, SECURITY_MAX_SID_SIZE, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, Authorization::{SetSecurityInfo, SE_FILE_OBJECT}}, Storage::FileSystem::FILE_WRITE_DATA};
    #[test]
    fn direct_parent_checked_before_opening_child() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailParent{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&path).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
        let _cleanup = Cleanup(path.clone());
        let child = path.join("child"); fs::create_dir(&child).unwrap();
        let f = OpenOptions::new().access_mode(0x00020000 | 0x00040000).custom_flags(0x02000000).open(&path).unwrap();
        unsafe {
            let user = current_user_sid().unwrap(); let mut world = [0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)]; let mut size = SECURITY_MAX_SID_SIZE;
            assert_ne!(CreateWellKnownSid(WinWorldSid, ptr::null_mut(), world.as_mut_ptr().cast(), &mut size), 0);
            let mut storage = [0u32; 128]; let acl = storage.as_mut_ptr().cast::<ACL>();
            assert_ne!(InitializeAcl(acl, mem::size_of_val(&storage) as u32, ACL_REVISION), 0);
            assert_ne!(AddAccessAllowedAceEx(acl, ACL_REVISION, CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE, 0x1f01ff, user.as_ptr() as *mut _), 0);
            let set = || SetSecurityInfo(f.as_raw_handle() as HANDLE, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(), acl, ptr::null());
            assert_eq!(set(), 0);
            let check = |h, direct| if direct { validate_ancestor_acl(h, true) } else { Ok(()) };
            open_directory_chain_with(&child, check).unwrap();
            assert_ne!(AddAccessAllowedAce(acl, ACL_REVISION, FILE_WRITE_DATA, world.as_mut_ptr().cast()), 0);
            assert_eq!(set(), 0);
            let error = open_directory_chain_with(&path.join("missing"), check).err().unwrap();
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            assert_eq!(error.to_string(), format!("ancestor {}: ancestor ACE grants dangerous access", path.file_name().unwrap().to_string_lossy()));
        }
    }
    #[test]
    fn real_home_chain_reports_result() {
        let path = std::env::var_os("USERPROFILE").unwrap();
        open_existing_directory_chain(std::path::Path::new(&path)).expect("USERPROFILE chain rejected");
    }
}


#[cfg(test)]
mod home_chain_fixture_tests {
    use super::*;
    use std::{fs::{self, OpenOptions}, os::windows::{fs::OpenOptionsExt, io::AsRawHandle}, time::{SystemTime, UNIX_EPOCH}};
    use windows_sys::Win32::{Security::{ACL, ACL_REVISION, AddAccessAllowedAce, AddAccessAllowedAceEx, CONTAINER_INHERIT_ACE, OBJECT_INHERIT_ACE, InitializeAcl, CreateWellKnownSid, WinWorldSid, SECURITY_MAX_SID_SIZE, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, Authorization::{SetSecurityInfo, SE_FILE_OBJECT}}, Storage::FileSystem::FILE_WRITE_DATA};
    #[test]
    fn profile_direct_parent_and_untrusted_child_parent() {
        let home = std::path::PathBuf::from(std::env::var_os("USERPROFILE").expect("USERPROFILE is required"));
        let fixture = home.join(format!("MindTrailW1dTest{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&fixture).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
        let _cleanup = Cleanup(fixture.clone());
        // USERPROFILE is the direct parent, with no policy bypass.
        drop(open_existing_directory_chain(&fixture).expect("profile fixture chain rejected"));
        let child = fixture.join("child"); fs::create_dir(&child).unwrap();
        let f = OpenOptions::new().access_mode(0x00020000 | 0x00040000).custom_flags(0x02000000).open(&fixture).unwrap();
        unsafe {
            let user = current_user_sid().unwrap(); let mut world = [0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)]; let mut size = SECURITY_MAX_SID_SIZE;
            assert_ne!(CreateWellKnownSid(WinWorldSid, ptr::null_mut(), world.as_mut_ptr().cast(), &mut size), 0);
            let mut storage = [0u32; 128]; let acl = storage.as_mut_ptr().cast::<ACL>();
            assert_ne!(InitializeAcl(acl, mem::size_of_val(&storage) as u32, ACL_REVISION), 0);
            assert_ne!(AddAccessAllowedAceEx(acl, ACL_REVISION, CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE, 0x1f01ff, user.as_ptr() as *mut _), 0);
            let set = || SetSecurityInfo(f.as_raw_handle() as HANDLE, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(), acl, ptr::null());
            assert_eq!(set(), 0);
            drop(open_existing_directory_chain(&child).expect("safe fixture chain rejected"));
            assert_ne!(AddAccessAllowedAce(acl, ACL_REVISION, FILE_WRITE_DATA, world.as_mut_ptr().cast()), 0);
            assert_eq!(set(), 0);
            let chain = open_existing_directory_chain(&fixture).unwrap();
            let error = create_private_leaf(&chain, std::ffi::OsStr::new("missing")).err().unwrap();
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            assert_eq!(error.to_string(), "ancestor ACE grants dangerous access");
        }
    }
}

/// Owns the absolute descriptor and ACL referenced by creation attributes.
struct PrivateDirectorySecurity {
    descriptor: Box<windows_sys::Win32::Security::SECURITY_DESCRIPTOR>,
    _acl: Box<[u32]>,
    _owner: Box<[u32]>,
}
impl PrivateDirectorySecurity {
    fn new() -> io::Result<Self> {
        use windows_sys::Win32::Security::{ACL, ACL_REVISION, AddAccessAllowedAceEx, CONTAINER_INHERIT_ACE, OBJECT_INHERIT_ACE, CreateWellKnownSid, InitializeAcl, InitializeSecurityDescriptor, SECURITY_DESCRIPTOR, SECURITY_MAX_SID_SIZE, SE_DACL_PROTECTED, SetSecurityDescriptorControl, SetSecurityDescriptorDacl, SetSecurityDescriptorOwner, WinLocalSystemSid};
        use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
        let mut user = current_user_sid()?.into_boxed_slice();
        let mut system = [0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)];
        let mut system_len = SECURITY_MAX_SID_SIZE;
        let mut acl = vec![0u32; 128].into_boxed_slice();
        let mut descriptor = Box::new(SECURITY_DESCRIPTOR::default());
        unsafe {
            if CreateWellKnownSid(WinLocalSystemSid, ptr::null_mut(), system.as_mut_ptr().cast(), &mut system_len) == 0 { return Err(io::Error::last_os_error()); }
            let acl_ptr = acl.as_mut_ptr().cast::<ACL>();
            if InitializeAcl(acl_ptr, mem::size_of_val(&*acl) as u32, ACL_REVISION) == 0 { return Err(io::Error::last_os_error()); }
            let flags = OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE;
            if AddAccessAllowedAceEx(acl_ptr, ACL_REVISION, flags, FILE_ALL_ACCESS, user.as_ptr() as *mut _) == 0 || AddAccessAllowedAceEx(acl_ptr, ACL_REVISION, flags, FILE_ALL_ACCESS, system.as_mut_ptr().cast()) == 0 { return Err(io::Error::last_os_error()); }
            let sd = (&mut *descriptor as *mut SECURITY_DESCRIPTOR).cast();
            if InitializeSecurityDescriptor(sd, 1) == 0 || SetSecurityDescriptorDacl(sd, 1, acl_ptr, 0) == 0 || SetSecurityDescriptorOwner(sd, user.as_mut_ptr().cast(), 0) == 0 || SetSecurityDescriptorControl(sd, SE_DACL_PROTECTED, SE_DACL_PROTECTED) == 0 { return Err(io::Error::last_os_error()); }
        }
        Ok(Self { descriptor, _acl: acl, _owner: user })
    }
    fn attributes(&self) -> windows_sys::Win32::Security::SECURITY_ATTRIBUTES {
        windows_sys::Win32::Security::SECURITY_ATTRIBUTES { nLength: mem::size_of::<windows_sys::Win32::Security::SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: (&*self.descriptor as *const _ as *mut core::ffi::c_void), bInheritHandle: 0 }
    }
}

#[cfg(test)]
mod builder_tests {
    use super::*;
    use windows_sys::Win32::Security::{ACL, ACCESS_ALLOWED_ACE, CONTAINER_INHERIT_ACE, OBJECT_INHERIT_ACE, CreateWellKnownSid, EqualSid, GetAce, GetSecurityDescriptorControl, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner, IsValidAcl, SECURITY_MAX_SID_SIZE, SE_DACL_PROTECTED, WinLocalSystemSid};
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    #[test]
    fn creation_descriptor_is_private_and_inheritable() {
        let security = PrivateDirectorySecurity::new().unwrap();
        let attrs = security.attributes();
        assert_eq!(attrs.bInheritHandle, 0);
        unsafe {
            let mut control = 0; let mut revision = 0;
            assert_ne!(GetSecurityDescriptorControl(attrs.lpSecurityDescriptor, &mut control, &mut revision), 0);
            assert_ne!(control & SE_DACL_PROTECTED, 0);
            let mut present = 0; let mut defaulted = 0; let mut acl: *mut ACL = ptr::null_mut();
            assert_ne!(GetSecurityDescriptorDacl(attrs.lpSecurityDescriptor, &mut present, &mut acl, &mut defaulted), 0);
            assert_ne!(present, 0); assert!(!acl.is_null()); assert_ne!(IsValidAcl(acl), 0); assert_eq!((*acl).AceCount, 2);
            let user = current_user_sid().unwrap(); let mut system = [0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)]; let mut len = SECURITY_MAX_SID_SIZE;
            let mut owner = ptr::null_mut(); let mut defaulted_owner = 0;
            assert_ne!(GetSecurityDescriptorOwner(attrs.lpSecurityDescriptor, &mut owner, &mut defaulted_owner), 0);
            assert_eq!(defaulted_owner, 0); assert!(!owner.is_null()); assert_ne!(EqualSid(owner, user.as_ptr() as *mut _), 0);
            assert_ne!(CreateWellKnownSid(WinLocalSystemSid, ptr::null_mut(), system.as_mut_ptr().cast(), &mut len), 0);
            for (i, expected) in [user.as_ptr() as *mut core::ffi::c_void, system.as_mut_ptr().cast()].into_iter().enumerate() {
                let mut raw = ptr::null_mut(); assert_ne!(GetAce(acl, i as u32, &mut raw), 0);
                let ace = &*raw.cast::<ACCESS_ALLOWED_ACE>();
                assert_eq!(ace.Header.AceType, 0); assert_eq!(ace.Header.AceFlags as u32, OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE); assert_eq!(ace.Mask, FILE_ALL_ACCESS);
                assert_ne!(EqualSid(ptr::addr_of!(ace.SidStart) as *mut _, expected), 0);
            }
        }
    }
}


fn same_parent_path(actual: &[u16], expected: &[u16]) -> bool {
    let slash = [b'\\' as u16];
    actual.strip_suffix(&slash).unwrap_or(actual) == expected.strip_suffix(&slash).unwrap_or(expected)
}
/// Creates or opens one leaf beneath an already verified and retained parent.
/// The caller must keep the verified ancestor chain and parent alive and supply
/// its exact GUID path. This helper does not establish the parent trust itself.
fn create_private_leaf(chain: &DirectoryChain, name: &std::ffi::OsStr) -> io::Result<Token> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::{Foundation::{ERROR_ALREADY_EXISTS, INVALID_HANDLE_VALUE}, Storage::FileSystem::{CreateDirectoryW, CreateFileW, GetFileInformationByHandleEx, GetFileType, GetFinalPathNameByHandleW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TYPE_DISK, FileAttributeTagInfo, FileIdInfo, OPEN_EXISTING, READ_CONTROL, VOLUME_NAME_GUID}};
    let parent = chain._directories.last().map_or(chain._root.handle, |h| h.0);
    let parent_path = &chain.path;
    let name_wide: Vec<u16> = name.encode_wide().collect();
    let mut candidate = vec![b'C' as u16, b':' as u16, b'\\' as u16]; candidate.extend_from_slice(&name_wide);
    let validated = validate_path_syntax(std::path::Path::new(&std::ffi::OsString::from_wide(&candidate)))?;
    if validated[3..].contains(&(b'\\' as u16)) { return Err(io::Error::new(io::ErrorKind::InvalidInput, "leaf name must be one component")); }
    // Bind the supplied handle and path before any filesystem mutation.
    validate_ancestor_acl(parent, true)?;
    let parent_expected: Vec<u16> = parent_path.as_os_str().encode_wide().collect();
    let mut parent_name = vec![0u16; 512];
    loop {
        let n = unsafe { GetFinalPathNameByHandleW(parent, parent_name.as_mut_ptr(), parent_name.len() as u32, VOLUME_NAME_GUID) } as usize;
        if n == 0 { return Err(io::Error::last_os_error()); }
        if n < parent_name.len() {
            let raw_actual = &parent_name[..n];
            if !same_parent_path(raw_actual, &parent_expected) { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "parent handle and path differ")); }
            break;
        }
        if n > 32768 { return Err(io::Error::new(io::ErrorKind::InvalidData, "parent normalized path exceeds limit")); }
        parent_name.resize(n + 1, 0);
    }
    let path = parent_path.join(name);
    let expected: Vec<u16> = path.as_os_str().encode_wide().collect();
    let mut terminated = expected.clone(); terminated.push(0);
    let security = PrivateDirectorySecurity::new()?;
    let attributes = security.attributes();
    unsafe {
        if CreateDirectoryW(terminated.as_ptr(), &attributes) == 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() != Some(ERROR_ALREADY_EXISTS as i32) { return Err(e); }
        }
        let handle = CreateFileW(terminated.as_ptr(), READ_CONTROL | FILE_READ_ATTRIBUTES, FILE_SHARE_READ | FILE_SHARE_WRITE, ptr::null(), OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT, ptr::null_mut());
        if handle == INVALID_HANDLE_VALUE { return Err(io::Error::last_os_error()); }
        let result = Token(handle);
        if GetFileType(handle) != FILE_TYPE_DISK { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "leaf is not a disk object")); }
        let mut attrs = FILE_ATTRIBUTE_TAG_INFO::default(); let mut parent_id = FILE_ID_INFO::default(); let mut id = FILE_ID_INFO::default();
        if GetFileInformationByHandleEx(handle, FileAttributeTagInfo, (&mut attrs as *mut FILE_ATTRIBUTE_TAG_INFO).cast(), mem::size_of_val(&attrs) as u32) == 0 || GetFileInformationByHandleEx(handle, FileIdInfo, (&mut id as *mut FILE_ID_INFO).cast(), mem::size_of_val(&id) as u32) == 0 || GetFileInformationByHandleEx(parent, FileIdInfo, (&mut parent_id as *mut FILE_ID_INFO).cast(), mem::size_of_val(&parent_id) as u32) == 0 { return Err(io::Error::last_os_error()); }
        if attrs.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 || attrs.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || id.VolumeSerialNumber != parent_id.VolumeSerialNumber { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "leaf is not a non-reparse directory on the parent volume")); }
        let mut buffer = vec![0u16; 512];
        loop {
            let n = GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), buffer.len() as u32, VOLUME_NAME_GUID) as usize;
            if n == 0 { return Err(io::Error::last_os_error()); }
            if n < buffer.len() { if buffer[..n] != expected { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "leaf name cannot be verified")); } break; }
            if n > 32768 { return Err(io::Error::new(io::ErrorKind::InvalidData, "normalized path exceeds limit")); }
            buffer.resize(n + 1, 0);
        }
        validate_directory_dacl_snapshot(handle, true)?;
        Ok(result)
    }
}

#[cfg(test)]
mod private_leaf_tests {
    use super::*;
    use std::{fs::{self, OpenOptions}, os::windows::{fs::OpenOptionsExt, io::AsRawHandle}, time::{SystemTime, UNIX_EPOCH}};
    #[test]
    fn creates_and_reopens_private_leaf() {
        let root: Vec<u16> = r"\\?\Volume{abc}\".encode_utf16().collect();
        assert!(same_parent_path(&root, &root[..root.len()-1]));
        assert!(same_parent_path(&root, &root));
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailLeaf{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
        let _cleanup = Cleanup(base.clone());
        let parent = OpenOptions::new().access_mode(0x00060080).custom_flags(0x02000000).open(&base).unwrap();
        let chain = open_directory_chain_with(&base, |_, _| Ok(())).unwrap();
        let first = create_private_leaf(&chain, std::ffi::OsStr::new("state")).unwrap();
        validate_directory_dacl(first.0).unwrap();
        let second = create_private_leaf(&chain, std::ffi::OsStr::new("state")).unwrap();
        validate_directory_dacl(second.0).unwrap();
        // Exercise actual inheritance for both a file and a subdirectory.
        fs::write(base.join("state").join("file"), b"probe").unwrap();
        fs::create_dir(base.join("state").join("subdir")).unwrap();
        for child in [base.join("state").join("file"), base.join("state").join("subdir")] {
            let f = OpenOptions::new().access_mode(0x00020000).custom_flags(if child.is_dir() { 0x02000000 } else { 0 }).open(&child).unwrap();
            unsafe {
                use windows_sys::Win32::{Foundation::LocalFree, Security::{ACL, INHERITED_ACE, GetAce, Authorization::{GetSecurityInfo, SE_FILE_OBJECT}, DACL_SECURITY_INFORMATION}};
                let mut acl: *mut ACL = ptr::null_mut(); let mut sd = ptr::null_mut();
                assert_eq!(GetSecurityInfo(f.as_raw_handle() as HANDLE, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(), &mut acl, ptr::null_mut(), &mut sd), 0);
                assert!(!acl.is_null()); assert_eq!((*acl).AceCount, 2);
                use windows_sys::Win32::Security::{ACCESS_ALLOWED_ACE, CreateWellKnownSid, EqualSid, WinLocalSystemSid, SECURITY_MAX_SID_SIZE, OBJECT_INHERIT_ACE, CONTAINER_INHERIT_ACE};
                use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
                let user = current_user_sid().unwrap(); let mut system = [0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)]; let mut size = SECURITY_MAX_SID_SIZE;
                assert_ne!(CreateWellKnownSid(WinLocalSystemSid, ptr::null_mut(), system.as_mut_ptr().cast(), &mut size), 0);
                let mut seen = [false; 2];
                for i in 0..2 {
                    let mut raw = ptr::null_mut(); assert_ne!(GetAce(acl, i, &mut raw), 0);
                    let ace = &*raw.cast::<ACCESS_ALLOWED_ACE>();
                    assert_eq!(ace.Header.AceType, 0); assert_ne!(ace.Header.AceFlags as u32 & INHERITED_ACE, 0);
                    assert_eq!(ace.Mask & FILE_ALL_ACCESS, FILE_ALL_ACCESS);
                    if child.is_dir() { assert_eq!(ace.Header.AceFlags as u32 & (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE), OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE); }
                    let sid = ptr::addr_of!(ace.SidStart) as *mut _;
                    if EqualSid(sid, user.as_ptr() as *mut _) != 0 { seen[0] = true; }
                    if EqualSid(sid, system.as_mut_ptr().cast()) != 0 { seen[1] = true; }
                }
                assert_eq!(seen, [true, true]);
                LocalFree(sd);
            }
        }
        drop(second); drop(first);
        // A private ACL without inheritable entries is rejected without repair.
        {
            use windows_sys::Win32::Security::{ACL, ACL_REVISION, AddAccessAllowedAce, InitializeAcl, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, Authorization::{SetSecurityInfo, SE_FILE_OBJECT}};
            let leaf = OpenOptions::new().access_mode(0x00060000).custom_flags(0x02000000).open(base.join("state")).unwrap();
            let user = current_user_sid().unwrap(); let mut storage = [0u32; 128];
            let acl = storage.as_mut_ptr().cast::<ACL>();
            unsafe {
                assert_ne!(InitializeAcl(acl, mem::size_of_val(&storage) as u32, ACL_REVISION), 0);
                assert_ne!(AddAccessAllowedAce(acl, ACL_REVISION, 0x1f01ff, user.as_ptr() as *mut _), 0);
                assert_eq!(SetSecurityInfo(leaf.as_raw_handle() as HANDLE, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(), acl, ptr::null()), 0);
            }
            drop(leaf);
            assert_eq!(create_private_leaf(&chain, std::ffi::OsStr::new("state")).err().unwrap().to_string(), "leaf DACL lacks full inheritable user and SYSTEM access");
        }
        // Keep one handle open to compare identity and exact ACL bytes across rejection.
        {
            use windows_sys::Win32::{Foundation::LocalFree, Security::{ACL, ACL_REVISION, AddAccessAllowedAceEx, CreateWellKnownSid, InitializeAcl, WinLocalSystemSid, SECURITY_MAX_SID_SIZE, OBJECT_INHERIT_ACE, CONTAINER_INHERIT_ACE, NO_PROPAGATE_INHERIT_ACE, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, Authorization::{GetSecurityInfo, SetSecurityInfo, SE_FILE_OBJECT}}, Storage::FileSystem::{FILE_ALL_ACCESS, FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx}};
            let leaf = OpenOptions::new().access_mode(0x00060000).custom_flags(0x02000000).open(base.join("state")).unwrap();
            let h = leaf.as_raw_handle() as HANDLE; let user = current_user_sid().unwrap();
            let mut system = [0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)]; let mut size = SECURITY_MAX_SID_SIZE;
            unsafe { assert_ne!(CreateWellKnownSid(WinLocalSystemSid, ptr::null_mut(), system.as_mut_ptr().cast(), &mut size), 0); }
            let snapshot = || unsafe {
                let mut id = FILE_ID_INFO::default(); assert_ne!(GetFileInformationByHandleEx(h, FileIdInfo, (&mut id as *mut FILE_ID_INFO).cast(), mem::size_of_val(&id) as u32), 0);
                let mut acl: *mut ACL = ptr::null_mut(); let mut sd = ptr::null_mut();
                assert_eq!(GetSecurityInfo(h, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(), &mut acl, ptr::null_mut(), &mut sd), 0);
                let bytes = std::slice::from_raw_parts(acl.cast::<u8>(), (*acl).AclSize as usize).to_vec(); LocalFree(sd);
                (id.VolumeSerialNumber, id.FileId.Identifier, bytes)
            };
            for (flags, mask) in [(OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE | NO_PROPAGATE_INHERIT_ACE, FILE_ALL_ACCESS), (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE, 0x1200a9)] {
                let mut storage = [0u32; 128]; let acl = storage.as_mut_ptr().cast::<ACL>();
                unsafe {
                    assert_ne!(InitializeAcl(acl, mem::size_of_val(&storage) as u32, ACL_REVISION), 0);
                    assert_ne!(AddAccessAllowedAceEx(acl, ACL_REVISION, flags, mask, user.as_ptr() as *mut _), 0);
                    assert_ne!(AddAccessAllowedAceEx(acl, ACL_REVISION, OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE, FILE_ALL_ACCESS, system.as_mut_ptr().cast()), 0);
                    assert_eq!(SetSecurityInfo(h, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(), acl, ptr::null()), 0);
                }
                let before = snapshot();
                assert_eq!(create_private_leaf(&chain, std::ffi::OsStr::new("state")).err().unwrap().to_string(), "leaf DACL lacks full inheritable user and SYSTEM access");
                assert_eq!(snapshot(), before);
            }
        }
        let other = base.join("other"); fs::create_dir(&other).unwrap();
        let mut wrong_chain = open_directory_chain_with(&base, |_, _| Ok(())).unwrap();
        wrong_chain.path = validate_local_volume_path(&other).unwrap();
        let mismatch = create_private_leaf(&wrong_chain, std::ffi::OsStr::new("must-not-exist")).err().unwrap();
        assert_eq!(mismatch.to_string(), "parent handle and path differ");
        assert!(!other.join("must-not-exist").exists());
        let junction = base.join("junction");
        let output = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&junction).arg(base.join("state")).output().unwrap();
        assert!(output.status.success(), "junction fixture failed");
        assert!(create_private_leaf(&chain, std::ffi::OsStr::new("junction")).is_err());
        fs::remove_dir(&junction).unwrap();
        {
            use windows_sys::Win32::Security::{ACL, ACL_REVISION, AddAccessAllowedAce, AddAccessAllowedAceEx, CONTAINER_INHERIT_ACE, OBJECT_INHERIT_ACE, CreateWellKnownSid, InitializeAcl, WinWorldSid, SECURITY_MAX_SID_SIZE, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, Authorization::{SetSecurityInfo, SE_FILE_OBJECT}};
            let user = current_user_sid().unwrap(); let mut world = [0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)]; let mut len = SECURITY_MAX_SID_SIZE;
            let mut storage = [0u32; 128]; let acl = storage.as_mut_ptr().cast::<ACL>();
            unsafe {
                assert_ne!(CreateWellKnownSid(WinWorldSid, ptr::null_mut(), world.as_mut_ptr().cast(), &mut len), 0);
                assert_ne!(InitializeAcl(acl, mem::size_of_val(&storage) as u32, ACL_REVISION), 0);
                assert_ne!(AddAccessAllowedAceEx(acl, ACL_REVISION, OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE, 0x1f01ff, user.as_ptr() as *mut _), 0);
                assert_ne!(AddAccessAllowedAce(acl, ACL_REVISION, 2, world.as_mut_ptr().cast()), 0);
                assert_eq!(SetSecurityInfo(parent.as_raw_handle() as HANDLE, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(), acl, ptr::null()), 0);
            }
            assert_eq!(create_private_leaf(&chain, std::ffi::OsStr::new("blocked")).err().unwrap().to_string(), "ancestor ACE grants dangerous access");
            assert!(!base.join("blocked").exists());
        }
        drop(parent);
    }
}

/// Conservative existing-leaf capability check: require full inheritable
/// grants to both principals and reject all denies, rather than attempting to
/// model token-specific deny ordering.
unsafe fn validate_leaf_inheritance(acl: *mut windows_sys::Win32::Security::ACL) -> io::Result<()> {
    use windows_sys::Win32::{Security::{ACE_HEADER, CONTAINER_INHERIT_ACE, OBJECT_INHERIT_ACE, INHERIT_ONLY_ACE, NO_PROPAGATE_INHERIT_ACE, CreateWellKnownSid, EqualSid, GetAce, SECURITY_MAX_SID_SIZE, WinLocalSystemSid}, Storage::FileSystem::FILE_ALL_ACCESS};
unsafe {
        let user = current_user_sid()?; let mut system = [0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)]; let mut len = SECURITY_MAX_SID_SIZE;
        if CreateWellKnownSid(WinLocalSystemSid, ptr::null_mut(), system.as_mut_ptr().cast(), &mut len) == 0 { return Err(io::Error::last_os_error()); }
        let mut found = [false; 2];
        for i in 0..(*acl).AceCount as u32 {
            let mut ace = ptr::null_mut(); if GetAce(acl, i, &mut ace) == 0 { return Err(io::Error::last_os_error()); }
            let header = &*ace.cast::<ACE_HEADER>();
            if header.AceType != 0 { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "leaf DACL has a deny or unsupported ACE")); }
            let bytes = std::slice::from_raw_parts(ace.cast::<u8>(), header.AceSize as usize);
            let (flags, mask, offset) = parse_ancestor_ace(bytes)?;
            let sid = ace.cast::<u8>().add(offset).cast();
            if IsValidSid(sid) == 0 { return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid leaf SID")); }
            let required = OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE;
            if flags as u32 & required == required && flags as u32 & (INHERIT_ONLY_ACE | NO_PROPAGATE_INHERIT_ACE) == 0 && mask & FILE_ALL_ACCESS == FILE_ALL_ACCESS {
                if EqualSid(sid, user.as_ptr() as *mut _) != 0 { found[0] = true; }
                if EqualSid(sid, system.as_mut_ptr().cast()) != 0 { found[1] = true; }
            }
        }
        if !found.into_iter().all(|v| v) { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "leaf DACL lacks full inheritable user and SYSTEM access")); }
        Ok(())
    }
}


















#[cfg(test)]
mod ancestor_creation_position_tests {
    use super::*;
    use std::{fs::{self, OpenOptions}, os::windows::{fs::OpenOptionsExt, io::AsRawHandle}, time::{SystemTime, UNIX_EPOCH}};
    use windows_sys::Win32::{Security::{ACL, ACL_REVISION, AddAccessAllowedAce, AddAccessAllowedAceEx, CreateWellKnownSid, InitializeAcl, WinWorldSid, SECURITY_MAX_SID_SIZE, OBJECT_INHERIT_ACE, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, Authorization::{SetSecurityInfo, SE_FILE_OBJECT}}, Storage::FileSystem::{FILE_ALL_ACCESS, FILE_WRITE_DATA, FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx}};
    #[test]
    fn creatable_ancestor_and_safe_final_parent() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailPosition{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
        let _cleanup = Cleanup(base.clone());
        let parent_path = base.join("safeparent"); fs::create_dir(&parent_path).unwrap();
        let open = |p: &std::path::Path| OpenOptions::new().access_mode(0x00060080).custom_flags(0x02000000).open(p).unwrap();
        let ancestor = open(&base); let parent = open(&parent_path);
        unsafe {
            let user = current_user_sid().unwrap(); let mut world = [0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)]; let mut size = SECURITY_MAX_SID_SIZE;
            assert_ne!(CreateWellKnownSid(WinWorldSid, ptr::null_mut(), world.as_mut_ptr().cast(), &mut size), 0);
            for (file, allow_add) in [(&ancestor, true), (&parent, false)] {
                let mut storage = [0u32; 128]; let acl = storage.as_mut_ptr().cast::<ACL>();
                assert_ne!(InitializeAcl(acl, mem::size_of_val(&storage) as u32, ACL_REVISION), 0);
                assert_ne!(AddAccessAllowedAceEx(acl, ACL_REVISION, OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE, FILE_ALL_ACCESS, user.as_ptr() as *mut _), 0);
                if allow_add { assert_ne!(AddAccessAllowedAce(acl, ACL_REVISION, FILE_WRITE_DATA, world.as_mut_ptr().cast()), 0); }
                assert_eq!(SetSecurityInfo(file.as_raw_handle() as HANDLE, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(), acl, ptr::null()), 0);
            }
            let mut wanted = FILE_ID_INFO::default();
            assert_ne!(GetFileInformationByHandleEx(ancestor.as_raw_handle() as HANDLE, FileIdInfo, (&mut wanted as *mut FILE_ID_INFO).cast(), mem::size_of_val(&wanted) as u32), 0);
            let chain = open_directory_chain_with(&parent_path, |h, _| {
                let mut id = FILE_ID_INFO::default();
                if GetFileInformationByHandleEx(h, FileIdInfo, (&mut id as *mut FILE_ID_INFO).cast(), mem::size_of_val(&id) as u32) == 0 { return Err(io::Error::last_os_error()); }
                if id.VolumeSerialNumber == wanted.VolumeSerialNumber && id.FileId.Identifier == wanted.FileId.Identifier { validate_ancestor_acl(h, false) } else { Ok(()) }
            }).unwrap();
            drop(create_private_leaf(&chain, std::ffi::OsStr::new("leaf")).unwrap());
        }
    }
}

/// An admitted state directory and the directory handles protecting its path.
/// This type alone does not provide exclusive locking or SQLite admission.
pub(super) struct StateDirectory {
    _parent_chain: DirectoryChain,
    _leaf: Token,
    path: std::path::PathBuf,
}
impl StateDirectory {
    pub(super) fn acquire(path: &std::path::Path) -> io::Result<Self> {
        validate_path_syntax(path)?;
        let name = path.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "state directory must have a leaf name"))?;
        let parent = path.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "state directory must have a parent"))?;
        let component = name.to_string_lossy();
        let parent_chain = open_existing_directory_chain(parent).map_err(|e| { #[cfg(feature="producer-test-fixture")] { crate::producer::faults::process_marker("stage-parent-chain"); crate::producer::faults::mark_os_error(&e); } io::Error::new(e.kind(), format!("open parent for {component}: {e}")) })?;
        let leaf = create_private_leaf(&parent_chain, name).map_err(|e| { #[cfg(feature="producer-test-fixture")] { crate::producer::faults::process_marker("stage-leaf"); crate::producer::faults::mark_os_error(&e); } io::Error::new(e.kind(), format!("acquire {component}: {e}")) })?;
        let path = parent_chain.path.join(name);
        Ok(Self { _parent_chain: parent_chain, _leaf: leaf, path })
    }
    pub(super) fn path(&self) -> &std::path::Path { &self.path }
}

#[cfg(test)]
mod state_acquire_tests {
    use super::*;
    use std::{fs, time::{SystemTime, UNIX_EPOCH}};
    #[test]
    fn acquire_workspace_state_directory() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailAcquire{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
        let _cleanup = Cleanup(base.clone());
        let state = StateDirectory::acquire(&base.join("state")).unwrap();
        assert!(state.path().to_string_lossy().starts_with(r"\\?\Volume{"));
        assert!(state.path().ends_with("state"));
        assert!(base.join("state").is_dir());
        validate_directory_dacl(state._leaf.0).unwrap();
        drop(state);
        let reopened = StateDirectory::acquire(&base.join("state")).unwrap();
        assert!(reopened.path().ends_with("state"));
    }
}


#[cfg(test)]
mod state_acquire_negative_tests {
    use super::*;
    use std::{fs::{self, OpenOptions}, os::windows::{fs::OpenOptionsExt, io::AsRawHandle}, time::{SystemTime, UNIX_EPOCH}};
    use windows_sys::Win32::Security::{ACL, ACL_REVISION, AddAccessAllowedAce, AddAccessAllowedAceEx, CreateWellKnownSid, InitializeAcl, WinWorldSid, SECURITY_MAX_SID_SIZE, OBJECT_INHERIT_ACE, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, Authorization::{SetSecurityInfo, SE_FILE_OBJECT}};
    #[test]
    fn rejects_unsafe_parent_leaf_and_junction_without_changing_objects() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailAcquireNegative{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
        let _cleanup = Cleanup(base.clone());
        let parent = OpenOptions::new().access_mode(0x00060000).custom_flags(0x02000000).open(&base).unwrap();
        let user = current_user_sid().unwrap(); let mut world = [0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)]; let mut size = SECURITY_MAX_SID_SIZE;
        unsafe { assert_ne!(CreateWellKnownSid(WinWorldSid, ptr::null_mut(), world.as_mut_ptr().cast(), &mut size), 0); }
        let set_acl = |file: &fs::File, world_mask: u32| unsafe {
            let mut storage = [0u32; 128]; let acl = storage.as_mut_ptr().cast::<ACL>();
            assert_ne!(InitializeAcl(acl, mem::size_of_val(&storage) as u32, ACL_REVISION), 0);
            assert_ne!(AddAccessAllowedAceEx(acl, ACL_REVISION, OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE, 0x1f01ff, user.as_ptr() as *mut _), 0);
            if world_mask != 0 { assert_ne!(AddAccessAllowedAce(acl, ACL_REVISION, world_mask, world.as_ptr() as *mut _), 0); }
            assert_eq!(SetSecurityInfo(file.as_raw_handle() as HANDLE, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(), acl, ptr::null()), 0);
        };
        set_acl(&parent, 2);
        let e = StateDirectory::acquire(&base.join("blocked")).err().unwrap();
        assert_eq!(e.to_string(), "acquire blocked: ancestor ACE grants dangerous access"); assert!(!base.join("blocked").exists());
        set_acl(&parent, 0);
        let leaf_path = base.join("weak"); fs::create_dir(&leaf_path).unwrap();
        let leaf = OpenOptions::new().access_mode(0x00060000).custom_flags(0x02000000).open(&leaf_path).unwrap();
        set_acl(&leaf, 1);
        let snapshot = |file: &fs::File| unsafe {
            use windows_sys::Win32::{Foundation::LocalFree, Security::Authorization::GetSecurityInfo, Storage::FileSystem::{FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx}};
            let mut id = FILE_ID_INFO::default(); let h = file.as_raw_handle() as HANDLE;
            assert_ne!(GetFileInformationByHandleEx(h, FileIdInfo, (&mut id as *mut FILE_ID_INFO).cast(), mem::size_of_val(&id) as u32), 0);
            let mut acl: *mut ACL = ptr::null_mut(); let mut sd = ptr::null_mut();
            assert_eq!(GetSecurityInfo(h, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(), &mut acl, ptr::null_mut(), &mut sd), 0);
            let bytes = std::slice::from_raw_parts(acl.cast::<u8>(), (*acl).AclSize as usize).to_vec(); LocalFree(sd);
            (id.VolumeSerialNumber, id.FileId.Identifier, bytes)
        };
        let before = snapshot(&leaf);
        let e = StateDirectory::acquire(&leaf_path).err().unwrap();
        assert_eq!(e.to_string(), "acquire weak: directory DACL grants access to an unexpected principal"); assert!(leaf_path.is_dir());
        let reopened = OpenOptions::new().access_mode(0x00020000).custom_flags(0x02200000).open(&leaf_path).unwrap();
        assert_eq!(snapshot(&reopened), before); drop(reopened);
        drop(leaf);
        let target = base.join("target"); fs::create_dir(&target).unwrap(); fs::write(target.join("sentinel"), b"unchanged").unwrap();
        let junction = base.join("junction"); let output = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&junction).arg(&target).output().unwrap(); assert!(output.status.success());
        let e = StateDirectory::acquire(&junction).err().unwrap();
        assert_eq!(e.to_string(), "acquire junction: leaf is not a non-reparse directory on the parent volume");
        assert_eq!(fs::read(target.join("sentinel")).unwrap(), b"unchanged");
        let ancestor_error = StateDirectory::acquire(&junction.join("child")).err().unwrap();
        assert!(ancestor_error.to_string().contains("non-reparse directory"));
        assert!(!target.join("child").exists()); fs::remove_dir(&junction).unwrap();
    }
}


#[cfg(test)]
mod cross_process_acquire_tests {
    use super::*;
    use std::{fs, io::Write, process::{Command, Stdio}, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};
    #[test]
    fn acquire_handles_live_until_drop_across_processes() {
        if let Some(dir) = std::env::var_os("MINDTRAIL_ACQUIRE_CHILD") {
            let base = std::path::PathBuf::from(dir);
            let state = StateDirectory::acquire(&base.join("state")).unwrap();
            let handles: Vec<String> = std::iter::once(state._parent_chain._root.handle).chain(state._parent_chain._directories.iter().map(|h| h.0)).chain(std::iter::once(state._leaf.0)).map(|h| (h as usize).to_string()).collect();
            fs::write(base.join("ready.tmp"), handles.join(",")).unwrap(); fs::rename(base.join("ready.tmp"), base.join("ready")).unwrap();
            let deadline = Instant::now() + Duration::from_secs(20);
            while !base.join("release").exists() { assert!(Instant::now() < deadline, "parent did not release"); std::thread::sleep(Duration::from_millis(10)); }
            drop(state);
            fs::write(base.join("dropped.tmp"), b"done").unwrap(); fs::rename(base.join("dropped.tmp"), base.join("dropped")).unwrap();
            let mut input = String::new(); std::io::stdin().read_line(&mut input).unwrap();
            return;
        }
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailCross{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
        let _cleanup = Cleanup(base.clone());
        let child = Command::new(std::env::current_exe().unwrap()).args(["--exact", "gateway::windows_security::cross_process_acquire_tests::acquire_handles_live_until_drop_across_processes", "--nocapture"]).env("MINDTRAIL_ACQUIRE_CHILD", &base).stdin(Stdio::piped()).stdout(Stdio::null()).spawn().unwrap();
        struct ChildGuard(std::process::Child); impl Drop for ChildGuard { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }
        let mut child = ChildGuard(child);
        let wait_file = |p: &std::path::Path| { let deadline = Instant::now() + Duration::from_secs(20); while !p.exists() { assert!(Instant::now() < deadline, "child barrier timed out"); std::thread::sleep(Duration::from_millis(10)); } };
        wait_file(&base.join("ready"));
        let ids: Vec<usize> = fs::read_to_string(base.join("ready")).unwrap().split(',').map(|v| v.parse().unwrap()).collect();
        use windows_sys::Win32::{Foundation::{CloseHandle, DuplicateHandle, DUPLICATE_SAME_ACCESS}, System::Threading::{GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE}};
        unsafe {
            let process = OpenProcess(PROCESS_DUP_HANDLE, 0, child.0.id()); assert!(!process.is_null());
            for &id in &ids { let mut duplicate = ptr::null_mut(); assert_ne!(DuplicateHandle(process, id as HANDLE, GetCurrentProcess(), &mut duplicate, 0, 0, DUPLICATE_SAME_ACCESS), 0); CloseHandle(duplicate); }
            fs::write(base.join("release"), b"go").unwrap(); wait_file(&base.join("dropped"));
            for &id in &ids { let mut duplicate = ptr::null_mut(); if DuplicateHandle(process, id as HANDLE, GetCurrentProcess(), &mut duplicate, 0, 0, DUPLICATE_SAME_ACCESS) != 0 { CloseHandle(duplicate); } }
            CloseHandle(process);
        }
        child.0.stdin.take().unwrap().write_all(b"done\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop { if let Some(status) = child.0.try_wait().unwrap() { assert!(status.success()); break; } assert!(Instant::now() < deadline, "child exit timed out"); std::thread::sleep(Duration::from_millis(10)); }
    }
}



fn file_access_expand(mut mask: u32) -> u32 {
    use windows_sys::Win32::{Foundation::{GENERIC_READ, GENERIC_WRITE, GENERIC_EXECUTE, GENERIC_ALL}, Storage::FileSystem::{FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_GENERIC_EXECUTE, FILE_ALL_ACCESS}};
    for (generic, mapped) in [(GENERIC_READ, FILE_GENERIC_READ), (GENERIC_WRITE, FILE_GENERIC_WRITE), (GENERIC_EXECUTE, FILE_GENERIC_EXECUTE), (GENERIC_ALL, FILE_ALL_ACCESS)] {
        if mask & generic != 0 { mask = (mask & !generic) | mapped; }
    }
    mask
}
fn file_grants_sufficient(grants: [u32; 2], same_principal: bool) -> bool {
    use windows_sys::Win32::Storage::FileSystem::{FILE_GENERIC_READ, FILE_GENERIC_WRITE};
    let required = FILE_GENERIC_READ | FILE_GENERIC_WRITE;
    grants[0] & required == required && (same_principal || grants[1] & required == required)
}
/// Validates a file handle's owner and DACL; callers must validate file type,
/// reparse status, link count and trusted parent independently.
fn validate_private_file_acl(handle: HANDLE) -> io::Result<()> {
    use windows_sys::Win32::{Foundation::LocalFree, Security::{Authorization::{GetSecurityInfo, SE_FILE_OBJECT}, ACL, ACE_HEADER, CreateWellKnownSid, EqualSid, GetAce, IsValidAcl, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, SECURITY_MAX_SID_SIZE, WinLocalSystemSid, INHERIT_ONLY_ACE}};
    struct Descriptor(*mut core::ffi::c_void); impl Drop for Descriptor { fn drop(&mut self) { unsafe { LocalFree(self.0); } } }
    unsafe {
        let user = current_user_sid()?; let mut system = [0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)]; let mut len = SECURITY_MAX_SID_SIZE;
        if CreateWellKnownSid(WinLocalSystemSid, ptr::null_mut(), system.as_mut_ptr().cast(), &mut len) == 0 { return Err(io::Error::last_os_error()); }
        let mut owner = ptr::null_mut(); let mut acl: *mut ACL = ptr::null_mut(); let mut sd = ptr::null_mut();
        let status = GetSecurityInfo(handle, SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION, &mut owner, ptr::null_mut(), &mut acl, ptr::null_mut(), &mut sd);
        if status != 0 { return Err(io::Error::from_raw_os_error(status as i32)); }
        let _sd = Descriptor(sd);
        let invalid = |s| io::Error::new(io::ErrorKind::PermissionDenied, s);
        if owner.is_null() || IsValidSid(owner) == 0 || (EqualSid(owner, user.as_ptr() as *mut _) == 0 && EqualSid(owner, system.as_mut_ptr().cast()) == 0) { return Err(invalid("file owner is not trusted")); }
        if acl.is_null() || IsValidAcl(acl) == 0 { return Err(invalid("file DACL is missing or invalid")); }
        let mut grants = [0u32; 2];
        for i in 0..(*acl).AceCount as u32 {
            let mut raw = ptr::null_mut(); if GetAce(acl, i, &mut raw) == 0 { return Err(io::Error::last_os_error()); }
            let header = &*raw.cast::<ACE_HEADER>();
            if header.AceType != 0 { return Err(invalid("file DACL has a deny or unsupported ACE")); }
            let bytes = std::slice::from_raw_parts(raw.cast::<u8>(), header.AceSize as usize);
            let (flags, mask, offset) = parse_ancestor_ace(bytes)?;
            let sid = raw.cast::<u8>().add(offset).cast();
            if IsValidSid(sid) == 0 { return Err(invalid("file DACL has an invalid SID")); }
            let who = if EqualSid(sid, user.as_ptr() as *mut _) != 0 { 0 } else if EqualSid(sid, system.as_mut_ptr().cast()) != 0 { 1 } else { return Err(invalid("file DACL grants an unexpected principal")); };
            if flags as u32 & INHERIT_ONLY_ACE == 0 { grants[who] |= file_access_expand(mask); }
        }
        let same_principal = EqualSid(user.as_ptr() as *mut _, system.as_mut_ptr().cast()) != 0;
        if !file_grants_sufficient(grants, same_principal) { return Err(invalid("file DACL lacks user or SYSTEM read/write access")); }
        Ok(())
    }
}

#[cfg(test)]
mod private_file_acl_tests {
    use super::*;
    use std::{fs::{self, OpenOptions}, os::windows::{fs::OpenOptionsExt, io::AsRawHandle}, time::{SystemTime, UNIX_EPOCH}};
    use windows_sys::Win32::Security::{ACL, ACL_REVISION, AddAccessAllowedAce, AddAuditAccessAce, CreateWellKnownSid, InitializeAcl, WinWorldSid, SECURITY_MAX_SID_SIZE, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, Authorization::{SetSecurityInfo, SE_FILE_OBJECT}};
    #[test]
    fn inherited_private_file_and_unsafe_entries() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailFileAcl{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { if let Err(e) = fs::remove_dir_all(&self.0) { eprintln!("fixture cleanup failed for {}: {e}", self.0.display()); if !std::thread::panicking() { panic!("fixture cleanup failed"); } } } }
        let _cleanup = Cleanup(base.clone());
        let state = StateDirectory::acquire(&base.join("state")).unwrap();
        let file_path = base.join("state").join("file"); fs::write(&file_path, b"test").unwrap();
        let file = OpenOptions::new().access_mode(0x00060000).open(&file_path).unwrap(); let h = file.as_raw_handle() as HANDLE;
        validate_private_file_acl(h).unwrap();
        unsafe {
            let user = current_user_sid().unwrap(); let mut world = [0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)]; let mut len = SECURITY_MAX_SID_SIZE;
            assert_ne!(CreateWellKnownSid(WinWorldSid, ptr::null_mut(), world.as_mut_ptr().cast(), &mut len), 0);
            {
                use windows_sys::Win32::{Foundation::{GENERIC_READ, GENERIC_WRITE}, Security::WinLocalSystemSid};
                let mut system = [0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)]; let mut system_len = SECURITY_MAX_SID_SIZE;
                assert_ne!(CreateWellKnownSid(WinLocalSystemSid, ptr::null_mut(), system.as_mut_ptr().cast(), &mut system_len), 0);
                let mut storage = [0u32; 128]; let acl = storage.as_mut_ptr().cast::<ACL>();
                assert_ne!(InitializeAcl(acl, mem::size_of_val(&storage) as u32, ACL_REVISION), 0);
                assert_ne!(AddAccessAllowedAce(acl, ACL_REVISION, GENERIC_READ | GENERIC_WRITE, user.as_ptr() as *mut _), 0);
                assert_ne!(AddAccessAllowedAce(acl, ACL_REVISION, GENERIC_READ | GENERIC_WRITE, system.as_mut_ptr().cast()), 0);
                assert_eq!(SetSecurityInfo(h, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(), acl, ptr::null()), 0);
                validate_private_file_acl(h).unwrap();
            }
            for audit in [false, true] {
                let mut storage = [0u32; 128]; let acl = storage.as_mut_ptr().cast::<ACL>();
                assert_ne!(InitializeAcl(acl, mem::size_of_val(&storage) as u32, ACL_REVISION), 0);
                if audit { assert_ne!(AddAuditAccessAce(acl, ACL_REVISION, 1, world.as_mut_ptr().cast(), 1, 0), 0); }
                else { assert_ne!(AddAccessAllowedAce(acl, ACL_REVISION, 1, world.as_mut_ptr().cast()), 0); }
                assert_ne!(AddAccessAllowedAce(acl, ACL_REVISION, 0x1f01ff, user.as_ptr() as *mut _), 0);
                assert_eq!(SetSecurityInfo(h, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(), acl, ptr::null()), 0);
                let error = validate_private_file_acl(h).unwrap_err().to_string();
                assert_eq!(error, if audit { "file DACL has a deny or unsupported ACE" } else { "file DACL grants an unexpected principal" });
            }
        }
        drop(file); drop(state);
    }
}


#[cfg(test)]
mod file_generic_tests {
    use super::*;
    use windows_sys::Win32::{Foundation::{GENERIC_ALL, GENERIC_READ, GENERIC_WRITE}, Storage::FileSystem::{FILE_GENERIC_READ, FILE_GENERIC_WRITE}};
    #[test]
    fn generic_rights_and_identical_system_user() {
        let both = FILE_GENERIC_READ | FILE_GENERIC_WRITE;
        assert_eq!(file_access_expand(GENERIC_READ | GENERIC_WRITE) & both, both);
        assert_eq!(file_access_expand(GENERIC_ALL) & both, both);
        assert!(file_grants_sufficient([both, 0], true));
        assert!(!file_grants_sufficient([both, 0], false));
        assert!(file_grants_sufficient([file_access_expand(GENERIC_READ) | FILE_GENERIC_WRITE, both], false));
    }
}


/// Checks an already-open handle; the caller remains responsible for how it
/// was opened, its path, its security descriptor and its lifetime.
fn validate_private_file_object(handle: HANDLE) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{GetFileType, GetFileInformationByHandleEx, FILE_TYPE_DISK, FILE_ATTRIBUTE_TAG_INFO, FILE_ATTRIBUTE_REPARSE_POINT, FILE_STANDARD_INFO, FileAttributeTagInfo, FileStandardInfo};
    unsafe {
        windows_sys::Win32::Foundation::SetLastError(0);
        let file_type = GetFileType(handle);
        if file_type == 0 { let e = io::Error::last_os_error(); if e.raw_os_error() != Some(0) { return Err(e); } }
        if file_type != FILE_TYPE_DISK { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "file handle is not a disk object")); }
        let mut attributes = FILE_ATTRIBUTE_TAG_INFO::default();
        let mut standard = FILE_STANDARD_INFO::default();
        if GetFileInformationByHandleEx(handle, FileAttributeTagInfo, (&mut attributes as *mut FILE_ATTRIBUTE_TAG_INFO).cast(), mem::size_of_val(&attributes) as u32) == 0 { return Err(io::Error::last_os_error()); }
        if GetFileInformationByHandleEx(handle, FileStandardInfo, (&mut standard as *mut FILE_STANDARD_INFO).cast(), mem::size_of_val(&standard) as u32) == 0 { return Err(io::Error::last_os_error()); }
        if attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "file is a reparse point")); }
        if standard.Directory { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "file handle refers to a directory")); }
        if standard.NumberOfLinks != 1 { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "file has multiple or invalid hard links")); }
        Ok(())
    }
}

#[cfg(test)]
mod file_object_tests {
    use super::*;
    use std::{fs::{self, OpenOptions}, os::windows::{fs::OpenOptionsExt, io::AsRawHandle}, time::{SystemTime, UNIX_EPOCH}};
    #[test]
    fn checks_file_directory_links_and_reparse() {
        assert_eq!(validate_private_file_object(windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE).unwrap_err().raw_os_error(), Some(6));
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailFileObject{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { if let Err(e) = fs::remove_dir_all(&self.0) { eprintln!("cleanup {}: {e}", self.0.display()); if !std::thread::panicking() { panic!("fixture cleanup failed"); } } } }
        let _cleanup = Cleanup(base.clone());
        let target = base.join("target"); fs::write(&target, b"sentinel").unwrap();
        let file = OpenOptions::new().read(true).open(&target).unwrap();
        validate_private_file_object(file.as_raw_handle() as HANDLE).unwrap();
        let dir = OpenOptions::new().read(true).custom_flags(0x02000000).open(&base).unwrap();
        assert_eq!(validate_private_file_object(dir.as_raw_handle() as HANDLE).unwrap_err().to_string(), "file handle refers to a directory");
        fs::hard_link(&target, base.join("link")).unwrap();
        assert_eq!(validate_private_file_object(file.as_raw_handle() as HANDLE).unwrap_err().to_string(), "file has multiple or invalid hard links");
        assert_eq!(fs::read(&target).unwrap(), b"sentinel");
        let junction = base.join("junction");
        let output = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&junction).arg(&base).output().unwrap();
        assert!(output.status.success(), "junction fixture unavailable");
        let reparse = OpenOptions::new().access_mode(0x80).custom_flags(0x02200000).open(&junction).unwrap();
        assert_eq!(validate_private_file_object(reparse.as_raw_handle() as HANDLE).unwrap_err().to_string(), "file is a reparse point");
        drop(reparse); fs::remove_dir(&junction).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"sentinel");
    }
}



/// A checked file handle tied to the retained state directory.
struct CheckedPrivateFile<'a> { _handle: Token, _directory: std::marker::PhantomData<&'a StateDirectory> }
impl StateDirectory {
    /// Read-only validation handle; permits delete sharing for later replacement.
    fn open_existing_private_file(&self, name: &'static str) -> io::Result<CheckedPrivateFile<'_>> { self.open_existing_private_file_with_access(name, false) }
    fn open_existing_private_file_with_access(&self, name: &'static str, delete: bool) -> io::Result<CheckedPrivateFile<'_>> { self.open_existing_private_file_checked(name, delete, || Ok(())) }
    fn open_existing_private_file_checked(&self, name: &'static str, delete: bool, check: impl FnOnce() -> io::Result<()>) -> io::Result<CheckedPrivateFile<'_>> {
        self.open_existing_private_file_checked_access(name, delete, false, check)
    }
    fn open_existing_private_file_checked_access(&self, name: &'static str, delete: bool, read_data: bool, check: impl FnOnce() -> io::Result<()>) -> io::Result<CheckedPrivateFile<'_>> {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        use windows_sys::Win32::{Foundation::INVALID_HANDLE_VALUE, Storage::FileSystem::{CreateFileW, GetFileInformationByHandleEx, GetFinalPathNameByHandleW, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_SHARE_DELETE, FileIdInfo, OPEN_EXISTING, READ_CONTROL, DELETE, VOLUME_NAME_GUID}};
        let words: Vec<u16> = std::ffi::OsStr::new(name).encode_wide().collect();
        let mut candidate = vec![b'C' as u16, b':' as u16, b'\\' as u16]; candidate.extend_from_slice(&words);
        let parsed = validate_path_syntax(std::path::Path::new(&std::ffi::OsString::from_wide(&candidate)))?;
        if parsed[3..].contains(&(b'\\' as u16)) { return Err(io::Error::new(io::ErrorKind::InvalidInput, "file name must be one component")); }
        let expected: Vec<u16> = self.path.join(name).as_os_str().encode_wide().collect();
        let mut terminated = expected.clone(); terminated.push(0);
        unsafe {
            let handle = CreateFileW(terminated.as_ptr(), READ_CONTROL | FILE_READ_ATTRIBUTES | if delete { DELETE } else { 0 } | if read_data { windows_sys::Win32::Foundation::GENERIC_READ } else { 0 }, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, ptr::null(), OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT, ptr::null_mut());
            if handle == INVALID_HANDLE_VALUE { return Err(io::Error::last_os_error()); }
            let guard = Token(handle);
            (|| -> io::Result<CheckedPrivateFile<'_>> {
            check()?;
            validate_private_file_object(handle)?;
            let mut dir_id = FILE_ID_INFO::default(); let mut file_id = FILE_ID_INFO::default();
            if GetFileInformationByHandleEx(self._leaf.0, FileIdInfo, (&mut dir_id as *mut FILE_ID_INFO).cast(), mem::size_of_val(&dir_id) as u32) == 0 || GetFileInformationByHandleEx(handle, FileIdInfo, (&mut file_id as *mut FILE_ID_INFO).cast(), mem::size_of_val(&file_id) as u32) == 0 { return Err(io::Error::last_os_error()); }
            if dir_id.VolumeSerialNumber != file_id.VolumeSerialNumber { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "file is on a different volume")); }
            let mut actual = vec![0u16; 512];
            loop {
                let n = GetFinalPathNameByHandleW(handle, actual.as_mut_ptr(), actual.len() as u32, VOLUME_NAME_GUID) as usize;
                if n == 0 { return Err(io::Error::last_os_error()); }
                if n < actual.len() { if actual[..n] != expected { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "file name cannot be verified")); } break; }
                if n > 32768 { return Err(io::Error::new(io::ErrorKind::InvalidData, "normalized file path exceeds limit")); }
                actual.resize(n + 1, 0);
            }
            validate_private_file_acl(handle)?;
            Ok(CheckedPrivateFile { _handle: guard, _directory: std::marker::PhantomData })
            })().map_err(|e| io::Error::new(e.kind(), e))
        }
    }
}

#[cfg(test)]
mod private_file_open_tests {
    use super::*;
    use std::{fs, time::{SystemTime, UNIX_EPOCH}};
    #[test]
    fn opens_private_file_and_rejects_hard_link() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailFileOpen{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { if let Err(e) = fs::remove_dir_all(&self.0) { eprintln!("fixture cleanup {}: {e}", self.0.display()); if !std::thread::panicking() { panic!("cleanup failed"); } } } }
        let _cleanup = Cleanup(base.clone());
        let state = StateDirectory::acquire(&base.join("state")).unwrap();
        let file = base.join("state").join("file"); fs::write(&file, b"sentinel").unwrap();
        drop(state.open_existing_private_file("file").unwrap());
        fs::hard_link(&file, base.join("state").join("other")).unwrap();
        assert_eq!(state.open_existing_private_file("file").err().unwrap().to_string(), "file has multiple or invalid hard links");
        assert_eq!(fs::read(&file).unwrap(), b"sentinel");
    }
}

#[derive(Debug)]
pub(super) enum FileLockError { Contended, System(io::Error) }
struct FileByteLock<'a> { handle: HANDLE, _borrow: std::marker::PhantomData<&'a Token> }
impl<'a> FileByteLock<'a> {
    /// The handle must be synchronous (opened without FILE_FLAG_OVERLAPPED).
    fn acquire(file: &'a Token) -> Result<Self, FileLockError> {
        use windows_sys::Win32::{Foundation::ERROR_LOCK_VIOLATION, Storage::FileSystem::{LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY}, System::IO::OVERLAPPED};
        let mut overlap = OVERLAPPED::default();
        if unsafe { LockFileEx(file.0, LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY, 0, 1, 0, &mut overlap) } == 0 {
            let e = io::Error::last_os_error();
            return Err(if e.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) { FileLockError::Contended } else { FileLockError::System(e) });
        }
        Ok(Self { handle: file.0, _borrow: std::marker::PhantomData })
    }
}
impl Drop for FileByteLock<'_> {
    fn drop(&mut self) {
        use windows_sys::Win32::{Storage::FileSystem::UnlockFileEx, System::IO::OVERLAPPED};
        let mut overlap = OVERLAPPED::default();
        unsafe { UnlockFileEx(self.handle, 0, 1, 0, &mut overlap); }
    }
}

#[cfg(test)]
mod file_lock_tests {
    use super::*;
    use std::{fs::{self, OpenOptions}, os::windows::{fs::OpenOptionsExt, io::IntoRawHandle}, time::{SystemTime, UNIX_EPOCH}};
    #[test]
    fn contention_and_release_on_independent_handles() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailLockTest{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { if let Err(e) = fs::remove_file(&self.0) { if e.kind() != io::ErrorKind::NotFound { eprintln!("cleanup {}: {e}", self.0.display()); if !std::thread::panicking() { panic!("fixture cleanup failed"); } } } } }
        let open = || Token(OpenOptions::new().read(true).write(true).create(true).share_mode(3).open(&path).unwrap().into_raw_handle() as HANDLE);
        let _cleanup = Cleanup(path.clone()); let first = open(); let second = open();
        let lock = FileByteLock::acquire(&first).unwrap();
        assert!(matches!(FileByteLock::acquire(&second), Err(FileLockError::Contended)));
        drop(lock);
        drop(FileByteLock::acquire(&second).unwrap());
        let bad = Token(windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE);
        match FileByteLock::acquire(&bad) { Err(FileLockError::System(e)) => assert_eq!(e.raw_os_error(), Some(6)), _ => panic!("expected invalid handle error") }
        std::mem::forget(bad);
        drop(second); drop(first);
    }
}


impl StateDirectory {
    /// Opens a synchronous lock-file handle without delete sharing. This does
    /// not take the lock; the returned handle must outlive its lock guard.
    fn open_or_create_lock_file(&self) -> io::Result<CheckedPrivateFile<'_>> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::{Foundation::INVALID_HANDLE_VALUE, Storage::FileSystem::{CreateFileW, GetFileInformationByHandleEx, GetFinalPathNameByHandleW, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdInfo, OPEN_ALWAYS, READ_CONTROL, VOLUME_NAME_GUID}};
        let expected: Vec<u16> = self.path.join("mindtrail.lock").as_os_str().encode_wide().collect();
        let mut name = expected.clone(); name.push(0);
        let security = PrivateDirectorySecurity::new()?; let attrs = security.attributes();
        unsafe {
            let handle = CreateFileW(name.as_ptr(), FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_READ_ATTRIBUTES | READ_CONTROL, FILE_SHARE_READ | FILE_SHARE_WRITE, &attrs, OPEN_ALWAYS, FILE_FLAG_OPEN_REPARSE_POINT, ptr::null_mut());
            if handle == INVALID_HANDLE_VALUE { return Err(io::Error::last_os_error()); }
            let guard = Token(handle);
            validate_private_file_object(handle)?;
            let mut parent_id = FILE_ID_INFO::default(); let mut file_id = FILE_ID_INFO::default();
            if GetFileInformationByHandleEx(self._leaf.0, FileIdInfo, (&mut parent_id as *mut FILE_ID_INFO).cast(), mem::size_of_val(&parent_id) as u32) == 0 || GetFileInformationByHandleEx(handle, FileIdInfo, (&mut file_id as *mut FILE_ID_INFO).cast(), mem::size_of_val(&file_id) as u32) == 0 { return Err(io::Error::last_os_error()); }
            if parent_id.VolumeSerialNumber != file_id.VolumeSerialNumber { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "lock file is on a different volume")); }
            let mut actual = vec![0u16; 512];
            loop {
                let n = GetFinalPathNameByHandleW(handle, actual.as_mut_ptr(), actual.len() as u32, VOLUME_NAME_GUID) as usize;
                if n == 0 { return Err(io::Error::last_os_error()); }
                if n < actual.len() { if actual[..n] != expected { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "lock file name cannot be verified")); } break; }
                if n > 32768 { return Err(io::Error::new(io::ErrorKind::InvalidData, "normalized lock path exceeds limit")); }
                actual.resize(n + 1, 0);
            }
            validate_private_file_acl(handle)?;
            Ok(CheckedPrivateFile { _handle: guard, _directory: std::marker::PhantomData })
        }
    }
}

#[cfg(test)]
mod lock_open_tests {
    use super::*;
    use std::{fs, time::{SystemTime, UNIX_EPOCH}};
    #[test]
    fn creates_and_reopens_lock_file() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailLockOpen{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { if let Err(e) = fs::remove_dir_all(&self.0) { eprintln!("cleanup {}: {e}", self.0.display()); if !std::thread::panicking() { panic!("cleanup failed"); } } } }
        let _cleanup = Cleanup(base.clone());
        let state = StateDirectory::acquire(&base.join("state")).unwrap();
        let first = state.open_or_create_lock_file().unwrap();
        let second = state.open_or_create_lock_file().unwrap();
        use windows_sys::Win32::Storage::FileSystem::{FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx};
        let info = |h: HANDLE| { let mut id = FILE_ID_INFO::default(); assert_ne!(unsafe { GetFileInformationByHandleEx(h, FileIdInfo, (&mut id as *mut FILE_ID_INFO).cast(), mem::size_of_val(&id) as u32) }, 0); id };
        let a = info(first._handle.0); let b = info(second._handle.0);
        assert_eq!(a.VolumeSerialNumber, b.VolumeSerialNumber); assert_eq!(a.FileId.Identifier, b.FileId.Identifier);
    }
}

/// Owns the synchronous checked handle and releases its byte lock on drop.
pub(super) struct StateDirectoryLock<'a> { file: CheckedPrivateFile<'a>, directory: &'a StateDirectory }
impl StateDirectory {
    pub(super) fn lock(&self) -> Result<StateDirectoryLock<'_>, FileLockError> {
        let file = self.open_or_create_lock_file().map_err(FileLockError::System)?;
        let guard = FileByteLock::acquire(&file._handle)?;
        // The owned wrapper assumes the guard's unlock responsibility before
        // moving the handle, avoiding a self-referential structure.
        std::mem::forget(guard);
        Ok(StateDirectoryLock { file, directory: self })
    }
}
impl StateDirectoryLock<'_> {
    fn directory(&self) -> &StateDirectory { self.directory }
}
impl Drop for StateDirectoryLock<'_> {
    fn drop(&mut self) {
        use windows_sys::Win32::{Storage::FileSystem::UnlockFileEx, System::IO::OVERLAPPED};
        let mut overlap = OVERLAPPED::default();
        unsafe { UnlockFileEx(self.file._handle.0, 0, 1, 0, &mut overlap); }
    }
}

#[cfg(test)]
mod owned_state_lock_tests {
    use super::*;
    use std::{fs, time::{SystemTime, UNIX_EPOCH}};
    #[test]
    fn state_lock_contends_and_reacquires() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailOwnedLock{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { if let Err(e) = fs::remove_dir_all(&self.0) { eprintln!("cleanup {}: {e}", self.0.display()); if !std::thread::panicking() { panic!("cleanup failed"); } } } }
        let _cleanup = Cleanup(base.clone());
        let state = StateDirectory::acquire(&base.join("state")).unwrap();
        let lock = state.lock().unwrap();
        assert!(std::ptr::eq(lock.directory(), &state));
        assert!(matches!(state.lock(), Err(FileLockError::Contended)));
        drop(lock); drop(state.lock().unwrap());
        drop(state);
        assert!(base.join("state").join("mindtrail.lock").is_file());
    }
}

#[cfg(test)]
mod lock_open_negative_tests {
    use super::*;
    use std::{fs::{self, OpenOptions}, os::windows::{fs::OpenOptionsExt, io::AsRawHandle}, time::{SystemTime, UNIX_EPOCH}};
    use windows_sys::Win32::{Foundation::LocalFree, Security::{ACL, ACL_REVISION, AddAccessAllowedAce, CreateWellKnownSid, InitializeAcl, WinWorldSid, SECURITY_MAX_SID_SIZE, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, Authorization::{GetSecurityInfo, SetSecurityInfo, SE_FILE_OBJECT}}, Storage::FileSystem::{FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx}};
    fn snapshot(h: HANDLE) -> (u64, [u8;16], Vec<u8>) { unsafe {
        let mut id = FILE_ID_INFO::default(); assert_ne!(GetFileInformationByHandleEx(h, FileIdInfo, (&mut id as *mut FILE_ID_INFO).cast(), mem::size_of_val(&id) as u32), 0);
        let mut acl: *mut ACL = ptr::null_mut(); let mut sd = ptr::null_mut(); assert_eq!(GetSecurityInfo(h, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(), &mut acl, ptr::null_mut(), &mut sd), 0);
        let bytes = std::slice::from_raw_parts(acl.cast::<u8>(), (*acl).AclSize as usize).to_vec(); LocalFree(sd); (id.VolumeSerialNumber, id.FileId.Identifier, bytes)
    }}
    #[test]
    fn preserves_existing_and_rejects_hardlink_acl_and_reparse() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailLockNegative{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { if let Err(e) = fs::remove_dir_all(&self.0) { eprintln!("cleanup {}: {e}", self.0.display()); if !std::thread::panicking() { panic!("cleanup failed"); } } } }
        let _cleanup = Cleanup(base.clone()); let state = StateDirectory::acquire(&base.join("state")).unwrap();
        let path = base.join("state").join("mindtrail.lock");
        drop(state.open_or_create_lock_file().unwrap()); fs::write(&path, b"sentinel").unwrap();
        let inspect = || OpenOptions::new().access_mode(0x00060000).share_mode(3).open(&path).unwrap();
        let file = inspect(); let original = snapshot(file.as_raw_handle() as HANDLE); drop(file);
        drop(state.open_or_create_lock_file().unwrap());
        let file = inspect(); assert_eq!(snapshot(file.as_raw_handle() as HANDLE), original); drop(file); assert_eq!(fs::read(&path).unwrap(), b"sentinel");
        let other = base.join("state").join("hardlink"); fs::hard_link(&path, &other).unwrap();
        let file = inspect(); let before = snapshot(file.as_raw_handle() as HANDLE);
        assert_eq!(state.open_or_create_lock_file().err().unwrap().to_string(), "file has multiple or invalid hard links");
        assert_eq!(snapshot(file.as_raw_handle() as HANDLE), before); drop(file); assert_eq!(fs::read(&path).unwrap(), b"sentinel"); fs::remove_file(other).unwrap();
        let file = inspect();
        unsafe {
            let user = current_user_sid().unwrap(); let mut world = [0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)]; let mut size = SECURITY_MAX_SID_SIZE;
            assert_ne!(CreateWellKnownSid(WinWorldSid, ptr::null_mut(), world.as_mut_ptr().cast(), &mut size), 0);
            let mut storage = [0u32; 128]; let acl = storage.as_mut_ptr().cast::<ACL>(); assert_ne!(InitializeAcl(acl, mem::size_of_val(&storage) as u32, ACL_REVISION), 0);
            assert_ne!(AddAccessAllowedAce(acl, ACL_REVISION, 0x1f01ff, user.as_ptr() as *mut _), 0); assert_ne!(AddAccessAllowedAce(acl, ACL_REVISION, 1, world.as_mut_ptr().cast()), 0);
            assert_eq!(SetSecurityInfo(file.as_raw_handle() as HANDLE, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(), acl, ptr::null()), 0);
        }
        let before = snapshot(file.as_raw_handle() as HANDLE);
        assert_eq!(state.open_or_create_lock_file().err().unwrap().to_string(), "file DACL grants an unexpected principal");
        assert_eq!(snapshot(file.as_raw_handle() as HANDLE), before); drop(file); assert_eq!(fs::read(&path).unwrap(), b"sentinel");
        fs::remove_file(&path).unwrap(); let target = base.join("target"); fs::create_dir(&target).unwrap(); fs::write(target.join("sentinel"), b"untouched").unwrap();
        let output = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&path).arg(&target).output().unwrap(); assert!(output.status.success(), "junction fixture unavailable");
        let link = OpenOptions::new().access_mode(0x00020080).custom_flags(0x02200000).open(&path).unwrap();
        let link_before = snapshot(link.as_raw_handle() as HANDLE);
        let error = state.open_or_create_lock_file().err().unwrap(); assert_eq!(error.raw_os_error(), Some(5));
        assert_eq!(snapshot(link.as_raw_handle() as HANDLE), link_before); drop(link);
        assert_eq!(fs::read(target.join("sentinel")).unwrap(), b"untouched"); fs::remove_dir(&path).unwrap();
    }
}


#[cfg(test)]
mod state_lock_process_tests {
    use super::*;
    use std::{fs, io::Write, process::{Child, Command, Stdio}, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};
    #[test]
    fn contention_and_recovery_after_process_exit() {
        if let Some(path) = std::env::var_os("MINDTRAIL_LOCK_CHILD") {
            let base = std::path::PathBuf::from(path);
            let state = StateDirectory::acquire(&base.join("state")).unwrap();
            match state.lock() {
                Ok(_guard) => {
                    fs::write(base.join("held.tmp"), b"held").unwrap(); fs::rename(base.join("held.tmp"), base.join("held")).unwrap();
                    let mut line = String::new(); std::io::stdin().read_line(&mut line).unwrap();
                }
                Err(FileLockError::Contended) => { fs::write(base.join("contended"), b"yes").unwrap(); }
                Err(e) => panic!("unexpected lock failure: {e:?}"),
            }
            return;
        }
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailLockProcess{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { if let Err(e) = fs::remove_dir_all(&self.0) { eprintln!("cleanup {}: {e}", self.0.display()); if !std::thread::panicking() { panic!("cleanup failed"); } } } }
        let _cleanup = Cleanup(base.clone());
        struct Guard(Child); impl Drop for Guard { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }
        let spawn = || Guard(Command::new(std::env::current_exe().unwrap()).args(["--exact", "gateway::windows_security::state_lock_process_tests::contention_and_recovery_after_process_exit", "--nocapture"]).env("MINDTRAIL_LOCK_CHILD", &base).stdin(Stdio::piped()).stdout(Stdio::null()).spawn().unwrap());
        let wait_path = |p: &std::path::Path| { let until = Instant::now() + Duration::from_secs(20); while !p.exists() { assert!(Instant::now() < until, "barrier timeout: {}", p.display()); std::thread::sleep(Duration::from_millis(10)); } };
        let wait_child = |c: &mut Child| { let until = Instant::now() + Duration::from_secs(20); loop { if let Some(s) = c.try_wait().unwrap() { assert!(s.success()); break; } assert!(Instant::now() < until, "child exit timeout"); std::thread::sleep(Duration::from_millis(10)); } };
        let mut first = spawn(); wait_path(&base.join("held"));
        let lock_path = base.join("state").join("mindtrail.lock");
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx};
        let identity = || { let f = fs::OpenOptions::new().read(true).share_mode(3).open(&lock_path).unwrap(); let mut id = FILE_ID_INFO::default(); assert_ne!(unsafe { GetFileInformationByHandleEx(f.as_raw_handle() as HANDLE, FileIdInfo, (&mut id as *mut FILE_ID_INFO).cast(), mem::size_of_val(&id) as u32) }, 0); (id.VolumeSerialNumber, id.FileId.Identifier) };
        let initial = identity();
        let deletion = fs::OpenOptions::new().access_mode(0x10000).share_mode(7).open(&lock_path);
        let delete_error = deletion.expect_err("DELETE access unexpectedly succeeded while lock is held");
        assert_eq!(delete_error.raw_os_error(), Some(windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION as i32), "unexpected DELETE-access failure: {delete_error}");
        let mut second = spawn(); wait_path(&base.join("contended")); wait_child(&mut second.0);
        first.0.stdin.take().unwrap().write_all(b"exit\n").unwrap(); wait_child(&mut first.0);
        fs::remove_file(base.join("held")).unwrap();
        let mut third = spawn(); wait_path(&base.join("held")); assert_eq!(identity(), initial);
        third.0.kill().unwrap(); let _ = third.0.wait().unwrap(); fs::remove_file(base.join("held")).unwrap();
        let mut fourth = spawn(); wait_path(&base.join("held")); assert_eq!(identity(), initial);
        fourth.0.stdin.take().unwrap().write_all(b"exit\n").unwrap(); wait_child(&mut fourth.0);
    }
}



/// Preliminary temporary file; cleanup and publication are not implemented.
struct TempOwnedFile { file: Token, cleanup: bool }
impl TempOwnedFile {
    fn remove(&mut self) -> io::Result<()> {
        use windows_sys::Win32::Storage::FileSystem::{SetFileInformationByHandle, FILE_DISPOSITION_INFO, FileDispositionInfo};
        let info = FILE_DISPOSITION_INFO { DeleteFile: true };
        if unsafe { SetFileInformationByHandle(self.file.0, FileDispositionInfo, (&info as *const FILE_DISPOSITION_INFO).cast(), mem::size_of_val(&info) as u32) } == 0 { return Err(io::Error::last_os_error()); }
        self.cleanup = false;
        Ok(())
    }
}
impl Drop for TempOwnedFile { fn drop(&mut self) { if self.cleanup { if let Err(e) = self.remove() { eprintln!("temporary file handle cleanup failed: {e}"); } } } }
struct EndpointTemp<'a> { _file: TempOwnedFile, path: std::path::PathBuf, lock: &'a StateDirectoryLock<'a> }
impl<'a> StateDirectoryLock<'a> {
    fn create_endpoint_temp(&'a self) -> io::Result<EndpointTemp<'a>> {
        let mut random = [0u8; 8]; getrandom::fill(&mut random).map_err(|e| io::Error::other(e.to_string()))?;
        let name = format!("endpoint-{}.tmp", random.iter().map(|b| format!("{b:02x}")).collect::<String>());
        self.create_endpoint_temp_named(&name)
    }
    fn create_endpoint_temp_named(&'a self, name: &str) -> io::Result<EndpointTemp<'a>> { self.create_endpoint_temp_checked(name, || Ok(())) }
    fn create_endpoint_temp_checked(&'a self, name: &str, check: impl FnOnce() -> io::Result<()>) -> io::Result<EndpointTemp<'a>> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::{Foundation::INVALID_HANDLE_VALUE, Storage::FileSystem::{CreateFileW, GetFileInformationByHandleEx, GetFinalPathNameByHandleW, CREATE_NEW, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_ID_INFO, FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdInfo, READ_CONTROL, DELETE, VOLUME_NAME_GUID}};
        if name.len() != 29 || !name.starts_with("endpoint-") || !name.ends_with(".tmp") || !name.as_bytes()[9..25].iter().all(|&b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) { return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid temporary file name")); }
        let directory = self.directory(); let path = directory.path.join(name);
        let expected: Vec<u16> = path.as_os_str().encode_wide().collect(); let mut wide = expected.clone(); wide.push(0);
        let security = PrivateDirectorySecurity::new()?; let attrs = security.attributes();
        unsafe {
            let h = CreateFileW(wide.as_ptr(), FILE_GENERIC_READ | FILE_GENERIC_WRITE | READ_CONTROL | DELETE, FILE_SHARE_READ | FILE_SHARE_WRITE, &attrs, CREATE_NEW, FILE_FLAG_OPEN_REPARSE_POINT, ptr::null_mut());
            if h == INVALID_HANDLE_VALUE { return Err(io::Error::last_os_error()); }
            let file = TempOwnedFile { file: Token(h), cleanup: true };
            check()?;
            validate_private_file_object(h)?; validate_private_file_acl(h)?;
            let mut parent = FILE_ID_INFO::default(); let mut item = FILE_ID_INFO::default();
            if GetFileInformationByHandleEx(directory._leaf.0, FileIdInfo, (&mut parent as *mut FILE_ID_INFO).cast(), mem::size_of_val(&parent) as u32) == 0 || GetFileInformationByHandleEx(h, FileIdInfo, (&mut item as *mut FILE_ID_INFO).cast(), mem::size_of_val(&item) as u32) == 0 { return Err(io::Error::last_os_error()); }
            if parent.VolumeSerialNumber != item.VolumeSerialNumber { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "temporary file volume mismatch")); }
            let mut actual = vec![0u16; 512]; loop {
                let n = GetFinalPathNameByHandleW(h, actual.as_mut_ptr(), actual.len() as u32, VOLUME_NAME_GUID) as usize;
                if n == 0 { return Err(io::Error::last_os_error()); }
                if n < actual.len() { if actual[..n] != expected { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "temporary file name mismatch")); } break; }
                if n > 32768 { return Err(io::Error::new(io::ErrorKind::InvalidData, "temporary path too long")); } actual.resize(n + 1, 0);
            }
            Ok(EndpointTemp { _file: file, path, lock: self })
        }
    }
}

#[cfg(test)]
mod endpoint_temp_tests {
    use super::*;
    use std::{fs, time::{SystemTime, UNIX_EPOCH}};
    #[test]
    fn creates_temp_and_does_not_overwrite_collision() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailTemp{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { if let Err(e) = fs::remove_dir_all(&self.0) { eprintln!("cleanup {}: {e}", self.0.display()); if !std::thread::panicking() { panic!("cleanup failed"); } } } }
        let _cleanup = Cleanup(base.clone());
        let state = StateDirectory::acquire(&base.join("state")).unwrap(); let lock = state.lock().unwrap();
        let temp = lock.create_endpoint_temp().unwrap(); assert!(temp.path.is_file()); validate_private_file_acl(temp._file.file.0).unwrap(); let temp_path = temp.path.clone(); drop(temp); assert!(!temp_path.exists());
        let failed = base.join("state").join("endpoint-fedcba9876543210.tmp");
        assert!(lock.create_endpoint_temp_checked("endpoint-fedcba9876543210.tmp", || Err(io::Error::other("injected verification failure"))).is_err());
        assert!(!failed.exists());
        let collision = base.join("state").join("endpoint-0123456789abcdef.tmp"); fs::write(&collision, b"sentinel").unwrap();
        assert_eq!(lock.create_endpoint_temp_named("endpoint-0123456789abcdef.tmp").err().unwrap().raw_os_error(), Some(80));
        assert_eq!(fs::read(collision).unwrap(), b"sentinel");
    }
}




struct PreparedEndpointTemp<'a> { _temp: EndpointTemp<'a> }
impl<'a> EndpointTemp<'a> {
    fn write_endpoint(self, endpoint: &super::discovery::Endpoint) -> io::Result<PreparedEndpointTemp<'a>> {
        use windows_sys::Win32::Storage::FileSystem::{FlushFileBuffers, WriteFile};
        let handle = self._file.file.0;
        self.write_endpoint_with(endpoint, |bytes| {
            let mut written = 0u32; let len = bytes.len().min(u32::MAX as usize) as u32;
            if unsafe { WriteFile(handle, bytes.as_ptr(), len, &mut written, ptr::null_mut()) } == 0 { return Err(io::Error::last_os_error()); }
            Ok(written as usize)
        }, || if unsafe { FlushFileBuffers(handle) } == 0 { Err(io::Error::last_os_error()) } else { Ok(()) })
    }
    fn write_endpoint_with(self, endpoint: &super::discovery::Endpoint, mut write: impl FnMut(&[u8]) -> io::Result<usize>, flush: impl FnOnce() -> io::Result<()>) -> io::Result<PreparedEndpointTemp<'a>> {
        let bytes = serde_json::to_vec(endpoint).map_err(|_| io::Error::other("endpoint serialization failed"))?;
        let mut remaining = bytes.as_slice();
        while !remaining.is_empty() {
            let n = write(remaining)?;
            if n == 0 || n > remaining.len() { return Err(io::Error::new(io::ErrorKind::WriteZero, "invalid endpoint write length")); }
            remaining = &remaining[n..];
        }
        flush()?;
        Ok(PreparedEndpointTemp { _temp: self })
    }
}

#[cfg(test)]
mod endpoint_write_tests {
    use super::*;
    use std::{fs, time::{SystemTime, UNIX_EPOCH}};
    #[test]
    fn writes_syncs_and_cleans_up_failed_writes() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailWrite{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { if let Err(e) = fs::remove_dir_all(&self.0) { eprintln!("fixture cleanup {}: {e}", self.0.display()); if !std::thread::panicking() { panic!("cleanup failed"); } } } }
        let _cleanup = Cleanup(base.clone()); let state = StateDirectory::acquire(&base.join("state")).unwrap(); let lock = state.lock().unwrap();
        let endpoint = super::super::discovery::Endpoint::new(1234, "a".repeat(64), "b".repeat(64)).unwrap();
        let temp = lock.create_endpoint_temp().unwrap(); let path = temp.path.clone();
        let prepared = temp.write_endpoint(&endpoint).unwrap(); assert_eq!(fs::read(&path).unwrap(), serde_json::to_vec(&endpoint).unwrap()); drop(prepared); assert!(!path.exists());
        let temp = lock.create_endpoint_temp().unwrap(); let path = temp.path.clone(); let mut collected = Vec::new();
        let prepared = temp.write_endpoint_with(&endpoint, |bytes| { let n = bytes.len().min(3); collected.extend_from_slice(&bytes[..n]); Ok(n) }, || Ok(())).unwrap();
        assert_eq!(collected, serde_json::to_vec(&endpoint).unwrap()); drop(prepared); assert!(!path.exists());
        let temp = lock.create_endpoint_temp().unwrap(); let path = temp.path.clone(); let mut calls = 0;
        let error = temp.write_endpoint_with(&endpoint, |bytes| { calls += 1; if calls == 1 { Ok(bytes.len().min(3)) } else { Err(io::Error::other("injected write failure")) } }, || Ok(())).err().unwrap();
        assert_eq!(error.to_string(), "injected write failure"); assert!(!error.to_string().contains(&"a".repeat(64))); assert!(!path.exists());
        let temp = lock.create_endpoint_temp().unwrap(); let path = temp.path.clone();
        assert!(temp.write_endpoint_with(&endpoint, |bytes| Ok(bytes.len()), || Err(io::Error::other("injected flush failure"))).is_err()); assert!(!path.exists());
        for excessive in [false, true] {
            let temp = lock.create_endpoint_temp().unwrap(); let path = temp.path.clone();
            let error = temp.write_endpoint_with(&endpoint, |bytes| Ok(if excessive { bytes.len() + 1 } else { 0 }), || Ok(())).err().unwrap();
            assert_eq!(error.kind(), io::ErrorKind::WriteZero);
            assert_eq!(error.to_string(), "invalid endpoint write length");
            assert!(!path.exists());
        }
    }
}



#[cfg(test)]
mod endpoint_rename_probe {
    use super::*;
    use std::{fs, os::windows::fs::OpenOptionsExt, time::{SystemTime, UNIX_EPOCH}};
    use windows_sys::Win32::Storage::FileSystem::{FILE_RENAME_INFO, FileRenameInfoEx, SetFileInformationByHandle};
    fn rename(prepared: &PreparedEndpointTemp<'_>, parent: HANDLE, replace: bool) -> io::Result<()> {
        let name: Vec<u16> = "endpoint.json".encode_utf16().collect();
        let size = mem::offset_of!(FILE_RENAME_INFO, FileName) + (name.len() + 1) * 2;
        let mut buffer = vec![0usize; size.div_ceil(mem::size_of::<usize>())];
        unsafe {
            let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
            (*info).Anonymous.Flags = if replace { 1 } else { 0 };
            (*info).RootDirectory = parent; (*info).FileNameLength = (name.len() * 2) as u32;
            ptr::copy_nonoverlapping(name.as_ptr(), ptr::addr_of_mut!((*info).FileName).cast::<u16>(), name.len());
            if SetFileInformationByHandle(prepared._temp._file.file.0, FileRenameInfoEx, info.cast(), size as u32) == 0 { return Err(io::Error::last_os_error()); }
        }
        Ok(())
    }
    #[test]
    fn endpoint_rename_probe_minimal() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailRenameProbe{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { if let Err(e) = fs::remove_dir_all(&self.0) { eprintln!("cleanup {}: {e}", self.0.display()); if !std::thread::panicking() { panic!("cleanup failed"); } } } }
        let _cleanup = Cleanup(base.clone());
        let state = StateDirectory::acquire(&base.join("state")).unwrap(); let lock = state.lock().unwrap();
        let endpoint = super::super::discovery::Endpoint::new(1234, "a".repeat(64), "b".repeat(64)).unwrap();
        let new = || lock.create_endpoint_temp().unwrap().write_endpoint(&endpoint).unwrap();
        let mut first = new(); let first_path = first._temp.path.clone();
        match rename(&first, state._leaf.0, false) {
            Ok(()) => { first._temp._file.cleanup = false; drop(first); assert!(!first_path.exists()); }
            Err(e) => { eprintln!("missing-target rename: {:?}", e.raw_os_error()); drop(first); assert!(!first_path.exists()); return; }
        }
        let dest = base.join("state").join("endpoint.json"); assert_eq!(fs::read(&dest).unwrap(), serde_json::to_vec(&endpoint).unwrap());
        let mut second = new(); let result = rename(&second, state._leaf.0, true);
        eprintln!("existing-target replace: {:?}", result.as_ref().err().and_then(io::Error::raw_os_error));
        if result.is_ok() { second._temp._file.cleanup = false; } drop(second);
        assert_eq!(fs::read(&dest).unwrap(), serde_json::to_vec(&endpoint).unwrap());
        let block = fs::OpenOptions::new().read(true).share_mode(3).open(&dest).unwrap();
        let third = new(); let result = rename(&third, state._leaf.0, true);
        eprintln!("existing-target blocked-reader replace: {:?}", result.as_ref().err().and_then(io::Error::raw_os_error));
        assert!(result.is_err()); drop(third); drop(block);
        assert_eq!(fs::read(&dest).unwrap(), serde_json::to_vec(&endpoint).unwrap());
    }
}


#[cfg(test)]
mod rename_layout_probe {
    use super::*;
    use std::{fs, os::windows::ffi::OsStrExt, time::{SystemTime, UNIX_EPOCH}};
    use windows_sys::Win32::Storage::FileSystem::{FILE_RENAME_INFO, FileRenameInfo, FileRenameInfoEx, SetFileInformationByHandle};
    #[test]
    fn compare_absolute_and_relative_rename_forms() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailRenameForms{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { if let Err(e) = fs::remove_dir_all(&self.0) { eprintln!("cleanup {}: {e}", self.0.display()); if !std::thread::panicking() { panic!("cleanup failed"); } } } }
        let _cleanup = Cleanup(base.clone()); let state = StateDirectory::acquire(&base.join("state")).unwrap(); let lock = state.lock().unwrap();
        let endpoint = super::super::discovery::Endpoint::new(1234, "a".repeat(64), "b".repeat(64)).unwrap();
        for (label, class, relative, replace) in [("ex absolute", FileRenameInfoEx, false, false), ("classic absolute", FileRenameInfo, false, false), ("classic absolute replace", FileRenameInfo, false, true), ("classic relative", FileRenameInfo, true, false), ("ex relative", FileRenameInfoEx, true, false)] {
            let mut temp = lock.create_endpoint_temp().unwrap().write_endpoint(&endpoint).unwrap();
            let dest = state.path.join("endpoint.json");
            if replace { fs::write(&dest, b"old sentinel").unwrap(); }
            let name: Vec<u16> = if relative { std::ffi::OsStr::new("endpoint.json").encode_wide().collect() } else { dest.as_os_str().encode_wide().collect() };
            let size = mem::offset_of!(FILE_RENAME_INFO, FileName) + (name.len() + 1) * 2;
            let mut storage = vec![0usize; size.div_ceil(mem::size_of::<usize>())];
            let result = unsafe {
                let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
                (*info).Anonymous.Flags = if replace { 1 } else { 0 };
                (*info).RootDirectory = if relative { state._leaf.0 } else { ptr::null_mut() };
                (*info).FileNameLength = (name.len() * 2) as u32;
                ptr::copy_nonoverlapping(name.as_ptr(), ptr::addr_of_mut!((*info).FileName).cast::<u16>(), name.len());
                SetFileInformationByHandle(temp._temp._file.file.0, class, info.cast(), size as u32)
            };
            let error = if result == 0 { io::Error::last_os_error().raw_os_error() } else { None };
            eprintln!("{label}: {error:?}");
            if result != 0 { temp._temp._file.cleanup = false; drop(temp); assert_eq!(fs::read(&dest).unwrap(), serde_json::to_vec(&endpoint).unwrap()); fs::remove_file(&dest).unwrap(); }
            else { let source = temp._temp.path.clone(); drop(temp); assert!(!source.exists()); if replace { assert_eq!(fs::read(&dest).unwrap(), b"old sentinel"); fs::remove_file(&dest).unwrap(); } else { assert!(!dest.exists()); } }
        }
    }
}


#[cfg(test)]
mod rename_replace_probe {
    use super::*;
    use std::{collections::BTreeMap, fs, os::windows::{fs::OpenOptionsExt, io::AsRawHandle}, sync::{Arc, Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}}, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};
    use windows_sys::Win32::Storage::FileSystem::{FILE_ID_INFO, FILE_RENAME_INFO, FileIdInfo, FileRenameInfoEx, GetFileInformationByHandleEx, SetFileInformationByHandle};
    fn identity(h: HANDLE) -> (u64, [u8;16]) { let mut id = FILE_ID_INFO::default(); assert_ne!(unsafe { GetFileInformationByHandleEx(h, FileIdInfo, (&mut id as *mut FILE_ID_INFO).cast(), mem::size_of_val(&id) as u32) }, 0); (id.VolumeSerialNumber, id.FileId.Identifier) }
    fn rename(p: &PreparedEndpointTemp<'_>, dest: &std::path::Path) -> io::Result<()> {
        use std::os::windows::ffi::OsStrExt;
        let name: Vec<u16> = dest.as_os_str().encode_wide().collect(); let size = mem::offset_of!(FILE_RENAME_INFO, FileName) + (name.len() + 1) * 2;
        let mut storage = vec![0usize; size.div_ceil(mem::size_of::<usize>())];
        unsafe { let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>(); (*info).Anonymous.Flags = 1; (*info).RootDirectory = ptr::null_mut(); (*info).FileNameLength = (name.len() * 2) as u32; ptr::copy_nonoverlapping(name.as_ptr(), ptr::addr_of_mut!((*info).FileName).cast::<u16>(), name.len()); if SetFileInformationByHandle(p._temp._file.file.0, FileRenameInfoEx, info.cast(), size as u32) == 0 { return Err(io::Error::last_os_error()); } } Ok(())
    }
    #[test]
    fn probe_absolute_replace_conflict_and_reader() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailRenameReplace{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos())); fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { if let Err(e) = fs::remove_dir_all(&self.0) { eprintln!("cleanup {}: {e}", self.0.display()); if !std::thread::panicking() { panic!("cleanup failed"); } } } }
        let _cleanup = Cleanup(base.clone()); let state = StateDirectory::acquire(&base.join("state")).unwrap(); let lock = state.lock().unwrap();
        let old = super::super::discovery::Endpoint::new(1111, "a".repeat(64), "b".repeat(64)).unwrap(); let new = super::super::discovery::Endpoint::new(2222, "c".repeat(64), "b".repeat(64)).unwrap();
        let old_bytes = serde_json::to_vec(&old).unwrap(); let new_bytes = serde_json::to_vec(&new).unwrap();
        let target = state.path.join("endpoint.json"); fs::write(&target, &old_bytes).unwrap();
        let blocked = fs::OpenOptions::new().read(true).share_mode(3).open(&target).unwrap();
        let pending = lock.create_endpoint_temp().unwrap().write_endpoint(&new).unwrap(); let pending_path = pending._temp.path.clone();
        let error = rename(&pending, &target).unwrap_err(); eprintln!("blocked replacement error: {:?}", error.raw_os_error()); assert_eq!(error.raw_os_error(), Some(5)); drop(pending); assert!(!pending_path.exists()); drop(blocked); assert_eq!(fs::read(&target).unwrap(), old_bytes);
        let stop = Arc::new(AtomicBool::new(false)); let pause = Arc::new(AtomicBool::new(false)); let paused = Arc::new(AtomicBool::new(false));
        let old_count = Arc::new(AtomicUsize::new(0)); let new_count = Arc::new(AtomicUsize::new(0)); let bad = Arc::new(AtomicUsize::new(0));
        let read_errors = Arc::new(Mutex::new(BTreeMap::<(&'static str, Option<i32>, String), usize>::new()));
        let reader = { let stop=stop.clone(); let pause=pause.clone(); let paused=paused.clone(); let old_count=old_count.clone(); let new_count=new_count.clone(); let bad=bad.clone(); let read_errors=read_errors.clone(); let target=target.clone(); let a=old_bytes.clone(); let b=new_bytes.clone(); std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                if pause.load(Ordering::SeqCst) {
                    // This acknowledgement is sent only between reads, with no
                    // target handle retained by this reader thread.
                    paused.store(true, Ordering::SeqCst);
                    while pause.load(Ordering::SeqCst) && !stop.load(Ordering::SeqCst) { std::thread::yield_now(); }
                    paused.store(false, Ordering::SeqCst);
                    continue;
                }
                let observation = (|| {
                    use std::io::Read;
                    let mut file = fs::OpenOptions::new().read(true).share_mode(7).open(&target).map_err(|e| ("open", e))?;
                    let mut bytes = Vec::new();
                    file.read_to_end(&mut bytes).map_err(|e| ("read", e))?;
                    Ok::<_, (&'static str, io::Error)>(bytes)
                })();
                match observation {
                    Ok(bytes) if bytes == a => { old_count.fetch_add(1, Ordering::SeqCst); }
                    Ok(bytes) if bytes == b => { new_count.fetch_add(1, Ordering::SeqCst); }
                    Ok(_) => { bad.fetch_add(1, Ordering::SeqCst); }
                    Err((stage, error)) => { *read_errors.lock().unwrap_or_else(|e| e.into_inner()).entry((stage, error.raw_os_error(), format!("{:?}", error.kind()))).or_default() += 1; }
                }
            }
        }) };
        struct ReaderGuard { stop: Arc<AtomicBool>, pause: Arc<AtomicBool>, thread: Option<std::thread::JoinHandle<()>> }
        impl Drop for ReaderGuard { fn drop(&mut self) { self.stop.store(true, Ordering::SeqCst); self.pause.store(false, Ordering::SeqCst); if let Some(thread) = self.thread.take() { let result = thread.join(); if !std::thread::panicking() { result.unwrap(); } } } }
        let mut reader = ReaderGuard { stop: stop.clone(), pause: pause.clone(), thread: Some(reader) };
        let wait = |condition: &dyn Fn() -> bool, message: &str| { let limit=Instant::now()+Duration::from_secs(10); while !condition() { assert!(Instant::now()<limit, "{message}"); std::thread::yield_now(); } };
        wait(&|| old_count.load(Ordering::SeqCst)>0, "old reader timeout");
        let old_id = { let file = fs::OpenOptions::new().read(true).share_mode(7).open(&target).unwrap(); identity(file.as_raw_handle() as HANDLE) };
        let mut prepared = lock.create_endpoint_temp().unwrap().write_endpoint(&new).unwrap(); let mut source_id = identity(prepared._temp._file.file.0);
        let source_path = prepared._temp.path.clone();
        match rename(&prepared, &target) {
            Ok(()) => { eprintln!("concurrent rename: succeeded"); prepared._temp._file.cleanup = false; drop(prepared); }
            Err(error) => {
                eprintln!("concurrent rename syscall failed: {:?}", error.raw_os_error());
                drop(prepared);
                assert!(!source_path.exists(), "failed source was not cleaned");
                let file = fs::OpenOptions::new().read(true).share_mode(7).open(&target).unwrap();
                assert_eq!(identity(file.as_raw_handle() as HANDLE), old_id);
                assert_eq!(fs::read(&target).unwrap(), old_bytes);
                drop(file);
                assert_eq!(error.raw_os_error(), Some(5), "unexpected concurrent rename failure: {error}");
                // Error 5 alone is not evidence of a transient sharing conflict.
                // Only this probe's own reader can be made quiescent; after the
                // acknowledgement, make one fresh attempt and require success.
                pause.store(true, Ordering::SeqCst);
                wait(&|| paused.load(Ordering::SeqCst), "reader pause timeout");
                let mut retry = lock.create_endpoint_temp().unwrap().write_endpoint(&new).unwrap();
                source_id = identity(retry._temp._file.file.0);
                let retry_path = retry._temp.path.clone();
                match rename(&retry, &target) {
                    Ok(()) => { retry._temp._file.cleanup = false; drop(retry); }
                    Err(error) => {
                        eprintln!("rename syscall with probe reader quiescent failed: {:?}", error.raw_os_error());
                        drop(retry);
                        assert!(!retry_path.exists(), "failed quiescent source was not cleaned");
                        let file = fs::OpenOptions::new().read(true).share_mode(7).open(&target).unwrap();
                        assert_eq!(identity(file.as_raw_handle() as HANDLE), old_id);
                        assert_eq!(fs::read(&target).unwrap(), old_bytes);
                        panic!("rename with probe reader quiescent failed: {error}");
                    }
                }
                pause.store(false, Ordering::SeqCst);
            }
        }
        wait(&|| new_count.load(Ordering::SeqCst)>0, "new reader timeout");
        stop.store(true,Ordering::SeqCst); reader.thread.take().unwrap().join().unwrap();
        let errors = read_errors.lock().unwrap_or_else(|e| e.into_inner());
        eprintln!("reader errors by stage and Win32 code: {errors:?}; old={}, new={}, invalid={}", old_count.load(Ordering::SeqCst), new_count.load(Ordering::SeqCst), bad.load(Ordering::SeqCst));
        assert_eq!(bad.load(Ordering::SeqCst), 0, "reader observed incomplete or unexpected content");
        let missing: usize = errors.iter().filter(|((_, code, _), _)| *code == Some(2)).map(|(_, count)| count).sum();
        assert_eq!(missing, 0, "reader observed ERROR_FILE_NOT_FOUND: {errors:?}");
        // Only an open-time sharing violation has a known explanation here;
        // all read failures and all other open failures remain visible failures.
        for ((stage, code, kind), count) in errors.iter() {
            assert!(*stage == "open" && *code == Some(32), "unexpected reader error at {stage}, code {code:?}, kind {kind}, count {count}");
        }
        let file = fs::OpenOptions::new().read(true).share_mode(7).open(&target).unwrap(); assert_eq!(identity(file.as_raw_handle() as HANDLE), source_id); assert_eq!(fs::read(&target).unwrap(), new_bytes);
    }
}




fn retry_rename(mut operation: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    use windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION;
    const ATTEMPTS: usize = 3;
    for attempt in 0..ATTEMPTS {
        match operation() {
            Ok(()) => return Ok(()),
            Err(e) if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION as i32) && attempt + 1 < ATTEMPTS => std::thread::sleep(std::time::Duration::from_millis(10)),
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}
impl<'a> PreparedEndpointTemp<'a> {
    /// Publishes the already-flushed source while retaining the directory lock.
    fn publish(self) -> io::Result<Publication<'a>> { self.publish_observed(|_| {}) }
    fn publish_observed(mut self, mut phase: impl FnMut(&'static str)) -> io::Result<Publication<'a>> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::{Foundation::ERROR_FILE_NOT_FOUND, Storage::FileSystem::{FILE_RENAME_INFO, FileRenameInfoEx, SetFileInformationByHandle}};
        let lock = self._temp.lock;
        let directory = lock.directory();
        phase("target validation");
        let replace = match directory.open_existing_private_file("endpoint.json") {
            Ok(existing) => { drop(existing); true }
            Err(e) if e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => false,
            Err(e) => return Err(e),
        };
        let mut published_id = windows_sys::Win32::Storage::FileSystem::FILE_ID_INFO::default();
        if unsafe { windows_sys::Win32::Storage::FileSystem::GetFileInformationByHandleEx(self._temp._file.file.0, windows_sys::Win32::Storage::FileSystem::FileIdInfo, (&mut published_id as *mut windows_sys::Win32::Storage::FileSystem::FILE_ID_INFO).cast(), mem::size_of_val(&published_id) as u32) } == 0 { return Err(io::Error::last_os_error()); }
        let dest = directory.path.join("endpoint.json");
        let name: Vec<u16> = dest.as_os_str().encode_wide().collect();
        let size = mem::offset_of!(FILE_RENAME_INFO, FileName) + (name.len() + 1) * 2;
        let mut buffer = vec![0usize; size.div_ceil(mem::size_of::<usize>())];
        unsafe {
            let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
            (*info).Anonymous.Flags = if replace { 1 } else { 0 };
            (*info).RootDirectory = ptr::null_mut(); (*info).FileNameLength = (name.len() * 2) as u32;
            ptr::copy_nonoverlapping(name.as_ptr(), ptr::addr_of_mut!((*info).FileName).cast::<u16>(), name.len());
            phase("rename");
            retry_rename(|| if SetFileInformationByHandle(self._temp._file.file.0, FileRenameInfoEx, info.cast(), size as u32) == 0 { Err(io::Error::last_os_error()) } else { Ok(()) })?;
        }
        self._temp._file.cleanup = false;
        Ok(Publication { lock, identity: (published_id.VolumeSerialNumber, published_id.FileId.Identifier), active: true })
    }
}

#[cfg(test)]
mod endpoint_publish_tests {
    use super::*;
    use std::{fs, os::windows::io::AsRawHandle, time::{SystemTime, UNIX_EPOCH}};
    use windows_sys::Win32::Storage::FileSystem::{FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx};
    fn id(h: HANDLE) -> (u64, [u8;16]) { let mut v = FILE_ID_INFO::default(); assert_ne!(unsafe { GetFileInformationByHandleEx(h, FileIdInfo, (&mut v as *mut FILE_ID_INFO).cast(), mem::size_of_val(&v) as u32) }, 0); (v.VolumeSerialNumber, v.FileId.Identifier) }
    #[test]
    fn publishes_and_replaces_valid_endpoint_rejects_hardlink() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailPublish{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos())); fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { if let Err(e) = fs::remove_dir_all(&self.0) { eprintln!("cleanup {}: {e}", self.0.display()); if !std::thread::panicking() { panic!("cleanup failed"); } } } }
        let _cleanup = Cleanup(base.clone()); let state = StateDirectory::acquire(&base.join("state")).unwrap(); let lock = state.lock().unwrap();
        let mut current: Option<Publication<'_>> = None;
        for port in [1111,2222] {
            let endpoint = super::super::discovery::Endpoint::new(port, "a".repeat(64), "b".repeat(64)).unwrap();
            let prepared = lock.create_endpoint_temp().unwrap().write_endpoint(&endpoint).unwrap(); let expected_id = id(prepared._temp._file.file.0);
            let next = prepared.publish().unwrap(); if let Some(mut previous) = current.replace(next) { previous.active = false; }

            let path = base.join("state").join("endpoint.json"); let file = fs::File::open(&path).unwrap();
            assert_eq!(id(file.as_raw_handle() as HANDLE), expected_id); assert_eq!(fs::read(&path).unwrap(), serde_json::to_vec(&endpoint).unwrap());
        }
        let mut names: Vec<_> = fs::read_dir(base.join("state")).unwrap().map(|e| e.unwrap().file_name()).collect(); names.sort();
        assert_eq!(names, vec![std::ffi::OsString::from("endpoint.json"), std::ffi::OsString::from("mindtrail.lock")]);
        {
            use std::os::windows::fs::OpenOptionsExt;
            let target = base.join("state").join("endpoint.json");
            let blocker = fs::OpenOptions::new().read(true).share_mode(3).open(&target).unwrap();
            let old_id = id(blocker.as_raw_handle() as HANDLE); let old_bytes = fs::read(&target).unwrap();
            let endpoint = super::super::discovery::Endpoint::new(4444, "a".repeat(64), "b".repeat(64)).unwrap();
            let pending = lock.create_endpoint_temp().unwrap().write_endpoint(&endpoint).unwrap(); let temp_path = pending._temp.path.clone();
            let error = pending.publish().unwrap_err(); assert_eq!(error.raw_os_error(), Some(5));
            assert!(!temp_path.exists()); let reopened = state.open_existing_private_file("endpoint.json").unwrap(); assert_eq!(id(reopened._handle.0), old_id); drop(reopened); assert_eq!(fs::read(&target).unwrap(), old_bytes); drop(blocker);
        }
        let target = base.join("state").join("endpoint.json"); let before = fs::read(&target).unwrap(); let link = base.join("state").join("other"); fs::hard_link(&target, &link).unwrap();
        let endpoint = super::super::discovery::Endpoint::new(3333, "a".repeat(64), "b".repeat(64)).unwrap();
        let prepared = lock.create_endpoint_temp().unwrap().write_endpoint(&endpoint).unwrap(); let temp_path = prepared._temp.path.clone();
        assert_eq!(prepared.publish().unwrap_err().to_string(), "file has multiple or invalid hard links"); assert!(!temp_path.exists()); assert_eq!(fs::read(&target).unwrap(), before);
    }
}






#[cfg(test)]
mod rename_retry_tests {
    use super::*;
    #[test]
    fn bounded_retry_only_for_sharing_violation() {
        let mut calls = 0;
        retry_rename(|| { calls += 1; if calls < 3 { Err(io::Error::from_raw_os_error(32)) } else { Ok(()) } }).unwrap(); assert_eq!(calls, 3);
        calls = 0; assert_eq!(retry_rename(|| { calls += 1; Err(io::Error::from_raw_os_error(32)) }).unwrap_err().raw_os_error(), Some(32)); assert_eq!(calls, 3);
        calls = 0; assert_eq!(retry_rename(|| { calls += 1; Err(io::Error::from_raw_os_error(5)) }).unwrap_err().raw_os_error(), Some(5)); assert_eq!(calls, 1);
    }
}

#[cfg(test)]
mod endpoint_reader_process_tests {
    use super::*;
    use std::{fs, io::Read, os::windows::fs::OpenOptionsExt, process::{Child, Command, Stdio}, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};
    #[test]
    fn reader_process_observes_complete_publications() {
        let make = |port| super::super::discovery::Endpoint::new(port, "a".repeat(64), "b".repeat(64)).unwrap();
        let old = serde_json::to_vec(&make(1111)).unwrap(); let new = serde_json::to_vec(&make(2222)).unwrap();
        if let Some(path) = std::env::var_os("MINDTRAIL_ENDPOINT_READER") {
            let base = std::path::PathBuf::from(path); let target = base.join("state").join("endpoint.json");
            let mut old_count = 0u64; let mut new_count = 0u64; let mut missing = 0u64; let mut sharing = 0u64; let mut other = 0u64; let mut bad = 0u64;
            let deadline = Instant::now() + Duration::from_secs(20);
            while !base.join("stop").exists() && Instant::now() < deadline {
                let read = || -> io::Result<Vec<u8>> { let mut file = fs::OpenOptions::new().read(true).share_mode(7).open(&target)?; let mut bytes = Vec::new(); file.read_to_end(&mut bytes)?; Ok(bytes) };
                match read() {
                    Ok(bytes) if bytes == old => { old_count += 1; if old_count == 1 { fs::write(base.join("old_seen"), b"1").unwrap(); } }
                    Ok(bytes) if bytes == new => { new_count += 1; if new_count == 1 { fs::write(base.join("new_seen"), b"1").unwrap(); } }
                    Ok(_) => bad += 1,
                    Err(e) if e.raw_os_error() == Some(2) => missing += 1,
                    Err(e) if e.raw_os_error() == Some(32) => sharing += 1,
                    Err(_) => other += 1,
                }
            }
            fs::write(base.join("stats.tmp"), format!("{old_count},{new_count},{missing},{sharing},{other},{bad}")).unwrap(); fs::rename(base.join("stats.tmp"), base.join("stats")).unwrap();
            return;
        }
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailReader{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos())); fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { if let Err(e) = fs::remove_dir_all(&self.0) { eprintln!("cleanup {}: {e}", self.0.display()); if !std::thread::panicking() { panic!("cleanup failed"); } } } }
        struct Guard(Child); impl Drop for Guard { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }
        let _cleanup = Cleanup(base.clone()); let state = StateDirectory::acquire(&base.join("state")).unwrap(); let lock = state.lock().unwrap();
        let mut current = lock.create_endpoint_temp().unwrap().write_endpoint(&make(1111)).unwrap().publish().unwrap();
        let target = base.join("state").join("endpoint.json"); let link = base.join("state").join("link"); fs::hard_link(&target, &link).unwrap();
        let temp = lock.create_endpoint_temp().unwrap().write_endpoint(&make(2222)).unwrap(); let temp_path = temp._temp.path.clone();
        assert_eq!(temp.publish().unwrap_err().to_string(), "file has multiple or invalid hard links"); assert!(!temp_path.exists()); assert_eq!(fs::read(&target).unwrap(), old); fs::remove_file(link).unwrap();
        let temp = lock.create_endpoint_temp().unwrap(); let temp_path = temp.path.clone();
        assert!(temp.write_endpoint_with(&make(2222), |_| Err(io::Error::other("injected write failure")), || Ok(())).is_err()); assert!(!temp_path.exists()); assert_eq!(fs::read(&target).unwrap(), old);
        let mut reader = Guard(Command::new(std::env::current_exe().unwrap()).args(["--exact", "gateway::windows_security::endpoint_reader_process_tests::reader_process_observes_complete_publications"]).env("MINDTRAIL_ENDPOINT_READER", &base).stdout(Stdio::null()).spawn().unwrap());
        let wait = |path: &std::path::Path| { let deadline = Instant::now() + Duration::from_secs(15); while !path.exists() { assert!(Instant::now() < deadline, "reader barrier timed out"); std::thread::sleep(Duration::from_millis(5)); } };
        wait(&base.join("old_seen"));
        let deadline = Instant::now() + Duration::from_secs(15); let mut published = 0; let mut publish_errors = 0;
        while published < 40 {
            assert!(Instant::now() < deadline, "publication timeout");
            let port = if published % 2 == 0 { 2222 } else { 1111 };
            use windows_sys::Win32::Storage::FileSystem::{FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx};
            let identity = |handle: HANDLE| { let mut v = FILE_ID_INFO::default(); assert_ne!(unsafe { GetFileInformationByHandleEx(handle, FileIdInfo, (&mut v as *mut FILE_ID_INFO).cast(), mem::size_of_val(&v) as u32) }, 0); (v.VolumeSerialNumber, v.FileId.Identifier) };
            let before = state.open_existing_private_file("endpoint.json").unwrap();
            let before_id = identity(before._handle.0); let before_bytes = fs::read(&target).unwrap(); drop(before);
            let prepared = lock.create_endpoint_temp().unwrap().write_endpoint(&make(port)).unwrap(); let temp_path = prepared._temp.path.clone();
            let mut phase = "not started";
            match prepared.publish_observed(|p| phase = p) {
                Ok(next) => { current.active = false; current = next; published += 1; },
                Err(e) if e.raw_os_error() == Some(5) => {
                    publish_errors += 1; eprintln!("publication access denied at {phase}");
                    assert!(!temp_path.exists());
                    let after = state.open_existing_private_file("endpoint.json").unwrap(); assert_eq!(identity(after._handle.0), before_id); drop(after);
                    assert_eq!(fs::read(&target).unwrap(), before_bytes);
                },
                Err(e) => panic!("unexpected publication error at {phase}: {e}"),
            }
        }
        eprintln!("successful publications={published}, access-denied failures={publish_errors}");
        wait(&base.join("new_seen")); fs::write(base.join("stop"), b"1").unwrap(); wait(&base.join("stats"));
        let deadline=Instant::now()+Duration::from_secs(15); loop { if let Some(status)=reader.0.try_wait().unwrap() { assert!(status.success()); break; } assert!(Instant::now()<deadline, "reader exit timeout"); std::thread::sleep(Duration::from_millis(5)); }
        let counts: Vec<u64> = fs::read_to_string(base.join("stats")).unwrap().split(',').map(|v| v.parse().unwrap()).collect();
        eprintln!("reader counts old={}, new={}, missing={}, sharing={}, other={}, invalid={}", counts[0], counts[1], counts[2], counts[3], counts[4], counts[5]);
        assert!(current.active);
        assert!(counts[0] >= 1 && counts[1] >= 1 && counts[0] + counts[1] >= 20); assert_eq!(counts[2], 0, "endpoint was missing during reading"); assert_eq!(counts[5], 0);
    }
}





struct DeleteCheckedFile<'a> { _file: CheckedPrivateFile<'a>, identity: (u64, [u8; 16]) }
impl StateDirectory {
    fn open_existing_private_file_for_delete(&self, name: &'static str) -> io::Result<DeleteCheckedFile<'_>> {
        use windows_sys::Win32::Storage::FileSystem::{FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx};
        let file = self.open_existing_private_file_with_access(name, true)?;
        let mut identity = FILE_ID_INFO::default();
        if unsafe { GetFileInformationByHandleEx(file._handle.0, FileIdInfo, (&mut identity as *mut FILE_ID_INFO).cast(), mem::size_of_val(&identity) as u32) } == 0 { return Err(io::Error::last_os_error()); }
        Ok(DeleteCheckedFile { _file: file, identity: (identity.VolumeSerialNumber, identity.FileId.Identifier) })
    }
}

#[cfg(test)]
mod delete_checked_file_tests {
    use super::*;
    use std::{fs::{self, OpenOptions}, os::windows::{fs::OpenOptionsExt, io::AsRawHandle}, time::{SystemTime, UNIX_EPOCH}};
    use windows_sys::Win32::Security::{ACL, ACL_REVISION, AddAccessAllowedAce, CreateWellKnownSid, InitializeAcl, WinWorldSid, SECURITY_MAX_SID_SIZE, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, Authorization::{SetSecurityInfo, SE_FILE_OBJECT}};
    #[test]
    fn checks_delete_access_file_identity_and_acl() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailDeleteCheck{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos())); fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { if let Err(e) = fs::remove_dir_all(&self.0) { eprintln!("cleanup {}: {e}", self.0.display()); if !std::thread::panicking() { panic!("cleanup failed"); } } } }
        let _cleanup = Cleanup(base.clone()); let state = StateDirectory::acquire(&base.join("state")).unwrap();
        let path = base.join("state").join("file"); fs::write(&path, b"sentinel").unwrap();
        let reader = OpenOptions::new().read(true).share_mode(7).open(&path).unwrap();
                let checked = state.open_existing_private_file_for_delete("file").unwrap();
        let mut independent = windows_sys::Win32::Storage::FileSystem::FILE_ID_INFO::default();
        assert_ne!(unsafe { windows_sys::Win32::Storage::FileSystem::GetFileInformationByHandleEx(reader.as_raw_handle() as HANDLE, windows_sys::Win32::Storage::FileSystem::FileIdInfo, (&mut independent as *mut windows_sys::Win32::Storage::FileSystem::FILE_ID_INFO).cast(), mem::size_of_val(&independent) as u32) }, 0);
        assert_eq!(checked.identity, (independent.VolumeSerialNumber, independent.FileId.Identifier)); drop(checked); drop(reader);
        let blocker = OpenOptions::new().read(true).share_mode(3).open(&path).unwrap();
        assert_eq!(state.open_existing_private_file_for_delete("file").err().unwrap().raw_os_error(), Some(32)); drop(blocker);
        let link = base.join("state").join("link"); fs::hard_link(&path, &link).unwrap();
        assert_eq!(state.open_existing_private_file_for_delete("file").err().unwrap().to_string(), "file has multiple or invalid hard links"); fs::remove_file(link).unwrap();
        let file = OpenOptions::new().access_mode(0x00060000).share_mode(7).open(&path).unwrap();
        unsafe {
            let user = current_user_sid().unwrap(); let mut world = [0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)]; let mut len = SECURITY_MAX_SID_SIZE;
            assert_ne!(CreateWellKnownSid(WinWorldSid, ptr::null_mut(), world.as_mut_ptr().cast(), &mut len), 0);
            let mut storage = [0u32; 128]; let acl = storage.as_mut_ptr().cast::<ACL>(); assert_ne!(InitializeAcl(acl, mem::size_of_val(&storage) as u32, ACL_REVISION), 0);
            assert_ne!(AddAccessAllowedAce(acl, ACL_REVISION, 0x1f01ff, user.as_ptr() as *mut _), 0); assert_ne!(AddAccessAllowedAce(acl, ACL_REVISION, 1, world.as_mut_ptr().cast()), 0);
            assert_eq!(SetSecurityInfo(file.as_raw_handle() as HANDLE, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(), acl, ptr::null()), 0);
        }
        assert_eq!(state.open_existing_private_file_for_delete("file").err().unwrap().to_string(), "file DACL grants an unexpected principal");
        assert_eq!(fs::read(&path).unwrap(), b"sentinel"); drop(file);
    }
}


/// A publication identity tied to its still-held directory lock. Construction
/// is internal; the identity must originate from the successful source handle.
pub(super) struct Publication<'a> { lock: &'a StateDirectoryLock<'a>, identity: (u64, [u8; 16]), active: bool }
impl std::fmt::Debug for Publication<'_> { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("Publication") } }
impl Publication<'_> {
    pub(super) fn withdraw(&mut self) -> io::Result<()> { if !self.active { return Ok(()); } self.withdraw_inner()?; self.active = false; Ok(()) }
    fn withdraw_inner(&self) -> io::Result<()> {
        use windows_sys::Win32::Storage::FileSystem::{FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle};
        let checked = self.lock.directory().open_existing_private_file_for_delete("endpoint.json")?;
        if checked.identity != self.identity { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "published endpoint identity changed")); }
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        if unsafe { SetFileInformationByHandle(checked._file._handle.0, FileDispositionInfo, (&disposition as *const FILE_DISPOSITION_INFO).cast(), mem::size_of_val(&disposition) as u32) } == 0 { return Err(io::Error::last_os_error()); }
        Ok(())
    }
}

impl Drop for Publication<'_> {
    fn drop(&mut self) { if self.active { if let Err(e) = self.withdraw_inner() { eprintln!("endpoint withdrawal failed: {e}"); } } }
}
#[cfg(test)]
mod publication_withdraw_tests {
    use super::*;
    use std::{fs, time::{SystemTime, UNIX_EPOCH}};
    use windows_sys::Win32::Storage::FileSystem::{FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx};
    fn identity(h: HANDLE) -> (u64,[u8;16]) { let mut v = FILE_ID_INFO::default(); assert_ne!(unsafe { GetFileInformationByHandleEx(h, FileIdInfo, (&mut v as *mut FILE_ID_INFO).cast(), mem::size_of_val(&v) as u32) }, 0); (v.VolumeSerialNumber, v.FileId.Identifier) }
    #[test]
    fn withdraws_own_endpoint_and_preserves_replacement() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailWithdraw{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos())); fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { if let Err(e) = fs::remove_dir_all(&self.0) { eprintln!("cleanup {}: {e}", self.0.display()); if !std::thread::panicking() { panic!("cleanup failed"); } } } }
        let _cleanup = Cleanup(base.clone()); let state = StateDirectory::acquire(&base.join("state")).unwrap(); let lock = state.lock().unwrap();
        let endpoint = super::super::discovery::Endpoint::new(1234, "a".repeat(64), "b".repeat(64)).unwrap();
        let prepared = lock.create_endpoint_temp().unwrap().write_endpoint(&endpoint).unwrap(); let id = identity(prepared._temp._file.file.0); let mut published = prepared.publish().unwrap(); assert_eq!(published.identity, id);
        published.withdraw().unwrap(); let target = base.join("state").join("endpoint.json"); assert!(!target.exists());
        let prepared = lock.create_endpoint_temp().unwrap().write_endpoint(&endpoint).unwrap(); let id = identity(prepared._temp._file.file.0); let mut published = prepared.publish().unwrap(); assert_eq!(published.identity, id);
        let old_path = base.join("state").join("previous.json"); fs::rename(&target, &old_path).unwrap(); fs::write(&target, b"replacement").unwrap();
        assert_eq!(published.withdraw().unwrap_err().to_string(), "published endpoint identity changed"); assert_eq!(fs::read(&target).unwrap(), b"replacement");
    }
}






#[cfg(test)]
mod publication_reader_tests {
    use super::*;
    use std::{fs, io::{Read, Seek, SeekFrom}, os::windows::fs::OpenOptionsExt, time::{SystemTime, UNIX_EPOCH}};
    #[test]
    fn withdrawal_with_existing_reader_and_drop() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailWithdrawReader{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos())); fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { if let Err(e) = fs::remove_dir_all(&self.0) { eprintln!("cleanup {}: {e}", self.0.display()); if !std::thread::panicking() { panic!("cleanup failed"); } } } }
        let _cleanup = Cleanup(base.clone()); let state = StateDirectory::acquire(&base.join("state")).unwrap(); let lock = state.lock().unwrap();
        let endpoint = super::super::discovery::Endpoint::new(1234, "a".repeat(64), "b".repeat(64)).unwrap(); let bytes = serde_json::to_vec(&endpoint).unwrap();
        let mut publication = lock.create_endpoint_temp().unwrap().write_endpoint(&endpoint).unwrap().publish().unwrap();
        let path = base.join("state").join("endpoint.json"); let mut reader = fs::OpenOptions::new().read(true).share_mode(7).open(&path).unwrap();
        publication.withdraw().unwrap();
        let e = fs::OpenOptions::new().read(true).share_mode(7).open(&path).unwrap_err(); assert_eq!(e.raw_os_error(), Some(5));
        let mut observed = Vec::new(); reader.seek(SeekFrom::Start(0)).unwrap(); reader.read_to_end(&mut observed).unwrap(); assert_eq!(observed, bytes);
        drop(reader); assert!(!path.exists());
        let publication = lock.create_endpoint_temp().unwrap().write_endpoint(&endpoint).unwrap().publish().unwrap(); drop(publication); assert!(!path.exists());
        let mut publication = lock.create_endpoint_temp().unwrap().write_endpoint(&endpoint).unwrap().publish().unwrap();
        let blocker = fs::OpenOptions::new().read(true).share_mode(3).open(&path).unwrap();
        assert_eq!(publication.withdraw().unwrap_err().raw_os_error(), Some(32)); assert!(publication.active); drop(blocker); assert!(path.exists());
        publication.withdraw().unwrap(); assert!(!publication.active); assert!(!path.exists());
        fs::write(&path, b"replacement").unwrap(); publication.withdraw().unwrap(); drop(publication); assert_eq!(fs::read(&path).unwrap(), b"replacement");
    }
}



impl StateDirectoryLock<'_> {
    /// Checks only pre-existing SQLite files; absence is the only accepted
    /// open failure. It does not open SQLite or guarantee later path opens.
    fn preflight_sqlite_files(&self) -> io::Result<()> {
        for name in ["mindtrail.sqlite3", "mindtrail.sqlite3-wal", "mindtrail.sqlite3-shm", "mindtrail.sqlite3-journal"] {
            match self.directory().open_existing_private_file(name) {
                Ok(file) => drop(file),
                Err(e) if e.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND as i32) => {},
                Err(e) => return Err(io::Error::new(e.kind(), format!("SQLite preflight {name}: {e}"))),
            }
        }
        Ok(())
    }

    #[cfg(feature = "producer")]
    fn preflight_queue_files(&self) -> io::Result<()> {
        for name in ["queue.sqlite3", "queue.sqlite3-wal", "queue.sqlite3-shm", "queue.sqlite3-journal"] {
            match self.directory().open_existing_private_file(name) {
                Ok(file) => drop(file),
                Err(e) if e.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND as i32) => {},
                Err(e) => return Err(io::Error::new(e.kind(), format!("queue preflight {name}: {e}"))),
            }
        }
        Ok(())
    }
}

/// Retain each verified directory and the queue lock for the complete callback.
#[cfg(feature = "producer")]
pub(crate) fn with_producer_queue<T>(root: &std::path::Path, mode: &str, name: &str, f: impl FnOnce(&std::path::Path, &dyn Fn() -> io::Result<()>) -> T) -> io::Result<T> {
    if !matches!(mode, "mcp" | "explicit") || !crate::producer::queue::valid_queue_name(name) { return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid queue")); }
    let observe_path = |stage: &'static str, e: io::Error| { #[cfg(feature="producer-test-fixture")] { crate::producer::faults::process_marker(stage); crate::producer::faults::mark_path_error(&e); } #[cfg(not(feature="producer-test-fixture"))] let _=stage; e };
    let _root = StateDirectory::acquire(root).map_err(|e| observe_path("stage-root",e))?;
    let producers_path = root.join("producers"); let _producers = StateDirectory::acquire(&producers_path).map_err(|e| observe_path("stage-producers",e))?;
    let mode_path = producers_path.join(mode); let _mode = StateDirectory::acquire(&mode_path).map_err(|e| observe_path("stage-mode",e))?;
    let queue_path = mode_path.join(name); let queue = StateDirectory::acquire(&queue_path).map_err(|e| observe_path("stage-queue",e))?;
    let lock = queue.lock().map_err(|e| { #[cfg(feature="producer-test-fixture")] crate::producer::faults::process_marker("open-lock"); match e { FileLockError::Contended => io::Error::new(io::ErrorKind::WouldBlock, "queue busy"), FileLockError::System(error) => error } })?;
    lock.preflight_queue_files().map_err(|e| observe_path("stage-preflight",e))?;
    Ok(f(&queue_path.join("queue.sqlite3"), &|| lock.preflight_queue_files()))
}

/// Read discovery through the same checked, no-follow handle; do not acquire the gateway lock.
#[cfg(feature = "producer")]
pub(crate) fn read_producer_discovery(root: &std::path::Path) -> io::Result<Vec<u8>> {
    use std::{io::Read, os::windows::io::FromRawHandle};
    let directory=StateDirectory::acquire(root)?;
    let checked=directory.open_existing_private_file_checked_access("endpoint.json",false,true,||Ok(()))?;
    let mut file=std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_handle(checked._handle.0.cast()) });
    let mut bytes=Vec::new(); std::io::Read::by_ref(&mut *file).take(4097).read_to_end(&mut bytes)?;
    if bytes.len()>4096 { return Err(io::Error::new(io::ErrorKind::InvalidData,"discovery too large")); }
    Ok(bytes)
}

#[cfg(test)]
mod sqlite_preflight_tests {
    use super::*;
    use std::{fs::{self, OpenOptions}, os::windows::{fs::OpenOptionsExt, io::AsRawHandle}, time::{SystemTime, UNIX_EPOCH}};
    use windows_sys::Win32::Security::{ACL, ACL_REVISION, AddAccessAllowedAce, CreateWellKnownSid, InitializeAcl, WinWorldSid, SECURITY_MAX_SID_SIZE, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, Authorization::{SetSecurityInfo, SE_FILE_OBJECT}};
    #[test]
    fn existing_missing_and_unsafe_sqlite_files() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailPreflight{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos())); fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { if let Err(e) = fs::remove_dir_all(&self.0) { eprintln!("cleanup {}: {e}", self.0.display()); if !std::thread::panicking() { panic!("cleanup failed"); } } } }
        let _cleanup = Cleanup(base.clone()); let state = StateDirectory::acquire(&base.join("state")).unwrap(); let lock = state.lock().unwrap(); let dir = base.join("state");
        lock.preflight_sqlite_files().unwrap();
        for name in ["mindtrail.sqlite3", "mindtrail.sqlite3-wal", "mindtrail.sqlite3-shm", "mindtrail.sqlite3-journal"] { fs::write(dir.join(name), b"sentinel").unwrap(); }
        lock.preflight_sqlite_files().unwrap();
        let wal = dir.join("mindtrail.sqlite3-wal"); let link = dir.join("hardlink"); fs::hard_link(&wal, &link).unwrap();
        assert!(lock.preflight_sqlite_files().unwrap_err().to_string().contains("mindtrail.sqlite3-wal: file has multiple")); assert_eq!(fs::read(&wal).unwrap(), b"sentinel"); fs::remove_file(link).unwrap();
        let shm = dir.join("mindtrail.sqlite3-shm"); let file = OpenOptions::new().access_mode(0x00060000).share_mode(7).open(&shm).unwrap();
        unsafe {
            let user = current_user_sid().unwrap(); let mut world = [0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)]; let mut len = SECURITY_MAX_SID_SIZE;
            assert_ne!(CreateWellKnownSid(WinWorldSid, ptr::null_mut(), world.as_mut_ptr().cast(), &mut len), 0);
            let mut storage = [0u32; 128]; let acl = storage.as_mut_ptr().cast::<ACL>(); assert_ne!(InitializeAcl(acl, mem::size_of_val(&storage) as u32, ACL_REVISION), 0);
            assert_ne!(AddAccessAllowedAce(acl, ACL_REVISION, 0x1f01ff, user.as_ptr() as *mut _), 0); assert_ne!(AddAccessAllowedAce(acl, ACL_REVISION, 1, world.as_mut_ptr().cast()), 0);
            assert_eq!(SetSecurityInfo(file.as_raw_handle() as HANDLE, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(), acl, ptr::null()), 0);
        }
        assert!(lock.preflight_sqlite_files().unwrap_err().to_string().contains("mindtrail.sqlite3-shm: file DACL grants an unexpected principal")); assert_eq!(fs::read(&shm).unwrap(), b"sentinel"); drop(file);
        fs::remove_file(&shm).unwrap(); let target = base.join("target"); fs::create_dir(&target).unwrap(); fs::write(target.join("sentinel"), b"untouched").unwrap();
        let output = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&shm).arg(&target).output().unwrap(); assert!(output.status.success());
        let error = lock.preflight_sqlite_files().unwrap_err(); assert!(error.to_string().contains("mindtrail.sqlite3-shm")); assert_eq!(fs::read(target.join("sentinel")).unwrap(), b"untouched"); fs::remove_dir(&shm).unwrap();
    }
}


#[cfg(test)]
mod sqlite_preflight_stage_tests {
    use super::*;
    use std::{fs, time::{SystemTime, UNIX_EPOCH}};
    #[test]
    fn only_initial_open_not_found_is_absence() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailPreflightStage{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos())); fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { if let Err(e) = fs::remove_dir_all(&self.0) { eprintln!("cleanup {}: {e}", self.0.display()); if !std::thread::panicking() { panic!("cleanup failed"); } } } }
        let _cleanup = Cleanup(base.clone()); let state = StateDirectory::acquire(&base.join("state")).unwrap(); let lock = state.lock().unwrap();
        let missing = state.open_existing_private_file_checked("mindtrail.sqlite3", false, || panic!("check must not run")).err().unwrap(); assert_eq!(missing.raw_os_error(), Some(2));
        lock.preflight_sqlite_files().unwrap(); fs::write(base.join("state").join("mindtrail.sqlite3"), b"sentinel").unwrap();
        let error = state.open_existing_private_file_checked("mindtrail.sqlite3", false, || Err(io::Error::from_raw_os_error(2))).err().unwrap();
        assert_eq!(error.raw_os_error(), None); assert_eq!(error.kind(), io::ErrorKind::NotFound); assert_eq!(fs::read(base.join("state").join("mindtrail.sqlite3")).unwrap(), b"sentinel");
    }
}

/// Retains the lock borrow while the returned Store is usable.
pub(super) struct LockedStore<'lock, 'dir> { _lock: &'lock StateDirectoryLock<'dir>, store: std::sync::Arc<super::store::Store> }
/// Keeps the directory lock borrowed until all admitted work has finished.
pub(super) struct StoreAccessGuard<'store, 'lock, 'dir> {
    _locked: &'store mut LockedStore<'lock, 'dir>,
    access: super::access::StoreAccess,
}
impl<'lock, 'dir> LockedStore<'lock, 'dir> {
    pub(super) fn access_guard(&mut self) -> StoreAccessGuard<'_, 'lock, 'dir> {
        let access = super::access::StoreAccess::new(self.store.clone());
        StoreAccessGuard { _locked: self, access }
    }
    pub(super) fn store_id(&self) -> &str { self.store.store_id() }
}
impl StoreAccessGuard<'_, '_, '_> {
    pub(super) fn access(&self) -> super::access::StoreAccess { self.access.clone() }
}
impl Drop for StoreAccessGuard<'_, '_, '_> {
    fn drop(&mut self) { self.access.close(); self.access.wait_idle(); }
}
impl<'dir> StateDirectoryLock<'dir> {
    pub(super) fn preflight_endpoint(&self) -> io::Result<()> {
        match self.directory().open_existing_private_file("endpoint.json") {
            Ok(file) => { drop(file); Ok(()) },
            Err(error) if error.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND as i32) => Ok(()),
            Err(error) => Err(error),
        }
    }
    pub(super) fn publish_endpoint(&'dir self, endpoint: &super::discovery::Endpoint) -> io::Result<Publication<'dir>> {
        self.create_endpoint_temp()?.write_endpoint(endpoint)?.publish()
    }
    pub(super) fn open_store<'lock>(&'lock self) -> Result<LockedStore<'lock, 'dir>, crate::protocol::ProtocolError> {
        self.preflight_sqlite_files().map_err(|e| crate::protocol::temporary_storage_error(e.to_string()))?;
        let path = self.directory().path.join("mindtrail.sqlite3");
        let store = super::store::Store::open_with_postcheck(&path, || {
            // The main database must exist; sidecars are checked if present.
            drop(self.directory().open_existing_private_file("mindtrail.sqlite3")?);
            self.preflight_sqlite_files()
        })?;
        Ok(LockedStore { _lock: self, store: std::sync::Arc::new(store) })
    }
}

#[cfg(test)]
mod locked_store_tests {
    use super::*;
    use std::{fs, time::{SystemTime, UNIX_EPOCH}};
    #[test]
    fn guard_closes_access_and_retains_lock_until_worker_finishes() {
        use std::{sync::mpsc, time::Duration};
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailGuard{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
        let _cleanup = Cleanup(base.clone());
        let (ready_tx, ready_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let state_path = base.join("state");
        let owner_path = state_path.clone();
        let owner = std::thread::spawn(move || {
            let state = StateDirectory::acquire(&owner_path).unwrap();
            let lock = state.lock().unwrap();
            let mut locked = lock.open_store().unwrap();
            let guard = locked.access_guard();
            let access = guard.access();
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            let (started_tx, started_rx) = mpsc::channel();
            let worker_access = access.clone();
            let task = runtime.spawn(async move {
                worker_access.run(move |_| {
                    started_tx.send(()).unwrap();
                    let _ = release_rx.recv_timeout(Duration::from_secs(15));
                    Ok(())
                }).await
            });
            runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(5), async {
                    loop { match started_rx.try_recv() {
                        Ok(()) => break,
                        Err(mpsc::TryRecvError::Disconnected) => panic!("worker exited"),
                        Err(mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
                    }}
                }).await.unwrap();
            });
            ready_tx.send(access).unwrap();
            let _ = stop_rx.recv_timeout(Duration::from_secs(5));
            drop(guard);
            drop(locked); drop(lock); drop(state);
            done_tx.send(()).unwrap();
            runtime.block_on(async { let _ = tokio::time::timeout(Duration::from_secs(5), task).await; });
            runtime.shutdown_timeout(Duration::from_secs(1));
        });
        let access = ready_rx.recv_timeout(Duration::from_secs(8)).unwrap();
        stop_tx.send(()).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let closed = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if access.run(|_| Ok(())).await.is_err() { break; }
                    tokio::task::yield_now().await;
                }
            }).await
        });
        assert!(closed.is_ok(), "access did not close");
        let contender = StateDirectory::acquire(&state_path).unwrap();
        assert!(contender.lock().is_err());
        assert!(matches!(done_rx.recv_timeout(Duration::from_millis(200)), Err(mpsc::RecvTimeoutError::Timeout)), "guard returned before worker finished");
        release_tx.send(()).unwrap();
        done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        owner.join().unwrap();
        assert!(contender.lock().is_ok());
        runtime.shutdown_timeout(Duration::from_secs(1));
    }
    #[test]
    fn unicode_store_wal_and_preflight_rejection() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrail数据库{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos())); fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { if let Err(e) = fs::remove_dir_all(&self.0) { eprintln!("cleanup {}: {e}", self.0.display()); if !std::thread::panicking() { panic!("cleanup failed"); } } } }
        let _cleanup = Cleanup(base.clone()); let state = StateDirectory::acquire(&base.join("state")).unwrap(); let lock = state.lock().unwrap();
        let locked = lock.open_store().unwrap(); assert_eq!(locked.store.store_id().len(), 64);
        let path = state.path().join("mindtrail.sqlite3"); let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS preflight_probe (value TEXT); INSERT INTO preflight_probe VALUES ('ok');").unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM preflight_probe WHERE value='ok'", [], |r| r.get(0)).unwrap(); assert_eq!(count, 1);
        assert!(base.join("state").join("mindtrail.sqlite3-wal").exists()); assert!(base.join("state").join("mindtrail.sqlite3-shm").exists()); lock.preflight_sqlite_files().unwrap();
        drop(conn); drop(locked); drop(lock); drop(state);
        let state2 = StateDirectory::acquire(&base.join("state2")).unwrap(); let lock2 = state2.lock().unwrap();
        lock2.preflight_sqlite_files().unwrap();
        let mut postcheck_called = false;
        let second = super::super::store::Store::open_with_postcheck(&state2.path().join("mindtrail.sqlite3"), || {
            postcheck_called = true;
            assert!(base.join("state2").join("mindtrail.sqlite3-wal").exists());
            assert!(base.join("state2").join("mindtrail.sqlite3-shm").exists());
            lock2.preflight_sqlite_files()
        }).unwrap();
        assert!(postcheck_called); assert_eq!(second.store_id().len(), 64);
    }
    #[test]
    fn store_api_reconnect_and_failed_postcheck_preserve_data() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailStoreApi{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { if let Err(e) = fs::remove_dir_all(&self.0) { if !std::thread::panicking() { panic!("cleanup failed: {e}"); } } } }
        let _cleanup = Cleanup(base.clone());
        let state = StateDirectory::acquire(&base.join("state")).unwrap(); let lock = state.lock().unwrap();
        let locked = lock.open_store().unwrap(); let id = locked.store.store_id().to_owned();
        let batch = crate::protocol::decode_append(br#"[{"v":1,"session":"s","operation_id":"1","ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}]"#).unwrap();
        locked.store.write_append(&batch).unwrap();
        assert_eq!(locked.store.events("s", 0, 10).unwrap().events.len(), 1);
        drop(locked);
        let path = state.path().join("mindtrail.sqlite3");
        let result = super::super::store::Store::open_with_postcheck(&path, || Err(std::io::Error::new(std::io::ErrorKind::Other, "injected postcheck failure")));
        assert!(result.is_err());
        let reopened = lock.open_store().unwrap();
        assert_eq!(reopened.store.store_id(), id);
        assert_eq!(reopened.store.sessions(None, 10).unwrap().len(), 1);
        assert_eq!(reopened.store.events("s", 0, 10).unwrap().events.len(), 1);
    }
    #[test]
    fn legacy_history_survives_failed_postcheck_after_migration() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailLegacy{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { if let Err(e) = fs::remove_dir_all(&self.0) { if !std::thread::panicking() { panic!("cleanup failed: {e}"); } } } }
        let _cleanup = Cleanup(base.clone());
        let state = StateDirectory::acquire(&base.join("state")).unwrap(); let lock = state.lock().unwrap();
        let store = lock.open_store().unwrap();
        let batch = crate::protocol::decode_explicit(br#"[{"v":1,"session":"legacy","seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}},{"v":1,"session":"legacy","seq":2,"ts":"2024-01-01T00:00:00Z","type":"task.done","payload":{}}]"#).unwrap();
        store.store.write_explicit(&batch).unwrap();
        let id = store.store.store_id().to_owned(); let history = store.store.events("legacy", 0, 10).unwrap();
        drop(store);
        let path = state.path().join("mindtrail.sqlite3");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("DROP TABLE event_owners; DROP INDEX operations_session_seq; PRAGMA user_version=1").unwrap(); drop(conn);
        let result = super::super::store::Store::open_with_postcheck(&path, || Err(std::io::Error::new(std::io::ErrorKind::Other, "injected postcheck failure")));
        assert!(result.is_err());
        let conn = rusqlite::Connection::open(&path).unwrap();
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap(); assert_eq!(version, 2); drop(conn);
        let reopened = lock.open_store().unwrap();
        assert_eq!(reopened.store.store_id(), id);
        assert_eq!(reopened.store.events("legacy", 0, 10).unwrap(), history);
        let receipt = reopened.store.write_explicit(&batch).unwrap();
        assert_eq!((receipt.accepted, receipt.duplicates, receipt.last_seq), (0, 2, 2));
    }
    #[test]
    fn malicious_sidecar_blocks_store_creation() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailPreflightBlock{}{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos())); fs::create_dir(&base).unwrap();
        struct Cleanup(std::path::PathBuf); impl Drop for Cleanup { fn drop(&mut self) { if let Err(e) = fs::remove_dir_all(&self.0) { eprintln!("cleanup {}: {e}", self.0.display()); if !std::thread::panicking() { panic!("cleanup failed"); } } } }
        let _cleanup = Cleanup(base.clone()); let state = StateDirectory::acquire(&base.join("state")).unwrap(); let lock = state.lock().unwrap();
        let wal = base.join("state").join("mindtrail.sqlite3-wal"); fs::write(&wal, b"sentinel").unwrap(); let link = base.join("state").join("other"); fs::hard_link(&wal, &link).unwrap();
        assert!(lock.open_store().is_err()); assert_eq!(fs::read(&wal).unwrap(), b"sentinel"); assert!(!base.join("state").join("mindtrail.sqlite3").exists());
    }
}








#[cfg(test)]
mod producer_stage0_probe {
    use super::*;
    use std::io::{BufRead, Read, Write};
    fn read_endpoint(state: &StateDirectory) -> serde_json::Value {
        use std::os::windows::io::FromRawHandle;
        let checked = state.open_existing_private_file_checked_access("endpoint.json", false, true, || Ok(())).unwrap();
        let mut borrowed = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_handle(checked._handle.0.cast()) });
        let mut bytes = Vec::new(); std::io::Read::by_ref(&mut *borrowed).take(4097).read_to_end(&mut bytes).unwrap(); assert!(bytes.len() <= 4096);
        serde_json::from_slice(&bytes).unwrap()
    }
    fn health(endpoint: &serde_json::Value) {
        let addr = format!("127.0.0.1:{}", endpoint["port"].as_u64().unwrap());
        let mut stream = std::net::TcpStream::connect(&addr).unwrap(); stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        write!(stream, "GET /v1/health HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n", endpoint["token"].as_str().unwrap()).unwrap();
        let mut response = String::new(); stream.read_to_string(&mut response).unwrap(); assert!(response.starts_with("HTTP/1.1 200"));
    }
    #[test]
    fn concurrent_gateway_and_queue_lock_rotation() {
        if let Ok(path) = std::env::var("MINDTRAIL_STAGE0_ROOT") {
            let root = StateDirectory::acquire(std::path::Path::new(&path)).unwrap();
            let queue = StateDirectory::acquire(&std::path::Path::new(&path).join("queue")).unwrap(); let _lock = queue.lock().unwrap();
            let first = read_endpoint(&root); health(&first); println!("PROBE_READY"); std::io::stdout().flush().unwrap();
            let mut line=String::new(); std::io::stdin().read_line(&mut line).unwrap();
            let second = read_endpoint(&root); assert_eq!(first["store_id"],second["store_id"]); assert_ne!(first["token"],second["token"]); health(&second);
            println!("PROBE_ROTATED"); std::io::stdout().flush().unwrap(); return;
        }
        let mut random=[0u8;16]; getrandom::fill(&mut random).unwrap(); let suffix: String=random.iter().map(|b|format!("{b:02x}")).collect();
        let base=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailStage0-{suffix}")); std::fs::create_dir(&base).unwrap(); let path=base.join("state");
        let gateway=super::super::lifecycle::Gateway::start(&path).unwrap();
        let mut child=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","gateway::windows_security::producer_stage0_probe::concurrent_gateway_and_queue_lock_rotation","--nocapture"]).env("MINDTRAIL_STAGE0_ROOT",&path).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
        struct Guard(std::process::Child); impl Drop for Guard { fn drop(&mut self) { if self.0.try_wait().ok().flatten().is_none() { let _=self.0.kill(); let _=self.0.wait(); } } }
        let out=child.stdout.take().unwrap(); let mut child=Guard(child); let (tx,rx)=std::sync::mpsc::channel();
        std::thread::spawn(move || for line in std::io::BufReader::new(out).lines().map_while(Result::ok) { let _=tx.send(line); });
        let read = |needle:&str| loop { let line=rx.recv_timeout(std::time::Duration::from_secs(20)).expect("probe timeout"); if line.contains(needle) { break; } };
        read("PROBE_READY"); gateway.shutdown().unwrap(); let gateway=super::super::lifecycle::Gateway::start(&path).unwrap();
        writeln!(child.0.stdin.as_mut().unwrap(),"continue").unwrap(); read("PROBE_ROTATED"); assert!(child.0.wait().unwrap().success());
        let state=StateDirectory::acquire(&path).unwrap(); health(&read_endpoint(&state)); gateway.shutdown().unwrap();
        eprintln!("stage0 isolated fixture retained: {}",base.display());
    }
}



#[cfg(all(test, feature="producer"))]
mod producer_queue_acl_tests {
    use super::*;
    use std::{fs::OpenOptions, os::windows::{fs::OpenOptionsExt, io::AsRawHandle}};
    use windows_sys::Win32::Security::{ACL, ACL_REVISION, AddAccessAllowedAce, CreateWellKnownSid, InitializeAcl, WinWorldSid, SECURITY_MAX_SID_SIZE, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, Authorization::{SetSecurityInfo, SE_FILE_OBJECT}};
    #[test]
    fn queue_database_broad_acl_is_rejected_and_preserved() {
        let mut random=[0u8;16]; getrandom::fill(&mut random).unwrap(); let suffix:String=random.iter().map(|b|format!("{b:02x}")).collect();
        let base=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailQueueAcl-{suffix}")); std::fs::create_dir(&base).unwrap(); let root=base.join("state");
        use crate::producer::queue::{with_queue,QueueMode};
        for (index,name) in ["queue.sqlite3","queue.sqlite3-wal","queue.sqlite3-shm","queue.sqlite3-journal"].iter().enumerate() {
        let queue_name=format!("sample{index}");
        with_queue(&root,QueueMode::Mcp,&queue_name,|_|Ok(())).unwrap();
        let path=root.join("producers").join("mcp").join(&queue_name).join(name);
        if index != 0 { std::fs::write(&path,b"sidecar-sentinel").unwrap(); }
        let original=std::fs::read(&path).unwrap();
        let file=OpenOptions::new().access_mode(0x00060000).share_mode(7).open(&path).unwrap();
        unsafe {
            let user=current_user_sid().unwrap(); let mut world=[0u32;(SECURITY_MAX_SID_SIZE as usize).div_ceil(4)]; let mut len=SECURITY_MAX_SID_SIZE;
            assert_ne!(CreateWellKnownSid(WinWorldSid,ptr::null_mut(),world.as_mut_ptr().cast(),&mut len),0);
            let mut storage=[0u32;128]; let acl=storage.as_mut_ptr().cast::<ACL>(); assert_ne!(InitializeAcl(acl,mem::size_of_val(&storage) as u32,ACL_REVISION),0);
            assert_ne!(AddAccessAllowedAce(acl,ACL_REVISION,0x1f01ff,user.as_ptr() as *mut _),0);
            assert_ne!(AddAccessAllowedAce(acl,ACL_REVISION,1,world.as_mut_ptr().cast()),0);
            assert_eq!(SetSecurityInfo(file.as_raw_handle() as HANDLE,SE_FILE_OBJECT,DACL_SECURITY_INFORMATION|PROTECTED_DACL_SECURITY_INFORMATION,ptr::null_mut(),ptr::null_mut(),acl,ptr::null()),0);
        }
        assert_eq!(with_queue(&root,QueueMode::Mcp,&queue_name,|_|Ok(())),Err("queue_unavailable".into()));
        assert_eq!(std::fs::read(&path).unwrap(),original); drop(file);
        }
        eprintln!("isolated ACL fixture retained: {}",base.display());
    }
}

#[cfg(all(test, feature="producer"))]
mod producer_directory_acl_tests {
    use super::*;
    use std::{fs::OpenOptions, os::windows::{fs::OpenOptionsExt, io::AsRawHandle}};
    use windows_sys::Win32::{Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS, Security::{ACL,ACL_REVISION,AddAccessAllowedAce,CreateWellKnownSid,InitializeAcl,WinWorldSid,SECURITY_MAX_SID_SIZE,DACL_SECURITY_INFORMATION,PROTECTED_DACL_SECURITY_INFORMATION,Authorization::{SetSecurityInfo,SE_FILE_OBJECT}}};
    #[test]
    fn root_directory_chain_and_lock_reject_broad_acl() {
        use crate::producer::queue::{with_queue,QueueMode};
        let mut random=[0u8;16]; getrandom::fill(&mut random).unwrap(); let suffix:String=random.iter().map(|b|format!("{b:02x}")).collect();
        let base=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailQueueDirs-{suffix}")); std::fs::create_dir(&base).unwrap();
        for index in 0..5 {
            let root=base.join(format!("state{index}")); with_queue(&root,QueueMode::Mcp,"sample",|_|Ok(())).unwrap();
            let path=match index { 0=>root.clone(),1=>root.join("producers"),2=>root.join("producers/mcp"),3=>root.join("producers/mcp/sample"),_=>root.join("producers/mcp/sample/mindtrail.lock") };
            let is_dir=index<4;
            let sentinel=if is_dir { let p=path.join("sentinel"); std::fs::write(&p,b"unchanged").unwrap(); p } else { path.clone() };
            let original=std::fs::read(&sentinel).unwrap();
            let mut sentinel_reader=OpenOptions::new().read(true).share_mode(7).open(&sentinel).unwrap();
            let file=OpenOptions::new().access_mode(0x00060000).share_mode(7).custom_flags(if is_dir { FILE_FLAG_BACKUP_SEMANTICS } else { 0 }).open(&path).unwrap();
            unsafe {
                let user=current_user_sid().unwrap(); let mut world=[0u32;(SECURITY_MAX_SID_SIZE as usize).div_ceil(4)]; let mut len=SECURITY_MAX_SID_SIZE;
                assert_ne!(CreateWellKnownSid(WinWorldSid,ptr::null_mut(),world.as_mut_ptr().cast(),&mut len),0);
                let mut storage=[0u32;128]; let acl=storage.as_mut_ptr().cast::<ACL>(); assert_ne!(InitializeAcl(acl,mem::size_of_val(&storage) as u32,ACL_REVISION),0);
                assert_ne!(AddAccessAllowedAce(acl,ACL_REVISION,0x1f01ff,user.as_ptr() as *mut _),0); assert_ne!(AddAccessAllowedAce(acl,ACL_REVISION,1,world.as_mut_ptr().cast()),0);
                assert_eq!(SetSecurityInfo(file.as_raw_handle() as HANDLE,SE_FILE_OBJECT,DACL_SECURITY_INFORMATION|PROTECTED_DACL_SECURITY_INFORMATION,ptr::null_mut(),ptr::null_mut(),acl,ptr::null()),0);
            }
            assert_eq!(with_queue(&root,QueueMode::Mcp,"sample",|_|Ok(())),Err("queue_unavailable".into()));
            let mut after=Vec::new(); std::io::Read::read_to_end(&mut sentinel_reader,&mut after).unwrap();
            assert_eq!(after,original); drop(file);
        }
        eprintln!("isolated directory ACL fixture retained: {}",base.display());
    }
}


#[cfg(all(test, feature="producer"))]
mod producer_junction_matrix_tests {
    use super::*;
    #[test]
    fn each_queue_path_component_and_lock_rejects_junction() {
        use crate::producer::queue::{with_queue,QueueMode};
        let mut random=[0u8;16]; getrandom::fill(&mut random).unwrap(); let suffix:String=random.iter().map(|b|format!("{b:02x}")).collect();
        let base=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailQueueJunction-{suffix}")); std::fs::create_dir(&base).unwrap();
        for depth in 0..5 {
            let root=base.join(format!("state{depth}")); let target=base.join(format!("target{depth}")); std::fs::create_dir(&target).unwrap(); std::fs::write(target.join("sentinel"),b"unchanged").unwrap();
            let mut path=root.clone();
            for component in ["producers","mcp","sample","mindtrail.lock"].iter().take(depth) { StateDirectory::acquire(&path).unwrap(); path=path.join(component); }
            let output=std::process::Command::new("cmd").args(["/C","mklink","/J"]).arg(&path).arg(&target).output().unwrap();
            assert!(output.status.success(),"junction creation failed: {}",String::from_utf8_lossy(&output.stderr));
            assert_eq!(with_queue(&root,QueueMode::Mcp,"sample",|_|Ok(())),Err("queue_unavailable".into()));
            assert_eq!(std::fs::read(target.join("sentinel")).unwrap(),b"unchanged");
        }
        eprintln!("isolated junction matrix retained: {}",base.display());
    }
}
