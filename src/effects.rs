//! Whole-picture effects for the Image menu. Each takes the flattened picture and returns a
//! new one.

use crate::render;
use eframe::egui::{Color32, pos2};
use image::{Rgba, RgbaImage, imageops};

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Effect {
    RotateLeft,
    RotateRight,
    FlipHorizontal,
    FlipVertical,
    Resize,
    Border,
    DropShadow,
    RoundedCorners,
    TornEdge,
    Grayscale,
    Invert,
    Sepia,
    AdjustColors,
    Sharpen,
    Soften,
    Tilt,
    CaptureInfo,
    Watermark,
}

impl Effect {
    pub const ALL: [Effect; 18] = [
        Effect::RotateLeft,
        Effect::RotateRight,
        Effect::FlipHorizontal,
        Effect::FlipVertical,
        Effect::Resize,
        Effect::Border,
        Effect::DropShadow,
        Effect::RoundedCorners,
        Effect::TornEdge,
        Effect::Grayscale,
        Effect::Invert,
        Effect::Sepia,
        Effect::AdjustColors,
        Effect::Sharpen,
        Effect::Soften,
        Effect::Tilt,
        Effect::CaptureInfo,
        Effect::Watermark,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Effect::RotateLeft => "Rotate left",
            Effect::RotateRight => "Rotate right",
            Effect::FlipHorizontal => "Flip horizontally",
            Effect::FlipVertical => "Flip vertically",
            Effect::Resize => "Resize",
            Effect::Border => "Add border",
            Effect::DropShadow => "Drop shadow",
            Effect::RoundedCorners => "Rounded corners",
            Effect::TornEdge => "Torn edge",
            Effect::Grayscale => "Grayscale",
            Effect::Invert => "Invert colors",
            Effect::Sepia => "Sepia",
            Effect::AdjustColors => "Adjust brightness, contrast and color",
            Effect::Sharpen => "Sharpen",
            Effect::Soften => "Soften (blur)",
            Effect::Tilt => "Perspective tilt",
            Effect::CaptureInfo => "Capture info (date, program, title)",
            Effect::Watermark => "Add watermark",
        }
    }

    /// A separator goes before these in the menu.
    pub fn starts_group(self) -> bool {
        matches!(self, Effect::Resize | Effect::Border | Effect::Grayscale | Effect::Tilt | Effect::Watermark)
    }
}

pub struct EffectOptions {
    pub resize_percent: u32,
    pub border_width: u32,
    pub border_color: [u8; 3],
    pub shadow_size: u32,
    pub rounded_radius: u32,
    pub watermark_text: String,
    pub tilt_percent: u32,
    pub brightness: i32,
    pub contrast: i32,
    pub saturation: i32,
    pub info_text: String,
}

pub fn apply(e: Effect, img: &RgbaImage, o: &EffectOptions) -> RgbaImage {
    match e {
        Effect::RotateLeft => imageops::rotate270(img),
        Effect::RotateRight => imageops::rotate90(img),
        Effect::FlipHorizontal => imageops::flip_horizontal(img),
        Effect::FlipVertical => imageops::flip_vertical(img),
        Effect::Resize => {
            let p = o.resize_percent.clamp(5, 800) as f32 / 100.0;
            let w = ((img.width() as f32 * p).round() as u32).max(1);
            let h = ((img.height() as f32 * p).round() as u32).max(1);
            imageops::resize(img, w, h, imageops::FilterType::Lanczos3)
        }
        Effect::Border => border(img, o.border_width, o.border_color),
        Effect::DropShadow => drop_shadow(img, o.shadow_size),
        Effect::RoundedCorners => rounded(img, o.rounded_radius as f32),
        Effect::TornEdge => torn_edge(img),
        Effect::Grayscale => map_pixels(img, |r, g, b| {
            let l = (0.299 * r + 0.587 * g + 0.114 * b).round();
            (l, l, l)
        }),
        Effect::Invert => map_pixels(img, |r, g, b| (255.0 - r, 255.0 - g, 255.0 - b)),
        Effect::Sepia => map_pixels(img, |r, g, b| {
            (
                0.393 * r + 0.769 * g + 0.189 * b,
                0.349 * r + 0.686 * g + 0.168 * b,
                0.272 * r + 0.534 * g + 0.131 * b,
            )
        }),
        Effect::AdjustColors => adjust(img, o.brightness, o.contrast, o.saturation),
        Effect::Sharpen => imageops::unsharpen(img, 1.2, 2),
        Effect::Soften => imageops::blur(img, 2.0),
        Effect::Tilt => tilt(img, o.tilt_percent),
        Effect::CaptureInfo => capture_info(img, &o.info_text),
        Effect::Watermark => watermark(img, &o.watermark_text),
    }
}

fn map_pixels(img: &RgbaImage, f: impl Fn(f32, f32, f32) -> (f32, f32, f32)) -> RgbaImage {
    let mut out = img.clone();
    for p in out.pixels_mut() {
        let (r, g, b) = f(p.0[0] as f32, p.0[1] as f32, p.0[2] as f32);
        p.0[0] = r.clamp(0.0, 255.0) as u8;
        p.0[1] = g.clamp(0.0, 255.0) as u8;
        p.0[2] = b.clamp(0.0, 255.0) as u8;
    }
    out
}

fn border(img: &RgbaImage, width: u32, c: [u8; 3]) -> RgbaImage {
    let w = width.min(500);
    let mut out = RgbaImage::from_pixel(img.width() + 2 * w, img.height() + 2 * w, Rgba([c[0], c[1], c[2], 255]));
    imageops::overlay(&mut out, img, w as i64, w as i64);
    out
}

/// Three box blurs in a row: close to a Gaussian blur, and fast.
fn blur_alpha(a: &mut [f32], w: usize, h: usize, radius: usize) {
    if radius == 0 {
        return;
    }
    let mut tmp = vec![0f32; a.len()];
    for _ in 0..3 {
        for y in 0..h {
            let row = &a[y * w..(y + 1) * w];
            let mut sum: f32 = row[..radius.min(w)].iter().sum();
            for x in 0..w {
                if x + radius < w {
                    sum += row[x + radius];
                }
                if x > radius {
                    sum -= row[x - radius - 1];
                }
                let lo = x.saturating_sub(radius);
                let hi = (x + radius).min(w - 1);
                tmp[y * w + x] = sum / (hi - lo + 1) as f32;
            }
        }
        for x in 0..w {
            let mut sum = 0f32;
            for y in 0..radius.min(h) {
                sum += tmp[y * w + x];
            }
            for y in 0..h {
                if y + radius < h {
                    sum += tmp[(y + radius) * w + x];
                }
                if y > radius {
                    sum -= tmp[(y - radius - 1) * w + x];
                }
                let lo = y.saturating_sub(radius);
                let hi = (y + radius).min(h - 1);
                a[y * w + x] = sum / (hi - lo + 1) as f32;
            }
        }
    }
}

fn drop_shadow(img: &RgbaImage, size: u32) -> RgbaImage {
    let size = size.clamp(2, 200);
    let margin = size * 2;
    let off = size / 2;
    let (w, h) = (img.width() + margin * 2, img.height() + margin * 2);
    let mut alpha = vec![0f32; (w * h) as usize];
    for (x, y, p) in img.enumerate_pixels() {
        let (sx, sy) = (x + margin + off, y + margin + off);
        if sx < w && sy < h {
            alpha[(sy * w + sx) as usize] = p.0[3] as f32 / 255.0;
        }
    }
    blur_alpha(&mut alpha, w as usize, h as usize, (size / 2).max(1) as usize);
    let mut out = RgbaImage::new(w, h);
    for (i, p) in out.pixels_mut().enumerate() {
        *p = Rgba([0, 0, 0, (alpha[i] * 150.0).min(255.0) as u8]);
    }
    imageops::overlay(&mut out, img, margin as i64, margin as i64);
    out
}

fn rounded(img: &RgbaImage, radius: f32) -> RgbaImage {
    let mut out = img.clone();
    let (w, h) = (img.width() as f32, img.height() as f32);
    let r = radius.min(w / 2.0).min(h / 2.0).max(0.0);
    for (x, y, p) in out.enumerate_pixels_mut() {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let cx = fx.clamp(r, w - r);
        let cy = fy.clamp(r, h - r);
        let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
        let cover = (r - d + 0.5).clamp(0.0, 1.0);
        if d > 0.0 {
            p.0[3] = (p.0[3] as f32 * cover) as u8;
        }
    }
    out
}

fn torn_edge(img: &RgbaImage) -> RgbaImage {
    let mut out = img.clone();
    let (w, h) = (img.width(), img.height());
    let depth = (h as f32 * 0.04).clamp(6.0, 24.0);
    let tooth = (depth * 1.4).max(8.0);
    // A fixed pseudo-random ragged line along the bottom.
    let mut seed: u32 = 0x9e37_79b9;
    let mut points = Vec::new();
    let mut x = 0.0;
    while x <= w as f32 + tooth {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let r = (seed % 1000) as f32 / 1000.0;
        points.push((x, h as f32 - depth * (0.2 + 0.8 * r)));
        x += tooth * (0.6 + 0.4 * r);
    }
    for px in 0..w {
        let fx = px as f32;
        let i = points.iter().position(|p| p.0 > fx).unwrap_or(points.len() - 1).max(1);
        let (a, b) = (points[i - 1], points[i]);
        let t = if b.0 > a.0 { (fx - a.0) / (b.0 - a.0) } else { 0.0 };
        let edge = a.1 + (b.1 - a.1) * t;
        for py in 0..h {
            let fy = py as f32 + 0.5;
            if fy > edge {
                let cover = (edge - (fy - 1.0)).clamp(0.0, 1.0);
                let p = out.get_pixel_mut(px, py);
                p.0[3] = (p.0[3] as f32 * cover) as u8;
            }
        }
    }
    out
}

fn watermark(img: &RgbaImage, text: &str) -> RgbaImage {
    let text = if text.trim().is_empty() { "Snagster" } else { text };
    let size = (img.height().min(img.width()) as f32 * 0.04).clamp(14.0, 64.0);
    let tw = render::text_width(text, size);
    let th = render::text_height(text, size);
    let pos = pos2(img.width() as f32 - tw - size, img.height() as f32 - th - size * 0.6);
    let Some(mut pm) = render::to_pixmap(img) else { return img.clone() };
    render::draw_text(
        &mut pm,
        text,
        pos,
        size,
        Color32::from_rgba_unmultiplied(255, 255, 255, 200),
        Some(Color32::from_rgba_unmultiplied(0, 0, 0, 120)),
        false,
    );
    render::from_pixmap(pm)
}

/// Put several pictures next to each other (or under each other) on one canvas.
pub fn combine(images: &[RgbaImage], vertical: bool, gap: u32) -> Option<RgbaImage> {
    if images.is_empty() {
        return None;
    }
    let (w, h) = if vertical {
        (
            images.iter().map(|i| i.width()).max()?,
            images.iter().map(|i| i.height()).sum::<u32>() + gap * (images.len() as u32 - 1),
        )
    } else {
        (
            images.iter().map(|i| i.width()).sum::<u32>() + gap * (images.len() as u32 - 1),
            images.iter().map(|i| i.height()).max()?,
        )
    };
    let mut out = RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255]));
    let mut at = 0i64;
    for i in images {
        if vertical {
            imageops::overlay(&mut out, i, 0, at);
            at += i.height() as i64 + gap as i64;
        } else {
            imageops::overlay(&mut out, i, at, 0);
            at += i.width() as i64 + gap as i64;
        }
    }
    Some(out)
}

/// Brightness and contrast in -100..100 steps, saturation likewise.
fn adjust(img: &RgbaImage, brightness: i32, contrast: i32, saturation: i32) -> RgbaImage {
    let b = brightness.clamp(-100, 100) as f32 * 1.28;
    let c = contrast.clamp(-100, 100) as f32 / 100.0;
    let k = (1.0 + c) / (1.0 - c * 0.99);
    let s = 1.0 + saturation.clamp(-100, 100) as f32 / 100.0;
    map_pixels(img, |r, g, bl| {
        let f = |v: f32| (v - 128.0) * k + 128.0 + b;
        let (r, g, bl) = (f(r), f(g), f(bl));
        let l = 0.299 * r + 0.587 * g + 0.114 * bl;
        (l + (r - l) * s, l + (g - l) * s, l + (bl - l) * s)
    })
}

/// Turn the picture away like an open door: the left edge shrinks by `percent`.
fn tilt(img: &RgbaImage, percent: u32) -> RgbaImage {
    let t = percent.clamp(1, 60) as f32 / 100.0;
    let (w, h) = (img.width(), img.height());
    let mut out = RgbaImage::new(w, h);
    let hf = h as f32;
    for x in 0..w {
        let u = x as f32 / (w.max(2) - 1) as f32;
        // Columns further away get shorter, and squeezed together.
        let k = 1.0 - t * (1.0 - u);
        let src_u = u.powf(1.0 + t * 0.6);
        let sx = (src_u * (w - 1) as f32).round() as u32;
        let top = hf * (1.0 - k) / 2.0;
        for y in 0..h {
            let fy = y as f32;
            if fy < top || fy >= hf - top {
                continue;
            }
            let sy = ((fy - top) / k).clamp(0.0, hf - 1.0);
            let y0 = sy.floor() as u32;
            let y1 = (y0 + 1).min(h - 1);
            let a = sy - y0 as f32;
            let p0 = img.get_pixel(sx, y0).0;
            let p1 = img.get_pixel(sx, y1).0;
            let mut px = [0u8; 4];
            for i in 0..4 {
                px[i] = (p0[i] as f32 * (1.0 - a) + p1[i] as f32 * a) as u8;
            }
            out.put_pixel(x, y, Rgba(px));
        }
    }
    out
}

/// A one-page guide: a title, then each picture with its numbered caption, on white.
pub fn guide_page(title: &str, items: &[(String, RgbaImage)], width: u32) -> Option<RgbaImage> {
    if items.is_empty() {
        return None;
    }
    let width = width.clamp(400, 3000);
    let margin = 48u32;
    let inner = width - margin * 2;
    let title_size = 40.0;
    let cap_size = 24.0;
    let mut parts: Vec<(String, RgbaImage)> = Vec::new();
    for (cap, img) in items {
        let img = if img.width() > inner {
            let h = (img.height() as f32 * inner as f32 / img.width() as f32).round().max(1.0) as u32;
            imageops::resize(img, inner, h, imageops::FilterType::Triangle)
        } else {
            img.clone()
        };
        parts.push((cap.clone(), img));
    }
    let cap_h = (cap_size * 1.6) as u32;
    let mut height = margin + (title_size * 1.6) as u32 + 16;
    for (_, img) in &parts {
        height += cap_h + img.height() + 36;
    }
    height += margin;
    let mut out = RgbaImage::from_pixel(width, height, Rgba([255, 255, 255, 255]));
    let mut pm = render::to_pixmap(&out)?;
    let ink = Color32::from_rgb(28, 30, 38);
    render::draw_text(&mut pm, title, pos2(margin as f32, margin as f32), title_size, ink, None, false);
    out = render::from_pixmap(pm);
    let mut y = margin + (title_size * 1.6) as u32 + 16;
    for (i, (cap, img)) in parts.iter().enumerate() {
        let mut pm = render::to_pixmap(&out)?;
        render::draw_text(&mut pm, &format!("{}.  {}", i + 1, cap), pos2(margin as f32, y as f32), cap_size, ink, None, false);
        out = render::from_pixmap(pm);
        y += cap_h;
        // A light frame around each picture.
        let framed = border(img, 1, [210, 214, 222]);
        imageops::overlay(&mut out, &framed, (margin - 1) as i64, (y - 1) as i64);
        y += img.height() + 36;
    }
    Some(out)
}

/// Add a strip under the picture with when and where it was captured.
fn capture_info(img: &RgbaImage, info: &str) -> RgbaImage {
    let size = (img.width() as f32 * 0.018).clamp(13.0, 28.0);
    let strip = (size * 2.0) as u32;
    let mut out = RgbaImage::from_pixel(img.width(), img.height() + strip, Rgba([32, 34, 42, 255]));
    imageops::overlay(&mut out, img, 0, 0);
    let Some(mut pm) = render::to_pixmap(&out) else { return out };
    let text = if info.trim().is_empty() { chrono::Local::now().format("%Y-%m-%d %H:%M").to_string() } else { info.to_string() };
    render::draw_text(&mut pm, &text, pos2(size * 0.7, img.height() as f32 + size * 0.45), size, Color32::from_rgb(225, 228, 236), None, false);
    render::from_pixmap(pm)
}

/// Remove edges that are one flat colour (within a small tolerance).
pub fn trim(img: &RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    if w < 3 || h < 3 {
        return img.clone();
    }
    let corner = img.get_pixel(0, 0).0;
    let same = |x: u32, y: u32| {
        let p = img.get_pixel(x, y).0;
        (0..4).all(|i| (p[i] as i16 - corner[i] as i16).abs() <= 12)
    };
    let row_flat = |y: u32| (0..w).all(|x| same(x, y));
    let col_flat = |x: u32, y0: u32, y1: u32| (y0..y1).all(|y| same(x, y));
    let mut top = 0;
    while top < h - 1 && row_flat(top) {
        top += 1;
    }
    let mut bottom = h;
    while bottom > top + 1 && row_flat(bottom - 1) {
        bottom -= 1;
    }
    let mut left = 0;
    while left < w - 1 && col_flat(left, top, bottom) {
        left += 1;
    }
    let mut right = w;
    while right > left + 1 && col_flat(right - 1, top, bottom) {
        right -= 1;
    }
    imageops::crop_imm(img, left, top, right - left, bottom - top).to_image()
}

/// Put the picture on a canvas of a new size and colour, centred.
pub fn canvas(img: &RgbaImage, w: u32, h: u32, color: [u8; 4]) -> RgbaImage {
    let mut out = RgbaImage::from_pixel(w.max(1), h.max(1), Rgba(color));
    let x = (w as i64 - img.width() as i64) / 2;
    let y = (h as i64 - img.height() as i64) / 2;
    imageops::overlay(&mut out, img, x, y);
    out
}

pub fn resize_exact(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    imageops::resize(img, w.clamp(1, 20000), h.clamp(1, 20000), imageops::FilterType::Lanczos3)
}

/// Pictures in a grid of `cols` columns on white, each cell the size of the largest picture.
pub fn grid(images: &[RgbaImage], cols: u32, gap: u32, labels: &[&str]) -> Option<RgbaImage> {
    if images.is_empty() {
        return None;
    }
    let cols = cols.max(1).min(images.len() as u32);
    let rows = (images.len() as u32).div_ceil(cols);
    let cw = images.iter().map(|i| i.width()).max()?.min(1600);
    let ch = images.iter().map(|i| (i.height() as f32 * (cw as f32 / i.width() as f32).min(1.0)) as u32).max()?.max(1);
    let label_h = if labels.is_empty() { 0 } else { 44 };
    let w = cols * cw + (cols + 1) * gap;
    let h = rows * (ch + label_h) + (rows + 1) * gap;
    let mut out = RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255]));
    for (i, img) in images.iter().enumerate() {
        let (c, r) = (i as u32 % cols, i as u32 / cols);
        let s = (cw as f32 / img.width() as f32).min(ch as f32 / img.height() as f32).min(1.0);
        let scaled = imageops::resize(img, ((img.width() as f32 * s) as u32).max(1), ((img.height() as f32 * s) as u32).max(1), imageops::FilterType::Triangle);
        let x = gap + c * (cw + gap) + (cw - scaled.width()) / 2;
        let y = gap + r * (ch + label_h + gap) + label_h;
        imageops::overlay(&mut out, &scaled, x as i64, y as i64);
        if let Some(l) = labels.get(i) {
            let mut pm = render::to_pixmap(&out)?;
            render::draw_text(&mut pm, l, pos2((gap + c * (cw + gap)) as f32, (gap + r * (ch + label_h + gap)) as f32 + 4.0), 28.0, Color32::from_rgb(28, 30, 38), None, false);
            out = render::from_pixmap(pm);
        }
    }
    Some(out)
}
