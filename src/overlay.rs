//! The frozen-screen picker: choose an area in any shape, click a window, button or menu,
//! pick a colour or measure. Snagster's window covers the screen while it is shown.

use crate::capture::detect::{self, Detail};
use crate::capture::{CutShape, Object, PickPurpose, Shot};
use crate::settings::Settings;
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Key, PointerButton, Pos2, Rect, Sense, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Vec2, pos2, vec2,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;

const ACCENT_DEFAULT: Color32 = Color32::from_rgb(79, 124, 255);

/// The accent colour: the desktop's where it has one (`look::accent`), else Snagster's blue.
fn accent() -> Color32 {
    crate::look::system_accent().unwrap_or(ACCENT_DEFAULT)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OverlayMode {
    Pick(PickPurpose),
    Window,
    Fixed(u32, u32),
    ColorPicker,
    Ruler,
}

/// What to do with a chosen area.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AreaAction {
    Capture,
    Copy,
    Scroll { horizontal: bool },
    ManualScroll,
    RecordGif,
    GrabText,
}

pub enum Outcome {
    Pending,
    Cancel,
    /// An area of the shot (its pixels), its shape, the outline points for lasso/polygon
    /// (relative to the area), and what to do with it.
    Area { rect: Rect, shape: CutShape, points: Vec<Pos2>, action: AreaAction },
    /// Several areas of the shot, to put together in one picture.
    Multi(Vec<Rect>),
    /// The whole frozen screen.
    FullScreen,
    Color([u8; 3]),
}

pub struct Overlay {
    pub mode: OverlayMode,
    pub shape: CutShape,
    pub shot: Shot,
    tex: Option<TextureHandle>,
    /// Windows and the controls inside them, in picture pixels.
    nodes: Vec<Node>,
    /// The part the mouse wheel stepped to (kept while the mouse stays inside it).
    picked: Option<Rect>,
    /// Wheel turns not yet used up.
    wheel: f32,
    /// Controls still arriving from the worker that reads the windows, and its stop switch.
    details: Option<Details>,
    start: Option<Pos2>,
    cur: Option<Pos2>,
    path: Vec<Pos2>,
    ruler: Option<(Pos2, Pos2)>,
    /// All-in-one: the chosen area waiting for the person to pick what to do.
    chosen: Option<(Rect, CutShape, Vec<Pos2>)>,
    multi: Vec<Rect>,
}

/// A window or a control in it, in picture pixels.
#[derive(Clone, Debug, PartialEq)]
struct Node {
    rect: Rect,
    label: String,
    /// Which top-level window it is in (front to back).
    top: usize,
    /// 0 for the window itself.
    depth: usize,
}

/// An object's desktop rectangle in picture pixels, cut to `clip` (the screen, or its parent).
fn to_rect(o: &Object, origin: (i32, i32), clip: Rect) -> Rect {
    Rect::from_min_size(
        pos2((o.x - origin.0) as f32, (o.y - origin.1) as f32),
        vec2(o.width as f32, o.height as f32),
    )
    .intersect(clip)
}

/// Too small to aim at.
fn tiny(r: Rect) -> bool {
    r.width() < 6.0 || r.height() < 6.0
}

/// Every window and control as one list, windows front to back, each followed by its
/// controls. Controls are cut to their parent; tiny or off-screen ones are left out.
fn flatten(objs: &[Object], origin: (i32, i32), full: Rect) -> Vec<Node> {
    fn add(kids: &[Object], clip: Rect, top: usize, depth: usize, origin: (i32, i32), out: &mut Vec<Node>) {
        for c in kids {
            let r = to_rect(c, origin, clip);
            if tiny(r) {
                continue;
            }
            out.push(Node { rect: r, label: c.label.clone(), top, depth });
            add(&c.children, r, top, depth + 1, origin, out);
        }
    }
    let mut out = Vec::new();
    for (top, o) in objs.iter().enumerate() {
        let r = to_rect(o, origin, full);
        if tiny(r) {
            continue;
        }
        out.push(Node { rect: r, label: o.label.clone(), top, depth: 0 });
        add(&o.children, r, top, 1, origin, &mut out);
    }
    out
}

/// What's under `p`, smallest first and outward to the window: the front window holding `p`,
/// and (with `deep`) every control of it that holds `p`. Parts that nearly fill the next
/// bigger one (a client area inside its window) count once, as the bigger.
fn chain_at(nodes: &[Node], p: Pos2, deep: bool) -> Vec<usize> {
    let Some(w) = nodes.iter().position(|n| n.depth == 0 && n.rect.contains(p)) else { return Vec::new() };
    if !deep {
        return vec![w];
    }
    let top = nodes[w].top;
    let mut under: Vec<usize> = (0..nodes.len()).filter(|&i| nodes[i].top == top && nodes[i].rect.contains(p)).collect();
    under.sort_by(|&a, &b| {
        nodes[a].rect.area().total_cmp(&nodes[b].rect.area()).then(nodes[b].depth.cmp(&nodes[a].depth)).then(a.cmp(&b))
    });
    let mut out: Vec<usize> = Vec::new();
    for i in under {
        match out.last_mut() {
            Some(last) if nodes[*last].rect.area() >= nodes[i].rect.area() * 0.97 => *last = i,
            _ => out.push(i),
        }
    }
    out
}

/// Which step of `chain` to show: the part chosen before if the mouse is still inside it,
/// else the smallest, then moved `steps` outward (negative: inward).
fn step_in(chain: &[Rect], before: Option<Rect>, steps: i32) -> usize {
    if chain.is_empty() {
        return 0;
    }
    let at = before.and_then(|b| chain.iter().position(|r| *r == b)).unwrap_or(0);
    (at as i32 + steps).clamp(0, chain.len() as i32 - 1) as usize
}

/// Mouse wheel movement in notches; up (away from you) is positive.
fn wheel_notches(unit: egui::MouseWheelUnit, dy: f32) -> f32 {
    match unit {
        egui::MouseWheelUnit::Line => dy,
        egui::MouseWheelUnit::Page => dy * 3.0,
        egui::MouseWheelUnit::Point => dy / 50.0,
    }
}

/// The worker reading controls, stopped when the picker closes.
struct Details {
    rx: Receiver<Detail>,
    cancel: Arc<AtomicBool>,
}

impl Drop for Details {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Overlay {
    pub fn new(mode: OverlayMode, shape: CutShape, shot: Shot) -> Self {
        let full = Rect::from_min_size(Pos2::ZERO, vec2(shot.image.width() as f32, shot.image.height() as f32));
        let nodes = flatten(&shot.objects, shot.origin, full);
        Self {
            mode,
            shape,
            shot,
            tex: None,
            nodes,
            picked: None,
            wheel: 0.0,
            details: None,
            start: None,
            cur: None,
            path: Vec::new(),
            ruler: None,
            chosen: None,
            multi: Vec::new(),
        }
    }

    /// Start reading the controls inside the windows on screen (on a worker; the picker
    /// snaps to whole windows until each one's controls arrive).
    pub fn read_controls(&mut self, ctx: &egui::Context) {
        let (w, h) = self.shot.image.dimensions();
        let view = (self.shot.origin.0, self.shot.origin.1, w, h);
        let targets = detect::order(&self.shot.objects, self.shot.cursor, view, detect::WINDOWS);
        let cancel = Arc::new(AtomicBool::new(false));
        let rx = detect::spawn(targets, self.shot.space, cancel.clone(), Some(ctx.clone()));
        self.details = Some(Details { rx, cancel });
    }

    /// Take in the controls that have arrived.
    fn take_details(&mut self) {
        let Some(details) = &self.details else { return };
        let mut changed = false;
        while let Ok(d) = details.rx.try_recv() {
            if let Some(o) = self.shot.objects.get_mut(d.index) {
                o.children = d.children;
                changed = true;
            }
        }
        if changed {
            let full = Rect::from_min_size(Pos2::ZERO, vec2(self.shot.image.width() as f32, self.shot.image.height() as f32));
            self.nodes = flatten(&self.shot.objects, self.shot.origin, full);
        }
    }

    /// The part under `p` after `steps` wheel notches: its rectangle, name, and which step
    /// of how many it is (0 is the smallest).
    fn pick_at(&mut self, p: Pos2, deep: bool, steps: i32) -> Option<(Rect, String, usize, usize)> {
        let chain = chain_at(&self.nodes, p, deep);
        if chain.is_empty() {
            self.picked = None;
            return None;
        }
        let rects: Vec<Rect> = chain.iter().map(|i| self.nodes[*i].rect).collect();
        let at = step_in(&rects, self.picked, steps);
        self.picked = Some(rects[at]);
        Some((rects[at], self.nodes[chain[at]].label.clone(), at, rects.len()))
    }

    /// What hovering at `p` (picture pixels) highlights after `wheel` notches, as the picker
    /// would show it; `whole_window` is Ctrl held. For the button test.
    pub(crate) fn hover_at(&mut self, p: Pos2, whole_window: bool, wheel: i32) -> Option<String> {
        self.pick_at(p, !whole_window, wheel).map(|f| f.1)
    }

    fn pixel(&self, p: Pos2) -> [u8; 3] {
        let x = (p.x.max(0.0) as u32).min(self.shot.image.width().saturating_sub(1));
        let y = (p.y.max(0.0) as u32).min(self.shot.image.height().saturating_sub(1));
        let c = self.shot.image.get_pixel(x, y).0;
        [c[0], c[1], c[2]]
    }

    fn purpose(&self) -> Option<PickPurpose> {
        if let OverlayMode::Pick(p) = self.mode { Some(p) } else { None }
    }

    /// Space: the whole screen. A capture takes it straight away; anything else (a recording,
    /// the all-in-one toolbar, a GIF...) gets it as the chosen area.
    fn whole_screen(&mut self, full: Rect) -> Outcome {
        match self.purpose() {
            None | Some(PickPurpose::Capture) => Outcome::FullScreen,
            _ => self.finish(full, CutShape::Rect, Vec::new()),
        }
    }

    fn finish(&mut self, rect: Rect, shape: CutShape, points: Vec<Pos2>) -> Outcome {
        let action = match self.purpose() {
            Some(PickPurpose::AllInOne) => {
                self.chosen = Some((rect, shape, points));
                return Outcome::Pending;
            }
            Some(PickPurpose::Scrolling { horizontal }) => AreaAction::Scroll { horizontal },
            Some(PickPurpose::ManualScroll) => AreaAction::ManualScroll,
            Some(PickPurpose::Multi) => {
                self.multi.push(rect);
                return Outcome::Pending;
            }
            Some(PickPurpose::RecordGif) => AreaAction::RecordGif,
            Some(PickPurpose::GrabText) => AreaAction::GrabText,
            _ => AreaAction::Capture,
        };
        Outcome::Area { rect, shape, points, action }
    }

    fn shape_outline(&self, shape: CutShape, r: Rect, path: &[Pos2]) -> Vec<Pos2> {
        match shape {
            CutShape::Rect | CutShape::Rounded => vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()],
            CutShape::Ellipse => (0..72)
                .map(|i| {
                    let a = i as f32 / 72.0 * std::f32::consts::TAU;
                    pos2(r.center().x + a.cos() * r.width() / 2.0, r.center().y + a.sin() * r.height() / 2.0)
                })
                .collect(),
            CutShape::Star => crate::render::star_points(r),
            CutShape::Heart => crate::render::heart_points(r),
            CutShape::Triangle => vec![pos2(r.center().x, r.min.y), r.right_bottom(), r.left_bottom()],
            CutShape::Freehand | CutShape::Polygon => path.to_vec(),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, s: &Settings) -> Outcome {
        let area = ui.max_rect();
        let (iw, ih) = (self.shot.image.width() as f32, self.shot.image.height() as f32);
        let k = (area.width() / iw).min(area.height() / ih);
        let origin = area.min;
        let to_scr = |p: Pos2| pos2(origin.x + p.x * k, origin.y + p.y * k);
        let to_img = |p: Pos2| pos2(((p.x - origin.x) / k).clamp(0.0, iw), ((p.y - origin.y) / k).clamp(0.0, ih));
        let scr_rect = |r: Rect| Rect::from_two_pos(to_scr(r.min), to_scr(r.max));
        let full = Rect::from_min_size(Pos2::ZERO, vec2(iw, ih));

        if self.tex.is_none() {
            let img = &self.shot.image;
            let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
            self.tex = Some(ui.ctx().load_texture("overlay-shot", ci, TextureOptions::NEAREST));
        }
        let tex = self.tex.as_ref().unwrap().id();
        let p = ui.painter_at(area);
        p.image(tex, scr_rect(full), Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
        let dim = Color32::from_black_alpha(s.dim_alpha);
        let dim_except = |hole: Rect| {
            for piece in dim_pieces(scr_rect(full), scr_rect(hole)) {
                if piece.is_positive() {
                    p.rect_filled(piece, CornerRadius::ZERO, dim);
                }
            }
        };

        // ---- all-in-one: an area is chosen, show what can be done with it ----
        if let Some((rect, shape, points)) = self.chosen.clone() {
            dim_except(rect);
            let outline: Vec<Pos2> = self.shape_outline(shape, rect, &points.iter().map(|q| *q + rect.min.to_vec2()).collect::<Vec<_>>());
            p.add(egui::Shape::closed_line(outline.iter().map(|q| to_scr(*q)).collect(), Stroke::new(2.0, accent())));
            let sr = scr_rect(rect);
            let below = sr.max.y + 56.0 < area.max.y;
            let anchor = if below { pos2(sr.center().x, sr.max.y + 10.0) } else { pos2(sr.center().x, sr.min.y - 10.0) };
            let align = if below { Align2::CENTER_TOP } else { Align2::CENTER_BOTTOM };
            let mut picked: Option<AreaAction> = None;
            let mut cancel = false;
            egui::Area::new(egui::Id::unique("aio-bar"))
                .order(egui::Order::Foreground)
                .pivot(align)
                .fixed_pos(anchor)
                .show(ui.ctx(), |ui| {
                    egui::Frame::new()
                        .fill(Color32::from_rgb(28, 30, 37))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(egui::Margin::same(6))
                        .stroke(Stroke::new(1.0, accent()))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(format!("{} x {}", rect.width().round(), rect.height().round()))
                                        .color(Color32::from_gray(170)),
                                );
                                let b = |ui: &mut egui::Ui, t: &str, tip: &str| ui.button(t).on_hover_text(tip).clicked();
                                if ui
                                    .add(egui::Button::new(egui::RichText::new(crate::i18n::t("Capture")).strong().color(Color32::WHITE)).fill(accent()))
                                    .on_hover_text("Open it in Snagster (Enter)")
                                    .clicked()
                                {
                                    picked = Some(AreaAction::Capture);
                                }
                                if b(ui, "Copy", "Straight to the clipboard") {
                                    picked = Some(AreaAction::Copy);
                                }
                                // (Not on Wayland: scrolling capture can't work there yet.)
                                if crate::capture::scrolling_supported() {
                                    if b(ui, "Scroll down", "Keep scrolling and stitch a long picture") {
                                        picked = Some(AreaAction::Scroll { horizontal: false });
                                    }
                                    if b(ui, "Scroll sideways", "A wide panoramic picture") {
                                        picked = Some(AreaAction::Scroll { horizontal: true });
                                    }
                                    if b(ui, "Scroll yourself", "Snagster keeps capturing while you scroll; press Stop when done") {
                                        picked = Some(AreaAction::ManualScroll);
                                    }
                                }
                                if b(ui, "Record GIF", "Record this area as an animated GIF") {
                                    picked = Some(AreaAction::RecordGif);
                                }
                                if b(ui, "Grab text", "Read the text in this area") {
                                    picked = Some(AreaAction::GrabText);
                                }
                                if b(ui, "Redo", "Choose again") {
                                    self.chosen = None;
                                }
                                if b(ui, "Cancel", "Esc") {
                                    cancel = true;
                                }
                            });
                        });
                });
            if ui.input(|i| i.key_pressed(Key::Enter)) {
                picked = Some(AreaAction::Capture);
            }
            if cancel || ui.input(|i| i.key_pressed(Key::Escape)) {
                return Outcome::Cancel;
            }
            if let Some(action) = picked {
                return Outcome::Area { rect, shape, points, action };
            }
            // Redrawn on input only.
            return Outcome::Pending;
        }

        if self.purpose() == Some(PickPurpose::Multi) {
            if ui.input(|i| i.key_pressed(Key::Enter)) && !self.multi.is_empty() {
                return Outcome::Multi(self.multi.clone());
            }
            if ui.input(|i| i.key_pressed(Key::Backspace)) {
                self.multi.pop();
            }
        }
        let resp = ui.allocate_rect(area, Sense::click_and_drag());
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        let hover = resp.hover_pos().or(resp.interact_pointer_pos()).map(to_img);
        // Where the button went down: a drag is only noticed after the pointer has moved a little,
        // and the area starts at the press, not where the drag was noticed.
        let pressed = drag_start(ui.input(|i| i.pointer.press_origin()).map(to_img), hover);
        let (shift, ctrl) = ui.input(|i| (i.modifiers.shift, i.modifiers.ctrl));

        if ui.input(|i| i.key_pressed(Key::Escape)) || resp.clicked_by(PointerButton::Secondary) {
            if self.mode == OverlayMode::Ruler && self.ruler.is_some() {
                self.ruler = None;
            } else {
                return Outcome::Cancel;
            }
        }
        // Shape keys.
        let picking = matches!(self.mode, OverlayMode::Pick(_));
        if picking && self.start.is_none() {
            for sh in CutShape::ALL {
                if let Some(key) = Key::from_name(sh.key())
                    && ui.input(|i| i.key_pressed(key))
                {
                    self.shape = sh;
                    self.path.clear();
                }
            }
            if ui.input(|i| i.key_pressed(Key::Space)) {
                return self.whole_screen(full);
            }
        }

        let snapping = s.detect_windows
            && matches!(self.mode, OverlayMode::Window)
            || (s.detect_windows && picking && self.shape == CutShape::Rect && self.start.is_none());
        self.take_details();
        // The mouse wheel steps out to the part around the highlighted one, or back in.
        let turned: f32 = ui.input(|i| {
            i.events
                .iter()
                .map(|e| match e {
                    egui::Event::MouseWheel { unit, delta, .. } => wheel_notches(*unit, delta.y),
                    _ => 0.0,
                })
                .sum()
        });
        let steps = if snapping {
            self.wheel += turned;
            let n = self.wheel.trunc();
            self.wheel -= n;
            n as i32
        } else {
            self.wheel = 0.0;
            0
        };
        let found = if snapping { hover.and_then(|h| self.pick_at(h, s.detect_objects && !ctrl, steps)) } else { None };
        let level = found.as_ref().map(|f| (f.2, f.3));
        let hovered = found.map(|f| (f.0, f.1));

        let mut outcome = Outcome::Pending;
        let mut label: Option<String> = None;
        match self.mode {
            OverlayMode::Pick(_) => match self.shape {
                CutShape::Polygon => {
                    if resp.clicked_by(PointerButton::Primary)
                        && let Some(h) = hover
                    {
                        if self.path.len() >= 3 && to_scr(self.path[0]).distance(to_scr(h)) < 10.0 {
                            outcome = self.close_path();
                        } else {
                            self.path.push(h);
                        }
                    }
                    if ui.input(|i| i.key_pressed(Key::Backspace)) {
                        self.path.pop();
                    }
                    if (resp.double_clicked() || ui.input(|i| i.key_pressed(Key::Enter))) && self.path.len() >= 3 {
                        outcome = self.close_path();
                    }
                }
                CutShape::Freehand => {
                    if resp.drag_started_by(PointerButton::Primary) {
                        self.path.clear();
                        self.start = pressed;
                    }
                    if resp.dragged_by(PointerButton::Primary)
                        && let Some(h) = hover
                        && self.path.last().is_none_or(|l| l.distance(h) * k > 2.0)
                    {
                        self.path.push(h);
                    }
                    if resp.drag_stopped_by(PointerButton::Primary) {
                        self.start = None;
                        if self.path.len() >= 3 {
                            outcome = self.close_path();
                        }
                    }
                }
                _ => {
                    if resp.drag_started_by(PointerButton::Primary) {
                        self.start = pressed;
                        self.cur = hover;
                    }
                    if resp.dragged_by(PointerButton::Primary) {
                        self.cur = hover;
                    }
                    if resp.drag_stopped_by(PointerButton::Primary) {
                        if let (Some(a), Some(b)) = (self.start, self.cur) {
                            let b = if shift { crate::doc::square(a, b) } else { b };
                            let r = Rect::from_two_pos(a, b).intersect(full);
                            if r.width() >= 3.0 && r.height() >= 3.0 {
                                outcome = self.finish(r, self.shape, Vec::new());
                            }
                        }
                        self.start = None;
                        self.cur = None;
                    }
                    if resp.clicked_by(PointerButton::Primary)
                        && let Some((r, _)) = &hovered
                    {
                        outcome = self.finish(*r, CutShape::Rect, Vec::new());
                    }
                }
            },
            OverlayMode::Window => {
                if resp.clicked_by(PointerButton::Primary) {
                    let r = hovered.as_ref().map(|w| w.0).unwrap_or(full);
                    outcome = Outcome::Area { rect: r, shape: CutShape::Rect, points: Vec::new(), action: AreaAction::Capture };
                }
            }
            OverlayMode::Fixed(w, h) => {
                if let Some(c) = hover {
                    let r = fixed_frame(c, w, h, iw, ih);
                    dim_except(r);
                    p.rect_stroke(scr_rect(r), CornerRadius::ZERO, Stroke::new(2.0, accent()), StrokeKind::Outside);
                    label = Some(format!("{w} x {h}   click to capture"));
                    if resp.clicked_by(PointerButton::Primary) {
                        outcome = Outcome::Area { rect: r, shape: CutShape::Rect, points: Vec::new(), action: AreaAction::Capture };
                    }
                }
            }
            OverlayMode::ColorPicker => {
                if resp.clicked_by(PointerButton::Primary)
                    && let Some(h) = hover
                {
                    outcome = Outcome::Color(self.pixel(h));
                }
            }
            OverlayMode::Ruler => {
                if resp.drag_started_by(PointerButton::Primary) {
                    self.start = pressed;
                    self.cur = hover;
                    self.ruler = None;
                }
                if resp.dragged_by(PointerButton::Primary) {
                    self.cur = hover;
                }
                if resp.drag_stopped_by(PointerButton::Primary) {
                    if let (Some(a), Some(b)) = (self.start, self.cur) {
                        self.ruler = Some((a, b));
                    }
                    self.start = None;
                    self.cur = None;
                }
            }
        }

        // ---- paint the selection ----
        let fixed = matches!(self.mode, OverlayMode::Fixed(..));
        if picking && !self.path.is_empty() {
            let r = Rect::from_points(&self.path);
            dim_except(r);
            let pts: Vec<Pos2> = self.path.iter().map(|q| to_scr(*q)).collect();
            if self.shape == CutShape::Polygon {
                p.add(egui::Shape::line(pts.clone(), Stroke::new(2.0, accent())));
                if let (Some(last), Some(h)) = (pts.last(), hover) {
                    p.line_segment([*last, to_scr(h)], Stroke::new(1.0, Color32::WHITE));
                }
                for q in &pts {
                    p.circle_filled(*q, 4.0, Color32::WHITE);
                }
                label = Some(format!("{} points. Click the first point, double-click or press Enter to finish.", pts.len()));
            } else {
                p.add(egui::Shape::line(pts, Stroke::new(2.0, accent())));
            }
        } else if picking && self.start.is_some() {
            let a = self.start.unwrap();
            let b = self.cur.unwrap_or(a);
            let b = if shift { crate::doc::square(a, b) } else { b };
            let r = Rect::from_two_pos(a, b);
            dim_except(r);
            let outline = self.shape_outline(self.shape, r, &[]);
            p.add(egui::Shape::closed_line(outline.iter().map(|q| to_scr(*q)).collect(), Stroke::new(2.0, accent())));
            if self.shape != CutShape::Rect {
                p.rect_stroke(scr_rect(r), CornerRadius::ZERO, Stroke::new(1.0, Color32::from_white_alpha(90)), StrokeKind::Middle);
            }
            label = Some(format!("{} x {}", r.width().round(), r.height().round()));
        } else if let Some((r, title)) = &hovered {
            dim_except(*r);
            let sr = scr_rect(*r);
            // A bright outline that shows on dark and light programs alike, and the size.
            p.rect_stroke(sr, CornerRadius::ZERO, Stroke::new(1.0, Color32::WHITE), StrokeKind::Outside);
            p.rect_stroke(sr, CornerRadius::ZERO, Stroke::new(3.0, accent()), StrokeKind::Inside);
            let size = format!("{} x {}", r.width().round(), r.height().round());
            let tag = size_tag(&size, level);
            let above = sr.min.y - 26.0 > area.min.y;
            let at = if above { pos2(sr.min.x, sr.min.y - 4.0) } else { pos2(sr.min.x + 4.0, sr.min.y + 4.0) };
            pill(&p, at, if above { Align2::LEFT_BOTTOM } else { Align2::LEFT_TOP }, &tag, 13.0);
            if level.is_some_and(|(_, n)| n > 1) {
                let hint = crate::i18n::t("Click: capture this. Wheel: bigger or smaller part.");
                pill(&p, pos2(area.center().x, area.min.y + 64.0), Align2::CENTER_TOP, hint, 13.0);
            }
            label = Some(format!("{title}   {size}"));
        } else if !fixed && self.mode != OverlayMode::ColorPicker {
            p.rect_filled(scr_rect(full), CornerRadius::ZERO, dim);
        }

        for (n, r) in self.multi.iter().enumerate() {
            let sr = scr_rect(*r);
            let uv = Rect::from_min_max(pos2(r.min.x / iw, r.min.y / ih), pos2(r.max.x / iw, r.max.y / ih));
            p.image(tex, sr, uv, Color32::WHITE);
            p.rect_stroke(sr, CornerRadius::ZERO, Stroke::new(2.0, accent()), StrokeKind::Outside);
            pill(&p, sr.left_top() + vec2(4.0, 4.0), Align2::LEFT_TOP, &format!("{}", n + 1), 13.0);
        }
        if self.mode == OverlayMode::Ruler {
            let line = match (self.start, self.cur, self.ruler) {
                (Some(a), Some(b), _) => Some((a, b)),
                (_, _, Some(l)) => Some(l),
                _ => None,
            };
            if let Some((a, b)) = line {
                p.line_segment([to_scr(a), to_scr(b)], Stroke::new(2.0, Color32::from_rgb(255, 197, 61)));
                p.rect_stroke(scr_rect(Rect::from_two_pos(a, b)), CornerRadius::ZERO, Stroke::new(1.0, Color32::WHITE), StrokeKind::Middle);
                let d = b - a;
                let ang = (-d.y).atan2(d.x).to_degrees();
                label = Some(format!("{:.0} x {:.0}   length {:.1} px   angle {:.1} deg", d.x.abs(), d.y.abs(), d.length(), ang));
            }
        }

        if let Some(h) = hover {
            let hs = to_scr(h);
            if s.show_crosshair {
                let f = scr_rect(full);
                let st = Stroke::new(1.0, Color32::from_white_alpha(140));
                p.line_segment([pos2(f.min.x, hs.y), pos2(f.max.x, hs.y)], st);
                p.line_segment([pos2(hs.x, f.min.y), pos2(hs.x, f.max.y)], st);
            }
            let px = self.pixel(h);
            let hex = format!("#{:02X}{:02X}{:02X}", px[0], px[1], px[2]);
            if self.mode == OverlayMode::ColorPicker {
                label = Some(format!("{hex}   rgb({}, {}, {})", px[0], px[1], px[2]));
            } else if label.is_none() {
                label = Some(format!("{}, {}", h.x.floor(), h.y.floor()));
            }
            let mag_on = s.show_magnifier || self.mode == OverlayMode::ColorPicker;
            let box_size = 132.0;
            let mut mag_pos = hs + vec2(24.0, 24.0);
            if mag_pos.x + box_size > area.max.x {
                mag_pos.x = hs.x - 24.0 - box_size;
            }
            if mag_pos.y + box_size + 50.0 > area.max.y {
                mag_pos.y = hs.y - 24.0 - box_size - 50.0;
            }
            if mag_on {
                let cells = magnifier_cells(box_size, s.magnifier_zoom);
                let cell = box_size / cells;
                let (cx, cy) = (h.x.floor(), h.y.floor());
                let src = Rect::from_min_size(pos2(cx - (cells - 1.0) / 2.0, cy - (cells - 1.0) / 2.0), Vec2::splat(cells));
                let uv = Rect::from_min_max(pos2(src.min.x / iw, src.min.y / ih), pos2(src.max.x / iw, src.max.y / ih));
                let mrect = Rect::from_min_size(mag_pos, Vec2::splat(cells * cell));
                p.rect_filled(mrect.expand(3.0), CornerRadius::same(4), Color32::from_rgb(24, 25, 30));
                p.image(tex, mrect, uv, Color32::WHITE);
                let grid = Stroke::new(1.0, Color32::from_black_alpha(50));
                for i in 1..cells as i32 {
                    let o = i as f32 * cell;
                    p.line_segment([pos2(mrect.min.x + o, mrect.min.y), pos2(mrect.min.x + o, mrect.max.y)], grid);
                    p.line_segment([pos2(mrect.min.x, mrect.min.y + o), pos2(mrect.max.x, mrect.min.y + o)], grid);
                }
                let mid = (cells - 1.0) / 2.0;
                let center = Rect::from_min_size(mrect.min + Vec2::splat(mid * cell), Vec2::splat(cell));
                p.rect_stroke(center, CornerRadius::ZERO, Stroke::new(2.0, Color32::WHITE), StrokeKind::Middle);
                p.rect_stroke(mrect, CornerRadius::same(2), Stroke::new(2.0, accent()), StrokeKind::Outside);
                let swatch = Rect::from_min_size(pos2(mrect.min.x, mrect.max.y + 6.0), vec2(18.0, 18.0));
                p.rect_filled(swatch, CornerRadius::same(3), Color32::from_rgb(px[0], px[1], px[2]));
                p.rect_stroke(swatch, CornerRadius::same(3), Stroke::new(1.0, Color32::WHITE), StrokeKind::Outside);
                pill(&p, pos2(swatch.max.x + 6.0, swatch.min.y - 2.0), Align2::LEFT_TOP, &hex, 13.0);
            }
            if s.show_info
                && let Some(text) = &label
            {
                let at = if mag_on { pos2(mag_pos.x, mag_pos.y + box_size + 32.0) } else { hs + vec2(18.0, 18.0) };
                pill(&p, at, Align2::LEFT_TOP, text, 13.0);
            }
        }

        let help = match self.mode {
            OverlayMode::Pick(PickPurpose::AllInOne) => format!(
                "{}: drag to choose, or click a window, button or menu (Ctrl = whole window). Space = full screen. Keys: R E O F P S H T change shape. Esc cancels.",
                self.shape.label()
            ),
            OverlayMode::Pick(PickPurpose::Scrolling { .. }) => "Choose the scrolling part of the window, then wait while Snagster scrolls it.".into(),
            OverlayMode::Pick(PickPurpose::RecordGif) => "Choose the area to record.".into(),
            OverlayMode::Pick(PickPurpose::GrabText) => "Choose the text to read.".into(),
            OverlayMode::Pick(PickPurpose::Multi) => format!("{} areas chosen. Drag more areas, Backspace removes the last, Enter puts them together. Esc cancels.", self.multi.len()),
            OverlayMode::Pick(PickPurpose::ManualScroll) => "Choose the part that scrolls. Then scroll it yourself and press Stop when you reach the end.".into(),
            OverlayMode::Pick(_) => format!("{}: drag to choose. Keys R E O F P S H T change shape. Esc cancels.", self.shape.label()),
            OverlayMode::Window => "Click the window, button or menu to capture (Ctrl = whole window). Esc cancels.".into(),
            OverlayMode::Fixed(..) => "Move the frame and click to capture. Esc cancels.".into(),
            OverlayMode::ColorPicker => "Click to pick a color. It is copied as #RRGGBB. Esc cancels.".into(),
            OverlayMode::Ruler => "Drag to measure. Esc or right-click closes the ruler.".into(),
        };
        pill(&p, pos2(area.center().x, area.min.y + 24.0), Align2::CENTER_TOP, &crate::i18n::tr(&help), 15.0);
        // No constant redrawing: egui redraws when the mouse moves or a key is pressed.
        outcome
    }

    fn close_path(&mut self) -> Outcome {
        let r = Rect::from_points(&self.path);
        let pts: Vec<Pos2> = self.path.iter().map(|q| *q - r.min.to_vec2()).collect();
        let shape = self.shape;
        self.path.clear();
        if r.width() < 3.0 || r.height() < 3.0 {
            return Outcome::Pending;
        }
        self.finish(r, shape, pts)
    }
}

/// The four strips of `full` around `hole` (above, below, left, right) that get dimmed.
/// Strips can be empty or negative when the hole touches or passes an edge.
fn dim_pieces(f: Rect, h: Rect) -> [Rect; 4] {
    [
        Rect::from_min_max(f.min, pos2(f.max.x, h.min.y)),
        Rect::from_min_max(pos2(f.min.x, h.max.y), f.max),
        Rect::from_min_max(pos2(f.min.x, h.min.y), pos2(h.min.x, h.max.y)),
        Rect::from_min_max(pos2(h.max.x, h.min.y), pos2(f.max.x, h.max.y)),
    ]
}

/// The fixed-size frame centred on `c`, pushed back inside an `iw` x `ih` picture.
fn fixed_frame(c: Pos2, w: u32, h: u32, iw: f32, ih: f32) -> Rect {
    let size = vec2((w as f32).min(iw), (h as f32).min(ih));
    let r = Rect::from_center_size(c, size);
    r.translate(vec2((-r.min.x).max(0.0) - (r.max.x - iw).max(0.0), (-r.min.y).max(0.0) - (r.max.y - ih).max(0.0)))
}

/// How many pixels across the magnifier shows: odd, so one sits in the middle, and at least 3.
fn magnifier_cells(box_size: f32, zoom: u32) -> f32 {
    let zoom = zoom.clamp(2, 32) as f32;
    let cells = (box_size / zoom).floor().max(3.0);
    if cells as i32 % 2 == 0 { cells - 1.0 } else { cells }
}

/// The size shown on a highlighted part and, when the wheel can change it, which of the
/// nested parts it is (1 is the whole window).
fn size_tag(size: &str, level: Option<(usize, usize)>) -> String {
    match level {
        Some((at, n)) if n > 1 => format!("{size}   {}/{n}", n - at),
        _ => size.to_string(),
    }
}

fn pill(p: &egui::Painter, at: Pos2, align: Align2, text: &str, size: f32) {
    let galley = p.layout(text.to_string(), FontId::proportional(size), Color32::WHITE, 900.0);
    let r = align.anchor_size(at, galley.size() + vec2(16.0, 8.0));
    p.rect_filled(r, CornerRadius::same(6), Color32::from_rgba_unmultiplied(20, 22, 28, 225));
    p.galley(r.min + vec2(8.0, 4.0), galley, Color32::WHITE);
}

/// Where a drag starts: the press, when it is known, else the pointer now.
fn drag_start(press: Option<Pos2>, hover: Option<Pos2>) -> Option<Pos2> {
    press.or(hover)
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_area_starts_where_the_button_went_down() {
        use super::drag_start;
        use eframe::egui::pos2;
        // The drag was noticed 20 px further on: the area still starts at the press.
        assert_eq!(drag_start(Some(pos2(900.0, 700.0)), Some(pos2(920.0, 715.0))), Some(pos2(900.0, 700.0)));
        assert_eq!(drag_start(None, Some(pos2(5.0, 6.0))), Some(pos2(5.0, 6.0)));
    }

    use super::*;
    use image::{Rgba, RgbaImage};

    fn obj(x: i32, y: i32, w: u32, h: u32, label: &str, children: Vec<Object>) -> Object {
        Object { x, y, width: w, height: h, label: label.into(), children, handle: 0, pid: 0 }
    }

    fn shot(w: u32, h: u32, origin: (i32, i32), objects: Vec<Object>) -> Shot {
        let image = RgbaImage::from_fn(w, h, |x, y| Rgba([x as u8, y as u8, 7, 255]));
        Shot { image, origin, objects, title: String::new(), app: String::new(), cursor: None, space: detect::Space::default() }
    }

    /// The label of what is picked at `p` (no wheel).
    fn at(o: &mut Overlay, p: Pos2, deep: bool) -> Option<String> {
        o.picked = None;
        o.pick_at(p, deep, 0).map(|f| f.1)
    }

    fn pick(p: PickPurpose) -> Overlay {
        Overlay::new(OverlayMode::Pick(p), CutShape::Rect, shot(200, 100, (0, 0), Vec::new()))
    }

    fn r(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_min_max(pos2(x0, y0), pos2(x1, y1))
    }

    #[test]
    fn objects_move_from_desktop_to_picture_pixels() {
        let full = r(0.0, 0.0, 1920.0, 1080.0);
        // The screen starts at x = -1920 (a monitor to the left of the primary one).
        let o = obj(-1800, 100, 400, 300, "W", Vec::new());
        assert_eq!(to_rect(&o, (-1920, 0), full), r(120.0, 100.0, 520.0, 400.0));
        // Windows hanging off the screen are cut to it.
        let o = obj(-2000, -50, 400, 300, "W", Vec::new());
        assert_eq!(to_rect(&o, (-1920, 0), full), r(0.0, 0.0, 320.0, 250.0));
    }

    #[test]
    fn tiny_and_off_screen_objects_are_dropped() {
        let objects = vec![
            obj(10, 10, 100, 80, "Window", vec![obj(20, 20, 30, 20, "Button", Vec::new()), obj(20, 50, 5, 5, "Dot", Vec::new())]),
            obj(150, 10, 5, 50, "Thin", Vec::new()),
            obj(5000, 10, 100, 100, "Elsewhere", Vec::new()),
        ];
        let o = Overlay::new(OverlayMode::Window, CutShape::Rect, shot(200, 100, (0, 0), objects));
        let labels: Vec<(&str, usize)> = o.nodes.iter().map(|n| (n.label.as_str(), n.depth)).collect();
        assert_eq!(labels, [("Window", 0), ("Button", 1)]);
    }

    #[test]
    fn the_front_window_and_its_smallest_control_are_picked() {
        let objects = vec![
            obj(10, 10, 100, 80, "Front", vec![obj(20, 20, 60, 40, "Panel", Vec::new()), obj(30, 30, 20, 10, "Button", Vec::new())]),
            obj(0, 0, 200, 100, "Back", Vec::new()),
        ];
        let mut o = Overlay::new(OverlayMode::Window, CutShape::Rect, shot(200, 100, (0, 0), objects));
        let p = pos2(35.0, 33.0);
        assert_eq!(at(&mut o, p, false).unwrap(), "Front");
        o.picked = None;
        let (rect, label, step, of) = o.pick_at(p, true, 0).unwrap();
        assert_eq!((label.as_str(), step, of), ("Button", 0, 3));
        assert_eq!(rect, r(30.0, 30.0, 50.0, 40.0));
        assert_eq!(at(&mut o, pos2(25.0, 70.0), true).unwrap(), "Front");
        assert_eq!(at(&mut o, pos2(150.0, 50.0), true).unwrap(), "Back", "the back window shows where the front one doesn't");
    }

    #[test]
    fn a_control_that_fills_its_window_is_skipped() {
        let objects = vec![obj(0, 0, 100, 100, "Window", vec![obj(0, 0, 100, 99, "Client", Vec::new())])];
        let mut o = Overlay::new(OverlayMode::Window, CutShape::Rect, shot(200, 100, (0, 0), objects));
        assert_eq!(at(&mut o, pos2(50.0, 50.0), true).unwrap(), "Window");
        assert!(at(&mut o, pos2(150.0, 50.0), true).is_none());
    }

    /// A window with a toolbar holding a button group holding a button, and a list.
    fn nested() -> Overlay {
        let objects = vec![obj(
            0,
            0,
            400,
            300,
            "Window",
            vec![
                obj(0, 0, 400, 60, "Toolbar", vec![obj(10, 10, 120, 40, "Group", vec![obj(15, 15, 40, 30, "Button: Save", Vec::new())])]),
                obj(0, 60, 400, 240, "List", vec![obj(0, 60, 400, 20, "Row", Vec::new())]),
            ],
        )];
        Overlay::new(OverlayMode::Window, CutShape::Rect, shot(400, 300, (0, 0), objects))
    }

    #[test]
    fn the_wheel_steps_out_to_parents_and_back_in() {
        let mut o = nested();
        let p = pos2(20.0, 20.0);
        let name = |o: &mut Overlay, steps: i32| o.pick_at(p, true, steps).map(|f| f.1).unwrap();
        assert_eq!(name(&mut o, 0), "Button: Save");
        assert_eq!(name(&mut o, 1), "Group");
        assert_eq!(name(&mut o, 1), "Toolbar");
        assert_eq!(name(&mut o, 1), "Window");
        assert_eq!(name(&mut o, 5), "Window", "it stops at the window");
        assert_eq!(name(&mut o, -2), "Group");
        assert_eq!(name(&mut o, -9), "Button: Save", "and at the smallest");
        // Moving within the chosen part keeps it; moving out of it starts again at the smallest.
        assert_eq!(name(&mut o, 2), "Toolbar");
        assert_eq!(o.pick_at(pos2(300.0, 30.0), true, 0).unwrap().1, "Toolbar");
        assert_eq!(o.pick_at(pos2(200.0, 70.0), true, 0).unwrap().1, "Row");
        // Off every window, nothing is kept.
        let mut o = nested();
        o.pick_at(p, true, 1);
        assert!(o.pick_at(pos2(500.0, 500.0), true, 0).is_none());
        assert!(o.picked.is_none());
    }

    #[test]
    fn whole_window_ignores_the_controls() {
        let mut o = nested();
        let (rect, label, _, of) = o.pick_at(pos2(20.0, 20.0), false, 0).unwrap();
        assert_eq!((label.as_str(), of), ("Window", 1));
        assert_eq!(rect, r(0.0, 0.0, 400.0, 300.0));
    }

    #[test]
    fn controls_are_cut_to_their_parent_and_the_screen() {
        // A list row reaching past its list, and a window hanging off the screen's left edge.
        let objects = vec![obj(-50, 10, 200, 80, "Window", vec![obj(-50, 20, 100, 60, "List", vec![obj(-60, 30, 400, 10, "Row", Vec::new())])])];
        let o = Overlay::new(OverlayMode::Window, CutShape::Rect, shot(300, 100, (0, 0), objects));
        let rects: Vec<Rect> = o.nodes.iter().map(|n| n.rect).collect();
        assert_eq!(rects, [r(0.0, 10.0, 150.0, 90.0), r(0.0, 20.0, 50.0, 80.0), r(0.0, 30.0, 50.0, 40.0)]);
        // Pictures of a second screen: desktop positions move by its origin.
        let objects = vec![obj(1930, 20, 100, 50, "Right", vec![obj(1940, 30, 20, 20, "Button", Vec::new())])];
        let o = Overlay::new(OverlayMode::Window, CutShape::Rect, shot(300, 100, (1920, 0), objects));
        assert_eq!(o.nodes[1].rect, r(20.0, 30.0, 40.0, 50.0));
    }

    #[test]
    fn controls_that_arrive_later_are_used() {
        let mut o = Overlay::new(OverlayMode::Window, CutShape::Rect, shot(200, 100, (0, 0), vec![obj(0, 0, 200, 100, "Window", Vec::new())]));
        assert_eq!(at(&mut o, pos2(15.0, 15.0), true).unwrap(), "Window");
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        o.details = Some(Details { rx, cancel: cancel.clone() });
        tx.send(Detail { index: 0, children: vec![obj(10, 10, 30, 20, "Button", Vec::new())] }).unwrap();
        tx.send(Detail { index: 7, children: Vec::new() }).unwrap();
        o.take_details();
        assert_eq!(at(&mut o, pos2(15.0, 15.0), true).unwrap(), "Button");
        // Closing the picker stops the worker.
        drop(o);
        assert!(cancel.load(Ordering::Relaxed));
    }

    #[test]
    fn z_order_decides_between_overlapping_windows() {
        let objects = vec![
            obj(50, 0, 100, 100, "Popup menu", vec![obj(55, 5, 90, 20, "Menu item", Vec::new())]),
            obj(0, 0, 200, 100, "Editor", vec![obj(60, 5, 80, 20, "Behind the menu", Vec::new())]),
        ];
        let mut o = Overlay::new(OverlayMode::Window, CutShape::Rect, shot(200, 100, (0, 0), objects));
        assert_eq!(at(&mut o, pos2(70.0, 10.0), true).unwrap(), "Menu item");
        assert_eq!(at(&mut o, pos2(10.0, 10.0), true).unwrap(), "Editor");
    }

    #[test]
    fn wheel_turns_become_notches() {
        use egui::MouseWheelUnit::*;
        assert_eq!(wheel_notches(Line, 1.0), 1.0);
        assert_eq!(wheel_notches(Line, -2.0), -2.0);
        assert_eq!(wheel_notches(Point, 50.0), 1.0);
        assert_eq!(wheel_notches(Page, 1.0), 3.0);
        assert_eq!(step_in(&[], None, 3), 0);
        let rs = [r(0.0, 0.0, 1.0, 1.0), r(0.0, 0.0, 2.0, 2.0)];
        assert_eq!(step_in(&rs, Some(r(5.0, 5.0, 6.0, 6.0)), 0), 0, "a part the mouse left is forgotten");
        assert_eq!(step_in(&rs, Some(rs[1]), 0), 1);
        assert_eq!(size_tag("40 x 30", Some((0, 4))), "40 x 30   4/4");
        assert_eq!(size_tag("40 x 30", Some((3, 4))), "40 x 30   1/4");
        assert_eq!(size_tag("40 x 30", Some((0, 1))), "40 x 30");
        assert_eq!(size_tag("40 x 30", None), "40 x 30");
    }

    #[test]
    fn finishing_an_area_does_what_the_pick_was_for() {
        let area = r(10.0, 10.0, 50.0, 40.0);
        let cases = [
            (PickPurpose::Capture, AreaAction::Capture),
            (PickPurpose::Scrolling { horizontal: true }, AreaAction::Scroll { horizontal: true }),
            (PickPurpose::Scrolling { horizontal: false }, AreaAction::Scroll { horizontal: false }),
            (PickPurpose::ManualScroll, AreaAction::ManualScroll),
            (PickPurpose::RecordGif, AreaAction::RecordGif),
            (PickPurpose::GrabText, AreaAction::GrabText),
        ];
        for (purpose, want) in cases {
            match pick(purpose).finish(area, CutShape::Ellipse, Vec::new()) {
                Outcome::Area { rect, shape, action, .. } => {
                    assert_eq!(rect, area);
                    assert_eq!(shape, CutShape::Ellipse);
                    assert_eq!(action, want, "{purpose:?}");
                }
                _ => panic!("{purpose:?} should give an area"),
            }
        }
        let mut w = Overlay::new(OverlayMode::Window, CutShape::Rect, shot(200, 100, (0, 0), Vec::new()));
        assert!(matches!(w.finish(area, CutShape::Rect, Vec::new()), Outcome::Area { action: AreaAction::Capture, .. }));
    }

    /// Space in "Choose the area to record" took a picture of the screen instead of recording it.
    #[test]
    fn space_picks_the_whole_screen_for_what_the_pick_is_for() {
        let full = r(0.0, 0.0, 200.0, 100.0);
        assert!(matches!(pick(PickPurpose::Capture).whole_screen(full), Outcome::FullScreen));
        match pick(PickPurpose::RecordGif).whole_screen(full) {
            Outcome::Area { rect, shape, action, .. } => {
                assert_eq!(rect, full);
                assert_eq!(shape, CutShape::Rect);
                assert_eq!(action, AreaAction::RecordGif);
            }
            _ => panic!("a recording should get the whole screen as its area"),
        }
        assert!(matches!(pick(PickPurpose::GrabText).whole_screen(full), Outcome::Area { action: AreaAction::GrabText, .. }));
        // The all-in-one toolbar opens around the whole screen.
        let mut o = pick(PickPurpose::AllInOne);
        assert!(matches!(o.whole_screen(full), Outcome::Pending));
        assert_eq!(o.chosen.as_ref().map(|c| c.0), Some(full));
    }

    #[test]
    fn all_in_one_waits_for_a_choice() {
        let mut o = pick(PickPurpose::AllInOne);
        let area = r(1.0, 2.0, 30.0, 40.0);
        assert!(matches!(o.finish(area, CutShape::Star, vec![pos2(1.0, 1.0)]), Outcome::Pending));
        let (rect, shape, points) = o.chosen.clone().unwrap();
        assert_eq!((rect, shape, points.len()), (area, CutShape::Star, 1));
    }

    #[test]
    fn multi_collects_areas() {
        let mut o = pick(PickPurpose::Multi);
        assert!(matches!(o.finish(r(0.0, 0.0, 10.0, 10.0), CutShape::Rect, Vec::new()), Outcome::Pending));
        assert!(matches!(o.finish(r(20.0, 0.0, 30.0, 10.0), CutShape::Rect, Vec::new()), Outcome::Pending));
        assert_eq!(o.multi.len(), 2);
        assert_eq!(o.multi[1], r(20.0, 0.0, 30.0, 10.0));
    }

    #[test]
    fn closing_a_path_makes_points_relative_to_its_box() {
        let mut o = pick(PickPurpose::Capture);
        o.shape = CutShape::Polygon;
        o.path = vec![pos2(10.0, 20.0), pos2(50.0, 20.0), pos2(30.0, 60.0)];
        match o.close_path() {
            Outcome::Area { rect, shape, points, .. } => {
                assert_eq!(rect, r(10.0, 20.0, 50.0, 60.0));
                assert_eq!(shape, CutShape::Polygon);
                assert_eq!(points, vec![pos2(0.0, 0.0), pos2(40.0, 0.0), pos2(20.0, 40.0)]);
            }
            _ => panic!("expected an area"),
        }
        assert!(o.path.is_empty());
    }

    #[test]
    fn a_path_too_thin_to_cut_is_ignored() {
        let mut o = pick(PickPurpose::Capture);
        o.shape = CutShape::Freehand;
        o.path = vec![pos2(10.0, 20.0), pos2(50.0, 21.0), pos2(30.0, 22.0)];
        assert!(matches!(o.close_path(), Outcome::Pending));
        assert!(o.path.is_empty());
    }

    #[test]
    fn outlines_follow_the_shape() {
        let o = pick(PickPurpose::Capture);
        let b = r(10.0, 20.0, 110.0, 70.0);
        assert_eq!(o.shape_outline(CutShape::Rect, b, &[]), vec![b.left_top(), b.right_top(), b.right_bottom(), b.left_bottom()]);
        assert_eq!(o.shape_outline(CutShape::Rounded, b, &[]).len(), 4);
        assert_eq!(o.shape_outline(CutShape::Triangle, b, &[]), vec![pos2(60.0, 20.0), b.right_bottom(), b.left_bottom()]);
        let ell = o.shape_outline(CutShape::Ellipse, b, &[]);
        assert_eq!(ell.len(), 72);
        for q in &ell {
            let (dx, dy) = ((q.x - 60.0) / 50.0, (q.y - 45.0) / 25.0);
            assert!((dx * dx + dy * dy - 1.0).abs() < 1e-3);
        }
        let path = [pos2(1.0, 1.0), pos2(5.0, 9.0)];
        assert_eq!(o.shape_outline(CutShape::Freehand, b, &path), path.to_vec());
        assert_eq!(o.shape_outline(CutShape::Polygon, b, &path), path.to_vec());
        for sh in [CutShape::Star, CutShape::Heart] {
            let pts = o.shape_outline(sh, b, &[]);
            assert!(pts.len() >= 10);
            // (The heart's tip dips a hair below its box.)
            assert!(pts.iter().all(|q| b.expand(2.0).contains(*q)), "{sh:?}");
        }
    }

    #[test]
    fn pixel_reads_the_frozen_screen_and_clamps() {
        let o = pick(PickPurpose::Capture);
        assert_eq!(o.pixel(pos2(12.7, 34.2)), [12, 34, 7]);
        assert_eq!(o.pixel(pos2(-5.0, -5.0)), [0, 0, 7]);
        assert_eq!(o.pixel(pos2(5000.0, 5000.0)), [199, 99, 7]);
    }

    #[test]
    fn dimmed_strips_and_the_hole_cover_the_screen() {
        let f = r(0.0, 0.0, 200.0, 100.0);
        let hole = r(50.0, 20.0, 120.0, 70.0);
        let pieces = dim_pieces(f, hole);
        let area: f32 = pieces.iter().filter(|p| p.is_positive()).map(|p| p.area()).sum();
        assert_eq!(area + hole.area(), f.area());
        for (i, a) in pieces.iter().enumerate() {
            assert!(!a.intersects(hole.shrink(0.01)), "piece {i} covers the hole");
            for b in &pieces[i + 1..] {
                assert!(!a.shrink(0.01).intersects(b.shrink(0.01)));
            }
        }
        // A hole at the corner leaves empty strips, which aren't painted.
        let pieces = dim_pieces(f, r(0.0, 0.0, 50.0, 50.0));
        assert_eq!(pieces.iter().filter(|p| p.is_positive()).count(), 2);
    }

    #[test]
    fn fixed_frame_stays_on_the_picture() {
        assert_eq!(fixed_frame(pos2(100.0, 50.0), 40, 20, 200.0, 100.0), r(80.0, 40.0, 120.0, 60.0));
        assert_eq!(fixed_frame(pos2(5.0, 5.0), 40, 20, 200.0, 100.0), r(0.0, 0.0, 40.0, 20.0));
        assert_eq!(fixed_frame(pos2(199.0, 99.0), 40, 20, 200.0, 100.0), r(160.0, 80.0, 200.0, 100.0));
        // Bigger than the picture: the whole picture.
        assert_eq!(fixed_frame(pos2(10.0, 90.0), 4000, 3000, 200.0, 100.0), r(0.0, 0.0, 200.0, 100.0));
    }

    #[test]
    fn magnifier_shows_an_odd_number_of_pixels() {
        assert_eq!(magnifier_cells(132.0, 8), 15.0);
        assert_eq!(magnifier_cells(132.0, 12), 11.0);
        assert_eq!(magnifier_cells(132.0, 32), 3.0);
        // Zoom is kept between 2 and 32.
        assert_eq!(magnifier_cells(132.0, 0), magnifier_cells(132.0, 2));
        assert_eq!(magnifier_cells(132.0, 1000), 3.0);
        for z in 0..40 {
            let c = magnifier_cells(132.0, z);
            assert!(c >= 3.0 && c as i32 % 2 == 1, "zoom {z}: {c}");
        }
    }
}
