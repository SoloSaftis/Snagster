//! The picture being edited: a base image plus marks on top of it.
//! All coordinates are in image pixels.

use crate::settings::{CalloutShape, StepStyle};
use eframe::egui::{Color32, Pos2, Rect, Vec2, pos2, vec2};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Selection,
    Arrow,
    Line,
    Rect,
    Ellipse,
    Pen,
    Highlight,
    Text,
    Callout,
    Step,
    Stamp,
    Magnify,
    Spotlight,
    Blur,
    CutOut,
    Fill,
    Eraser,
    Crop,
}

impl Tool {
    pub const ALL: [Tool; 19] = [
        Tool::Select,
        Tool::Selection,
        Tool::Arrow,
        Tool::Line,
        Tool::Rect,
        Tool::Ellipse,
        Tool::Pen,
        Tool::Highlight,
        Tool::Text,
        Tool::Callout,
        Tool::Step,
        Tool::Stamp,
        Tool::Magnify,
        Tool::Spotlight,
        Tool::Blur,
        Tool::CutOut,
        Tool::Fill,
        Tool::Eraser,
        Tool::Crop,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Selection => "Selection",
            Tool::Arrow => "Arrow",
            Tool::Line => "Line",
            Tool::Rect => "Box",
            Tool::Ellipse => "Oval",
            Tool::Pen => "Pen",
            Tool::Highlight => "Marker",
            Tool::Text => "Text",
            Tool::Callout => "Callout",
            Tool::Step => "Step",
            Tool::Stamp => "Stamp",
            Tool::Magnify => "Magnify",
            Tool::Spotlight => "Spotlight",
            Tool::Blur => "Blur",
            Tool::CutOut => "Cut out",
            Tool::Fill => "Fill",
            Tool::Eraser => "Eraser",
            Tool::Crop => "Crop",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Tool::Select => "V",
            Tool::Selection => "Q",
            Tool::Arrow => "A",
            Tool::Line => "L",
            Tool::Rect => "R",
            Tool::Ellipse => "O",
            Tool::Pen => "P",
            Tool::Highlight => "H",
            Tool::Text => "T",
            Tool::Callout => "K",
            Tool::Step => "N",
            Tool::Stamp => "E",
            Tool::Magnify => "M",
            Tool::Spotlight => "S",
            Tool::Blur => "B",
            Tool::CutOut => "X",
            Tool::Fill => "F",
            Tool::Eraser => "D",
            Tool::Crop => "C",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Tool::Select => "Click a mark to pick it, drag to move it, Delete removes it. Double-click text to edit it.",
            Tool::Selection => "Drag around part of the picture to lift it out: move it, resize it or delete it. The hole is filled with the colors around it.",
            Tool::Arrow => "Drag from the tail to the point of the arrow. Shift snaps to 45 degrees.",
            Tool::Line => "Drag to draw a straight line. Shift snaps to 45 degrees.",
            Tool::Rect => "Drag to draw a box. Shift makes a square. Set rounded corners on the right.",
            Tool::Ellipse => "Drag to draw an oval. Shift makes a circle.",
            Tool::Pen => "Drag to draw freehand.",
            Tool::Highlight => "Drag over text to highlight it. Shift keeps it level.",
            Tool::Text => "Click where the text should go, type, then press Enter. Shift+Enter starts a new line.",
            Tool::Callout => "Press on the thing to point at, drag to where the bubble goes, then type.",
            Tool::Step => "Click to drop the next step: numbers, letters or roman numerals.",
            Tool::Stamp => "Pick an emoji, badge or pointer on the right, then click to place it.",
            Tool::Magnify => "Drag from the spot to enlarge outwards: a round magnifying glass appears.",
            Tool::Spotlight => "Drag around what matters; everything else is dimmed.",
            Tool::Blur => "Drag over anything private to pixelate it.",
            Tool::CutOut => "Drag across a wide band to remove those rows, or a tall band to remove those columns.",
            Tool::Fill => "Click an area of one color to fill it with the current color.",
            Tool::Eraser => "Drag to erase the picture underneath (it becomes see-through).",
            Tool::Crop => "Drag around the part you want to keep.",
        }
    }
}

impl Default for StepStyle {
    fn default() -> Self {
        StepStyle::Numbers
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Shape {
    Arrow {
        a: Pos2,
        b: Pos2,
        color: Color32,
        width: f32,
        shadow: bool,
        /// 0 = no heads, 1 = at the end, 2 = at both ends.
        #[serde(default = "one")]
        heads: u8,
    },
    Line { a: Pos2, b: Pos2, color: Color32, width: f32, shadow: bool },
    Rect { rect: Rect, color: Color32, width: f32, fill: bool, radius: f32, shadow: bool },
    Ellipse { rect: Rect, color: Color32, width: f32, fill: bool, shadow: bool },
    Pen { points: Vec<Pos2>, color: Color32, width: f32, shadow: bool },
    Highlight { points: Vec<Pos2>, color: Color32, width: f32 },
    Text {
        pos: Pos2,
        text: String,
        color: Color32,
        size: f32,
        bg: bool,
        shadow: bool,
        #[serde(default)]
        font: String,
        #[serde(default)]
        bold: bool,
        #[serde(default)]
        italic: bool,
        #[serde(default = "yes")]
        outline: bool,
        /// 0 left, 1 centre, 2 right.
        #[serde(default)]
        align: u8,
    },
    Callout {
        pos: Pos2,
        tail: Pos2,
        text: String,
        color: Color32,
        size: f32,
        shadow: bool,
        #[serde(default)]
        font: String,
        #[serde(default)]
        bold: bool,
        #[serde(default)]
        italic: bool,
        #[serde(default)]
        shape: CalloutShape,
        #[serde(default)]
        outline: bool,
        #[serde(default)]
        align: u8,
    },
    Step {
        center: Pos2,
        n: u32,
        color: Color32,
        radius: f32,
        shadow: bool,
        #[serde(default)]
        style: StepStyle,
    },
    Stamp { center: Pos2, emoji: String, size: f32, color: Color32 },
    Magnify { center: Pos2, radius: f32, zoom: f32 },
    Pointer { pos: Pos2, size: f32 },
    Spotlight { rect: Rect },
    /// A piece of picture that can be moved and resized (pasted, or lifted with Selection).
    Image { rect: Rect, image: ImgData },
}

fn one() -> u8 {
    1
}
fn yes() -> bool {
    true
}

/// A picture kept inside a mark; saved as PNG in project files.
#[derive(Clone, Debug)]
pub struct ImgData(pub Arc<RgbaImage>);

impl Serialize for ImgData {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use base64::Engine as _;
        let png = crate::export::png_bytes(&self.0).map_err(serde::ser::Error::custom)?;
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(png))
    }
}

impl<'de> Deserialize<'de> for ImgData {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use base64::Engine as _;
        let s = String::deserialize(d)?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(s).map_err(serde::de::Error::custom)?;
        let img = crate::export::decode_image(&bytes).map_err(serde::de::Error::custom)?.to_rgba8();
        Ok(ImgData(Arc::new(img)))
    }
}

/// Highlighter strokes are this many times wider than the chosen line width.
pub const HIGHLIGHT_WIDTH_FACTOR: f32 = 5.0;
/// Highlighter colour opacity (0-255).
pub const HIGHLIGHT_ALPHA: u8 = 110;
/// Arrow head length relative to the line width (with a minimum).
pub const ARROW_HEAD_FACTOR: f32 = 4.0;
pub const ARROW_HEAD_MIN: f32 = 14.0;
/// Step badge radius relative to the line width (with a minimum).
pub const STEP_RADIUS_FACTOR: f32 = 3.5;
pub const STEP_RADIUS_MIN: f32 = 14.0;
/// Text size relative to the line width (with a minimum).
pub const TEXT_SIZE_FACTOR: f32 = 6.0;
pub const TEXT_SIZE_MIN: f32 = 18.0;
/// Smallest drag (in image pixels) that makes a shape.
pub const MIN_DRAG: f32 = 3.0;
/// Mark shadows: offset relative to the line width (with a minimum) and opacity.
pub const SHADOW_OFFSET_FACTOR: f32 = 0.6;
pub const SHADOW_OFFSET_MIN: f32 = 3.0;
pub const SHADOW_ALPHA: u8 = 100;
/// Spotlight dims the rest of the picture to this opacity of black.
pub const SPOTLIGHT_ALPHA: u8 = 150;
/// Text background padding relative to the text size.
pub const TEXT_PAD_FACTOR: f32 = 0.3;
/// Stamps are this many times the text size.
pub const STAMP_FACTOR: f32 = 1.6;
/// Pointer marks: height relative to the line width (with a minimum).
pub const POINTER_FACTOR: f32 = 5.0;
pub const POINTER_MIN: f32 = 24.0;

pub fn arrow_head_len(width: f32) -> f32 {
    (width * ARROW_HEAD_FACTOR).max(ARROW_HEAD_MIN)
}
pub fn step_radius(width: f32) -> f32 {
    (width * STEP_RADIUS_FACTOR).max(STEP_RADIUS_MIN)
}
pub fn text_size(width: f32) -> f32 {
    (width * TEXT_SIZE_FACTOR).max(TEXT_SIZE_MIN)
}
pub fn pointer_size(width: f32) -> f32 {
    (width * POINTER_FACTOR).max(POINTER_MIN)
}
pub fn highlight_color(c: Color32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), HIGHLIGHT_ALPHA)
}
pub fn shadow_color() -> Color32 {
    Color32::from_rgba_unmultiplied(0, 0, 0, SHADOW_ALPHA)
}

/// The three corners of an arrow head pointing at `b`.
pub fn arrow_head(a: Pos2, b: Pos2, width: f32) -> [Pos2; 3] {
    let dir = (b - a).normalized();
    let dir = if dir.is_finite() { dir } else { vec2(1.0, 0.0) };
    let len = arrow_head_len(width);
    let side = vec2(-dir.y, dir.x) * len * 0.5;
    let base = b - dir * len;
    [b, base + side, base - side]
}

/// The outline of a mouse pointer of height `size` with its tip at `pos`.
pub fn pointer_outline(pos: Pos2, size: f32) -> Vec<Pos2> {
    let k = size / 20.0;
    [(0.0, 0.0), (0.0, 17.0), (4.5, 13.0), (7.5, 19.5), (10.0, 18.5), (7.0, 12.0), (12.5, 12.0)]
        .iter()
        .map(|(x, y)| pos2(pos.x + x * k, pos.y + y * k))
        .collect()
}

/// The tail of a callout: a triangle from the bubble's edge to `tail`.
pub fn callout_tail(bubble: Rect, tail: Pos2) -> [Pos2; 3] {
    let c = bubble.center();
    let d = tail - c;
    let base_w = (bubble.height().min(bubble.width()) * 0.35).max(8.0);
    // Leave from the side the tail is on.
    let (a, b) = if d.x.abs() * bubble.height() > d.y.abs() * bubble.width() {
        let x = if d.x > 0.0 { bubble.max.x - 2.0 } else { bubble.min.x + 2.0 };
        let y = c.y.clamp(bubble.min.y + base_w, bubble.max.y - base_w);
        (pos2(x, y - base_w / 2.0), pos2(x, y + base_w / 2.0))
    } else {
        let y = if d.y > 0.0 { bubble.max.y - 2.0 } else { bubble.min.y + 2.0 };
        let x = (c.x + d.x * 0.3).clamp(bubble.min.x + base_w, bubble.max.x - base_w);
        (pos2(x - base_w / 2.0, y), pos2(x + base_w / 2.0, y))
    };
    [a, b, tail]
}

fn seg_dist(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    if len2 <= f32::EPSILON {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Rough size of text in the default font, for hit testing and selection boxes.
pub fn text_extent(text: &str, size: f32) -> Vec2 {
    let lines: Vec<&str> = text.split('\n').collect();
    let widest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as f32;
    vec2(widest.max(1.0) * size * 0.52, lines.len().max(1) as f32 * size * 1.2)
}

/// The bubble of a callout (text box plus padding).
pub fn callout_bubble(pos: Pos2, text: &str, size: f32) -> Rect {
    Rect::from_min_size(pos, text_extent(text, size)).expand(size * 0.5)
}

impl Shape {
    pub fn shadow(&self) -> bool {
        match self {
            Shape::Arrow { shadow, .. }
            | Shape::Line { shadow, .. }
            | Shape::Rect { shadow, .. }
            | Shape::Ellipse { shadow, .. }
            | Shape::Pen { shadow, .. }
            | Shape::Text { shadow, .. }
            | Shape::Callout { shadow, .. }
            | Shape::Step { shadow, .. } => *shadow,
            _ => false,
        }
    }

    pub fn width(&self) -> f32 {
        match self {
            Shape::Arrow { width, .. }
            | Shape::Line { width, .. }
            | Shape::Rect { width, .. }
            | Shape::Ellipse { width, .. }
            | Shape::Pen { width, .. }
            | Shape::Highlight { width, .. } => *width,
            Shape::Text { size, .. } | Shape::Callout { size, .. } => size / TEXT_SIZE_FACTOR,
            Shape::Step { radius, .. } => radius / STEP_RADIUS_FACTOR,
            Shape::Stamp { size, .. } => size / TEXT_SIZE_FACTOR / STAMP_FACTOR,
            Shape::Pointer { size, .. } => size / POINTER_FACTOR,
            _ => 0.0,
        }
    }

    pub fn shadow_offset(&self) -> Vec2 {
        Vec2::splat((self.width() * SHADOW_OFFSET_FACTOR).max(SHADOW_OFFSET_MIN))
    }

    pub fn set_color(&mut self, c: Color32) {
        match self {
            Shape::Arrow { color, .. }
            | Shape::Line { color, .. }
            | Shape::Rect { color, .. }
            | Shape::Ellipse { color, .. }
            | Shape::Pen { color, .. }
            | Shape::Highlight { color, .. }
            | Shape::Text { color, .. }
            | Shape::Callout { color, .. }
            | Shape::Stamp { color, .. }
            | Shape::Step { color, .. } => *color = c,
            _ => {}
        }
    }

    /// Set the line width (text, steps, stamps and pointers grow with it, as when drawing).
    /// False when the mark has no width (magnifier, spotlight, picture piece).
    pub fn set_width(&mut self, w: f32) -> bool {
        let w = w.clamp(0.5, 200.0);
        match self {
            Shape::Arrow { width, .. } | Shape::Line { width, .. } | Shape::Rect { width, .. } | Shape::Ellipse { width, .. } | Shape::Pen { width, .. } | Shape::Highlight { width, .. } => *width = w,
            Shape::Text { size, .. } | Shape::Callout { size, .. } => *size = text_size(w),
            Shape::Step { radius, .. } => *radius = step_radius(w),
            Shape::Stamp { size, .. } => *size = text_size(w) * STAMP_FACTOR,
            Shape::Pointer { size, .. } => *size = pointer_size(w),
            Shape::Magnify { .. } | Shape::Spotlight { .. } | Shape::Image { .. } => return false,
        }
        true
    }

    /// Turn the shadow on or off; false when the mark has none.
    pub fn set_shadow(&mut self, on: bool) -> bool {
        match self {
            Shape::Arrow { shadow, .. }
            | Shape::Line { shadow, .. }
            | Shape::Rect { shadow, .. }
            | Shape::Ellipse { shadow, .. }
            | Shape::Pen { shadow, .. }
            | Shape::Text { shadow, .. }
            | Shape::Callout { shadow, .. }
            | Shape::Step { shadow, .. } => {
                *shadow = on;
                true
            }
            _ => false,
        }
    }

    /// Fill a box or oval, or give text a background; false for other marks.
    pub fn set_fill(&mut self, on: bool) -> bool {
        match self {
            Shape::Rect { fill, .. } | Shape::Ellipse { fill, .. } | Shape::Text { bg: fill, .. } => {
                *fill = on;
                true
            }
            _ => false,
        }
    }

    /// Change the words of a text or callout (or a stamp's emoji); false for other marks.
    pub fn set_text(&mut self, t: &str) -> bool {
        match self {
            Shape::Text { text, .. } | Shape::Callout { text, .. } | Shape::Stamp { emoji: text, .. } => {
                *text = t.to_string();
                true
            }
            _ => false,
        }
    }

    pub fn bounds(&self) -> Rect {
        match self {
            Shape::Arrow { a, b, width, .. } | Shape::Line { a, b, width, .. } => {
                Rect::from_two_pos(*a, *b).expand(arrow_head_len(*width) * 0.5)
            }
            Shape::Rect { rect, width, .. } | Shape::Ellipse { rect, width, .. } => rect.expand(width * 0.5),
            Shape::Pen { points, width, .. } => Rect::from_points(points).expand(width * 0.5),
            Shape::Highlight { points, width, .. } => {
                Rect::from_points(points).expand(width * HIGHLIGHT_WIDTH_FACTOR * 0.5)
            }
            Shape::Text { pos, text, size, bg, .. } => {
                let r = Rect::from_min_size(*pos, text_extent(text, *size));
                if *bg { r.expand(size * TEXT_PAD_FACTOR) } else { r }
            }
            Shape::Callout { pos, tail, text, size, .. } => callout_bubble(*pos, text, *size).union(Rect::from_center_size(*tail, Vec2::splat(4.0))),
            Shape::Step { center, radius, .. } => Rect::from_center_size(*center, Vec2::splat(radius * 2.0)),
            Shape::Stamp { center, size, .. } => Rect::from_center_size(*center, Vec2::splat(*size)),
            Shape::Magnify { center, radius, .. } => Rect::from_center_size(*center, Vec2::splat(radius * 2.0)),
            Shape::Pointer { pos, size } => Rect::from_points(&pointer_outline(*pos, *size)),
            Shape::Spotlight { rect } | Shape::Image { rect, .. } => *rect,
        }
    }

    pub fn translate(&mut self, d: Vec2) {
        match self {
            Shape::Arrow { a, b, .. } | Shape::Line { a, b, .. } => {
                *a += d;
                *b += d;
            }
            Shape::Rect { rect, .. } | Shape::Ellipse { rect, .. } | Shape::Spotlight { rect } | Shape::Image { rect, .. } => *rect = rect.translate(d),
            Shape::Pen { points, .. } | Shape::Highlight { points, .. } => {
                for p in points {
                    *p += d;
                }
            }
            Shape::Text { pos, .. } | Shape::Pointer { pos, .. } => *pos += d,
            Shape::Callout { pos, tail, .. } => {
                *pos += d;
                *tail += d;
            }
            Shape::Step { center, .. } | Shape::Stamp { center, .. } | Shape::Magnify { center, .. } => *center += d,
        }
    }

    /// Make the mark bigger or smaller around its middle (for the selection handles).
    pub fn scale(&mut self, f: f32) {
        let c = self.bounds().center();
        let sp = |p: &mut Pos2| *p = c + (*p - c) * f;
        match self {
            Shape::Arrow { a, b, .. } | Shape::Line { a, b, .. } => {
                sp(a);
                sp(b);
            }
            Shape::Rect { rect, .. } | Shape::Ellipse { rect, .. } | Shape::Spotlight { rect } | Shape::Image { rect, .. } => {
                *rect = Rect::from_center_size(rect.center(), rect.size() * f)
            }
            Shape::Pen { points, .. } | Shape::Highlight { points, .. } => points.iter_mut().for_each(sp),
            Shape::Text { size, .. } | Shape::Callout { size, .. } => *size = (*size * f).clamp(6.0, 600.0),
            Shape::Step { radius, .. } => *radius = (*radius * f).clamp(4.0, 400.0),
            Shape::Stamp { size, .. } => *size = (*size * f).clamp(8.0, 1000.0),
            Shape::Magnify { radius, .. } => *radius = (*radius * f).clamp(8.0, 2000.0),
            Shape::Pointer { size, .. } => *size = (*size * f).clamp(8.0, 400.0),
        }
    }

    pub fn hit(&self, p: Pos2, tol: f32) -> bool {
        match self {
            Shape::Arrow { a, b, width, .. } | Shape::Line { a, b, width, .. } => {
                seg_dist(p, *a, *b) <= tol + width * 0.5
            }
            Shape::Rect { rect, width, fill, .. } | Shape::Ellipse { rect, width, fill, .. } => {
                if *fill {
                    rect.expand(tol).contains(p)
                } else {
                    rect.expand(tol + width).contains(p) && !rect.shrink(tol + width).contains(p)
                }
            }
            Shape::Pen { points, width, .. } => {
                points.windows(2).any(|w| seg_dist(p, w[0], w[1]) <= tol + width * 0.5)
                    || points.first().is_some_and(|q| q.distance(p) <= tol + width)
            }
            Shape::Highlight { points, width, .. } => {
                let w = width * HIGHLIGHT_WIDTH_FACTOR;
                points.windows(2).any(|s| seg_dist(p, s[0], s[1]) <= tol + w * 0.5)
            }
            Shape::Step { center, radius, .. } => center.distance(p) <= radius + tol,
            Shape::Magnify { center, radius, .. } => center.distance(p) <= radius + tol,
            Shape::Spotlight { rect } => rect.expand(tol).contains(p) && !rect.shrink(tol).contains(p),
            _ => self.bounds().expand(tol).contains(p),
        }
    }
}

#[derive(Clone)]
pub struct Doc {
    pub image: Arc<RgbaImage>,
    pub shapes: Vec<Shape>,
}

impl Doc {
    pub fn new(image: RgbaImage) -> Self {
        Self { image: Arc::new(image), shapes: Vec::new() }
    }

    pub fn size(&self) -> Vec2 {
        vec2(self.image.width() as f32, self.image.height() as f32)
    }

    pub fn image_rect(&self) -> Rect {
        Rect::from_min_size(Pos2::ZERO, self.size())
    }

    /// The number the next step badge gets: one more than the highest, or `start`.
    pub fn next_step(&self, start: u32) -> u32 {
        self.shapes
            .iter()
            .filter_map(|s| if let Shape::Step { n, .. } = s { Some(*n + 1) } else { None })
            .max()
            .unwrap_or(start.max(1))
    }

    /// Keep only `r` (clamped to the picture) and move the marks with it.
    pub fn crop(&self, r: Rect) -> Option<Doc> {
        let r = r.intersect(self.image_rect());
        let x = r.min.x.round().max(0.0) as u32;
        let y = r.min.y.round().max(0.0) as u32;
        let w = (r.width().round() as u32).min(self.image.width().saturating_sub(x));
        let h = (r.height().round() as u32).min(self.image.height().saturating_sub(y));
        if w < 2 || h < 2 {
            return None;
        }
        let img = image::imageops::crop_imm(self.image.as_ref(), x, y, w, h).to_image();
        let mut shapes = self.shapes.clone();
        for s in &mut shapes {
            s.translate(vec2(-(x as f32), -(y as f32)));
        }
        Some(Doc { image: Arc::new(img), shapes })
    }

    /// Pixelate the area `r` of the base picture in `block`-pixel squares.
    pub fn blur(&self, r: Rect, block: u32) -> Option<Doc> {
        let r = r.intersect(self.image_rect());
        if r.width() < 2.0 || r.height() < 2.0 {
            return None;
        }
        let mut img = (*self.image).clone();
        pixelate(&mut img, r, block.max(2));
        Some(Doc { image: Arc::new(img), shapes: self.shapes.clone() })
    }

    /// Remove a band: rows when the band is wider than tall, columns otherwise. Marks after
    /// the band move up (or left) to close the gap.
    pub fn cut_out(&self, band: Rect) -> Option<Doc> {
        let (w, h) = self.image.dimensions();
        let rows = band.width() >= band.height();
        let (a, b) = if rows {
            (band.min.y.max(0.0) as u32, (band.max.y.ceil() as u32).min(h))
        } else {
            (band.min.x.max(0.0) as u32, (band.max.x.ceil() as u32).min(w))
        };
        if b <= a + 1 {
            return None;
        }
        let cut = b - a;
        let img = if rows {
            if cut >= h {
                return None;
            }
            let mut out = RgbaImage::new(w, h - cut);
            image::imageops::replace(&mut out, &image::imageops::crop_imm(self.image.as_ref(), 0, 0, w, a).to_image(), 0, 0);
            image::imageops::replace(&mut out, &image::imageops::crop_imm(self.image.as_ref(), 0, b, w, h - b).to_image(), 0, a as i64);
            out
        } else {
            if cut >= w {
                return None;
            }
            let mut out = RgbaImage::new(w - cut, h);
            image::imageops::replace(&mut out, &image::imageops::crop_imm(self.image.as_ref(), 0, 0, a, h).to_image(), 0, 0);
            image::imageops::replace(&mut out, &image::imageops::crop_imm(self.image.as_ref(), b, 0, w - b, h).to_image(), a as i64, 0);
            out
        };
        let mut shapes = Vec::new();
        for s in &self.shapes {
            let bb = s.bounds();
            let (lo, hi) = if rows { (bb.min.y, bb.max.y) } else { (bb.min.x, bb.max.x) };
            if lo >= a as f32 && hi <= b as f32 {
                continue; // entirely inside the removed band
            }
            let mut s = s.clone();
            if lo >= b as f32 {
                s.translate(if rows { vec2(0.0, -(cut as f32)) } else { vec2(-(cut as f32), 0.0) });
            }
            shapes.push(s);
        }
        Some(Doc { image: Arc::new(img), shapes })
    }
}

pub fn pixelate(img: &mut RgbaImage, r: Rect, block: u32) {
    let x0 = r.min.x.max(0.0) as u32;
    let y0 = r.min.y.max(0.0) as u32;
    let x1 = (r.max.x.ceil() as u32).min(img.width());
    let y1 = (r.max.y.ceil() as u32).min(img.height());
    let mut by = y0;
    while by < y1 {
        let bh = block.min(y1 - by);
        let mut bx = x0;
        while bx < x1 {
            let bw = block.min(x1 - bx);
            let mut sum = [0u64; 4];
            for y in by..by + bh {
                for x in bx..bx + bw {
                    let p = img.get_pixel(x, y).0;
                    for c in 0..4 {
                        sum[c] += p[c] as u64;
                    }
                }
            }
            let n = (bw * bh) as u64;
            let avg = image::Rgba([
                (sum[0] / n) as u8,
                (sum[1] / n) as u8,
                (sum[2] / n) as u8,
                (sum[3] / n) as u8,
            ]);
            for y in by..by + bh {
                for x in bx..bx + bw {
                    img.put_pixel(x, y, avg);
                }
            }
            bx += block;
        }
        by += block;
    }
}

/// Snap `b` so the segment a-b is at a multiple of 45 degrees.
pub fn snap45(a: Pos2, b: Pos2) -> Pos2 {
    let d = b - a;
    let angle = d.y.atan2(d.x);
    let step = std::f32::consts::FRAC_PI_4;
    let snapped = (angle / step).round() * step;
    let len = d.length();
    a + vec2(snapped.cos(), snapped.sin()) * len
}

/// Make the rectangle from `a` to `b` square (keeping the drag direction).
pub fn square(a: Pos2, b: Pos2) -> Pos2 {
    let d = b - a;
    let s = d.x.abs().max(d.y.abs());
    pos2(a.x + s * d.x.signum(), a.y + s * d.y.signum())
}

impl Doc {
    /// Hide `r` according to the blur type: pixelate, soft blur, or a solid box.
    pub fn redact(&self, r: Rect, mode: crate::settings::BlurMode, block: u32, color: Color32) -> Option<Doc> {
        use crate::settings::BlurMode;
        let r = r.intersect(self.image_rect());
        if r.width() < 2.0 || r.height() < 2.0 {
            return None;
        }
        match mode {
            BlurMode::Pixelate => self.blur(r, block),
            BlurMode::Blur | BlurMode::Solid => {
                let mut img = (*self.image).clone();
                let (x, y) = (r.min.x as u32, r.min.y as u32);
                let (w, h) = (r.width() as u32, r.height() as u32);
                let patch = if mode == BlurMode::Blur {
                    let part = image::imageops::crop_imm(&img, x, y, w, h).to_image();
                    image::imageops::blur(&part, (block as f32 * 0.6).max(2.0))
                } else {
                    RgbaImage::from_pixel(w.max(1), h.max(1), image::Rgba(color.to_srgba_unmultiplied()))
                };
                image::imageops::replace(&mut img, &patch, x as i64, y as i64);
                Some(Doc { image: Arc::new(img), shapes: self.shapes.clone() })
            }
        }
    }

    /// Fill the area of similar colour around `p` with `color` (tolerance 0-255).
    pub fn flood_fill(&self, p: Pos2, color: Color32, tolerance: u8) -> Option<Doc> {
        let (w, h) = self.image.dimensions();
        if p.x < 0.0 || p.y < 0.0 || p.x >= w as f32 || p.y >= h as f32 {
            return None;
        }
        let mut img = (*self.image).clone();
        let start = img.get_pixel(p.x as u32, p.y as u32).0;
        let target = color.to_srgba_unmultiplied();
        if start == target {
            return None;
        }
        let close = |c: [u8; 4]| (0..4).all(|i| (c[i] as i16 - start[i] as i16).unsigned_abs() <= tolerance as u16);
        let mut seen = vec![false; (w * h) as usize];
        let mut stack = vec![(p.x as u32, p.y as u32)];
        while let Some((x, y)) = stack.pop() {
            let i = (y * w + x) as usize;
            if seen[i] || !close(img.get_pixel(x, y).0) {
                continue;
            }
            seen[i] = true;
            img.put_pixel(x, y, image::Rgba(target));
            if x > 0 {
                stack.push((x - 1, y));
            }
            if y > 0 {
                stack.push((x, y - 1));
            }
            if x + 1 < w {
                stack.push((x + 1, y));
            }
            if y + 1 < h {
                stack.push((x, y + 1));
            }
        }
        Some(Doc { image: Arc::new(img), shapes: self.shapes.clone() })
    }

    /// Make the picture see-through along a stroke.
    pub fn erase(&self, points: &[Pos2], width: f32) -> Option<Doc> {
        let first = points.first()?;
        let (w, h) = self.image.dimensions();
        let mut mask = tiny_skia::Mask::new(w, h)?;
        let mut pb = tiny_skia::PathBuilder::new();
        pb.move_to(first.x, first.y);
        if points.len() == 1 {
            pb.line_to(first.x + 0.01, first.y);
        }
        for p in &points[1..] {
            pb.line_to(p.x, p.y);
        }
        let path = pb.finish()?;
        let stroke = tiny_skia::Stroke {
            width: width.max(1.0),
            line_cap: tiny_skia::LineCap::Round,
            line_join: tiny_skia::LineJoin::Round,
            ..Default::default()
        };
        let outline = path.stroke(&stroke, 1.0)?;
        mask.fill_path(&outline, tiny_skia::FillRule::Winding, true, tiny_skia::Transform::identity());
        let mut img = (*self.image).clone();
        for (px, m) in img.pixels_mut().zip(mask.data()) {
            px.0[3] = ((px.0[3] as u16 * (255 - *m as u16)) / 255) as u8;
        }
        Some(Doc { image: Arc::new(img), shapes: self.shapes.clone() })
    }

    /// Lift `r` out of the picture as a movable piece; the hole takes the average colour of
    /// its edge. Returns the new document; the piece is its last mark.
    pub fn lift(&self, r: Rect) -> Option<Doc> {
        let r = r.intersect(self.image_rect());
        let (x, y) = (r.min.x.round() as u32, r.min.y.round() as u32);
        // Rounding the corner and the size separately can reach one pixel past the edge.
        let w = (r.width().round() as u32).min(self.image.width().saturating_sub(x));
        let h = (r.height().round() as u32).min(self.image.height().saturating_sub(y));
        if w < 2 || h < 2 {
            return None;
        }
        let mut img = (*self.image).clone();
        let piece = image::imageops::crop_imm(&img, x, y, w, h).to_image();
        let mut sum = [0u64; 4];
        let mut n = 0u64;
        let (iw, ih) = img.dimensions();
        let mut add = |px: u32, py: u32| {
            if px < iw && py < ih {
                let c = img.get_pixel(px, py).0;
                for i in 0..4 {
                    sum[i] += c[i] as u64;
                }
                n += 1;
            }
        };
        for i in 0..w {
            add(x + i, y.saturating_sub(1));
            add(x + i, y + h);
        }
        for j in 0..h {
            add(x.saturating_sub(1), y + j);
            add(x + w, y + j);
        }
        let n = n.max(1);
        let fill = image::Rgba([(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8, (sum[3] / n) as u8]);
        for j in y..(y + h).min(ih) {
            for i in x..(x + w).min(iw) {
                img.put_pixel(i, j, fill);
            }
        }
        let mut shapes = self.shapes.clone();
        shapes.push(Shape::Image {
            rect: Rect::from_min_size(pos2(x as f32, y as f32), vec2(w as f32, h as f32)),
            image: ImgData(Arc::new(piece)),
        });
        Some(Doc { image: Arc::new(img), shapes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::BlurMode;

    fn solid(w: u32, h: u32, c: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba(c))
    }

    /// Each row y is filled with red value y*10, so rows can be told apart after a cut.
    fn rows_image(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |_, y| image::Rgba([(y * 10) as u8, 0, 0, 255]))
    }

    fn cols_image(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, _| image::Rgba([0, (x * 10) as u8, 0, 255]))
    }

    fn step(center: Pos2, n: u32) -> Shape {
        Shape::Step { center, n, color: Color32::RED, radius: 10.0, shadow: false, style: StepStyle::Numbers }
    }

    fn rect_shape(rect: Rect, fill: bool) -> Shape {
        Shape::Rect { rect, color: Color32::RED, width: 2.0, fill, radius: 0.0, shadow: true }
    }

    fn close(a: Pos2, b: Pos2) -> bool {
        a.distance(b) < 1e-3
    }

    #[test]
    fn marks_are_restyled_where_it_makes_sense() {
        let mut r = rect_shape(Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 10.0)), false);
        assert!(r.set_width(5.0) && r.width() == 5.0);
        assert!(r.set_width(9999.0) && r.width() == 200.0, "kept in range");
        assert!(r.set_fill(true) && matches!(r, Shape::Rect { fill: true, .. }));
        assert!(r.set_shadow(false) && !r.shadow());
        assert!(!r.set_text("x"), "a box has no text");
        let mut s = step(pos2(5.0, 5.0), 1);
        assert!(s.set_width(4.0));
        assert!(matches!(s, Shape::Step { radius, .. } if radius == step_radius(4.0)));
        assert!(!s.set_fill(true));
        let mut t = Shape::Text { pos: Pos2::ZERO, text: "a".into(), color: Color32::RED, size: 20.0, bg: false, shadow: false, font: String::new(), bold: false, italic: false, outline: true, align: 0 };
        assert!(t.set_text("hello") && t.set_fill(true) && t.set_width(3.0));
        assert!(matches!(&t, Shape::Text { text, bg: true, size, .. } if text == "hello" && *size == text_size(3.0)));
        let mut stamp = Shape::Stamp { center: Pos2::ZERO, emoji: "x".into(), size: 30.0, color: Color32::RED };
        assert!(stamp.set_text("\u{2b50}") && !stamp.set_shadow(true));
        let mut m = Shape::Magnify { center: Pos2::ZERO, radius: 40.0, zoom: 2.0 };
        assert!(!m.set_width(3.0) && !m.set_shadow(true) && !m.set_fill(true) && !m.set_text("x"));
    }

    #[test]
    fn tools_have_unique_keys_and_labels() {
        let mut keys: Vec<_> = Tool::ALL.iter().map(|t| t.key()).collect();
        let mut labels: Vec<_> = Tool::ALL.iter().map(|t| t.label()).collect();
        keys.sort();
        keys.dedup();
        labels.sort();
        labels.dedup();
        assert_eq!(keys.len(), Tool::ALL.len());
        assert_eq!(labels.len(), Tool::ALL.len());
        for t in Tool::ALL {
            assert_eq!(t.key().chars().count(), 1);
            assert!(!t.hint().is_empty());
        }
    }

    #[test]
    fn size_helpers_have_minimums() {
        assert_eq!(arrow_head_len(1.0), ARROW_HEAD_MIN);
        assert_eq!(arrow_head_len(10.0), 40.0);
        assert_eq!(step_radius(0.0), STEP_RADIUS_MIN);
        assert_eq!(step_radius(10.0), 35.0);
        assert_eq!(text_size(1.0), TEXT_SIZE_MIN);
        assert_eq!(text_size(10.0), 60.0);
        assert_eq!(pointer_size(1.0), POINTER_MIN);
        assert_eq!(pointer_size(10.0), 50.0);
        let h = highlight_color(Color32::from_rgb(10, 20, 30));
        // Color32 is stored premultiplied, so the colour comes back within rounding.
        let [r, g, b, a] = h.to_srgba_unmultiplied();
        assert_eq!(a, HIGHLIGHT_ALPHA);
        assert!([(r, 10), (g, 20), (b, 30)].iter().all(|(v, want)| v.abs_diff(*want) <= 1));
        assert_eq!(shadow_color().to_srgba_unmultiplied(), [0, 0, 0, SHADOW_ALPHA]);
    }

    #[test]
    fn arrow_head_points_at_the_end() {
        let [tip, l, r] = arrow_head(pos2(0.0, 0.0), pos2(100.0, 0.0), 10.0);
        assert!(close(tip, pos2(100.0, 0.0)));
        // The base is one head length back from the tip and symmetric around the line.
        assert!(close(l, pos2(60.0, 20.0)));
        assert!(close(r, pos2(60.0, -20.0)));
        // A zero-length arrow still gives finite points.
        let pts = arrow_head(pos2(5.0, 5.0), pos2(5.0, 5.0), 2.0);
        assert!(pts.iter().all(|p| p.x.is_finite() && p.y.is_finite()));
    }

    #[test]
    fn pointer_outline_scales_from_its_tip() {
        let pts = pointer_outline(pos2(10.0, 20.0), 40.0);
        assert_eq!(pts.len(), 7);
        assert!(close(pts[0], pos2(10.0, 20.0)));
        assert!(close(pts[1], pos2(10.0, 54.0)));
        let bb = Rect::from_points(&pts);
        assert!((bb.height() - 39.0).abs() < 1e-3);
    }

    #[test]
    fn callout_tail_leaves_from_the_nearest_side() {
        let bubble = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 40.0));
        let [a, b, t] = callout_tail(bubble, pos2(300.0, 20.0));
        assert_eq!(t, pos2(300.0, 20.0));
        assert_eq!(a.x, 98.0);
        assert_eq!(b.x, 98.0);
        let [a, b, _] = callout_tail(bubble, pos2(50.0, 200.0));
        assert_eq!(a.y, 38.0);
        assert_eq!(b.y, 38.0);
        let [a, _, _] = callout_tail(bubble, pos2(-300.0, 20.0));
        assert_eq!(a.x, 2.0);
        let [a, _, _] = callout_tail(bubble, pos2(50.0, -200.0));
        assert_eq!(a.y, 2.0);
    }

    #[test]
    fn segment_distance() {
        let (a, b) = (pos2(0.0, 0.0), pos2(10.0, 0.0));
        assert_eq!(seg_dist(pos2(5.0, 3.0), a, b), 3.0);
        assert_eq!(seg_dist(pos2(-4.0, 3.0), a, b), 5.0); // past the start
        assert_eq!(seg_dist(pos2(13.0, 4.0), a, b), 5.0); // past the end
        assert_eq!(seg_dist(pos2(3.0, 4.0), a, a), 5.0); // degenerate segment
    }

    #[test]
    fn text_extent_counts_lines_and_widest_line() {
        let one = text_extent("abcd", 10.0);
        assert!((one.x - 4.0 * 5.2).abs() < 1e-3);
        assert!((one.y - 12.0).abs() < 1e-3);
        let two = text_extent("ab\nabcdef", 10.0);
        assert!((two.x - 6.0 * 5.2).abs() < 1e-3);
        assert!((two.y - 24.0).abs() < 1e-3);
        // Empty text still has a box one letter wide.
        assert!(text_extent("", 10.0).x > 0.0);
        let b = callout_bubble(pos2(0.0, 0.0), "abcd", 10.0);
        assert_eq!(b.min, pos2(-5.0, -5.0));
    }

    #[test]
    fn shape_properties() {
        let mut s = rect_shape(Rect::from_min_max(pos2(0.0, 0.0), pos2(10.0, 10.0)), false);
        assert!(s.shadow());
        assert_eq!(s.width(), 2.0);
        assert_eq!(s.shadow_offset(), Vec2::splat(SHADOW_OFFSET_MIN));
        s.set_color(Color32::BLUE);
        assert!(matches!(s, Shape::Rect { color, .. } if color == Color32::BLUE));
        let st = step(pos2(0.0, 0.0), 1);
        assert!((st.width() - 10.0 / STEP_RADIUS_FACTOR).abs() < 1e-5);
        let sp = Shape::Spotlight { rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)) };
        assert!(!sp.shadow());
        assert_eq!(sp.width(), 0.0);
        let big = Shape::Pen { points: vec![pos2(0.0, 0.0)], color: Color32::RED, width: 20.0, shadow: true };
        assert_eq!(big.shadow_offset(), Vec2::splat(12.0));
    }

    #[test]
    fn bounds_of_each_kind() {
        let line = Shape::Line { a: pos2(0.0, 0.0), b: pos2(100.0, 50.0), color: Color32::RED, width: 10.0, shadow: false };
        assert_eq!(line.bounds(), Rect::from_min_max(pos2(-20.0, -20.0), pos2(120.0, 70.0)));
        let r = rect_shape(Rect::from_min_max(pos2(10.0, 10.0), pos2(20.0, 30.0)), false);
        assert_eq!(r.bounds(), Rect::from_min_max(pos2(9.0, 9.0), pos2(21.0, 31.0)));
        assert_eq!(step(pos2(50.0, 50.0), 1).bounds(), Rect::from_min_max(pos2(40.0, 40.0), pos2(60.0, 60.0)));
        let hl = Shape::Highlight { points: vec![pos2(0.0, 0.0), pos2(10.0, 0.0)], color: Color32::RED, width: 2.0 };
        assert_eq!(hl.bounds(), Rect::from_min_max(pos2(-5.0, -5.0), pos2(15.0, 5.0)));
        let stamp = Shape::Stamp { center: pos2(0.0, 0.0), emoji: "x".into(), size: 20.0, color: Color32::RED };
        assert_eq!(stamp.bounds(), Rect::from_min_max(pos2(-10.0, -10.0), pos2(10.0, 10.0)));
        let text = |bg| Shape::Text {
            pos: pos2(0.0, 0.0),
            text: "ab".into(),
            color: Color32::RED,
            size: 10.0,
            bg,
            shadow: false,
            font: String::new(),
            bold: false,
            italic: false,
            outline: true,
            align: 0,
        };
        assert!(text(true).bounds().contains_rect(text(false).bounds()));
        assert_eq!(text(true).bounds().min, pos2(-3.0, -3.0));
    }

    #[test]
    fn translate_moves_every_point() {
        let d = vec2(5.0, -3.0);
        let shapes = vec![
            Shape::Arrow { a: pos2(0.0, 0.0), b: pos2(10.0, 10.0), color: Color32::RED, width: 2.0, shadow: false, heads: 1 },
            rect_shape(Rect::from_min_max(pos2(0.0, 0.0), pos2(10.0, 10.0)), true),
            Shape::Pen { points: vec![pos2(0.0, 0.0), pos2(4.0, 4.0)], color: Color32::RED, width: 2.0, shadow: false },
            Shape::Callout {
                pos: pos2(0.0, 0.0),
                tail: pos2(40.0, 40.0),
                text: "hi".into(),
                color: Color32::RED,
                size: 12.0,
                shadow: false,
                font: String::new(),
                bold: false,
                italic: false,
                shape: CalloutShape::Rounded,
                outline: false,
                align: 0,
            },
            step(pos2(3.0, 3.0), 1),
            Shape::Magnify { center: pos2(3.0, 3.0), radius: 5.0, zoom: 2.0 },
            Shape::Pointer { pos: pos2(1.0, 1.0), size: 24.0 },
            Shape::Spotlight { rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(10.0, 10.0)) },
        ];
        for s in shapes {
            let before = s.bounds();
            let mut m = s.clone();
            m.translate(d);
            let after = m.bounds();
            assert!(close(after.min, before.min + d), "{s:?}");
            assert!(close(after.max, before.max + d), "{s:?}");
        }
    }

    #[test]
    fn scale_keeps_the_centre_and_clamps_sizes() {
        let mut r = rect_shape(Rect::from_min_max(pos2(0.0, 0.0), pos2(10.0, 20.0)), true);
        r.scale(2.0);
        let Shape::Rect { rect, .. } = r else { unreachable!() };
        assert_eq!(rect, Rect::from_min_max(pos2(-5.0, -10.0), pos2(15.0, 30.0)));
        let mut l = Shape::Line { a: pos2(0.0, 0.0), b: pos2(10.0, 0.0), color: Color32::RED, width: 1.0, shadow: false };
        l.scale(0.5);
        let Shape::Line { a, b, .. } = l else { unreachable!() };
        assert!(close(a, pos2(2.5, 0.0)) && close(b, pos2(7.5, 0.0)));
        let mut s = step(pos2(0.0, 0.0), 1);
        s.scale(0.01);
        assert!(matches!(s, Shape::Step { radius, .. } if radius == 4.0));
        let mut s = step(pos2(0.0, 0.0), 1);
        s.scale(1000.0);
        assert!(matches!(s, Shape::Step { radius, .. } if radius == 400.0));
    }

    #[test]
    fn hit_testing() {
        let line = Shape::Line { a: pos2(0.0, 0.0), b: pos2(100.0, 0.0), color: Color32::RED, width: 4.0, shadow: false };
        assert!(line.hit(pos2(50.0, 5.0), 3.0));
        assert!(!line.hit(pos2(50.0, 6.0), 3.0));
        let r = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0));
        // An outline is hit near its edge only; a filled box anywhere inside.
        assert!(rect_shape(r, false).hit(pos2(1.0, 50.0), 2.0));
        assert!(!rect_shape(r, false).hit(pos2(50.0, 50.0), 2.0));
        assert!(rect_shape(r, true).hit(pos2(50.0, 50.0), 2.0));
        assert!(!rect_shape(r, true).hit(pos2(150.0, 50.0), 2.0));
        let sp = Shape::Spotlight { rect: r };
        assert!(sp.hit(pos2(0.0, 50.0), 2.0));
        assert!(!sp.hit(pos2(50.0, 50.0), 2.0));
        assert!(step(pos2(0.0, 0.0), 1).hit(pos2(11.0, 0.0), 2.0));
        assert!(!step(pos2(0.0, 0.0), 1).hit(pos2(13.0, 0.0), 2.0));
        let pen = Shape::Pen { points: vec![pos2(0.0, 0.0)], color: Color32::RED, width: 2.0, shadow: false };
        assert!(pen.hit(pos2(1.0, 1.0), 1.0));
        let pen2 = Shape::Pen { points: vec![pos2(0.0, 0.0), pos2(0.0, 50.0)], color: Color32::RED, width: 2.0, shadow: false };
        assert!(pen2.hit(pos2(1.5, 25.0), 1.0));
        assert!(!pen2.hit(pos2(5.0, 25.0), 1.0));
        let hl = Shape::Highlight { points: vec![pos2(0.0, 0.0), pos2(50.0, 0.0)], color: Color32::RED, width: 2.0 };
        assert!(hl.hit(pos2(25.0, 5.0), 0.0));
        assert!(!hl.hit(pos2(25.0, 6.0), 0.0));
        let stamp = Shape::Stamp { center: pos2(0.0, 0.0), emoji: "x".into(), size: 20.0, color: Color32::RED };
        assert!(stamp.hit(pos2(9.0, 9.0), 0.0));
        assert!(!stamp.hit(pos2(12.0, 0.0), 1.0));
    }

    #[test]
    fn shapes_round_trip_through_json() {
        let piece = RgbaImage::from_fn(3, 2, |x, y| image::Rgba([x as u8 * 50, y as u8 * 100, 7, 200]));
        let shapes = vec![
            Shape::Arrow { a: pos2(1.0, 2.0), b: pos2(3.0, 4.0), color: Color32::RED, width: 2.0, shadow: true, heads: 2 },
            rect_shape(Rect::from_min_max(pos2(0.0, 0.0), pos2(5.0, 5.0)), true),
            step(pos2(9.0, 9.0), 7),
            Shape::Image { rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(3.0, 2.0)), image: ImgData(Arc::new(piece.clone())) },
        ];
        let json = serde_json::to_string(&shapes).unwrap();
        let back: Vec<Shape> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), shapes.len());
        assert!(matches!(back[0], Shape::Arrow { heads: 2, shadow: true, .. }));
        assert!(matches!(back[2], Shape::Step { n: 7, .. }));
        let Shape::Image { image, rect } = &back[3] else { panic!("image mark lost") };
        assert_eq!(*rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(3.0, 2.0)));
        assert_eq!(*image.0, piece, "the picture inside a mark must survive exactly (PNG is lossless)");
        // Re-serialising gives the same text.
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }

    #[test]
    fn older_project_fields_get_defaults() {
        let arrow = Shape::Arrow { a: pos2(0.0, 0.0), b: pos2(1.0, 1.0), color: Color32::RED, width: 2.0, shadow: false, heads: 2 };
        let mut v = serde_json::to_value(&arrow).unwrap();
        v["Arrow"].as_object_mut().unwrap().remove("heads");
        let back: Shape = serde_json::from_value(v).unwrap();
        assert!(matches!(back, Shape::Arrow { heads: 1, .. }), "old arrows have one head");

        let text = Shape::Text {
            pos: pos2(0.0, 0.0),
            text: "x".into(),
            color: Color32::RED,
            size: 10.0,
            bg: false,
            shadow: false,
            font: "Arial".into(),
            bold: true,
            italic: true,
            outline: false,
            align: 2,
        };
        let mut v = serde_json::to_value(&text).unwrap();
        let o = v["Text"].as_object_mut().unwrap();
        for k in ["font", "bold", "italic", "outline", "align"] {
            o.remove(k);
        }
        let back: Shape = serde_json::from_value(v).unwrap();
        let Shape::Text { font, bold, italic, outline, align, .. } = back else { panic!() };
        assert_eq!((font.as_str(), bold, italic, outline, align), ("", false, false, true, 0));

        let mut v = serde_json::to_value(step(pos2(0.0, 0.0), 3)).unwrap();
        v["Step"].as_object_mut().unwrap().remove("style");
        let back: Shape = serde_json::from_value(v).unwrap();
        assert!(matches!(back, Shape::Step { style: StepStyle::Numbers, .. }));
    }

    #[test]
    fn bad_image_data_is_an_error() {
        assert!(serde_json::from_str::<ImgData>("\"not base64!!\"").is_err());
        assert!(serde_json::from_str::<ImgData>("\"aGVsbG8=\"").is_err()); // base64 of "hello", not a picture
    }

    #[test]
    fn doc_size_and_next_step() {
        let mut d = Doc::new(solid(30, 20, [0, 0, 0, 255]));
        assert_eq!(d.size(), vec2(30.0, 20.0));
        assert_eq!(d.image_rect(), Rect::from_min_max(pos2(0.0, 0.0), pos2(30.0, 20.0)));
        assert_eq!(d.next_step(1), 1);
        assert_eq!(d.next_step(0), 1);
        assert_eq!(d.next_step(5), 5);
        d.shapes.push(step(pos2(0.0, 0.0), 3));
        d.shapes.push(step(pos2(0.0, 0.0), 9));
        d.shapes.push(rect_shape(Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), true));
        assert_eq!(d.next_step(1), 10);
    }

    #[test]
    fn crop_keeps_the_area_and_moves_marks() {
        let img = RgbaImage::from_fn(10, 10, |x, y| image::Rgba([x as u8, y as u8, 0, 255]));
        let mut d = Doc::new(img);
        d.shapes.push(step(pos2(5.0, 5.0), 1));
        let c = d.crop(Rect::from_min_max(pos2(2.0, 3.0), pos2(7.0, 9.0))).unwrap();
        assert_eq!(c.image.dimensions(), (5, 6));
        assert_eq!(c.image.get_pixel(0, 0).0, [2, 3, 0, 255]);
        assert!(matches!(c.shapes[0], Shape::Step { center, .. } if center == pos2(3.0, 2.0)));
        // Clamped to the picture.
        let c = d.crop(Rect::from_min_max(pos2(-5.0, -5.0), pos2(50.0, 4.0))).unwrap();
        assert_eq!(c.image.dimensions(), (10, 4));
        // Too small or outside.
        assert!(d.crop(Rect::from_min_max(pos2(2.0, 2.0), pos2(3.0, 9.0))).is_none());
        assert!(d.crop(Rect::from_min_max(pos2(20.0, 20.0), pos2(30.0, 30.0))).is_none());
    }

    #[test]
    fn pixelate_averages_each_block() {
        let mut img = RgbaImage::from_fn(4, 2, |x, _| if x < 2 { image::Rgba([0, 0, 0, 255]) } else { image::Rgba([200, 100, 50, 255]) });
        pixelate(&mut img, Rect::from_min_max(pos2(0.0, 0.0), pos2(4.0, 2.0)), 4);
        for p in img.pixels() {
            assert_eq!(p.0, [100, 50, 25, 255]);
        }
        // Only inside the rectangle.
        let mut img = RgbaImage::from_fn(4, 4, |x, y| image::Rgba([(x * 60) as u8, (y * 60) as u8, 0, 255]));
        let orig = img.clone();
        pixelate(&mut img, Rect::from_min_max(pos2(0.0, 0.0), pos2(2.0, 2.0)), 2);
        assert_eq!(img.get_pixel(0, 0).0, [30, 30, 0, 255]);
        assert_eq!(img.get_pixel(3, 3), orig.get_pixel(3, 3));
        assert_eq!(img.get_pixel(2, 0), orig.get_pixel(2, 0));
    }

    #[test]
    fn blur_and_redact() {
        let img = RgbaImage::from_fn(20, 20, |x, y| image::Rgba([((x + y) * 6) as u8, 0, 0, 255]));
        let d = Doc::new(img.clone());
        let area = Rect::from_min_max(pos2(4.0, 4.0), pos2(12.0, 12.0));
        assert!(d.blur(Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 10.0)), 4).is_none());
        let b = d.blur(area, 4).unwrap();
        assert_eq!(b.image.get_pixel(0, 0), img.get_pixel(0, 0));
        assert_ne!(b.image.get_pixel(4, 4), img.get_pixel(4, 4));

        let solid_box = d.redact(area, BlurMode::Solid, 4, Color32::from_rgb(1, 2, 3)).unwrap();
        for y in 4..12 {
            for x in 4..12 {
                assert_eq!(solid_box.image.get_pixel(x, y).0, [1, 2, 3, 255]);
            }
        }
        assert_eq!(solid_box.image.get_pixel(12, 12), img.get_pixel(12, 12));
        assert_eq!(solid_box.image.get_pixel(3, 3), img.get_pixel(3, 3));

        let soft = d.redact(area, BlurMode::Blur, 6, Color32::BLACK).unwrap();
        assert_eq!(soft.image.dimensions(), (20, 20));
        assert_eq!(soft.image.get_pixel(0, 0), img.get_pixel(0, 0));
        assert_ne!(soft.image.get_pixel(4, 4), img.get_pixel(4, 4));

        let px = d.redact(area, BlurMode::Pixelate, 4, Color32::BLACK).unwrap();
        assert_eq!(*px.image, *b.image);
        assert!(d.redact(Rect::from_min_max(pos2(30.0, 30.0), pos2(40.0, 40.0)), BlurMode::Solid, 4, Color32::BLACK).is_none());
    }

    #[test]
    fn cut_out_rows_closes_the_gap() {
        let mut d = Doc::new(rows_image(10, 10));
        let above = step(pos2(5.0, 1.0), 1); // bounds y -9..11: overlaps the band, stays put
        let inside = Shape::Spotlight { rect: Rect::from_min_max(pos2(0.0, 3.0), pos2(5.0, 4.0)) };
        let below = Shape::Spotlight { rect: Rect::from_min_max(pos2(0.0, 6.0), pos2(5.0, 8.0)) };
        d.shapes = vec![above, inside, below];
        let c = d.cut_out(Rect::from_min_max(pos2(0.0, 2.0), pos2(10.0, 5.0))).unwrap();
        assert_eq!(c.image.dimensions(), (10, 7));
        let reds: Vec<u8> = (0..7).map(|y| c.image.get_pixel(0, y).0[0]).collect();
        assert_eq!(reds, vec![0, 10, 50, 60, 70, 80, 90]);
        assert_eq!(c.shapes.len(), 2, "the mark inside the band goes");
        assert!(matches!(c.shapes[0], Shape::Step { center, .. } if center == pos2(5.0, 1.0)));
        assert!(matches!(c.shapes[1], Shape::Spotlight { rect } if rect.min.y == 3.0 && rect.max.y == 5.0));
    }

    #[test]
    fn cut_out_columns() {
        let d = Doc::new(cols_image(10, 10));
        let c = d.cut_out(Rect::from_min_max(pos2(3.0, 0.0), pos2(6.0, 10.0))).unwrap();
        assert_eq!(c.image.dimensions(), (7, 10));
        let greens: Vec<u8> = (0..7).map(|x| c.image.get_pixel(x, 0).0[1]).collect();
        assert_eq!(greens, vec![0, 10, 20, 60, 70, 80, 90]);
        // Too thin, or the whole picture.
        assert!(d.cut_out(Rect::from_min_max(pos2(3.0, 0.0), pos2(3.5, 10.0))).is_none());
        assert!(d.cut_out(Rect::from_min_max(pos2(0.0, -1.0), pos2(10.0, 20.0))).is_none());
        assert!(d.cut_out(Rect::from_min_max(pos2(-5.0, -2.0), pos2(20.0, 15.0))).is_none());
    }

    #[test]
    fn snap_and_square() {
        assert!(close(snap45(pos2(0.0, 0.0), pos2(10.0, 1.0)), pos2(10.0f32.hypot(1.0), 0.0)));
        let p = snap45(pos2(0.0, 0.0), pos2(10.0, 9.0));
        assert!((p.x - p.y).abs() < 1e-3 && p.x > 0.0);
        let p = snap45(pos2(5.0, 5.0), pos2(5.5, -10.0));
        assert!((p.x - 5.0).abs() < 1e-3 && p.y < 5.0);
        assert_eq!(square(pos2(0.0, 0.0), pos2(10.0, 4.0)), pos2(10.0, 10.0));
        assert_eq!(square(pos2(0.0, 0.0), pos2(-3.0, 8.0)), pos2(-8.0, 8.0));
        assert_eq!(square(pos2(10.0, 10.0), pos2(4.0, 2.0)), pos2(2.0, 2.0));
    }

    #[test]
    fn flood_fill_stops_at_other_colours() {
        // White on both sides of a black wall at x=5.
        let img = RgbaImage::from_fn(10, 6, |x, _| if x == 5 { image::Rgba([0, 0, 0, 255]) } else { image::Rgba([255, 255, 255, 255]) });
        let d = Doc::new(img);
        let f = d.flood_fill(pos2(1.0, 1.0), Color32::RED, 10).unwrap();
        assert_eq!(f.image.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(f.image.get_pixel(4, 5).0, [255, 0, 0, 255]);
        assert_eq!(f.image.get_pixel(5, 0).0, [0, 0, 0, 255]);
        assert_eq!(f.image.get_pixel(9, 0).0, [255, 255, 255, 255]);
        // Already that colour, or outside the picture.
        assert!(d.flood_fill(pos2(1.0, 1.0), Color32::WHITE, 10).is_none());
        assert!(d.flood_fill(pos2(-1.0, 1.0), Color32::RED, 10).is_none());
        assert!(d.flood_fill(pos2(10.0, 1.0), Color32::RED, 10).is_none());
        // A high tolerance crosses the wall.
        let all = d.flood_fill(pos2(1.0, 1.0), Color32::RED, 255).unwrap();
        assert!(all.image.pixels().all(|p| p.0 == [255, 0, 0, 255]));
    }

    #[test]
    fn erase_makes_the_stroke_see_through() {
        let d = Doc::new(solid(20, 20, [10, 20, 30, 255]));
        let e = d.erase(&[pos2(2.0, 10.0), pos2(18.0, 10.0)], 4.0).unwrap();
        assert_eq!(e.image.get_pixel(10, 10).0[3], 0);
        assert_eq!(e.image.get_pixel(10, 2).0, [10, 20, 30, 255]);
        // A single point still erases a dot.
        let dot = d.erase(&[pos2(10.0, 10.0)], 6.0).unwrap();
        assert!(dot.image.get_pixel(10, 10).0[3] < 255);
        assert!(d.erase(&[], 4.0).is_none());
    }

    #[test]
    fn lift_takes_a_piece_and_fills_the_hole() {
        // White picture with a red square in the middle.
        let img = RgbaImage::from_fn(10, 10, |x, y| {
            if (3..6).contains(&x) && (3..6).contains(&y) { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([255, 255, 255, 255]) }
        });
        let d = Doc::new(img);
        let l = d.lift(Rect::from_min_max(pos2(3.0, 3.0), pos2(6.0, 6.0))).unwrap();
        assert_eq!(l.image.get_pixel(4, 4).0, [255, 255, 255, 255], "the hole takes the edge colour");
        let Some(Shape::Image { rect, image }) = l.shapes.last() else { panic!("no piece") };
        assert_eq!(*rect, Rect::from_min_max(pos2(3.0, 3.0), pos2(6.0, 6.0)));
        assert_eq!(image.0.dimensions(), (3, 3));
        assert!(image.0.pixels().all(|p| p.0 == [255, 0, 0, 255]));
        assert!(d.lift(Rect::from_min_max(pos2(3.0, 3.0), pos2(4.0, 6.0))).is_none());
    }

    #[test]
    fn lift_near_the_edge_stays_inside_the_picture() {
        // Rounding 0.5 up for the corner and 9.5 up for the size used to reach 11 pixels on a
        // 10-pixel picture: the piece (clipped to 9) was then stretched over 10.
        let d = Doc::new(solid(10, 10, [1, 2, 3, 255]));
        let l = d.lift(Rect::from_min_max(pos2(0.5, 0.5), pos2(10.0, 10.0))).unwrap();
        let Some(Shape::Image { rect, image }) = l.shapes.last() else { panic!("no piece") };
        assert!(rect.max.x <= 10.0 && rect.max.y <= 10.0, "{rect:?}");
        assert_eq!((image.0.width() as f32, image.0.height() as f32), (rect.width(), rect.height()));
    }
}
