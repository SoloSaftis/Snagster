//! `snagster --hw-report [file]`: everything about this computer's graphics-chip encoding in one
//! file, for a hardware report or a test round on a rented machine. The system and processor, every
//! graphics chip the encoders see (on Linux each VA-API render node and the NVIDIA driver), every
//! encoder opened by its name and timed, then `--video-test quick` and `--stream-test quick`
//! (run as child processes, so a crash in one still leaves the rest of the report). Nothing is
//! sent anywhere: the report is written to a file and printed.

use crate::encode::{self, Codec, VideoConfig};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Where the report goes when no file is given: the current folder, else the temp folder (the
/// Windows program may be started from a folder it can't write to).
pub fn default_path(cwd_writable: bool) -> PathBuf {
    let name = "snagster-hw-report.txt";
    if cwd_writable { PathBuf::from(name) } else { std::env::temp_dir().join(name) }
}

/// `PRETTY_NAME` from /etc/os-release.
#[cfg(any(target_os = "linux", test))]
pub fn os_release_name(text: &str) -> Option<String> {
    text.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=")).map(|v| v.trim().trim_matches('"').to_string()).filter(|v| !v.is_empty())
}

/// The processor's name from /proc/cpuinfo.
#[cfg(any(target_os = "linux", test))]
pub fn cpuinfo_model(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim() == "model name").then(|| v.trim().to_string())
    })
}

/// How many lines of a test's output say FAIL.
pub fn failures(text: &str) -> usize {
    text.lines().filter(|l| l.trim_start().starts_with("FAIL")).count()
}

/// What the window can be drawn with: the Direct3D 12 graphics chips (Windows), whether this
/// is a remote session, and whether Snagster runs installed or portable.
fn drawing_lines() -> Vec<String> {
    let order = format!(
        "Window drawing tried in this order: {}",
        crate::drawing::order_here().iter().map(|b| b.name()).collect::<Vec<_>>().join(", then ")
    );
    #[cfg(not(windows))]
    {
        vec![order]
    }
    #[cfg(windows)]
    {
        let mut v = vec![order];
        use eframe::egui_wgpu::wgpu;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::DX12, ..wgpu::InstanceDescriptor::new_without_display_handle() });
        let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::DX12));
        if adapters.is_empty() {
            v.push("Direct3D 12: no graphics chip (the window falls back to OpenGL)".into());
        }
        for a in adapters {
            let i = a.get_info();
            let line = crate::drawing::adapter_line("Direct3D 12", &i.name, i.device_type == wgpu::DeviceType::Cpu);
            let driver = format!("{} {}", i.driver, i.driver_info);
            v.push(if driver.trim().is_empty() { line } else { format!("{line}, driver {}", driver.trim()) });
        }
        v.push(format!("Remote session: {}", if crate::drawing::remote_session() { "yes" } else { "no" }));
        v.push(format!("Snagster runs {}", if crate::packaged::is_packaged() { "installed (MSIX)" } else { "portable" }));
        v
    }
}

fn system_lines() -> Vec<String> {
    let mut v = vec![format!("Snagster {} ({} {})", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH)];
    #[cfg(target_os = "linux")]
    {
        if let Some(n) = std::fs::read_to_string("/etc/os-release").ok().as_deref().and_then(os_release_name) {
            v.push(format!("System: {n}"));
        }
        if let Ok(k) = std::fs::read_to_string("/proc/sys/kernel/osrelease") {
            v.push(format!("Kernel: {}", k.trim()));
        }
        if let Some(c) = std::fs::read_to_string("/proc/cpuinfo").ok().as_deref().and_then(cpuinfo_model) {
            v.push(format!("Processor: {c}"));
        }
        let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
        v.push(format!("Desktop session: {}", if session.is_empty() { "unknown" } else { &session }));
    }
    #[cfg(windows)]
    {
        if let Ok(p) = std::env::var("PROCESSOR_IDENTIFIER") {
            v.push(format!("Processor: {p}"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        let sysctl = |k: &str| std::process::Command::new("sysctl").args(["-n", k]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|s| !s.is_empty());
        if let Some(c) = sysctl("machdep.cpu.brand_string") {
            v.push(format!("Processor: {c}"));
        }
        if let Some(m) = sysctl("hw.model") {
            v.push(format!("Model: {m}"));
        }
        if let Ok(o) = std::process::Command::new("sw_vers").arg("-productVersion").output() {
            v.push(format!("macOS {}", String::from_utf8_lossy(&o.stdout).trim()));
        }
    }
    v.push(format!("Processor threads: {}", std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)));
    v.extend(drawing_lines());
    for m in crate::capture::monitors() {
        v.push(format!("Screen: {} x {} at {}, {}", m.width, m.height, m.x, m.y));
    }
    v
}

/// Open the encoder `name` for `codec` at 1280 x 720 and encode 60 pictures: "ok ..." with the
/// speed, or "FAIL ..." with why.
fn check_encoder(codec: Codec, name: &str) -> String {
    let cfg = VideoConfig { width: 1280, height: 720, fps: 30, bitrate_bps: 4_000_000, keyframe_every_s: 2, codec, prefer_hardware: true, low_latency: true };
    let label = format!("{} {name}", codec.label());
    let mut e = match encode::open_named(&cfg, name) {
        Ok(e) => e,
        Err(why) => return format!("FAIL {label}: doesn't start ({why})"),
    };
    let mut rgba = vec![0u8; cfg.rgba_len()];
    let mut out = Vec::new();
    let t = Instant::now();
    for i in 0..60u32 {
        encode::test_picture(&mut rgba, 1280, 720, i);
        match e.encode(&rgba, i as i64 * cfg.frame_us(), i == 0) {
            Ok(p) => out.extend(p),
            Err(why) => return format!("FAIL {label}: stopped at picture {i} ({why})"),
        }
    }
    if let Ok(p) = e.finish() {
        out.extend(p);
    }
    let fps = 60.0 / t.elapsed().as_secs_f64().max(1e-6);
    match out.first() {
        None => format!("FAIL {label}: nothing came out"),
        Some(p) if !p.keyframe || encode::parameter_sets(codec, &p.data).is_empty() => format!("FAIL {label}: the first frame isn't a keyframe with its parameter sets"),
        _ if out.len() < 55 => format!("FAIL {label}: {} of 60 frames came out", out.len()),
        _ => format!("ok   {label}: {fps:.0} fps at 1280x720 (including the picture copy)"),
    }
}

/// Run this program again with `args` and keep what it prints.
fn child(args: &[&str]) -> String {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => return format!("FAIL couldn't find Snagster itself: {e}"),
    };
    match std::process::Command::new(exe).args(args).output() {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).to_string();
            let err = String::from_utf8_lossy(&o.stderr);
            if !err.trim().is_empty() {
                s.push_str(&format!("\n(messages)\n{err}"));
            }
            if !o.status.success() && failures(&s) == 0 {
                s.push_str(&format!("\nFAIL it ended with {}\n", o.status));
            }
            s
        }
        Err(e) => format!("FAIL couldn't start: {e}"),
    }
}

/// Make the report, printing it and writing it to `out` (or `default_path`) line by line, so a
/// test that hangs or is stopped still leaves everything before it. True when nothing failed.
pub fn run(out: Option<PathBuf>) -> bool {
    use std::io::Write;
    let path = out.unwrap_or_else(|| default_path(writable(Path::new("."))));
    let mut file = match std::fs::File::create(&path) {
        Ok(f) => Some(f),
        Err(e) => {
            println!("Couldn't write the report to {}: {e} (it is only printed)", path.display());
            None
        }
    };
    let mut say = |s: &str| {
        println!("{s}");
        if let Some(f) = file.as_mut() {
            let _ = writeln!(f, "{s}");
            let _ = f.flush();
        }
    };
    say("Snagster hardware report");
    say("");
    say("== System");
    system_lines().iter().for_each(|l| say(l));
    say("");
    say("== Graphics chips");
    let hw = encode::describe_hardware();
    if hw.is_empty() {
        say("(the encoders below name them)");
    }
    hw.iter().for_each(|l| say(l));
    say("");
    say("== Encoders, each opened by its name");
    let mut failed = 0;
    for (codec, n) in encode::available() {
        let line = check_encoder(codec, &n.name);
        failed += failures(&line);
        say(&format!("{line} [{}]", if n.hardware { "graphics chip" } else { "processor" }));
    }
    for (title, args) in [("Recording test (--video-test quick)", ["--video-test", "quick"]), ("Streaming test (--stream-test quick)", ["--stream-test", "quick"])] {
        say("");
        say(&format!("== {title}"));
        let out = child(&args);
        failed += failures(&out);
        out.lines().for_each(|l| say(l));
    }
    say("");
    say(&if failed == 0 { "All checks passed.".to_string() } else { format!("{failed} check(s) failed: the FAIL lines above say which.") });
    if file.is_some() {
        println!("The report is in {}", std::fs::canonicalize(&path).unwrap_or(path).display());
    }
    failed == 0
}
/// Whether a file can be made in `dir`.
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".snagster-write-check-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_details_are_read_from_linuxs_files() {
        let os = "NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\n";
        assert_eq!(os_release_name(os).as_deref(), Some("Ubuntu 24.04.1 LTS"));
        assert_eq!(os_release_name("NAME=x\n"), None);
        assert_eq!(os_release_name("PRETTY_NAME=\"\"\n"), None);
        let cpu = "processor\t: 0\nvendor_id\t: AuthenticAMD\nmodel name\t: AMD Ryzen 7 7840U w/ Radeon 780M Graphics\n";
        assert_eq!(cpuinfo_model(cpu).as_deref(), Some("AMD Ryzen 7 7840U w/ Radeon 780M Graphics"));
        assert_eq!(cpuinfo_model("Hardware : x\n"), None);
    }

    #[test]
    fn failures_are_counted_from_the_tests_output() {
        assert_eq!(failures("ok   a\nFAIL b: x\n  FAIL c\nnot FAIL\n"), 2);
        assert_eq!(failures(""), 0);
    }

    #[test]
    fn the_report_goes_to_the_current_folder_or_else_temp() {
        assert_eq!(default_path(true), PathBuf::from("snagster-hw-report.txt"));
        assert!(default_path(false).starts_with(std::env::temp_dir()));
        assert!(writable(&std::env::temp_dir()));
        assert!(!writable(Path::new("/this/folder/does/not/exist")));
    }

    /// Snagster's own encoder is on every system: the check opens it by name and times it.
    #[test]
    fn snagsters_own_encoder_passes_the_check() {
        let line = check_encoder(Codec::H264, "OpenH264 (software)");
        assert!(line.starts_with("ok   H.264 OpenH264 (software): "), "{line}");
        assert!(check_encoder(Codec::Hevc, "OpenH264 (software)").starts_with("FAIL HEVC OpenH264 (software): doesn't start"));
        assert!(check_encoder(Codec::H264, "No Such Encoder").contains("isn't on this computer"));
    }
}