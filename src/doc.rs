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
        let img = image::load_from_memory(&bytes).map_err(serde::de::Error::custom)?.to_rgba8();
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
        let (w, h) = (r.width().round() as u32, r.height().round() as u32);
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
