//! Every font installed on the computer, for text and callouts.
//! The list is read once in the background; a font is loaded the first time it is used.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

#[derive(Default, Clone)]
pub struct Family {
    pub regular: Option<PathBuf>,
    pub bold: Option<PathBuf>,
    pub italic: Option<PathBuf>,
    pub bold_italic: Option<PathBuf>,
}

impl Family {
    pub fn pick(&self, bold: bool, italic: bool) -> Option<&PathBuf> {
        let want = match (bold, italic) {
            (false, false) => &self.regular,
            (true, false) => &self.bold,
            (false, true) => &self.italic,
            (true, true) => &self.bold_italic,
        };
        want.as_ref()
            .or(self.regular.as_ref())
            .or(self.bold.as_ref())
            .or(self.italic.as_ref())
            .or(self.bold_italic.as_ref())
    }
}

fn library() -> &'static RwLock<Option<BTreeMap<String, Family>>> {
    static L: OnceLock<RwLock<Option<BTreeMap<String, Family>>>> = OnceLock::new();
    L.get_or_init(|| RwLock::new(None))
}

/// The installed font families, once the background scan has finished.
pub fn families() -> Option<Vec<String>> {
    library().read().ok()?.as_ref().map(|m| m.keys().cloned().collect())
}

pub fn path_of(family: &str, bold: bool, italic: bool) -> Option<PathBuf> {
    library().read().ok()?.as_ref()?.get(family)?.pick(bold, italic).cloned()
}

fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(w) = std::env::var_os("WINDIR") {
        dirs.push(PathBuf::from(w).join("Fonts"));
    }
    if let Some(l) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(l).join("Microsoft").join("Windows").join("Fonts"));
    }
    if let Some(h) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(&h).join(".fonts"));
        dirs.push(PathBuf::from(&h).join(".local/share/fonts"));
        dirs.push(PathBuf::from(&h).join("Library/Fonts"));
    }
    dirs.push(PathBuf::from("/usr/share/fonts"));
    dirs.push(PathBuf::from("/Library/Fonts"));
    dirs.push(PathBuf::from("/System/Library/Fonts"));
    dirs
}

fn english_name(face: &ttf_parser::Face, id: u16) -> Option<String> {
    let names = face.names();
    let mut fallback = None;
    for n in names {
        if n.name_id != id {
            continue;
        }
        if let Some(s) = n.to_string() {
            if n.language_id == 0x409 {
                return Some(s);
            }
            fallback.get_or_insert(s);
        }
    }
    fallback
}

/// Read every font file's names in the background.
pub fn start_scan(ctx: eframe::egui::Context) {
    std::thread::spawn(move || {
        scan_now();
        ctx.request_repaint();
    });
}

/// What one font file says about itself (cached so files aren't re-read every start).
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct FontFacts {
    size: u64,
    modified: u64,
    /// None when the file isn't a usable font.
    family: Option<String>,
    bold: bool,
    italic: bool,
    plain: bool,
}

fn cache_path() -> Option<PathBuf> {
    cache_path_from(std::env::var_os("APPDATA"), std::env::var_os("HOME"))
}

/// The font cache for these APPDATA and HOME values (APPDATA wins).
fn cache_path_from(appdata: Option<std::ffi::OsString>, home: Option<std::ffi::OsString>) -> Option<PathBuf> {
    let base = appdata.map(PathBuf::from).or_else(|| home.map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("Snagster").join("fonts-cache.json"))
}

fn read_facts(p: &std::path::Path, size: u64, modified: u64) -> FontFacts {
    let none = FontFacts { size, modified, family: None, bold: false, italic: false, plain: false };
    let Ok(data) = std::fs::read(p) else { return none };
    let Ok(face) = ttf_parser::Face::parse(&data, 0) else { return none };
    let Some(family) = english_name(&face, 16).or_else(|| english_name(&face, 1)) else { return none };
    if family.starts_with('.') || family.trim().is_empty() {
        return none;
    }
    let style = english_name(&face, 17).or_else(|| english_name(&face, 2)).unwrap_or_default().to_lowercase();
    FontFacts {
        size,
        modified,
        family: Some(family),
        bold: face.is_bold() || style.contains("bold"),
        italic: face.is_italic() || style.contains("italic") || style.contains("oblique"),
        plain: style == "regular" || style == "normal" || style == "roman" || style.is_empty() || style == "book",
    }
}

/// Read the installed fonts now (on this thread). Files seen before (same size and date) come
/// from a small cache, so only new or changed fonts are opened.
pub fn scan_now() {
    let cache_file = cache_path();
    let old: HashMap<String, FontFacts> = cache_file
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let mut fresh: HashMap<String, FontFacts> = HashMap::new();
    let mut map: BTreeMap<String, Family> = BTreeMap::new();
    let mut stack = font_dirs();
    let mut seen = 0;
    let mut opened = 0;
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                if seen < 20000 {
                    stack.push(p);
                }
                continue;
            }
            let ext = p.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
            if !matches!(ext.as_str(), "ttf" | "otf" | "ttc") {
                continue;
            }
            seen += 1;
            let size = md.len();
            let modified = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let key = p.display().to_string();
            let facts = match old.get(&key) {
                Some(f) if f.size == size && f.modified == modified => f.clone(),
                _ => {
                    opened += 1;
                    read_facts(&p, size, modified)
                }
            };
            if let Some(family) = &facts.family {
                let entry = map.entry(family.clone()).or_default();
                let slot = match (facts.bold, facts.italic) {
                    (false, false) => &mut entry.regular,
                    (true, false) => &mut entry.bold,
                    (false, true) => &mut entry.italic,
                    (true, true) => &mut entry.bold_italic,
                };
                // Prefer the plain style over light/semibold variants for the same slot.
                if slot.is_none() || facts.plain {
                    *slot = Some(p.clone());
                }
            }
            fresh.insert(key, facts);
        }
    }
    if (opened > 0 || fresh.len() != old.len())
        && let Some(cf) = cache_file
    {
        if let Some(dir) = cf.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = serde_json::to_string(&fresh) {
            let _ = std::fs::write(cf, s);
        }
    }
    if let Ok(mut l) = library().write() {
        *l = Some(map);
    }
}

/// The egui family name for a font choice.
pub fn key(family: &str, bold: bool, italic: bool) -> String {
    format!("{family}#{}{}", if bold { "b" } else { "" }, if italic { "i" } else { "" })
}

/// Keeps track of the fonts handed to egui.
pub struct UiFonts {
    defs: eframe::egui::FontDefinitions,
    loaded: HashSet<String>,
    failed: HashSet<String>,
    /// The fonts loaded for text marks, kept so the base fonts can be rebuilt.
    user: Vec<(String, Arc<eframe::egui::FontData>)>,
    /// Whether the base includes the computer's Chinese, Japanese and Korean fonts, and for which language.
    pub base: Option<(bool, String)>,
}

impl Default for UiFonts {
    fn default() -> Self {
        Self { defs: crate::look::base_fonts(false, "en"), loaded: HashSet::new(), failed: HashSet::new(), user: Vec::new(), base: None }
    }
}

impl UiFonts {
    /// Make sure the interface fonts suit `lang` (and include CJK fonts when `cjk`), keeping
    /// the fonts loaded for text marks. Cheap when nothing changes.
    pub fn ensure_base(&mut self, ctx: &eframe::egui::Context, lang: &str, cjk: bool) {
        use eframe::egui::FontFamily;
        let want = (cjk, lang.to_string());
        if self.base.as_ref() == Some(&want) {
            return;
        }
        let mut defs = crate::look::base_fonts(cjk, lang);
        let base = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
        for (k, data) in &self.user {
            defs.font_data.insert(k.clone(), data.clone());
            let mut list = vec![k.clone()];
            list.extend(base.iter().cloned());
            defs.families.insert(FontFamily::Name(k.clone().into()), list);
        }
        self.defs = defs;
        self.base = Some(want);
        ctx.set_fonts(self.defs.clone());
        ctx.request_repaint();
    }

    /// The egui font family for this choice, loading it if needed (it shows from the next frame).
    pub fn family(&mut self, ctx: &eframe::egui::Context, family: &str, bold: bool, italic: bool) -> eframe::egui::FontFamily {
        use eframe::egui::{FontData, FontFamily};
        if family.is_empty() {
            return FontFamily::Proportional;
        }
        let k = key(family, bold, italic);
        if self.loaded.contains(&k) {
            return FontFamily::Name(k.into());
        }
        if self.failed.contains(&k) {
            return FontFamily::Proportional;
        }
        let Some(path) = path_of(family, bold, italic) else {
            if families().is_some() {
                self.failed.insert(k);
            }
            return FontFamily::Proportional;
        };
        match std::fs::read(&path) {
            Ok(bytes) if ttf_parser::Face::parse(&bytes, 0).is_ok() => {
                let data = Arc::new(FontData::from_owned(bytes));
                self.defs.font_data.insert(k.clone(), data.clone());
                self.user.push((k.clone(), data));
                let mut list = vec![k.clone()];
                if let Some(base) = self.defs.families.get(&FontFamily::Proportional) {
                    list.extend(base.iter().cloned());
                }
                self.defs.families.insert(FontFamily::Name(k.clone().into()), list);
                ctx.set_fonts(self.defs.clone());
                self.loaded.insert(k);
                ctx.request_repaint();
            }
            _ => {
                self.failed.insert(k);
            }
        }
        FontFamily::Proportional
    }
}

/// The same font for drawing into saved pictures.
pub fn export_font(family: &str, bold: bool, italic: bool) -> Option<ab_glyph::FontArc> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<ab_glyph::FontArc>>>> = OnceLock::new();
    if family.is_empty() {
        return None;
    }
    let k = key(family, bold, italic);
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(v) = cache.lock().ok()?.get(&k) {
        return v.clone();
    }
    let font = path_of(family, bold, italic)
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| ab_glyph::FontVec::try_from_vec(b).ok())
        .map(ab_glyph::FontArc::new);
    cache.lock().ok()?.insert(k, font.clone());
    font
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn p(s: &str) -> Option<PathBuf> {
        Some(PathBuf::from(s))
    }

    fn font_file() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/fonts/scripts/NotoSansHebrew-Regular.ttf")
    }

    #[test]
    fn family_picks_the_asked_style() {
        let f = Family { regular: p("r"), bold: p("b"), italic: p("i"), bold_italic: p("bi") };
        assert_eq!(f.pick(false, false), p("r").as_ref());
        assert_eq!(f.pick(true, false), p("b").as_ref());
        assert_eq!(f.pick(false, true), p("i").as_ref());
        assert_eq!(f.pick(true, true), p("bi").as_ref());
    }

    #[test]
    fn family_falls_back_to_what_it_has() {
        let only_bold = Family { bold: p("b"), ..Default::default() };
        for (b, i) in [(false, false), (true, false), (false, true), (true, true)] {
            assert_eq!(only_bold.pick(b, i), p("b").as_ref());
        }
        let regular_and_italic = Family { regular: p("r"), italic: p("i"), ..Default::default() };
        assert_eq!(regular_and_italic.pick(true, true), p("r").as_ref(), "regular comes first");
        assert_eq!(regular_and_italic.pick(true, false), p("r").as_ref());
        assert_eq!(regular_and_italic.pick(false, true), p("i").as_ref());
        let only_bi = Family { bold_italic: p("bi"), ..Default::default() };
        assert_eq!(only_bi.pick(false, false), p("bi").as_ref());
        assert_eq!(Family::default().pick(false, false), None);
    }

    #[test]
    fn keys_name_each_style_apart() {
        assert_eq!(key("Arial", false, false), "Arial#");
        assert_eq!(key("Arial", true, false), "Arial#b");
        assert_eq!(key("Arial", false, true), "Arial#i");
        assert_eq!(key("Arial", true, true), "Arial#bi");
        assert_ne!(key("A", true, false), key("A", false, true));
    }

    #[test]
    fn cache_path_prefers_appdata_then_home() {
        let c = cache_path_from(Some(OsString::from("ad")), Some(OsString::from("h"))).unwrap();
        assert_eq!(c, PathBuf::from("ad").join("Snagster").join("fonts-cache.json"));
        let c = cache_path_from(None, Some(OsString::from("h"))).unwrap();
        assert_eq!(c, PathBuf::from("h").join(".config").join("Snagster").join("fonts-cache.json"));
        assert!(cache_path_from(None, None).is_none());
    }

    #[test]
    fn a_real_font_file_is_read() {
        let f = read_facts(&font_file(), 123, 456);
        assert_eq!((f.size, f.modified), (123, 456));
        let family = f.family.expect("the built-in Hebrew font has a family name");
        assert!(family.starts_with("Noto Sans"), "{family}");
        assert!(!f.bold && !f.italic);
        assert!(f.plain);
    }

    #[test]
    fn files_that_are_not_fonts_have_no_family() {
        let dir = std::env::temp_dir().join(format!("snagster-fonts-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let junk = dir.join("junk.ttf");
        std::fs::write(&junk, b"definitely not a font").unwrap();
        let f = read_facts(&junk, 1, 2);
        assert!(f.family.is_none() && !f.bold && !f.italic && !f.plain);
        assert_eq!((f.size, f.modified), (1, 2));
        assert!(read_facts(&dir.join("missing.ttf"), 0, 0).family.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn font_facts_survive_the_cache() {
        let f = read_facts(&font_file(), 9, 8);
        let mut m = HashMap::new();
        m.insert("k".to_string(), f.clone());
        let back: HashMap<String, FontFacts> = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        let g = &back["k"];
        assert_eq!((g.size, g.modified, &g.family, g.bold, g.italic, g.plain), (f.size, f.modified, &f.family, f.bold, f.italic, f.plain));
    }

    #[test]
    fn font_folders_include_the_usual_places() {
        let d = font_dirs();
        assert!(d.contains(&PathBuf::from("/usr/share/fonts")));
        assert!(d.contains(&PathBuf::from("/System/Library/Fonts")));
    }

    #[test]
    fn no_family_means_the_interface_font() {
        assert!(export_font("", false, false).is_none());
        assert!(path_of("No Such Font Family 1234", false, false).is_none());
        assert!(export_font("No Such Font Family 1234", true, true).is_none());
        let ctx = eframe::egui::Context::default();
        let mut ui = UiFonts::default();
        assert_eq!(ui.family(&ctx, "", false, false), eframe::egui::FontFamily::Proportional);
        assert_eq!(ui.family(&ctx, "No Such Font Family 1234", false, false), eframe::egui::FontFamily::Proportional);
    }

    #[test]
    fn base_fonts_are_set_once_per_language() {
        let ctx = eframe::egui::Context::default();
        let mut ui = UiFonts::default();
        assert!(ui.base.is_none());
        ui.ensure_base(&ctx, "de", false);
        assert_eq!(ui.base, Some((false, "de".to_string())));
        ui.ensure_base(&ctx, "de", false);
        assert_eq!(ui.base, Some((false, "de".to_string())));
        ui.ensure_base(&ctx, "fr", false);
        assert_eq!(ui.base, Some((false, "fr".to_string())));
    }
}
