//! Emojis in full colour, drawn from the computer's own emoji font (its colour layers and
//! gradients), with the built-in black-and-white emoji font as a fallback.

use image::RgbaImage;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tiny_skia as ts;

pub struct EmojiFont {
    data: &'static [u8],
    pub color: bool,
    pub groups: Vec<(&'static str, Vec<char>)>,
}

/// Ready-made badges and pointers people reach for most.
pub const STAMPS: &[char] = &[
    '✅', '❌', '⚠', 'ℹ', '❓', '❗', '⭐', '❤', '👍', '👎', '👉', '👆', '👇', '👈', '🔒', '🔓', '🔑', '🔥', '💡', '📌',
    '🎯', '✔', '✖', '➡', '⬅', '⬆', '⬇', '🆕', '🆗', '🆘', '🚫', '⛔', '💬', '📎', '🖱', '⌨', '🖥', '📱', '📧', '📁',
    '🗑', '🔍', '⚙', '🔔', '⏰', '✏', '📝', '📷', '🎉', '💯',
];

const BLOCKS: &[(&str, &[(u32, u32)])] = &[
    ("Faces", &[(0x1F600, 0x1F64F), (0x1F910, 0x1F92F), (0x1F970, 0x1F97F), (0x1F9D0, 0x1F9DF)]),
    ("People & body", &[(0x1F440, 0x1F4AA), (0x1F930, 0x1F94F), (0x1F9B0, 0x1F9CF), (0x1F9E0, 0x1F9FF)]),
    ("Nature & food", &[(0x1F330, 0x1F37F), (0x1F400, 0x1F43F), (0x1F950, 0x1F96F), (0x1F980, 0x1F9AF)]),
    ("Activities & things", &[(0x1F380, 0x1F3FA), (0x1F4AB, 0x1F5FF), (0x1F900, 0x1F90F)]),
    ("Travel & places", &[(0x1F300, 0x1F32F), (0x1F680, 0x1F6FF)]),
    ("Newest", &[(0x1FA70, 0x1FAFF)]),
    ("Symbols", &[
        (0x2600, 0x26FF),
        (0x2700, 0x27BF),
        (0x2190, 0x21FF),
        (0x2B00, 0x2BFF),
        (0x2300, 0x23FF),
        (0x25A0, 0x25FF),
        (0x203C, 0x203C),
        (0x2049, 0x2049),
        (0x2122, 0x2122),
        (0x2139, 0x2139),
        (0x3030, 0x3030),
        (0x303D, 0x303D),
        (0x3297, 0x3299),
        (0x1F004, 0x1F004),
        (0x1F0CF, 0x1F0CF),
        (0x1F170, 0x1F251),
    ]),
];

fn find_color_font() -> Option<Vec<u8>> {
    let mut candidates = Vec::new();
    if let Some(w) = std::env::var_os("WINDIR") {
        candidates.push(std::path::PathBuf::from(w).join("Fonts").join("seguiemj.ttf"));
    }
    candidates.push("/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf".into());
    candidates.push("/System/Library/Fonts/Apple Color Emoji.ttc".into());
    for c in candidates {
        if let Ok(d) = std::fs::read(&c)
            && let Ok(f) = ttf_parser::Face::parse(&d, 0)
            && f.tables().colr.is_some()
        {
            return Some(d);
        }
    }
    None
}

pub fn font() -> &'static EmojiFont {
    static F: OnceLock<EmojiFont> = OnceLock::new();
    F.get_or_init(|| {
        // The system's colour emoji font, unless it is missing or can't be read.
        let (data, color): (&'static [u8], bool) = match find_color_font().filter(|d| ttf_parser::Face::parse(d, 0).is_ok()) {
            Some(d) => (Box::leak(d.into_boxed_slice()), true),
            None => (epaint_default_fonts::NOTO_EMOJI_REGULAR, false),
        };
        let face = ttf_parser::Face::parse(data, 0).expect("the built-in emoji font is always readable");
        let mut groups = vec![("Stamps & badges", STAMPS.iter().copied().filter(|c| face.glyph_index(*c).is_some()).collect())];
        for (name, ranges) in BLOCKS {
            let mut v = Vec::new();
            for (a, b) in *ranges {
                for cp in *a..=*b {
                    let Some(ch) = char::from_u32(cp) else { continue };
                    // Skin-tone pieces aren't emojis on their own.
                    if (0x1F3FB..=0x1F3FF).contains(&cp) {
                        continue;
                    }
                    if let Some(g) = face.glyph_index(ch)
                        && (!color || face.is_color_glyph(g))
                    {
                        v.push(ch);
                    }
                }
            }
            if !v.is_empty() {
                groups.push((*name, v));
            }
        }
        EmojiFont { data, color, groups }
    })
}

impl EmojiFont {
    pub fn count(&self) -> usize {
        self.groups.iter().skip(1).map(|g| g.1.len()).sum()
    }
}

struct PathSink(ts::PathBuilder);

impl ttf_parser::OutlineBuilder for PathSink {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.quad_to(x1, y1, x, y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.cubic_to(x1, y1, x2, y2, x, y);
    }
    fn close(&mut self) {
        self.0.close();
    }
}

fn glyph_path(face: &ttf_parser::Face, g: ttf_parser::GlyphId, t: ts::Transform) -> Option<ts::Path> {
    let mut sink = PathSink(ts::PathBuilder::new());
    face.outline_glyph(g, &mut sink)?;
    sink.0.finish()?.transform(t)
}

struct Painter<'a> {
    face: &'a ttf_parser::Face<'a>,
    pm: ts::Pixmap,
    transforms: Vec<ts::Transform>,
    outline: Option<ts::Path>,
    clips: Vec<ts::Mask>,
}

impl Painter<'_> {
    fn current(&self) -> ts::Transform {
        *self.transforms.last().unwrap()
    }

    fn push_mask(&mut self, path: Option<ts::Path>) {
        let (w, h) = (self.pm.width(), self.pm.height());
        let mut m = ts::Mask::new(w, h).unwrap();
        if let Some(p) = path {
            m.fill_path(&p, ts::FillRule::Winding, true, ts::Transform::identity());
        }
        if let Some(top) = self.clips.last() {
            for (a, b) in m.data_mut().iter_mut().zip(top.data()) {
                *a = ((*a as u16 * *b as u16) / 255) as u8;
            }
        }
        self.clips.push(m);
    }

    fn stops(it: impl Iterator<Item = ttf_parser::colr::ColorStop>) -> Vec<(f32, ts::Color)> {
        let mut v: Vec<(f32, ts::Color)> = it
            .map(|s| (s.stop_offset.clamp(0.0, 1.0), ts::Color::from_rgba8(s.color.red, s.color.green, s.color.blue, s.color.alpha)))
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v
    }

    fn shader(stops: Vec<(f32, ts::Color)>, make: impl FnOnce(Vec<ts::GradientStop>) -> Option<ts::Shader<'static>>) -> ts::Shader<'static> {
        match stops.len() {
            0 => ts::Shader::SolidColor(ts::Color::BLACK),
            1 => ts::Shader::SolidColor(stops[0].1),
            _ => {
                let fallback = stops[stops.len() / 2].1;
                make(stops.into_iter().map(|(o, c)| ts::GradientStop::new(o, c)).collect()).unwrap_or(ts::Shader::SolidColor(fallback))
            }
        }
    }

    fn spread(e: ttf_parser::colr::GradientExtend) -> ts::SpreadMode {
        match e {
            ttf_parser::colr::GradientExtend::Pad => ts::SpreadMode::Pad,
            ttf_parser::colr::GradientExtend::Repeat => ts::SpreadMode::Repeat,
            ttf_parser::colr::GradientExtend::Reflect => ts::SpreadMode::Reflect,
        }
    }
}

impl<'a> ttf_parser::colr::Painter<'a> for Painter<'a> {
    fn outline_glyph(&mut self, glyph_id: ttf_parser::GlyphId) {
        self.outline = glyph_path(self.face, glyph_id, self.current());
    }

    fn paint(&mut self, paint: ttf_parser::colr::Paint<'a>) {
        use ttf_parser::colr::Paint;
        let t = self.current();
        let shader = match &paint {
            Paint::Solid(c) => ts::Shader::SolidColor(ts::Color::from_rgba8(c.red, c.green, c.blue, c.alpha)),
            Paint::LinearGradient(g) => Self::shader(Self::stops(g.stops(0, &[])), |s| {
                ts::LinearGradient::new(ts::Point::from_xy(g.x0, g.y0), ts::Point::from_xy(g.x1, g.y1), s, Self::spread(g.extend), t)
            }),
            Paint::RadialGradient(g) => Self::shader(Self::stops(g.stops(0, &[])), |s| {
                ts::RadialGradient::new(
                    ts::Point::from_xy(g.x0, g.y0),
                    g.r0.max(0.0),
                    ts::Point::from_xy(g.x1, g.y1),
                    g.r1.max(0.0),
                    s,
                    Self::spread(g.extend),
                    t,
                )
            }),
            Paint::SweepGradient(g) => {
                // Sweeps are rare in emoji fonts; use the middle colour.
                let s = Self::stops(g.stops(0, &[]));
                ts::Shader::SolidColor(s.get(s.len() / 2).map(|x| x.1).unwrap_or(ts::Color::BLACK))
            }
        };
        let p = ts::Paint { shader, anti_alias: true, ..Default::default() };
        let (w, h) = (self.pm.width() as f32, self.pm.height() as f32);
        if let Some(mask) = self.clips.last() {
            let mask = mask.clone();
            if let Some(r) = ts::Rect::from_xywh(0.0, 0.0, w, h) {
                self.pm.fill_rect(r, &p, ts::Transform::identity(), Some(&mask));
            }
        } else if let Some(o) = &self.outline {
            self.pm.fill_path(o, &p, ts::FillRule::Winding, ts::Transform::identity(), None);
        }
    }

    fn push_clip(&mut self) {
        let o = self.outline.clone();
        self.push_mask(o);
    }

    fn push_clip_box(&mut self, b: ttf_parser::colr::ClipBox) {
        let path = ts::Rect::from_ltrb(b.x_min, b.y_min, b.x_max, b.y_max)
            .map(ts::PathBuilder::from_rect)
            .and_then(|p| p.transform(self.current()));
        self.push_mask(path);
    }

    fn pop_clip(&mut self) {
        self.clips.pop();
    }

    fn push_layer(&mut self, _mode: ttf_parser::colr::CompositeMode) {}

    fn pop_layer(&mut self) {}

    fn push_transform(&mut self, t: ttf_parser::Transform) {
        let next = self.current().pre_concat(ts::Transform::from_row(t.a, t.b, t.c, t.d, t.e, t.f));
        self.transforms.push(next);
    }

    fn pop_transform(&mut self) {
        if self.transforms.len() > 1 {
            self.transforms.pop();
        }
    }
}

/// Draw one emoji into a square picture `px` pixels wide. `tint` colours a black-and-white
/// emoji (ignored for colour ones).
pub fn render(ch: char, px: u32, tint: [u8; 4]) -> Option<Arc<RgbaImage>> {
    static CACHE: OnceLock<Mutex<HashMap<(char, u32, [u8; 4]), Option<Arc<RgbaImage>>>>> = OnceLock::new();
    let px = px.clamp(8, 1024);
    let ef = font();
    let tint = if ef.color { [0, 0, 0, 255] } else { tint };
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(v) = cache.lock().ok()?.get(&(ch, px, tint)) {
        return v.clone();
    }
    let out = draw(ef, ch, px, tint).map(Arc::new);
    if let Ok(mut c) = cache.lock() {
        if c.len() > 4000 {
            c.clear();
        }
        c.insert((ch, px, tint), out.clone());
    }
    out
}

fn draw(ef: &EmojiFont, ch: char, px: u32, tint: [u8; 4]) -> Option<RgbaImage> {
    let face = ttf_parser::Face::parse(ef.data, 0).ok()?;
    let g = face.glyph_index(ch)?;
    let asc = face.ascender() as f32;
    let desc = face.descender() as f32;
    let height = (asc - desc).max(1.0);
    let adv = face.glyph_hor_advance(g).unwrap_or(face.units_per_em()) as f32;
    let s = px as f32 / height.max(adv);
    let ox = (px as f32 - adv * s) / 2.0;
    let oy = (px as f32 - height * s) / 2.0 + asc * s;
    let base = ts::Transform::from_row(s, 0.0, 0.0, -s, ox, oy);
    let mut pm = ts::Pixmap::new(px, px)?;
    if ef.color && face.is_color_glyph(g) {
        let mut painter = Painter { face: &face, pm, transforms: vec![base], outline: None, clips: Vec::new() };
        face.paint_color_glyph(g, 0, ttf_parser::RgbaColor::new(0, 0, 0, 255), &mut painter)?;
        pm = painter.pm;
    } else {
        let path = glyph_path(&face, g, base)?;
        let mut paint = ts::Paint::default();
        paint.set_color_rgba8(tint[0], tint[1], tint[2], tint[3]);
        paint.anti_alias = true;
        pm.fill_path(&path, &paint, ts::FillRule::Winding, ts::Transform::identity(), None);
    }
    Some(crate::render::from_pixmap(pm))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ttf_parser::RgbaColor;
    use ttf_parser::colr::{ColorStop, GradientExtend};

    /// The built-in black-and-white font, the same on every system.
    fn builtin() -> EmojiFont {
        EmojiFont { data: epaint_default_fonts::NOTO_EMOJI_REGULAR, color: false, groups: Vec::new() }
    }

    /// A stamp the built-in font can draw.
    fn drawable() -> char {
        let face = ttf_parser::Face::parse(epaint_default_fonts::NOTO_EMOJI_REGULAR, 0).unwrap();
        *STAMPS.iter().find(|c| face.glyph_index(**c).is_some()).expect("the built-in font has some stamps")
    }

    #[test]
    fn stamps_are_unique() {
        let mut v = STAMPS.to_vec();
        v.sort();
        v.dedup();
        assert_eq!(v.len(), STAMPS.len());
    }

    #[test]
    fn blocks_are_valid_ranges() {
        for (name, ranges) in BLOCKS {
            assert!(!name.is_empty());
            for (a, b) in *ranges {
                assert!(a <= b, "{name}: {a:x} > {b:x}");
                assert!(char::from_u32(*a).is_some() && char::from_u32(*b).is_some());
            }
        }
    }

    #[test]
    fn font_groups_start_with_stamps() {
        // Uses whichever emoji font this computer has; the shape of the result is the same.
        let f = font();
        assert_eq!(f.groups[0].0, "Stamps & badges");
        assert!(f.groups.len() > 1, "at least one block of emojis");
        assert!(f.groups.iter().skip(1).all(|g| !g.1.is_empty()));
        assert_eq!(f.count(), f.groups.iter().skip(1).map(|g| g.1.len()).sum::<usize>());
        // Skin-tone pieces never show up on their own.
        assert!(f.groups.iter().flat_map(|g| g.1.iter()).all(|c| !(0x1F3FB..=0x1F3FF).contains(&(*c as u32))));
        assert!(std::ptr::eq(f, font()), "loaded once");
    }

    #[test]
    fn count_skips_the_stamps_group() {
        let ef = EmojiFont {
            data: epaint_default_fonts::NOTO_EMOJI_REGULAR,
            color: false,
            groups: vec![("Stamps & badges", vec!['a', 'b']), ("x", vec!['c']), ("y", vec!['d', 'e', 'f'])],
        };
        assert_eq!(ef.count(), 4);
        assert_eq!(builtin().count(), 0);
    }

    #[test]
    fn built_in_emoji_draws_in_the_tint() {
        let ch = drawable();
        let img = draw(&builtin(), ch, 48, [200, 30, 40, 255]).expect("drawn");
        assert_eq!(img.dimensions(), (48, 48));
        let inked: Vec<_> = img.pixels().filter(|p| p.0[3] > 0).collect();
        assert!(inked.len() > 48, "something is drawn");
        assert!(inked.len() < 48 * 48, "not a solid square");
        // Fully covered pixels have exactly the tint colour.
        for p in inked.iter().filter(|p| p.0[3] == 255) {
            assert_eq!(p.0, [200, 30, 40, 255]);
        }
        // The glyph sits inside the square, roughly centred.
        let (mut sx, mut n) = (0f32, 0f32);
        for (x, _, p) in img.enumerate_pixels() {
            if p.0[3] > 0 {
                sx += x as f32;
                n += 1.0;
            }
        }
        assert!((sx / n - 24.0).abs() < 12.0);
    }

    #[test]
    fn missing_glyphs_draw_nothing() {
        assert!(draw(&builtin(), '\u{FDD0}', 32, [0, 0, 0, 255]).is_none());
    }

    #[test]
    fn render_clamps_and_caches() {
        let ch = drawable();
        let a = render(ch, 1, [1, 2, 3, 255]);
        let b = render(ch, 1, [1, 2, 3, 255]);
        if let (Some(a), Some(b)) = (&a, &b) {
            assert_eq!(a.dimensions(), (8, 8), "never smaller than 8 pixels");
            assert!(Arc::ptr_eq(a, b), "the second call comes from the cache");
        }
        if let Some(big) = render(ch, 100_000, [1, 2, 3, 255]) {
            assert_eq!(big.dimensions(), (1024, 1024), "never bigger than 1024 pixels");
        }
        assert!(render('\u{FDD0}', 32, [0, 0, 0, 255]).is_none());
    }

    #[test]
    fn glyph_paths_follow_the_transform() {
        let face = ttf_parser::Face::parse(epaint_default_fonts::NOTO_EMOJI_REGULAR, 0).unwrap();
        let g = face.glyph_index(drawable()).unwrap();
        let plain = glyph_path(&face, g, ts::Transform::identity()).unwrap();
        let moved = glyph_path(&face, g, ts::Transform::from_translate(100.0, 50.0)).unwrap();
        let (a, b) = (plain.bounds(), moved.bounds());
        assert!((b.left() - a.left() - 100.0).abs() < 1e-2);
        assert!((b.top() - a.top() - 50.0).abs() < 1e-2);
        let half = glyph_path(&face, g, ts::Transform::from_scale(0.5, 0.5)).unwrap();
        assert!((half.bounds().width() - a.width() / 2.0).abs() < 1e-2);
        // The space glyph has no outline.
        if let Some(sp) = face.glyph_index(' ') {
            assert!(glyph_path(&face, sp, ts::Transform::identity()).is_none());
        }
    }

    fn stop(offset: f32, red: u8) -> ColorStop {
        ColorStop { stop_offset: offset, color: RgbaColor::new(red, 0, 0, 255) }
    }

    #[test]
    fn gradient_stops_are_sorted_and_clamped() {
        let v = Painter::stops([stop(0.9, 3), stop(-1.0, 1), stop(2.0, 4), stop(0.5, 2)].into_iter());
        let offsets: Vec<f32> = v.iter().map(|s| s.0).collect();
        assert_eq!(offsets, vec![0.0, 0.5, 0.9, 1.0]);
        let reds: Vec<u8> = v.iter().map(|s| s.1.to_color_u8().red()).collect();
        assert_eq!(reds, vec![1, 2, 3, 4]);
    }

    #[test]
    fn shaders_fall_back_to_solid_colours() {
        let black = Painter::shader(Vec::new(), |_| panic!("not needed"));
        assert!(matches!(black, ts::Shader::SolidColor(c) if c == ts::Color::BLACK));
        let red = ts::Color::from_rgba8(255, 0, 0, 255);
        let one = Painter::shader(vec![(0.0, red)], |_| panic!("not needed"));
        assert!(matches!(one, ts::Shader::SolidColor(c) if c == red));
        // When the gradient can't be made, the middle colour is used.
        let green = ts::Color::from_rgba8(0, 255, 0, 255);
        let blue = ts::Color::from_rgba8(0, 0, 255, 255);
        let failed = Painter::shader(vec![(0.0, red), (0.5, green), (1.0, blue)], |_| None);
        assert!(matches!(failed, ts::Shader::SolidColor(c) if c == green));
        let made = Painter::shader(vec![(0.0, red), (1.0, blue)], |s| {
            assert_eq!(s.len(), 2);
            ts::LinearGradient::new(ts::Point::from_xy(0.0, 0.0), ts::Point::from_xy(10.0, 0.0), s, ts::SpreadMode::Pad, ts::Transform::identity())
        });
        assert!(!matches!(made, ts::Shader::SolidColor(_)));
    }

    #[test]
    fn spread_modes_map_one_to_one() {
        assert_eq!(Painter::spread(GradientExtend::Pad), ts::SpreadMode::Pad);
        assert_eq!(Painter::spread(GradientExtend::Repeat), ts::SpreadMode::Repeat);
        assert_eq!(Painter::spread(GradientExtend::Reflect), ts::SpreadMode::Reflect);
    }

    #[test]
    fn clip_masks_multiply() {
        let face = ttf_parser::Face::parse(epaint_default_fonts::NOTO_EMOJI_REGULAR, 0).unwrap();
        let mut p = Painter { face: &face, pm: ts::Pixmap::new(10, 10).unwrap(), transforms: vec![ts::Transform::identity()], outline: None, clips: Vec::new() };
        let left = ts::PathBuilder::from_rect(ts::Rect::from_ltrb(0.0, 0.0, 6.0, 10.0).unwrap());
        let top = ts::PathBuilder::from_rect(ts::Rect::from_ltrb(0.0, 0.0, 10.0, 6.0).unwrap());
        p.push_mask(Some(left));
        p.push_mask(Some(top));
        let m = p.clips.last().unwrap();
        // Only the top-left square is inside both clips.
        assert_eq!(m.data()[2 * 10 + 2], 255);
        assert_eq!(m.data()[2 * 10 + 8], 0);
        assert_eq!(m.data()[8 * 10 + 2], 0);
        // No path means nothing is inside.
        p.push_mask(None);
        assert!(p.clips.last().unwrap().data().iter().all(|v| *v == 0));
        assert_eq!(p.clips.len(), 3);
        // The base transform is never popped.
        use ttf_parser::colr::Painter as _;
        p.pop_transform();
        p.pop_transform();
        assert_eq!(p.transforms.len(), 1);
        p.push_transform(ttf_parser::Transform { a: 2.0, b: 0.0, c: 0.0, d: 2.0, e: 1.0, f: 0.0 });
        assert_eq!(p.current().sx, 2.0);
        assert_eq!(p.current().tx, 1.0);
        p.pop_transform();
        assert_eq!(p.current(), ts::Transform::identity());
    }
}
