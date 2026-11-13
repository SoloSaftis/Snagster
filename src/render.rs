//! Flatten a document (picture + marks) into one image for saving and copying.

use crate::doc::{self, Doc, Shape};
use ab_glyph::{Font, FontArc, PxScale, ScaleFont, point};
use eframe::egui::{Color32, Pos2, Rect, Vec2, pos2};
use image::RgbaImage;
use std::sync::OnceLock;
use tiny_skia as ts;

fn default_font() -> &'static FontArc {
    static F: OnceLock<FontArc> = OnceLock::new();
    F.get_or_init(|| FontArc::try_from_slice(epaint_default_fonts::UBUNTU_LIGHT).expect("built-in font"))
}

fn emoji_font() -> &'static FontArc {
    static F: OnceLock<FontArc> = OnceLock::new();
    F.get_or_init(|| FontArc::try_from_slice(epaint_default_fonts::NOTO_EMOJI_REGULAR).expect("built-in emoji font"))
}

/// The chosen font, then the built-in one, then the emoji one: the first that has the letter.
struct Fonts {
    list: Vec<FontArc>,
}

impl Fonts {
    fn new(family: &str, bold: bool, italic: bool) -> Self {
        let mut list = Vec::new();
        if let Some(f) = crate::fonts::export_font(family, bold, italic) {
            list.push(f);
        }
        list.push(default_font().clone());
        list.push(emoji_font().clone());
        Self { list }
    }

    fn pick(&self, c: char) -> &FontArc {
        self.list.iter().find(|f| f.glyph_id(c).0 != 0).unwrap_or(&self.list[0])
    }

    fn advance(&self, c: char, size: f32) -> f32 {
        let f = self.pick(c);
        f.as_scaled(PxScale::from(size)).h_advance(f.glyph_id(c))
    }

    fn line_width(&self, line: &str, size: f32) -> f32 {
        line.chars().map(|c| self.advance(c, size)).sum()
    }

    fn line_height(&self, size: f32) -> f32 {
        let sf = self.list[0].as_scaled(PxScale::from(size));
        sf.height() + sf.line_gap()
    }

    fn ascent(&self, size: f32) -> f32 {
        self.list[0].as_scaled(PxScale::from(size)).ascent()
    }

    fn measure(&self, text: &str, size: f32) -> Vec2 {
        let w = text.split('\n').map(|l| self.line_width(l, size)).fold(0.0, f32::max);
        Vec2::new(w, self.line_height(size) * text.split('\n').count().max(1) as f32)
    }
}

fn paint(c: Color32) -> ts::Paint<'static> {
    let mut p = ts::Paint::default();
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    p.set_color_rgba8(r, g, b, a);
    p.anti_alias = true;
    p
}

fn stroke(width: f32) -> ts::Stroke {
    ts::Stroke {
        width: width.max(0.5),
        line_cap: ts::LineCap::Round,
        line_join: ts::LineJoin::Round,
        ..Default::default()
    }
}

fn polyline(points: &[Pos2]) -> Option<ts::Path> {
    let mut pb = ts::PathBuilder::new();
    let first = points.first()?;
    pb.move_to(first.x, first.y);
    if points.len() == 1 {
        pb.line_to(first.x + 0.01, first.y);
    }
    for p in &points[1..] {
        pb.line_to(p.x, p.y);
    }
    pb.finish()
}

fn polygon(points: &[Pos2]) -> Option<ts::Path> {
    let mut pb = ts::PathBuilder::new();
    let first = points.first()?;
    pb.move_to(first.x, first.y);
    for p in &points[1..] {
        pb.line_to(p.x, p.y);
    }
    pb.close();
    pb.finish()
}

/// A rectangle path with rounded corners (`radius` 0 gives square corners).
pub fn rounded_rect(r: Rect, radius: f32) -> Option<ts::Path> {
    let radius = radius.min(r.width() / 2.0).min(r.height() / 2.0).max(0.0);
    if radius <= 0.0 {
        return ts::Rect::from_ltrb(r.min.x, r.min.y, r.max.x, r.max.y).map(ts::PathBuilder::from_rect);
    }
    let (l, t, rr, b) = (r.min.x, r.min.y, r.max.x, r.max.y);
    let k = radius * 0.4477;
    let mut pb = ts::PathBuilder::new();
    pb.move_to(l + radius, t);
    pb.line_to(rr - radius, t);
    pb.cubic_to(rr - k, t, rr, t + k, rr, t + radius);
    pb.line_to(rr, b - radius);
    pb.cubic_to(rr, b - k, rr - k, b, rr - radius, b);
    pb.line_to(l + radius, b);
    pb.cubic_to(l + k, b, l, b - k, l, b - radius);
    pb.line_to(l, t + radius);
    pb.cubic_to(l, t + k, l + k, t, l + radius, t);
    pb.close();
    pb.finish()
}

const ID: ts::Transform = ts::Transform { sx: 1.0, kx: 0.0, ky: 0.0, sy: 1.0, tx: 0.0, ty: 0.0 };

/// Contrasting colour so text stays readable on any background.
pub fn halo_color(c: Color32) -> Color32 {
    let lum = 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
    if lum > 150.0 { Color32::from_rgb(20, 20, 24) } else { Color32::WHITE }
}

/// Width of text in the built-in font.
pub fn text_width(text: &str, size: f32) -> f32 {
    Fonts::new("", false, false).measure(text, size).x
}

pub fn text_height(text: &str, size: f32) -> f32 {
    Fonts::new("", false, false).measure(text, size).y
}

/// Size of text in a chosen font.
pub fn measure(text: &str, size: f32, font: &str, bold: bool, italic: bool) -> Vec2 {
    Fonts::new(font, bold, italic).measure(text, size)
}

/// Draw `text` with its top-left at `pos` (or centred on `pos` when `center`) in the built-in font.
pub fn draw_text(pm: &mut ts::Pixmap, text: &str, pos: Pos2, size: f32, color: Color32, halo: Option<Color32>, center: bool) {
    draw_text_in(pm, &Fonts::new("", false, false), text, pos, size, color, halo, center);
}

#[allow(clippy::too_many_arguments)]
fn draw_text_in(pm: &mut ts::Pixmap, fonts: &Fonts, text: &str, pos: Pos2, size: f32, color: Color32, halo: Option<Color32>, center: bool) {
    draw_text_aligned(pm, fonts, text, pos, size, color, halo, center, 0);
}

/// Like `draw_text_in`, with the lines lined up left (0), centred (1) or right (2).
#[allow(clippy::too_many_arguments)]
fn draw_text_aligned(
    pm: &mut ts::Pixmap,
    fonts: &Fonts,
    text: &str,
    pos: Pos2,
    size: f32,
    color: Color32,
    halo: Option<Color32>,
    center: bool,
    align: u8,
) {
    let scale = PxScale::from(size);
    let line_h = fonts.line_height(size);
    let lines: Vec<&str> = text.split('\n').collect();
    let widths: Vec<f32> = lines.iter().map(|l| fonts.line_width(l, size)).collect();
    let block_w = widths.iter().copied().fold(0.0, f32::max);
    let shift = |lw: f32| match align {
        1 => (block_w - lw) / 2.0,
        2 => block_w - lw,
        _ => 0.0,
    };
    let total_h = line_h * lines.len() as f32;
    let top = if center { pos.y - total_h / 2.0 } else { pos.y };
    let left = if center { pos.x - block_w / 2.0 } else { pos.x };
    // Work only inside the text's own box (plus room for the outline and overhangs),
    // not the whole picture: much faster on big screenshots.
    let r = if halo.is_some() { (size / 14.0).max(1.0) } else { 0.0 };
    let margin = (r.ceil() + size * 0.5 + 2.0) as i32;
    let x0 = (left.floor() as i32 - margin).max(0);
    let y0 = (top.floor() as i32 - margin).max(0);
    let x1 = ((left + block_w).ceil() as i32 + margin).min(pm.width() as i32);
    let y1 = ((top + total_h).ceil() as i32 + margin).min(pm.height() as i32);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
    let mut data = vec![0u8; (w * h) as usize];
    for (i, line) in lines.iter().enumerate() {
        let mut x = if center { pos.x - widths[i] / 2.0 } else { pos.x + shift(widths[i]) } - x0 as f32;
        let baseline = top + i as f32 * line_h + fonts.ascent(size) - y0 as f32;
        for ch in line.chars() {
            let font = fonts.pick(ch);
            let gid = font.glyph_id(ch);
            let glyph = gid.with_scale_and_position(scale, point(x, baseline));
            x += font.as_scaled(scale).h_advance(gid);
            if let Some(og) = font.outline_glyph(glyph) {
                let bb = og.px_bounds();
                og.draw(|gx, gy, cov| {
                    let px = bb.min.x as i32 + gx as i32;
                    let py = bb.min.y as i32 + gy as i32;
                    if px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < h {
                        let idx = py as usize * w as usize + px as usize;
                        let v = (cov.clamp(0.0, 1.0) * 255.0) as u8;
                        data[idx] = data[idx].max(v);
                    }
                });
            }
        }
    }
    let Some(size_i) = ts::IntSize::from_wh(w, h) else { return };
    let Some(mut sub) = ts::Pixmap::new(w, h) else { return };
    let full = ts::Rect::from_xywh(0.0, 0.0, w as f32, h as f32).unwrap();
    if let Some(halo) = halo {
        // Grow the letters by `r` pixels: a max filter over a disc, done as one pass along
        // rows per disc row (each disc row is a horizontal run).
        let ri = r.ceil() as i32;
        let mut halo_data = vec![0u8; data.len()];
        let (wi, hi) = (w as i32, h as i32);
        for dy in -ri..=ri {
            let half = ((r * r + 0.5 - (dy * dy) as f32).max(0.0)).sqrt().floor() as i32;
            for y in 0..hi {
                let sy = y - dy;
                if sy < 0 || sy >= hi {
                    continue;
                }
                let src = &data[(sy * wi) as usize..((sy + 1) * wi) as usize];
                let dst = &mut halo_data[(y * wi) as usize..((y + 1) * wi) as usize];
                for (x, &v) in src.iter().enumerate() {
                    if v == 0 {
                        continue;
                    }
                    let a = (x as i32 - half).max(0) as usize;
                    let b = (x as i32 + half).min(wi - 1) as usize;
                    for d in &mut dst[a..=b] {
                        if *d < v {
                            *d = v;
                        }
                    }
                }
            }
        }
        if let Some(m) = ts::Mask::from_vec(halo_data, size_i) {
            sub.fill_rect(full, &paint(halo), ID, Some(&m));
        }
    }
    if let Some(m) = ts::Mask::from_vec(data, size_i) {
        sub.fill_rect(full, &paint(color), ID, Some(&m));
    }
    pm.draw_pixmap(x0, y0, sub.as_ref(), &ts::PixmapPaint::default(), ID, None);
}

/// The box behind text with a background.
pub fn text_box(pos: Pos2, text: &str, size: f32, font: &str, bold: bool, italic: bool) -> Rect {
    let pad = size * doc::TEXT_PAD_FACTOR;
    Rect::from_min_size(pos, measure(text, size, font, bold, italic)).expand(pad)
}

/// The bubble of a callout, measured in its own font.
pub fn callout_bubble(pos: Pos2, text: &str, size: f32, font: &str, bold: bool, italic: bool) -> Rect {
    let t = if text.is_empty() { " " } else { text };
    Rect::from_min_size(pos, measure(t, size, font, bold, italic)).expand(size * 0.5)
}

fn draw_image(pm: &mut ts::Pixmap, img: &RgbaImage, at: Pos2) {
    if let Some(src) = to_pixmap(img) {
        pm.draw_pixmap(at.x.round() as i32, at.y.round() as i32, src.as_ref(), &ts::PixmapPaint::default(), ID, None);
    }
}

fn magnify(pm: &mut ts::Pixmap, base: &RgbaImage, center: Pos2, radius: f32, zoom: f32) {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let (bw, bh) = (base.width() as f32, base.height() as f32);
    let zoom = zoom.max(1.0);
    let r2 = radius * radius;
    let x0 = (center.x - radius).floor().max(0.0) as i32;
    let y0 = (center.y - radius).floor().max(0.0) as i32;
    let x1 = ((center.x + radius).ceil() as i32).min(w - 1);
    let y1 = ((center.y + radius).ceil() as i32).min(h - 1);
    let pixels = pm.pixels_mut();
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (dx, dy) = (x as f32 + 0.5 - center.x, y as f32 + 0.5 - center.y);
            let d2 = dx * dx + dy * dy;
            if d2 > r2 {
                continue;
            }
            let sx = (center.x + dx / zoom).clamp(0.0, bw - 1.0);
            let sy = (center.y + dy / zoom).clamp(0.0, bh - 1.0);
            let p = base.get_pixel(sx as u32, sy as u32).0;
            let edge = (radius - d2.sqrt()).clamp(0.0, 1.0);
            let a = (p[3] as f32 * edge) as u8;
            let pm_c = |c: u8| ((c as u16 * a as u16) / 255) as u8;
            let old = pixels[(y * w + x) as usize];
            let inv = 255 - a as u16;
            let mix = |n: u8, o: u8| (n as u16 + (o as u16 * inv) / 255).min(255) as u8;
            let out = ts::PremultipliedColorU8::from_rgba(
                mix(pm_c(p[0]), old.red()),
                mix(pm_c(p[1]), old.green()),
                mix(pm_c(p[2]), old.blue()),
                mix(a, old.alpha()),
            );
            if let Some(c) = out {
                pixels[(y * w + x) as usize] = c;
            }
        }
    }
    if let Some(circle) = ts::PathBuilder::from_circle(center.x, center.y, radius) {
        let ring = (radius / 30.0).max(2.0);
        pm.stroke_path(&circle, &paint(Color32::from_rgb(40, 42, 50)), &stroke(ring + 2.0), ID, None);
        pm.stroke_path(&circle, &paint(Color32::WHITE), &stroke(ring), ID, None);
    }
}

fn draw_shape(pm: &mut ts::Pixmap, base: &RgbaImage, s: &Shape, as_shadow: bool) {
    let col = |c: Color32| if as_shadow { doc::shadow_color() } else { c };
    match s {
        Shape::Arrow { a, b, color, width, heads, .. } => {
            let head = doc::arrow_head(*a, *b, *width);
            let back = doc::arrow_head(*b, *a, *width);
            let end = if *heads >= 1 { head[1] + (head[2] - head[1]) * 0.5 } else { *b };
            let start = if *heads >= 2 { back[1] + (back[2] - back[1]) * 0.5 } else { *a };
            if let Some(p) = polyline(&[start, end]) {
                pm.stroke_path(&p, &paint(col(*color)), &stroke(*width), ID, None);
            }
            let mut tips = Vec::new();
            if *heads >= 1 {
                tips.push(head);
            }
            if *heads >= 2 {
                tips.push(back);
            }
            for t in tips {
                if let Some(p) = polygon(&t) {
                    pm.fill_path(&p, &paint(col(*color)), ts::FillRule::Winding, ID, None);
                }
            }
        }
        Shape::Line { a, b, color, width, .. } => {
            if let Some(p) = polyline(&[*a, *b]) {
                pm.stroke_path(&p, &paint(col(*color)), &stroke(*width), ID, None);
            }
        }
        Shape::Rect { rect, color, width, fill, radius, .. } => {
            if let Some(p) = rounded_rect(*rect, *radius) {
                if *fill {
                    pm.fill_path(&p, &paint(col(*color)), ts::FillRule::Winding, ID, None);
                } else {
                    let mut st = stroke(*width);
                    if *radius <= 0.0 {
                        st.line_cap = ts::LineCap::Square;
                        st.line_join = ts::LineJoin::Miter;
                    }
                    pm.stroke_path(&p, &paint(col(*color)), &st, ID, None);
                }
            }
        }
        Shape::Ellipse { rect, color, width, fill, .. } => {
            if let Some(p) = ts::Rect::from_ltrb(rect.min.x, rect.min.y, rect.max.x, rect.max.y).and_then(ts::PathBuilder::from_oval) {
                if *fill {
                    pm.fill_path(&p, &paint(col(*color)), ts::FillRule::Winding, ID, None);
                } else {
                    pm.stroke_path(&p, &paint(col(*color)), &stroke(*width), ID, None);
                }
            }
        }
        Shape::Pen { points, color, width, .. } => {
            if let Some(p) = polyline(points) {
                pm.stroke_path(&p, &paint(col(*color)), &stroke(*width), ID, None);
            }
        }
        Shape::Highlight { points, color, width } => {
            if let Some(p) = polyline(points) {
                let mut st = stroke(width * doc::HIGHLIGHT_WIDTH_FACTOR);
                st.line_cap = ts::LineCap::Square;
                pm.stroke_path(&p, &paint(doc::highlight_color(*color)), &st, ID, None);
            }
        }
        Shape::Text { pos, text, color, size, bg, font, bold, italic, outline, align, .. } => {
            let fonts = Fonts::new(font, *bold, *italic);
            if *bg {
                let r = text_box(*pos, text, *size, font, *bold, *italic);
                if let Some(p) = rounded_rect(r, size * 0.25) {
                    pm.fill_path(&p, &paint(col(*color)), ts::FillRule::Winding, ID, None);
                }
                if !as_shadow {
                    draw_text_aligned(pm, &fonts, text, *pos, *size, halo_color(*color), None, false, *align);
                }
            } else if as_shadow {
                draw_text_aligned(pm, &fonts, text, *pos, *size, doc::shadow_color(), None, false, *align);
            } else {
                let halo = if *outline { Some(halo_color(*color)) } else { None };
                draw_text_aligned(pm, &fonts, text, *pos, *size, *color, halo, false, *align);
            }
        }
        Shape::Callout { pos, tail, text, color, size, font, bold, italic, shape, outline, align, .. } => {
            let bubble = callout_bubble(*pos, text, *size, font, *bold, *italic);
            let body = callout_body(bubble, *shape, *size);
            let tri = doc::callout_tail(bubble, *tail);
            let tri_path = polygon(&tri);
            if let Some(p) = &body {
                pm.fill_path(p, &paint(col(*color)), ts::FillRule::Winding, ID, None);
            }
            if let Some(p) = &tri_path {
                pm.fill_path(p, &paint(col(*color)), ts::FillRule::Winding, ID, None);
            }
            if *outline && !as_shadow {
                let edge = color.gamma_multiply(0.55);
                let edge = Color32::from_rgb(edge.r(), edge.g(), edge.b());
                for p in [&body, &tri_path].into_iter().flatten() {
                    pm.stroke_path(p, &paint(edge), &stroke((size / 12.0).max(1.5)), ID, None);
                }
            }
            if !as_shadow {
                draw_text_aligned(pm, &Fonts::new(font, *bold, *italic), text, *pos, *size, halo_color(*color), None, false, *align);
            }
        }
        Shape::Image { rect, image } => {
            if let Some(src) = to_pixmap(&image.0) {
                let sx = rect.width() / image.0.width().max(1) as f32;
                let sy = rect.height() / image.0.height().max(1) as f32;
                let t = ts::Transform::from_row(sx, 0.0, 0.0, sy, rect.min.x, rect.min.y);
                let paint = ts::PixmapPaint { quality: ts::FilterQuality::Bilinear, ..Default::default() };
                pm.draw_pixmap(0, 0, src.as_ref(), &paint, t, None);
            }
        }
        Shape::Step { center, n, color, radius, style, .. } => {
            if let Some(p) = ts::PathBuilder::from_circle(center.x, center.y, *radius) {
                pm.fill_path(&p, &paint(col(*color)), ts::FillRule::Winding, ID, None);
                if !as_shadow {
                    pm.stroke_path(&p, &paint(Color32::WHITE), &stroke((radius / 8.0).max(2.0)), ID, None);
                }
            }
            if !as_shadow {
                let label = style.format(*n);
                let shrink = if label.chars().count() > 2 { 0.75 } else { 1.0 };
                draw_text(pm, &label, *center, radius * 1.15 * shrink, halo_color(*color), None, true);
            }
        }
        Shape::Stamp { center, emoji, size, color } => {
            if let Some(ch) = emoji.chars().next() {
                let px = size.round().max(8.0) as u32;
                let [r, g, b, _] = color.to_srgba_unmultiplied();
                if let Some(img) = crate::emoji::render(ch, px, [r, g, b, 255]) {
                    draw_image(pm, &img, *center - Vec2::splat(px as f32 / 2.0));
                }
            }
        }
        Shape::Magnify { center, radius, zoom } => magnify(pm, base, *center, *radius, *zoom),
        Shape::Pointer { pos, size } => {
            let pts = doc::pointer_outline(*pos, *size);
            if let Some(p) = polygon(&pts) {
                pm.fill_path(&p, &paint(col(Color32::WHITE)), ts::FillRule::Winding, ID, None);
                if !as_shadow {
                    pm.stroke_path(&p, &paint(Color32::from_rgb(20, 20, 20)), &stroke((size / 18.0).max(1.0)), ID, None);
                }
            }
        }
        Shape::Spotlight { rect } => {
            let (w, h) = (pm.width() as f32, pm.height() as f32);
            let mut pb = ts::PathBuilder::new();
            if let Some(full) = ts::Rect::from_xywh(0.0, 0.0, w, h) {
                pb.push_rect(full);
            }
            if let Some(hole) = ts::Rect::from_ltrb(rect.min.x, rect.min.y, rect.max.x, rect.max.y) {
                pb.push_rect(hole);
            }
            if let Some(p) = pb.finish() {
                pm.fill_path(&p, &paint(Color32::from_black_alpha(doc::SPOTLIGHT_ALPHA)), ts::FillRule::EvenOdd, ID, None);
            }
        }
    }
}

pub fn to_pixmap(img: &RgbaImage) -> Option<ts::Pixmap> {
    let (w, h) = img.dimensions();
    let mut data = img.as_raw().clone();
    for px in data.chunks_exact_mut(4) {
        let a = px[3] as u16;
        if a < 255 {
            for c in 0..3 {
                px[c] = ((px[c] as u16 * a + 127) / 255) as u8;
            }
        }
    }
    ts::IntSize::from_wh(w, h).and_then(|s| ts::Pixmap::from_vec(data, s))
}

pub fn from_pixmap(pm: ts::Pixmap) -> RgbaImage {
    let (w, h) = (pm.width(), pm.height());
    let mut out = pm.take();
    for px in out.chunks_exact_mut(4) {
        let a = px[3] as u16;
        if a > 0 && a < 255 {
            for c in 0..3 {
                px[c] = ((px[c] as u16 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    RgbaImage::from_raw(w, h, out).expect("pixmap size")
}

pub fn compose(d: &Doc) -> RgbaImage {
    let Some(mut pm) = to_pixmap(&d.image) else { return (*d.image).clone() };
    for s in &d.shapes {
        if s.shadow() {
            let mut sh = s.clone();
            sh.translate(s.shadow_offset());
            draw_shape(&mut pm, &d.image, &sh, true);
        }
        draw_shape(&mut pm, &d.image, s, false);
    }
    from_pixmap(pm)
}

/// Cut a picture to a shape (alpha outside it): circle, star, lasso... `points` are in the
/// picture's pixels; for the built-in shapes they're ignored.
pub fn cut_to_shape(img: &RgbaImage, shape: crate::capture::CutShape, points: &[Pos2]) -> RgbaImage {
    use crate::capture::CutShape;
    let (w, h) = (img.width() as f32, img.height() as f32);
    let r = Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h));
    let path = match shape {
        CutShape::Rect => return img.clone(),
        CutShape::Ellipse => ts::Rect::from_xywh(0.0, 0.0, w, h).and_then(ts::PathBuilder::from_oval),
        CutShape::Rounded => rounded_rect(r, w.min(h) * 0.12),
        CutShape::Freehand | CutShape::Polygon => polygon(points),
        CutShape::Triangle => polygon(&[pos2(w / 2.0, 0.0), pos2(w, h), pos2(0.0, h)]),
        CutShape::Star => polygon(&star_points(r)),
        CutShape::Heart => heart_path(r),
    };
    let Some(path) = path else { return img.clone() };
    let Some(mut mask) = ts::Mask::new(img.width(), img.height()) else { return img.clone() };
    mask.fill_path(&path, ts::FillRule::Winding, true, ID);
    let mut out = img.clone();
    for (p, m) in out.pixels_mut().zip(mask.data()) {
        p.0[3] = ((p.0[3] as u16 * *m as u16) / 255) as u8;
    }
    out
}

pub fn star_points(r: Rect) -> Vec<Pos2> {
    let c = r.center();
    let (rx, ry) = (r.width() / 2.0, r.height() / 2.0);
    (0..10)
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
            let k = if i % 2 == 0 { 1.0 } else { 0.42 };
            pos2(c.x + a.cos() * rx * k, c.y + a.sin() * ry * k + ry * 0.05)
        })
        .collect()
}

/// A heart filling `r`, as points (for the on-screen outline) and a path.
pub fn heart_points(r: Rect) -> Vec<Pos2> {
    (0..64)
        .map(|i| {
            let t = i as f32 / 64.0 * std::f32::consts::TAU;
            let x = 16.0 * t.sin().powi(3);
            let y = 13.0 * t.cos() - 5.0 * (2.0 * t).cos() - 2.0 * (3.0 * t).cos() - (4.0 * t).cos();
            pos2(r.center().x + x / 17.0 * r.width() / 2.0, r.min.y + (12.5 - y) / 29.0 * r.height())
        })
        .collect()
}

fn heart_path(r: Rect) -> Option<ts::Path> {
    polygon(&heart_points(r))
}

/// The body of a callout in its chosen shape.
pub fn callout_body(bubble: Rect, shape: crate::settings::CalloutShape, size: f32) -> Option<ts::Path> {
    use crate::settings::CalloutShape;
    match shape {
        CalloutShape::Rounded => rounded_rect(bubble, size * 0.45),
        CalloutShape::Rectangle => rounded_rect(bubble, 0.0),
        CalloutShape::Oval => {
            let r = bubble.expand2(Vec2::new(bubble.width() * 0.12, bubble.height() * 0.18));
            ts::Rect::from_ltrb(r.min.x, r.min.y, r.max.x, r.max.y).and_then(ts::PathBuilder::from_oval)
        }
    }
}
