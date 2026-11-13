//! Sound for recordings: what the computer plays (system sound), a microphone, or both, mixed
//! to 48 kHz stereo 16-bit samples.
//!
//! Windows: WASAPI in shared mode (loopback for the system sound), with Windows converting each
//! device's own format to 48 kHz stereo 16-bit. Each source runs on its own thread and keeps to
//! real time: when the system is silent (loopback sends nothing), silence is filled in so sound
//! and picture stay in step. Other systems: no sound yet.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const RATE: u32 = 48_000;
pub const CHANNELS: u32 = 2;

/// The mixer (Studio's volume sliders): volume 0..2 as f32 bits and mute, for the system sound
/// and the microphone. Every recording uses it.
static MIX_GAIN: [AtomicU32; 2] = [AtomicU32::new(0x3f80_0000), AtomicU32::new(0x3f80_0000)];
static MIX_MUTE: [AtomicBool; 2] = [AtomicBool::new(false), AtomicBool::new(false)];

/// Set the mixer: (volume, muted) for the system sound and the microphone.
pub fn set_mix(system: (f32, bool), mic: (f32, bool)) {
    for (i, (g, m)) in [system, mic].into_iter().enumerate() {
        MIX_GAIN[i].store(g.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
        MIX_MUTE[i].store(m, Ordering::Relaxed);
    }
}

/// Which sound goes into a recording.
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

    /// Whether this computer's Snagster can record sound.
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

/// One running sound source: samples waiting to be taken, its loudness and volume.
struct Feed {
    queue: Mutex<VecDeque<i16>>,
    /// Loudest sample of the latest chunk, 0..1 (f32 bits).
    peak: AtomicU32,
    /// Volume 0..2 (f32 bits).
    gain: AtomicU32,
    muted: AtomicBool,
    error: Mutex<Option<String>>,
}

impl Feed {
    fn new() -> Arc<Feed> {
        Arc::new(Feed {
            queue: Mutex::new(VecDeque::new()),
            peak: AtomicU32::new(0),
            gain: AtomicU32::new(1.0f32.to_bits()),
            muted: AtomicBool::new(false),
            error: Mutex::new(None),
        })
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

    fn silence(&self, n: usize) {
        self.peak.store(0, Ordering::Relaxed);
        if let Ok(mut q) = self.queue.lock() {
            q.extend(std::iter::repeat_n(0i16, n));
        }
    }
}

/// Live sound capture from the chosen sources.
pub struct Capture {
    system: Option<Arc<Feed>>,
    mic: Option<Arc<Feed>>,
    stop: Arc<AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl Capture {
    /// Start capturing (`mic` is a device id, empty for the default microphone).
    pub fn start(source: AudioSource, mic: &str) -> Result<Capture, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let mut cap = Capture { system: None, mic: None, stop: stop.clone(), threads: Vec::new() };
        if source == AudioSource::None {
            return Ok(cap);
        }
        if !AudioSource::supported() {
            return Err("Recording sound isn't available on this system yet.".into());
        }
        if source.system() {
            let feed = Feed::new();
            cap.threads.push(spawn_source(None, feed.clone(), stop.clone())?);
            cap.system = Some(feed);
        }
        if source.mic() {
            let feed = Feed::new();
            let id = if mic.is_empty() { Some(String::new()) } else { Some(mic.to_string()) };
            cap.threads.push(spawn_source(id, feed.clone(), stop.clone())?);
            cap.mic = Some(feed);
        }
        // Give the devices a moment to start (and report a failure).
        std::thread::sleep(Duration::from_millis(60));
        for f in [&cap.system, &cap.mic].into_iter().flatten() {
            if let Some(e) = f.error.lock().ok().and_then(|e| e.clone()) {
                return Err(e);
            }
        }
        Ok(cap)
    }

    pub fn is_silent(&self) -> bool {
        self.system.is_none() && self.mic.is_none()
    }

    /// Take `frames` stereo frames, mixed (silence where a source has nothing yet).
    pub fn take(&self, frames: usize) -> Vec<i16> {
        let n = frames * CHANNELS as usize;
        let mut out = vec![0i32; n];
        for (i, f) in [&self.system, &self.mic].into_iter().enumerate() {
            let Some(f) = f else { continue };
            let gain = f32::from_bits(f.gain.load(Ordering::Relaxed)) * f32::from_bits(MIX_GAIN[i].load(Ordering::Relaxed));
            let muted = f.muted.load(Ordering::Relaxed) || MIX_MUTE[i].load(Ordering::Relaxed);
            if let Ok(mut q) = f.queue.lock() {
                let k = n.min(q.len());
                for (o, s) in out.iter_mut().zip(q.drain(..k)) {
                    if !muted {
                        *o += (s as f32 * gain) as i32;
                    }
                }
            }
        }
        out.into_iter().map(|s| s.clamp(i16::MIN as i32, i16::MAX as i32) as i16).collect()
    }

    /// Drop what has been captured so far (after a pause).
    pub fn discard(&self) {
        for f in [&self.system, &self.mic].into_iter().flatten() {
            if let Ok(mut q) = f.queue.lock() {
                q.clear();
            }
        }
    }

    /// Loudness of (system, microphone), 0..1.
    pub fn levels(&self) -> (f32, f32) {
        let l = |f: &Option<Arc<Feed>>| f.as_ref().map(|f| f32::from_bits(f.peak.load(Ordering::Relaxed))).unwrap_or(0.0);
        (l(&self.system), l(&self.mic))
    }

    /// Set volume (0..2) and mute for (system, microphone).
    pub fn set_mix(&self, system: (f32, bool), mic: (f32, bool)) {
        for (f, (g, m)) in [(&self.system, system), (&self.mic, mic)] {
            if let Some(f) = f {
                f.gain.store(g.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
                f.muted.store(m, Ordering::Relaxed);
            }
        }
    }

    pub fn has_system(&self) -> bool {
        self.system.is_some()
    }

    pub fn has_mic(&self) -> bool {
        self.mic.is_some()
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

#[cfg(windows)]
fn spawn_source(mic: Option<String>, feed: Arc<Feed>, stop: Arc<AtomicBool>) -> Result<std::thread::JoinHandle<()>, String> {
    std::thread::Builder::new()
        .name(if mic.is_some() { "snagster-mic".into() } else { "snagster-loopback".into() })
        .spawn(move || {
            if let Err(e) = win::run(mic.as_deref(), &feed, &stop) {
                if let Ok(mut slot) = feed.error.lock() {
                    *slot = Some(e);
                }
            }
        })
        .map_err(|e| e.to_string())
}

#[cfg(not(windows))]
fn spawn_source(_mic: Option<String>, _feed: Arc<Feed>, _stop: Arc<AtomicBool>) -> Result<std::thread::JoinHandle<()>, String> {
    Err("Recording sound isn't available on this system yet.".into())
}

#[cfg(windows)]
mod win {
    use super::*;
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
