//! Reading text in pictures with the OCR engine built into Windows, and the features built
//! on it: grab text, smart redact, simplify.

#[cfg(any(windows, test))]
use eframe::egui::pos2;
use eframe::egui::{Rect, vec2};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RedactKinds {
    pub emails: bool,
    pub phones: bool,
    pub cards: bool,
    pub addresses: bool,
    pub ips: bool,
    pub urls: bool,
}

impl RedactKinds {
    /// The names `from_names` understands, in order.
    pub const NAMES: [&'static str; 6] = ["emails", "phones", "cards", "addresses", "ips", "urls"];

    /// What the person chose to hide (Settings > Redact).
    pub fn from_settings(s: &crate::settings::Settings) -> RedactKinds {
        RedactKinds { emails: s.redact_emails, phones: s.redact_phones, cards: s.redact_cards, addresses: s.redact_addresses, ips: s.redact_ips, urls: s.redact_urls }
    }

    /// Only the named kinds (see `NAMES`; singular names work too).
    pub fn from_names<S: AsRef<str>>(names: &[S]) -> Result<RedactKinds, String> {
        let mut k = RedactKinds { emails: false, phones: false, cards: false, addresses: false, ips: false, urls: false };
        for n in names {
            let n = n.as_ref().trim().to_ascii_lowercase();
            match n.as_str() {
                "emails" | "email" => k.emails = true,
                "phones" | "phone" => k.phones = true,
                "cards" | "card" => k.cards = true,
                "addresses" | "address" => k.addresses = true,
                "ips" | "ip" => k.ips = true,
                "urls" | "url" | "web" => k.urls = true,
                _ => return Err(format!("unknown kind {n}; use {}", Self::NAMES.join(", "))),
            }
        }
        Ok(k)
    }
}

/// Smart redact: pixelate everything `find_sensitive` finds (in `block`-pixel squares). The
/// new picture and what was hidden, or None when nothing was found.
pub fn redact(doc: &crate::doc::Doc, lines: &[Line], kinds: RedactKinds, block: u32) -> Option<(crate::doc::Doc, Vec<&'static str>)> {
    let found = find_sensitive(lines, kinds);
    if found.is_empty() {
        return None;
    }
    let mut d = doc.clone();
    for (r, _) in &found {
        if let Some(n) = d.blur(*r, block) {
            d = n;
        }
    }
    Some((d, found.into_iter().map(|f| f.1).collect()))
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
    // Before phone numbers: "192.168.10.254" also looks like one, and the first name found wins.
    add(k.ips, "IP address", r"\b(?:\d{1,3}\.){3}\d{1,3}\b");
    add(k.phones, "phone number", r"(?:\+\d{1,3}[ \-.]?)?(?:\(\d{1,4}\)[ \-.]?)?\d{2,4}[ \-.]?\d{2,4}[ \-.]?\d{2,4}\b");
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    const ALL: RedactKinds = RedactKinds { emails: true, phones: true, cards: true, addresses: true, ips: true, urls: true };
    const NONE: RedactKinds = RedactKinds { emails: false, phones: false, cards: false, addresses: false, ips: false, urls: false };

    /// A line laid out as words 10 px per character with a 10 px gap, like OCR gives back.
    fn line(text: &str, y: f32) -> Line {
        let mut x = 0.0;
        let words = text
            .split(' ')
            .map(|w| {
                let width = w.chars().count() as f32 * 10.0;
                let word = Word { text: w.into(), rect: Rect::from_min_size(pos2(x, y), vec2(width, 20.0)) };
                x += width + 10.0;
                word
            })
            .collect();
        Line { text: text.into(), words }
    }

    fn kinds(lines: &[Line], k: RedactKinds) -> Vec<&'static str> {
        find_sensitive(lines, k).into_iter().map(|f| f.1).collect()
    }

    #[test]
    fn line_rect_is_the_union_of_its_words() {
        let l = line("hello big world", 5.0);
        assert_eq!(l.rect(), Rect::from_min_max(pos2(0.0, 5.0), pos2(150.0, 25.0)));
        let empty = Line { text: String::new(), words: Vec::new() };
        assert_eq!(empty.rect(), Rect::NOTHING);
    }

    #[test]
    fn plain_text_joins_lines() {
        assert_eq!(plain_text(&[line("one two", 0.0), line("three", 30.0)]), "one two\nthree");
        assert_eq!(plain_text(&[]), "");
    }

    #[cfg(not(windows))]
    #[test]
    fn recognition_says_it_needs_windows() {
        let err = recognize(&RgbaImage::new(4, 4)).err().unwrap();
        assert!(err.contains("Windows"));
    }

    #[test]
    fn finds_an_email_and_covers_only_its_word() {
        let lines = [line("Mail me at test.person@example.com today", 0.0)];
        let found = find_sensitive(&lines, ALL);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1, "email");
        // "Mail me at " is 4+2+2 chars plus 3 gaps: the address starts at x = 110.
        let email = lines[0].words[3].rect;
        assert_eq!(found[0].0, email.expand(3.0));
    }

    #[test]
    fn finds_each_kind() {
        assert_eq!(kinds(&[line("card 4111 1111 1111 1111 ok", 0.0)], ALL), vec!["card number"]);
        // An IP address also looks like a phone number; it must be named as what it is.
        assert_eq!(kinds(&[line("server 192.168.10.254 down", 0.0)], ALL), vec!["IP address"]);
        assert_eq!(kinds(&[line("see https://example.com/x?y=1 now", 0.0)], ALL), vec!["web address"]);
        assert_eq!(kinds(&[line("see www.example.org", 0.0)], ALL), vec!["web address"]);
        assert_eq!(kinds(&[line("call +47 912 34 567 now", 0.0)], ALL), vec!["phone number"]);
        assert_eq!(kinds(&[line("ship to 221 Baker Street please", 0.0)], ALL), vec!["address"]);
        assert_eq!(kinds(&[line("bor i Storgata 12 i dag", 0.0)], ALL), vec!["address"]);
    }

    #[test]
    fn short_numbers_are_not_phone_numbers() {
        assert!(kinds(&[line("page 12 of 3456", 0.0)], ALL).is_empty());
        assert!(kinds(&[line("total 1234", 0.0)], ALL).is_empty());
    }

    #[test]
    fn switched_off_kinds_are_not_looked_for() {
        let lines = [line("a@b.com 10.0.0.1 https://x.io 4111111111111111", 0.0)];
        assert!(find_sensitive(&lines, NONE).is_empty());
        let only_ip = RedactKinds { ips: true, ..NONE };
        assert_eq!(kinds(&lines, only_ip), vec!["IP address"]);
        let only_mail = RedactKinds { emails: true, ..NONE };
        assert_eq!(kinds(&lines, only_mail), vec!["email"]);
    }

    #[test]
    fn a_match_inside_one_already_found_is_not_repeated() {
        // The web address holds an email-like part; one box is enough.
        let lines = [line("https://user@example.com/path", 0.0)];
        let found = find_sensitive(&lines, ALL);
        assert_eq!(found.len(), 1, "{:?}", found.iter().map(|f| f.1).collect::<Vec<_>>());
    }

    #[test]
    fn matches_across_words_cover_all_of_them() {
        let lines = [line("pay 4111 1111 1111 1111", 40.0)];
        let found = find_sensitive(&lines, RedactKinds { cards: true, ..NONE });
        assert_eq!(found.len(), 1);
        let want = lines[0].words[1].rect.union(lines[0].words[4].rect).expand(3.0);
        assert_eq!(found[0].0, want);
    }

    #[test]
    fn each_line_is_searched() {
        let lines = [line("first a@b.no", 0.0), line("nothing here", 30.0), line("10.1.2.3", 60.0)];
        assert_eq!(kinds(&lines, ALL), vec!["email", "IP address"]);
    }

    #[test]
    fn kinds_come_from_the_settings_or_by_name() {
        let s = crate::settings::Settings { redact_urls: true, redact_phones: false, ..Default::default() };
        let k = RedactKinds::from_settings(&s);
        assert!(k.urls && !k.phones && k.emails && k.cards && k.addresses && k.ips);
        assert_eq!(RedactKinds::from_names(&RedactKinds::NAMES).unwrap(), ALL);
        assert_eq!(RedactKinds::from_names::<&str>(&[]).unwrap(), NONE);
        assert_eq!(RedactKinds::from_names(&["Email", " url ", "address"]).unwrap(), RedactKinds { emails: true, urls: true, addresses: true, ..NONE });
        assert_eq!(RedactKinds::from_names(&["ip", "phone", "card"]).unwrap(), RedactKinds { ips: true, phones: true, cards: true, ..NONE });
        assert!(RedactKinds::from_names(&["names"]).unwrap_err().contains("unknown kind names"));
    }

    #[test]
    fn redact_pixelates_what_it_finds_in_blocks() {
        // A picture with a fine checkerboard; the "email" word sits over part of it.
        let img = RgbaImage::from_fn(300, 40, |x, y| if (x + y) % 2 == 0 { Rgba([0, 0, 0, 255]) } else { Rgba([255, 255, 255, 255]) });
        let d = crate::doc::Doc::new(img.clone());
        let lines = [line("mail a@b.no", 10.0)];
        assert!(redact(&d, &lines, NONE, 12).is_none(), "nothing asked for, nothing found");
        assert!(redact(&d, &[line("nothing here", 0.0)], ALL, 12).is_none());
        let (out, kinds) = redact(&d, &lines, ALL, 8).unwrap();
        assert_eq!(kinds, ["email"]);
        // The word box (x 50..120) is now flat in 8-pixel squares; outside it is unchanged.
        let p = |x, y| out.image.get_pixel(x, y).0;
        assert_eq!(p(56, 16), p(57, 17), "one block, one colour");
        assert_eq!(out.image.get_pixel(5, 5), img.get_pixel(5, 5));
        assert_eq!(out.image.get_pixel(250, 20), img.get_pixel(250, 20));
    }

    #[test]
    fn sample_clamps_to_the_picture() {
        let img = RgbaImage::from_fn(10, 8, |x, y| Rgba([x as u8, y as u8, 0, 255]));
        assert_eq!(sample(&img, 3.9, 2.1), [3, 2, 0]);
        assert_eq!(sample(&img, -4.0, -1.0), [0, 0, 0]);
        assert_eq!(sample(&img, 99.0, 99.0), [9, 7, 0]);
    }

    #[test]
    fn contrast_ink_picks_the_colour_furthest_from_the_background() {
        let mut img = RgbaImage::from_pixel(60, 20, Rgba([250, 250, 250, 255]));
        for y in 6..14 {
            for x in 10..50 {
                img.put_pixel(x, y, Rgba([20, 30, 40, 255]));
            }
        }
        let r = Rect::from_min_max(pos2(0.0, 0.0), pos2(60.0, 20.0));
        assert_eq!(contrast_ink(&img, r, [250, 250, 250]), [20, 30, 40]);
        // Nothing differs: the background itself.
        let plain = RgbaImage::from_pixel(10, 10, Rgba([9, 9, 9, 255]));
        assert_eq!(contrast_ink(&plain, Rect::from_min_max(pos2(0.0, 0.0), pos2(10.0, 10.0)), [9, 9, 9]), [9, 9, 9]);
    }

    #[test]
    fn simplify_replaces_words_with_bars() {
        // White picture with a black "word"; simplify paints over it in soft grey.
        let mut img = RgbaImage::from_pixel(80, 40, Rgba([255, 255, 255, 255]));
        for y in 12..28 {
            for x in 20..60 {
                if (x + y) % 3 == 0 {
                    img.put_pixel(x, y, Rgba([0, 0, 0, 255]));
                }
            }
        }
        let lines = [Line {
            text: "word".into(),
            words: vec![Word { text: "word".into(), rect: Rect::from_min_max(pos2(20.0, 12.0), pos2(60.0, 28.0)) }],
        }];
        let out = simplify(&img, &lines);
        assert_eq!(out.dimensions(), img.dimensions());
        // No pure black left; the middle is the soft bar colour (a third of the way to black).
        assert!(out.pixels().all(|p| p.0[0] > 60), "black left over");
        let mid = out.get_pixel(40, 20).0;
        assert!((150..=190).contains(&mid[0]), "{mid:?}");
        // Far from the word nothing changed.
        assert_eq!(out.get_pixel(2, 2).0, [255, 255, 255, 255]);
        // Tiny words are left alone.
        let tiny = [Line { text: "x".into(), words: vec![Word { text: "x".into(), rect: Rect::from_min_size(pos2(5.0, 5.0), vec2(1.0, 1.0)) }] }];
        assert_eq!(simplify(&img, &tiny), img);
    }
}
