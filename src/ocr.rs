//! Reading text in pictures with the OCR engine built into Windows, and the features built
//! on it: grab text, smart redact, simplify.

use eframe::egui::{Rect, pos2, vec2};
use image::RgbaImage;

pub struct Word {
    pub text: String,
    pub rect: Rect,
}

pub struct Line {
    pub text: String,
    pub words: Vec<Word>,
}

impl Line {
    pub fn rect(&self) -> Rect {
        self.words.iter().map(|w| w.rect).reduce(|a, b| a.union(b)).unwrap_or(Rect::NOTHING)
    }
}

/// Read the text in a picture, with where each word is (in the picture's pixels).
#[cfg(windows)]
pub fn recognize(img: &RgbaImage) -> Result<Vec<Line>, String> {
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::DataWriter;

    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|e| format!("Windows text recognition isn't available: {e}"))?;
    let max = OcrEngine::MaxImageDimension().unwrap_or(10000).max(1000);
    // Small text reads better a bit larger; very large pictures must shrink.
    let (w, h) = img.dimensions();
    let mut scale = if h < 900 && w < 2000 { 2.0f32 } else { 1.0 };
    let longest = (w.max(h) as f32 * scale) as u32;
    if longest > max {
        scale *= max as f32 / longest as f32;
    }
    let work = if (scale - 1.0).abs() > 0.01 {
        image::imageops::resize(
            img,
            ((w as f32 * scale) as u32).max(1),
            ((h as f32 * scale) as u32).max(1),
            image::imageops::FilterType::CatmullRom,
        )
    } else {
        img.clone()
    };
    let mut bgra = work.as_raw().clone();
    for p in bgra.chunks_exact_mut(4) {
        p.swap(0, 2);
        p[3] = 255;
    }
    let writer = DataWriter::new().map_err(|e| e.to_string())?;
    writer.WriteBytes(&bgra).map_err(|e| e.to_string())?;
    let buffer = writer.DetachBuffer().map_err(|e| e.to_string())?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, work.width() as i32, work.height() as i32)
        .map_err(|e| e.to_string())?;
    let result = engine.RecognizeAsync(&bitmap).map_err(|e| e.to_string())?.join().map_err(|e| e.to_string())?;
    let mut lines = Vec::new();
    for line in result.Lines().map_err(|e| e.to_string())? {
        let mut words = Vec::new();
        for word in line.Words().map_err(|e| e.to_string())? {
            let r = word.BoundingRect().map_err(|e| e.to_string())?;
            let text = word.Text().map(|t| t.to_string()).unwrap_or_default();
            words.push(Word {
                text,
                rect: Rect::from_min_size(pos2(r.X / scale, r.Y / scale), vec2(r.Width / scale, r.Height / scale)),
            });
        }
        let text = line.Text().map(|t| t.to_string()).unwrap_or_default();
        lines.push(Line { text, words });
    }
    Ok(lines)
}

#[cfg(not(windows))]
pub fn recognize(_img: &RgbaImage) -> Result<Vec<Line>, String> {
    Err("Text recognition uses the engine built into Windows.".into())
}

/// All the text, line by line.
pub fn plain_text(lines: &[Line]) -> String {
    lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
}

#[derive(Clone, Copy)]
pub struct RedactKinds {
    pub emails: bool,
    pub phones: bool,
    pub cards: bool,
    pub addresses: bool,
    pub ips: bool,
    pub urls: bool,
}

/// What looks sensitive: areas to blur, and a short note of what they were.
pub fn find_sensitive(lines: &[Line], k: RedactKinds) -> Vec<(Rect, &'static str)> {
    use regex::Regex;
    let mut patterns: Vec<(&'static str, Regex)> = Vec::new();
    let mut add = |on: bool, name: &'static str, re: &str| {
        if on && let Ok(r) = Regex::new(re) {
            patterns.push((name, r));
        }
    };
    add(k.emails, "email", r"(?i)[a-z0-9._%+\-]+@[a-z0-9.\-]+\.[a-z]{2,}");
    add(k.cards, "card number", r"\b(?:\d[ \-]?){13,19}\b");
    add(k.phones, "phone number", r"(?:\+\d{1,3}[ \-.]?)?(?:\(\d{1,4}\)[ \-.]?)?\d{2,4}[ \-.]?\d{2,4}[ \-.]?\d{2,4}\b");
    add(k.ips, "IP address", r"\b(?:\d{1,3}\.){3}\d{1,3}\b");
    add(
        k.addresses,
        "address",
        r"(?i)\b\d{1,5}\s+[a-zæøåäöü .'\-]{2,40}\s(?:street|st|road|rd|avenue|ave|lane|ln|drive|dr|blvd|boulevard|way|court|ct|place|pl|gate|gata|veien|vei|vegen|veg|gatan|strasse|straße)\b\.?|(?i)\b[a-zæøåäöü .'\-]{2,40}(?:gate|gata|veien|vei|vegen|veg|gatan|strasse|straße)\s+\d{1,5}[a-z]?\b",
    );
    add(k.urls, "web address", r"(?i)\b(?:https?://|www\.)\S+");
    let mut found = Vec::new();
    for line in lines {
        // Rebuild the line from its words so a match maps back to word boxes.
        let mut text = String::new();
        let mut spans = Vec::new();
        for w in &line.words {
            if !text.is_empty() {
                text.push(' ');
            }
            let start = text.len();
            text.push_str(&w.text);
            spans.push((start, text.len(), w.rect));
        }
        for (name, re) in &patterns {
            for m in re.find_iter(&text) {
                // Phone-like matches need enough digits.
                if *name == "phone number" && m.as_str().chars().filter(|c| c.is_ascii_digit()).count() < 7 {
                    continue;
                }
                let r = spans
                    .iter()
                    .filter(|(a, b, _)| *a < m.end() && *b > m.start())
                    .map(|s| s.2)
                    .reduce(|a, b| a.union(b));
                if let Some(r) = r
                    && !found.iter().any(|(f, _): &(Rect, &str)| f.contains_rect(r))
                {
                    found.push((r.expand(3.0), *name));
                }
            }
        }
    }
    found
}

/// Replace every line of text with a plain rounded bar in a soft colour taken from the text
/// itself, so the picture shows layout without the detail.
pub fn simplify(img: &RgbaImage, lines: &[Line]) -> RgbaImage {
    let mut out = img.clone();
    let mut pm = match crate::render::to_pixmap(&out) {
        Some(p) => p,
        None => return out,
    };
    for line in lines {
        for w in &line.words {
            let r = w.rect;
            if r.width() < 2.0 || r.height() < 2.0 {
                continue;
            }
            // Background: the pixel just outside the word; ink: the darkest-or-lightest inside.
            let bg = sample(img, r.min.x - 2.0, r.center().y);
            let ink = contrast_ink(img, r, bg);
            let mut paint = tiny_skia::Paint::default();
            paint.anti_alias = true;
            paint.set_color_rgba8(bg[0], bg[1], bg[2], 255);
            if let Some(rr) = tiny_skia::Rect::from_ltrb(r.min.x - 1.0, r.min.y - 1.0, r.max.x + 1.0, r.max.y + 1.0) {
                pm.fill_rect(rr, &paint, tiny_skia::Transform::identity(), None);
            }
            let bar = Rect::from_center_size(r.center(), vec2(r.width(), (r.height() * 0.45).max(3.0)));
            let soft = [
                ((ink[0] as u16 + bg[0] as u16 * 2) / 3) as u8,
                ((ink[1] as u16 + bg[1] as u16 * 2) / 3) as u8,
                ((ink[2] as u16 + bg[2] as u16 * 2) / 3) as u8,
            ];
            paint.set_color_rgba8(soft[0], soft[1], soft[2], 255);
            if let Some(p) = crate::render::rounded_rect(bar, bar.height() / 2.0) {
                pm.fill_path(&p, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
            }
        }
    }
    out = crate::render::from_pixmap(pm);
    out
}

fn sample(img: &RgbaImage, x: f32, y: f32) -> [u8; 3] {
    let x = (x.max(0.0) as u32).min(img.width().saturating_sub(1));
    let y = (y.max(0.0) as u32).min(img.height().saturating_sub(1));
    let p = img.get_pixel(x, y).0;
    [p[0], p[1], p[2]]
}

fn contrast_ink(img: &RgbaImage, r: Rect, bg: [u8; 3]) -> [u8; 3] {
    let lum = |c: [u8; 3]| c[0] as i32 * 3 + c[1] as i32 * 6 + c[2] as i32;
    let bl = lum(bg);
    let mut best = bg;
    let mut best_d = 0;
    let steps = 12;
    for i in 0..steps {
        for j in 0..4 {
            let x = r.min.x + r.width() * (i as f32 + 0.5) / steps as f32;
            let y = r.min.y + r.height() * (j as f32 + 0.5) / 4.0;
            let c = sample(img, x, y);
            let d = (lum(c) - bl).abs();
            if d > best_d {
                best_d = d;
                best = c;
            }
        }
    }
    best
}
