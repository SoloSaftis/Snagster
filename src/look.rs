//! Snagster's look: the interface fonts (with every script the 86 languages need), the
//! Phosphor icon font, and the ribbon's building blocks (tabs, tool buttons with an icon over
//! their name, named groups). Every tool has a tooltip.

use eframe::egui::{self, Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Rect, Response, Sense, Stroke, Ui, pos2, vec2};
use std::sync::Arc;

pub use crate::theme::pal;

/// The accent colour of the palette in use (selection, focus, the main buttons): the desktop's
/// where it has one (Linux), else Snagster's blue, made readable on the palette's panels.
pub fn accent() -> Color32 {
    pal().link
}

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
    pub const KEY: &str = "\u{e2d6}";
    pub const EYE: &str = "\u{e220}";
    pub const MONITOR_PLAY: &str = "\u{e58c}";
    pub const STACK: &str = "\u{e466}";
    pub const PLUS: &str = "\u{e3d4}";
    pub const TRASH: &str = "\u{e4a6}";
    pub const TIMER: &str = "\u{e492}";
    pub const CLOCK: &str = "\u{e19a}";
    pub const CLOCK_COUNTER_CLOCKWISE: &str = "\u{e1a0}";
    pub const DOWNLOAD_SIMPLE: &str = "\u{e20e}";
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
    pub const PEN_NIB: &str = "\u{e3ac}";
    pub const BROOM: &str = "\u{ec54}";
    pub const VIDEO: &str = "\u{e740}";
    pub const LAYOUT: &str = "\u{e6d6}";
    pub const CHECK: &str = "\u{e182}";
    pub const WARNING: &str = "\u{e4e0}";
    pub const WARNING_OCTAGON: &str = "\u{e4e4}";
    pub const CHART_LINE: &str = "\u{e154}";
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

/// How wide `tab` draws a tab with this label.
pub fn tab_width(ui: &Ui, label: &str) -> f32 {
    ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(13.5), pal().dim).size().x + 22.0
}

/// How high `tab` draws a tab: 30, or the text and some room when the text is bigger.
pub fn tab_height(ui: &Ui) -> f32 {
    (ui.painter().layout_no_wrap("Hg".into(), FontId::proportional(13.5), pal().dim).size().y + 8.0).max(30.0)
}

/// One tab of the tab row.
pub fn tab(ui: &mut Ui, label: &str, tip: &str, on: bool) -> Response {
    named_tab(ui, label, label, tip, on)
}

/// A tab showing `label` (which may be just an icon), known to screen readers and the key tips
/// as `name`.
pub fn named_tab(ui: &mut Ui, label: &str, name: &str, tip: &str, on: bool) -> Response {
    let font = FontId::proportional(13.5);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, if on { pal().strong } else { pal().dim });
    // 30 high, taller when Windows' bigger text needs it.
    let size = vec2(tab_width(ui, label), tab_height(ui));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let resp = resp.on_hover_text(crate::platform::keys(tip));
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, name));
    crate::app::keys_ui::note(ui, &resp, name, true, on);
    if resp.hovered() && !on {
        ui.painter().rect_filled(rect.shrink2(vec2(2.0, 3.0)), CornerRadius::same(6), pal().hover);
    }
    let hovered = resp.hovered() && !on;
    let color = if on { pal().strong } else if hovered { pal().on_hover } else { pal().dim };
    let pos = rect.center() - galley.size() / 2.0;
    ui.painter().galley(pos, galley, color);
    if on {
        let y = rect.bottom() - 2.0;
        ui.painter().line_segment([pos2(rect.left() + 8.0, y), pos2(rect.right() - 8.0, y)], Stroke::new(2.5, accent()));
    }
    resp
}

/// A big ribbon tool: its icon over its name, flat until hovered, with a tooltip.
pub fn big_tool(ui: &mut Ui, icon: &str, label: &str, tip: &str, enabled: bool, active: bool) -> Response {
    let font = FontId::proportional(12.0);
    // Names wrap at 88 (wider with Windows' bigger text), and the tool grows with them.
    let galley = ui.painter().layout(label.to_string(), font, pal().text, 88.0 * crate::fonts::text_scale());
    let w = galley.size().x.max(34.0) + 14.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, (36.0 + galley.size().y).max(62.0)), if enabled { Sense::click() } else { Sense::hover() });
    let resp = resp.on_hover_text(crate::platform::keys(tip));
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, enabled, active, label));
    crate::app::keys_ui::note(ui, &resp, label, enabled, active);
    let p = ui.painter();
    let pal = pal();
    let down = active || (enabled && resp.is_pointer_button_down_on());
    if active {
        p.rect_filled(rect, CornerRadius::same(7), pal.pressed);
        p.rect_stroke(rect, CornerRadius::same(7), Stroke::new(1.0, pal.link), egui::StrokeKind::Inside);
    } else if enabled && resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(7), if down { pal.pressed } else { pal.hover });
    }
    // In a contrast theme the system's colours win: no red or blue icons, and the chosen
    // tool's icon and name in the highlight's own text colour.
    let lit = enabled && resp.hovered() && !down;
    let icol = if !enabled {
        pal.disabled
    } else if pal.high_contrast {
        if down { pal.selected_text } else if lit { pal.on_hover } else { pal.text }
    } else if icon == ph::RECORD || icon == ph::STOP || icon == ph::BROADCAST {
        REC
    } else {
        pal.link
    };
    p.text(pos2(rect.center().x, rect.top() + 18.0), Align2::CENTER_CENTER, icon, FontId::new(23.0, icons()), icol);
    let tcol = if !enabled { pal.disabled } else if down { pal.selected_text } else if lit { pal.on_hover } else { pal.text };
    let pos = pos2(rect.center().x - galley.size().x / 2.0, rect.top() + 34.0);
    p.galley(pos, galley, tcol);
    resp
}

/// The narrowest a `pane_tool` gets.
pub const PANE_TOOL_W: f32 = 76.0;

/// The name size of a `pane_tool` (points), which a row too wide for its window lowers.
pub const PANE_TOOL_TEXT: f32 = 11.5;

/// How wide a `pane_tool` with `label` at `size` points is: its name and a margin, at least
/// `PANE_TOOL_W`, so a long name (German, Estonian, Arabic…) is never cut off.
pub fn pane_tool_width(ui: &Ui, label: &str, size: f32) -> f32 {
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_string(), FontId::proportional(size), Color32::WHITE).size().x);
    (w + 14.0).max(PANE_TOOL_W).ceil()
}

/// One icon of a Mac Settings window's toolbar: the icon over the pane's name (at `size`
/// points), the pane shown sitting on a soft filled square (as in Apple's own Settings windows).
pub fn pane_tool(ui: &mut Ui, icon: &str, label: &str, size: f32, on: bool) -> Response {
    let font = FontId::proportional(size);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, pal().text);
    let width = pane_tool_width(ui, label, size);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 54.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, on, label));
    let p = ui.painter();
    let pal = pal();
    if on {
        p.rect_filled(rect.shrink(2.0), CornerRadius::same(7), pal.pressed);
    } else if resp.hovered() {
        p.rect_filled(rect.shrink(2.0), CornerRadius::same(7), pal.hover);
    }
    // Reached with the keyboard (Full Keyboard Access): the focus ring.
    if resp.has_focus() {
        p.rect_stroke(rect.shrink(1.0), CornerRadius::same(8), Stroke::new(2.0, pal.link), egui::StrokeKind::Inside);
    }
    let icol = if pal.high_contrast { if on { pal.selected_text } else { pal.text } } else if on { pal.link } else { pal.dim };
    p.text(pos2(rect.center().x, rect.top() + 19.0), Align2::CENTER_CENTER, icon, FontId::new(22.0, icons()), icol);
    let tcol = if on { pal.strong } else { pal.text };
    // The button is as wide as its name (`pane_tool_width`), so the name always shows whole.
    let pos = pos2(rect.center().x - galley.size().x / 2.0, rect.top() + 35.0);
    p.galley(pos, galley, tcol);
    resp
}

/// A small ribbon tool: icon and name side by side (three fit in a column).
pub fn small_tool(ui: &mut Ui, icon: &str, label: &str, tip: &str, enabled: bool) -> Response {
    let font = FontId::proportional(12.5);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, pal().text);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 32.0, galley.size().y.max(20.0)), if enabled { Sense::click() } else { Sense::hover() });
    let resp = resp.on_hover_text(crate::platform::keys(tip));
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    crate::app::keys_ui::note(ui, &resp, label, enabled, false);
    let p = ui.painter();
    let pal = pal();
    let lit = enabled && resp.hovered();
    if lit {
        p.rect_filled(rect.expand(1.0), CornerRadius::same(5), pal.hover);
    }
    // In a contrast theme icons at rest are plain text colour, not the highlight.
    let icol = if !enabled {
        pal.disabled
    } else if lit {
        if pal.high_contrast { pal.on_hover } else { pal.link }
    } else if pal.high_contrast {
        pal.text
    } else {
        pal.link
    };
    p.text(pos2(rect.left() + 12.0, rect.center().y), Align2::CENTER_CENTER, icon, FontId::new(15.0, icons()), icol);
    p.galley(pos2(rect.left() + 26.0, rect.center().y - galley.size().y / 2.0), galley, if !enabled { pal.disabled } else if lit { pal.on_hover } else { pal.text });
    resp
}

/// A ribbon group: its tools, its name underneath, a divider after it.
pub fn group(ui: &mut Ui, name: &str, body: impl FnOnce(&mut Ui)) {
    // In a narrow window the ribbon shows some groups in the band and the rest in its More menu.
    let Some(index) = crate::app::ribbon::fit::next_group() else { return };
    let r = ui.vertical(|ui| {
        // Every group as tall as three small tools (taller with bigger text), so names line up.
        let row = ui.painter().layout_no_wrap("Hg".into(), FontId::proportional(12.5), pal().text).size().y.max(20.0);
        let body_h = (row * 3.0 + 4.0).max(64.0);
        ui.set_min_height(body_h + 22.0);
        ui.horizontal(|ui| {
            ui.set_min_height(body_h);
            ui.spacing_mut().item_spacing.x = 2.0;
            body(ui);
        });
        ui.add_space(1.0);
        // As wide as its name at least (bigger text makes names wider than the tools).
        let name_w = ui.painter().layout_no_wrap(name.to_string(), FontId::proportional(12.0), pal().dim).size().x;
        let w = ui.min_rect().width().max(name_w);
        ui.allocate_ui(vec2(w, 16.0), |ui| {
            ui.centered_and_justified(|ui| ui.label(egui::RichText::new(name).size(12.0).color(pal().dim)));
        });
    });
    let x = r.response.rect.right() + 6.0;
    let rect = r.response.rect;
    ui.add_space(12.0);
    ui.painter().line_segment([pos2(x, rect.top() + 6.0), pos2(x, rect.bottom() - 4.0)], Stroke::new(1.0, pal().line));
    crate::app::ribbon::fit::measured(index, rect.width() + 12.0 + ui.spacing().item_spacing.x, x);
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
    let resp = resp.on_hover_text(crate::platform::keys(tip));
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tip));
    // Its key tip comes from its name, not its keys: "Settings (Ctrl+,)" is Settings.
    crate::app::keys_ui::note(ui, &resp, tip.split(" (").next().unwrap_or(tip), true, false);
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(6), pal().hover);
    }
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, icon, FontId::new(17.0, icons()), if resp.hovered() { pal().on_hover } else { pal().dim });
    resp
}

/// A checkbox in the ribbon (the keyboard reaches it like the tools around it).
pub fn check(ui: &mut Ui, on: &mut bool, label: &str, tip: &str) -> Response {
    let resp = ui.checkbox(on, label).on_hover_text(tip);
    crate::app::keys_ui::note(ui, &resp, label, true, *on);
    resp
}

/// The ring around the control that has the keyboard: two pixels of the strongest text colour
/// with the background's colour inside it, so it shows on any fill (and in contrast themes,
/// where these are the system's own colours). The picture gets it inside its edge.
pub fn focus_ring(p: &egui::Painter, rect: Rect, inside: bool) {
    let pal = pal();
    // GNOME and KDE draw focus in the accent colour; Windows in the strongest text colour.
    let ring = if crate::platform::linux() && !pal.high_contrast { accent() } else { pal.strong };
    let (r, kind) = if inside { (rect.shrink(2.0), egui::StrokeKind::Inside) } else { (rect.expand(1.0), egui::StrokeKind::Outside) };
    p.rect_stroke(r, CornerRadius::same(6), Stroke::new(2.0, ring), kind);
    p.rect_stroke(r, CornerRadius::same(6), Stroke::new(1.0, pal.bg), if inside { egui::StrokeKind::Outside } else { egui::StrokeKind::Inside });
}

/// A key tip: the key's letters in a small box at the bottom middle of the control, as
/// Office shows them. Dimmed for a control that can't be used now.
pub fn key_tip(p: &egui::Painter, rect: Rect, keys: &str, enabled: bool) {
    let pal = pal();
    let font = FontId::proportional(12.0);
    let g = p.layout_no_wrap(keys.to_string(), font, pal.bg);
    let size = vec2(g.size().x + 8.0, g.size().y + 2.0).max(vec2(16.0, 16.0));
    let at = pos2(rect.center().x, rect.bottom() - size.y / 2.0 + 2.0);
    let b = Rect::from_center_size(at, size);
    p.rect_filled(b, CornerRadius::same(3), if enabled { pal.strong } else { pal.disabled });
    p.rect_stroke(b, CornerRadius::same(3), Stroke::new(1.0, pal.bg), egui::StrokeKind::Outside);
    p.galley(b.center() - g.size() / 2.0, g, pal.bg);
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn interface_fonts_have_icons_after_the_first_font_and_every_script() {
        let f = base_fonts(false, "en");
        assert!(f.font_data.contains_key("Phosphor"));
        assert_eq!(f.families.get(&icons()), Some(&vec!["Phosphor".to_string()]));
        for fam in [FontFamily::Proportional, FontFamily::Monospace] {
            let list = &f.families[&fam];
            assert_eq!(list[1], "Phosphor", "{fam:?}: {list:?}");
            assert_eq!(list.iter().filter(|n| *n == "Phosphor").count(), 1);
            for script in ["NotoSansArabic", "NotoSansHebrew", "NotoSansThai", "NotoSansDevanagari", "NotoSansEthiopic"] {
                assert!(list.iter().any(|n| n == script), "{fam:?} lacks {script}");
            }
            // Every font a family names is loaded.
            for n in list {
                assert!(f.font_data.contains_key(n), "{n} isn't loaded");
            }
            assert!(!list.iter().any(|n| n.starts_with("cjk-")), "no CJK fonts unless asked");
        }
    }

    #[test]
    fn built_in_fonts_parse() {
        let f = base_fonts(false, "en");
        for name in ["Phosphor", "NotoSansArabic", "NotoSansKhmer", "NotoSansGeorgian"] {
            let data = &f.font_data[name];
            assert!(ttf_parser::Face::parse((*data.font).as_ref(), data.index).is_ok(), "{name}");
        }
    }

    #[test]
    fn cjk_fonts_are_added_only_when_found() {
        for lang in ["zh", "zh-Hant", "ja", "ko", "en"] {
            let files = cjk_font_files(lang);
            let unique: HashSet<_> = files.iter().map(|(p, _)| p.clone()).collect();
            assert_eq!(unique.len(), files.len(), "{lang}: a file is listed twice");
            for (p, index) in &files {
                assert!(p.exists());
                if p.to_string_lossy().contains("NotoSansCJK") {
                    let want = match lang {
                        "ja" => 0,
                        "ko" => 1,
                        "zh-Hant" => 3,
                        _ => 2,
                    };
                    assert_eq!(*index, want, "{lang}");
                } else {
                    assert_eq!(*index, 0);
                }
            }
            let f = base_fonts(true, lang);
            let cjk = f.families[&FontFamily::Proportional].iter().filter(|n| n.starts_with("cjk-")).count();
            assert!(cjk <= files.len());
        }
    }

    /// Every icon in `ph`: one private-use character, and no two names share one.
    #[test]
    fn icons_are_single_private_use_characters() {
        let src = include_str!("look.rs");
        let start = src.find("pub mod ph {").unwrap();
        let end = start + src[start..].find("\n}").unwrap();
        let mut seen = std::collections::HashMap::new();
        let mut count = 0;
        for line in src[start..end].lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("pub const ") else { continue };
            let name = rest.split(':').next().unwrap();
            let hex = rest.split(r"\u{").nth(1).and_then(|h| h.split('}').next()).unwrap();
            let cp = u32::from_str_radix(hex, 16).unwrap();
            assert!((0xE000..=0xF8FF).contains(&cp), "{name} U+{cp:X} isn't private use");
            if let Some(other) = seen.insert(cp, name.to_string()) {
                panic!("{name} and {other} are the same icon (U+{cp:X})");
            }
            count += 1;
        }
        assert!(count > 50, "only {count} icons found; did the module change shape?");
        for icon in [ph::CAMERA, ph::RECORD, ph::STOP, ph::CARET_DOWN, ph::X] {
            assert_eq!(icon.chars().count(), 1);
        }
    }

    #[test]
    fn colours_stand_apart() {
        // Hover and pressed must show on the ribbon in every palette, and recording is red.
        for p in [crate::theme::DARK, crate::theme::LIGHT] {
            assert_ne!(p.hover, p.band);
            assert_ne!(p.pressed, p.hover);
            assert_ne!(p.tabs, p.band);
            for c in [p.accent, p.text, p.dim, p.faint, p.hover, p.pressed, p.band, p.tabs, REC] {
                assert_eq!(c.a(), 255, "interface colours are opaque");
            }
        }
        assert!(REC.r() > REC.g() && REC.r() > REC.b(), "recording is red");
    }
}
