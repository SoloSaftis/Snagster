//! Snagster installed from its MSIX package (Windows) rather than run from the portable zip.
//!
//! An installed Snagster lives in a read-only, versioned folder under `C:\Program Files\WindowsApps`
//! that changes with every update, so a few things differ from the portable copy:
//!
//! - **Updates** come as the next `Snagster-windows-x64.msix`: the updater downloads it, checks it
//!   against `SHA256SUMS` as always, and opens it in Windows' App Installer, which updates the
//!   package (`update.rs`). Nothing is written next to the program.
//! - **Starting with Windows** is the package's startup task (`packaging/msix/AppxManifest.xml`,
//!   `TaskId` below), switched with Windows' StartupTask API; a registry Run entry would point
//!   at a folder the next update removes.
//! - **AI apps** start Snagster through the `snagster.exe` alias Windows puts in
//!   `%LOCALAPPDATA%\Microsoft\WindowsApps`, which always runs the installed version.
//! - **Settings** are in `%APPDATA%\Snagster`. The GitHub package turns off the package's private
//!   copy of AppData (`unvirtualizedResources`), so it shares the folder with the portable copy.
//!   The Microsoft Store refused that capability, so the Store package leaves it out
//!   (`make-msix.ps1 -Store`): Windows then puts the files Snagster creates under AppData in the
//!   package's own folder, `%LOCALAPPDATA%\Packages\<family name>\LocalCache\Roaming\...`, which
//!   other programs don't see at the usual path (files that were already there, from the
//!   portable copy, are changed in place). [`visible_path`] says where such a file really is, so
//!   Explorer opens the right folder (Help > Problem reports) and Settings names the real
//!   crash.log. Pictures and Videos are outside AppData and never redirected.

use std::path::{Path, PathBuf};

/// The startup task's id in the package manifest.
#[cfg(any(windows, test))]
pub const STARTUP_TASK: &str = "SnagsterStartup";

/// The command-line name the package registers.
pub const ALIAS: &str = "snagster.exe";

/// Whether this Snagster runs from an installed package (it has a package identity).
pub fn is_packaged() -> bool {
    #[cfg(windows)]
    {
        use std::sync::OnceLock;
        static PACKAGED: OnceLock<bool> = OnceLock::new();
        *PACKAGED.get_or_init(|| {
            use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
            use windows_sys::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;
            let mut len = 0u32;
            // Without a package: APPMODEL_ERROR_NO_PACKAGE. With one: the buffer is too small.
            unsafe { GetCurrentPackageFullName(&mut len, std::ptr::null_mut()) == ERROR_INSUFFICIENT_BUFFER }
        })
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Whether the installed package came from the Microsoft Store (the Store signed it, and the
/// Store installs its updates). False for a package from GitHub and for the portable copy.
pub fn from_store() -> bool {
    #[cfg(windows)]
    {
        use std::sync::OnceLock;
        static STORE: OnceLock<bool> = OnceLock::new();
        *STORE.get_or_init(|| {
            use windows::ApplicationModel::{Package, PackageSignatureKind};
            is_packaged() && Package::Current().and_then(|p| p.SignatureKind()).is_ok_and(|k| k == PackageSignatureKind::Store)
        })
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// The installed package's family name (`Name_publisherhash`), the name of its private folder
/// under `%LOCALAPPDATA%\Packages`. None for the portable copy.
pub fn family_name() -> Option<String> {
    #[cfg(windows)]
    {
        use std::sync::OnceLock;
        static FAMILY: OnceLock<Option<String>> = OnceLock::new();
        FAMILY
            .get_or_init(|| {
                use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS};
                use windows_sys::Win32::Storage::Packaging::Appx::GetCurrentPackageFamilyName;
                let mut len = 0u32;
                // Without a package: APPMODEL_ERROR_NO_PACKAGE. With one: the buffer is too small.
                if unsafe { GetCurrentPackageFamilyName(&mut len, std::ptr::null_mut()) } != ERROR_INSUFFICIENT_BUFFER {
                    return None;
                }
                let mut buf = vec![0u16; len as usize];
                if unsafe { GetCurrentPackageFamilyName(&mut len, buf.as_mut_ptr()) } != ERROR_SUCCESS {
                    return None;
                }
                let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                Some(String::from_utf16_lossy(&buf[..end])).filter(|f| !f.is_empty())
            })
            .clone()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Where `path` (a file or folder Snagster made under AppData) is for other programs such as
/// Explorer and Notepad. In a package whose AppData is private (the Store edition) that is the
/// package's own copy when Windows put it there; otherwise, and for the portable copy, `path`.
pub fn visible_path(path: &Path) -> PathBuf {
    let Some(family) = family_name() else {
        return path.to_path_buf();
    };
    visible_path_for(path, Some(&family), crate::userdirs::config_dir().as_deref(), crate::userdirs::local_app_data().as_deref(), &|p| p.exists())
}

/// [`visible_path`] with what it depends on passed in, for the tests: the package family name,
/// `%APPDATA%` (Roaming), `%LOCALAPPDATA%` (Local) and whether a path exists.
pub(crate) fn visible_path_for(path: &Path, family: Option<&str>, roaming: Option<&Path>, local: Option<&Path>, exists: &dyn Fn(&Path) -> bool) -> PathBuf {
    let (Some(family), Some(local)) = (family, local) else {
        return path.to_path_buf();
    };
    // Already in a package's own folder: nothing to translate.
    if path.starts_with(local.join("Packages")) {
        return path.to_path_buf();
    }
    let private = local.join("Packages").join(family).join("LocalCache");
    let Some((real_root, private_root, rel)) = [(roaming, "Roaming"), (Some(local), "Local")].into_iter().find_map(|(root, sub)| {
        let root = root?;
        Some((root, private.join(sub), path.strip_prefix(root).ok()?))
    }) else {
        return path.to_path_buf();
    };
    let private_path = private_root.join(rel);
    // The nearest of the file and the folders above it that exists says which side it is on:
    // a crash.log not written yet goes beside the settings.json Windows put in the package's
    // folder. Nothing at all yet: the package's folder, where Windows puts every new file in
    // the Store edition. (The GitHub MSIX writes to the real folder; it only gets this answer
    // before anything exists, since the window makes the folder at start: `instance.rs`.)
    for part in rel.ancestors().filter(|a| !a.as_os_str().is_empty()) {
        if exists(&private_root.join(part)) {
            return private_path;
        }
        if exists(&real_root.join(part)) {
            return path.to_path_buf();
        }
    }
    private_path
}

/// crash.log where other programs find it, for the Settings window: worked out at most every
/// two seconds, not on every frame (in a package it asks Windows for folders and looks at the disk).
pub fn shown_crash_log() -> Option<PathBuf> {
    use std::cell::RefCell;
    use std::time::{Duration, Instant};
    thread_local! {
        static SHOWN: RefCell<Option<(Instant, Option<PathBuf>)>> = const { RefCell::new(None) };
    }
    SHOWN.with(|s| {
        let mut s = s.borrow_mut();
        match &*s {
            Some((at, p)) if at.elapsed() < Duration::from_secs(2) => p.clone(),
            _ => {
                let p = crate::crash_log_path().map(|p| visible_path(&p));
                *s = Some((Instant::now(), p.clone()));
                p
            }
        }
    })
}

/// The folder Help > Problem reports opens: the one that really holds `log` (crash.log), else
/// Snagster's folder as other programs see it.
pub fn problem_reports_folder(log: &Path) -> Option<PathBuf> {
    problem_reports_folder_with(log, &visible_path)
}

pub(crate) fn problem_reports_folder_with(log: &Path, visible: &dyn Fn(&Path) -> PathBuf) -> Option<PathBuf> {
    let shown = visible(log);
    if shown != log {
        return shown.parent().map(Path::to_path_buf);
    }
    log.parent().map(visible)
}

/// The program AI apps should start: the package's alias when installed, else this program.
pub fn mcp_command() -> String {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("snagster"));
    mcp_command_for(&exe, is_packaged(), crate::userdirs::local_app_data().as_deref())
}

pub(crate) fn mcp_command_for(exe: &Path, packaged: bool, local_app_data: Option<&Path>) -> String {
    match (packaged, local_app_data) {
        (true, Some(d)) => d.join("Microsoft").join("WindowsApps").join(ALIAS).display().to_string(),
        (true, None) => ALIAS.to_string(),
        _ => exe.display().to_string(),
    }
}

/// Where the startup task stands, in Windows' words.
#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Startup {
    On,
    Off,
    /// Turned off in Task Manager or Settings: only the person can turn it on again there.
    OffByPerson,
    /// Turned off by the IT department's policy.
    OffByPolicy,
}

/// The message for a startup task that couldn't be switched on.
#[cfg(any(windows, test))]
pub fn startup_refusal(s: Startup) -> Option<&'static str> {
    match s {
        Startup::OffByPerson => Some("Windows has Snagster turned off as a startup app. Turn it on in Settings > Apps > Startup."),
        Startup::OffByPolicy => Some("Your organisation's settings don't allow Snagster to start with Windows."),
        _ => None,
    }
}

#[cfg(windows)]
fn startup_state(s: windows::ApplicationModel::StartupTaskState) -> Startup {
    use windows::ApplicationModel::StartupTaskState as S;
    match s {
        S::Enabled | S::EnabledByPolicy => Startup::On,
        S::DisabledByUser => Startup::OffByPerson,
        S::DisabledByPolicy => Startup::OffByPolicy,
        _ => Startup::Off,
    }
}

/// The package's startup task now.
#[cfg(windows)]
pub fn startup() -> Result<Startup, String> {
    let task = windows::ApplicationModel::StartupTask::GetAsync(&windows::core::HSTRING::from(STARTUP_TASK)).and_then(|a| a.join()).map_err(|e| e.to_string())?;
    Ok(startup_state(task.State().map_err(|e| e.to_string())?))
}

/// Switch the package's startup task on or off.
#[cfg(windows)]
pub fn set_startup(on: bool) -> Result<(), String> {
    let task = windows::ApplicationModel::StartupTask::GetAsync(&windows::core::HSTRING::from(STARTUP_TASK)).and_then(|a| a.join()).map_err(|e| e.to_string())?;
    if !on {
        return task.Disable().map_err(|e| e.to_string());
    }
    let now = startup_state(task.RequestEnableAsync().and_then(|a| a.join()).map_err(|e| e.to_string())?);
    match startup_refusal(now) {
        Some(why) => Err(why.into()),
        None => Ok(()),
    }
}

/// Open a downloaded `.msix` in Windows' App Installer (which updates the installed package).
pub fn open_installer(msix: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        use windows::core::{HSTRING, w};
        let r = unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(msix.as_os_str()), None, None, SW_SHOWNORMAL) };
        // ShellExecute returns a number above 32 when it worked.
        if r.0 as isize > 32 {
            Ok(())
        } else {
            Err(format!("Windows couldn't open the installer (error {}). App Installer may be turned off on this computer; ask your IT department, or download the update from the release page.", r.0 as isize))
        }
    }
    #[cfg(not(windows))]
    {
        let _ = msix;
        Err("MSIX packages are for Windows.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_apps_start_the_alias_when_installed() {
        let exe = Path::new(r"C:\Program Files\WindowsApps\SoloSaftis.Snagster_0.10.6.0_x64__abc\Snagster.exe");
        let local = Path::new(r"C:\Users\pat\AppData\Local");
        let alias = mcp_command_for(exe, true, Some(local));
        assert!(alias.ends_with("snagster.exe"), "{alias}");
        assert!(alias.contains("WindowsApps") && alias.starts_with(r"C:\Users\pat"), "{alias}");
        assert!(!alias.contains("0.10.6"), "the alias must not name a version folder: {alias}");
        assert_eq!(mcp_command_for(exe, true, None), "snagster.exe");
        let portable = Path::new(r"D:\Tools\Snagster\Snagster.exe");
        assert_eq!(mcp_command_for(portable, false, Some(local)), portable.display().to_string());
    }

    /// The Store edition's AppData is private: a file Snagster made there is shown at the
    /// package's own copy, a file that was already in the real folder (from the portable copy)
    /// stays where it is, and the portable copy's paths never change.
    #[test]
    fn files_in_private_appdata_are_shown_where_they_really_are() {
        let roaming = Path::new(r"C:\Users\pat\AppData\Roaming");
        let local = Path::new(r"C:\Users\pat\AppData\Local");
        let family = Some("SoloSaftis.Snagster_abc123");
        let log = roaming.join("Snagster").join("crash.log");
        let private_log = local.join("Packages").join("SoloSaftis.Snagster_abc123").join("LocalCache").join("Roaming").join("Snagster").join("crash.log");
        let is_private = |p: &Path| p == private_log;
        // Made by the Store edition: in the package's folder.
        assert_eq!(visible_path_for(&log, family, Some(roaming), Some(local), &is_private), private_log);
        // Already in the real AppData (made by the portable copy): Windows changes it in place.
        let real_log = log.clone();
        assert_eq!(visible_path_for(&log, family, Some(roaming), Some(local), &|p: &Path| p == real_log), log);
        // Not written yet: beside the settings Windows put in the package's folder...
        let private_dir = private_log.parent().unwrap().to_path_buf();
        assert_eq!(visible_path_for(&log, family, Some(roaming), Some(local), &|p: &Path| p == private_dir), private_log);
        // ...or beside the portable copy's settings in the real folder...
        let real_dir = log.parent().unwrap().to_path_buf();
        assert_eq!(visible_path_for(&log, family, Some(roaming), Some(local), &|p: &Path| p == real_dir), log);
        // ...and on a fresh computer where Windows will put the first file: the package's folder.
        assert_eq!(visible_path_for(&log, family, Some(roaming), Some(local), &|_| false), private_log);
        // The portable copy (no package): never translated, whatever exists.
        assert_eq!(visible_path_for(&log, None, Some(roaming), Some(local), &|_| true), log);
        // Local AppData is redirected too, to LocalCache\Local.
        let cache = local.join("Snagster").join("x.json");
        assert_eq!(visible_path_for(&cache, family, Some(roaming), Some(local), &|_| true), local.join("Packages").join("SoloSaftis.Snagster_abc123").join("LocalCache").join("Local").join("Snagster").join("x.json"));
        // Outside AppData (Pictures, Videos, SNAGSTER_HOME) and the package's folder itself: as is.
        let pictures = &Path::new(r"C:\Users\pat").join("Pictures").join("Snagster").join("a.png");
        assert_eq!(visible_path_for(pictures, family, Some(roaming), Some(local), &|_| true), *pictures);
        assert_eq!(visible_path_for(&private_log, family, Some(roaming), Some(local), &|_| true), private_log);
        // No %LOCALAPPDATA%: nothing to translate to.
        assert_eq!(visible_path_for(&log, family, Some(roaming), None, &|_| true), log);
    }

    #[test]
    fn problem_reports_open_the_folder_that_holds_crash_log() {
        let log = &Path::new(r"C:\Users\pat\AppData\Roaming").join("Snagster").join("crash.log");
        let dir = log.parent().unwrap();
        let private = &Path::new(r"C:\P").join("LocalCache").join("Roaming").join("Snagster");
        // crash.log is in the package's folder: open that one.
        let moved = |p: &Path| if p == log { private.join("crash.log") } else { p.to_path_buf() };
        assert_eq!(problem_reports_folder_with(log, &moved).unwrap(), *private);
        // No crash.log yet, but the package's folder exists: open it.
        let folder = |p: &Path| if p == dir { private.to_path_buf() } else { p.to_path_buf() };
        assert_eq!(problem_reports_folder_with(log, &folder).unwrap(), *private);
        // The portable copy and the GitHub package: the usual folder.
        assert_eq!(problem_reports_folder_with(log, &|p: &Path| p.to_path_buf()).unwrap(), dir);
    }

    #[test]
    fn a_refused_startup_task_says_where_to_turn_it_on() {
        assert!(startup_refusal(Startup::OffByPerson).unwrap().contains("Startup"));
        assert!(startup_refusal(Startup::OffByPolicy).unwrap().contains("organisation"));
        assert_eq!(startup_refusal(Startup::On), None);
        assert_eq!(startup_refusal(Startup::Off), None);
    }

    /// The manifest declares what the code expects: the startup task and alias by the same
    /// names, the program the build makes, Snagster projects, and settings in the real AppData.
    #[test]
    fn the_package_manifest_matches_the_code() {
        const MANIFEST: &str = include_str!("../packaging/msix/AppxManifest.xml");
        assert!(MANIFEST.contains(&format!("TaskId=\"{STARTUP_TASK}\"")));
        assert!(MANIFEST.contains(&format!("Alias=\"{ALIAS}\"")));
        assert_eq!(MANIFEST.matches("Executable=\"Snagster.exe\"").count(), 3, "the app, its startup task and its alias");
        assert!(MANIFEST.contains("<uap:FileType>.snag</uap:FileType>"));
        assert!(MANIFEST.contains("<desktop6:FileSystemWriteVirtualization>disabled<"));
        assert!(MANIFEST.contains("Name=\"unvirtualizedResources\"") && MANIFEST.contains("Name=\"runFullTrust\""));
        for fill in ["Version=\"__VERSION__\"", "Name=\"__IDENTITY_NAME__\"", "Publisher=\"__PUBLISHER__\"", ">__PUBLISHER_DISPLAY_NAME__<"] {
            assert!(MANIFEST.contains(fill), "make-msix.ps1 fills in {fill}");
        }
        let script = include_str!("../packaging/msix/make-msix.ps1");
        assert!(MANIFEST.contains("<Resources>\n__RESOURCES__") || MANIFEST.contains("<Resources>\r\n__RESOURCES__"));
        for fill in ["__RESOURCES__", "__VERSION__", "__IDENTITY_NAME__", "__PUBLISHER__", "__PUBLISHER_DISPLAY_NAME__"] {
            assert!(script.contains(&format!("\"{fill}\"")), "make-msix.ps1 replaces {fill}");
        }
        // CI makes the package under the name the updater looks for.
        let ci = include_str!("../.github/workflows/build.yml");
        assert!(ci.contains(crate::update::MSIX_NAME), "build.yml makes {}", crate::update::MSIX_NAME);
    }

    /// The Store package (`make-msix.ps1 -Store`, which CI uses for it) leaves out exactly what
    /// certification refused and keeps everything else; the GitHub package keeps it all.
    #[test]
    fn the_store_package_leaves_out_unvirtualized_resources() {
        const MANIFEST: &str = include_str!("../packaging/msix/AppxManifest.xml");
        const MARK: &str = "<!-- github-only -->";
        let script = include_str!("../packaging/msix/make-msix.ps1");
        assert!(script.contains(&format!("Contains(\"{MARK}\")")), "make-msix.ps1 -Store drops the lines marked {MARK}");
        let ci = include_str!("../.github/workflows/build.yml");
        let store_line = ci.lines().find(|l| l.contains("Snagster-windows-x64-store.msix") && l.contains("make-msix.ps1")).expect("CI makes the Store package");
        assert!(store_line.trim_end().ends_with("-Store"), "CI makes the Store package with -Store: {store_line}");
        let github_line = ci.lines().find(|l| l.contains("Snagster-windows-x64.msix") && l.contains("make-msix.ps1")).unwrap();
        assert!(!github_line.contains("-Store"), "the GitHub package keeps its AppData shared with the portable copy");

        let marked: Vec<&str> = MANIFEST.lines().filter(|l| l.contains(MARK)).collect();
        assert_eq!(marked.len(), 3, "the capability and the two virtualization lines: {marked:?}");
        let store: String = MANIFEST.lines().filter(|l| !l.contains(MARK)).collect::<Vec<_>>().join("\n");
        assert!(!store.contains("Name=\"unvirtualizedResources\""));
        assert!(!store.contains("WriteVirtualization>"));
        assert!(store.contains("<rescap:Capability Name=\"runFullTrust\" />"));
        assert!(store.contains("<DeviceCapability Name=\"microphone\" />"));
        assert!(store.contains("</Properties>") && store.contains("</Capabilities>") && store.trim_end().ends_with("</Package>"));
    }

    /// The package (and so the Store) lists every language Snagster speaks, each as a language
    /// code Windows and the Store accept, and no language Snagster doesn't have.
    #[test]
    fn the_package_declares_every_interface_language() {
        let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(include_str!("../packaging/msix/store-languages.json")).unwrap();
        let names: serde_json::Map<String, serde_json::Value> = serde_json::from_str(include_str!("../assets/i18n/language-names.json")).unwrap();
        let ours: std::collections::BTreeSet<&str> = names.keys().map(String::as_str).collect();
        let mapped: std::collections::BTreeSet<&str> = map.keys().map(String::as_str).filter(|k| *k != "_comment").collect();
        assert_eq!(ours, mapped, "store-languages.json has one entry per language pack");
        let mut seen = std::collections::BTreeSet::new();
        for (ours, code) in map.iter().filter(|(k, _)| *k != "_comment") {
            let code = code.as_str().unwrap();
            assert!(seen.insert(code), "{code} is used twice");
            assert_eq!(code, code.to_ascii_lowercase(), "{ours}: Store codes are lower case");
            assert!(code.split('-').all(|p| (2..=8).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric())), "{ours}: {code} isn't a language tag");
        }
        assert_eq!(map["en"], "en-us", "English is the package's default language");
    }

    /// The Store listing (packaging/msix/listing, made into Partner Center's import folder by
    /// make-listing.js) has every language the package declares, each with every text the
    /// English one has and within the Store's limits.
    #[test]
    fn the_store_listing_is_in_every_language() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packaging/msix/listing");
        let read = |code: &str| -> serde_json::Map<String, serde_json::Value> {
            serde_json::from_str(&std::fs::read_to_string(dir.join(format!("{code}.json"))).unwrap_or_else(|e| panic!("{code}: {e}"))).unwrap_or_else(|e| panic!("{code}: {e}"))
        };
        let english = read("en-us");
        let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(include_str!("../packaging/msix/store-languages.json")).unwrap();
        for (_, code) in map.iter().filter(|(k, _)| *k != "_comment") {
            let code = code.as_str().unwrap();
            let listing = read(code);
            for key in english.keys() {
                let text = listing.get(key).and_then(|v| v.as_str()).unwrap_or_default();
                assert!(!text.trim().is_empty(), "{code}: {key} is missing");
                let max = match key.as_str() {
                    "Description" => 10000,
                    "ShortDescription" => 1000,
                    "ReleaseNotes" => 1500,
                    k if k.starts_with("SearchTerm") => 30,
                    _ => 200,
                };
                assert!(text.chars().count() <= max, "{code}: {key} is longer than {max} characters");
            }
        }
    }

    #[test]
    fn tests_do_not_run_from_a_package() {
        // `cargo test` runs a plain program, never an installed package.
        assert!(!is_packaged());
        assert!(!from_store());
        assert_eq!(family_name(), None);
        let p = Path::new("/some/where/crash.log");
        assert_eq!(visible_path(p), p);
        assert_eq!(shown_crash_log(), crate::crash_log_path());
        assert_eq!(shown_crash_log(), crate::crash_log_path(), "asking again within two seconds gives the same answer");
        assert!(!mcp_command().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn without_a_package_there_is_no_startup_task() {
        // StartupTask needs a package identity; a plain program gets an error, not a crash.
        assert!(startup().is_err());
    }
}
