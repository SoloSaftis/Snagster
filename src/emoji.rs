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
