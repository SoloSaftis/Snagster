//! Sound for recordings and streams: what the computer plays, microphones and sound files,
//! mixed to 48 kHz stereo 16-bit samples.
//!
//! A `Capture` has lanes, one per input, each on its own thread, keeping to real time (when an
//! input is silent, silence is filled in, so sound and picture stay in step). A normal
//! recording has the computer sound and/or one microphone (`Capture::start`). Studio's scenes
//! have up to six inputs each (`Capture::start_inputs` with every input any scene uses); the
//! scene mix (`set_scene_mix`, set from the current scene) says which are heard and how loud,
//! and a lane fades over `FADE_MS` when the scene changes rather than cutting.
//!
//! Windows: WASAPI in shared mode (loopback for the computer sound), Windows converting each
//! device's format to 48 kHz stereo 16-bit. Sound files (WAV, `wav.rs`) play on every system.
//! Computer sound and microphones are Windows-only so far.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

mod programs;
#[cfg(target_os = "linux")]
mod programs_linux;
#[cfg(target_os = "macos")]
mod programs_mac;
#[cfg(windows)]
mod programs_win;
pub use programs::{Program, Unsupported, key as program_key, label_from_key as program_label, settings_link as program_settings_link, list as programs, support as program_support};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const RATE: u32 = 48_000;
pub const CHANNELS: u32 = 2;

/// How long a scene lane takes to fade to a new volume (a scene change, a mute).
pub const FADE_MS: u32 = 150;
const FADE_FRAMES: f32 = (RATE * FADE_MS / 1000) as f32;

/// A mixer (Studio's old two sliders): volume 0..2 as f32 bits and mute, for the computer sound
/// and the microphone of a normal recording.
struct Mixer {
    gain: [AtomicU32; 2],
    mute: [AtomicBool; 2],
}

impl Mixer {
    const fn new() -> Mixer {
        Mixer { gain: [AtomicU32::new(0x3f80_0000), AtomicU32::new(0x3f80_0000)], mute: [AtomicBool::new(false), AtomicBool::new(false)] }
    }

    #[cfg(test)]
    fn set(&self, system: (f32, bool), mic: (f32, bool)) {
        for (i, (g, m)) in [system, mic].into_iter().enumerate() {
            self.gain[i].store(g.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
            self.mute[i].store(m, Ordering::Relaxed);
        }
    }

    /// (volume, muted) of slot `i` (0 computer sound, 1 microphone).
    fn get(&self, i: usize) -> (f32, bool) {
        (f32::from_bits(self.gain[i].load(Ordering::Relaxed)), self.mute[i].load(Ordering::Relaxed))
    }
}

/// The mixer normal recordings use. Captures hold a reference to it (tests give theirs a
/// private one, so tests that start Studio or a stream can't change another test's sums).
static MIXER: Mixer = Mixer::new();

/// Set the normal recordings' mixer: (volume, muted) for the computer sound and the microphone
/// (normal recordings keep it at full volume; tests check it is applied).
#[cfg(test)]
pub fn set_mix(system: (f32, bool), mic: (f32, bool)) {
    MIXER.set(system, mic);
}

/// One input of a Studio scene's sound.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SoundInput {
    /// What the computer plays.
    Computer,
    /// A microphone, by its device id ("" the computer's default one).
    Microphone { id: String },
    /// A WAV file, played over and over.
    File { path: String },
    /// One program's sound, by its key (`programs::key`): every running copy of it, silence
    /// while it isn't running.
    Program { name: String },
}

impl SoundInput {
    /// Whether this computer can capture it (sound files play everywhere).
    pub fn supported(&self) -> bool {
        match self {
            SoundInput::File { .. } => true,
            SoundInput::Program { .. } => programs::support().is_ok(),
            _ => AudioSource::supported(),
        }
    }
}

/// What Studio's current scene wants heard: (input, volume 0..2, muted). Inputs not listed are
/// silent.
struct SceneMix {
    lanes: Mutex<Vec<(SoundInput, f32, bool)>>,
}

impl SceneMix {
    const fn new() -> SceneMix {
        SceneMix { lanes: Mutex::new(Vec::new()) }
    }

    fn set(&self, lanes: Vec<(SoundInput, f32, bool)>) {
        if let Ok(mut g) = self.lanes.lock() {
            *g = lanes.into_iter().map(|(i, v, m)| (i, v.clamp(0.0, 2.0), m)).collect();
        }
    }

    /// (volume, muted) for `input`: muted when the scene doesn't have it.
    fn get(&self, input: &SoundInput) -> (f32, bool) {
        self.lanes.lock().ok().and_then(|g| g.iter().find(|(i, ..)| i == input).map(|(_, v, m)| (*v, *m))).unwrap_or((0.0, true))
    }
}

static SCENE_MIX: SceneMix = SceneMix::new();

/// Set what the current scene wants heard (Studio does this every frame; a stream or recording
/// of Studio follows it at once, fading).
pub fn set_scene_mix(lanes: Vec<(SoundInput, f32, bool)>) {
    SCENE_MIX.set(lanes);
}

/// Which sound goes into a normal recording.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AudioSource {
    /// No sound.
    None,
    /// What the computer plays.
    #[default]
    System,
    /// A microphone.
    Microphone,
    /// Both, mixed.
    Both,
}

impl AudioSource {
    pub const ALL: [AudioSource; 4] = [AudioSource::None, AudioSource::System, AudioSource::Microphone, AudioSource::Both];

    pub fn label(self) -> &'static str {
        match self {
            AudioSource::None => "No sound",
            AudioSource::System => "Computer sound",
            AudioSource::Microphone => "Microphone",
            AudioSource::Both => "Computer sound and microphone",
        }
    }

    pub fn system(self) -> bool {
        matches!(self, AudioSource::System | AudioSource::Both)
    }

    pub fn mic(self) -> bool {
        matches!(self, AudioSource::Microphone | AudioSource::Both)
    }

    pub fn parse(s: &str) -> Option<AudioSource> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "off" | "no" | "silent" => Some(AudioSource::None),
            "system" | "computer" | "desktop" => Some(AudioSource::System),
            "mic" | "microphone" => Some(AudioSource::Microphone),
            "both" | "all" => Some(AudioSource::Both),
            _ => None,
        }
    }

    /// Whether this computer's Snagster can capture the computer sound and microphones.
    pub fn supported() -> bool {
        cfg!(windows)
    }
}

/// A microphone.
#[derive(Clone, Debug, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
}

/// One running input: samples waiting to be taken, its loudness and volume.
struct Feed {
    queue: Mutex<VecDeque<i16>>,
    /// Loudest sample of the latest chunk, 0..1 (f32 bits).
    peak: AtomicU32,
    /// Volume 0..2 (f32 bits).
    gain: AtomicU32,
    muted: AtomicBool,
    error: Mutex<Option<String>>,
    /// The input has started, or failed (then `error` says why).
    settled: AtomicBool,
    /// A program's lane while the program isn't running.
    waiting: AtomicBool,
}

impl Feed {
    fn new() -> Arc<Feed> {
        Arc::new(Feed {
            queue: Mutex::new(VecDeque::new()),
            peak: AtomicU32::new(0),
            gain: AtomicU32::new(1.0f32.to_bits()),
            muted: AtomicBool::new(false),
            error: Mutex::new(None),
            settled: AtomicBool::new(false),
            waiting: AtomicBool::new(false),
        })
    }

    /// The input has started.
    fn settle(&self) {
        self.settled.store(true, Ordering::Release);
    }

    /// The input couldn't start, or stopped, because of `e`.
    fn fail(&self, e: String) {
        *self.error.lock().unwrap_or_else(|p| p.into_inner()) = Some(e);
        self.settle();
    }

    fn settled(&self) -> bool {
        self.settled.load(Ordering::Acquire)
    }

    fn push(&self, samples: &[i16]) {
        let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0) as f32 / 32768.0;
        self.peak.store(peak.to_bits(), Ordering::Relaxed);
        if let Ok(mut q) = self.queue.lock() {
            q.extend(samples.iter().copied());
            // Never hold more than 5 seconds (nobody is taking them).
            let max = (RATE * CHANNELS * 5) as usize;
            if q.len() > max {
                let extra = q.len() - max;
                q.drain(..extra);
            }
        }
    }

    #[cfg(any(windows, test))]
    fn silence(&self, n: usize) {
        self.peak.store(0, Ordering::Relaxed);
        if let Ok(mut q) = self.queue.lock() {
            q.extend(std::iter::repeat_n(0i16, n));
        }
    }

    fn error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|e| e.clone())
    }
}

/// How a lane's volume is set.
#[derive(Clone, Copy, Debug, PartialEq)]
enum MixBy {
    /// A normal recording's slider (0 computer sound, 1 microphone): no fading.
    Slot(usize),
    /// The current scene (`set_scene_mix`): fades.
    Scene,
}

/// One input of a capture.
struct Lane {
    input: SoundInput,
    feed: Arc<Feed>,
    mix: MixBy,
    /// The volume applied at the end of the last take (f32 bits), where the next fade starts.
    now: AtomicU32,
}

/// Live sound from the chosen inputs.
pub struct Capture {
    lanes: Vec<Lane>,
    stop: Arc<AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
    mixer: &'static Mixer,
    scene: &'static SceneMix,
    /// Inputs that couldn't start, and why (the others still run).
    notes: Vec<String>,
}

impl Capture {
    fn empty() -> Capture {
        Capture { lanes: Vec::new(), stop: Arc::new(AtomicBool::new(false)), threads: Vec::new(), mixer: &MIXER, scene: &SCENE_MIX, notes: Vec::new() }
    }

    fn add(&mut self, input: SoundInput, mix: MixBy) -> Result<(), String> {
        let feed = Feed::new();
        let t = match &input {
            SoundInput::Computer => spawn_device(None, feed.clone(), self.stop.clone())?,
            SoundInput::Microphone { id } => spawn_device(Some(id.clone()), feed.clone(), self.stop.clone())?,
            SoundInput::File { path } => spawn_file(path.clone(), feed.clone(), self.stop.clone())?,
            SoundInput::Program { name } => programs::spawn(name.clone(), feed.clone(), self.stop.clone())?,
        };
        self.threads.push(t);
        self.lanes.push(Lane { input, feed, mix, now: AtomicU32::new(0) });
        Ok(())
    }

    /// Start capturing for a normal recording (`mic` is a device id, empty for the default
    /// microphone). Fails when a device can't start.
    pub fn start(source: AudioSource, mic: &str) -> Result<Capture, String> {
        let mut cap = Capture::empty();
        if source == AudioSource::None {
            return Ok(cap);
        }
        if !AudioSource::supported() {
            return Err("Recording sound isn't available on this system yet.".into());
        }
        if source.system() {
            cap.add(SoundInput::Computer, MixBy::Slot(0))?;
        }
        if source.mic() {
            cap.add(SoundInput::Microphone { id: mic.to_string() }, MixBy::Slot(1))?;
        }
        // Wait for the devices to start or report a failure (on the recording's, replay's or
        // stream's own thread). A fixed moment let a device that failed late record silence.
        let feeds: Vec<Arc<Feed>> = cap.lanes.iter().map(|l| l.feed.clone()).collect();
        wait_settled(&feeds, START_WAIT);
        if let Some(e) = cap.lanes.iter().find_map(|l| l.feed.error()) {
            return Err(e);
        }
        Ok(cap)
    }

    /// Start every input Studio's scenes use, mixed by the current scene (`set_scene_mix`).
    /// An input that can't start (a microphone unplugged, a file gone, computer sound off
    /// Windows) is left out and noted (`notes`); the others still run. Blocks until every input
    /// has started or failed (at most `START_WAIT`): for a stream's or a recording's own thread.
    pub fn start_inputs(inputs: &[SoundInput]) -> Capture {
        Self::start_inputs_within(inputs, START_WAIT)
    }

    /// `start_inputs` waiting at most `most`, for the window's own thread (Studio's meters),
    /// which mustn't stop drawing for a device that is slow to open.
    pub fn start_inputs_within(inputs: &[SoundInput], most: Duration) -> Capture {
        let mut cap = Capture::empty();
        let mut seen: Vec<&SoundInput> = Vec::new();
        for input in inputs {
            if seen.contains(&input) {
                continue;
            }
            seen.push(input);
            if let SoundInput::Program { .. } = input
                && let Err(why) = programs::support()
            {
                cap.notes.push(format!("{}: {}", input_label(input), why.text()));
                continue;
            }
            if !input.supported() {
                cap.notes.push(format!("{}: sound from devices isn't available on this system yet", input_label(input)));
                continue;
            }
            if let Err(e) = cap.add(input.clone(), MixBy::Scene) {
                cap.notes.push(format!("{}: {e}", input_label(input)));
            }
        }
        // Each input says whether it started; one that can't (a missing file, a device in use)
        // is left out with a note. Waited for, not a fixed moment: on a busy computer a file
        // that isn't there could take longer than 60 ms to say so, and was then kept silently.
        let feeds: Vec<Arc<Feed>> = cap.lanes.iter().map(|l| l.feed.clone()).collect();
        wait_settled(&feeds, most);
        cap.drop_failed();
        cap
    }

    /// Leave out the inputs that have failed, each with a note; whether there were any. Studio's
    /// meters call it every frame too, since they wait only briefly when they start: an input
    /// that fails later (a big file that turns out broken) still gets its note.
    pub fn drop_failed(&mut self) -> bool {
        let failed: Vec<(usize, String)> = self.lanes.iter().enumerate().filter_map(|(i, l)| l.feed.error().map(|e| (i, format!("{}: {e}", input_label(&l.input))))).collect();
        let any = !failed.is_empty();
        for (i, note) in failed.into_iter().rev() {
            self.lanes.remove(i);
            self.notes.push(note);
        }
        any
    }

    /// The inputs that couldn't start, and why.
    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    pub fn is_silent(&self) -> bool {
        self.lanes.is_empty()
    }

    /// Take `frames` stereo frames, mixed (silence where an input has nothing yet).
    pub fn take(&self, frames: usize) -> Vec<i16> {
        let n = frames * CHANNELS as usize;
        let mut out = vec![0f32; n];
        for lane in &self.lanes {
            let (mix_gain, mix_muted) = match lane.mix {
                MixBy::Slot(i) => self.mixer.get(i),
                MixBy::Scene => self.scene.get(&lane.input),
            };
            let f = &lane.feed;
            let target = if mix_muted || f.muted.load(Ordering::Relaxed) { 0.0 } else { f32::from_bits(f.gain.load(Ordering::Relaxed)) * mix_gain };
            // Scene lanes move to the new volume over FADE_MS; slots jump.
            let mut g = if lane.mix == MixBy::Scene { f32::from_bits(lane.now.load(Ordering::Relaxed)) } else { target };
            let step = (target - g) / FADE_FRAMES;
            if let Ok(mut q) = f.queue.lock() {
                let k = n.min(q.len());
                for (i, (o, s)) in out.iter_mut().zip(q.drain(..k)).enumerate() {
                    if i % CHANNELS as usize == 0 {
                        g = fade_step(g, target, step);
                    }
                    *o += s as f32 * g;
                }
            }
            lane.now.store(if lane.mix == MixBy::Scene { g } else { target }.to_bits(), Ordering::Relaxed);
        }
        out.into_iter().map(|s| s.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16).collect()
    }

    /// Drop what has been captured so far (after a pause).
    #[cfg(any(windows, test))]
    pub fn discard(&self) {
        for l in &self.lanes {
            if let Ok(mut q) = l.feed.queue.lock() {
                q.clear();
            }
        }
    }

    #[cfg(test)]
    fn peak_of(&self, want: impl Fn(&SoundInput) -> bool) -> f32 {
        self.lanes.iter().find(|l| want(&l.input)).map(|l| f32::from_bits(l.feed.peak.load(Ordering::Relaxed))).unwrap_or(0.0)
    }

    /// Loudness of (computer sound, microphone), 0..1.
    #[cfg(test)]
    pub fn levels(&self) -> (f32, f32) {
        (self.peak_of(|i| *i == SoundInput::Computer), self.peak_of(|i| matches!(i, SoundInput::Microphone { .. })))
    }

    /// Loudness of `input`, 0..1, and whether it is running.
    pub fn level_of(&self, input: &SoundInput) -> Option<f32> {
        self.lanes.iter().find(|l| l.input == *input).map(|l| f32::from_bits(l.feed.peak.load(Ordering::Relaxed)))
    }

    /// Whether `input` is a program that isn't running now (its lane is silent until it starts).
    pub fn waiting(&self, input: &SoundInput) -> bool {
        self.lanes.iter().any(|l| l.input == *input && l.feed.waiting.load(Ordering::Relaxed))
    }

    #[cfg(test)]
    pub fn has_system(&self) -> bool {
        self.lanes.iter().any(|l| l.input == SoundInput::Computer)
    }

    #[cfg(test)]
    pub fn has_mic(&self) -> bool {
        self.lanes.iter().any(|l| matches!(l.input, SoundInput::Microphone { .. }))
    }
}

/// One fade step from `g` towards `target` by `step`, never past it.
fn fade_step(g: f32, target: f32, step: f32) -> f32 {
    let next = g + step;
    // Lands exactly on the target (adding many small steps drifts a little).
    if (step >= 0.0 && next >= target - 1e-4) || (step < 0.0 && next <= target + 1e-4) { target } else { next }
}

/// What an input is called in notes.
pub(crate) fn input_label(i: &SoundInput) -> String {
    match i {
        SoundInput::Computer => "Computer sound".into(),
        SoundInput::Microphone { id } if id.is_empty() => "Microphone".into(),
        SoundInput::Microphone { .. } => "A microphone".into(),
        SoundInput::File { path } => std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.clone()),
        SoundInput::Program { name } => programs::label_from_key(name),
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// The microphones this computer has.
pub fn microphones() -> Vec<Device> {
    #[cfg(windows)]
    {
        win::microphones().unwrap_or_default()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// How long `start_inputs` waits at most for its inputs to start or fail (a sound device can take
/// a moment to open; a computer busy with other work slows everything down).
const START_WAIT: Duration = Duration::from_secs(3);

/// Wait until every feed has started or failed, or `most` has passed; whether they all did.
fn wait_settled(feeds: &[Arc<Feed>], most: Duration) -> bool {
    let t = std::time::Instant::now();
    loop {
        if feeds.iter().all(|f| f.settled()) {
            return true;
        }
        if t.elapsed() >= most {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Play a WAV file into `feed` in real time, over and over, until `stop`.
fn spawn_file(path: String, feed: Arc<Feed>, stop: Arc<AtomicBool>) -> Result<std::thread::JoinHandle<()>, String> {
    std::thread::Builder::new()
        .name("snagster-sound-file".into())
        .spawn(move || {
            let samples = match crate::wav::read_48k_stereo(std::path::Path::new(&path)) {
                Ok(s) if !s.is_empty() => s,
                Ok(_) => return feed.fail("the file has no sound in it".into()),
                Err(e) => return feed.fail(e),
            };
            feed.settle();
            let start = std::time::Instant::now();
            let (mut written, mut at) = (0u64, 0usize);
            let mut chunk = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                let due = (start.elapsed().as_secs_f64() * RATE as f64) as u64;
                if due > written {
                    let frames = (due - written) as usize;
                    chunk.clear();
                    for _ in 0..frames * CHANNELS as usize {
                        chunk.push(samples[at]);
                        at = (at + 1) % samples.len();
                    }
                    feed.push(&chunk);
                    written = due;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        })
        .map_err(|e| e.to_string())
}

#[cfg(windows)]
fn spawn_device(mic: Option<String>, feed: Arc<Feed>, stop: Arc<AtomicBool>) -> Result<std::thread::JoinHandle<()>, String> {
    std::thread::Builder::new()
        .name(if mic.is_some() { "snagster-mic".into() } else { "snagster-loopback".into() })
        .spawn(move || {
            if let Err(e) = win::run(mic.as_deref(), &feed, &stop) {
                feed.fail(e);
            }
        })
        .map_err(|e| e.to_string())
}

#[cfg(not(windows))]
fn spawn_device(_mic: Option<String>, _feed: Arc<Feed>, _stop: Arc<AtomicBool>) -> Result<std::thread::JoinHandle<()>, String> {
    Err("Recording sound isn't available on this system yet.".into())
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::time::Instant;
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, STGM_READ};
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::core::PCWSTR;

    fn enumerator() -> Result<IMMDeviceEnumerator, String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| format!("sound devices aren't available: {e}"))
        }
    }

    pub fn microphones() -> Result<Vec<Device>, String> {
        let en = enumerator()?;
        let mut out = Vec::new();
        unsafe {
            let list = en.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE).map_err(|e| e.to_string())?;
            let n = list.GetCount().map_err(|e| e.to_string())?;
            for i in 0..n {
                let Ok(dev) = list.Item(i) else { continue };
                let Ok(idp) = dev.GetId() else { continue };
                let id = idp.to_string().unwrap_or_default();
                CoTaskMemFree(Some(idp.0 as *const _));
                let key = PROPERTYKEY { fmtid: PKEY_Device_FriendlyName.fmtid, pid: PKEY_Device_FriendlyName.pid };
                let name = dev.OpenPropertyStore(STGM_READ).and_then(|s| s.GetValue(&key)).map(|v| v.to_string()).unwrap_or_else(|_| "Microphone".into());
                out.push(Device { id, name });
            }
        }
        Ok(out)
    }

    /// Capture one device until `stop`: the default speakers' loopback (`mic` None), the
    /// default microphone (`Some("")`), or a microphone by id.
    pub fn run(mic: Option<&str>, feed: &Feed, stop: &AtomicBool) -> Result<(), String> {
        let en = enumerator()?;
        unsafe {
            let dev = match mic {
                None => en.GetDefaultAudioEndpoint(eRender, eConsole),
                Some("") => en.GetDefaultAudioEndpoint(eCapture, eConsole),
                Some(id) => {
                    let w: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
                    en.GetDevice(PCWSTR(w.as_ptr()))
                }
            }
            .map_err(|e| if mic.is_none() { format!("no speakers or headphones to record from: {e}") } else { format!("no microphone found: {e}") })?;
            let client: IAudioClient = dev.Activate(CLSCTX_ALL, None).map_err(|e| e.to_string())?;
            // Ask Windows for 48 kHz stereo 16-bit, whatever the device runs at.
            let fmt = WAVEFORMATEX {
                wFormatTag: 1, // WAVE_FORMAT_PCM
                nChannels: CHANNELS as u16,
                nSamplesPerSec: RATE,
                nAvgBytesPerSec: RATE * CHANNELS * 2,
                nBlockAlign: (CHANNELS * 2) as u16,
                wBitsPerSample: 16,
                cbSize: 0,
            };
            let mut flags = AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
            if mic.is_none() {
                flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
            }
            client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, &fmt, None).map_err(|e| format!("couldn't open the sound device: {e}"))?;
            let cap: IAudioCaptureClient = client.GetService().map_err(|e| e.to_string())?;
            client.Start().map_err(|e| e.to_string())?;
            feed.settle();
            let start = Instant::now();
            let mut written: u64 = 0;
            let mut buf: Vec<i16> = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
                loop {
                    let n = cap.GetNextPacketSize().unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    let mut data = std::ptr::null_mut();
                    let mut frames = 0u32;
                    let mut bflags = 0u32;
                    if cap.GetBuffer(&mut data, &mut frames, &mut bflags, None, None).is_err() {
                        break;
                    }
                    let count = (frames * CHANNELS) as usize;
                    buf.clear();
                    if bflags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                        buf.resize(count, 0);
                    } else {
                        buf.extend_from_slice(std::slice::from_raw_parts(data as *const i16, count));
                    }
                    let _ = cap.ReleaseBuffer(frames);
                    feed.push(&buf);
                    written += frames as u64;
                }
                // Loopback sends nothing while the computer is silent: keep to real time.
                let due = (start.elapsed().as_secs_f64() * RATE as f64) as u64;
                let slack = (RATE / 10) as u64; // 100 ms
                if due > written + slack {
                    let missing = due - slack / 2 - written;
                    feed.silence((missing * CHANNELS as u64) as usize);
                    written += missing;
                }
            }
            let _ = client.Stop();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_parse_and_say_what_they_record() {
        assert_eq!(AudioSource::parse("both"), Some(AudioSource::Both));
        assert_eq!(AudioSource::parse("Mic"), Some(AudioSource::Microphone));
        assert_eq!(AudioSource::parse("loud"), None);
        assert!(AudioSource::Both.system() && AudioSource::Both.mic());
        assert!(!AudioSource::None.system() && !AudioSource::None.mic());
    }

    #[test]
    fn no_sound_needs_no_device() {
        let c = Capture::start(AudioSource::None, "").unwrap();
        assert!(c.is_silent());
        assert_eq!(c.take(480).len(), 960);
        assert!(c.take(10).iter().all(|s| *s == 0));
    }

    /// A mixer of this test's own: other tests (Studio, streaming) set the global one while
    /// running, which made sums here flaky when it was shared.
    fn own_mixer() -> &'static Mixer {
        Box::leak(Box::new(Mixer::new()))
    }

    /// A capture made of plain feeds (no devices, no threads), with a mixer of its own.
    fn capture(system: Option<Arc<Feed>>, mic: Option<Arc<Feed>>) -> Capture {
        capture_with(system, mic, own_mixer())
    }

    fn capture_with(system: Option<Arc<Feed>>, mic: Option<Arc<Feed>>, mixer: &'static Mixer) -> Capture {
        let mut c = Capture::empty();
        c.mixer = mixer;
        for (feed, input, slot) in [(system, SoundInput::Computer, 0), (mic, SoundInput::Microphone { id: String::new() }, 1)] {
            if let Some(feed) = feed {
                c.lanes.push(Lane { input, feed, mix: MixBy::Slot(slot), now: AtomicU32::new(0) });
            }
        }
        c
    }

    /// A capture of scene lanes (no devices), with a scene mix of its own.
    fn scene_capture(lanes: Vec<(SoundInput, Arc<Feed>)>) -> (Capture, &'static SceneMix) {
        let mix: &'static SceneMix = Box::leak(Box::new(SceneMix::new()));
        let mut c = Capture::empty();
        c.scene = mix;
        for (input, feed) in lanes {
            c.lanes.push(Lane { input, feed, mix: MixBy::Scene, now: AtomicU32::new(0) });
        }
        (c, mix)
    }

    /// A scene hears only its own inputs; switching scenes fades a lane in or out over
    /// FADE_MS instead of cutting.
    #[test]
    fn the_scene_mix_picks_the_inputs_and_fades_between_scenes() {
        let (a, b) = (Feed::new(), Feed::new());
        let mic = SoundInput::Microphone { id: "usb".into() };
        let (c, mix) = scene_capture(vec![(SoundInput::Computer, a.clone()), (mic.clone(), b.clone())]);
        let frames = FADE_FRAMES as usize;
        // Scene 1: only the computer sound, at full volume (fading in from silence).
        mix.set(vec![(SoundInput::Computer, 1.0, false)]);
        a.push(&vec![10_000; frames * 2]);
        b.push(&vec![10_000; frames * 2]);
        let out = c.take(frames);
        assert!(out[0] < 200, "it fades in: {}", out[0]);
        assert_eq!(out[out.len() - 1], 10_000, "and reaches full volume after the fade");
        assert_eq!(queued(&b), 0, "an input the scene doesn't use is still used up");
        // Scene 2: only the microphone, at half volume. The computer sound fades out, the
        // microphone fades in.
        mix.set(vec![(mic.clone(), 0.5, false)]);
        a.push(&vec![10_000; frames * 2]);
        b.push(&vec![10_000; frames * 2]);
        let out = c.take(frames);
        assert!(out[0] > 9_000, "no cut: {}", out[0]);
        assert_eq!(out[out.len() - 1], 5_000);
        // Muted in the scene: silent (after the fade).
        mix.set(vec![(mic, 0.5, true)]);
        b.push(&vec![10_000; frames * 4]);
        c.take(frames);
        assert!(c.take(10).iter().all(|s| *s == 0));
    }

    /// A program's lane is mixed like any other scene input, and says when it waits for the
    /// program to start.
    #[test]
    fn a_program_lane_follows_the_scene_mix_and_reports_waiting() {
        let game = SoundInput::Program { name: "game".into() };
        let f = Feed::new();
        let (c, mix) = scene_capture(vec![(game.clone(), f.clone())]);
        mix.set(vec![(game.clone(), 0.5, false)]);
        let frames = FADE_FRAMES as usize;
        f.push(&vec![8_000; frames * 4]);
        c.take(frames);
        assert_eq!(*c.take(1).last().unwrap(), 4_000, "at half volume after the fade");
        assert!(!c.waiting(&game));
        f.waiting.store(true, Ordering::Relaxed);
        assert!(c.waiting(&game));
        assert!(!c.waiting(&SoundInput::Program { name: "other".into() }), "not in this capture");
        assert_eq!(input_label(&game), "Game");
        assert!(game.supported() == program_support().is_ok());
    }

    /// Where one program's sound can't be captured, its lane is left out with the reason and
    /// the rest still run.
    #[test]
    fn a_program_that_cant_be_captured_is_noted() {
        let Err(why) = program_support() else { return };
        let c = Capture::start_inputs(&[SoundInput::Program { name: "game".into() }]);
        assert!(c.is_silent());
        assert_eq!(c.notes(), [format!("Game: {}", why.text())]);
    }

    #[test]
    fn fading_never_overshoots() {
        assert_eq!(fade_step(0.9, 1.0, 0.5), 1.0);
        assert_eq!(fade_step(0.1, 0.0, -0.5), 0.0);
        assert_eq!(fade_step(0.5, 1.0, 0.25), 0.75);
        assert_eq!(fade_step(1.0, 1.0, 0.0), 1.0);
    }

    /// The flake that showed it: on a busy computer a missing file said so after the fixed
    /// 60 ms start_inputs used to wait, and was kept as a silent input with no note.
    #[test]
    fn starting_waits_for_every_input_to_start_or_fail() {
        let (quick, slow, never) = (Feed::new(), Feed::new(), Feed::new());
        quick.settle();
        let s = slow.clone();
        let late = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            s.fail("gone.wav isn't there".into());
        });
        assert!(wait_settled(&[quick.clone(), slow.clone()], Duration::from_secs(5)), "waited for the slow one");
        assert_eq!(slow.error().as_deref(), Some("gone.wav isn't there"), "its failure is seen");
        late.join().unwrap();
        let t = std::time::Instant::now();
        assert!(!wait_settled(&[quick, never], Duration::from_millis(50)), "one that never answers isn't waited for forever");
        assert!(t.elapsed() < Duration::from_secs(1));
        assert!(wait_settled(&[], Duration::ZERO), "nothing to wait for");
    }

    /// Studio's meters wait only briefly; a file that fails after that still gets its note.
    #[test]
    fn an_input_that_fails_later_is_left_out_with_a_note_then() {
        let dir = std::env::temp_dir().join(format!("snagster-audio-late-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tone = dir.join("tone.wav");
        std::fs::write(&tone, crate::wav::tests::wav_bytes(1, 48_000, 16, &[8_000, -8_000].repeat(2_400))).unwrap();
        let file = SoundInput::File { path: tone.to_string_lossy().to_string() };
        let mut c = Capture::start_inputs(std::slice::from_ref(&file));
        assert_eq!(c.lanes.len(), 1);
        assert!(!c.drop_failed() && c.notes().is_empty(), "a working input stays");
        c.lanes[0].feed.fail("the file was damaged".into());
        assert!(c.drop_failed());
        assert!(c.is_silent());
        assert_eq!(c.notes(), ["tone.wav: the file was damaged"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Inputs that can't start are noted, never fatal; the same input twice runs once.
    #[test]
    fn inputs_that_cant_start_are_noted_and_the_rest_run() {
        let dir = std::env::temp_dir().join(format!("snagster-audio-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tone = dir.join("tone.wav");
        std::fs::write(&tone, crate::wav::tests::wav_bytes(1, 48_000, 16, &[8_000, -8_000].repeat(2_400))).unwrap();
        let file = SoundInput::File { path: tone.to_string_lossy().to_string() };
        let gone = SoundInput::File { path: dir.join("gone.wav").to_string_lossy().to_string() };
        let c = Capture::start_inputs(&[file.clone(), file.clone(), gone]);
        assert_eq!(c.lanes.len(), 1, "the file once; the missing one left out");
        assert!(c.notes().iter().any(|n| n.starts_with("gone.wav: ")), "{:?}", c.notes());
        assert_eq!(c.level_of(&SoundInput::Computer), None);
        // The file plays in real time (waited for: a busy computer runs its thread late).
        let t = std::time::Instant::now();
        while !c.level_of(&file).is_some_and(|l| l > 0.2) && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(c.level_of(&file).is_some_and(|l| l > 0.2), "{:?}", c.level_of(&file));
        drop(c);
        if !AudioSource::supported() {
            let c = Capture::start_inputs(&[SoundInput::Computer]);
            assert!(c.is_silent() && c.notes()[0].contains("isn't available"), "{:?}", c.notes());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_capture_uses_its_own_mixer_not_whatever_another_test_set_globally() {
        let a = Feed::new();
        a.push(&[1000, 1000]);
        let c = capture(Some(a), None);
        // Another test starting Studio sets the global mixer meanwhile.
        let before = (MIXER.get(0), MIXER.get(1));
        set_mix((0.0, true), (0.0, true));
        let got = c.take(1);
        set_mix(before.0, before.1);
        assert_eq!(got, [1000, 1000]);
        // Real captures use the global one.
        assert!(std::ptr::eq(Capture::start(AudioSource::None, "").unwrap().mixer, &MIXER));
    }

    fn queued(f: &Feed) -> usize {
        f.queue.lock().unwrap().len()
    }

    #[test]
    fn every_source_has_a_label_and_parses_its_synonyms() {
        let mut labels: Vec<&str> = AudioSource::ALL.iter().map(|s| s.label()).collect();
        labels.sort();
        labels.dedup();
        assert_eq!(labels.len(), AudioSource::ALL.len());
        for (words, want) in [
            (&["none", "OFF", "no", "silent"][..], AudioSource::None),
            (&["system", "Computer", "desktop"][..], AudioSource::System),
            (&["mic", " microphone "][..], AudioSource::Microphone),
            (&["both", "ALL"][..], AudioSource::Both),
        ] {
            for w in words {
                assert_eq!(AudioSource::parse(w), Some(want), "{w}");
            }
        }
        assert_eq!(AudioSource::parse(""), None);
        assert_eq!(AudioSource::default(), AudioSource::System);
        assert!(AudioSource::System.system() && !AudioSource::System.mic());
        assert!(!AudioSource::Microphone.system() && AudioSource::Microphone.mic());
    }

    #[test]
    fn sources_save_by_name() {
        for s in AudioSource::ALL {
            let j = serde_json::to_string(&s).unwrap();
            assert_eq!(serde_json::from_str::<AudioSource>(&j).unwrap(), s);
        }
        assert_eq!(serde_json::to_string(&AudioSource::Both).unwrap(), "\"Both\"");
    }

    #[test]
    fn sound_is_recorded_only_where_it_is_supported() {
        assert_eq!(AudioSource::supported(), cfg!(windows));
    }

    #[cfg(not(windows))]
    #[test]
    fn off_windows_sound_is_refused_plainly() {
        for s in [AudioSource::System, AudioSource::Microphone, AudioSource::Both] {
            let e = Capture::start(s, "").err().expect("no sound here");
            assert!(e.contains("isn't available"), "{e}");
        }
        assert!(microphones().is_empty());
    }

    #[test]
    fn a_feed_tracks_its_peak_and_keeps_at_most_five_seconds() {
        let f = Feed::new();
        f.push(&[100, -16384, 50, 0]);
        assert_eq!(f32::from_bits(f.peak.load(Ordering::Relaxed)), 0.5);
        f.push(&[i16::MIN, 0]);
        assert_eq!(f32::from_bits(f.peak.load(Ordering::Relaxed)), 1.0, "the loudest negative sample is full scale");
        assert_eq!(queued(&f), 6);
        f.push(&[]);
        assert_eq!(f.peak.load(Ordering::Relaxed), 0, "an empty chunk is silent");
        let max = (RATE * CHANNELS * 5) as usize;
        f.push(&vec![7i16; max]);
        assert_eq!(queued(&f), max, "the oldest samples are dropped");
        assert!(f.queue.lock().unwrap().iter().all(|s| *s == 7));
        f.push(&[1, 2]);
        let q = f.queue.lock().unwrap();
        assert_eq!(q.len(), max);
        assert_eq!((q[max - 2], q[max - 1]), (1, 2), "the newest are kept at the end");
    }

    #[test]
    fn silence_fills_in_zeros() {
        let f = Feed::new();
        f.push(&[9000, 9000]);
        f.silence(4);
        assert_eq!(f.peak.load(Ordering::Relaxed), 0);
        assert_eq!(f.queue.lock().unwrap().iter().copied().collect::<Vec<_>>(), [9000, 9000, 0, 0, 0, 0]);
    }

    #[test]
    fn two_sources_are_added_together_and_clipped() {
        let (a, b) = (Feed::new(), Feed::new());
        a.push(&[1000, -1000, 30000, -30000]);
        b.push(&[234, 66, 10000, -10000]);
        let c = capture(Some(a.clone()), Some(b.clone()));
        assert!(!c.is_silent() && c.has_system() && c.has_mic());
        assert_eq!(c.take(2), [1234, -934, i16::MAX, i16::MIN]);
        assert_eq!(queued(&a) + queued(&b), 0, "what was mixed is gone");
    }

    #[test]
    fn a_short_source_is_padded_with_silence_and_order_is_kept() {
        let a = Feed::new();
        a.push(&[1, 2, 3, 4, 5, 6]);
        let c = capture(Some(a.clone()), None);
        assert!(c.has_system() && !c.has_mic());
        assert_eq!(c.take(1), [1, 2]);
        assert_eq!(c.take(3), [3, 4, 5, 6, 0, 0]);
        assert_eq!(c.take(1), [0, 0]);
        assert!(c.take(0).is_empty());
    }

    #[test]
    fn volume_and_mute_apply_per_source() {
        let (a, b) = (Feed::new(), Feed::new());
        a.gain.store(0.5f32.to_bits(), Ordering::Relaxed);
        b.muted.store(true, Ordering::Relaxed);
        a.push(&[1000, -2000]);
        b.push(&[500, 500]);
        let c = capture(Some(a), Some(b.clone()));
        assert_eq!(c.take(1), [500, -1000]);
        assert_eq!(queued(&b), 0, "a muted source is still used up, so it doesn't fall behind");
    }

    #[test]
    fn the_mixer_sliders_scale_mute_and_are_clamped() {
        let mix = own_mixer();
        let take_one = |sys: i16, mic: i16| {
            let (a, b) = (Feed::new(), Feed::new());
            a.push(&[sys, sys]);
            b.push(&[mic, mic]);
            capture_with(Some(a), Some(b), mix).take(1)
        };
        mix.set((2.0, false), (0.25, false));
        assert_eq!(take_one(1000, 1000), [2250, 2250]);
        mix.set((5.0, false), (-3.0, false));
        assert_eq!(mix.get(0), (2.0, false), "at most 2");
        assert_eq!(mix.get(1), (0.0, false), "at least 0");
        assert_eq!(take_one(1000, 1000), [2000, 2000]);
        mix.set((1.0, true), (1.0, false));
        assert_eq!(take_one(1000, 7), [7, 7], "the system sound is muted");
        mix.set((1.0, false), (1.0, false));
        assert_eq!(take_one(1, 2), [3, 3]);
    }

    #[test]
    fn levels_and_discard() {
        let (a, b) = (Feed::new(), Feed::new());
        a.push(&[8192, 0]);
        b.push(&[0, -32768]);
        let c = capture(Some(a.clone()), Some(b.clone()));
        assert_eq!(c.levels(), (0.25, 1.0));
        assert_eq!(capture(None, None).levels(), (0.0, 0.0));
        c.discard();
        assert_eq!(queued(&a) + queued(&b), 0);
        assert_eq!(c.take(1), [0, 0]);
    }

    #[test]
    fn dropping_a_capture_sets_its_stop_flag() {
        let c = capture(None, None);
        let stop = c.stop.clone();
        drop(c);
        assert!(stop.load(Ordering::Relaxed));
    }
}
