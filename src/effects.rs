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

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> EffectOptions {
        EffectOptions {
            resize_percent: 50,
            border_width: 3,
            border_color: [10, 20, 30],
            shadow_size: 8,
            rounded_radius: 6,
            watermark_text: String::new(),
            tilt_percent: 20,
            brightness: 0,
            contrast: 0,
            saturation: 0,
            info_text: "Captured here".into(),
        }
    }

    /// A small picture where every pixel is different.
    fn gradient(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| Rgba([(x * 20) as u8, (y * 20) as u8, ((x + y) * 7) as u8, 255]))
    }

    fn solid(w: u32, h: u32, c: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba(c))
    }

    fn differs(a: &RgbaImage, b: &RgbaImage) -> usize {
        a.pixels().zip(b.pixels()).filter(|(p, q)| p != q).count()
    }

    #[test]
    fn every_effect_has_a_unique_label() {
        let mut labels: Vec<_> = Effect::ALL.iter().map(|e| e.label()).collect();
        assert!(labels.iter().all(|l| !l.is_empty()));
        labels.sort();
        labels.dedup();
        assert_eq!(labels.len(), Effect::ALL.len());
        assert!(!Effect::RotateLeft.starts_group());
        assert!(Effect::Border.starts_group());
        assert_eq!(Effect::ALL.iter().filter(|e| e.starts_group()).count(), 5);
    }

    #[test]
    fn every_effect_runs_on_a_small_picture() {
        let img = gradient(12, 9);
        for e in Effect::ALL {
            let out = apply(e, &img, &opts());
            assert!(out.width() > 0 && out.height() > 0, "{e:?}");
        }
    }

    #[test]
    fn rotate_and_flip_move_pixels() {
        let img = gradient(4, 3);
        let o = opts();
        let l = apply(Effect::RotateLeft, &img, &o);
        let r = apply(Effect::RotateRight, &img, &o);
        assert_eq!(l.dimensions(), (3, 4));
        assert_eq!(r.dimensions(), (3, 4));
        // Rotating right puts the top-left corner at the top-right.
        assert_eq!(r.get_pixel(2, 0), img.get_pixel(0, 0));
        // Rotating left puts it at the bottom-left.
        assert_eq!(l.get_pixel(0, 3), img.get_pixel(0, 0));
        assert_eq!(apply(Effect::RotateLeft, &r, &o), img);
        let h = apply(Effect::FlipHorizontal, &img, &o);
        assert_eq!(h.get_pixel(3, 0), img.get_pixel(0, 0));
        let v = apply(Effect::FlipVertical, &img, &o);
        assert_eq!(v.get_pixel(0, 2), img.get_pixel(0, 0));
        assert_eq!(apply(Effect::FlipVertical, &v, &o), img);
    }

    #[test]
    fn resize_uses_the_percentage_within_limits() {
        let img = gradient(20, 10);
        let mut o = opts();
        assert_eq!(apply(Effect::Resize, &img, &o).dimensions(), (10, 5));
        o.resize_percent = 200;
        assert_eq!(apply(Effect::Resize, &img, &o).dimensions(), (40, 20));
        o.resize_percent = 0; // clamped to 5 %, never smaller than one pixel
        assert_eq!(apply(Effect::Resize, &img, &o).dimensions(), (1, 1));
        o.resize_percent = 10_000; // clamped to 800 %
        assert_eq!(apply(Effect::Resize, &img, &o).dimensions(), (160, 80));
    }

    #[test]
    fn border_surrounds_the_picture() {
        let img = gradient(5, 4);
        let out = border(&img, 3, [10, 20, 30]);
        assert_eq!(out.dimensions(), (11, 10));
        assert_eq!(out.get_pixel(0, 0).0, [10, 20, 30, 255]);
        assert_eq!(out.get_pixel(10, 9).0, [10, 20, 30, 255]);
        assert_eq!(out.get_pixel(3, 3), img.get_pixel(0, 0));
        assert_eq!(out.get_pixel(7, 6), img.get_pixel(4, 3));
        assert_eq!(border(&img, 10_000, [0, 0, 0]).width(), 5 + 1000, "width is capped at 500");
    }

    #[test]
    fn blur_alpha_keeps_flat_areas_and_spreads_dots() {
        let mut flat = vec![0.5f32; 30 * 20];
        blur_alpha(&mut flat, 30, 20, 3);
        assert!(flat.iter().all(|v| (v - 0.5).abs() < 1e-5));

        let mut same = vec![0f32; 25];
        same[12] = 1.0;
        let before = same.clone();
        blur_alpha(&mut same, 5, 5, 0);
        assert_eq!(same, before, "radius 0 does nothing");

        let (w, h) = (21usize, 21usize);
        let mut dot = vec![0f32; w * h];
        dot[10 * w + 10] = 1.0;
        blur_alpha(&mut dot, w, h, 2);
        let c = dot[10 * w + 10];
        assert!(c < 1.0 && c > 0.0);
        // Symmetric spread, falling off with distance.
        assert!((dot[10 * w + 8] - dot[10 * w + 12]).abs() < 1e-6);
        assert!((dot[8 * w + 10] - dot[12 * w + 10]).abs() < 1e-6);
        assert!(dot[10 * w + 11] < c && dot[10 * w + 13] < dot[10 * w + 11]);
        assert_eq!(dot[0], 0.0);
    }

    #[test]
    fn drop_shadow_adds_a_margin_with_a_soft_shadow() {
        let img = solid(10, 10, [200, 0, 0, 255]);
        let out = drop_shadow(&img, 8);
        // margin = 2 * size on every side.
        assert_eq!(out.dimensions(), (10 + 32, 10 + 32));
        assert_eq!(out.get_pixel(16, 16).0, [200, 0, 0, 255]);
        assert_eq!(out.get_pixel(0, 0).0[3], 0, "far corner has no shadow");
        // Down and to the right of the picture there is a dark see-through shadow.
        let s = out.get_pixel(16 + 11, 16 + 11).0;
        assert_eq!(&s[..3], &[0, 0, 0]);
        assert!(s[3] > 0 && s[3] <= 150);
        // Tiny sizes are clamped up to 2.
        assert_eq!(drop_shadow(&img, 0).dimensions(), (18, 18));
    }

    #[test]
    fn rounded_corners_clear_the_corners_only() {
        let img = solid(20, 20, [9, 9, 9, 255]);
        let out = rounded(&img, 6.0);
        assert_eq!(out.get_pixel(0, 0).0[3], 0);
        assert_eq!(out.get_pixel(19, 19).0[3], 0);
        assert_eq!(out.get_pixel(10, 10).0[3], 255);
        assert_eq!(out.get_pixel(10, 0).0[3], 255);
        assert_eq!(rounded(&img, 0.0), img);
        // A huge radius becomes a circle, never more.
        let circle = rounded(&img, 1000.0);
        assert_eq!(circle.get_pixel(10, 10).0[3], 255);
        assert_eq!(circle.get_pixel(1, 1).0[3], 0);
    }

    #[test]
    fn torn_edge_only_touches_the_bottom() {
        let img = solid(50, 100, [1, 2, 3, 255]);
        let out = torn_edge(&img);
        assert_eq!(out.dimensions(), img.dimensions());
        for x in 0..50 {
            assert_eq!(out.get_pixel(x, 0).0[3], 255);
            assert_eq!(out.get_pixel(x, 80).0[3], 255);
            assert!(out.get_pixel(x, 99).0[3] < 100, "the last row is always (mostly) torn off");
        }
        assert_eq!(torn_edge(&img), out, "the tear is the same every time");
    }

    #[test]
    fn colour_effects() {
        let img = RgbaImage::from_fn(3, 1, |x, _| match x {
            0 => Rgba([255, 0, 0, 255]),
            1 => Rgba([10, 200, 30, 128]),
            _ => Rgba([255, 255, 255, 255]),
        });
        let o = opts();
        let g = apply(Effect::Grayscale, &img, &o);
        for p in g.pixels() {
            assert!(p.0[0] == p.0[1] && p.0[1] == p.0[2]);
        }
        assert_eq!(g.get_pixel(0, 0).0, [76, 76, 76, 255]);
        assert_eq!(g.get_pixel(1, 0).0[3], 128, "alpha is kept");
        let inv = apply(Effect::Invert, &img, &o);
        assert_eq!(inv.get_pixel(0, 0).0, [0, 255, 255, 255]);
        assert_eq!(apply(Effect::Invert, &inv, &o), img);
        let sep = apply(Effect::Sepia, &img, &o);
        assert_eq!(sep.get_pixel(2, 0).0, [255, 255, 238, 255], "white is clamped, not wrapped");
    }

    #[test]
    fn adjust_colours() {
        let img = gradient(8, 8);
        let same = adjust(&img, 0, 0, 0);
        for (a, b) in same.pixels().zip(img.pixels()) {
            for i in 0..4 {
                assert!((a.0[i] as i16 - b.0[i] as i16).abs() <= 1);
            }
        }
        let grey = adjust(&img, 0, 0, -100);
        for p in grey.pixels() {
            assert!((p.0[0] as i16 - p.0[1] as i16).abs() <= 1 && (p.0[1] as i16 - p.0[2] as i16).abs() <= 1);
        }
        let mid = solid(1, 1, [100, 100, 100, 255]);
        assert!(adjust(&mid, 50, 0, 0).get_pixel(0, 0).0[0] > 150);
        assert!(adjust(&mid, -50, 0, 0).get_pixel(0, 0).0[0] < 50);
        // More contrast pushes values away from the middle.
        let dark = solid(1, 1, [100, 100, 100, 255]);
        assert!(adjust(&dark, 0, 50, 0).get_pixel(0, 0).0[0] < 100);
        // Out-of-range settings are clamped, not wrapped.
        assert_eq!(adjust(&mid, 1000, 0, 0), adjust(&mid, 100, 0, 0));
    }

    #[test]
    fn soften_and_sharpen_keep_a_flat_picture() {
        let img = solid(10, 10, [80, 90, 100, 255]);
        let o = opts();
        for (a, b) in apply(Effect::Soften, &img, &o).pixels().zip(img.pixels()) {
            assert!((0..4).all(|i| (a.0[i] as i16 - b.0[i] as i16).abs() <= 1));
        }
        assert_eq!(apply(Effect::Sharpen, &img, &o), img);
        let g = gradient(10, 10);
        assert_eq!(apply(Effect::Soften, &g, &o).dimensions(), (10, 10));
        assert!(differs(&apply(Effect::Soften, &g, &o), &g) > 0);
    }

    #[test]
    fn tilt_shrinks_the_left_edge() {
        let img = solid(40, 40, [5, 6, 7, 255]);
        let out = tilt(&img, 30);
        assert_eq!(out.dimensions(), (40, 40));
        // The left column is shorter: its top and bottom are empty.
        assert_eq!(out.get_pixel(0, 0).0[3], 0);
        assert_eq!(out.get_pixel(0, 39).0[3], 0);
        assert_eq!(out.get_pixel(0, 20).0, [5, 6, 7, 255]);
        // The right column keeps its full height.
        assert_eq!(out.get_pixel(39, 0).0, [5, 6, 7, 255]);
        assert_eq!(out.get_pixel(39, 39).0, [5, 6, 7, 255]);
        // One-pixel-wide pictures don't divide by zero.
        assert_eq!(tilt(&solid(1, 5, [1, 1, 1, 255]), 30).dimensions(), (1, 5));
    }

    #[test]
    fn watermark_and_capture_info_draw_text() {
        let img = solid(300, 200, [40, 40, 40, 255]);
        let wm = watermark(&img, "");
        assert_eq!(wm.dimensions(), img.dimensions());
        assert!(differs(&wm, &img) > 0, "the default text is drawn");
        // Only the bottom-right corner is touched.
        assert_eq!(wm.get_pixel(5, 5), img.get_pixel(5, 5));
        let info = capture_info(&img, "Hello");
        let strip = (13.0f32 * 2.0) as u32; // 300 * 0.018 < 13, so the smallest size
        assert_eq!(info.dimensions(), (300, 200 + strip));
        assert_eq!(info.get_pixel(299, 200 + strip - 1).0, [32, 34, 42, 255]);
        assert_eq!(info.get_pixel(10, 10), img.get_pixel(10, 10));
        let blank_strip = RgbaImage::from_pixel(300, strip, Rgba([32, 34, 42, 255]));
        let drawn = imageops::crop_imm(&info, 0, 200, 300, strip).to_image();
        assert!(differs(&drawn, &blank_strip) > 0, "the caption is drawn in the strip");
    }

    #[test]
    fn combine_side_by_side_and_stacked() {
        let a = solid(10, 5, [255, 0, 0, 255]);
        let b = solid(4, 8, [0, 0, 255, 255]);
        assert!(combine(&[], false, 4).is_none());
        let h = combine(&[a.clone(), b.clone()], false, 2).unwrap();
        assert_eq!(h.dimensions(), (16, 8));
        assert_eq!(h.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(h.get_pixel(11, 0).0, [255, 255, 255, 255], "the gap is white");
        assert_eq!(h.get_pixel(12, 7).0, [0, 0, 255, 255]);
        assert_eq!(h.get_pixel(0, 7).0, [255, 255, 255, 255]);
        let v = combine(&[a, b], true, 3).unwrap();
        assert_eq!(v.dimensions(), (10, 16));
        assert_eq!(v.get_pixel(0, 9).0, [0, 0, 255, 255]);
        assert_eq!(v.get_pixel(0, 6).0, [255, 255, 255, 255]);
    }

    #[test]
    fn guide_page_layout() {
        assert!(guide_page("T", &[], 800).is_none());
        let items = vec![("First".to_string(), solid(100, 50, [255, 0, 0, 255])), ("Second".to_string(), solid(2000, 100, [0, 255, 0, 255]))];
        let page = guide_page("Guide", &items, 100).unwrap();
        assert_eq!(page.width(), 400, "width is clamped to at least 400");
        // The wide picture is shrunk to the inner width (400 - 2*48 = 304), keeping its shape.
        let inner = 304u32;
        let second_h = (100.0f32 * inner as f32 / 2000.0).round() as u32;
        let expected = 48 + 64 + 16 + (38 + 50 + 36) + (38 + second_h + 36) + 48;
        assert_eq!(page.height(), expected);
        assert_eq!(page.get_pixel(399, page.height() - 1).0, [255, 255, 255, 255]);
        // The first picture sits at the left margin under its caption.
        let y = 48 + 64 + 16 + 38;
        assert_eq!(page.get_pixel(48 + 10, y + 10).0, [255, 0, 0, 255]);
    }

    #[test]
    fn trim_removes_flat_edges() {
        let mut img = solid(20, 10, [255, 255, 255, 255]);
        for y in 3..6 {
            for x in 5..9 {
                img.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
        let t = trim(&img);
        assert_eq!(t.dimensions(), (4, 3));
        assert!(t.pixels().all(|p| p.0 == [0, 0, 0, 255]));
        // Nearly the same colour counts as flat.
        img.put_pixel(19, 9, Rgba([250, 250, 250, 255]));
        assert_eq!(trim(&img).dimensions(), (4, 3));
        // Tiny pictures and pictures with no flat edge stay as they are.
        assert_eq!(trim(&solid(2, 2, [0, 0, 0, 255])).dimensions(), (2, 2));
        let g = gradient(8, 8);
        assert_eq!(trim(&g), g);
        assert_eq!(trim(&solid(6, 6, [1, 1, 1, 255])).dimensions(), (1, 1));
    }

    #[test]
    fn canvas_centres_the_picture() {
        let img = solid(4, 2, [255, 0, 0, 255]);
        let big = canvas(&img, 10, 6, [0, 0, 0, 0]);
        assert_eq!(big.dimensions(), (10, 6));
        assert_eq!(big.get_pixel(3, 2).0, [255, 0, 0, 255]);
        assert_eq!(big.get_pixel(2, 2).0, [0, 0, 0, 0]);
        assert_eq!(big.get_pixel(6, 3).0, [255, 0, 0, 255]);
        assert_eq!(big.get_pixel(7, 3).0, [0, 0, 0, 0]);
        let small = canvas(&gradient(10, 10), 4, 4, [0, 0, 0, 255]);
        assert_eq!(small.get_pixel(0, 0), gradient(10, 10).get_pixel(3, 3));
        assert_eq!(canvas(&img, 0, 0, [0; 4]).dimensions(), (1, 1));
    }

    #[test]
    fn resize_exact_clamps() {
        let img = gradient(10, 10);
        assert_eq!(resize_exact(&img, 3, 7).dimensions(), (3, 7));
        assert_eq!(resize_exact(&img, 0, 0).dimensions(), (1, 1));
    }

    #[test]
    fn grid_layout() {
        assert!(grid(&[], 2, 4, &[]).is_none());
        let imgs = vec![solid(10, 10, [255, 0, 0, 255]), solid(10, 10, [0, 255, 0, 255]), solid(10, 10, [0, 0, 255, 255])];
        let g = grid(&imgs, 2, 4, &[]).unwrap();
        // 2 columns, 2 rows of 10x10 cells with 4-pixel gaps.
        assert_eq!(g.dimensions(), (2 * 10 + 3 * 4, 2 * 10 + 3 * 4));
        assert_eq!(g.get_pixel(4, 4).0, [255, 0, 0, 255]);
        assert_eq!(g.get_pixel(18, 4).0, [0, 255, 0, 255]);
        assert_eq!(g.get_pixel(4, 18).0, [0, 0, 255, 255]);
        assert_eq!(g.get_pixel(18, 18).0, [255, 255, 255, 255], "empty cell");
        // More columns than pictures collapses to one row.
        assert_eq!(grid(&imgs, 10, 0, &[]).unwrap().dimensions(), (30, 10));
        // Labels add a 44-pixel band above each row.
        let labelled = grid(&imgs, 3, 0, &["a", "b", "c"]).unwrap();
        assert_eq!(labelled.dimensions(), (30, 54));
        assert_eq!(labelled.get_pixel(5, 50).0, [255, 0, 0, 255]);
    }
}
