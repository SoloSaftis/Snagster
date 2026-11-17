//! How Snagster's window is drawn, and whether the drawing keeps up.
//!
//! - **Which renderer.** Windows draws through Direct3D 12 (wgpu) first and falls back to OpenGL
//!   when Direct3D can't start; Linux and macOS use OpenGL. On company laptops the OpenGL driver
//!   is often an old one the IT department pinned, and in Remote Desktop, Citrix or other VDI
//!   sessions Windows offers only OpenGL 1.1 ("GDI Generic"), too old for the window. Direct3D 12
//!   is part of Windows and always has a renderer: the graphics chip's, or Microsoft's WARP
//!   software renderer in a remote session. `SNAGSTER_RENDERER=opengl` (or `direct3d`) picks one
//!   by hand, for comparing on a slow computer.
//! - **Frame pacing.** The slow-frame line in crash.log (`app/work.rs`) times only Snagster's own
//!   work in a frame; a slow graphics driver, a remote session or security software holding up
//!   the drawing never showed there. `Pacing` times whole frames (from one to the next) while the
//!   person is using the window, and when drawing lags it writes one line to crash.log saying how
//!   often, how badly, with which renderer and graphics chip, and whether it was a remote session.

use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// A way to draw the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// Direct3D 12 through wgpu (Windows only).
    Direct3D,
    /// OpenGL through glow.
    OpenGl,
}

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Backend::Direct3D => "Direct3D 12",
            Backend::OpenGl => "OpenGL",
        }
    }
}

/// The renderers to try, in order: on Windows Direct3D then OpenGL, elsewhere OpenGL.
/// `choice` is `SNAGSTER_RENDERER` (`opengl`/`gl`/`glow` or `direct3d`/`d3d`/`dx12`/`wgpu`):
/// the chosen one goes first, the other stays as the fallback.
pub fn order(windows: bool, choice: Option<&str>) -> Vec<Backend> {
    if !windows {
        return vec![Backend::OpenGl];
    }
    let choice = choice.map(|c| c.trim().to_ascii_lowercase());
    match choice.as_deref() {
        Some("opengl" | "gl" | "glow") => vec![Backend::OpenGl, Backend::Direct3D],
        _ => vec![Backend::Direct3D, Backend::OpenGl],
    }
}

/// The renderers for this computer (and `SNAGSTER_RENDERER`).
pub fn order_here() -> Vec<Backend> {
    order(cfg!(windows), std::env::var("SNAGSTER_RENDERER").ok().as_deref())
}

/// Set up `options` to draw with `backend`.
pub fn apply(options: &mut eframe::NativeOptions, backend: Backend) {
    match backend {
        Backend::OpenGl => options.renderer = eframe::Renderer::Glow,
        Backend::Direct3D => {
            #[cfg(windows)]
            {
                options.renderer = eframe::Renderer::Wgpu;
                options.wgpu_options = wgpu_options();
            }
        }
    }
}

/// Direct3D 12 only, on the graphics chip that drives the screen (the built-in one on laptops
/// with two: waking the separate one costs battery and a copy between chips for every frame), and
/// textures as large as that chip allows (scrolling captures are tall), like OpenGL.
#[cfg(windows)]
fn wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew, wgpu};
    let mut setup = WgpuSetupCreateNew::without_display_handle();
    setup.instance_descriptor.backends = wgpu::Backends::DX12;
    setup.power_preference = wgpu::PowerPreference::LowPower;
    setup.device_descriptor = std::sync::Arc::new(|adapter: &wgpu::Adapter| wgpu::DeviceDescriptor {
        label: Some("snagster"),
        required_limits: wgpu::Limits { max_texture_dimension_2d: adapter.limits().max_texture_dimension_2d.max(8192), ..wgpu::Limits::default() },
        ..Default::default()
    });
    WgpuConfiguration { wgpu_setup: WgpuSetup::CreateNew(setup), ..Default::default() }
}

/// What the window is drawn with, once it is open ("Direct3D 12 on Intel(R) Arc(TM) 140T GPU").
static DRAWN_WITH: OnceLock<String> = OnceLock::new();

/// Note what the window got, from eframe's creation context.
pub fn note(cc: &eframe::CreationContext<'_>) {
    let _ = DRAWN_WITH.set(describe(cc));
}

pub fn drawn_with() -> &'static str {
    DRAWN_WITH.get().map(String::as_str).unwrap_or("not drawn yet")
}

fn describe(cc: &eframe::CreationContext<'_>) -> String {
    #[cfg(windows)]
    if let Some(rs) = cc.wgpu_render_state.as_ref() {
        let info = rs.adapter.get_info();
        return adapter_line("Direct3D 12", &info.name, info.device_type == eframe::egui_wgpu::wgpu::DeviceType::Cpu);
    }
    if let Some(gl) = cc.gl.as_ref() {
        use eframe::glow::HasContext;
        let (renderer, version) = unsafe { (gl.get_parameter_string(eframe::glow::RENDERER), gl.get_parameter_string(eframe::glow::VERSION)) };
        let software = is_software_gl(&renderer);
        return adapter_line(&format!("OpenGL {}", version.split_whitespace().next().unwrap_or("")), &renderer, software);
    }
    "unknown".into()
}

/// "Direct3D 12 on <chip>", marked when it is a software renderer (no graphics chip at work).
pub fn adapter_line(api: &str, chip: &str, software: bool) -> String {
    let chip = chip.trim();
    let chip = if chip.is_empty() { "an unnamed graphics chip" } else { chip };
    if software { format!("{api} on {chip} (software, no graphics chip)") } else { format!("{api} on {chip}") }
}

/// OpenGL renderers that draw on the processor: Windows' own 1.1 ("GDI Generic"), Mesa's
/// llvmpipe and softpipe, Microsoft's Basic Render Driver.
pub fn is_software_gl(renderer: &str) -> bool {
    let r = renderer.to_ascii_lowercase();
    ["gdi generic", "llvmpipe", "softpipe", "software rasterizer", "basic render"].iter().any(|s| r.contains(s))
}

/// Whether this is a Remote Desktop (or Citrix, or other VDI) session.
pub fn remote_session() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_REMOTESESSION};
        unsafe { GetSystemMetrics(SM_REMOTESESSION) != 0 }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// A frame slower than this, while the person is moving the mouse or typing, is a laggy one
/// (ten frames a second; a smooth window draws sixty).
pub const LAGGY_FRAME: Duration = Duration::from_millis(100);
/// Frames further apart than this weren't one stream of use (the window waited for input).
const GAP: Duration = Duration::from_millis(1500);
/// The pointer moving at least this fast (points a second) between two frames is someone moving
/// the mouse over the window. With smooth drawing every small move gets its own frame at once,
/// so the distance from one frame to the next stays small; when drawing lags, the moves pile up
/// behind the slow frame and the next one finds the pointer far away. A hand resting on the
/// mouse, a held button or slow typing don't count.
const MOVING: f32 = 200.0;
/// How often the frames are summed up.
const WINDOW: Duration = Duration::from_secs(60);
/// At least this many laggy frames in a minute makes a line in crash.log...
const LAGGY_COUNT: u32 = 10;
/// ...and at most one line this often.
const QUIET: Duration = Duration::from_secs(15 * 60);

/// Whole-frame times while the window is in use.
#[derive(Debug)]
pub struct Pacing {
    last: Option<Instant>,
    pointer: Option<(f32, f32)>,
    since: Option<Instant>,
    frames: u32,
    laggy: u32,
    worst: Duration,
    total: Duration,
    reported: Option<Instant>,
}

impl Pacing {
    pub const fn new() -> Self {
        Pacing { last: None, pointer: None, since: None, frames: 0, laggy: 0, worst: Duration::ZERO, total: Duration::ZERO, reported: None }
    }

    /// A frame starts at `now` with the pointer at `pointer` (None: not over the window).
    /// Returns the summary when the last minute lagged enough to report.
    pub fn frame(&mut self, now: Instant, pointer: Option<(f32, f32)>) -> Option<Summary> {
        let gap = self.last.map(|l| now.saturating_duration_since(l));
        let in_use = match (self.pointer, pointer, gap) {
            (Some(a), Some(b), Some(g)) if !g.is_zero() => ((b.0 - a.0).hypot(b.1 - a.1) / g.as_secs_f32()) >= MOVING,
            _ => false,
        };
        self.last = Some(now);
        self.pointer = pointer;
        let since = *self.since.get_or_insert(now);
        if in_use && let Some(gap) = gap.filter(|g| *g < GAP) {
            self.frames += 1;
            self.total += gap;
            if gap >= LAGGY_FRAME {
                self.laggy += 1;
            }
            self.worst = self.worst.max(gap);
        }
        if now.saturating_duration_since(since) < WINDOW {
            return None;
        }
        let summary = Summary { frames: self.frames, laggy: self.laggy, worst: self.worst, average: if self.frames > 0 { self.total / self.frames } else { Duration::ZERO } };
        self.since = Some(now);
        (self.frames, self.laggy, self.worst, self.total) = (0, 0, Duration::ZERO, Duration::ZERO);
        let quiet = self.reported.is_some_and(|r| now.saturating_duration_since(r) < QUIET);
        if summary.laggy < LAGGY_COUNT || quiet {
            return None;
        }
        self.reported = Some(now);
        Some(summary)
    }
}

impl Default for Pacing {
    fn default() -> Self {
        Self::new()
    }
}

/// A minute of frames in use.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Summary {
    pub frames: u32,
    pub laggy: u32,
    pub worst: Duration,
    pub average: Duration,
}

/// The crash.log line for a laggy minute.
pub fn laggy_line(s: &Summary, drawn_with: &str, remote: bool, packaged: bool) -> String {
    format!(
        "{} Snagster {} ({} {}) laggy drawing: {} of {} frames in use took {} ms or more (average {} ms, worst {} ms); drawn with {}; {}; {}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        s.laggy,
        s.frames,
        LAGGY_FRAME.as_millis(),
        s.average.as_millis(),
        s.worst.as_millis(),
        drawn_with,
        if remote { "remote session" } else { "local session" },
        if packaged { "installed (MSIX)" } else { "portable" },
    )
}

static PACING: Mutex<Pacing> = Mutex::new(Pacing::new());

/// Called at the start of every frame of the main window; writes the crash.log line when the
/// drawing lags (only for the real window, not the check runs).
pub fn frame(ctx: &egui::Context, persist: bool) {
    let pointer = ctx.input(|i| i.pointer.latest_pos()).map(|p| (p.x, p.y));
    let summary = PACING.lock().ok().and_then(|mut p| p.frame(Instant::now(), pointer));
    if let Some(s) = summary {
        let line = laggy_line(&s, drawn_with(), remote_session(), crate::packaged::is_packaged());
        if persist {
            crate::append_crash_log(&line);
        } else {
            eprint!("{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_tries_direct3d_first_and_keeps_opengl_as_the_fallback() {
        assert_eq!(order(true, None), vec![Backend::Direct3D, Backend::OpenGl]);
        assert_eq!(order(true, Some("")), vec![Backend::Direct3D, Backend::OpenGl]);
        assert_eq!(order(true, Some("direct3d")), vec![Backend::Direct3D, Backend::OpenGl]);
        assert_eq!(order(true, Some(" OpenGL ")), vec![Backend::OpenGl, Backend::Direct3D]);
        assert_eq!(order(true, Some("glow")), vec![Backend::OpenGl, Backend::Direct3D]);
        assert_eq!(order(true, Some("nonsense")), vec![Backend::Direct3D, Backend::OpenGl]);
    }

    #[test]
    fn other_systems_draw_with_opengl_whatever_is_asked() {
        assert_eq!(order(false, None), vec![Backend::OpenGl]);
        assert_eq!(order(false, Some("direct3d")), vec![Backend::OpenGl]);
    }

    #[test]
    fn options_get_the_renderer_asked_for() {
        let mut o = eframe::NativeOptions::default();
        apply(&mut o, Backend::OpenGl);
        assert_eq!(o.renderer, eframe::Renderer::Glow);
        #[cfg(windows)]
        {
            apply(&mut o, Backend::Direct3D);
            assert_eq!(o.renderer, eframe::Renderer::Wgpu);
        }
    }

    #[test]
    fn software_opengl_is_recognised() {
        assert!(is_software_gl("GDI Generic"));
        assert!(is_software_gl("llvmpipe (LLVM 19.1.0, 256 bits)"));
        assert!(is_software_gl("Microsoft Basic Render Driver"));
        assert!(!is_software_gl("Intel(R) UHD Graphics 620"));
        assert!(!is_software_gl("NVIDIA GeForce RTX 3050 Laptop GPU/PCIe/SSE2"));
    }

    #[test]
    fn the_adapter_line_says_when_there_is_no_graphics_chip() {
        assert_eq!(adapter_line("Direct3D 12", "Intel(R) Arc(TM) 140T GPU", false), "Direct3D 12 on Intel(R) Arc(TM) 140T GPU");
        assert_eq!(adapter_line("Direct3D 12", "Microsoft Basic Render Driver", true), "Direct3D 12 on Microsoft Basic Render Driver (software, no graphics chip)");
        assert_eq!(adapter_line("OpenGL 1.1.0", " ", false), "OpenGL 1.1.0 on an unnamed graphics chip");
    }

    /// `n` frames `every` apart from `start`, the mouse moving 400 points a second; returns
    /// what was reported.
    fn run(p: &mut Pacing, start: Instant, n: u32, every: Duration) -> (Instant, Option<Summary>) {
        let mut t = start;
        let mut out = None;
        for i in 0..n {
            t += every;
            let x = (i as f32 * 400.0 * every.as_secs_f32()) % 2000.0;
            if let Some(s) = p.frame(t, Some((x, 300.0))) {
                out = Some(s);
            }
        }
        (t, out)
    }

    #[test]
    fn smooth_drawing_is_never_reported() {
        let mut p = Pacing::new();
        let (_, s) = run(&mut p, Instant::now(), 60 * 60 * 2, Duration::from_millis(16));
        assert_eq!(s, None);
    }

    #[test]
    fn a_laggy_minute_is_reported_once_with_its_numbers() {
        let mut p = Pacing::new();
        let t0 = Instant::now();
        p.frame(t0, Some((0.0, 300.0)));
        // 200 ms a frame for a little over a minute.
        let (t, s) = run(&mut p, t0, 301, Duration::from_millis(200));
        let s = s.expect("a laggy minute is reported");
        assert!(s.laggy >= LAGGY_COUNT && s.laggy == s.frames, "{s:?}");
        assert_eq!(s.worst, Duration::from_millis(200));
        assert_eq!(s.average, Duration::from_millis(200));
        // The next laggy minutes stay quiet for a while (one line, not one a minute).
        let (t, s) = run(&mut p, t, 600, Duration::from_millis(200));
        assert_eq!(s, None);
        // After the quiet time it may report again.
        let (_, s) = run(&mut p, t + QUIET, 400, Duration::from_millis(200));
        assert!(s.is_some());
    }

    #[test]
    fn waiting_for_input_is_not_lag() {
        let mut p = Pacing::new();
        let mut t = Instant::now();
        // A frame every 2 s (the window idles between clicks) is not counted at all.
        for i in 0..100 {
            t += Duration::from_secs(2);
            assert_eq!(p.frame(t, Some((i as f32 * 900.0 % 1800.0, 10.0))), None);
        }
        // Frames 500 ms apart with the pointer still (a held button, typing) or away aren't
        // counted, and neither are slow moves (a pixel at a time).
        for i in 0..1000 {
            t += Duration::from_millis(500);
            let at = match i % 3 {
                0 => Some((50.0, 50.0)),
                1 => None,
                _ => Some((50.0 + (i % 2) as f32 * 2.0, 50.0)),
            };
            assert_eq!(p.frame(t, at), None);
        }
        assert_eq!((p.frames, p.laggy), (0, 0));
    }

    #[test]
    fn a_few_hiccups_are_not_reported() {
        let mut p = Pacing::new();
        let t0 = Instant::now();
        p.frame(t0, None);
        let (t, _) = run(&mut p, t0, 3000, Duration::from_millis(16));
        let (t, s1) = run(&mut p, t, LAGGY_COUNT - 1, Duration::from_millis(300));
        let (_, s2) = run(&mut p, t, 4000, Duration::from_millis(16));
        assert_eq!((s1, s2), (None, None));
    }

    #[test]
    fn the_laggy_line_has_what_is_needed_to_find_the_cause() {
        let s = Summary { frames: 120, laggy: 80, worst: Duration::from_millis(900), average: Duration::from_millis(150) };
        let line = laggy_line(&s, "OpenGL 1.1.0 on GDI Generic (software, no graphics chip)", true, false);
        assert!(line.ends_with('\n'));
        for want in ["laggy drawing: 80 of 120 frames", "average 150 ms", "worst 900 ms", "GDI Generic", "remote session", "portable"] {
            assert!(line.contains(want), "{want} missing in {line}");
        }
        assert!(laggy_line(&s, "x", false, true).contains("local session; installed (MSIX)"));
    }
}
