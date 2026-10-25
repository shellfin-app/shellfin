//! The OS name and version for error reports (§9.6). Read without child processes or new crates:
//! Windows from the registry (`CurrentVersion`), macOS from `sysctl kern.osproductversion`,
//! Linux from `/etc/os-release` plus `uname`. Every value is reduced to a short, printable ASCII
//! text; when nothing can be read the OS family alone is returned.

/// Longest OS description returned.
pub const MAX_OS_CHARS: usize = 100;

/// "Windows 11 24H2 (build 26100.2033)", "macOS 15.1", "Ubuntu 24.04.1 LTS (Linux 6.8.0)".
pub fn os_description() -> String {
    clean(&describe(), MAX_OS_CHARS)
}

/// The local computer name (to redact it from error reports): Windows `%COMPUTERNAME%`, Unix
/// `gethostname(2)` (the `HOSTNAME` variable is a shell variable, usually not exported).
pub fn computer_name() -> Option<String> {
    #[cfg(windows)]
    let name = std::env::var("COMPUTERNAME").ok();
    #[cfg(unix)]
    let name = {
        let mut buf = [0u8; 256];
        // SAFETY: `buf` is writable for its length; on success the name is NUL-terminated
        // within it (truncated names may lack the NUL: the length is bounded below).
        let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
        (rc == 0).then(|| {
            let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            String::from_utf8_lossy(buf.get(..len).unwrap_or_default()).into_owned()
        })
    };
    name.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty())
}

/// Printable ASCII only (anything else becomes `?`), whitespace collapsed, at most `max` chars.
pub fn clean(s: &str, max: usize) -> String {
    let mut out = String::new();
    for word in s.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.extend(
            word.chars()
                .map(|c| if c.is_ascii_graphic() { c } else { '?' }),
        );
    }
    if out.chars().count() > max {
        out = out.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
    }
    out
}

/// The Windows marketing name for a build number (Windows 11 kept the 10.0 version).
pub fn windows_name(build: u32) -> &'static str {
    if build >= 22_000 {
        "Windows 11"
    } else {
        "Windows 10"
    }
}

/// `PRETTY_NAME` (else `NAME` + `VERSION_ID`) of an os-release file.
pub fn parse_os_release(text: &str) -> Option<String> {
    let value = |key: &str| {
        text.lines().find_map(|line| {
            let rest = line.trim().strip_prefix(key)?.strip_prefix('=')?;
            let v = rest.trim().trim_matches(|c| c == '"' || c == '\'').trim();
            (!v.is_empty()).then(|| v.to_owned())
        })
    };
    value("PRETTY_NAME").or_else(|| {
        let name = value("NAME")?;
        Some(match value("VERSION_ID") {
            Some(v) => format!("{name} {v}"),
            None => name,
        })
    })
}

#[cfg(windows)]
fn describe() -> String {
    let build = registry_string("CurrentBuildNumber").and_then(|b| b.parse::<u32>().ok());
    let Some(build) = build else {
        return "Windows".to_owned();
    };
    let mut out = windows_name(build).to_owned();
    if let Some(display) = registry_string("DisplayVersion") {
        out.push(' ');
        out.push_str(&display);
    }
    match registry_dword("UBR") {
        Some(ubr) => out.push_str(&format!(" (build {build}.{ubr})")),
        None => out.push_str(&format!(" (build {build})")),
    }
    out
}

#[cfg(windows)]
const CURRENT_VERSION_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn registry_string(name: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY, RegGetValueW,
    };
    let key = wide(CURRENT_VERSION_KEY);
    let value = wide(name);
    let mut buf = [0u16; 64];
    let mut bytes = u32::try_from(std::mem::size_of_val(&buf)).ok()?;
    // SAFETY: both names are NUL-terminated UTF-16; `buf` is writable for `bytes` bytes and
    // RegGetValueW NUL-terminates REG_SZ data within that size (or fails with ERROR_MORE_DATA).
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if rc != 0 {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let text = String::from_utf16_lossy(buf.get(..len)?);
    let text = clean(&text, 32);
    (!text.is_empty()).then_some(text)
}

#[cfg(windows)]
fn registry_dword(name: &str) -> Option<u32> {
    use windows_sys::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_SUBKEY_WOW6464KEY, RegGetValueW,
    };
    let key = wide(CURRENT_VERSION_KEY);
    let value = wide(name);
    let mut data: u32 = 0;
    let mut bytes = u32::try_from(std::mem::size_of::<u32>()).ok()?;
    // SAFETY: NUL-terminated names; `data` is a writable u32 and `bytes` its size.
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD | RRF_SUBKEY_WOW6464KEY,
            std::ptr::null_mut(),
            (&raw mut data).cast(),
            &mut bytes,
        )
    };
    (rc == 0).then_some(data)
}

#[cfg(target_os = "macos")]
fn describe() -> String {
    let mut buf = [0u8; 64];
    let mut len: libc::size_t = buf.len();
    // SAFETY: the name is NUL-terminated; `buf` is writable for `len` bytes and the kernel
    // writes at most `len` bytes and updates `len`.
    let rc = unsafe {
        libc::sysctlbyname(
            c"kern.osproductversion".as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    let version = (rc == 0)
        .then(|| buf.get(..len.min(buf.len())))
        .flatten()
        .map(|b| {
            String::from_utf8_lossy(b)
                .trim_end_matches('\0')
                .trim()
                .to_owned()
        })
        .filter(|v| !v.is_empty());
    match version {
        Some(v) => format!("macOS {v}"),
        None => "macOS".to_owned(),
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn describe() -> String {
    use std::io::Read;
    // A fixed system file, bounded; never a path from the WebView.
    let release = ["/etc/os-release", "/usr/lib/os-release"]
        .iter()
        .find_map(|path| {
            let mut text = String::new();
            std::fs::File::open(path)
                .ok()?
                .take(16 * 1024)
                .read_to_string(&mut text)
                .ok()?;
            parse_os_release(&text)
        });
    let kernel = kernel_release();
    match (release, kernel) {
        (Some(r), Some(k)) => format!("{} (Linux {k})", clean(&r, 60)),
        (Some(r), None) => r,
        (None, Some(k)) => format!("Linux {k}"),
        (None, None) => "Linux".to_owned(),
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn kernel_release() -> Option<String> {
    // SAFETY: a zeroed utsname is a valid out-parameter; `release` is NUL-terminated on success.
    let release = unsafe {
        let mut uts: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut uts) != 0 {
            return None;
        }
        std::ffi::CStr::from_ptr(uts.release.as_ptr())
            .to_string_lossy()
            .into_owned()
    };
    let release = clean(&release, 40);
    (!release.is_empty()).then_some(release)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn description_is_short_printable_ascii() {
        let d = os_description();
        assert!(!d.is_empty());
        assert!(d.chars().count() <= MAX_OS_CHARS);
        assert!(
            d.chars()
                .all(|c| c.is_ascii_graphic() || c == ' ' || c == '…'),
            "{d}"
        );
        let family = if cfg!(windows) {
            "Windows"
        } else if cfg!(target_os = "macos") {
            "macOS"
        } else {
            ""
        };
        assert!(d.starts_with(family), "{d}");
    }

    #[test]
    fn clean_replaces_controls_and_bounds_length() {
        assert_eq!(
            clean("Ubuntu\u{1b}[2J 24.04\u{202E}\n LTS", 100),
            "Ubuntu?[2J 24.04? LTS"
        );
        assert_eq!(clean("Привіт 1", 100), "?????? 1");
        let long = clean(&"x".repeat(500), 10);
        assert_eq!(long.chars().count(), 10);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn computer_name_is_read_from_the_system() {
        // Every CI and developer machine has one; it is never printed here.
        let name = computer_name().unwrap();
        assert!(!name.is_empty() && name.trim() == name && !name.contains('\0'));
    }

    #[test]
    fn windows_11_starts_at_build_22000() {
        assert_eq!(windows_name(19_045), "Windows 10");
        assert_eq!(windows_name(22_000), "Windows 11");
        assert_eq!(windows_name(26_100), "Windows 11");
    }

    #[test]
    fn os_release_parsing() {
        let text = "NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\n";
        assert_eq!(parse_os_release(text).unwrap(), "Ubuntu 24.04.1 LTS");
        assert_eq!(
            parse_os_release("NAME=Fedora Linux\nVERSION_ID=41\n").unwrap(),
            "Fedora Linux 41"
        );
        assert_eq!(parse_os_release("NAME='Arch'\n").unwrap(), "Arch");
        assert_eq!(parse_os_release("PRETTY_NAME=\"\"\nID=x\n"), None);
        assert_eq!(parse_os_release(""), None);
        // Prefix matches only whole keys.
        assert_eq!(parse_os_release("NAMEX=1\n"), None);
    }
}
