//! One program's sound (Studio's "Program sound"): which programs play sound, and a lane that
//! captures every running copy of one program, keeping to real time.
//!
//! A program is known by a key that stays the same across restarts, so a scene remembers it and
//! its sound comes in whenever it runs: the exe name on Windows ("spotify.exe" as "spotify"), the
//! program PipeWire reports on Linux (its binary, else its application name), the bundle id on
//! macOS ("com.spotify.client"). Keys are compared without case and without ".exe".
//!
//! Each system captures with its own interface, written here from the system's documentation:
//! Windows process loopback (Windows 10 version 2004 and later, `programs_win.rs`), PipeWire
//! (`programs_linux.rs`), ScreenCaptureKit (macOS 13 and later, `programs_mac.rs`). A capture
//! hands its samples to a `Gather`, which adds the program's copies together and gives the lane
//! exactly real time, silence while the program isn't running.

use super::{CHANNELS, Feed, RATE};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(any(windows, target_os = "macos", test))]
use std::time::Duration;
use std::time::Instant;

/// A program that plays sound (or has opened sound and is silent now).
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Program {
    /// The key a scene keeps (`key`).
    pub name: String,
    /// What people call it ("Spotify", "Google Chrome").
    pub label: String,
    /// Playing sound right now (false: running, sound opened, silent).
    pub playing: bool,
}

/// Why this computer can't capture one program's sound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unsupported {
    /// Windows older than 10 version 2004.
    #[cfg_attr(not(windows), allow(dead_code))] // each system meets only its own reasons
    OldWindows,
    /// macOS older than 13.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    OldMac,
    /// Linux without a PipeWire sound service.
    NoPipeWire,
    /// The Flatpak sandbox doesn't let Snagster reach PipeWire.
    Flatpak,
    /// macOS: Screen Recording permission not given.
    Permission,
}

/// The Flatpak's app id, for the override command.
pub const FLATPAK_APP: &str = "io.github.SoloSaftis.Snagster";

impl Unsupported {
    /// The sentence the interface shows.
    pub fn text(&self) -> String {
        use crate::i18n::{t, tf};
        match self {
            Unsupported::OldWindows => t("Sound from one program needs Windows 10 version 2004 or later. Everything else here still works.").into(),
            Unsupported::OldMac => t("Sound from one program needs macOS 13 Ventura or later. Everything else here still works.").into(),
            Unsupported::NoPipeWire => t("Sound from one program needs PipeWire, the sound service most Linux systems now use, and this system isn't running it. Everything else here still works.").into(),
            Unsupported::Flatpak => t("The Flatpak version of Snagster isn't allowed to hear other programs. To allow it, run this once in a terminal, then restart Snagster:").into(),
            Unsupported::Permission => tf(
                "To hear other programs, Snagster needs permission. Turn on Snagster in System Settings > Privacy & Security > {pane}, then quit and reopen Snagster.",
                &[("pane", mac_pane())],
            ),
        }
    }

    /// A command that fixes it, when there is one (the Flatpak override).
    pub fn command(&self) -> Option<String> {
        (*self == Unsupported::Flatpak).then(|| format!("flatpak override --user --filesystem=xdg-run/pipewire-0 {FLATPAK_APP}"))
    }

    /// A short word for MCP answers.
    pub fn code(&self) -> &'static str {
        match self {
            Unsupported::OldWindows => "old_windows",
            Unsupported::OldMac => "old_macos",
            Unsupported::NoPipeWire => "no_pipewire",
            Unsupported::Flatpak => "flatpak_sandbox",
            Unsupported::Permission => "needs_permission",
        }
    }
}

/// The macOS settings pane that holds the screen permission ("Screen & System Audio Recording"
/// since macOS 15).
fn mac_pane() -> &'static str {
    if mac_major() >= 15 { crate::i18n::t("Screen & System Audio Recording") } else { crate::i18n::t("Screen Recording") }
}

#[cfg(target_os = "macos")]
fn mac_major() -> u32 {
    super::programs_mac::major()
}

#[cfg(not(target_os = "macos"))]
fn mac_major() -> u32 {
    0
}

/// A program's key: trimmed, lower case, without ".exe".
pub fn key(raw: &str) -> String {
    let k = raw.trim().to_lowercase();
    match k.strip_suffix(".exe") {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => k,
    }
}

/// Whether `candidate` (as a system names it) is the program `wanted` (a key).
pub fn same(wanted: &str, candidate: &str) -> bool {
    !wanted.is_empty() && key(candidate) == key(wanted)
}

/// A name people read for a program known only by its exe or binary: "spotify.exe" → "Spotify".
pub fn label_from_key(raw: &str) -> String {
    let k = key(raw);
    let mut c = k.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// One list per program: entries with the same key become one (playing when any copy plays,
/// the label of the first that has one), playing ones first, each group A to Z. Snagster
/// itself and nameless entries are left out.
pub fn tidy(found: Vec<Program>) -> Vec<Program> {
    let own = own_key();
    let mut out: Vec<Program> = Vec::new();
    for mut p in found {
        p.name = key(&p.name);
        if p.name.is_empty() || p.name == own {
            continue;
        }
        if p.label.trim().is_empty() {
            p.label = label_from_key(&p.name);
        }
        match out.iter_mut().find(|x| x.name == p.name) {
            Some(x) => x.playing |= p.playing,
            None => out.push(p),
        }
    }
    out.sort_by(|a, b| b.playing.cmp(&a.playing).then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase())).then_with(|| a.name.cmp(&b.name)));
    out
}

/// Snagster's own key (never offered: it would hear its own preview).
fn own_key() -> String {
    #[cfg(target_os = "macos")]
    {
        "app.snagster".into()
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| key(&n.to_string_lossy()))).unwrap_or_else(|| "snagster".into())
    }
}

/// Whether this computer can capture one program's sound (`Err` says why not).
pub fn support() -> Result<(), Unsupported> {
    #[cfg(windows)]
    return super::programs_win::support();
    #[cfg(target_os = "linux")]
    return super::programs_linux::support();
    #[cfg(target_os = "macos")]
    return super::programs_mac::support();
    #[allow(unreachable_code)]
    Err(Unsupported::NoPipeWire)
}

/// The programs that have sound open now (empty when this computer can't capture them).
pub fn list() -> Vec<Program> {
    if support().is_err() {
        return Vec::new();
    }
    #[cfg(windows)]
    let found = super::programs_win::list();
    #[cfg(target_os = "linux")]
    let found = super::programs_linux::list();
    #[cfg(target_os = "macos")]
    let found = super::programs_mac::list();
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    let found = Vec::new();
    tidy(found)
}

/// Start the lane for program `name` (a key): silence until it runs, its sound while it does.
pub(super) fn spawn(name: String, feed: Arc<Feed>, stop: Arc<AtomicBool>) -> Result<std::thread::JoinHandle<()>, String> {
    support().map_err(|u| u.text())?;
    feed.waiting.store(true, Ordering::Relaxed);
    std::thread::Builder::new()
        .name("snagster-program-sound".into())
        .spawn(move || {
            #[cfg(windows)]
            let r = super::programs_win::run(&name, &feed, &stop);
            #[cfg(target_os = "linux")]
            let r = super::programs_linux::run(&name, &feed, &stop);
            #[cfg(target_os = "macos")]
            let r = super::programs_mac::run(&name, &feed, &stop);
            #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
            let r: Result<(), String> = Err(String::new());
            if let Err(e) = r
                && let Ok(mut slot) = feed.error.lock()
            {
                *slot = Some(e);
            }
        })
        .map_err(|e| e.to_string())
}

/// How far behind real time the lane runs, so copies whose samples arrive a little late are
/// still added in time (60 ms).
pub(super) const LAG_FRAMES: u64 = RATE as u64 * 60 / 1000;
/// The most a copy may hold before its oldest samples are dropped (it ran ahead: 300 ms).
const MAX_HELD: usize = (RATE * CHANNELS * 300 / 1000) as usize;
/// How often a lane looks for new or ended copies of its program.
#[cfg(any(windows, target_os = "macos"))]
pub(super) const LOOK_EVERY: Duration = Duration::from_secs(2);

/// The copies of one program, added together and handed to the lane at exactly real time.
pub(super) struct Gather {
    start: Instant,
    /// Stereo frames handed over so far.
    written: u64,
    /// Samples (interleaved stereo) waiting from each running copy, by the copy's own id.
    parts: Vec<(u64, VecDeque<i16>)>,
}

impl Gather {
    pub fn new(start: Instant) -> Gather {
        Gather { start, written: 0, parts: Vec::new() }
    }

    /// Samples from copy `id` (added as a new copy the first time).
    pub fn put(&mut self, id: u64, samples: &[i16]) {
        let q = match self.parts.iter().position(|(i, _)| *i == id) {
            Some(i) => &mut self.parts[i].1,
            None => {
                self.parts.push((id, VecDeque::new()));
                &mut self.parts.last_mut().expect("just pushed").1
            }
        };
        q.extend(samples.iter().copied());
        if q.len() > MAX_HELD {
            // MAX_HELD is whole frames, so whole frames go.
            let extra = q.len() - MAX_HELD;
            q.drain(..extra);
        }
    }

    /// Copy `id` ended: what it still held is dropped.
    pub fn end(&mut self, id: u64) {
        self.parts.retain(|(i, _)| *i != id);
    }

    /// The copies being added.
    #[cfg(test)]
    pub fn copies(&self) -> usize {
        self.parts.len()
    }

    /// Hand the lane what is due by `now`: every copy added together, silence where none has
    /// anything.
    pub fn tick(&mut self, now: Instant, feed: &Feed) {
        let due = (now.saturating_duration_since(self.start).as_secs_f64() * RATE as f64) as u64;
        let due = due.saturating_sub(LAG_FRAMES);
        if due <= self.written {
            return;
        }
        let frames = (due - self.written) as usize;
        let out = mix(&mut self.parts, frames);
        feed.push(&out);
        self.written = due;
    }
}

/// `frames` stereo frames from every part added together (clipped), taken from the front of
/// each; a part with too few is padded with silence.
fn mix(parts: &mut [(u64, VecDeque<i16>)], frames: usize) -> Vec<i16> {
    let n = frames * CHANNELS as usize;
    let mut acc = vec![0i32; n];
    for (_, q) in parts.iter_mut() {
        let k = n.min(q.len());
        for (a, s) in acc.iter_mut().zip(q.drain(..k)) {
            *a += s as i32;
        }
    }
    acc.into_iter().map(|s| s.clamp(i16::MIN as i32, i16::MAX as i32) as i16).collect()
}

/// Float samples (-1..1) to 16-bit, clipped.
#[cfg(any(target_os = "macos", test))]
pub(super) fn to_i16(x: f32) -> i16 {
    (x * 32767.0).round().clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

/// How a system hands over its samples (macOS says so with each buffer).
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug)]
pub(super) struct Layout {
    pub channels: usize,
    /// 32-bit float (else 16-bit integer).
    pub float: bool,
    /// Each channel's samples one after the other (else interleaved).
    pub planar: bool,
}

/// `frames` frames of native-endian samples as 16-bit stereo: the first two channels, one
/// channel on both sides. Too few bytes: what is there; an unknown layout: nothing.
#[cfg(any(target_os = "macos", test))]
pub(super) fn to_stereo(bytes: &[u8], frames: usize, l: Layout) -> Vec<i16> {
    let size = if l.float { 4 } else { 2 };
    if l.channels == 0 || frames == 0 {
        return Vec::new();
    }
    let frames = frames.min(bytes.len() / (size * l.channels));
    let at = |frame: usize, ch: usize| -> i16 {
        let i = if l.planar { ch * frames + frame } else { frame * l.channels + ch } * size;
        if l.float { to_i16(f32::from_ne_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]])) } else { i16::from_ne_bytes([bytes[i], bytes[i + 1]]) }
    };
    let right = if l.channels > 1 { 1 } else { 0 };
    let mut out = Vec::with_capacity(frames * 2);
    for f in 0..frames {
        out.push(at(f, 0));
        out.push(at(f, right));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_ignore_case_spaces_and_exe() {
        assert_eq!(key(" Spotify.EXE "), "spotify");
        assert_eq!(key("com.spotify.client"), "com.spotify.client");
        assert_eq!(key("firefox"), "firefox");
        assert_eq!(key(".exe"), ".exe", "nothing before .exe: left as it is");
        assert_eq!(key(""), "");
        assert!(same("spotify", "Spotify.exe"));
        assert!(same("spotify", "SPOTIFY"));
        assert!(!same("spotify", "spotifyhelper.exe"));
        assert!(!same("", ""), "an empty key matches nothing");
    }

    #[test]
    fn a_program_known_by_its_exe_gets_a_readable_name() {
        assert_eq!(label_from_key("spotify.exe"), "Spotify");
        assert_eq!(label_from_key("vlc"), "Vlc");
        assert_eq!(label_from_key("ölspiel.exe"), "Ölspiel");
        assert_eq!(label_from_key(""), "");
    }

    fn p(name: &str, label: &str, playing: bool) -> Program {
        Program { name: name.into(), label: label.into(), playing }
    }

    /// Several copies of a program are one line; playing ones come first; Snagster itself and
    /// nameless entries are left out.
    #[test]
    fn the_list_has_one_line_per_program_playing_first() {
        let own = own_key();
        let got = tidy(vec![
            p("chrome.exe", "Google Chrome", false),
            p("Chrome.exe", "", true),
            p("zoom.exe", "Zoom", false),
            p("spotify.exe", "", true),
            p(&own, "Snagster", true),
            p("", "Nothing", true),
            p("audacity.exe", "Audacity", false),
        ]);
        let names: Vec<(&str, &str, bool)> = got.iter().map(|p| (p.name.as_str(), p.label.as_str(), p.playing)).collect();
        assert_eq!(names, [("chrome", "Google Chrome", true), ("spotify", "Spotify", true), ("audacity", "Audacity", false), ("zoom", "Zoom", false)]);
    }

    #[test]
    fn copies_are_added_together_and_clipped() {
        let mut parts = vec![(1, VecDeque::from(vec![1000i16, -1000, 30000, -30000])), (2, VecDeque::from(vec![5i16, 5, 10000, -10000, 7, 7]))];
        assert_eq!(mix(&mut parts, 2), [1005, -995, i16::MAX, i16::MIN]);
        assert_eq!(parts[0].1.len(), 0);
        assert_eq!(parts[1].1.len(), 2, "what wasn't due yet stays");
        assert_eq!(mix(&mut parts, 2), [7, 7, 0, 0], "padded with silence");
        assert_eq!(mix(&mut [], 1), [0, 0], "no copy running: silence");
    }

    /// The lane gets exactly real time (60 ms behind), whether or not the program runs.
    #[test]
    fn the_lane_keeps_to_real_time_with_or_without_the_program() {
        let feed = Feed::new();
        let t0 = Instant::now();
        let mut g = Gather::new(t0);
        g.tick(t0 + Duration::from_millis(50), &feed);
        assert_eq!(feed.queue.lock().unwrap().len(), 0, "nothing is due in the first 60 ms");
        g.tick(t0 + Duration::from_millis(160), &feed);
        assert_eq!(feed.queue.lock().unwrap().len(), 4800 * 2, "100 ms of silence");
        g.put(7, &[100, 200]);
        assert_eq!(g.copies(), 1);
        g.tick(t0 + Duration::from_millis(160), &feed);
        assert_eq!(feed.queue.lock().unwrap().len(), 4800 * 2, "nothing more is due at the same moment");
        g.tick(t0 + Duration::from_millis(1060), &feed);
        let q: Vec<i16> = feed.queue.lock().unwrap().iter().copied().collect();
        assert_eq!(q.len(), 48_000 * 2, "one second in all");
        assert_eq!(&q[9600..9602], [100, 200], "the copy's sound follows the silence");
        g.end(7);
        assert_eq!(g.copies(), 0);
    }

    #[test]
    fn a_copy_that_runs_ahead_is_trimmed_to_whole_frames() {
        let mut g = Gather::new(Instant::now());
        let lots: Vec<i16> = (0..(MAX_HELD + 6) as i32).map(|i| (i % 1000) as i16).collect();
        g.put(1, &lots);
        let q = &g.parts[0].1;
        assert_eq!(q.len(), MAX_HELD);
        assert_eq!((q[0], q[1]), (lots[6], lots[7]), "whole frames dropped from the front, left stays left");
        assert_eq!(*q.back().unwrap(), *lots.last().unwrap(), "the newest are kept");
    }

    #[test]
    fn float_sound_is_turned_into_16_bit_stereo() {
        assert_eq!(to_i16(1.0), 32767);
        assert_eq!(to_i16(-2.0), i16::MIN);
        assert_eq!(to_i16(0.0), 0);
        let f = |v: &[f32]| v.iter().flat_map(|x| x.to_ne_bytes()).collect::<Vec<u8>>();
        let planar = Layout { channels: 2, float: true, planar: true };
        // Left 0.5, -0.5 then right 0.25, 1.0 (macOS's usual layout).
        assert_eq!(to_stereo(&f(&[0.5, -0.5, 0.25, 1.0]), 2, planar), [16384, 8192, -16384, 32767]);
        let mixed = Layout { planar: false, ..planar };
        assert_eq!(to_stereo(&f(&[0.5, 0.25, -0.5, 1.0]), 2, mixed), [16384, 8192, -16384, 32767]);
        let mono = Layout { channels: 1, float: true, planar: true };
        assert_eq!(to_stereo(&f(&[0.5]), 1, mono), [16384, 16384], "one channel on both sides");
        let six = Layout { channels: 6, float: false, planar: false };
        let ints: Vec<u8> = [1i16, 2, 3, 4, 5, 6].iter().flat_map(|x| x.to_ne_bytes()).collect();
        assert_eq!(to_stereo(&ints, 1, six), [1, 2], "the first two channels");
        assert_eq!(to_stereo(&ints, 5, six), [1, 2], "never past the bytes there are");
        assert!(to_stereo(&ints, 1, Layout { channels: 0, ..six }).is_empty());
    }

    #[test]
    fn every_reason_has_a_sentence_and_a_code() {
        let all = [Unsupported::OldWindows, Unsupported::OldMac, Unsupported::NoPipeWire, Unsupported::Flatpak, Unsupported::Permission];
        let mut codes: Vec<&str> = all.iter().map(|u| u.code()).collect();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), all.len());
        for u in &all {
            assert!(u.text().len() > 30, "{u:?}");
            assert!(!u.text().contains('{'), "placeholders filled: {u:?}");
        }
        assert_eq!(Unsupported::Flatpak.command().unwrap(), format!("flatpak override --user --filesystem=xdg-run/pipewire-0 {FLATPAK_APP}"));
        assert_eq!(Unsupported::OldMac.command(), None);
    }
}
