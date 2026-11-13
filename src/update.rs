//! New versions: ask GitHub for Snagster's latest release, and install it in place.
//!
//! Windows: the portable zip's Snagster.exe replaces the running one (which is renamed first, as
//! Windows allows), then the new one starts. Linux: the AppImage (or the portable binary) is
//! replaced the same way. macOS: the app bundle is replaced. The download is checked against the
//! size GitHub lists for it. Nothing runs before the person chooses to update.

use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const REPO: &str = "SoloSaftis/Snagster";

/// What GitHub says about the newest release.
#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page: String,
    /// The file for this computer, its address and size.
    pub asset: Option<(String, String, u64)>,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<ApiAsset>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// The release file this kind of computer uses.
pub fn asset_name() -> &'static str {
    if cfg!(windows) {
        "Snagster-windows-x64-portable.zip"
    } else if cfg!(target_os = "macos") {
        "Snagster-macos-arm64.zip"
    } else if std::env::var_os("APPIMAGE").is_some() {
        "Snagster-linux-x86_64.AppImage"
    } else {
        "Snagster-linux-x86_64-portable.tar.gz"
    }
}

/// "v0.10.2" → (0, 10, 2).
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let core = v.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((it.next()??, it.next().flatten().unwrap_or(0), it.next().flatten().unwrap_or(0)))
}

/// Whether `latest` is newer than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(20)))
        .user_agent(concat!("Snagster/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// Ask GitHub for the newest release. Errors are plain sentences.
pub fn latest() -> Result<Release, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut resp = match agent().get(&url).header("Accept", "application/vnd.github+json").call() {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(404)) => {
            return Err("No published release was found (the download page may not be public).".into());
        }
        Err(ureq::Error::StatusCode(403)) | Err(ureq::Error::StatusCode(429)) => {
            return Err("GitHub asked Snagster to wait; try again in an hour.".into());
        }
        Err(e) => return Err(format!("Couldn't reach GitHub: {e}")),
    };
    let r: ApiRelease = resp.body_mut().read_json().map_err(|e| format!("GitHub's answer couldn't be read: {e}"))?;
    if r.draft || r.prerelease {
        return Err("The newest release isn't final yet.".into());
    }
    let want = asset_name();
    let asset = r.assets.iter().find(|a| a.name == want).map(|a| (a.name.clone(), a.browser_download_url.clone(), a.size));
    Ok(Release { version: r.tag_name.trim_start_matches('v').to_string(), notes: r.body, page: r.html_url, asset })
}

/// Download the release file into a new temporary folder; returns its path.
fn download(url: &str, name: &str, size: u64, progress: &dyn Fn(f32)) -> Result<PathBuf, String> {
    if !url.starts_with("https://github.com/") && !url.starts_with("https://objects.githubusercontent.com/") {
        return Err("The download address isn't GitHub's.".into());
    }
    let dir = std::env::temp_dir().join(format!("snagster-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(name);
    let mut resp = agent().get(url).call().map_err(|e| format!("The download failed: {e}"))?;
    let mut reader = resp.body_mut().with_config().limit(1 << 30).reader();
    let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 1 << 16];
    let mut got: u64 = 0;
    loop {
        use std::io::{Read, Write};
        let n = reader.read(&mut buf).map_err(|e| format!("The download stopped: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        got += n as u64;
        if size > 0 {
            progress(got as f32 / size as f32);
        }
    }
    if size > 0 && got != size {
        return Err(format!("The download is incomplete ({got} of {size} bytes)."));
    }
    Ok(path)
}

/// Unpack one file named `want` (any folder) from a zip.
fn unzip_one(zip: &Path, want: &str, to: &Path) -> Result<(), String> {
    let f = std::fs::File::open(zip).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
    for i in 0..z.len() {
        let mut entry = z.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().map(|n| n.replace('\\', "/")).unwrap_or_default();
        if name.rsplit('/').next().is_some_and(|n| n.eq_ignore_ascii_case(want)) {
            let mut out = std::fs::File::create(to).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
            return Ok(());
        }
    }
    Err(format!("{want} isn't in the download."))
}

/// Replace `target` with `new`: the running file is moved aside first (allowed while it runs).
fn swap(target: &Path, new: &Path) -> Result<(), String> {
    let old = target.with_extension(if cfg!(windows) { "old.exe" } else { "old" });
    let _ = std::fs::remove_file(&old);
    std::fs::rename(target, &old).map_err(|e| format!("Couldn't move the old version aside: {e}"))?;
    if let Err(e) = std::fs::copy(new, target) {
        let _ = std::fs::rename(&old, target);
        return Err(format!("Couldn't put the new version in place: {e}"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o755));
    }
    Ok(())
}

/// Remove what an earlier update left behind (the previous version's file).
pub fn clean_up() {
    if let Ok(exe) = std::env::current_exe() {
        let old = exe.with_extension(if cfg!(windows) { "old.exe" } else { "old" });
        let _ = std::fs::remove_file(old);
    }
}

/// Download and install `release`, returning the program to start next.
pub fn install(release: &Release, progress: &dyn Fn(f32)) -> Result<PathBuf, String> {
    let (name, url, size) = release.asset.clone().ok_or("There's no download for this kind of computer in that release.")?;
    let file = download(&url, &name, size, progress)?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if cfg!(windows) {
        let fresh = file.with_file_name("Snagster.exe");
        unzip_one(&file, "Snagster.exe", &fresh)?;
        swap(&exe, &fresh)?;
        return Ok(exe);
    }
    if cfg!(target_os = "macos") {
        // …/Snagster.app/Contents/MacOS/snagster → …/Snagster.app
        let bundle = exe.ancestors().nth(3).map(Path::to_path_buf).filter(|p| p.extension().is_some_and(|e| e == "app"));
        let Some(bundle) = bundle else {
            let fresh = file.with_file_name("snagster");
            unzip_one(&file, "snagster", &fresh)?;
            swap(&exe, &fresh)?;
            return Ok(exe);
        };
        let dir = file.with_file_name("unpacked");
        let st = std::process::Command::new("ditto").arg("-x").arg("-k").arg(&file).arg(&dir).status().map_err(|e| e.to_string())?;
        if !st.success() {
            return Err("Couldn't unpack the download.".into());
        }
        let new_app = dir.join("Snagster.app");
        if !new_app.exists() {
            return Err("Snagster.app isn't in the download.".into());
        }
        let old = bundle.with_extension("old.app");
        let _ = std::fs::remove_dir_all(&old);
        std::fs::rename(&bundle, &old).map_err(|e| format!("Couldn't move the old version aside: {e}"))?;
        let st = std::process::Command::new("ditto").arg(&new_app).arg(&bundle).status().map_err(|e| e.to_string())?;
        if !st.success() {
            let _ = std::fs::rename(&old, &bundle);
            return Err("Couldn't put the new version in place.".into());
        }
        let _ = std::fs::remove_dir_all(&old);
        return Ok(bundle.join("Contents").join("MacOS").join("snagster"));
    }
    // Linux.
    if let Some(app_image) = std::env::var_os("APPIMAGE").map(PathBuf::from) {
        swap(&app_image, &file)?;
        return Ok(app_image);
    }
    let dir = file.with_file_name("unpacked");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let st = std::process::Command::new("tar").arg("-xzf").arg(&file).arg("-C").arg(&dir).status().map_err(|e| e.to_string())?;
    if !st.success() {
        return Err("Couldn't unpack the download.".into());
    }
    let fresh = dir.join("Snagster").join("Snagster");
    if !fresh.exists() {
        return Err("Snagster isn't in the download.".into());
    }
    swap(&exe, &fresh)?;
    Ok(exe)
}

/// Start the new version (and let this one close).
pub fn restart(program: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    if let Some(bundle) = program.ancestors().nth(3).filter(|p| p.extension().is_some_and(|e| e == "app")) {
        return std::process::Command::new("open").arg("-n").arg(bundle).spawn().map(|_| ()).map_err(|e| e.to_string());
    }
    std::process::Command::new(program).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare() {
        assert!(is_newer("v0.10.0", "0.9.3"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.9.0", "0.9.0"));
        assert!(!is_newer("v0.8.1", "0.9.0"));
        assert!(!is_newer("nonsense", "0.9.0"));
        assert_eq!(parse_version("v1.2.3-beta"), Some((1, 2, 3)));
    }
}
