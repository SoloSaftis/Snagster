//! macOS: one program's sound through ScreenCaptureKit (sound capture needs macOS 13), written
//! from Apple's documentation. A program is its bundle id. A lane runs a stream whose filter
//! takes in that application only and asks for its sound (48 kHz stereo) and the smallest
//! picture, which it never looks at. It needs the Screen Recording permission, as everything
//! Snagster takes from the screen does.
//!
//! Which programs: the applications ScreenCaptureKit offers that have a window on the screen
//! (macOS doesn't say which ones play sound).

use super::programs::{Gather, LOOK_EVERY, Layout, Program, Unsupported, key, same, to_stereo};
use super::Feed;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_core_media::{CMSampleBuffer, CMTime, CMTimeFlags};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol, NSProcessInfo};
use objc2_screen_capture_kit::{SCContentFilter, SCRunningApplication, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamOutput, SCStreamOutputType, SCWindow};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// macOS's major version (13 for Ventura).
pub fn major() -> u32 {
    NSProcessInfo::processInfo().operatingSystemVersion().majorVersion as u32
}

/// Looked at again at most once a second (the interface asks every frame).
pub fn support() -> Result<(), Unsupported> {
    static LAST: Mutex<Option<(Instant, Result<(), Unsupported>)>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, r)) = &*last
        && at.elapsed() < Duration::from_secs(1)
    {
        return r.clone();
    }
    let r = if major() < 13 {
        Err(Unsupported::OldMac)
    } else if !preflight() {
        Err(Unsupported::Permission)
    } else {
        Ok(())
    };
    *last = Some((Instant::now(), r.clone()));
    r
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
}

/// The Screen Recording permission is given (asks nothing).
fn preflight() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// What ScreenCaptureKit can capture now (None: refused, or no answer within 3 s).
fn content() -> Option<Retained<SCShareableContent>> {
    struct Sendable(Option<Retained<SCShareableContent>>);
    // SAFETY: the content is only read after it has been handed over, on the receiving thread.
    unsafe impl Send for Sendable {}
    let (tx, rx) = channel::<Sendable>();
    let tx = Mutex::new(Some(tx));
    let block = RcBlock::new(move |c: *mut SCShareableContent, _e: *mut NSError| {
        let got = unsafe { Retained::retain(c) };
        if let Some(tx) = tx.lock().ok().and_then(|mut t| t.take()) {
            let _ = tx.send(Sendable(got));
        }
    });
    unsafe { SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(true, true, &block) };
    rx.recv_timeout(Duration::from_secs(3)).ok().and_then(|s| s.0)
}

/// The applications with a window on the screen (Snagster's own left out by `tidy`).
fn applications(c: &SCShareableContent) -> Vec<Retained<SCRunningApplication>> {
    let windows: Retained<NSArray<SCWindow>> = unsafe { c.windows() };
    let shown: Vec<i32> = windows.iter().filter_map(|w| unsafe { w.owningApplication() }).map(|a| unsafe { a.processID() }).collect();
    unsafe { c.applications() }.iter().filter(|a| shown.contains(&unsafe { a.processID() })).collect()
}

pub fn list() -> Vec<Program> {
    let Some(c) = content() else { return Vec::new() };
    applications(&c)
        .iter()
        .map(|a| Program { name: key(&unsafe { a.bundleIdentifier() }.to_string()), label: unsafe { a.applicationName() }.to_string(), playing: false })
        .collect()
}

/// The CoreMedia calls that read a sound buffer.
#[repr(C)]
struct Asbd {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

// objc2-core-media declares the sample and block buffer calls (declaring them again here clashes);
// this one needs a crate feature that brings in CoreAudio's types, so it is declared here.
#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    fn CMAudioFormatDescriptionGetStreamBasicDescription(desc: *const c_void) -> *const Asbd;
}

const FLAG_FLOAT: u32 = 1;
const FLAG_NON_INTERLEAVED: u32 = 1 << 5;

/// A sound buffer's samples as 16-bit stereo.
fn samples(sb: &CMSampleBuffer) -> Vec<i16> {
    unsafe {
        let frames = sb.num_samples().max(0) as usize;
        let (Some(desc), Some(data)) = (sb.format_description(), sb.data_buffer()) else { return Vec::new() };
        if frames == 0 {
            return Vec::new();
        }
        let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(&*desc as *const _ as *const c_void);
        if asbd.is_null() {
            return Vec::new();
        }
        let a = &*asbd;
        let layout = Layout { channels: a.channels_per_frame as usize, float: a.format_flags & FLAG_FLOAT != 0, planar: a.format_flags & FLAG_NON_INTERLEAVED != 0 };
        if (layout.float && a.bits_per_channel != 32) || (!layout.float && a.bits_per_channel != 16) {
            return Vec::new();
        }
        let len = data.data_length();
        if len == 0 {
            return Vec::new();
        }
        let mut bytes = vec![0u8; len];
        if data.copy_data_bytes(0, len, std::ptr::NonNull::new_unchecked(bytes.as_mut_ptr().cast())) != 0 {
            return Vec::new();
        }
        to_stereo(&bytes, frames, layout)
    }
}

/// Samples the stream has handed over, taken by the lane.
type Inbox = Arc<Mutex<Vec<i16>>>;

define_class!(
    // SAFETY: NSObject has no subclassing rules; the class adds no Drop, only its inbox.
    #[unsafe(super(NSObject))]
    #[name = "SnagsterProgramSound"]
    #[ivars = Inbox]
    struct Output;

    unsafe impl NSObjectProtocol for Output {}

    unsafe impl SCStreamOutput for Output {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output(&self, _stream: &SCStream, sample_buffer: &CMSampleBuffer, kind: SCStreamOutputType) {
            if kind != SCStreamOutputType::Audio {
                return;
            }
            let s = samples(sample_buffer);
            if let Ok(mut inbox) = self.ivars().lock() {
                inbox.extend_from_slice(&s);
                // Nobody taking them: keep at most 2 s.
                let max = 48_000 * 2 * 2;
                if inbox.len() > max {
                    let extra = inbox.len() - max;
                    let cut = extra.next_multiple_of(2).min(inbox.len());
                    inbox.drain(..cut);
                }
            }
        }
    }
);

impl Output {
    fn new(inbox: Inbox) -> Retained<Output> {
        let this = Output::alloc().set_ivars(inbox);
        unsafe { msg_send![super(this), init] }
    }
}

/// A running stream of one application's sound.
struct Running {
    pid: i32,
    stream: Retained<SCStream>,
    // Kept alive while the stream may call it.
    _output: Retained<Output>,
    _queue: dispatch2::DispatchRetained<dispatch2::DispatchQueue>,
}

impl Drop for Running {
    /// Stops the stream and waits (up to 2 s) until it has, so it no longer calls the output,
    /// then takes the output off.
    fn drop(&mut self) {
        let (tx, rx) = channel::<()>();
        let tx = Mutex::new(Some(tx));
        let done = RcBlock::new(move |_e: *mut NSError| {
            if let Some(tx) = tx.lock().ok().and_then(|mut t| t.take()) {
                let _ = tx.send(());
            }
        });
        unsafe { self.stream.stopCaptureWithCompletionHandler(Some(&done)) };
        let _ = rx.recv_timeout(Duration::from_secs(2));
        let _ = unsafe { self.stream.removeStreamOutput_type_error(ProtocolObject::from_ref(&*self._output), SCStreamOutputType::Audio) };
    }
}

/// Start a stream of `app`'s sound into `inbox`.
fn start(c: &SCShareableContent, app: &SCRunningApplication, inbox: Inbox) -> Result<Running, String> {
    unsafe {
        let display = c.displays().firstObject().ok_or("there is no screen")?;
        let apps = NSArray::from_retained_slice(&[Retained::retain(app as *const _ as *mut SCRunningApplication).ok_or("no application")?]);
        let filter = SCContentFilter::initWithDisplay_includingApplications_exceptingWindows(SCContentFilter::alloc(), &display, &apps, &NSArray::new());
        let config = SCStreamConfiguration::new();
        config.setCapturesAudio(true);
        config.setSampleRate(48_000);
        config.setChannelCount(2);
        config.setExcludesCurrentProcessAudio(true);
        // The picture isn't used: the smallest, once a second.
        config.setWidth(2);
        config.setHeight(2);
        config.setMinimumFrameInterval(CMTime { value: 1, timescale: 1, flags: CMTimeFlags::Valid, epoch: 0 });
        config.setQueueDepth(3);
        let stream = SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &config, None);
        let output = Output::new(inbox);
        let queue = dispatch2::DispatchQueue::new("app.snagster.program-sound", None);
        stream
            .addStreamOutput_type_sampleHandlerQueue_error(ProtocolObject::from_ref(&*output), SCStreamOutputType::Audio, Some(&queue))
            .map_err(|e| e.localizedDescription().to_string())?;
        let (tx, rx) = channel::<Option<String>>();
        let tx = Mutex::new(Some(tx));
        let done = RcBlock::new(move |e: *mut NSError| {
            let why = e.as_ref().map(|e| e.localizedDescription().to_string());
            if let Some(tx) = tx.lock().ok().and_then(|mut t| t.take()) {
                let _ = tx.send(why);
            }
        });
        stream.startCaptureWithCompletionHandler(Some(&done));
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(None) => Ok(Running { pid: app.processID(), stream, _output: output, _queue: queue }),
            Ok(Some(why)) => Err(why),
            Err(_) => Err("macOS didn't start the sound capture".into()),
        }
    }
}

pub fn run(name: &str, feed: &Arc<Feed>, stop: &Arc<AtomicBool>) -> Result<(), String> {
    let mut gather = Gather::new(Instant::now());
    let inbox: Inbox = Arc::new(Mutex::new(Vec::new()));
    let mut running: Option<Running> = None;
    let mut looked: Option<Instant> = None;
    while !stop.load(Ordering::Relaxed) {
        if looked.is_none_or(|t| t.elapsed() >= LOOK_EVERY) {
            looked = Some(Instant::now());
            if let Some(c) = content() {
                let app = unsafe { c.applications() }.iter().find(|a| same(name, &unsafe { a.bundleIdentifier() }.to_string()));
                match (&running, app) {
                    // Still there.
                    (Some(r), Some(a)) if r.pid == unsafe { a.processID() } => {}
                    // Gone, or started again as a new process.
                    (Some(_), _) => {
                        running = None;
                        gather.end(0);
                        looked = None;
                    }
                    (None, Some(a)) => running = start(&c, &a, inbox.clone()).ok(),
                    (None, None) => {}
                }
            }
            feed.waiting.store(running.is_none(), Ordering::Relaxed);
        }
        let got = inbox.lock().map(|mut i| std::mem::take(&mut *i)).unwrap_or_default();
        if !got.is_empty() {
            gather.put(0, &got);
        }
        gather.tick(Instant::now(), feed);
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
