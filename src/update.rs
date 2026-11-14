//! New versions: ask GitHub for Snagster's latest release, and install it in place.
//!
//! Windows: the portable zip's Snagster.exe replaces the running one (which is renamed first, as
//! Windows allows), then the new one starts. Linux: the AppImage (or the portable binary) is
//! replaced the same way. macOS: the app bundle is replaced. The download is checked against the
//! size GitHub lists for it and against its SHA-256 in the release's `SHA256SUMS` file (which
//! the release job of `.github/workflows/build.yml` makes); a release without that file, or a
//! download whose checksum differs, is not installed. Nothing runs before the person chooses
//! to update.

use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const REPO: &str = "SoloSaftis/Snagster";

/// The release file with the SHA-256 of every other file, in `sha256sum`'s format.
pub const SUMS_NAME: &str = "SHA256SUMS";

/// What GitHub says about the newest release.
#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page: String,
    /// The file for this computer, its address and size.
    pub asset: Option<(String, String, u64)>,
    /// Where the release's SHA256SUMS file is (None: the release has none).
    pub sums: Option<String>,
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
    asset_name_for(std::env::consts::OS, std::env::var_os("APPIMAGE").is_some())
}

/// The release file for an operating system ("windows", "macos", "linux"...), and on Linux
/// whether Snagster runs as an AppImage.
fn asset_name_for(os: &str, app_image: bool) -> &'static str {
    match os {
        "windows" => "Snagster-windows-x64-portable.zip",
        "macos" => "Snagster-macos-arm64.zip",
        _ if app_image => "Snagster-linux-x86_64.AppImage",
        _ => "Snagster-linux-x86_64-portable.tar.gz",
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
    release_from(r, asset_name())
}

/// The release GitHub described, with the file named `want`.
fn release_from(r: ApiRelease, want: &str) -> Result<Release, String> {
    if r.draft || r.prerelease {
        return Err("The newest release isn't final yet.".into());
    }
    let asset = r.assets.iter().find(|a| a.name == want).map(|a| (a.name.clone(), a.browser_download_url.clone(), a.size));
    let sums = r.assets.iter().find(|a| a.name == SUMS_NAME).map(|a| a.browser_download_url.clone());
    Ok(Release { version: r.tag_name.trim_start_matches('v').to_string(), notes: r.body, page: r.html_url, asset, sums })
}

fn from_github(url: &str) -> Result<(), String> {
    if !url.starts_with("https://github.com/") && !url.starts_with("https://objects.githubusercontent.com/") {
        return Err("The download address isn't GitHub's.".into());
    }
    Ok(())
}

/// The SHA-256 of every file a SHA256SUMS file lists. Lines are `<64 hex digits>  <name>`
/// (`sha256sum`'s text mode) or `<hex> *<name>` (binary mode); blank and broken lines are
/// skipped.
pub fn parse_sums(text: &str) -> std::collections::HashMap<String, [u8; 32]> {
    let mut out = std::collections::HashMap::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        let Some((hex, name)) = line.split_once(char::is_whitespace) else { continue };
        let name = name.trim_start();
        let name = name.strip_prefix('*').unwrap_or(name).trim_end();
        if let Some(sum) = from_hex(hex)
            && !name.is_empty()
        {
            out.insert(name.to_string(), sum);
        }
    }
    out
}

fn from_hex(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 || !hex.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn to_hex(sum: &[u8]) -> String {
    sum.iter().map(|b| format!("{b:02x}")).collect()
}

/// The SHA-256 the SHA256SUMS text gives for `name`.
fn expected_sum(sums: &str, name: &str) -> Result<[u8; 32], String> {
    parse_sums(sums).get(name).copied().ok_or_else(|| format!("The release's {SUMS_NAME} file has no line for {name}, so the download can't be checked; nothing was installed."))
}

/// The SHA-256 of a file.
fn sha256_file(path: &Path) -> Result<[u8; 32], String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        ctx.update(&buf[..n]);
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(ctx.finish().as_ref());
    Ok(out)
}

/// Check a downloaded file against the SHA-256 SHA256SUMS gives for it; a file that doesn't
/// match is deleted.
fn verify(path: &Path, name: &str, sums: &str) -> Result<(), String> {
    let want = expected_sum(sums, name)?;
    let got = sha256_file(path)?;
    if got != want {
        let _ = std::fs::remove_file(path);
        return Err(format!(
            "The download of {name} doesn't match its SHA-256 checksum (expected {}, got {}); it may be damaged or not the real file. Nothing was installed.",
            to_hex(&want),
            to_hex(&got)
        ));
    }
    Ok(())
}

/// The release's SHA256SUMS file (small text).
fn download_sums(url: &str) -> Result<String, String> {
    from_github(url)?;
    let mut resp = agent().get(url).call().map_err(|e| format!("The {SUMS_NAME} file couldn't be downloaded: {e}"))?;
    resp.body_mut().with_config().limit(1 << 20).read_to_string().map_err(|e| format!("The {SUMS_NAME} file couldn't be read: {e}"))
}

/// Download the release file into a new temporary folder; returns its path.
fn download(url: &str, name: &str, size: u64, progress: &dyn Fn(f32)) -> Result<PathBuf, String> {
    from_github(url)?;
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
    let sums_url = release.sums.as_deref().ok_or(format!(
        "That release has no {SUMS_NAME} file, so the download can't be checked; nothing was installed. Download it from the release page instead."
    ))?;
    let sums = download_sums(sums_url)?;
    // Refuse before the large download when the file isn't listed.
    expected_sum(&sums, &name)?;
    let file = download(&url, &name, size, progress)?;
    verify(&file, &name, &sums)?;
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

    fn temp_folder(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("snagster-update-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn versions_parse() {
        assert_eq!(parse_version("0.9.0"), Some((0, 9, 0)));
        assert_eq!(parse_version("  V2.0.1 "), Some((2, 0, 1)));
        assert_eq!(parse_version("1"), Some((1, 0, 0)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("1.2.3+build.7"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2.3.4"), Some((1, 2, 3)));
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("v"), None);
        assert_eq!(parse_version("latest"), None);
    }

    #[test]
    fn versions_compare_as_numbers_not_text() {
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("0.9.10", "0.9.9"));
        assert!(is_newer("v1.0", "0.9.0"));
        assert!(!is_newer("0.9.9", "0.10.0"));
        // A pre-release of the same version isn't newer, and neither is the same version written differently.
        assert!(!is_newer("0.9.0-beta", "0.9.0"));
        assert!(!is_newer("v0.9", "0.9.0"));
        assert!(!is_newer("0.9.0", ""));
        assert!(!is_newer("", ""));
    }

    #[test]
    fn this_version_is_readable() {
        assert!(parse_version(env!("CARGO_PKG_VERSION")).is_some());
        assert!(!is_newer(env!("CARGO_PKG_VERSION"), env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn every_system_gets_its_own_file() {
        assert_eq!(asset_name_for("windows", false), "Snagster-windows-x64-portable.zip");
        assert_eq!(asset_name_for("windows", true), "Snagster-windows-x64-portable.zip");
        assert_eq!(asset_name_for("macos", false), "Snagster-macos-arm64.zip");
        assert_eq!(asset_name_for("macos", true), "Snagster-macos-arm64.zip");
        assert_eq!(asset_name_for("linux", false), "Snagster-linux-x86_64-portable.tar.gz");
        assert_eq!(asset_name_for("linux", true), "Snagster-linux-x86_64.AppImage");
        // This computer's choice is one of them.
        let here = asset_name();
        if cfg!(windows) {
            assert_eq!(here, "Snagster-windows-x64-portable.zip");
        } else if cfg!(target_os = "macos") {
            assert_eq!(here, "Snagster-macos-arm64.zip");
        } else {
            assert!(here.starts_with("Snagster-linux-x86_64"));
        }
    }

    const ANSWER: &str = r#"{
        "tag_name": "v0.10.2",
        "body": "What changed",
        "html_url": "https://github.com/SoloSaftis/Snagster/releases/tag/v0.10.2",
        "draft": false,
        "prerelease": false,
        "unknown_field": 1,
        "assets": [
            { "name": "Snagster-windows-x64-portable.zip", "browser_download_url": "https://github.com/w.zip", "size": 100 },
            { "name": "Snagster-macos-arm64.zip", "browser_download_url": "https://github.com/m.zip", "size": 200 },
            { "name": "Snagster-linux-x86_64.AppImage", "browser_download_url": "https://github.com/l.AppImage", "size": 300 },
            { "name": "Snagster-linux-x86_64-portable.tar.gz", "browser_download_url": "https://github.com/l.tar.gz" },
            { "name": "SHA256SUMS", "browser_download_url": "https://github.com/SHA256SUMS", "size": 400 }
        ]
    }"#;

    fn answer() -> ApiRelease {
        serde_json::from_str(ANSWER).unwrap()
    }

    #[test]
    fn release_picks_the_file_for_each_system() {
        let cases = [
            (("windows", false), ("https://github.com/w.zip", 100)),
            (("macos", false), ("https://github.com/m.zip", 200)),
            (("linux", true), ("https://github.com/l.AppImage", 300)),
            (("linux", false), ("https://github.com/l.tar.gz", 0)),
        ];
        for ((os, app_image), (url, size)) in cases {
            let want = asset_name_for(os, app_image);
            let r = release_from(answer(), want).unwrap();
            assert_eq!(r.version, "0.10.2");
            assert_eq!(r.notes, "What changed");
            assert!(r.page.ends_with("/v0.10.2"));
            assert_eq!(r.asset, Some((want.to_string(), url.to_string(), size)), "{os}");
            assert_eq!(r.sums.as_deref(), Some("https://github.com/SHA256SUMS"));
        }
    }

    // `sha256sum` of the three bytes "abc" (the FIPS 180-2 example) and of nothing.
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn sums_files_are_read_in_both_sha256sum_formats() {
        let text = format!("{ABC}  Snagster-windows-x64-portable.zip\n{EMPTY} *Snagster-macos-arm64.zip\r\n\nnot a line\n{}  short.zip\nzz{}  bad-hex.zip\n", &ABC[..10], &ABC[2..]);
        let s = parse_sums(&text);
        assert_eq!(s.len(), 2, "{s:?}");
        assert_eq!(to_hex(&s["Snagster-windows-x64-portable.zip"]), ABC);
        assert_eq!(to_hex(&s["Snagster-macos-arm64.zip"]), EMPTY, "binary-mode star and CRLF");
        assert!(parse_sums("").is_empty());
        // Upper-case hex reads the same.
        let up = parse_sums(&format!("{}  a.zip", ABC.to_uppercase()));
        assert_eq!(to_hex(&up["a.zip"]), ABC);
        // A name with spaces stays whole.
        assert!(parse_sums(&format!("{ABC}  my file.zip")).contains_key("my file.zip"));
    }

    #[test]
    fn a_file_without_a_line_is_refused() {
        let e = expected_sum(&format!("{ABC}  other.zip\n"), "Snagster-windows-x64-portable.zip").unwrap_err();
        assert!(e.contains("no line for Snagster-windows-x64-portable.zip") && e.contains("nothing was installed"), "{e}");
    }

    #[test]
    fn downloads_are_checked_against_their_sha256() {
        let dir = temp_folder("sha");
        let f = dir.join("a.zip");
        std::fs::write(&f, b"abc").unwrap();
        assert_eq!(to_hex(&sha256_file(&f).unwrap()), ABC);
        let sums = format!("{ABC}  a.zip\n{EMPTY}  b.zip\n");
        assert_eq!(verify(&f, "a.zip", &sums), Ok(()));
        // The same bytes under another name's checksum: refused and deleted.
        let e = verify(&f, "b.zip", &sums).unwrap_err();
        assert!(e.contains("doesn't match its SHA-256") && e.contains(EMPTY) && e.contains(ABC), "{e}");
        assert!(!f.exists(), "a file that doesn't match is not left to be used");
        // A big file is read in pieces.
        let big = dir.join("big.bin");
        std::fs::write(&big, vec![7u8; 300_000]).unwrap();
        let one = ring::digest::digest(&ring::digest::SHA256, &vec![7u8; 300_000]);
        assert_eq!(sha256_file(&big).unwrap().as_slice(), one.as_ref());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_release_without_sums_is_not_installed() {
        let r = Release {
            version: "9.9.9".into(),
            notes: String::new(),
            page: String::new(),
            asset: Some((asset_name().into(), "https://github.com/x.zip".into(), 1)),
            sums: None,
        };
        let e = install(&r, &|_| {}).unwrap_err();
        assert!(e.contains("no SHA256SUMS file") && e.contains("nothing was installed"), "{e}");
        let e = download_sums("https://example.com/SHA256SUMS").unwrap_err();
        assert_eq!(e, "The download address isn't GitHub's.");
    }

    #[test]
    fn release_without_a_file_for_this_system_has_no_asset() {
        let r = release_from(answer(), "Snagster-solaris.zip").unwrap();
        assert!(r.asset.is_none());
        let bare: ApiRelease = serde_json::from_str(r#"{ "tag_name": "0.11.0" }"#).unwrap();
        let r = release_from(bare, asset_name()).unwrap();
        assert_eq!(r.version, "0.11.0");
        assert!(r.asset.is_none() && r.sums.is_none() && r.notes.is_empty() && r.page.is_empty());
    }

    #[test]
    fn drafts_and_prereleases_are_not_offered() {
        let mut a = answer();
        a.draft = true;
        assert!(release_from(a, asset_name()).is_err());
        let mut a = answer();
        a.prerelease = true;
        assert!(release_from(a, asset_name()).is_err());
    }

    #[test]
    fn downloads_only_come_from_github() {
        let none = |_: f32| {};
        for url in [
            "http://github.com/x.zip",
            "https://example.com/x.zip",
            "https://github.com.evil.example/x.zip",
            "file:///C:/x.zip",
            "",
        ] {
            let e = download(url, "x.zip", 1, &none).unwrap_err();
            assert_eq!(e, "The download address isn't GitHub's.", "{url}");
        }
    }

    fn make_zip(path: &Path, files: &[(&str, &[u8])]) {
        use std::io::Write;
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, data) in files {
            z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn unzip_finds_the_file_in_any_folder_and_any_case() {
        let dir = temp_folder("unzip");
        let zip = dir.join("a.zip");
        make_zip(&zip, &[("readme.txt", b"hello"), ("Snagster/SNAGSTER.EXE", b"program")]);
        let out = dir.join("out.exe");
        unzip_one(&zip, "Snagster.exe", &out).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"program");
        let e = unzip_one(&zip, "missing.exe", &dir.join("m")).unwrap_err();
        assert!(e.contains("missing.exe"));
        assert!(unzip_one(&dir.join("no-such.zip"), "Snagster.exe", &out).is_err());
        std::fs::write(dir.join("bad.zip"), b"not a zip").unwrap();
        assert!(unzip_one(&dir.join("bad.zip"), "Snagster.exe", &out).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn swap_moves_the_old_version_aside() {
        let dir = temp_folder("swap");
        let target = dir.join("prog.bin");
        let fresh = dir.join("fresh.bin");
        std::fs::write(&target, b"old").unwrap();
        std::fs::write(&fresh, b"new").unwrap();
        swap(&target, &fresh).unwrap();
        let aside = target.with_extension(if cfg!(windows) { "old.exe" } else { "old" });
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert_eq!(std::fs::read(&aside).unwrap(), b"old");
        // A second update replaces the file left from the first.
        std::fs::write(&fresh, b"newer").unwrap();
        swap(&target, &fresh).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"newer");
        assert_eq!(std::fs::read(&aside).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn swap_puts_the_old_version_back_when_the_new_one_is_missing() {
        let dir = temp_folder("swap-fail");
        let target = dir.join("prog.bin");
        std::fs::write(&target, b"old").unwrap();
        assert!(swap(&target, &dir.join("nothing-here")).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert!(swap(&dir.join("no-target"), &target).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_needs_a_file_for_this_computer() {
        let r = Release { version: "9.9.9".into(), notes: String::new(), page: String::new(), asset: None, sums: None };
        assert!(install(&r, &|_| {}).is_err());
    }
}
