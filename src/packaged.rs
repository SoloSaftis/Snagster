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
//! - **Settings** stay in `%APPDATA%\Snagster`, shared with the portable copy: the manifest turns
//!   off the package's private copy of AppData (`unvirtualizedResources`), so crash.log and the
//!   settings folder are where Snagster says they are, for Explorer and Notepad too.

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
        assert!(MANIFEST.contains("Version=\"__VERSION__\"") && MANIFEST.contains("Publisher=\"__PUBLISHER__\""));
        // CI makes the package under the name the updater looks for.
        let ci = include_str!("../.github/workflows/build.yml");
        assert!(ci.contains(crate::update::MSIX_NAME), "build.yml makes {}", crate::update::MSIX_NAME);
    }

    #[test]
    fn tests_do_not_run_from_a_package() {
        // `cargo test` runs a plain program, never an installed package.
        assert!(!is_packaged());
        assert!(!mcp_command().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn without_a_package_there_is_no_startup_task() {
        // StartupTask needs a package identity; a plain program gets an error, not a crash.
        assert!(startup().is_err());
    }
}
