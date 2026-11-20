//! Studio's live ink: what the person draws on the screen (or on the Studio preview) while
//! streaming or recording. The strokes live here, in one place the whole program shares: the
//! Studio engine draws them into every composed frame (so they are in the stream and the
//! video), and the on-screen overlay (`ink_overlay`) shows them over the desktop.
//!
//! Strokes drawn on the screen are kept in desktop pixels and show wherever that part of the
//! desktop is in the scene (a screen, window or area source, scaled with it); strokes drawn on
//! the preview are kept in canvas pixels. Ink either fades a few seconds after the pen lifts
//! (every stroke together, so a drawing made of several strokes goes at once) or stays until
//! cleared. Times are passed in, so everything here is tested without a screen or a clock.

use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// How long ink stays after the pen lifts.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum InkMode {
    /// Fades `ink_fade_s` seconds after the last stroke.
    #[default]
    Fade,
    /// Stays until Clear.
    Stay,
}

/// The fade at the end of the hold time.
pub const FADE_OUT: Duration = Duration::from_millis(500);
/// The colours the Color menu offers (red first: the default).
pub const COLORS: [[u8; 4]; 6] = [[230, 40, 40, 255], [250, 210, 20, 255], [40, 190, 70, 255], [30, 120, 240, 255], [255, 255, 255, 255], [0, 0, 0, 255]];
/// The pen widths the Pen menu offers (desktop pixels at 100 %), with their names.
pub const WIDTHS: [(f32, &str); 3] = [(3.0, "Thin"), (6.0, "Medium"), (12.0, "Thick")];
/// The hold times the menu offers, in seconds.
pub const HOLDS: [f32; 5] = [1.0, 2.0, 3.0, 5.0, 10.0];
/// Limits, so a long show can't fill the memory: the oldest stroke goes first.
const MAX_STROKES: usize = 500;
const MAX_POINTS: usize = 20_000;

/// What a stroke's points are measured in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    /// Desktop pixels (drawn on the screen).
    Desktop,
    /// Studio canvas pixels (drawn on the preview).
    Canvas,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    pub space: Space,
    pub points: Vec<[f32; 2]>,
    pub color: [u8; 4],
    /// Pen width in the stroke's own pixels.
    pub width: f32,
}

impl Stroke {
    /// The box the stroke covers, its width included: (left, top, right, bottom).
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        let r = self.width / 2.0 + 1.0;
        let mut b = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in &self.points {
            b = (b.0.min(p[0] - r), b.1.min(p[1] - r), b.2.max(p[0] + r), b.3.max(p[1] + r));
        }
        b
    }
}

/// The pen for new strokes (Studio's settings).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pen {
    pub color: [u8; 4],
    pub width: f32,
    pub mode: InkMode,
    /// Seconds ink holds before fading (Fade mode).
    pub hold_s: f32,
}

impl Default for Pen {
    fn default() -> Self {
        Pen { color: COLORS[0], width: 6.0, mode: InkMode::Fade, hold_s: 3.0 }
    }
}

/// Every stroke, the one being drawn, and whether drawing is on.
#[derive(Default)]
pub struct Ink {
    strokes: Vec<Stroke>,
    drawing: Option<Stroke>,
    /// When the pen last lifted (the fade's clock).
    lifted: Option<Instant>,
    /// What the last Clear removed (Undo brings it back).
    cleared: Vec<Stroke>,
    pub pen: Pen,
    /// Draw is on: the overlay and the preview take the mouse.
    pub on: bool,
    /// Counts changes, so the overlay redraws only when something changed.
    generation: u64,
}

impl Ink {
    fn changed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Turn drawing on or off (ink already drawn stays).
    pub fn set_on(&mut self, on: bool, now: Instant) {
        if self.on != on {
            self.on = on;
            if !on {
                self.end(now);
            }
            self.changed();
        }
    }

    /// Set the pen (from the settings). A change of mode or hold time restarts the clock.
    pub fn set_pen(&mut self, pen: Pen, now: Instant) {
        if self.pen != pen {
            if (self.pen.mode, self.pen.hold_s) != (pen.mode, pen.hold_s) && self.lifted.is_some() {
                self.lifted = Some(now);
            }
            self.pen = pen;
            self.changed();
        }
    }

    /// The pen goes down at `p`.
    pub fn begin(&mut self, space: Space, p: [f32; 2], now: Instant) {
        self.end(now);
        self.drop_faded(now);
        self.drawing = Some(Stroke { space, points: vec![p], color: self.pen.color, width: self.pen.width.clamp(1.0, 64.0) });
        self.changed();
    }

    /// The pen moves to `p` (ignored when it moved less than half a pixel).
    pub fn extend(&mut self, p: [f32; 2]) {
        let Some(s) = &mut self.drawing else { return };
        if s.points.len() >= MAX_POINTS {
            return;
        }
        let last = s.points[s.points.len() - 1];
        if (last[0] - p[0]).hypot(last[1] - p[1]) >= 0.5 {
            s.points.push(p);
            self.changed();
        }
    }

    /// The pen lifts: the stroke is kept, and the fade's clock starts again for all ink.
    pub fn end(&mut self, now: Instant) {
        if let Some(s) = self.drawing.take() {
            self.strokes.push(s);
            if self.strokes.len() > MAX_STROKES {
                let extra = self.strokes.len() - MAX_STROKES;
                self.strokes.drain(..extra);
            }
            self.lifted = Some(now);
            self.changed();
        }
    }

    #[cfg(test)]
    pub fn drawing(&self) -> bool {
        self.drawing.is_some()
    }

    /// Remove all ink (Undo brings it back).
    pub fn clear(&mut self) {
        let mut gone = std::mem::take(&mut self.strokes);
        gone.extend(self.drawing.take());
        if !gone.is_empty() {
            self.cleared = gone;
        }
        self.lifted = None;
        self.changed();
    }

    /// Take back the last stroke, or, right after Clear, bring back what it removed.
    pub fn undo(&mut self, now: Instant) {
        self.end(now);
        if self.strokes.is_empty() && !self.cleared.is_empty() {
            self.strokes = std::mem::take(&mut self.cleared);
            self.lifted = Some(now);
        } else {
            self.strokes.pop();
        }
        self.changed();
    }

    /// How strongly ink shows at `now`: 1 while drawing, while holding and in Stay mode, then
    /// down to 0 over `FADE_OUT`.
    pub fn alpha(&self, now: Instant) -> f32 {
        if self.drawing.is_some() || self.pen.mode == InkMode::Stay {
            return 1.0;
        }
        let Some(lifted) = self.lifted else { return 1.0 };
        let hold = Duration::from_secs_f32(self.pen.hold_s.clamp(0.1, 600.0));
        let t = now.saturating_duration_since(lifted);
        if t <= hold {
            1.0
        } else {
            1.0 - ((t - hold).as_secs_f32() / FADE_OUT.as_secs_f32()).min(1.0)
        }
    }

    /// Whether the ink is fading right now (the overlay redraws every frame meanwhile).
    pub fn fading(&self, now: Instant) -> bool {
        let a = self.alpha(now);
        a > 0.0 && a < 1.0 && !self.strokes.is_empty()
    }

    /// When the fade will next change what is shown (None: not until something is drawn).
    pub fn next_change(&self, now: Instant) -> Option<Instant> {
        if self.drawing.is_some() || self.pen.mode == InkMode::Stay || self.strokes.is_empty() {
            return None;
        }
        let lifted = self.lifted?;
        let hold = lifted + Duration::from_secs_f32(self.pen.hold_s.clamp(0.1, 600.0));
        Some(if now < hold { hold } else { now })
    }

    /// Forget ink that has faded away.
    fn drop_faded(&mut self, now: Instant) {
        if !self.strokes.is_empty() && self.alpha(now) <= 0.0 {
            self.strokes.clear();
            self.lifted = None;
            self.changed();
        }
    }

    /// Whether any ink is there (Clear has something to do).
    pub fn any(&self, now: Instant) -> bool {
        self.drawing.is_some() || (!self.strokes.is_empty() && self.alpha(now) > 0.0)
    }

    /// The ink to draw at `now`, with how strongly it shows. Faded ink is forgotten.
    pub fn snapshot(&mut self, now: Instant) -> Snapshot {
        self.drop_faded(now);
        let a = self.alpha(now);
        let strokes: Vec<Stroke> = self.strokes.iter().chain(self.drawing.iter()).cloned().collect();
        Snapshot { strokes, alpha: if a > 0.0 { a } else { 0.0 } }
    }
}

/// The ink as it shows at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub strokes: Vec<Stroke>,
    pub alpha: f32,
}

impl Snapshot {
    pub fn is_empty(&self) -> bool {
        self.strokes.is_empty() || self.alpha <= 0.0
    }

    pub fn in_space(&self, space: Space) -> impl Iterator<Item = &Stroke> {
        self.strokes.iter().filter(move |s| s.space == space)
    }
}

static INK: LazyLock<Mutex<Ink>> = LazyLock::new(|| Mutex::new(Ink::default()));

/// Do something with the program's ink.
pub fn with<R>(f: impl FnOnce(&mut Ink) -> R) -> R {
    let mut g = INK.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut g)
}

/// Where a part of the desktop is shown on the canvas: a screen, window or area source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    /// The desktop rectangle the source shows.
    pub desk: (i32, i32, u32, u32),
    /// Its box on the canvas.
    pub canvas: (i64, i64, u32, u32),
}

impl Placed {
    /// A desktop point on the canvas.
    pub fn to_canvas(&self, p: [f32; 2]) -> [f32; 2] {
        let (sx, sy) = self.scale();
        [self.canvas.0 as f32 + (p[0] - self.desk.0 as f32) * sx, self.canvas.1 as f32 + (p[1] - self.desk.1 as f32) * sy]
    }

    /// A canvas point on the desktop (for ink drawn on the preview over a screen).
    pub fn to_desk(&self, p: [f32; 2]) -> [f32; 2] {
        let (sx, sy) = self.scale();
        [self.desk.0 as f32 + (p[0] - self.canvas.0 as f32) / sx, self.desk.1 as f32 + (p[1] - self.canvas.1 as f32) / sy]
    }

    pub fn contains_canvas(&self, p: [f32; 2]) -> bool {
        let (x, y, w, h) = self.canvas;
        p[0] >= x as f32 && p[1] >= y as f32 && p[0] < (x + w as i64) as f32 && p[1] < (y + h as i64) as f32
    }

    fn scale(&self) -> (f32, f32) {
        (self.canvas.2 as f32 / self.desk.2.max(1) as f32, self.canvas.3 as f32 / self.desk.3.max(1) as f32)
    }
}

/// How a stroke's points land in a picture: `out = (p - from) * scale + to`, clipped to `clip`
/// (left, top, right, bottom in picture pixels).
#[derive(Clone, Copy, Debug)]
pub struct Map {
    pub from: [f32; 2],
    pub scale: [f32; 2],
    pub to: [f32; 2],
    pub clip: (i64, i64, i64, i64),
}

impl Map {
    /// Points as they are, the whole picture.
    pub fn identity(out: &RgbaImage) -> Map {
        Map { from: [0.0, 0.0], scale: [1.0, 1.0], to: [0.0, 0.0], clip: (0, 0, out.width() as i64, out.height() as i64) }
    }

    /// Desktop ink onto the canvas box of `p`.
    pub fn placed(p: &Placed, out: &RgbaImage) -> Map {
        let (x, y, w, h) = p.canvas;
        let clip = (x.max(0), y.max(0), (x + w as i64).min(out.width() as i64), (y + h as i64).min(out.height() as i64));
        let s = p.scale();
        Map { from: [p.desk.0 as f32, p.desk.1 as f32], scale: [s.0, s.1], to: [x as f32, y as f32], clip }
    }

    fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        [(p[0] - self.from[0]) * self.scale[0] + self.to[0], (p[1] - self.from[1]) * self.scale[1] + self.to[1]]
    }
}

/// Draw `stroke` into `out`, `alpha` strong, with round ends and joins and soft edges. A stroke
/// over itself doesn't get darker where it crosses (one coverage per pixel), so a fading
/// drawing stays even. Blends "over" what is there, a see-through picture included.
pub fn paint(out: &mut RgbaImage, stroke: &Stroke, alpha: f32, map: &Map) {
    if stroke.points.is_empty() || alpha <= 0.0 {
        return;
    }
    let pts: Vec<[f32; 2]> = stroke.points.iter().map(|p| map.apply(*p)).collect();
    let r = (stroke.width * (map.scale[0].abs() + map.scale[1].abs()) / 2.0 / 2.0).max(0.5);
    let (mut l, mut t, mut rr, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in &pts {
        (l, t, rr, b) = (l.min(p[0]), t.min(p[1]), rr.max(p[0]), b.max(p[1]));
    }
    let x0 = ((l - r - 1.0).floor() as i64).max(map.clip.0);
    let y0 = ((t - r - 1.0).floor() as i64).max(map.clip.1);
    let x1 = ((rr + r + 2.0).ceil() as i64).min(map.clip.2);
    let y1 = ((b + r + 2.0).ceil() as i64).min(map.clip.3);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let (bw, bh) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let mut cover = vec![0u8; bw * bh];
    let segs: Vec<([f32; 2], [f32; 2])> = if pts.len() == 1 { vec![(pts[0], pts[0])] } else { pts.windows(2).map(|w| (w[0], w[1])).collect() };
    for (a, c) in segs {
        let sx0 = ((a[0].min(c[0]) - r - 1.0).floor() as i64).max(x0);
        let sy0 = ((a[1].min(c[1]) - r - 1.0).floor() as i64).max(y0);
        let sx1 = ((a[0].max(c[0]) + r + 2.0).ceil() as i64).min(x1);
        let sy1 = ((a[1].max(c[1]) + r + 2.0).ceil() as i64).min(y1);
        let (dx, dy) = (c[0] - a[0], c[1] - a[1]);
        let len2 = dx * dx + dy * dy;
        for py in sy0..sy1 {
            for px in sx0..sx1 {
                let (qx, qy) = (px as f32 + 0.5, py as f32 + 0.5);
                let tt = if len2 > 0.0 { (((qx - a[0]) * dx + (qy - a[1]) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
                let d = (qx - a[0] - tt * dx).hypot(qy - a[1] - tt * dy);
                let cv = (r + 0.5 - d).clamp(0.0, 1.0);
                if cv > 0.0 {
                    let k = (py - y0) as usize * bw + (px - x0) as usize;
                    cover[k] = cover[k].max((cv * 255.0) as u8);
                }
            }
        }
    }
    let ca = stroke.color[3] as f32 / 255.0 * alpha.min(1.0);
    let ow = out.width() as usize;
    let raw: &mut [u8] = out;
    for yy in 0..bh {
        for xx in 0..bw {
            let cv = cover[yy * bw + xx];
            if cv == 0 {
                continue;
            }
            let a = ca * cv as f32 / 255.0;
            let i = ((y0 as usize + yy) * ow + x0 as usize + xx) * 4;
            let da = raw[i + 3] as f32 / 255.0;
            let oa = a + da * (1.0 - a);
            if oa <= 0.0 {
                continue;
            }
            for k in 0..3 {
                let v = (stroke.color[k] as f32 * a + raw[i + k] as f32 * da * (1.0 - a)) / oa;
                raw[i + k] = v.round().clamp(0.0, 255.0) as u8;
            }
            raw[i + 3] = (oa * 255.0).round() as u8;
        }
    }
}

/// Draw the snapshot's desktop ink onto the source box `p` (right after the source is drawn,
/// so sources above it cover it as they cover the screen).
pub fn paint_placed(out: &mut RgbaImage, ink: &Snapshot, p: &Placed) {
    if ink.is_empty() || p.desk.2 == 0 || p.desk.3 == 0 {
        return;
    }
    let map = Map::placed(p, out);
    let (dl, dt, dr, db) = (p.desk.0 as f32, p.desk.1 as f32, p.desk.0 as f32 + p.desk.2 as f32, p.desk.1 as f32 + p.desk.3 as f32);
    for s in ink.in_space(Space::Desktop) {
        let b = s.bounds();
        if b.2 < dl || b.3 < dt || b.0 > dr || b.1 > db {
            continue;
        }
        paint(out, s, ink.alpha, &map);
    }
}

/// Draw the snapshot's preview ink on the whole canvas (on top of everything).
pub fn paint_canvas(out: &mut RgbaImage, ink: &Snapshot) {
    if ink.is_empty() {
        return;
    }
    let map = Map::identity(out);
    for s in ink.in_space(Space::Canvas) {
        paint(out, s, ink.alpha, &map);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(space: Space) -> Stroke {
        // A little off the pixel grid, so its edges fall part-way into pixels.
        Stroke { space, points: vec![[2.0, 10.3], [18.0, 10.3]], color: [255, 0, 0, 255], width: 4.0 }
    }

    fn ink_with(mode: InkMode, hold_s: f32) -> Ink {
        Ink { pen: Pen { mode, hold_s, ..Pen::default() }, ..Ink::default() }
    }

    #[test]
    fn a_stroke_is_drawn_point_by_point_and_kept_when_the_pen_lifts() {
        let t = Instant::now();
        let mut ink = Ink::default();
        ink.begin(Space::Desktop, [1.0, 1.0], t);
        assert!(ink.drawing());
        ink.extend([1.2, 1.1]); // under half a pixel: skipped
        ink.extend([5.0, 1.0]);
        ink.extend([9.0, 4.0]);
        let snap = ink.snapshot(t);
        assert_eq!(snap.strokes.len(), 1, "the stroke being drawn shows too");
        assert_eq!(snap.strokes[0].points, vec![[1.0, 1.0], [5.0, 1.0], [9.0, 4.0]]);
        ink.end(t);
        assert!(!ink.drawing());
        assert_eq!(ink.snapshot(t).strokes.len(), 1);
        assert_eq!(ink.snapshot(t).strokes[0].color, Pen::default().color, "the pen's colour and width");
        assert_eq!(ink.snapshot(t).strokes[0].width, 6.0);
        // Moving without the pen down does nothing.
        ink.extend([50.0, 50.0]);
        assert_eq!(ink.snapshot(t).strokes[0].points.len(), 3);
    }

    #[test]
    fn ink_holds_then_fades_out_in_half_a_second() {
        let t = Instant::now();
        let mut ink = ink_with(InkMode::Fade, 3.0);
        ink.begin(Space::Desktop, [0.0, 0.0], t);
        assert_eq!(ink.alpha(t + Duration::from_secs(30)), 1.0, "not while drawing");
        ink.end(t);
        assert_eq!(ink.alpha(t + Duration::from_millis(2999)), 1.0);
        assert_eq!(ink.alpha(t + Duration::from_secs(3)), 1.0);
        let mid = ink.alpha(t + Duration::from_millis(3250));
        assert!((mid - 0.5).abs() < 0.01, "half way through the fade: {mid}");
        assert!(ink.fading(t + Duration::from_millis(3250)));
        assert!(!ink.fading(t + Duration::from_secs(1)));
        assert_eq!(ink.alpha(t + Duration::from_millis(3500)), 0.0);
        assert!(ink.any(t + Duration::from_secs(1)) && !ink.any(t + Duration::from_secs(4)));
        assert_eq!(ink.next_change(t), Some(t + Duration::from_secs(3)));
        let snap = ink.snapshot(t + Duration::from_secs(4));
        assert!(snap.strokes.is_empty() && snap.is_empty(), "faded ink is forgotten");
    }

    #[test]
    fn a_new_stroke_restarts_the_clock_for_all_ink() {
        let t = Instant::now();
        let mut ink = ink_with(InkMode::Fade, 2.0);
        ink.begin(Space::Desktop, [0.0, 0.0], t);
        ink.end(t);
        let later = t + Duration::from_millis(1500);
        ink.begin(Space::Desktop, [5.0, 5.0], later);
        ink.end(later);
        let at = t + Duration::from_millis(3000);
        assert_eq!(ink.alpha(at), 1.0, "the first stroke waits for the second");
        assert_eq!(ink.snapshot(at).strokes.len(), 2);
        assert_eq!(ink.alpha(later + Duration::from_millis(2500)), 0.0, "and they go together");
        // A stroke started after everything faded starts on an empty page.
        let much_later = t + Duration::from_secs(60);
        ink.begin(Space::Canvas, [1.0, 1.0], much_later);
        assert_eq!(ink.snapshot(much_later).strokes.len(), 1);
    }

    #[test]
    fn stay_mode_keeps_ink_until_cleared_and_undo_brings_it_back() {
        let t = Instant::now();
        let mut ink = ink_with(InkMode::Stay, 3.0);
        for i in 0..3 {
            ink.begin(Space::Desktop, [i as f32, 0.0], t);
            ink.end(t);
        }
        let hour = t + Duration::from_secs(3600);
        assert_eq!(ink.alpha(hour), 1.0);
        assert_eq!(ink.snapshot(hour).strokes.len(), 3);
        assert_eq!(ink.next_change(hour), None);
        ink.undo(hour);
        assert_eq!(ink.snapshot(hour).strokes.len(), 2, "undo takes back the last stroke");
        ink.clear();
        assert!(!ink.any(hour));
        ink.undo(hour);
        assert_eq!(ink.snapshot(hour).strokes.len(), 2, "undo right after Clear brings it all back");
        ink.undo(hour);
        assert_eq!(ink.snapshot(hour).strokes.len(), 1, "then strokes one by one");
        // Clear while drawing removes the stroke being drawn too.
        ink.begin(Space::Canvas, [0.0, 0.0], hour);
        ink.clear();
        assert!(!ink.drawing() && ink.snapshot(hour).strokes.is_empty());
    }

    #[test]
    fn changing_the_hold_or_mode_restarts_the_clock_and_turning_off_lifts_the_pen() {
        let t = Instant::now();
        let mut ink = ink_with(InkMode::Stay, 3.0);
        ink.begin(Space::Desktop, [0.0, 0.0], t);
        ink.end(t);
        let later = t + Duration::from_secs(100);
        ink.set_pen(Pen { mode: InkMode::Fade, ..ink.pen }, later);
        assert_eq!(ink.alpha(later + Duration::from_secs(1)), 1.0, "not gone at once on switching to Fade");
        assert_eq!(ink.alpha(later + Duration::from_secs(4)), 0.0);
        // Turning drawing off ends the stroke being drawn (it is kept).
        ink.set_on(true, t);
        ink.begin(Space::Desktop, [0.0, 0.0], later);
        let g = ink.generation();
        ink.set_on(false, later);
        assert!(!ink.on && !ink.drawing() && ink.generation() != g);
        assert_eq!(ink.snapshot(later).strokes.len(), 2, "the earlier stroke (still holding) and this one");
    }

    #[test]
    fn long_shows_keep_a_bounded_amount_of_ink() {
        let t = Instant::now();
        let mut ink = ink_with(InkMode::Stay, 3.0);
        for i in 0..MAX_STROKES + 10 {
            ink.begin(Space::Desktop, [i as f32, 0.0], t);
            ink.end(t);
        }
        let snap = ink.snapshot(t);
        assert_eq!(snap.strokes.len(), MAX_STROKES);
        assert_eq!(snap.strokes[0].points[0], [10.0, 0.0], "the oldest went first");
        ink.begin(Space::Desktop, [0.0, 0.0], t);
        for i in 0..MAX_POINTS + 50 {
            ink.extend([i as f32, 1.0]);
        }
        assert_eq!(ink.snapshot(t).strokes.last().unwrap().points.len(), MAX_POINTS);
    }

    #[test]
    fn a_stroke_paints_its_width_with_soft_edges_and_nothing_else() {
        let mut out = RgbaImage::from_pixel(20, 20, image::Rgba([0, 0, 0, 255]));
        paint(&mut out, &line(Space::Canvas), 1.0, &Map::identity(&RgbaImage::new(20, 20)));
        assert_eq!(out.get_pixel(10, 10).0, [255, 0, 0, 255], "the middle is the pen colour");
        assert_eq!(out.get_pixel(10, 9).0, [255, 0, 0, 255]);
        assert_eq!(out.get_pixel(10, 3).0, [0, 0, 0, 255], "well away from it, untouched");
        assert_eq!(out.get_pixel(10, 16).0, [0, 0, 0, 255]);
        let edge = out.get_pixel(10, 12).0[0];
        assert!(edge > 0 && edge < 255, "a soft edge: {edge}");
        assert!(out.pixels().all(|p| p.0[3] == 255), "an opaque frame stays opaque");
        // Round ends: just past the end point, still ink.
        assert!(out.get_pixel(19, 10).0[0] > 0 && out.get_pixel(0, 10).0[0] > 0);
    }

    #[test]
    fn a_fading_stroke_blends_evenly_even_where_it_crosses_itself() {
        let cross = Stroke { space: Space::Canvas, points: vec![[2.0, 10.0], [18.0, 10.0], [10.0, 2.0], [10.0, 18.0]], color: [255, 255, 255, 255], width: 4.0 };
        let mut out = RgbaImage::from_pixel(20, 20, image::Rgba([0, 0, 0, 255]));
        paint(&mut out, &cross, 0.5, &Map::identity(&RgbaImage::new(20, 20)));
        let a = out.get_pixel(10, 10).0[0];
        let b = out.get_pixel(4, 10).0[0];
        assert!((127..=129).contains(&a), "half strong: {a}");
        assert_eq!(a, b, "the crossing isn't darker");
        // Alpha 0 draws nothing; an empty stroke draws nothing.
        let before = out.clone();
        paint(&mut out, &cross, 0.0, &Map::identity(&before));
        paint(&mut out, &Stroke { points: vec![], ..cross.clone() }, 1.0, &Map::identity(&before));
        assert_eq!(out, before);
    }

    #[test]
    fn ink_on_a_see_through_picture_is_see_through_where_it_fades() {
        // The overlay draws onto a clear picture.
        let mut out = RgbaImage::new(20, 20);
        paint(&mut out, &line(Space::Desktop), 1.0, &Map::identity(&RgbaImage::new(20, 20)));
        assert_eq!(out.get_pixel(10, 10).0, [255, 0, 0, 255]);
        assert_eq!(out.get_pixel(10, 3).0, [0, 0, 0, 0]);
        let mut out = RgbaImage::new(20, 20);
        paint(&mut out, &line(Space::Desktop), 0.5, &Map::identity(&RgbaImage::new(20, 20)));
        let p = out.get_pixel(10, 10).0;
        assert_eq!(p[0], 255, "the colour stays");
        assert!((126..=129).contains(&p[3]), "half see-through: {}", p[3]);
    }

    #[test]
    fn a_single_point_is_a_dot() {
        let dot = Stroke { space: Space::Canvas, points: vec![[10.0, 10.0]], color: [0, 255, 0, 255], width: 6.0 };
        let mut out = RgbaImage::from_pixel(20, 20, image::Rgba([0, 0, 0, 255]));
        paint(&mut out, &dot, 1.0, &Map::identity(&RgbaImage::new(20, 20)));
        assert_eq!(out.get_pixel(10, 10).0, [0, 255, 0, 255]);
        assert_eq!(out.get_pixel(10, 15).0, [0, 0, 0, 255]);
    }

    #[test]
    fn desktop_ink_lands_scaled_in_the_box_that_shows_that_screen_and_is_clipped_to_it() {
        // A 1000 x 500 screen at x 1000 (the second monitor), shown at half size from (10, 20).
        let p = Placed { desk: (1000, 0, 1000, 500), canvas: (10, 20, 500, 250) };
        assert_eq!(p.to_canvas([1200.0, 100.0]), [110.0, 70.0]);
        assert_eq!(p.to_desk([110.0, 70.0]), [1200.0, 100.0]);
        assert!(p.contains_canvas([10.0, 20.0]) && !p.contains_canvas([510.0, 20.0]));
        let ink = Snapshot {
            strokes: vec![Stroke { space: Space::Desktop, points: vec![[1100.0, 200.0], [1300.0, 200.0]], color: [0, 0, 255, 255], width: 8.0 }],
            alpha: 1.0,
        };
        let mut out = RgbaImage::from_pixel(600, 300, image::Rgba([0, 0, 0, 255]));
        paint_placed(&mut out, &ink, &p);
        // (1200, 200) on the desktop is (110, 120) on the canvas; the 8 px pen is 4 px wide here.
        assert_eq!(out.get_pixel(110, 120).0, [0, 0, 255, 255]);
        assert_eq!(out.get_pixel(110, 123).0, [0, 0, 0, 255], "scaled with the source");
        // A stroke past the box's edge is cut at the edge.
        let wide = Snapshot { strokes: vec![Stroke { points: vec![[900.0, 200.0], [1100.0, 200.0]], ..ink.strokes[0].clone() }], alpha: 1.0 };
        let mut out = RgbaImage::from_pixel(600, 300, image::Rgba([0, 0, 0, 255]));
        paint_placed(&mut out, &wide, &p);
        assert_eq!(out.get_pixel(10, 120).0, [0, 0, 255, 255]);
        assert_eq!(out.get_pixel(8, 120).0, [0, 0, 0, 255], "nothing left of the box");
        // Ink on another screen, and preview ink, don't land here.
        let other = Snapshot {
            strokes: vec![
                Stroke { points: vec![[100.0, 200.0], [300.0, 200.0]], ..ink.strokes[0].clone() },
                Stroke { space: Space::Canvas, ..ink.strokes[0].clone() },
            ],
            alpha: 1.0,
        };
        let mut out = RgbaImage::from_pixel(600, 300, image::Rgba([0, 0, 0, 255]));
        paint_placed(&mut out, &other, &p);
        assert!(out.pixels().all(|q| q.0 == [0, 0, 0, 255]));
    }

    #[test]
    fn preview_ink_is_drawn_on_the_canvas_as_it_is() {
        let ink = Snapshot { strokes: vec![line(Space::Canvas), Stroke { points: vec![[2.0, 2.0], [18.0, 2.0]], ..line(Space::Desktop) }], alpha: 1.0 };
        let mut out = RgbaImage::from_pixel(20, 20, image::Rgba([0, 0, 0, 255]));
        paint_canvas(&mut out, &ink);
        assert_eq!(out.get_pixel(10, 10).0, [255, 0, 0, 255]);
        assert_eq!(out.get_pixel(10, 1).0, [0, 0, 0, 255], "desktop ink isn't canvas ink");
    }
}
