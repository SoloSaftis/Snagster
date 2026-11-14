//! Snagster's look: the interface fonts (with every script the 86 languages need), the
//! Phosphor icon font, and the ribbon's building blocks (tabs, tool buttons with an icon over
//! their name, named groups). Every tool has a tooltip.

use eframe::egui::{self, Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Response, Sense, Stroke, Ui, pos2, vec2};
use std::sync::Arc;

pub const ACCENT: Color32 = Color32::from_rgb(91, 127, 245);
pub const TEXT: Color32 = Color32::from_rgb(226, 229, 238);
pub const DIM: Color32 = Color32::from_rgb(150, 156, 172);
pub const FAINT: Color32 = Color32::from_rgb(92, 98, 112);
pub const HOVER: Color32 = Color32::from_rgb(44, 48, 60);
pub const PRESSED: Color32 = Color32::from_rgb(36, 46, 82);
pub const BAND: Color32 = Color32::from_rgb(30, 32, 40);
pub const TABS: Color32 = Color32::from_rgb(22, 24, 30);
pub const REC: Color32 = Color32::from_rgb(229, 72, 77);

/// The icon font family (Phosphor, regular weight, MIT licence).
pub fn icons() -> FontFamily {
    FontFamily::Name("icons".into())
}

/// Phosphor's icons by name (https://phosphoricons.com); private-use code points.
#[allow(dead_code)]
pub mod ph {
    pub const CAMERA: &str = "\u{e10e}";
    pub const MONITOR: &str = "\u{e32e}";
    pub const APP_WINDOW: &str = "\u{e5da}";
    pub const FRAME_CORNERS: &str = "\u{e626}";
    pub const SCROLL: &str = "\u{eb7a}";
    pub const RECORD: &str = "\u{e3ee}";
    pub const VIDEO_CAMERA: &str = "\u{e4da}";
    pub const GIF: &str = "\u{e274}";
    pub const FILM_STRIP: &str = "\u{e792}";
    pub const FOOTPRINTS: &str = "\u{ea88}";
    pub const CROP: &str = "\u{e1d4}";
    pub const ARROWS_OUT: &str = "\u{e0a2}";
    pub const ARROW_CLOCKWISE: &str = "\u{e036}";
    pub const ARROW_COUNTER_CLOCKWISE: &str = "\u{e038}";
    pub const FLIP_HORIZONTAL: &str = "\u{ed6a}";
    pub const MAGIC_WAND: &str = "\u{e6b6}";
    pub const DROP: &str = "\u{e210}";
    pub const EYE_SLASH: &str = "\u{e224}";
    pub const TEXT_T: &str = "\u{e48a}";
    pub const PALETTE: &str = "\u{e6c8}";
    pub const EYEDROPPER: &str = "\u{e568}";
    pub const RULER: &str = "\u{e6b8}";
    pub const PUSH_PIN: &str = "\u{e3e2}";
    pub const CLIPBOARD: &str = "\u{e196}";
    pub const ENVELOPE: &str = "\u{e214}";
    pub const PRINTER: &str = "\u{e3dc}";
    pub const SHARE: &str = "\u{e408}";
    pub const FOLDER_OPEN: &str = "\u{e256}";
    pub const IMAGE: &str = "\u{e2ca}";
    pub const IMAGES: &str = "\u{e836}";
    pub const ZOOM_IN: &str = "\u{e310}";
    pub const ZOOM_OUT: &str = "\u{e30e}";
    pub const ARROWS_IN: &str = "\u{e09a}";
    pub const SIDEBAR: &str = "\u{ec24}";
    pub const GEAR: &str = "\u{e270}";
    pub const QUESTION: &str = "\u{e3e8}";
    pub const INFO: &str = "\u{e2ce}";
    pub const SPARKLE: &str = "\u{e6a2}";
    pub const DOWNLOAD: &str = "\u{e20c}";
    pub const GLOBE: &str = "\u{e288}";
    pub const MICROPHONE: &str = "\u{e326}";
    pub const MICROPHONE_SLASH: &str = "\u{e328}";
    pub const SPEAKER: &str = "\u{e44a}";
    pub const SPEAKER_SLASH: &str = "\u{e45a}";
    pub const PAUSE: &str = "\u{e39e}";
    pub const STOP: &str = "\u{e46c}";
    pub const PLAY: &str = "\u{e3d0}";
    pub const BROADCAST: &str = "\u{e0f2}";
    pub const MONITOR_PLAY: &str = "\u{e58c}";
    pub const STACK: &str = "\u{e466}";
    pub const PLUS: &str = "\u{e3d4}";
    pub const TRASH: &str = "\u{e4a6}";
    pub const TIMER: &str = "\u{e492}";
    pub const CLOCK: &str = "\u{e19a}";
    pub const SELECTION: &str = "\u{e69a}";
    pub const CURSOR: &str = "\u{e1dc}";
    pub const CIRCLE: &str = "\u{e18a}";
    pub const STAR: &str = "\u{e46a}";
    pub const SCISSORS: &str = "\u{eae0}";
    pub const FLOPPY_DISK: &str = "\u{e248}";
    pub const COPY: &str = "\u{e1ca}";
    pub const FILE_PDF: &str = "\u{e702}";
    pub const FILE_DOC: &str = "\u{eb1e}";
    pub const UNDO: &str = "\u{e08a}";
    pub const REDO: &str = "\u{e08c}";
    pub const ARROWS_CLOCKWISE: &str = "\u{e094}";
    pub const LIST: &str = "\u{e2f0}";
    pub const CARDS: &str = "\u{e0f8}";
    pub const SQUARES_FOUR: &str = "\u{e464}";
    pub const SELECTION_PLUS: &str = "\u{e69c}";
    pub const SLIDERS: &str = "\u{e434}";
    pub const TEXT_AA: &str = "\u{e6ee}";
    pub const PAINT_BRUSH: &str = "\u{e6f0}";
    pub const STAMP: &str = "\u{ea48}";
    pub const POLYGON: &str = "\u{e6d0}";
    pub const LASSO: &str = "\u{edc6}";
    pub const CURSOR_CLICK: &str = "\u{e7c8}";
    pub const BROWSER: &str = "\u{e0f4}";
    pub const CLOUD_UP: &str = "\u{e1ae}";
    pub const ARROW_SQUARE_OUT: &str = "\u{e5de}";
    pub const X: &str = "\u{e4f6}";
    pub const VIDEO: &str = "\u{e740}";
    pub const LAYOUT: &str = "\u{e6d6}";
    pub const CHECK: &str = "\u{e182}";
    pub const WARNING: &str = "\u{e4e0}";
    pub const ROBOT: &str = "\u{e762}";
    pub const PLUGS: &str = "\u{eb5a}";
    pub const CARET_DOWN: &str = "\u{e136}";
    pub const CARET_UP: &str = "\u{e13c}";
    pub const WAVEFORM: &str = "\u{e802}";
    pub const DESKTOP: &str = "\u{e560}";
    pub const TEXTBOX: &str = "\u{eb0a}";
    pub const PAINT_BUCKET: &str = "\u{e392}";
    pub const STACK_PLUS: &str = "\u{edf6}";
    pub const KEYBOARD: &str = "\u{e2d8}";
    pub const TRANSLATE: &str = "\u{e4a2}";
    pub const PANORAMA: &str = "\u{eaa2}";
    pub const IMAGES_SQUARE: &str = "\u{e834}";
    pub const COLUMNS: &str = "\u{e546}";
    pub const ROWS: &str = "\u{e5a2}";
    pub const FILM_SLATE: &str = "\u{e8c2}";
    pub const WEBCAM: &str = "\u{e9b2}";
    pub const HEART: &str = "\u{e2a8}";
    pub const FILE_PLUS: &str = "\u{e236}";
    pub const SQUARE: &str = "\u{e45e}";
}

/// Interface fonts: egui's own, the icons, and a font for every script the interface languages
/// use (built in), plus the computer's Chinese, Japanese and Korean fonts when `cjk` (they are
/// large, so they are only read when needed). `lang` puts the language's own CJK font first.
pub fn base_fonts(cjk: bool, lang: &str) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("Phosphor".into(), Arc::new(FontData::from_static(include_bytes!("../assets/fonts/Phosphor.ttf"))));
    let scripts: [(&str, &'static [u8]); 16] = [
        ("NotoSansArabic", include_bytes!("../assets/fonts/scripts/NotoSansArabic.ttf")),
        ("NotoSansHebrew", include_bytes!("../assets/fonts/scripts/NotoSansHebrew-Regular.ttf")),
        ("NotoSansThai", include_bytes!("../assets/fonts/scripts/NotoSansThai.ttf")),
        ("NotoSansDevanagari", include_bytes!("../assets/fonts/scripts/NotoSansDevanagari-Regular.ttf")),
        ("NotoSansBengali", include_bytes!("../assets/fonts/scripts/NotoSansBengali-Regular.ttf")),
        ("NotoSansGujarati", include_bytes!("../assets/fonts/scripts/NotoSansGujarati-Regular.ttf")),
        ("NotoSansGurmukhi", include_bytes!("../assets/fonts/scripts/NotoSansGurmukhi-Regular.ttf")),
        ("NotoSansTamil", include_bytes!("../assets/fonts/scripts/NotoSansTamil-Regular.ttf")),
        ("NotoSansTelugu", include_bytes!("../assets/fonts/scripts/NotoSansTelugu-Regular.ttf")),
        ("NotoSansKannada", include_bytes!("../assets/fonts/scripts/NotoSansKannada-Regular.ttf")),
        ("NotoSansMalayalam", include_bytes!("../assets/fonts/scripts/NotoSansMalayalam-Regular.ttf")),
        ("NotoSansOriya", include_bytes!("../assets/fonts/scripts/NotoSansOriya-Regular.ttf")),
        ("NotoSansKhmer", include_bytes!("../assets/fonts/scripts/NotoSansKhmer-Regular.ttf")),
        ("NotoSansGeorgian", include_bytes!("../assets/fonts/scripts/NotoSansGeorgian.ttf")),
        ("NotoSansArmenian", include_bytes!("../assets/fonts/scripts/NotoSansArmenian-Regular.ttf")),
        ("NotoSansEthiopic", include_bytes!("../assets/fonts/scripts/NotoSansEthiopic-Regular.ttf")),
    ];
    let mut extra = Vec::new();
    for (name, bytes) in scripts {
        let mut fd = FontData::from_static(bytes);
        fd.tweak.fallback_align = egui::epaint::text::FallbackAlign::Baseline;
        fonts.font_data.insert(name.into(), Arc::new(fd));
        extra.push(name.to_string());
    }
    if cjk {
        for (i, (path, index)) in cjk_font_files(lang).into_iter().enumerate() {
            if let Ok(bytes) = std::fs::read(&path) {
                let name = format!("cjk-{i}");
                let mut fd = FontData::from_owned(bytes);
                fd.index = index;
                fd.tweak.fallback_align = egui::epaint::text::FallbackAlign::Baseline;
                fonts.font_data.insert(name.clone(), Arc::new(fd));
                extra.push(name);
            }
        }
    }
    for fam in [FontFamily::Proportional, FontFamily::Monospace] {
        let list = fonts.families.entry(fam).or_default();
        // Icons go right after the first font: egui's emoji font uses some of the same
        // private-use code points.
        list.insert(1.min(list.len()), "Phosphor".into());
        list.extend(extra.iter().cloned());
    }
    fonts.families.insert(icons(), vec!["Phosphor".into()]);
    fonts
}

/// The computer's own fonts for Chinese, Japanese and Korean (file and index in a collection),
/// the interface language's first.
fn cjk_font_files(lang: &str) -> Vec<(std::path::PathBuf, u32)> {
    #[cfg(windows)]
    let (sc, tc, ja, ko): (Vec<&str>, Vec<&str>, Vec<&str>, Vec<&str>) = (
        vec![r"C:\Windows\Fonts\msyh.ttc", r"C:\Windows\Fonts\simsun.ttc"],
        vec![r"C:\Windows\Fonts\msjh.ttc", r"C:\Windows\Fonts\mingliub.ttc"],
        vec![r"C:\Windows\Fonts\YuGothM.ttc", r"C:\Windows\Fonts\YuGothR.ttc", r"C:\Windows\Fonts\meiryo.ttc", r"C:\Windows\Fonts\msgothic.ttc"],
        vec![r"C:\Windows\Fonts\malgun.ttf", r"C:\Windows\Fonts\gulim.ttc"],
    );
    #[cfg(target_os = "macos")]
    let (sc, tc, ja, ko): (Vec<&str>, Vec<&str>, Vec<&str>, Vec<&str>) = (
        vec!["/System/Library/Fonts/PingFang.ttc", "/System/Library/Fonts/Hiragino Sans GB.ttc", "/System/Library/Fonts/STHeiti Light.ttc"],
        vec!["/System/Library/Fonts/PingFang.ttc", "/System/Library/Fonts/STHeiti Light.ttc"],
        vec!["/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc", "/System/Library/Fonts/Hiragino Sans GB.ttc"],
        vec!["/System/Library/Fonts/AppleSDGothicNeo.ttc"],
    );
    #[cfg(all(unix, not(target_os = "macos")))]
    let (sc, tc, ja, ko): (Vec<&str>, Vec<&str>, Vec<&str>, Vec<&str>) = {
        let noto = vec![
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/OTF/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc",
            "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        ];
        (noto.clone(), noto.clone(), noto.clone(), noto)
    };
    let first = |list: &[&str]| list.iter().map(std::path::PathBuf::from).find(|p| p.exists());
    let order: [&[&str]; 4] = match lang.split('-').next().unwrap_or("") {
        "ja" => [&ja, &sc, &ko, &tc],
        "ko" => [&ko, &sc, &ja, &tc],
        "zh" if lang.contains("Hant") => [&tc, &sc, &ja, &ko],
        _ => [&sc, &ja, &ko, &tc],
    };
    let mut out: Vec<(std::path::PathBuf, u32)> = Vec::new();
    for list in order {
        if let Some(p) = first(list)
            && !out.iter().any(|(q, _)| q == &p)
        {
            // Noto CJK collections hold one face per region: pick the language's.
            let index = if p.to_string_lossy().contains("NotoSansCJK") {
                match lang.split('-').next().unwrap_or("") {
                    "ja" => 0,
                    "ko" => 1,
                    "zh" if lang.contains("Hant") => 3,
                    _ => 2,
                }
            } else {
                0
            };
            out.push((p, index));
        }
    }
    out
}

/// One tab of the tab row.
pub fn tab(ui: &mut Ui, label: &str, tip: &str, on: bool) -> Response {
    let font = FontId::proportional(13.5);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, if on { Color32::WHITE } else { DIM });
    let size = vec2(galley.size().x + 22.0, 30.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let resp = resp.on_hover_text(tip);
    if resp.hovered() && !on {
        ui.painter().rect_filled(rect.shrink2(vec2(2.0, 3.0)), CornerRadius::same(6), HOVER);
    }
    let color = if on { Color32::WHITE } else if resp.hovered() { TEXT } else { DIM };
    let pos = rect.center() - galley.size() / 2.0;
    ui.painter().galley(pos, galley, color);
    if on {
        let y = rect.bottom() - 2.0;
        ui.painter().line_segment([pos2(rect.left() + 8.0, y), pos2(rect.right() - 8.0, y)], Stroke::new(2.5, ACCENT));
    }
    resp
}

/// A big ribbon tool: its icon over its name, flat until hovered, with a tooltip.
pub fn big_tool(ui: &mut Ui, icon: &str, label: &str, tip: &str, enabled: bool, active: bool) -> Response {
    let font = FontId::proportional(11.5);
    let galley = ui.painter().layout(label.to_string(), font, TEXT, 88.0);
    let w = galley.size().x.max(34.0) + 14.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 62.0), if enabled { Sense::click() } else { Sense::hover() });
    let resp = resp.on_hover_text(tip);
    let p = ui.painter();
    if active {
        p.rect_filled(rect, CornerRadius::same(7), PRESSED);
        p.rect_stroke(rect, CornerRadius::same(7), Stroke::new(1.0, ACCENT), egui::StrokeKind::Inside);
    } else if enabled && resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(7), if resp.is_pointer_button_down_on() { PRESSED } else { HOVER });
    }
    let icol = if !enabled {
        FAINT
    } else if icon == ph::RECORD || icon == ph::STOP {
        REC
    } else {
        ACCENT
    };
    p.text(pos2(rect.center().x, rect.top() + 18.0), Align2::CENTER_CENTER, icon, FontId::new(23.0, icons()), icol);
    let tcol = if enabled { TEXT } else { FAINT };
    let pos = pos2(rect.center().x - galley.size().x / 2.0, rect.top() + 34.0);
    p.galley(pos, galley, tcol);
    resp
}

/// A small ribbon tool: icon and name side by side (three fit in a column).
pub fn small_tool(ui: &mut Ui, icon: &str, label: &str, tip: &str, enabled: bool) -> Response {
    let font = FontId::proportional(12.5);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, TEXT);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 32.0, 20.0), if enabled { Sense::click() } else { Sense::hover() });
    let resp = resp.on_hover_text(tip);
    let p = ui.painter();
    if enabled && resp.hovered() {
        p.rect_filled(rect.expand(1.0), CornerRadius::same(5), HOVER);
    }
    p.text(pos2(rect.left() + 12.0, rect.center().y), Align2::CENTER_CENTER, icon, FontId::new(15.0, icons()), if enabled { ACCENT } else { FAINT });
    p.galley(pos2(rect.left() + 26.0, rect.center().y - galley.size().y / 2.0), galley, if enabled { TEXT } else { FAINT });
    resp
}

/// A ribbon group: its tools, its name underneath, a divider after it.
pub fn group(ui: &mut Ui, name: &str, body: impl FnOnce(&mut Ui)) {
    let r = ui.vertical(|ui| {
        ui.set_min_height(84.0);
        ui.horizontal(|ui| {
            ui.set_min_height(64.0);
            ui.spacing_mut().item_spacing.x = 2.0;
            body(ui);
        });
        ui.add_space(1.0);
        let w = ui.min_rect().width();
        ui.allocate_ui(vec2(w, 14.0), |ui| {
            ui.centered_and_justified(|ui| ui.label(egui::RichText::new(name).size(10.5).color(FAINT)));
        });
    });
    let x = r.response.rect.right() + 6.0;
    let rect = r.response.rect;
    ui.add_space(12.0);
    ui.painter().line_segment([pos2(x, rect.top() + 6.0), pos2(x, rect.bottom() - 4.0)], Stroke::new(1.0, Color32::from_rgb(52, 56, 68)));
}

/// A column of small tools inside a group.
pub fn column(ui: &mut Ui, body: impl FnOnce(&mut Ui)) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        body(ui);
    });
}

/// A big tool that opens a menu (marked with a small caret).
pub fn big_menu(ui: &mut Ui, icon: &str, label: &str, tip: &str, body: impl FnOnce(&mut Ui)) {
    let resp = big_tool(ui, icon, &format!("{label} {}", ph::CARET_DOWN), tip, true, false);
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(220.0);
        body(ui);
    });
}

/// A small tool that opens a menu.
pub fn small_menu(ui: &mut Ui, icon: &str, label: &str, tip: &str, body: impl FnOnce(&mut Ui)) {
    let resp = small_tool(ui, icon, &format!("{label} {}", ph::CARET_DOWN), tip, true);
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(200.0);
        body(ui);
    });
}

/// An icon-only button for tight places (tooltip required).
pub fn icon_button(ui: &mut Ui, icon: &str, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    let resp = resp.on_hover_text(tip);
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(6), HOVER);
    }
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, icon, FontId::new(17.0, icons()), if resp.hovered() { Color32::WHITE } else { DIM });
    resp
}

