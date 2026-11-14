use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::iter;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::ptr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, FILETIME, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::Cryptography::{
    CERT_NAME_RDN_TYPE, CERT_NAME_STR_REVERSE_FLAG, CERT_X500_NAME_STR, CertGetNameStringW,
};
use windows_sys::Win32::Security::WinTrust::{
    WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
    WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE,
    WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTHelperGetProvCertFromChain,
    WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSID, TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER, TokenElevation, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetProcessTimes, OpenProcess, OpenProcessToken,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};

/// Another process, opened only to ask about it.
///
/// While this is held, the process's id cannot be given to a new process,
/// so everything read through it is about the process that was opened. It
/// does not show that the process opened is the one the id named before it
/// was opened: if that one had exited, the id may already belong to another.
/// [`Process::created`] is how a caller tells.
#[derive(Debug)]
pub struct Process(OwnedHandle);

impl Process {
    /// Opens process `pid` with `PROCESS_QUERY_LIMITED_INFORMATION`, the
    /// least right that can read its image and its user.
    ///
    /// # Errors
    ///
    /// When there is no such process, or this user may not query it.
    #[allow(unsafe_code)]
    pub fn open(pid: u32) -> io::Result<Self> {
        // SAFETY: OpenProcess takes no pointers. It returns a new handle,
        // or null on failure.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `handle` is a valid process handle that nothing else owns,
        // and OwnedHandle closes it exactly once.
        Ok(Self(unsafe { OwnedHandle::from_raw_handle(handle) }))
    }

    /// When the process was created.
    ///
    /// # Errors
    ///
    /// When Windows cannot report it.
    #[allow(unsafe_code)]
    pub fn created(&self) -> io::Result<SystemTime> {
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: the handle is a live process handle opened with
        // PROCESS_QUERY_LIMITED_INFORMATION, which GetProcessTimes accepts,
        // and the four FILETIMEs are valid places to write.
        let ok = unsafe {
            GetProcessTimes(
                self.0.as_raw_handle(),
                &raw mut created,
                &raw mut exited,
                &raw mut kernel,
                &raw mut user,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // A FILETIME counts 100ns intervals since 1601-01-01 UTC.
        let ticks = u64::from(created.dwHighDateTime) << 32 | u64::from(created.dwLowDateTime);
        let since_1601 = Duration::from_nanos(ticks.saturating_mul(100));
        // 1970-01-01 is 134 774 days after 1601-01-01.
        let unix_in_1601 = Duration::from_hours(134_774 * 24);
        since_1601
            .checked_sub(unix_in_1601)
            .map(|since_unix| UNIX_EPOCH + since_unix)
            .ok_or_else(|| io::Error::other("a process created before 1970"))
    }

    /// The string form of the SID of the user the process runs as.
    ///
    /// # Errors
    ///
    /// When the process's token cannot be opened or read.
    pub fn user_sid(&self) -> io::Result<String> {
        token_user_sid(self.0.as_raw_handle())
    }
}

/// The full path of the image `process` runs, in Win32 form, of any length.
///
/// # Errors
///
/// When Windows cannot report it, for instance because the process has
/// exited.
#[allow(unsafe_code)]
pub fn process_image(process: &Process) -> io::Result<PathBuf> {
    // Windows paths can be far longer than MAX_PATH, so the buffer grows
    // until the name fits.
    let mut capacity: u32 = 1024;
    loop {
        let mut buffer = vec![0u16; capacity as usize];
        let mut size = capacity;
        // SAFETY: the handle is a live process handle opened with
        // PROCESS_QUERY_LIMITED_INFORMATION; `buffer` holds `size` u16s, and
        // `size` is a valid u32 the call updates.
        let ok = unsafe {
            QueryFullProcessImageNameW(
                process.0.as_raw_handle(),
                PROCESS_NAME_WIN32,
                buffer.as_mut_ptr(),
                &raw mut size,
            )
        };
        if ok != 0 {
            buffer.truncate(size as usize);
            return Ok(PathBuf::from(OsString::from_wide(&buffer)));
        }
        let error = io::Error::last_os_error();
        let too_small = error.raw_os_error() == i32::try_from(ERROR_INSUFFICIENT_BUFFER).ok();
        if !too_small || capacity >= 1 << 16 {
            return Err(error);
        }
        capacity *= 2;
    }
}

/// The final path of the open file `file`: every link and junction
/// resolved, in the form `std::fs::canonicalize` gives (`\\?\C:\…`).
///
/// Two names for one file compare equal only in this form, so a path read
/// from Windows is compared with a pinned path as final paths.
///
/// # Errors
///
/// When Windows cannot report it.
#[allow(unsafe_code)]
pub fn final_path(file: &File) -> io::Result<PathBuf> {
    let mut capacity: u32 = 1024;
    loop {
        let mut buffer = vec![0u16; capacity as usize];
        // SAFETY: the handle is an open file's; `buffer` holds `capacity`
        // u16s. Flags 0 are FILE_NAME_NORMALIZED and VOLUME_NAME_DOS, as
        // std::fs::canonicalize asks.
        let length = unsafe {
            GetFinalPathNameByHandleW(file.as_raw_handle(), buffer.as_mut_ptr(), capacity, 0)
        };
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        if length < capacity {
            buffer.truncate(length as usize);
            return Ok(PathBuf::from(OsString::from_wide(&buffer)));
        }
        // Too small: `length` is the size needed, counting the NUL.
        if length > 1 << 16 {
            return Err(io::Error::other("a final path longer than Windows allows"));
        }
        capacity = length;
    }
}

/// The string form of the SID of the user this process runs as, for a named
/// pipe's security descriptor and for comparing with a peer's.
///
/// # Errors
///
/// When this process's token cannot be read.
#[allow(unsafe_code)]
pub fn current_user_sid() -> io::Result<String> {
    // SAFETY: GetCurrentProcess takes nothing and returns a pseudo-handle
    // that needs no closing.
    let me = unsafe { GetCurrentProcess() };
    token_user_sid(me)
}

/// The logon session this process runs in.
///
/// # Errors
///
/// When Windows cannot report it.
#[allow(unsafe_code)]
pub fn current_session() -> io::Result<u32> {
    // SAFETY: GetCurrentProcessId takes nothing and cannot fail.
    let pid = unsafe { GetCurrentProcessId() };
    let mut session = 0u32;
    // SAFETY: `session` is a valid u32 for the call to write.
    if unsafe { ProcessIdToSessionId(pid, &raw mut session) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(session)
}

/// Whether this process runs elevated, with an administrator's rights.
///
/// # Errors
///
/// When this process's token cannot be read.
#[allow(unsafe_code)]
pub fn is_elevated() -> io::Result<bool> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: GetCurrentProcess takes nothing and returns a pseudo-handle.
    let me = unsafe { GetCurrentProcess() };
    // SAFETY: `me` is this process; `token` is a valid place for the new
    // handle.
    if unsafe { OpenProcessToken(me, TOKEN_QUERY, &raw mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `token` is a valid token handle that nothing else owns.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut length = 0u32;
    let size = u32::try_from(size_of::<TOKEN_ELEVATION>()).map_err(io::Error::other)?;
    // SAFETY: `elevation` is a TOKEN_ELEVATION of `size` bytes, and
    // `length` a valid u32.
    let ok = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenElevation,
            (&raw mut elevation).cast(),
            size,
            &raw mut length,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(elevation.TokenIsElevated != 0)
}

#[allow(unsafe_code)]
fn token_user_sid(process: HANDLE) -> io::Result<String> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: `process` is a live process handle with at least
    // PROCESS_QUERY_LIMITED_INFORMATION, which OpenProcessToken requires,
    // and `token` is a valid place for the new handle.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &raw mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `token` is a valid token handle that nothing else owns.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };

    let mut needed = 0u32;
    // SAFETY: a null buffer of length zero only asks for the size needed,
    // which the call writes to `needed`. It fails by design.
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            ptr::null_mut(),
            0,
            &raw mut needed,
        )
    };
    // u64 elements keep the TOKEN_USER at its start aligned.
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8).max(1)];
    let length = u32::try_from(buffer.len() * 8).map_err(io::Error::other)?;
    // SAFETY: `buffer` is writable for `length` bytes and aligned for
    // TOKEN_USER; `needed` is a valid u32.
    let ok = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            length,
            &raw mut needed,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call succeeded, so the buffer starts with a TOKEN_USER,
    // and the buffer is aligned for it.
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    // The SID it points to lives in `buffer`, which outlives this call.
    sid_string(user.User.Sid)
}

#[allow(unsafe_code)]
fn sid_string(sid: PSID) -> io::Result<String> {
    let mut text: *mut u16 = ptr::null_mut();
    // SAFETY: `sid` points to a valid SID; `text` is a valid place for the
    // pointer to the string the call allocates.
    if unsafe { ConvertSidToStringSidW(sid, &raw mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call succeeded, so `text` is a NUL-terminated wide string
    // that stays valid until it is freed below.
    let wide = unsafe { nul_terminated(text) };
    let sid = String::from_utf16(wide).map_err(io::Error::other);
    // SAFETY: `text` was allocated by ConvertSidToStringSidW with LocalAlloc,
    // which LocalFree releases; it is not used after this.
    unsafe { LocalFree(text.cast()) };
    sid
}

/// The wide string at `text`, without its NUL.
///
/// # Safety
///
/// `text` points to a NUL-terminated wide string that stays valid for the
/// returned slice's life.
#[allow(unsafe_code)]
unsafe fn nul_terminated<'a>(text: *const u16) -> &'a [u16] {
    let mut length = 0;
    // SAFETY: the caller promises a NUL within the string, so every element
    // up to and including it is readable.
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: the `length` elements before the NUL are readable, and the
    // caller promises they stay valid.
    unsafe { std::slice::from_raw_parts(text, length) }
}

/// Who signed an image, as Authenticode verified it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signer {
    /// The leaf certificate's subject, as CertGetNameStringW writes it for
    /// CERT_NAME_RDN_TYPE with CERT_X500_NAME_STR and
    /// CERT_NAME_STR_REVERSE_FLAG: the X.500 parts in reverse of their
    /// encoded order, such as `CN=…, O=…, C=…`.
    pub subject: String,
}

/// The leaf signer of the image at `path`, if Authenticode verifies its
/// embedded signature and trusts the chain.
///
/// Revocation is not checked, and nothing is fetched from the network: the
/// question is which publisher built the file, asked on the owner's machine.
/// `None` for an unsigned image, a signature that does not verify, or a
/// signer that cannot be read.
#[allow(unsafe_code)]
#[must_use]
pub fn authenticode_signer(path: &Path) -> Option<Signer> {
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect();
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: u32::try_from(size_of::<WINTRUST_FILE_INFO>()).ok()?,
        pcwszFilePath: wide.as_ptr(),
        hFile: ptr::null_mut(),
        pgKnownSubject: ptr::null_mut(),
    };
    let mut data = WINTRUST_DATA {
        cbStruct: u32::try_from(size_of::<WINTRUST_DATA>()).ok()?,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 {
            pFile: &raw mut file,
        },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL,
        ..WINTRUST_DATA::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // SAFETY: `data` describes `file`, whose path `wide` is NUL-terminated,
    // and all three outlive the call; INVALID_HANDLE_VALUE means no user
    // interface.
    let status = unsafe {
        WinVerifyTrust(
            INVALID_HANDLE_VALUE,
            &raw mut action,
            (&raw mut data).cast(),
        )
    };
    let subject = if status == 0 {
        leaf_subject(data.hWVTStateData)
    } else {
        None
    };
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: the same `data`, now asking to release the state the verify
    // call kept, which it requires whatever the verify call returned.
    unsafe {
        WinVerifyTrust(
            INVALID_HANDLE_VALUE,
            &raw mut action,
            (&raw mut data).cast(),
        )
    };
    subject.map(|subject| Signer { subject })
}

#[allow(unsafe_code)]
fn leaf_subject(state: HANDLE) -> Option<String> {
    // SAFETY: `state` is the state a successful WinVerifyTrust kept, and it
    // is not yet released.
    let provider = unsafe { WTHelperProvDataFromStateData(state) };
    if provider.is_null() {
        return None;
    }
    // SAFETY: `provider` is the provider data of that state; signer 0 is
    // the primary signer, not a countersigner.
    let signer = unsafe { WTHelperGetProvSignerFromChain(provider, 0, 0, 0) };
    if signer.is_null() {
        return None;
    }
    // SAFETY: `signer` belongs to that state; certificate 0 is the leaf.
    let cert = unsafe { WTHelperGetProvCertFromChain(signer, 0) };
    if cert.is_null() {
        return None;
    }
    // SAFETY: `cert` is non-null and belongs to the state, which is still
    // held.
    let context = unsafe { (*cert).pCert };
    if context.is_null() {
        return None;
    }
    let kind: u32 = CERT_X500_NAME_STR | CERT_NAME_STR_REVERSE_FLAG;
    // SAFETY: `context` is a valid certificate; with no buffer the call
    // returns the length needed, counting the NUL. `kind` outlives it.
    let needed = unsafe {
        CertGetNameStringW(
            context,
            CERT_NAME_RDN_TYPE,
            0,
            (&raw const kind).cast(),
            ptr::null_mut(),
            0,
        )
    };
    if needed <= 1 {
        return None;
    }
    let mut buffer = vec![0u16; needed as usize];
    // SAFETY: `buffer` holds `needed` u16s.
    let written = unsafe {
        CertGetNameStringW(
            context,
            CERT_NAME_RDN_TYPE,
            0,
            (&raw const kind).cast(),
            buffer.as_mut_ptr(),
            needed,
        )
    };
    buffer.truncate((written as usize).saturating_sub(1));
    String::from_utf16(&buffer).ok()
}
