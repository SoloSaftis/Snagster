//! The frozen-screen picker: choose an area in any shape, click a window, button or menu,
//! pick a colour or measure. Snagster's window covers the screen while it is shown.

use crate::capture::{CutShape, Object, PickPurpose, Shot};
use crate::settings::Settings;
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Key, PointerButton, Pos2, Rect, Sense, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Vec2, pos2, vec2,
};

pub const ACCENT: Color32 = Color32::from_rgb(79, 124, 255);

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
    objects: Vec<(Rect, String, Vec<(Rect, String)>)>,
    start: Option<Pos2>,
    cur: Option<Pos2>,
    path: Vec<Pos2>,
    ruler: Option<(Pos2, Pos2)>,
    /// All-in-one: the chosen area waiting for the person to pick what to do.
    chosen: Option<(Rect, CutShape, Vec<Pos2>)>,
    multi: Vec<Rect>,
}

fn to_rect(o: &Object, origin: (i32, i32), full: Rect) -> Rect {
    Rect::from_min_size(
        pos2((o.x - origin.0) as f32, (o.y - origin.1) as f32),
        vec2(o.width as f32, o.height as f32),
    )
    .intersect(full)
}

impl Overlay {
    pub fn new(mode: OverlayMode, shape: CutShape, shot: Shot) -> Self {
        let full = Rect::from_min_size(Pos2::ZERO, vec2(shot.image.width() as f32, shot.image.height() as f32));
        let objects = shot
            .objects
            .iter()
            .filter_map(|o| {
                let r = to_rect(o, shot.origin, full);
                if r.width() < 6.0 || r.height() < 6.0 {
                    return None;
                }
                let kids = o
                    .children
                    .iter()
                    .map(|c| (to_rect(c, shot.origin, full), c.label.clone()))
                    .filter(|(r, _)| r.width() >= 6.0 && r.height() >= 6.0)
                    .collect();
                Some((r, o.label.clone(), kids))
            })
            .collect();
        Self { mode, shape, shot, tex: None, objects, start: None, cur: None, path: Vec::new(), ruler: None, chosen: None, multi: Vec::new() }
    }

    fn pixel(&self, p: Pos2) -> [u8; 3] {
        let x = (p.x.max(0.0) as u32).min(self.shot.image.width().saturating_sub(1));
        let y = (p.y.max(0.0) as u32).min(self.shot.image.height().saturating_sub(1));
        let c = self.shot.image.get_pixel(x, y).0;
        [c[0], c[1], c[2]]
    }

    /// The window (or, with `objects`, the smallest control) under `p`.
    fn object_at(&self, p: Pos2, objects: bool) -> Option<(Rect, String)> {
        let (r, label, kids) = self.objects.iter().find(|(r, _, _)| r.contains(p))?;
        if objects
            && let Some((kr, kl)) = kids
                .iter()
                .filter(|(kr, _)| kr.contains(p) && kr.area() < r.area() * 0.97)
                .min_by(|a, b| a.0.area().total_cmp(&b.0.area()))
        {
            return Some((*kr, kl.clone()));
        }
        Some((*r, label.clone()))
    }

    fn purpose(&self) -> Option<PickPurpose> {
        if let OverlayMode::Pick(p) = self.mode { Some(p) } else { None }
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
            let f = scr_rect(full);
            let h = scr_rect(hole);
            for piece in [
                Rect::from_min_max(f.min, pos2(f.max.x, h.min.y)),
                Rect::from_min_max(pos2(f.min.x, h.max.y), f.max),
                Rect::from_min_max(pos2(f.min.x, h.min.y), pos2(h.min.x, h.max.y)),
                Rect::from_min_max(pos2(h.max.x, h.min.y), pos2(f.max.x, h.max.y)),
            ] {
                if piece.is_positive() {
                    p.rect_filled(piece, CornerRadius::ZERO, dim);
                }
            }
        };

        // ---- all-in-one: an area is chosen, show what can be done with it ----
        if let Some((rect, shape, points)) = self.chosen.clone() {
            dim_except(rect);
            let outline: Vec<Pos2> = self.shape_outline(shape, rect, &points.iter().map(|q| *q + rect.min.to_vec2()).collect::<Vec<_>>());
            p.add(egui::Shape::closed_line(outline.iter().map(|q| to_scr(*q)).collect(), Stroke::new(2.0, ACCENT)));
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
                        .stroke(Stroke::new(1.0, ACCENT))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(format!("{} x {}", rect.width().round(), rect.height().round()))
                                        .color(Color32::from_gray(170)),
                                );
                                let b = |ui: &mut egui::Ui, t: &str, tip: &str| ui.button(t).on_hover_text(tip).clicked();
                                if ui
                                    .add(egui::Button::new(egui::RichText::new(crate::i18n::t("Capture")).strong().color(Color32::WHITE)).fill(ACCENT))
                                    .on_hover_text("Open it in Snagster (Enter)")
                                    .clicked()
                                {
                                    picked = Some(AreaAction::Capture);
                                }
                                if b(ui, "Copy", "Straight to the clipboard") {
                                    picked = Some(AreaAction::Copy);
                                }
                                if b(ui, "Scroll down", "Keep scrolling and stitch a long picture") {
                                    picked = Some(AreaAction::Scroll { horizontal: false });
                                }
                                if b(ui, "Scroll sideways", "A wide panoramic picture") {
                                    picked = Some(AreaAction::Scroll { horizontal: true });
                                }
                                if b(ui, "Scroll yourself", "Snagster keeps capturing while you scroll; press Stop when done") {
                                    picked = Some(AreaAction::ManualScroll);
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
                return Outcome::FullScreen;
            }
        }

        let snapping = s.detect_windows
            && matches!(self.mode, OverlayMode::Window)
            || (s.detect_windows && picking && self.shape == CutShape::Rect && self.start.is_none());
        let hovered = if snapping { hover.and_then(|h| self.object_at(h, s.detect_objects && !ctrl)) } else { None };

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
                        self.start = hover;
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
                        self.start = hover;
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
                    let size = vec2((w as f32).min(iw), (h as f32).min(ih));
                    let mut r = Rect::from_center_size(c, size);
                    r = r.translate(vec2((-r.min.x).max(0.0) - (r.max.x - iw).max(0.0), (-r.min.y).max(0.0) - (r.max.y - ih).max(0.0)));
                    dim_except(r);
                    p.rect_stroke(scr_rect(r), CornerRadius::ZERO, Stroke::new(2.0, ACCENT), StrokeKind::Outside);
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
                    self.start = hover;
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
                p.add(egui::Shape::line(pts.clone(), Stroke::new(2.0, ACCENT)));
                if let (Some(last), Some(h)) = (pts.last(), hover) {
                    p.line_segment([*last, to_scr(h)], Stroke::new(1.0, Color32::WHITE));
                }
                for q in &pts {
                    p.circle_filled(*q, 4.0, Color32::WHITE);
                }
                label = Some(format!("{} points. Click the first point, double-click or press Enter to finish.", pts.len()));
            } else {
                p.add(egui::Shape::line(pts, Stroke::new(2.0, ACCENT)));
            }
        } else if picking && self.start.is_some() {
            let a = self.start.unwrap();
            let b = self.cur.unwrap_or(a);
            let b = if shift { crate::doc::square(a, b) } else { b };
            let r = Rect::from_two_pos(a, b);
            dim_except(r);
            let outline = self.shape_outline(self.shape, r, &[]);
            p.add(egui::Shape::closed_line(outline.iter().map(|q| to_scr(*q)).collect(), Stroke::new(2.0, ACCENT)));
            if self.shape != CutShape::Rect {
                p.rect_stroke(scr_rect(r), CornerRadius::ZERO, Stroke::new(1.0, Color32::from_white_alpha(90)), StrokeKind::Middle);
            }
            label = Some(format!("{} x {}", r.width().round(), r.height().round()));
        } else if let Some((r, title)) = &hovered {
            dim_except(*r);
            p.rect_stroke(scr_rect(*r), CornerRadius::ZERO, Stroke::new(3.0, ACCENT), StrokeKind::Inside);
            label = Some(format!("{title}   {} x {}", r.width().round(), r.height().round()));
        } else if !fixed && self.mode != OverlayMode::ColorPicker {
            p.rect_filled(scr_rect(full), CornerRadius::ZERO, dim);
        }

        for (n, r) in self.multi.iter().enumerate() {
            let sr = scr_rect(*r);
            let uv = Rect::from_min_max(pos2(r.min.x / iw, r.min.y / ih), pos2(r.max.x / iw, r.max.y / ih));
            p.image(tex, sr, uv, Color32::WHITE);
            p.rect_stroke(sr, CornerRadius::ZERO, Stroke::new(2.0, ACCENT), StrokeKind::Outside);
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
                let zoom = s.magnifier_zoom.clamp(2, 32) as f32;
                let cells = (box_size / zoom).floor().max(3.0);
                let cells = if cells as i32 % 2 == 0 { cells - 1.0 } else { cells };
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
                p.rect_stroke(mrect, CornerRadius::same(2), Stroke::new(2.0, ACCENT), StrokeKind::Outside);
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

fn pill(p: &egui::Painter, at: Pos2, align: Align2, text: &str, size: f32) {
    let galley = p.layout(text.to_string(), FontId::proportional(size), Color32::WHITE, 900.0);
    let r = align.anchor_size(at, galley.size() + vec2(16.0, 8.0));
    p.rect_filled(r, CornerRadius::same(6), Color32::from_rgba_unmultiplied(20, 22, 28, 225));
    p.galley(r.min + vec2(8.0, 4.0), galley, Color32::WHITE);
}
