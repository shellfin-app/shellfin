//! Win32 security helpers (§3, §7.4.1, §5.5): the current user's SID, owner + DACL of an
//! opened handle (files and named pipes) or of a path (only for the `%ProgramData%\ssh`
//! directory chain), drive type, final path of a handle.
//!
//! The policy decision is a pure function ([`evaluate`]) over the extracted [`SecurityInfo`], so
//! the §7.4.1 table is unit-tested without crafting real ACLs. **Any API error, a NULL owner or a
//! NULL DACL ⇒ untrusted** (callers map `Err` to untrusted; never infer trust from an error).

#![allow(unsafe_code)]

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, HLOCAL, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, GetNamedSecurityInfoW, GetSecurityInfo, SE_FILE_OBJECT,
    SE_KERNEL_OBJECT, SE_OBJECT_TYPE,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
    DACL_SECURITY_INFORMATION, GetAce, GetAclInformation, GetTokenInformation, LookupAccountSidW,
    OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SID_NAME_USE, SidTypeUser, TOKEN_QUERY,
    TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetFinalPathNameByHandleW};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::Win32::System::WindowsProgramming::GetComputerNameW;

pub const SID_LOCAL_SYSTEM: &str = "S-1-5-18";
pub const SID_ADMINISTRATORS: &str = "S-1-5-32-544";
pub const SID_TRUSTED_INSTALLER: &str =
    "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";

// Access mask bits (winnt.h).
const FILE_READ_DATA: u32 = 0x0001;
const FILE_WRITE_DATA: u32 = 0x0002;
const FILE_APPEND_DATA: u32 = 0x0004;
const DELETE: u32 = 0x0001_0000;
const WRITE_DAC: u32 = 0x0004_0000;
const WRITE_OWNER: u32 = 0x0008_0000;
const GENERIC_ALL: u32 = 0x1000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const GENERIC_READ: u32 = 0x8000_0000;

/// `FILE_DELETE_CHILD`: on a directory, delete any entry (a user who may delete
/// `ssh_known_hosts` in `%ProgramData%\ssh` can wipe admin-pinned host keys).
const FILE_DELETE_CHILD: u32 = 0x0040;

/// "Write-type rights" of §7.4.1 (+ `FILE_DELETE_CHILD` for directories).
pub const WRITE_RIGHTS: u32 = FILE_WRITE_DATA
    | FILE_APPEND_DATA
    | FILE_DELETE_CHILD
    | WRITE_DAC
    | WRITE_OWNER
    | DELETE
    | GENERIC_WRITE
    | GENERIC_ALL;
/// "Read rights" of §7.4.1.
pub const READ_RIGHTS: u32 = FILE_READ_DATA | GENERIC_READ | GENERIC_ALL;

// ACE types / flags (winnt.h).
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0x0;
const ACCESS_ALLOWED_OBJECT_ACE_TYPE: u8 = 0x5;
const ACCESS_ALLOWED_CALLBACK_ACE_TYPE: u8 = 0x9;
const ACCESS_ALLOWED_CALLBACK_OBJECT_ACE_TYPE: u8 = 0xB;
const INHERIT_ONLY_ACE: u8 = 0x08;

const DRIVE_NO_ROOT_DIR: u32 = 1;
const DRIVE_REMOTE: u32 = 4;
const DRIVE_UNKNOWN: u32 = 0;

/// Owner and the effective allow-ACEs of an object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityInfo {
    /// `None` = NULL owner.
    pub owner: Option<String>,
    /// `None` = NULL DACL (everyone has full access).
    pub allow_aces: Option<Vec<Ace>>,
    /// An allow-ACE type we cannot interpret (object ACEs) was present ⇒ fail closed.
    pub uninterpretable_ace: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ace {
    pub sid: String,
    pub mask: u32,
}

/// The owner/ACE policy of one §7.4.1 cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Owner ∈ {user, SYSTEM, Administrators}; no write-type ACE for other SIDs
    /// (user config, user known_hosts, app storage dirs).
    UserWritable,
    /// As `UserWritable`, and no **read** ACE for other SIDs either (private keys).
    UserPrivate,
    /// Owner ∈ {user, SYSTEM, Administrators}, no ACE check (`.pub` files).
    UserOwnerOnly,
    /// Owner ∈ {SYSTEM, Administrators, TrustedInstaller}; no write-type ACE for any other SID
    /// (`%ProgramData%\ssh` files and every directory down to them).
    SystemOnly,
}

/// Pure §7.4.1 decision. `Err` carries an English reason for logs/detail.
pub fn evaluate(info: &SecurityInfo, policy: Policy, current_user: &str) -> Result<(), String> {
    let Some(owner) = info.owner.as_deref() else {
        return Err("object has no owner".into());
    };
    let user_like = [current_user, SID_LOCAL_SYSTEM, SID_ADMINISTRATORS];
    let system_like = [SID_LOCAL_SYSTEM, SID_ADMINISTRATORS, SID_TRUSTED_INSTALLER];
    let (trusted, forbidden): (&[&str], u32) = match policy {
        Policy::UserWritable => (&user_like, WRITE_RIGHTS),
        Policy::UserPrivate => (&user_like, WRITE_RIGHTS | READ_RIGHTS),
        Policy::UserOwnerOnly => (&user_like, 0),
        Policy::SystemOnly => (&system_like, WRITE_RIGHTS),
    };
    if !trusted.contains(&owner) {
        return Err(format!("owner {owner} is not trusted"));
    }
    if forbidden == 0 {
        return Ok(());
    }
    if info.uninterpretable_ace {
        return Err("DACL contains an allow-ACE type that cannot be evaluated".into());
    }
    let Some(aces) = info.allow_aces.as_ref() else {
        return Err("NULL DACL (everyone has full access)".into());
    };
    for ace in aces {
        if !trusted.contains(&ace.sid.as_str()) && ace.mask & forbidden != 0 {
            return Err(format!("{} has access 0x{:08x}", ace.sid, ace.mask));
        }
    }
    Ok(())
}

/// RAII for memory returned by Win32 security APIs (`LocalFree`).
struct LocalMem(PSECURITY_DESCRIPTOR);

impl Drop for LocalMem {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer was allocated by the security API with LocalAlloc.
            unsafe { LocalFree(self.0 as HLOCAL) };
        }
    }
}

fn sid_to_string(sid: PSID) -> Option<String> {
    if sid.is_null() {
        return None;
    }
    let mut out: *mut u16 = std::ptr::null_mut();
    // SAFETY: `sid` comes from a security descriptor/token buffer that is alive; `out` receives
    // a LocalAlloc'ed NUL-terminated string that we free below.
    let ok = unsafe { ConvertSidToStringSidW(sid, &mut out) };
    if ok == 0 || out.is_null() {
        return None;
    }
    let _guard = LocalMem(out.cast());
    let mut len = 0usize;
    // SAFETY: `out` is NUL-terminated (API contract).
    while unsafe { *out.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` u16s are readable.
    let wide = unsafe { std::slice::from_raw_parts(out, len) };
    Some(String::from_utf16_lossy(wide))
}

/// How [`read_dacl`] treats an ACE type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AceClass {
    /// `ACCESS_ALLOWED` / `ACCESS_ALLOWED_CALLBACK`: header, mask, SID — evaluated.
    Allow,
    /// May grant access but cannot be evaluated here ⇒ the check fails closed.
    Uninterpretable,
    /// Can only deny, audit or label; never grants access.
    NonGranting,
}

/// An allowlist of the ACE types that never grant access; every other type, including
/// any type a future Windows adds, fails closed (it was silently ignored before).
fn classify_ace(ace_type: u8) -> AceClass {
    match ace_type {
        ACCESS_ALLOWED_ACE_TYPE | ACCESS_ALLOWED_CALLBACK_ACE_TYPE => AceClass::Allow,
        // ACCESS_DENIED, _OBJECT, _CALLBACK, _CALLBACK_OBJECT.
        0x01 | 0x06 | 0x0A | 0x0C => AceClass::NonGranting,
        // SYSTEM_AUDIT / SYSTEM_ALARM and their object/callback variants.
        0x02 | 0x03 | 0x07 | 0x08 | 0x0D | 0x0E | 0x0F | 0x10 => AceClass::NonGranting,
        // SYSTEM_MANDATORY_LABEL, _RESOURCE_ATTRIBUTE, _SCOPED_POLICY_ID, _PROCESS_TRUST_LABEL,
        // _ACCESS_FILTER: restrict or annotate, never grant.
        0x11..=0x15 => AceClass::NonGranting,
        ACCESS_ALLOWED_OBJECT_ACE_TYPE | ACCESS_ALLOWED_CALLBACK_OBJECT_ACE_TYPE => {
            AceClass::Uninterpretable
        }
        // ACCESS_ALLOWED_COMPOUND (0x04) and unknown types.
        _ => AceClass::Uninterpretable,
    }
}

/// Collect allow-ACEs that are not INHERIT_ONLY.
fn read_dacl(acl: *mut ACL) -> (Option<Vec<Ace>>, bool) {
    if acl.is_null() {
        return (None, false);
    }
    let mut size = ACL_SIZE_INFORMATION {
        AceCount: 0,
        AclBytesInUse: 0,
        AclBytesFree: 0,
    };
    // SAFETY: `acl` is a valid ACL inside a live security descriptor; `size` is correctly sized.
    let ok = unsafe {
        GetAclInformation(
            acl,
            (&raw mut size).cast(),
            u32::try_from(size_of::<ACL_SIZE_INFORMATION>()).unwrap_or(u32::MAX),
            AclSizeInformation,
        )
    };
    if ok == 0 {
        // Unreadable ACL ⇒ fail closed (treated like a NULL DACL by `evaluate`).
        return (None, true);
    }
    let mut aces = Vec::new();
    let mut uninterpretable = false;
    for i in 0..size.AceCount {
        let mut ace: *mut core::ffi::c_void = std::ptr::null_mut();
        // SAFETY: index < AceCount.
        if unsafe { GetAce(acl, i, &mut ace) } == 0 || ace.is_null() {
            return (None, true);
        }
        // SAFETY: every ACE starts with an ACE_HEADER.
        let header: ACE_HEADER = unsafe { std::ptr::read_unaligned(ace.cast::<ACE_HEADER>()) };
        if header.AceFlags & INHERIT_ONLY_ACE != 0 {
            continue;
        }
        match classify_ace(header.AceType) {
            AceClass::Allow => {
                // SAFETY: both types share the ACCESS_ALLOWED_ACE layout (header, mask, SID).
                let typed: ACCESS_ALLOWED_ACE =
                    unsafe { std::ptr::read_unaligned(ace.cast::<ACCESS_ALLOWED_ACE>()) };
                // SAFETY: the SID starts at `SidStart` inside the ACE.
                let sid_ptr = unsafe {
                    ace.cast::<u8>()
                        .add(std::mem::offset_of!(ACCESS_ALLOWED_ACE, SidStart))
                };
                match sid_to_string(sid_ptr.cast()) {
                    Some(sid) => aces.push(Ace {
                        sid,
                        mask: typed.Mask,
                    }),
                    None => uninterpretable = true,
                }
            }
            AceClass::Uninterpretable => uninterpretable = true,
            AceClass::NonGranting => {}
        }
    }
    (Some(aces), uninterpretable)
}

fn collect(owner: PSID, dacl: *mut ACL, sd: PSECURITY_DESCRIPTOR) -> SecurityInfo {
    let _guard = LocalMem(sd);
    let (allow_aces, uninterpretable_ace) = read_dacl(dacl);
    SecurityInfo {
        owner: sid_to_string(owner),
        allow_aces,
        uninterpretable_ace,
    }
}

fn security_of_raw_handle(handle: HANDLE, object: SE_OBJECT_TYPE) -> io::Result<SecurityInfo> {
    let mut owner: PSID = std::ptr::null_mut();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: out-pointers are valid; `handle` is a live handle owned by the caller.
    let rc = unsafe {
        GetSecurityInfo(
            handle,
            object,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut sd,
        )
    };
    if rc != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(
            i32::try_from(rc).unwrap_or(i32::MAX),
        ));
    }
    Ok(collect(owner, dacl, sd))
}

/// Owner + DACL of an opened file (`SE_FILE_OBJECT`). Checks the object actually opened (no TOCTOU).
pub fn security_of_file(file: &impl AsRawHandle) -> io::Result<SecurityInfo> {
    security_of_raw_handle(file.as_raw_handle(), SE_FILE_OBJECT)
}

/// Owner + DACL of an opened kernel object such as a named-pipe client handle (`SE_KERNEL_OBJECT`, §5.5).
pub fn security_of_kernel_object(handle: &impl AsRawHandle) -> io::Result<SecurityInfo> {
    security_of_raw_handle(handle.as_raw_handle(), SE_KERNEL_OBJECT)
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Owner + DACL by path (`GetNamedSecurityInfoW`). Used only for the `%ProgramData%\ssh`
/// directory chain, after the path allowlist.
pub fn security_of_path(path: &Path) -> io::Result<SecurityInfo> {
    let name = wide(path);
    let mut owner: PSID = std::ptr::null_mut();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: `name` is NUL-terminated; out-pointers are valid.
    let rc = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut sd,
        )
    };
    if rc != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(
            i32::try_from(rc).unwrap_or(i32::MAX),
        ));
    }
    Ok(collect(owner, dacl, sd))
}

fn query_current_user_sid() -> io::Result<String> {
    with_current_user_sid(|sid| {
        sid_to_string(sid).ok_or_else(|| io::Error::other("cannot convert the user SID"))
    })
}

/// Run `f` with the process token's user SID (valid only during the call).
fn with_current_user_sid<R>(f: impl FnOnce(PSID) -> io::Result<R>) -> io::Result<R> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: GetCurrentProcess returns a pseudo-handle; `token` receives a real handle closed below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    struct Token(HANDLE);
    impl Drop for Token {
        fn drop(&mut self) {
            // SAFETY: handle from OpenProcessToken.
            unsafe { CloseHandle(self.0) };
        }
    }
    let token = Token(token);
    let mut needed = 0u32;
    // SAFETY: size query with a null buffer.
    unsafe { GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut needed) };
    if needed == 0 {
        return Err(io::Error::last_os_error());
    }
    // u64 storage keeps the TOKEN_USER struct suitably aligned.
    let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: `buf` holds at least `needed` bytes.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buf.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the buffer now starts with a TOKEN_USER whose SID points into `buf`.
    let user: &TOKEN_USER = unsafe { &*buf.as_ptr().cast::<TOKEN_USER>() };
    f(user.User.Sid)
}

/// The current process user's account as `(user name, domain name)` from
/// `LookupAccountSidW(NULL, <token user SID>)`, and the NetBIOS computer name
/// (`GetComputerNameW`): exactly what Win32-OpenSSH's `get_passwd` builds `pw_name` from
/// (`platform::openssh_windows_user`). `None` on any error or if the SID is not a user account.
pub fn current_account() -> Option<(String, String, String)> {
    let (user, domain) = with_current_user_sid(|sid| {
        let mut name = [0u16; 257]; // UNLEN + 1
        let mut domain = [0u16; 256]; // DNLEN + 1 is 16; leave room for longer names
        let mut name_len = u32::try_from(name.len()).unwrap_or(0);
        let mut domain_len = u32::try_from(domain.len()).unwrap_or(0);
        let mut kind: SID_NAME_USE = 0;
        // SAFETY: `sid` is valid for the duration of this closure; both buffers hold the given
        // number of UTF-16 units and outlive the call; on success the lengths are the units
        // written without the terminating NUL.
        let ok = unsafe {
            LookupAccountSidW(
                std::ptr::null(),
                sid,
                name.as_mut_ptr(),
                &mut name_len,
                domain.as_mut_ptr(),
                &mut domain_len,
                &mut kind,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        if kind != SidTypeUser {
            return Err(io::Error::other("not a user account"));
        }
        let take = |buf: &[u16], len: u32| -> io::Result<String> {
            let units = usize::try_from(len)
                .ok()
                .and_then(|n| buf.get(..n))
                .ok_or_else(|| io::Error::other("bad length"))?;
            String::from_utf16(units).map_err(|_| io::Error::other("not UTF-16"))
        };
        Ok((take(&name, name_len)?, take(&domain, domain_len)?))
    })
    .ok()?;
    let mut computer = [0u16; 64]; // MAX_COMPUTERNAME_LENGTH (15) + 1, with room
    let mut len = u32::try_from(computer.len()).ok()?;
    // SAFETY: `computer` holds `len` UTF-16 units and outlives the call; on success `len` is
    // the number of units written without the terminating NUL.
    if unsafe { GetComputerNameW(computer.as_mut_ptr(), &mut len) } == 0 {
        return None;
    }
    let units = computer.get(..usize::try_from(len).ok()?)?;
    Some((user, domain, String::from_utf16(units).ok()?))
}

/// The current process user's SID string (cached).
pub fn current_user_sid() -> io::Result<String> {
    static SID: OnceLock<Result<String, String>> = OnceLock::new();
    SID.get_or_init(|| query_current_user_sid().map_err(|e| e.to_string()))
        .clone()
        .map_err(io::Error::other)
}

/// `true` if `GetDriveTypeW("<letter>:\")` is a local drive type (not remote, not missing, not unknown).
pub fn drive_is_local(letter: u8) -> bool {
    let root = [u16::from(letter), u16::from(b':'), u16::from(b'\\'), 0];
    // SAFETY: NUL-terminated buffer.
    let kind = unsafe { GetDriveTypeW(root.as_ptr()) };
    !matches!(kind, DRIVE_REMOTE | DRIVE_NO_ROOT_DIR | DRIVE_UNKNOWN)
}

/// `GetFinalPathNameByHandleW` (e.g. `\\?\C:\Users\…`).
pub fn final_path(file: &impl AsRawHandle) -> io::Result<PathBuf> {
    let mut buf = vec![0u16; 512];
    loop {
        let cap = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        // SAFETY: `buf` has `cap` u16s; flags 0 = FILE_NAME_NORMALIZED | VOLUME_NAME_DOS.
        let n =
            unsafe { GetFinalPathNameByHandleW(file.as_raw_handle(), buf.as_mut_ptr(), cap, 0) };
        if n == 0 {
            return Err(io::Error::last_os_error());
        }
        let n = n as usize;
        if n < buf.len() {
            buf.truncate(n);
            return Ok(PathBuf::from(OsString::from_wide(&buf)));
        }
        buf.resize(n + 1, 0);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// Only the known non-granting ACE types are skipped; every other type fails closed.
    #[test]
    fn unknown_ace_types_fail_closed() {
        assert_eq!(classify_ace(0x00), AceClass::Allow);
        assert_eq!(classify_ace(0x09), AceClass::Allow);
        for t in [
            0x01, 0x02, 0x03, 0x06, 0x07, 0x08, 0x0A, 0x0C, 0x0D, 0x10, 0x11, 0x15,
        ] {
            assert_eq!(classify_ace(t), AceClass::NonGranting, "{t:#x}");
        }
        for t in [0x04, 0x05, 0x0B, 0x16, 0x7F, 0xFF] {
            assert_eq!(classify_ace(t), AceClass::Uninterpretable, "{t:#x}");
        }
    }

    const ME: &str = "S-1-5-21-1-2-3-1001";
    const OTHER: &str = "S-1-5-21-1-2-3-1002";
    const USERS: &str = "S-1-5-32-545";
    const AUTH_USERS: &str = "S-1-5-11";

    fn info(owner: Option<&str>, aces: Option<&[(&str, u32)]>) -> SecurityInfo {
        SecurityInfo {
            owner: owner.map(str::to_owned),
            allow_aces: aces.map(|a| {
                a.iter()
                    .map(|(s, m)| Ace {
                        sid: (*s).to_owned(),
                        mask: *m,
                    })
                    .collect()
            }),
            uninterpretable_ace: false,
        }
    }

    const FULL: u32 = 0x001f_01ff;
    const READ_EXEC: u32 = 0x0012_00a9;

    #[test]
    fn user_writable_row() {
        let ok = info(
            Some(ME),
            Some(&[
                (ME, FULL),
                (SID_LOCAL_SYSTEM, FULL),
                (SID_ADMINISTRATORS, FULL),
            ]),
        );
        assert!(evaluate(&ok, Policy::UserWritable, ME).is_ok());
        // Read for others is fine for configs.
        let read = info(Some(ME), Some(&[(ME, FULL), (USERS, READ_EXEC)]));
        assert!(evaluate(&read, Policy::UserWritable, ME).is_ok());
        // Foreign owner, foreign writer, NULL owner, NULL DACL ⇒ untrusted.
        assert!(evaluate(&info(Some(OTHER), Some(&[])), Policy::UserWritable, ME).is_err());
        assert!(
            evaluate(
                &info(Some(ME), Some(&[(AUTH_USERS, 0x0013_01bf)])),
                Policy::UserWritable,
                ME
            )
            .is_err()
        );
        assert!(
            evaluate(
                &info(Some(ME), Some(&[(USERS, FILE_APPEND_DATA)])),
                Policy::UserWritable,
                ME
            )
            .is_err()
        );
        assert!(
            evaluate(
                &info(Some(ME), Some(&[(OTHER, WRITE_DAC)])),
                Policy::UserWritable,
                ME
            )
            .is_err()
        );
        assert!(evaluate(&info(None, Some(&[])), Policy::UserWritable, ME).is_err());
        assert!(evaluate(&info(Some(ME), None), Policy::UserWritable, ME).is_err());
    }

    #[test]
    fn private_key_row_rejects_foreign_read() {
        let ok = info(Some(ME), Some(&[(ME, FULL), (SID_LOCAL_SYSTEM, FULL)]));
        assert!(evaluate(&ok, Policy::UserPrivate, ME).is_ok());
        let read = info(Some(ME), Some(&[(ME, FULL), (USERS, READ_EXEC)]));
        assert!(evaluate(&read, Policy::UserPrivate, ME).is_err());
        let generic = info(Some(ME), Some(&[(OTHER, GENERIC_READ)]));
        assert!(evaluate(&generic, Policy::UserPrivate, ME).is_err());
    }

    #[test]
    fn pub_key_row_checks_owner_only() {
        assert!(evaluate(&info(Some(ME), None), Policy::UserOwnerOnly, ME).is_ok());
        assert!(evaluate(&info(Some(OTHER), Some(&[])), Policy::UserOwnerOnly, ME).is_err());
    }

    #[test]
    fn system_row() {
        let ok = info(
            Some(SID_ADMINISTRATORS),
            Some(&[
                (SID_LOCAL_SYSTEM, FULL),
                (SID_ADMINISTRATORS, FULL),
                (USERS, READ_EXEC),
            ]),
        );
        assert!(evaluate(&ok, Policy::SystemOnly, ME).is_ok());
        let ti = info(Some(SID_TRUSTED_INSTALLER), Some(&[]));
        assert!(evaluate(&ti, Policy::SystemOnly, ME).is_ok());
        // A directory planted by a non-admin (owner = that user) is rejected, even if it is us.
        assert!(evaluate(&info(Some(ME), Some(&[])), Policy::SystemOnly, ME).is_err());
        // ProgramData's default "Users: create files/folders" (0x116) must not appear on the chain.
        let planted = info(Some(SID_ADMINISTRATORS), Some(&[(USERS, 0x116)]));
        assert!(evaluate(&planted, Policy::SystemOnly, ME).is_err());
        // Read/execute + delete-child (0x1200e9 = 0x1200a9 | 0x40) is a write right.
        let delete_child = info(Some(SID_ADMINISTRATORS), Some(&[(USERS, 0x0012_00e9)]));
        assert!(evaluate(&delete_child, Policy::SystemOnly, ME).is_err());
        let read_exec = info(Some(SID_ADMINISTRATORS), Some(&[(USERS, 0x0012_00a9)]));
        assert!(evaluate(&read_exec, Policy::SystemOnly, ME).is_ok());
    }

    #[test]
    fn uninterpretable_ace_fails_closed() {
        let mut i = info(Some(ME), Some(&[]));
        i.uninterpretable_ace = true;
        assert!(evaluate(&i, Policy::UserWritable, ME).is_err());
        assert!(evaluate(&i, Policy::UserOwnerOnly, ME).is_ok()); // owner-only row ignores the DACL
    }

    #[test]
    fn live_apis_on_a_temp_file() {
        let me = current_user_sid().unwrap();
        assert!(me.starts_with("S-1-5-"), "{me}");
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, b"x").unwrap();
        let f = std::fs::File::open(&p).unwrap();
        let sec = security_of_file(&f).unwrap();
        assert!(sec.owner.is_some());
        assert!(sec.allow_aces.is_some());
        let fp = final_path(&f).unwrap();
        assert!(
            fp.to_string_lossy().starts_with(r"\\?\"),
            "{}",
            fp.display()
        );
        let by_path = security_of_path(&p).unwrap();
        assert_eq!(by_path.owner, sec.owner);
        let drive = fp.to_string_lossy().as_bytes()[4];
        assert!(drive_is_local(drive));
    }
}
