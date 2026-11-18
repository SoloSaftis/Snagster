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

/// The font cache, in Snagster's folder beside settings.json.
fn cache_path() -> Option<PathBuf> {
    Some(crate::userdirs::snagster_dir()?.join("fonts-cache.json"))
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
    /// The text size (percent) the interface fonts were made for.
    pub text_scale: u32,
}

impl Default for UiFonts {
    fn default() -> Self {
        let mut defs = crate::look::base_fonts(false, "en");
        interface_fonts(&mut defs, None, 100);
        Self { defs, loaded: HashSet::new(), failed: HashSet::new(), user: Vec::new(), base: None, text_scale: 100 }
    }
}

impl UiFonts {
    /// Make sure the interface fonts suit `lang` (and include CJK fonts when `cjk`) and the
    /// system's text size, keeping the fonts loaded for text marks. Cheap when nothing changes.
    pub fn ensure_base(&mut self, ctx: &eframe::egui::Context, lang: &str, cjk: bool) {
        watch_text_scale(ctx);
        self.ensure_base_sized(ctx, lang, cjk, current_text_scale());
    }

    fn ensure_base_sized(&mut self, ctx: &eframe::egui::Context, lang: &str, cjk: bool, text_scale: u32) {
        let want = (cjk, lang.to_string());
        if self.base.as_ref() == Some(&want) && self.text_scale == text_scale {
            return;
        }
        let mut defs = crate::look::base_fonts(cjk, lang);
        interface_fonts(&mut defs, system_ui_font(), text_scale);
        // Text marks fall back to the picture fonts, not the (system, resized) interface ones.
        let base = defs.families.get(&canvas_family()).cloned().unwrap_or_default();
        for (k, data) in &self.user {
            defs.font_data.insert(k.clone(), data.clone());
            let mut list = vec![k.clone()];
            list.extend(base.iter().cloned());
            defs.families.insert(eframe::egui::FontFamily::Name(k.clone().into()), list);
        }
        self.defs = defs;
        self.base = Some(want);
        self.text_scale = text_scale;
        ctx.set_fonts(self.defs.clone());
        ctx.request_repaint();
    }

    /// The egui font family for this choice, loading it if needed (it shows from the next frame).
    pub fn family(&mut self, ctx: &eframe::egui::Context, family: &str, bold: bool, italic: bool) -> eframe::egui::FontFamily {
        use eframe::egui::{FontData, FontFamily};
        if family.is_empty() {
            return canvas_family();
        }
        let k = key(family, bold, italic);
        if self.loaded.contains(&k) {
            return FontFamily::Name(k.into());
        }
        if self.failed.contains(&k) {
            return canvas_family();
        }
        let Some(path) = path_of(family, bold, italic) else {
            if families().is_some() {
                self.failed.insert(k);
            }
            return canvas_family();
        };
        match std::fs::read(&path) {
            Ok(bytes) if ttf_parser::Face::parse(&bytes, 0).is_ok() => {
                let data = Arc::new(FontData::from_owned(bytes));
                self.defs.font_data.insert(k.clone(), data.clone());
                self.user.push((k.clone(), data));
                let mut list = vec![k.clone()];
                if let Some(base) = self.defs.families.get(&canvas_family()) {
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
        canvas_family()
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


// ---------- the system's interface font and text size ----------

/// The egui family for text drawn into pictures (text and callout marks, step numbers) when no
/// font is chosen: egui's built-in font at its own size, the same one saved pictures use
/// (`render.rs`). The interface fonts may be the system's and bigger (Windows' text size); the
/// picture must not change with them.
pub fn canvas_family() -> eframe::egui::FontFamily {
    eframe::egui::FontFamily::Name("canvas".into())
}

/// The first Windows 11 build. Windows 11 still says "10.0" in its version number.
const WINDOWS_11_BUILD: u32 = 22000;

/// The file of Windows' interface font in the Fonts folder: Segoe UI Variable on Windows 11,
/// Segoe UI on Windows 10 (and on 11 if the variable font is missing). `has` says whether a
/// file is there. None when neither is.
pub fn windows_ui_font(build: Option<u32>, has: impl Fn(&str) -> bool) -> Option<&'static str> {
    if build.is_some_and(|b| b >= WINDOWS_11_BUILD) && has("SegUIVar.ttf") {
        return Some("SegUIVar.ttf");
    }
    has("segoeui.ttf").then_some("segoeui.ttf")
}

/// The text size in percent: Windows' "Text size" setting (Accessibility, 100 to 225) or the
/// `SNAGSTER_TEXT_SCALE` override (for testing), kept within what Windows allows. Neither set
/// is 100.
pub fn text_scale_percent(overridden: Option<&str>, setting: Option<u32>) -> u32 {
    overridden
        .and_then(|s| s.trim().trim_end_matches('%').parse::<u32>().ok())
        .or(setting)
        .unwrap_or(100)
        .clamp(100, 225)
}

/// The text size now, read at most every two seconds (the setting can change while Snagster
/// runs; reading it is a registry lookup).
fn current_text_scale() -> u32 {
    static LAST: Mutex<Option<(std::time::Instant, u32)>> = Mutex::new(None);
    let Ok(mut last) = LAST.lock() else { return 100 };
    if let Some((at, v)) = *last
        && at.elapsed() < std::time::Duration::from_secs(2)
    {
        return v;
    }
    let v = text_scale_percent(std::env::var("SNAGSTER_TEXT_SCALE").ok().as_deref(), sysui::text_scale_setting());
    *last = Some((std::time::Instant::now(), v));
    v
}

/// How big interface text is drawn (1.0 normal, up to 2.25), from Windows' Text size setting.
pub fn text_scale() -> f32 {
    current_text_scale() as f32 / 100.0
}

/// Redraw when Windows' Text size setting changes while Snagster sits idle (a frame reads it
/// again). A thread looks every two seconds; nothing where there is no such setting.
fn watch_text_scale(ctx: &eframe::egui::Context) {
    static STARTED: OnceLock<()> = OnceLock::new();
    if !cfg!(windows) {
        return;
    }
    STARTED.get_or_init(|| {
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new().name("text-size".into()).spawn(move || {
            let mut last = sysui::text_scale_setting();
            loop {
                std::thread::sleep(std::time::Duration::from_secs(2));
                let now = sysui::text_scale_setting();
                if now != last {
                    last = now;
                    ctx.request_repaint_after(std::time::Duration::from_millis(2100));
                }
            }
        });
    });
}

/// The system's interface font, read once (Windows; None elsewhere, where egui's font stays).
fn system_ui_font() -> Option<Arc<eframe::egui::FontData>> {
    static F: OnceLock<Option<Arc<eframe::egui::FontData>>> = OnceLock::new();
    F.get_or_init(|| {
        let dir = sysui::fonts_folder()?;
        let file = windows_ui_font(sysui::windows_build(), |f| dir.join(f).is_file())?;
        let bytes = std::fs::read(dir.join(file)).ok()?;
        ttf_parser::Face::parse(&bytes, 0).ok()?;
        Some(Arc::new(eframe::egui::FontData::from_owned(bytes)))
    })
    .clone()
}

/// Make `defs` (from `look::base_fonts`) the interface fonts: the system's font first when there
/// is one, and every text font `percent` big (the icons stay as they are). The family
/// `canvas_family` keeps the fonts as they were, for picture text.
pub fn interface_fonts(defs: &mut eframe::egui::FontDefinitions, system: Option<Arc<eframe::egui::FontData>>, percent: u32) {
    use eframe::egui::FontFamily;
    let canvas = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    defs.families.insert(canvas_family(), canvas);
    let scale = percent as f32 / 100.0;
    let sized = |defs: &mut eframe::egui::FontDefinitions, name: &str, data: Arc<eframe::egui::FontData>| -> String {
        if percent == 100 {
            defs.font_data.insert(name.to_string(), data);
            return name.to_string();
        }
        let key = format!("ui-{name}");
        if !defs.font_data.contains_key(&key) {
            let mut d = (*data).clone();
            d.tweak.scale *= scale;
            defs.font_data.insert(key.clone(), Arc::new(d));
        }
        key
    };
    for fam in [FontFamily::Proportional, FontFamily::Monospace] {
        let Some(old) = defs.families.get(&fam).cloned() else { continue };
        let mut list = Vec::with_capacity(old.len() + 1);
        if fam == FontFamily::Proportional
            && let Some(sys) = &system
        {
            list.push(sized(defs, "system-ui", sys.clone()));
        }
        for name in old {
            if name == "Phosphor" {
                list.push(name);
                continue;
            }
            match defs.font_data.get(&name).cloned() {
                Some(data) => list.push(sized(defs, &name, data)),
                None => list.push(name),
            }
        }
        // The icons stay right after the first font (see `look::base_fonts`).
        if let Some(i) = list.iter().position(|n| n == "Phosphor")
            && i != 1
            && list.len() > 1
        {
            let p = list.remove(i);
            list.insert(1, p);
        }
        defs.families.insert(fam, list);
    }
}

/// What the system says (Windows' registry); nothing elsewhere.
#[cfg(windows)]
mod sysui {
    use std::path::PathBuf;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::HSTRING;

    fn dword(root: HKEY, key: &str, name: &str) -> Option<u32> {
        let mut v = 0u32;
        let mut bytes = 4u32;
        let r = unsafe { RegGetValueW(root, &HSTRING::from(key), &HSTRING::from(name), RRF_RT_REG_DWORD, None, Some((&mut v as *mut u32).cast()), Some(&mut bytes)) };
        (r == ERROR_SUCCESS).then_some(v)
    }

    fn text(root: HKEY, key: &str, name: &str) -> Option<String> {
        let mut buf = vec![0u16; 64];
        let mut bytes = (buf.len() * 2) as u32;
        let r = unsafe { RegGetValueW(root, &HSTRING::from(key), &HSTRING::from(name), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut bytes)) };
        if r != ERROR_SUCCESS {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }

    /// Settings > Accessibility > Text size, in percent (absent until it is first changed).
    pub fn text_scale_setting() -> Option<u32> {
        dword(HKEY_CURRENT_USER, r"Software\Microsoft\Accessibility", "TextScaleFactor")
    }

    /// The Windows build number (22000 and up is Windows 11).
    pub fn windows_build() -> Option<u32> {
        text(HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "CurrentBuildNumber")?.trim().parse().ok()
    }

    pub fn fonts_folder() -> Option<PathBuf> {
        Some(PathBuf::from(std::env::var_os("WINDIR")?).join("Fonts"))
    }
}

/// Other systems keep egui's built-in interface font at its own size for now.
#[cfg(not(windows))]
mod sysui {
    use std::path::PathBuf;

    pub fn text_scale_setting() -> Option<u32> {
        None
    }

    pub fn windows_build() -> Option<u32> {
        None
    }

    pub fn fonts_folder() -> Option<PathBuf> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn the_cache_sits_beside_the_settings() {
        if let (Some(c), Some(s)) = (cache_path(), crate::settings::settings_path()) {
            assert_eq!(c.parent(), s.parent());
            assert_eq!(c.file_name().unwrap(), "fonts-cache.json");
        }
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
        assert_eq!(ui.family(&ctx, "", false, false), canvas_family());
        assert_eq!(ui.family(&ctx, "No Such Font Family 1234", false, false), canvas_family());
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

    #[test]
    fn windows_11_gets_segoe_ui_variable_and_windows_10_segoe_ui() {
        let all = |_: &str| true;
        assert_eq!(windows_ui_font(Some(22000), all), Some("SegUIVar.ttf"));
        assert_eq!(windows_ui_font(Some(26100), all), Some("SegUIVar.ttf"));
        // Windows 10, even with the variable font installed by hand.
        assert_eq!(windows_ui_font(Some(19045), all), Some("segoeui.ttf"));
        assert_eq!(windows_ui_font(None, all), Some("segoeui.ttf"), "unknown build: the safe choice");
        // Windows 11 without the variable font falls back; nothing at all keeps egui's font.
        assert_eq!(windows_ui_font(Some(22631), |f| f == "segoeui.ttf"), Some("segoeui.ttf"));
        assert_eq!(windows_ui_font(Some(22631), |_| false), None);
        assert_eq!(windows_ui_font(Some(19045), |f| f == "SegUIVar.ttf"), None);
    }

    #[test]
    fn text_size_follows_the_setting_within_what_windows_allows() {
        assert_eq!(text_scale_percent(None, None), 100, "never changed: normal size");
        assert_eq!(text_scale_percent(None, Some(100)), 100);
        assert_eq!(text_scale_percent(None, Some(150)), 150);
        assert_eq!(text_scale_percent(None, Some(225)), 225);
        assert_eq!(text_scale_percent(None, Some(400)), 225, "kept to Windows' largest");
        assert_eq!(text_scale_percent(None, Some(0)), 100, "never smaller than normal");
        assert_eq!(text_scale_percent(Some("175"), Some(120)), 175, "the test override wins");
        assert_eq!(text_scale_percent(Some(" 130% "), None), 130);
        assert_eq!(text_scale_percent(Some("big"), Some(120)), 120, "a bad override is ignored");
        assert_eq!(text_scale_percent(Some("999"), None), 225);
    }

    fn system_font() -> Arc<eframe::egui::FontData> {
        Arc::new(eframe::egui::FontData::from_owned(std::fs::read(font_file()).unwrap()))
    }

    #[test]
    fn the_system_font_comes_first_and_icons_stay_second() {
        use eframe::egui::FontFamily;
        let mut defs = crate::look::base_fonts(false, "en");
        let before = defs.families[&FontFamily::Proportional].clone();
        let mono_before = defs.families[&FontFamily::Monospace].clone();
        interface_fonts(&mut defs, Some(system_font()), 100);
        let list = &defs.families[&FontFamily::Proportional];
        assert_eq!(list[0], "system-ui");
        assert_eq!(list[1], "Phosphor");
        assert_eq!(list.len(), before.len() + 1);
        assert_eq!(defs.families[&FontFamily::Monospace], mono_before, "code text keeps its font");
        assert_eq!(defs.families[&canvas_family()], before, "picture text keeps egui's font");
        for fam in [FontFamily::Proportional, FontFamily::Monospace, canvas_family()] {
            for n in &defs.families[&fam] {
                assert!(defs.font_data.contains_key(n), "{fam:?}: {n} isn't loaded");
            }
        }
    }

    #[test]
    fn without_a_system_font_or_text_size_nothing_changes() {
        use eframe::egui::FontFamily;
        let mut defs = crate::look::base_fonts(false, "en");
        let before = defs.families.clone();
        interface_fonts(&mut defs, None, 100);
        for fam in [FontFamily::Proportional, FontFamily::Monospace] {
            assert_eq!(defs.families[&fam], before[&fam]);
        }
        assert_eq!(defs.families[&canvas_family()], before[&FontFamily::Proportional]);
    }

    #[test]
    fn bigger_text_grows_the_text_fonts_but_not_icons_or_picture_text() {
        use eframe::egui::FontFamily;
        let mut defs = crate::look::base_fonts(false, "en");
        let own = |defs: &eframe::egui::FontDefinitions, n: &str| defs.font_data.get(n).map_or(1.0, |d| d.tweak.scale);
        let before = defs.clone();
        interface_fonts(&mut defs, Some(system_font()), 150);
        for fam in [FontFamily::Proportional, FontFamily::Monospace] {
            for n in &defs.families[&fam] {
                // egui's emoji fonts carry their own scale; the text size multiplies it.
                let want = match n.as_str() {
                    "Phosphor" => 1.0,
                    "ui-system-ui" => 1.5,
                    _ => own(&before, n.trim_start_matches("ui-")) * 1.5,
                };
                assert!((defs.font_data[n].tweak.scale - want).abs() < 1e-5, "{fam:?}: {n}");
            }
        }
        for n in &defs.families[&canvas_family()] {
            assert_eq!(defs.font_data[n].tweak.scale, own(&before, n), "picture text: {n}");
        }
        assert_eq!(defs.families[&crate::look::icons()], vec!["Phosphor".to_string()]);
    }

    #[test]
    fn bigger_text_lays_out_bigger_and_picture_text_does_not() {
        use eframe::egui::{FontFamily, FontId};
        let measure = |percent: u32, family: FontFamily| {
            let ctx = eframe::egui::Context::default();
            let mut defs = crate::look::base_fonts(false, "en");
            interface_fonts(&mut defs, None, percent);
            ctx.set_fonts(defs);
            let mut size = eframe::egui::Vec2::ZERO;
            let mut out = ctx.run_ui(Default::default(), |ui| {
                size = ui.painter().layout_no_wrap("Capture".into(), FontId::new(14.0, family.clone()), eframe::egui::Color32::WHITE).size();
            });
            // No GPU here: the font pictures the frame would upload are dropped.
            out.textures_delta.clear();
            size
        };
        let (normal, big) = (measure(100, FontFamily::Proportional), measure(200, FontFamily::Proportional));
        assert!(big.x > normal.x * 1.8 && big.y > normal.y * 1.8, "{normal:?} -> {big:?}");
        assert_eq!(measure(100, canvas_family()), measure(200, canvas_family()));
    }

    #[test]
    fn the_text_size_is_part_of_what_the_base_fonts_are_made_for() {
        let ctx = eframe::egui::Context::default();
        let mut ui = UiFonts::default();
        ui.ensure_base_sized(&ctx, "en", false, 100);
        assert_eq!(ui.text_scale, 100);
        ui.ensure_base_sized(&ctx, "en", false, 175);
        assert_eq!(ui.text_scale, 175);
        assert!(ui.defs.font_data.contains_key("ui-Ubuntu-Light"));
    }

    /// The real system font on this computer (Windows only): it parses, and it has no letters
    /// where the icons are, so putting the icons second doesn't hide any.
    #[cfg(windows)]
    #[test]
    fn the_windows_font_is_found_and_leaves_the_icons_alone() {
        let Some(f) = system_ui_font() else { return };
        let face = ttf_parser::Face::parse(f.bytes(), 0).unwrap();
        for cp in 0xE000u32..=0xEAFF {
            let c = char::from_u32(cp).unwrap();
            assert!(face.glyph_index(c).is_none(), "the system font has U+{cp:X}");
        }
    }
}
