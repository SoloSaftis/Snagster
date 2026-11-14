//! Where the stream key is kept: never in settings.json.
//!
//! - Windows: a file next to the settings, encrypted for this Windows account with DPAPI
//!   (`CryptProtectData`); another account or computer can't read it.
//! - macOS: the login Keychain (service "Snagster", account "stream-key").
//! - Linux: a file only you can read (permissions 0600), scrambled so it isn't plain text. It
//!   is not encrypted: anyone who can read your files could recover it. (The Secret Service
//!   would need a D-Bus client and an unlocked keyring; this works everywhere.)

/// How the key is kept on this system, for the Settings page.
pub fn how_kept() -> &'static str {
    if cfg!(windows) {
        "The stream key is encrypted for your Windows account (not stored in the settings file)."
    } else if cfg!(target_os = "macos") {
        "The stream key is kept in your Keychain (not in the settings file)."
    } else {
        "The stream key is kept in a file only you can read, scrambled but not encrypted (not in the settings file)."
    }
}

/// The saved key, if there is one.
pub fn load() -> Option<String> {
    imp::load().filter(|k| !k.trim().is_empty())
}

/// Keep `key` (an empty key forgets the saved one).
pub fn save(key: &str) -> Result<(), String> {
    if key.trim().is_empty() {
        return forget();
    }
    imp::save(key.trim())
}

pub fn forget() -> Result<(), String> {
    imp::forget()
}

#[cfg(target_os = "macos")]
mod imp {
    use security_framework::passwords::{delete_generic_password, get_generic_password, set_generic_password};
    const SERVICE: &str = "Snagster";
    const ACCOUNT: &str = "stream-key";

    pub fn load() -> Option<String> {
        get_generic_password(SERVICE, ACCOUNT).ok().and_then(|b| String::from_utf8(b).ok())
    }

    pub fn save(key: &str) -> Result<(), String> {
        set_generic_password(SERVICE, ACCOUNT, key.as_bytes()).map_err(|e| format!("Couldn't keep the stream key in the Keychain: {e}"))
    }

    pub fn forget() -> Result<(), String> {
        match delete_generic_password(SERVICE, ACCOUNT) {
            Ok(()) => Ok(()),
            // Nothing saved (errSecItemNotFound).
            Err(e) if e.code() == -25300 => Ok(()),
            Err(e) => Err(format!("Couldn't remove the stream key from the Keychain: {e}")),
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::path::{Path, PathBuf};

    fn path() -> Option<PathBuf> {
        Some(crate::settings::settings_path()?.parent()?.join("stream-key.bin"))
    }

    pub fn load() -> Option<String> {
        load_from(&path()?)
    }

    pub fn save(key: &str) -> Result<(), String> {
        save_to(&path().ok_or("There's no folder for Snagster's settings")?, key)
    }

    pub fn forget() -> Result<(), String> {
        match path() {
            Some(p) if p.exists() => std::fs::remove_file(&p).map_err(|e| format!("Couldn't remove the stream key: {e}")),
            _ => Ok(()),
        }
    }

    pub(super) fn load_from(p: &Path) -> Option<String> {
        let data = std::fs::read(p).ok()?;
        String::from_utf8(super::unprotect(&data).ok()?).ok()
    }

    pub(super) fn save_to(p: &Path, key: &str) -> Result<(), String> {
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let data = super::protect(key.as_bytes())?;
        write_private(p, &data).map_err(|e| format!("Couldn't keep the stream key: {e}"))
    }

    #[cfg(unix)]
    fn write_private(p: &Path, data: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(p)?;
        // An older file may have been made with wider permissions.
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        f.write_all(data)
    }

    #[cfg(not(unix))]
    fn write_private(p: &Path, data: &[u8]) -> std::io::Result<()> {
        std::fs::write(p, data)
    }
}

/// DPAPI: encrypted for the Windows account that runs Snagster.
#[cfg(windows)]
fn protect(data: &[u8]) -> Result<Vec<u8>, String> {
    dpapi(data, true)
}

#[cfg(windows)]
fn unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
    dpapi(data, false)
}

#[cfg(windows)]
fn dpapi(data: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData};
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB::default();
    let r = unsafe {
        if encrypt {
            CryptProtectData(&input, windows::core::w!("Snagster stream key"), None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        } else {
            CryptUnprotectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        }
    };
    r.map_err(|e| format!("Windows couldn't {} the stream key: {e}", if encrypt { "encrypt" } else { "decrypt" }))?;
    let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
    unsafe {
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut core::ffi::c_void)));
    }
    Ok(bytes)
}

/// Not Windows (and not the Keychain): scrambled with a pad made from this account and
/// computer, behind a marker. Keeps the key out of plain sight, nothing more.
#[cfg(not(any(windows, target_os = "macos")))]
const MARK: &[u8] = b"SNAGKEY1";

#[cfg(not(any(windows, target_os = "macos")))]
fn pad(n: usize) -> Vec<u8> {
    let machine = std::fs::read_to_string("/etc/machine-id").unwrap_or_default();
    let user = std::env::var("USER").unwrap_or_default();
    // FNV-1a of who and where, then xorshift.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in format!("snagster\u{1}{}\u{1}{user}", machine.trim()).bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
    }
    let mut x = h | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

#[cfg(not(any(windows, target_os = "macos")))]
fn protect(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = MARK.to_vec();
    out.extend(data.iter().zip(pad(data.len())).map(|(a, b)| a ^ b));
    Ok(out)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
    let body = data.strip_prefix(MARK).ok_or("not a Snagster key file")?;
    Ok(body.iter().zip(pad(body.len())).map(|(a, b)| a ^ b).collect())
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn a_kept_key_comes_back_and_is_not_plain_text() {
        let dir = std::env::temp_dir().join(format!("snagster-key-test-{}", std::process::id()));
        let p = dir.join("stream-key.bin");
        let key = "live_123456789_secretKEY";
        imp::save_to(&p, key).unwrap();
        let raw = std::fs::read(&p).unwrap();
        assert!(!raw.windows(key.len()).any(|w| w == key.as_bytes()), "the key is stored as plain text");
        assert_eq!(imp::load_from(&p).as_deref(), Some(key));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // A damaged file gives no key rather than garbage.
        std::fs::write(&p, b"nonsense").unwrap();
        assert_eq!(imp::load_from(&p), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn protecting_round_trips_any_bytes() {
        for data in [&b""[..], b"k", "ключ-密钥-🔑".as_bytes()] {
            let p = protect(data).unwrap();
            assert_eq!(unprotect(&p).unwrap(), data);
        }
        assert!(unprotect(b"").is_err());
    }

    #[test]
    fn the_settings_page_says_how_the_key_is_kept() {
        assert!(how_kept().contains("not") && how_kept().contains("settings file"));
    }
}
