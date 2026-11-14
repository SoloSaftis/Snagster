//! The person's own folders: home, where programs keep their settings, Pictures and Videos.
//!
//! - Windows: the known folders (`SHGetKnownFolderPath`), with `%APPDATA%` / `%USERPROFILE%`
//!   when that fails.
//! - macOS: `$HOME`, `~/Library/Application Support`, `~/Pictures`, `~/Movies`.
//! - Linux and other Unix: `$HOME`, `$XDG_CONFIG_HOME` (else `~/.config`), and Pictures and
//!   Videos from `$XDG_PICTURES_DIR` / `$XDG_VIDEOS_DIR` or `user-dirs.dirs` (what
//!   `xdg-user-dirs-update` writes), else `~/Pictures` and `~/Videos`.
//!
//! Snagster's own folder (`snagster_dir`) moved on Linux (to honour `XDG_CONFIG_HOME`) and macOS
//! (to Application Support) in 0.10: the first call copies what an older Snagster kept in
//! `~/.config/Snagster`, so no setting is lost.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The system the folder rules are for (tests check every system on any computer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Os {
    Windows,
    Mac,
    Unix,
}

const OS: Os = if cfg!(windows) {
    Os::Windows
} else if cfg!(target_os = "macos") {
    Os::Mac
} else {
    Os::Unix
};

fn env(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|v| !v.is_empty())
}

/// An absolute path from an environment variable (relative ones are ignored, as the XDG rules say).
fn abs(v: Option<OsString>) -> Option<PathBuf> {
    v.map(PathBuf::from).filter(|p| p.is_absolute())
}

/// The person's home folder.
pub fn home() -> Option<PathBuf> {
    #[cfg(windows)]
    if let Some(p) = known::profile() {
        return Some(p);
    }
    home_from(OS, &env)
}

fn home_from(os: Os, env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    match os {
        Os::Windows => env("USERPROFILE").or_else(|| env("HOME")).map(PathBuf::from),
        _ => env("HOME").map(PathBuf::from),
    }
}

/// Where programs keep their settings: `%APPDATA%`, `~/Library/Application Support`, or
/// `$XDG_CONFIG_HOME` (else `~/.config`).
pub fn config_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    if let Some(p) = known::roaming_app_data() {
        return Some(p);
    }
    config_from(OS, &env)
}

fn config_from(os: Os, env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    match os {
        Os::Windows => env("APPDATA").map(PathBuf::from).or_else(|| Some(home_from(os, env)?.join("AppData").join("Roaming"))),
        Os::Mac => Some(home_from(os, env)?.join("Library").join("Application Support")),
        Os::Unix => abs(env("XDG_CONFIG_HOME")).or_else(|| Some(home_from(os, env)?.join(".config"))),
    }
}

/// Where Snagster 0.9 and older kept its folder on Linux and macOS (`~/.config/Snagster`).
fn legacy_snagster_dir(os: Os, env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    match os {
        Os::Windows => None,
        _ => Some(home_from(os, env)?.join(".config").join("Snagster")),
    }
}

/// Snagster's own folder (settings.json, crash.log, the stream key file, the font cache).
/// The first call in a run brings over the folder an older Snagster used.
pub fn snagster_dir() -> Option<PathBuf> {
    static DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let dir = config_dir()?.join("Snagster");
        if let Some(old) = legacy_snagster_dir(OS, &env)
            && let Err(e) = migrate(&old, &dir)
        {
            eprintln!("Snagster: couldn't bring the settings over from {}: {e}", old.display());
        }
        Some(dir)
    })
    .clone()
}

/// Copy an older Snagster folder `old` into `new` when `new` has no settings yet and `old` has.
/// Everything in it comes along (settings.json, crash.log, stream-key.bin, fonts-cache.json and
/// anything else); `old` is left as it was, so nothing can be lost half way. Returns whether
/// it copied.
pub fn migrate(old: &Path, new: &Path) -> Result<bool, String> {
    if same_place(old, new) || new.join("settings.json").exists() || !old.join("settings.json").is_file() {
        return Ok(false);
    }
    copy_tree(old, new)?;
    Ok(true)
}

fn same_place(a: &Path, b: &Path) -> bool {
    a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
}

/// Copy a folder with what is in it; files already in `to` are kept, except settings.json,
/// which is what the copy is for. File permissions come along (the stream key file is private).
fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_dir() {
            copy_tree(&src, &dst)?;
        } else if kind.is_file() && (!dst.exists() || entry.file_name() == "settings.json") {
            std::fs::copy(&src, &dst).map_err(|e| format!("{}: {e}", src.display()))?;
        }
    }
    Ok(())
}

/// Which of the person's folders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Folder {
    Pictures,
    Videos,
}

impl Folder {
    /// The `user-dirs.dirs` name.
    fn xdg_key(self) -> &'static str {
        match self {
            Folder::Pictures => "XDG_PICTURES_DIR",
            Folder::Videos => "XDG_VIDEOS_DIR",
        }
    }

    /// The usual folder name in the home folder.
    fn fallback(self, os: Os) -> &'static str {
        match (self, os) {
            (Folder::Pictures, _) => "Pictures",
            (Folder::Videos, Os::Mac) => "Movies",
            (Folder::Videos, _) => "Videos",
        }
    }
}

/// The person's Pictures folder.
pub fn pictures() -> Option<PathBuf> {
    #[cfg(windows)]
    if let Some(p) = known::pictures() {
        return Some(p);
    }
    folder(OS, Folder::Pictures, &env, &read_user_dirs)
}

/// The person's Videos folder (Movies on macOS).
pub fn videos() -> Option<PathBuf> {
    #[cfg(windows)]
    if let Some(p) = known::videos() {
        return Some(p);
    }
    folder(OS, Folder::Videos, &env, &read_user_dirs)
}

/// `user-dirs.dirs` in a config folder, if it is there.
fn read_user_dirs(config: &Path) -> Option<String> {
    std::fs::read_to_string(config.join("user-dirs.dirs")).ok()
}

fn folder(os: Os, which: Folder, env: &dyn Fn(&str) -> Option<OsString>, user_dirs: &dyn Fn(&Path) -> Option<String>) -> Option<PathBuf> {
    let home = home_from(os, env)?;
    if os == Os::Unix {
        if let Some(p) = abs(env(which.xdg_key())) {
            return Some(p);
        }
        if let Some(text) = config_from(os, env).and_then(|c| user_dirs(&c))
            && let Some(p) = parse_user_dirs(&text, &home, which.xdg_key())
        {
            return Some(p);
        }
    }
    Some(home.join(which.fallback(os)))
}

/// The folder `key` names in a `user-dirs.dirs` file: lines such as
/// `XDG_PICTURES_DIR="$HOME/Pictures"` (a path that starts with `$HOME`, or an absolute one).
/// `"$HOME/"` alone means the home folder itself, which the XDG rules treat as "not set".
fn parse_user_dirs(text: &str, home: &Path, key: &str) -> Option<PathBuf> {
    let mut found = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        if k.trim() != key {
            continue;
        }
        let v = v.trim();
        let v = v.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(v);
        let v = v.replace("\\\"", "\"").replace("\\\\", "\\");
        // The last line for a key wins, as when the shell reads the file.
        found = if let Some(rest) = v.strip_prefix("$HOME").or_else(|| v.strip_prefix("${HOME}")) {
            let rest = rest.trim_start_matches('/');
            if rest.is_empty() { None } else { Some(home.join(rest)) }
        } else if v.starts_with('/') {
            Some(PathBuf::from(v))
        } else {
            None
        };
    }
    found
}

/// Windows known folders.
#[cfg(windows)]
mod known {
    use std::path::PathBuf;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Pictures, FOLDERID_Profile, FOLDERID_RoamingAppData, FOLDERID_Videos, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath};
    use windows::core::GUID;

    fn get(id: &GUID) -> Option<PathBuf> {
        unsafe {
            let p = SHGetKnownFolderPath(id, KNOWN_FOLDER_FLAG(0), None).ok()?;
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const _));
            s.filter(|s| !s.is_empty()).map(PathBuf::from)
        }
    }

    pub fn profile() -> Option<PathBuf> {
        get(&FOLDERID_Profile)
    }

    pub fn roaming_app_data() -> Option<PathBuf> {
        get(&FOLDERID_RoamingAppData)
    }

    pub fn pictures() -> Option<PathBuf> {
        get(&FOLDERID_Pictures)
    }

    pub fn videos() -> Option<PathBuf> {
        get(&FOLDERID_Videos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let m: HashMap<String, OsString> = pairs.iter().map(|(k, v)| (k.to_string(), OsString::from(v))).collect();
        move |k: &str| m.get(k).cloned().filter(|v| !v.is_empty())
    }

    fn no_file(_: &Path) -> Option<String> {
        None
    }

    #[test]
    fn home_comes_from_userprofile_on_windows_and_home_elsewhere() {
        let e = vars(&[("USERPROFILE", r"C:\Users\Ann"), ("HOME", "/home/ann")]);
        assert_eq!(home_from(Os::Windows, &e), Some(PathBuf::from(r"C:\Users\Ann")));
        assert_eq!(home_from(Os::Unix, &e), Some(PathBuf::from("/home/ann")));
        assert_eq!(home_from(Os::Mac, &e), Some(PathBuf::from("/home/ann")));
        assert_eq!(home_from(Os::Windows, &vars(&[("HOME", "h")])), Some(PathBuf::from("h")));
        assert_eq!(home_from(Os::Unix, &vars(&[])), None);
        assert_eq!(home_from(Os::Unix, &vars(&[("HOME", "")])), None, "an empty HOME is no home");
    }

    #[test]
    fn config_folder_per_system() {
        let e = vars(&[("APPDATA", r"C:\Users\Ann\AppData\Roaming"), ("HOME", "/home/ann")]);
        assert_eq!(config_from(Os::Windows, &e), Some(PathBuf::from(r"C:\Users\Ann\AppData\Roaming")));
        assert_eq!(config_from(Os::Mac, &e), Some(PathBuf::from("/home/ann").join("Library").join("Application Support")));
        assert_eq!(config_from(Os::Unix, &e), Some(PathBuf::from("/home/ann").join(".config")));
        assert_eq!(config_from(Os::Windows, &vars(&[("USERPROFILE", "u")])), Some(PathBuf::from("u").join("AppData").join("Roaming")));
        assert_eq!(config_from(Os::Unix, &vars(&[])), None);
    }

    #[test]
    fn xdg_config_home_is_honoured_when_absolute() {
        let abs_dir = if cfg!(windows) { r"C:\xdg" } else { "/xdg" };
        let e = vars(&[("HOME", "/home/ann"), ("XDG_CONFIG_HOME", abs_dir)]);
        assert_eq!(config_from(Os::Unix, &e), Some(PathBuf::from(abs_dir)));
        // macOS keeps to Application Support.
        assert_eq!(config_from(Os::Mac, &e), Some(PathBuf::from("/home/ann").join("Library").join("Application Support")));
        let rel = vars(&[("HOME", "/home/ann"), ("XDG_CONFIG_HOME", "relative/dir")]);
        assert_eq!(config_from(Os::Unix, &rel), Some(PathBuf::from("/home/ann").join(".config")), "relative paths are ignored");
    }

    #[test]
    fn the_old_folder_is_only_on_linux_and_macos() {
        let e = vars(&[("HOME", "/home/ann")]);
        assert_eq!(legacy_snagster_dir(Os::Unix, &e), Some(PathBuf::from("/home/ann").join(".config").join("Snagster")));
        assert_eq!(legacy_snagster_dir(Os::Mac, &e), Some(PathBuf::from("/home/ann").join(".config").join("Snagster")));
        assert_eq!(legacy_snagster_dir(Os::Windows, &e), None);
    }

    const USER_DIRS: &str = r#"
# This file is written by xdg-user-dirs-update
# If you want to change or add directories, just edit the line you're
XDG_DESKTOP_DIR="$HOME/Desktop"
XDG_PICTURES_DIR="$HOME/Bilder"
XDG_VIDEOS_DIR="/media/big disk/Filme"
XDG_MUSIC_DIR="$HOME/"
"#;

    #[test]
    fn user_dirs_file_is_read() {
        let home = Path::new("/home/ann");
        assert_eq!(parse_user_dirs(USER_DIRS, home, "XDG_PICTURES_DIR"), Some(home.join("Bilder")));
        assert_eq!(parse_user_dirs(USER_DIRS, home, "XDG_VIDEOS_DIR"), Some(PathBuf::from("/media/big disk/Filme")));
        assert_eq!(parse_user_dirs(USER_DIRS, home, "XDG_DESKTOP_DIR"), Some(home.join("Desktop")));
        assert_eq!(parse_user_dirs(USER_DIRS, home, "XDG_MUSIC_DIR"), None, "the home folder itself means not set");
        assert_eq!(parse_user_dirs(USER_DIRS, home, "XDG_DOWNLOAD_DIR"), None);
    }

    #[test]
    fn user_dirs_odd_lines() {
        let home = Path::new("/h");
        let text = "  XDG_PICTURES_DIR = \"${HOME}/Pics\"  \n#XDG_VIDEOS_DIR=\"$HOME/Nope\"\ngarbage\nXDG_VIDEOS_DIR=relative\n";
        assert_eq!(parse_user_dirs(text, home, "XDG_PICTURES_DIR"), Some(home.join("Pics")));
        assert_eq!(parse_user_dirs(text, home, "XDG_VIDEOS_DIR"), None, "comments and relative paths don't count");
        let twice = "XDG_PICTURES_DIR=\"$HOME/One\"\nXDG_PICTURES_DIR=\"$HOME/Two\"\n";
        assert_eq!(parse_user_dirs(twice, home, "XDG_PICTURES_DIR"), Some(home.join("Two")), "the last line wins");
        assert_eq!(parse_user_dirs("XDG_PICTURES_DIR=\"/srv/a \\\"b\\\"\"", home, "XDG_PICTURES_DIR"), Some(PathBuf::from("/srv/a \"b\"")));
        assert_eq!(parse_user_dirs("", home, "XDG_PICTURES_DIR"), None);
    }

    #[test]
    fn pictures_and_videos_with_fallbacks() {
        let e = vars(&[("HOME", "/home/ann")]);
        let home = PathBuf::from("/home/ann");
        // Nothing set: the usual names.
        assert_eq!(folder(Os::Unix, Folder::Pictures, &e, &no_file), Some(home.join("Pictures")));
        assert_eq!(folder(Os::Unix, Folder::Videos, &e, &no_file), Some(home.join("Videos")));
        assert_eq!(folder(Os::Mac, Folder::Videos, &e, &no_file), Some(home.join("Movies")));
        assert_eq!(folder(Os::Mac, Folder::Pictures, &e, &no_file), Some(home.join("Pictures")));
        // user-dirs.dirs, read from the config folder.
        let asked = std::cell::RefCell::new(None);
        let file = |c: &Path| {
            *asked.borrow_mut() = Some(c.to_path_buf());
            Some(USER_DIRS.to_string())
        };
        assert_eq!(folder(Os::Unix, Folder::Pictures, &e, &file), Some(home.join("Bilder")));
        assert_eq!(asked.borrow().clone(), Some(home.join(".config")));
        // macOS doesn't read it.
        assert_eq!(folder(Os::Mac, Folder::Pictures, &e, &file), Some(home.join("Pictures")));
        // An environment variable wins over the file.
        let abs_dir = if cfg!(windows) { r"C:\shots" } else { "/shots" };
        let e2 = vars(&[("HOME", "/home/ann"), ("XDG_PICTURES_DIR", abs_dir)]);
        assert_eq!(folder(Os::Unix, Folder::Pictures, &e2, &file), Some(PathBuf::from(abs_dir)));
        // A file without the key: the usual name.
        let other = |_: &Path| Some("XDG_DESKTOP_DIR=\"$HOME/Desktop\"\n".to_string());
        assert_eq!(folder(Os::Unix, Folder::Videos, &e, &other), Some(home.join("Videos")));
        // No home: nothing.
        assert_eq!(folder(Os::Unix, Folder::Pictures, &vars(&[]), &no_file), None);
    }

    #[test]
    fn this_computer_has_its_folders() {
        if home().is_none() {
            return;
        }
        assert!(config_dir().is_some());
        assert!(pictures().is_some() && videos().is_some());
        assert_eq!(snagster_dir().unwrap().file_name().unwrap(), "Snagster");
        if cfg!(target_os = "macos") {
            assert!(snagster_dir().unwrap().ends_with("Library/Application Support/Snagster"));
        }
    }

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("snagster-userdirs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn an_old_folder_is_brought_over_whole() {
        let root = temp("migrate");
        let (old, new) = (root.join("old").join("Snagster"), root.join("new").join("Snagster"));
        std::fs::create_dir_all(old.join("sub")).unwrap();
        std::fs::write(old.join("settings.json"), b"{\"color\":1}").unwrap();
        std::fs::write(old.join("crash.log"), b"log").unwrap();
        std::fs::write(old.join("stream-key.bin"), b"key").unwrap();
        std::fs::write(old.join("fonts-cache.json"), b"[]").unwrap();
        std::fs::write(old.join("sub").join("x.txt"), b"x").unwrap();
        assert_eq!(migrate(&old, &new), Ok(true));
        for f in ["settings.json", "crash.log", "stream-key.bin", "fonts-cache.json"] {
            assert_eq!(std::fs::read(new.join(f)).unwrap(), std::fs::read(old.join(f)).unwrap(), "{f}");
        }
        assert_eq!(std::fs::read(new.join("sub").join("x.txt")).unwrap(), b"x");
        assert!(old.join("settings.json").exists(), "the old folder is left as it was");
        // A second run changes nothing: the new folder has settings now.
        std::fs::write(new.join("settings.json"), b"{\"color\":2}").unwrap();
        assert_eq!(migrate(&old, &new), Ok(false));
        assert_eq!(std::fs::read(new.join("settings.json")).unwrap(), b"{\"color\":2}", "newer settings are never overwritten");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn nothing_is_brought_over_without_old_settings_or_into_the_same_folder() {
        let root = temp("migrate-none");
        let (old, new) = (root.join("old"), root.join("new"));
        assert_eq!(migrate(&old, &new), Ok(false), "no old folder");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("crash.log"), b"log").unwrap();
        assert_eq!(migrate(&old, &new), Ok(false), "an old folder without settings");
        assert!(!new.exists());
        std::fs::write(old.join("settings.json"), b"{}").unwrap();
        assert_eq!(migrate(&old, &old), Ok(false), "Linux without XDG_CONFIG_HOME: the same folder");
        // A new folder that already has other files (a crash log written first) still gets the settings.
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("crash.log"), b"newer log").unwrap();
        assert_eq!(migrate(&old, &new), Ok(true));
        assert_eq!(std::fs::read(new.join("settings.json")).unwrap(), b"{}");
        assert_eq!(std::fs::read(new.join("crash.log")).unwrap(), b"newer log", "files already there are kept");
        let _ = std::fs::remove_dir_all(&root);
    }
}
