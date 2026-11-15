//! macOS zero-copy screen feed for live streaming: a `CGDisplayStream` (macOS 10.8 and later, so
//! every macOS Snagster runs on) asked for pictures in NV12 video range ('420v', BT.709) at the
//! stream's size. The window server scales and converts on the graphics chip and hands each
//! picture over as an IOSurface; it is wrapped in a CVPixelBuffer (`encode::PixelBuffer`) and
//! given to VideoToolbox as it is, so no picture is copied to memory or converted by the processor.
//!
//! The stream's handler is an Objective-C block. It is built here by hand following the
//! documented block layout ("Block Implementation Specification", clang): a global block (no
//! copy helpers) whose one captured value points at the feed's shared state; it lives until the
//! stream has reported that it stopped. The functions and property keys are looked up while
//! running (`dlsym`), so a macOS without them falls back to the copy through memory.
//! ScreenCaptureKit (macOS 12.3+) would need delegates and asynchronous completion handlers; the
//! display stream does what streaming needs with one block.

use super::*;
use crate::encode::{PixelBuffer, Surface, SurfaceDevice};
use std::ffi::{c_char, c_void};

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

type CFTypeRef = *const c_void;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGGetDisplaysWithPoint(p: CGPoint, max: u32, ids: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayBounds(id: u32) -> CGRect;
    fn CGRectCreateDictionaryRepresentation(r: CGRect) -> CFTypeRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFTypeDictionaryKeyCallBacks: u8;
    static kCFTypeDictionaryValueCallBacks: u8;
    static kCFBooleanTrue: CFTypeRef;
    static kCFBooleanFalse: CFTypeRef;
    fn CFDictionaryCreateMutable(a: *const c_void, capacity: isize, keys: *const c_void, values: *const c_void) -> *mut c_void;
    fn CFDictionarySetValue(d: *mut c_void, key: *const c_void, value: *const c_void);
    fn CFNumberCreate(a: *const c_void, kind: isize, value: *const c_void) -> CFTypeRef;
    fn CFRelease(cf: CFTypeRef);
}

unsafe extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> *mut c_void;
    fn dispatch_release(o: *mut c_void);
    /// The class of blocks that live for the whole program (libSystem's block runtime).
    static _NSConcreteGlobalBlock: [*const c_void; 32];
}

const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
const CF_NUMBER_FLOAT64: isize = 6;
const CF_NUMBER_SINT32: isize = 3;
/// 'kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange': NV12, video range.
const PIXEL_420V: i32 = i32::from_be_bytes(*b"420v");
/// `CGDisplayStreamFrameStatus`.
const FRAME_COMPLETE: i32 = 0;
const STOPPED: i32 = 3;
/// `BLOCK_IS_GLOBAL`: the block is never copied to the heap or released.
const BLOCK_IS_GLOBAL: i32 = 1 << 28;
/// Pictures the window server may have out at once (the newest held here, a few in the
/// encoder); it allows at most 8.
const QUEUE_DEPTH: i32 = 6;

type CreateFn = unsafe extern "C" fn(u32, usize, usize, i32, *const c_void, *mut c_void, *const c_void) -> *mut c_void;
type StartStopFn = unsafe extern "C" fn(*mut c_void) -> i32;
type HandlerFn = unsafe extern "C" fn(*mut Block, i32, u64, *mut c_void, *mut c_void);

#[repr(C)]
struct Descriptor {
    reserved: usize,
    size: usize,
}

/// A block's layout: isa, flags, reserved, invoke, descriptor, then what it captured.
#[repr(C)]
struct Block {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: HandlerFn,
    descriptor: *const Descriptor,
    ctx: *const Ctx,
}

static DESCRIPTOR: Descriptor = Descriptor { reserved: 0, size: std::mem::size_of::<Block>() };

/// What the handler reaches through the block.
struct Ctx {
    sh: Arc<SurfaceShared>,
    stopped: AtomicBool,
}

unsafe extern "C" fn on_frame(block: *mut Block, status: i32, _time: u64, surface: *mut c_void, _update: *mut c_void) {
    let ctx = unsafe { &*(*block).ctx };
    match status {
        FRAME_COMPLETE => {
            let began = Instant::now();
            if let Some(pb) = unsafe { PixelBuffer::from_iosurface(surface) } {
                ctx.sh.publish(Surface::PixelBuffer(pb), began);
            }
        }
        STOPPED => ctx.stopped.store(true, Ordering::Release),
        _ => {}
    }
}

fn symbol<T: Copy>(name: &std::ffi::CStr) -> Option<T> {
    let p = unsafe { dlsym(RTLD_DEFAULT, name.as_ptr()) };
    (!p.is_null()).then(|| unsafe { std::mem::transmute_copy::<*mut c_void, T>(&p) })
}

/// A property key (a CFString constant) looked up by name.
fn key(name: &std::ffi::CStr) -> Option<CFTypeRef> {
    symbol::<*const CFTypeRef>(name).map(|p| unsafe { *p }).filter(|k| !k.is_null())
}

/// The running display stream, its queue and its block.
struct Running {
    stream: *mut c_void,
    queue: *mut c_void,
    stop: StartStopFn,
    block: *mut Block,
    ctx: *const Ctx,
}

// The stream and queue are Core Foundation / dispatch objects usable from any thread.
unsafe impl Send for Running {}
unsafe impl Sync for Running {}

impl Running {
    fn end(self) {
        unsafe {
            (self.stop)(self.stream);
            // The handler hears "stopped" on the queue; after that nothing calls it again.
            let t = Instant::now();
            while !(*self.ctx).stopped.load(Ordering::Acquire) && t.elapsed() < Duration::from_secs(1) {
                std::thread::sleep(Duration::from_millis(5));
            }
            let stopped = (*self.ctx).stopped.load(Ordering::Acquire);
            CFRelease(self.stream);
            dispatch_release(self.queue);
            // A stream that never said it stopped may still call the block: keep it then.
            if stopped {
                drop(Box::from_raw(self.block));
                drop(Box::from_raw(self.ctx as *mut Ctx));
            }
        }
    }
}

/// Start a display stream of `rect` (desktop points) at `o.fit` (the stream's size).
pub(super) fn start(rect: (i32, i32, u32, u32), o: FeedOptions) -> Result<SurfaceFeed, String> {
    let create: CreateFn = symbol(c"CGDisplayStreamCreateWithDispatchQueue").ok_or("this macOS has no display stream")?;
    let start: StartStopFn = symbol(c"CGDisplayStreamStart").ok_or("this macOS has no display stream")?;
    let stop: StartStopFn = symbol(c"CGDisplayStreamStop").ok_or("this macOS has no display stream")?;
    let (x, y, w, h) = rect;
    let (ow, oh) = o.fit.unwrap_or((w & !1, h & !1));
    unsafe {
        let centre = CGPoint { x: x as f64 + w as f64 / 2.0, y: y as f64 + h as f64 / 2.0 };
        let (mut id, mut n) = (0u32, 0u32);
        if CGGetDisplaysWithPoint(centre, 1, &mut id, &mut n) != 0 || n == 0 {
            return Err("that area is off screen".into());
        }
        let b = CGDisplayBounds(id);
        let bounds = (b.origin.x as i32, b.origin.y as i32, b.size.width as u32, b.size.height as u32);
        let (cx, cy, cw, ch) = clip_area(rect, bounds).ok_or("that area is off screen")?;
        let props = CFDictionaryCreateMutable(std::ptr::null(), 0, &raw const kCFTypeDictionaryKeyCallBacks as *const c_void, &raw const kCFTypeDictionaryValueCallBacks as *const c_void);
        if props.is_null() {
            return Err("no memory for the display stream's settings".into());
        }
        let mut owned: Vec<CFTypeRef> = Vec::new();
        let mut set = |k: Option<CFTypeRef>, v: CFTypeRef, own: bool| {
            if let Some(k) = k
                && !v.is_null()
            {
                CFDictionarySetValue(props, k, v);
            }
            if own && !v.is_null() {
                owned.push(v);
            }
        };
        let local = CGRect { origin: CGPoint { x: (cx - bounds.0) as f64, y: (cy - bounds.1) as f64 }, size: CGSize { width: cw as f64, height: ch as f64 } };
        set(key(c"kCGDisplayStreamSourceRect"), CGRectCreateDictionaryRepresentation(local), true);
        set(key(c"kCGDisplayStreamPreserveAspectRatio"), kCFBooleanTrue, false);
        set(key(c"kCGDisplayStreamShowCursor"), if o.cursor { kCFBooleanTrue } else { kCFBooleanFalse }, false);
        let frame_time = 1.0 / o.fps.clamp(1, 240) as f64;
        set(key(c"kCGDisplayStreamMinimumFrameTime"), CFNumberCreate(std::ptr::null(), CF_NUMBER_FLOAT64, &frame_time as *const f64 as *const c_void), true);
        set(key(c"kCGDisplayStreamQueueDepth"), CFNumberCreate(std::ptr::null(), CF_NUMBER_SINT32, &QUEUE_DEPTH as *const i32 as *const c_void), true);
        set(key(c"kCGDisplayStreamYCbCrMatrix"), key(c"kCGDisplayStreamYCbCrMatrix_ITU_R_709_2").unwrap_or(std::ptr::null()), false);
        let sh = Arc::new(SurfaceShared::default());
        let ctx = Box::into_raw(Box::new(Ctx { sh: sh.clone(), stopped: AtomicBool::new(false) }));
        let block = Box::into_raw(Box::new(Block {
            isa: &raw const _NSConcreteGlobalBlock as *const c_void,
            flags: BLOCK_IS_GLOBAL,
            reserved: 0,
            invoke: on_frame,
            descriptor: &DESCRIPTOR,
            ctx,
        }));
        let queue = dispatch_queue_create(c"snagster.display-stream".as_ptr(), std::ptr::null());
        let stream = create(id, ow as usize, oh as usize, PIXEL_420V, props as *const c_void, queue, block as *const c_void);
        CFRelease(props as CFTypeRef);
        owned.into_iter().for_each(|v| CFRelease(v));
        let free = || {
            drop(Box::from_raw(block));
            drop(Box::from_raw(ctx));
            if !queue.is_null() {
                dispatch_release(queue);
            }
        };
        if stream.is_null() {
            free();
            return Err("macOS wouldn't start a display stream (is screen recording allowed?)".into());
        }
        let e = start(stream);
        if e != 0 {
            CFRelease(stream);
            free();
            return Err(format!("the display stream didn't start (error {e})"));
        }
        let running = Running { stream, queue, stop, block, ctx };
        // For measuring (`every_frame`): the display stream only sends changed pictures, so the
        // newest one counts again each frame nothing new came (Windows converts it again; the
        // stream repeats it either way).
        let ticker = o.every_frame.then(|| {
            let (sh, every) = (sh.clone(), Duration::from_nanos(1_000_000_000 / o.fps.clamp(1, 240) as u64));
            std::thread::spawn(move || {
                let mut last = sh.count.load(Ordering::Relaxed);
                while !sh.stop.load(Ordering::Relaxed) {
                    std::thread::sleep(every);
                    let now = sh.count.load(Ordering::Relaxed);
                    if now == last
                        && let Some(s) = sh.latest.lock().ok().and_then(|l| l.clone())
                    {
                        sh.publish(s, Instant::now());
                    }
                    last = sh.count.load(Ordering::Relaxed);
                }
            })
        });
        Ok(SurfaceFeed::new(sh, SurfaceDevice::CoreVideo, "display stream, zero-copy", Box::new(move || {
            if let Some(t) = ticker {
                let _ = t.join();
            }
            running.end()
        })))
    }
}
