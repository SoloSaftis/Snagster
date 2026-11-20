//! Light, dark and high contrast: the colours of Snagster's own interface.
//!
//! Every window paints with `pal()`, the palette in use. Which palette that is follows the
//! person's choice in Settings (Follow system, Light, Dark) and what the system says: light or
//! dark (winit on Windows and macOS, the Settings portal on Linux), high contrast (Windows'
//! contrast themes give their own colours; Linux's portal `contrast` and macOS's Increase
//! contrast make Snagster's own palette stronger) and the desktop's accent colour (Linux).
//!
//! What is drawn on a picture (marks, the capture overlay's frame and labels) is content and keeps
//! its own fixed colours, so it reads the same on any picture.

use eframe::egui::{self, Color32, CornerRadius, Stroke};
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock, RwLock};

/// What the person picked in Settings → General → Appearance (App theme on Windows).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

/// The heading over the choice: Windows' own apps say "App theme".
pub fn heading() -> &'static str {
    if cfg!(target_os = "windows") { "App theme" } else { "Appearance" }
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark];

    /// The name shown in Settings (English; passed through `i18n::t`), in each system's words.
    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::System if cfg!(target_os = "windows") => "Use system setting",
            ThemeChoice::System if cfg!(target_os = "macos") => "Match system",
            ThemeChoice::System => "Follow system",
            ThemeChoice::Light => "Light",
            ThemeChoice::Dark => "Dark",
        }
    }
}

/// The colours a high-contrast theme gives (Windows' contrast themes: `GetSysColor`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SystemColors {
    pub window: Color32,
    pub window_text: Color32,
    pub highlight: Color32,
    pub highlight_text: Color32,
    pub button: Color32,
    pub button_text: Color32,
    pub gray_text: Color32,
    pub link: Color32,
}

/// What the system says, as far as it says anything.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SystemLook {
    /// Dark (true) or light (false); None when the system doesn't say.
    pub dark: Option<bool>,
    /// A high-contrast theme with its own colours is on (Windows).
    pub contrast_colors: Option<SystemColors>,
    /// More contrast is asked for, without colours of its own (Linux portal, macOS).
    pub more_contrast: bool,
    /// The desktop's accent colour (Linux).
    pub accent: Option<Color32>,
}

/// Every colour Snagster's interface paints with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Palette {
    pub dark: bool,
    pub high_contrast: bool,
    /// Text fields, the canvas round the picture, the recording bar.
    pub bg: Color32,
    /// Windows and side panels.
    pub panel: Color32,
    /// Buttons and fields at rest.
    pub panel_hi: Color32,
    /// Buttons under the pointer.
    pub widget_hover: Color32,
    /// The ribbon's tool band and the row of tabs above it.
    pub band: Color32,
    pub tabs: Color32,
    /// A tool under the pointer, and the tool that is on.
    pub hover: Color32,
    pub pressed: Color32,
    /// Body text, headings, and text on a `pressed` tool or a `hover` one.
    pub text: Color32,
    pub on_hover: Color32,
    /// Text on egui's buttons.
    pub button_text: Color32,
    pub strong: Color32,
    pub selected_text: Color32,
    /// Secondary text (notes under a setting, an icon at rest).
    pub dim: Color32,
    /// Group names on the ribbon and other quiet labels; still readable (4.5:1).
    pub faint: Color32,
    /// Things that can't be used now.
    pub disabled: Color32,
    /// Separators, frames and borders.
    pub line: Color32,
    pub border: Color32,
    /// Cards in the Library and the recent-pictures tray.
    pub card: Color32,
    pub card_hover: Color32,
    pub card_text: Color32,
    /// The accent as a fill (selection, the main buttons, the chosen segment), and the text on it.
    pub accent: Color32,
    pub on_accent: Color32,
    /// The accent as text, icons, links and focus rings: readable on the panels.
    pub link: Color32,
    /// Buttons that delete or stop (white text on them).
    pub danger: Color32,
    /// Messages: it worked, take care, it failed (text colours).
    pub ok: Color32,
    pub warn: Color32,
    pub err: Color32,
}

/// Snagster's brand blue (one value everywhere; the desktop's accent wins where it has one).
pub const BRAND: Color32 = Color32::from_rgb(79, 124, 255);

pub const DARK: Palette = Palette {
    dark: true,
    high_contrast: false,
    bg: Color32::from_rgb(24, 25, 30),
    panel: Color32::from_rgb(33, 35, 42),
    panel_hi: Color32::from_rgb(46, 49, 59),
    widget_hover: Color32::from_rgb(60, 64, 78),
    band: Color32::from_rgb(30, 32, 40),
    tabs: Color32::from_rgb(22, 24, 30),
    hover: Color32::from_rgb(44, 48, 60),
    pressed: Color32::from_rgb(36, 46, 82),
    text: Color32::from_rgb(226, 229, 238),
    on_hover: Color32::WHITE,
    button_text: Color32::from_rgb(226, 229, 238),
    strong: Color32::WHITE,
    selected_text: Color32::WHITE,
    dim: Color32::from_rgb(150, 155, 170),
    faint: Color32::from_rgb(134, 140, 156),
    disabled: Color32::from_rgb(104, 110, 124),
    line: Color32::from_rgb(52, 56, 68),
    border: Color32::from_rgb(112, 118, 134),
    card: Color32::from_rgb(28, 30, 36),
    card_hover: Color32::from_rgb(46, 49, 59),
    card_text: Color32::from_rgb(210, 214, 225),
    accent: Color32::from_rgb(60, 98, 222),
    on_accent: Color32::WHITE,
    link: Color32::from_rgb(122, 156, 255),
    danger: Color32::from_rgb(196, 43, 28),
    ok: Color32::from_rgb(90, 200, 130),
    warn: Color32::from_rgb(240, 160, 110),
    err: Color32::from_rgb(240, 120, 110),
};

pub const LIGHT: Palette = Palette {
    dark: false,
    high_contrast: false,
    bg: Color32::from_rgb(255, 255, 255),
    panel: Color32::from_rgb(243, 243, 246),
    panel_hi: Color32::from_rgb(226, 228, 234),
    widget_hover: Color32::from_rgb(212, 215, 223),
    band: Color32::from_rgb(250, 250, 251),
    tabs: Color32::from_rgb(235, 236, 240),
    hover: Color32::from_rgb(228, 231, 238),
    pressed: Color32::from_rgb(212, 223, 250),
    text: Color32::from_rgb(28, 30, 36),
    on_hover: Color32::BLACK,
    button_text: Color32::from_rgb(28, 30, 36),
    strong: Color32::BLACK,
    selected_text: Color32::BLACK,
    dim: Color32::from_rgb(84, 90, 104),
    faint: Color32::from_rgb(96, 102, 116),
    disabled: Color32::from_rgb(150, 155, 166),
    line: Color32::from_rgb(214, 217, 224),
    border: Color32::from_rgb(128, 134, 148),
    card: Color32::from_rgb(255, 255, 255),
    card_hover: Color32::from_rgb(232, 237, 248),
    card_text: Color32::from_rgb(40, 44, 52),
    accent: Color32::from_rgb(38, 90, 214),
    on_accent: Color32::WHITE,
    link: Color32::from_rgb(38, 90, 214),
    danger: Color32::from_rgb(196, 43, 28),
    ok: Color32::from_rgb(16, 120, 64),
    warn: Color32::from_rgb(166, 78, 0),
    err: Color32::from_rgb(190, 40, 30),
};

/// Our own palette made stronger (Linux's portal `contrast`, macOS's Increase contrast):
/// black or white text, quiet text nearly as strong, borders that show.
pub fn stronger(p: Palette) -> Palette {
    let (fg, quiet, edge) = if p.dark {
        (Color32::WHITE, Color32::from_rgb(214, 218, 228), Color32::from_rgb(170, 176, 190))
    } else {
        (Color32::BLACK, Color32::from_rgb(40, 44, 54), Color32::from_rgb(80, 86, 100))
    };
    Palette {
        high_contrast: true,
        text: fg,
        on_hover: fg,
        button_text: fg,
        strong: fg,
        selected_text: fg,
        dim: quiet,
        faint: quiet,
        card_text: fg,
        line: edge,
        border: edge,
        ..p
    }
}

/// A Windows contrast theme: its own colours, every one of them.
pub fn from_system(c: SystemColors) -> Palette {
    let dark = luminance(c.window) < 0.5;
    Palette {
        dark,
        high_contrast: true,
        bg: c.window,
        panel: c.window,
        panel_hi: c.button,
        // Hover, pressed and chosen all use the highlight and its text (Microsoft's rule).
        widget_hover: c.highlight,
        band: c.window,
        tabs: c.window,
        hover: c.highlight,
        pressed: c.highlight,
        text: c.window_text,
        on_hover: c.highlight_text,
        button_text: c.button_text,
        strong: c.window_text,
        selected_text: c.highlight_text,
        dim: c.window_text,
        // GrayText is for what can't be used, never for quiet text.
        faint: c.window_text,
        disabled: c.gray_text,
        line: c.window_text,
        border: c.window_text,
        card: c.window,
        card_hover: c.button,
        card_text: c.window_text,
        accent: c.highlight,
        on_accent: c.highlight_text,
        link: c.link,
        danger: c.highlight,
        ok: c.window_text,
        warn: c.window_text,
        err: c.window_text,
    }
}

/// Dark or light: the choice, else the system's answer, else dark (Snagster's look until now).
pub fn wants_dark(choice: ThemeChoice, system_dark: Option<bool>) -> bool {
    match choice {
        ThemeChoice::Dark => true,
        ThemeChoice::Light => false,
        ThemeChoice::System => system_dark.unwrap_or(true),
    }
}

/// The palette for a choice and what the system says.
pub fn choose(choice: ThemeChoice, sys: &SystemLook) -> Palette {
    // A contrast theme's colours win over every choice (Windows: "the system's colours win").
    if let Some(c) = sys.contrast_colors {
        return from_system(c);
    }
    let base = if wants_dark(choice, sys.dark) { DARK } else { LIGHT };
    let mut p = if sys.more_contrast { stronger(base) } else { base };
    // One accent, two uses: as a fill it is darkened until white text on it reads (as libadwaita
    // and Breeze always put white on the accent); as text and rings, made readable on the panels.
    let raw = sys.accent.unwrap_or(if p.dark { BRAND } else { LIGHT.accent });
    let need = if p.high_contrast { 7.0 } else { 4.5 };
    p.accent = readable_on(raw, Color32::WHITE, 4.5);
    p.on_accent = Color32::WHITE;
    p.link = readable_on(raw, p.panel, need);
    p
}

/// WCAG relative luminance of a colour (0 black .. 1 white).
pub fn luminance(c: Color32) -> f32 {
    let ch = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
}

/// WCAG contrast ratio of two colours (1 .. 21).
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// `c` made lighter (on a dark background) or darker (on a light one), keeping its hue, until it
/// stands `need`:1 against `bg`. Used for the accent: a desktop's accent picked for a light
/// desktop can be too dark on our dark panels, and the other way round.
pub fn readable_on(c: Color32, bg: Color32, need: f32) -> Color32 {
    if contrast(c, bg) >= need {
        return c;
    }
    let toward = if luminance(bg) < 0.5 { Color32::WHITE } else { Color32::BLACK };
    for i in 1..=20 {
        let m = mix(c, toward, i as f32 / 20.0);
        if contrast(m, bg) >= need {
            return m;
        }
    }
    toward
}

/// The outline a colour swatch (or a quick style's ring) of colour `c` gets on `behind`, so it
/// never melts into the panel: black on a dark panel, white or yellow on a light one. None when
/// the colour already stands out (3:1, WCAG's rule for things that aren't text). In a
/// high-contrast theme every swatch has one, as the theme's other controls do.
pub fn swatch_edge(c: Color32, behind: Color32, p: &Palette) -> Option<Color32> {
    (p.high_contrast || contrast(c, behind) < 3.0).then(|| readable_on(p.border, behind, 3.0))
}

/// Black or white, whichever reads better on `bg`.
pub fn text_on(bg: Color32) -> Color32 {
    if contrast(Color32::WHITE, bg) >= contrast(Color32::BLACK, bg) { Color32::WHITE } else { Color32::BLACK }
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// egui's own widgets in a palette's colours.
pub fn visuals(p: &Palette) -> egui::Visuals {
    let mut v = if p.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.override_text_color = None;
    v.panel_fill = p.panel;
    v.window_fill = p.panel;
    // Contrast themes: windows, menus and popups get a 2 px edge.
    v.window_stroke = Stroke::new(if p.high_contrast { 2.0 } else { 1.0 }, p.border);
    v.extreme_bg_color = p.bg;
    v.text_edit_bg_color = Some(p.bg);
    v.faint_bg_color = p.band;
    v.code_bg_color = p.panel_hi;
    v.hyperlink_color = p.link;
    v.selection.bg_fill = p.accent;
    v.selection.stroke = Stroke::new(1.0, p.on_accent);
    v.warn_fg_color = p.warn;
    v.error_fg_color = p.err;
    let r = CornerRadius::same(6);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.panel;
    w.noninteractive.weak_bg_fill = p.panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.line);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.bg_fill = p.panel_hi;
    w.inactive.weak_bg_fill = p.panel_hi;
    // A field, tick box or button needs an edge that stands 3:1 to be seen (WCAG 1.4.11).
    w.inactive.bg_stroke = Stroke::new(1.0, p.border);
    w.inactive.fg_stroke = Stroke::new(1.0, p.button_text);
    // Hover and keyboard focus look the same in egui: an accent ring that stands 3:1.
    w.hovered.bg_fill = p.widget_hover;
    w.hovered.weak_bg_fill = p.widget_hover;
    w.hovered.bg_stroke = Stroke::new(1.5, p.link);
    w.hovered.fg_stroke = Stroke::new(1.5, p.on_hover);
    w.active.bg_fill = p.pressed;
    w.active.weak_bg_fill = p.pressed;
    w.active.bg_stroke = Stroke::new(1.5, p.link);
    w.active.fg_stroke = Stroke::new(2.0, p.selected_text);
    w.open.bg_fill = p.panel_hi;
    w.open.weak_bg_fill = p.panel_hi;
    w.open.bg_stroke = Stroke::new(1.0, p.border);
    w.open.fg_stroke = Stroke::new(1.0, p.text);
    for s in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        s.corner_radius = r;
    }
    v
}

// ---- The palette in use --------------------------------------------------------------------

static PAL: RwLock<Palette> = RwLock::new(DARK);

/// The palette every window paints with now.
pub fn pal() -> Palette {
    *PAL.read().unwrap_or_else(|e| e.into_inner())
}

fn set_pal(p: Palette) {
    *PAL.write().unwrap_or_else(|e| e.into_inner()) = p;
}

/// What the Linux portal last said (it is watched on a thread of its own).
static PORTAL: Mutex<SystemLook> = Mutex::new(SystemLook { dark: None, contrast_colors: None, more_contrast: false, accent: None });
static CTX: OnceLock<egui::Context> = OnceLock::new();

/// The desktop's accent colour, if it gives one (Linux's portal, macOS's accent colour).
pub fn system_accent() -> Option<Color32> {
    PORTAL.lock().unwrap_or_else(|e| e.into_inner()).accent.or_else(mac_accent)
}

/// Ask the desktop for its colour scheme, contrast and accent colour before the first frame
/// (Linux: the Settings portal, at most a second), then keep listening for changes.
pub fn start() {
    #[cfg(target_os = "linux")]
    linux::start();
}

/// What the system says right now. Cheap enough to call every frame.
fn system_look(ctx: &egui::Context) -> SystemLook {
    let portal = *PORTAL.lock().unwrap_or_else(|e| e.into_inner());
    let winit = ctx.system_theme().map(|t| t == egui::Theme::Dark);
    SystemLook {
        // The portal knows best on Linux (winit often can't tell there).
        dark: portal.dark.or(winit),
        contrast_colors: windows_contrast(),
        more_contrast: portal.more_contrast || mac_more_contrast(),
        accent: portal.accent.or_else(mac_accent),
    }
}

/// What was last applied, so the style is only rebuilt when something changed.
static APPLIED: Mutex<Option<(ThemeChoice, SystemLook)>> = Mutex::new(None);

/// The note Settings shows under Appearance when the system asks for contrast (English).
pub fn contrast_note() -> Option<&'static str> {
    let applied = *APPLIED.lock().unwrap_or_else(|e| e.into_inner());
    note_for(&applied?.1)
}

fn note_for(sys: &SystemLook) -> Option<&'static str> {
    if sys.contrast_colors.is_some() {
        Some("A contrast theme is on, so Snagster uses its colors.")
    } else if sys.more_contrast && cfg!(target_os = "macos") {
        Some("Increase contrast is on in System Settings, so text and borders are stronger.")
    } else if sys.more_contrast {
        Some("The system asks for more contrast, so text and borders are stronger.")
    } else {
        None
    }
}

/// Pick the palette for `choice` and what the system says, and give it to egui and the title
/// bars. Call at the start of every frame (it does nothing when nothing changed).
pub fn apply(ctx: &egui::Context, choice: ThemeChoice) {
    let _ = CTX.set(ctx.clone());
    // Windows and macOS send no event when contrast is switched on or off: look again now and then.
    if cfg!(any(target_os = "windows", target_os = "macos")) {
        ctx.request_repaint_after(std::time::Duration::from_secs(2));
    }
    let sys = system_look(ctx);
    let mut applied = APPLIED.lock().unwrap_or_else(|e| e.into_inner());
    if *applied == Some((choice, sys)) {
        return;
    }
    *applied = Some((choice, sys));
    let p = choose(choice, &sys);
    set_pal(p);
    let v = visuals(&p);
    // Both of egui's styles get the palette, so whichever egui picks it looks the same.
    ctx.set_visuals_of(egui::Theme::Dark, v.clone());
    ctx.set_visuals_of(egui::Theme::Light, v);
    ctx.set_theme(egui_preference(choice, &sys, p.dark));
    ctx.options_mut(|o| o.sync_window_theme = true);
    ctx.request_repaint();
}

/// What egui is told, which also sets the title bars. "System" keeps winit following the system
/// (a fixed theme would stop it hearing changes); the palette decides otherwise.
fn egui_preference(choice: ThemeChoice, sys: &SystemLook, dark: bool) -> egui::ThemePreference {
    let winit_knows = cfg!(any(target_os = "windows", target_os = "macos"));
    if choice == ThemeChoice::System && winit_knows && sys.contrast_colors.is_none() {
        egui::ThemePreference::System
    } else if dark {
        egui::ThemePreference::Dark
    } else {
        egui::ThemePreference::Light
    }
}

/// Windows: the colours of a contrast theme, when one is on.
#[cfg(target_os = "windows")]
fn windows_contrast() -> Option<SystemColors> {
    use windows_sys::Win32::Graphics::Gdi::{
        COLOR_BTNFACE, COLOR_BTNTEXT, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_HOTLIGHT, COLOR_WINDOW, COLOR_WINDOWTEXT, GetSysColor,
    };
    use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETHIGHCONTRAST, SystemParametersInfoW};
    let mut hc = HIGHCONTRASTW { cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32, dwFlags: 0, lpszDefaultScheme: std::ptr::null_mut() };
    // SAFETY: SPI_GETHIGHCONTRAST fills the HIGHCONTRASTW whose size we pass.
    let ok = unsafe { SystemParametersInfoW(SPI_GETHIGHCONTRAST, hc.cbSize, &mut hc as *mut _ as *mut _, 0) };
    if ok == 0 || hc.dwFlags & HCF_HIGHCONTRASTON == 0 {
        return None;
    }
    // SAFETY: GetSysColor only reads a system colour.
    let get = |i| {
        let c = unsafe { GetSysColor(i) };
        Color32::from_rgb((c & 0xff) as u8, (c >> 8 & 0xff) as u8, (c >> 16 & 0xff) as u8)
    };
    Some(SystemColors {
        window: get(COLOR_WINDOW),
        window_text: get(COLOR_WINDOWTEXT),
        highlight: get(COLOR_HIGHLIGHT),
        highlight_text: get(COLOR_HIGHLIGHTTEXT),
        button: get(COLOR_BTNFACE),
        button_text: get(COLOR_BTNTEXT),
        gray_text: get(COLOR_GRAYTEXT),
        link: get(COLOR_HOTLIGHT),
    })
}

#[cfg(not(target_os = "windows"))]
fn windows_contrast() -> Option<SystemColors> {
    None
}

/// macOS: System Settings → Accessibility → Display → Increase contrast.
#[cfg(target_os = "macos")]
fn mac_more_contrast() -> bool {
    objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldIncreaseContrast()
}

#[cfg(not(target_os = "macos"))]
fn mac_more_contrast() -> bool {
    false
}

/// macOS: the accent colour picked in System Settings → Appearance.
#[cfg(target_os = "macos")]
fn mac_accent() -> Option<Color32> {
    use objc2_app_kit::{NSColor, NSColorSpace};
    let c = NSColor::controlAccentColor().colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())?;
    accent_from(c.redComponent(), c.greenComponent(), c.blueComponent())
}

#[cfg(not(target_os = "macos"))]
fn mac_accent() -> Option<Color32> {
    None
}

/// The portal's colour components are 0..1; out of range means "no accent set".
#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn accent_from(r: f64, g: f64, b: f64) -> Option<Color32> {
    let ok = |v: f64| (0.0..=1.0).contains(&v);
    (ok(r) && ok(g) && ok(b)).then(|| Color32::from_rgb((r * 255.0).round() as u8, (g * 255.0).round() as u8, (b * 255.0).round() as u8))
}

/// The portal's colour-scheme answer: 1 prefers dark; 0 (GNOME's "Default", no preference) and
/// 2 are light, as libadwaita apps show them. No answer at all (None) is no preference.
#[cfg(any(target_os = "linux", test))]
fn dark_from_scheme(scheme: Option<u32>) -> Option<bool> {
    scheme.map(|s| s == 1)
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use ashpd::desktop::settings::{ColorScheme, Contrast, Settings};

    async fn read(s: &Settings) -> SystemLook {
        let scheme = match s.color_scheme().await {
            Ok(ColorScheme::PreferDark) => Some(1),
            Ok(ColorScheme::PreferLight) => Some(2),
            Ok(_) => Some(0),
            Err(_) => None,
        };
        let accent = s.accent_color().await.ok().and_then(|c| accent_from(c.red(), c.green(), c.blue()));
        SystemLook {
            dark: dark_from_scheme(scheme),
            contrast_colors: None,
            more_contrast: matches!(s.contrast().await, Ok(Contrast::High)),
            accent,
        }
    }

    fn store(look: SystemLook) {
        *PORTAL.lock().unwrap_or_else(|e| e.into_inner()) = look;
        if let Some(ctx) = CTX.get() {
            ctx.request_repaint();
        }
    }

    pub fn start() {
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = std::thread::Builder::new().name("snagster-appearance".into()).spawn(move || {
            pollster::block_on(async move {
                use futures_util::StreamExt;
                let Ok(s) = Settings::new().await else {
                    let _ = tx.send(());
                    return;
                };
                store(read(&s).await);
                let _ = tx.send(());
                let Ok(changes) = s.receive_setting_changed().await else { return };
                let mut changes = Box::pin(changes);
                while let Some(c) = changes.next().await {
                    if c.namespace() == "org.freedesktop.appearance" {
                        store(read(&s).await);
                    }
                }
            });
        });
        // The first answer before the first frame, but never more than a second's wait.
        let _ = rx.recv_timeout(std::time::Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sys(dark: Option<bool>) -> SystemLook {
        SystemLook { dark, ..Default::default() }
    }

    /// A Windows contrast theme like Night sky (dark) and like Desert (light).
    fn night_sky() -> SystemColors {
        SystemColors {
            window: Color32::BLACK,
            window_text: Color32::WHITE,
            highlight: Color32::from_rgb(209, 147, 255),
            highlight_text: Color32::BLACK,
            button: Color32::BLACK,
            button_text: Color32::WHITE,
            gray_text: Color32::from_rgb(166, 166, 166),
            link: Color32::from_rgb(128, 128, 255),
        }
    }
    fn desert() -> SystemColors {
        SystemColors {
            window: Color32::from_rgb(255, 250, 239),
            window_text: Color32::from_rgb(61, 61, 61),
            highlight: Color32::from_rgb(144, 57, 4),
            highlight_text: Color32::from_rgb(255, 245, 227),
            button: Color32::from_rgb(255, 250, 239),
            button_text: Color32::from_rgb(61, 61, 61),
            gray_text: Color32::from_rgb(103, 103, 103),
            link: Color32::from_rgb(0, 99, 177),
        }
    }

    #[test]
    fn the_choice_wins_and_follow_system_follows() {
        assert!(wants_dark(ThemeChoice::Dark, Some(false)));
        assert!(!wants_dark(ThemeChoice::Light, Some(true)));
        assert!(wants_dark(ThemeChoice::System, Some(true)));
        assert!(!wants_dark(ThemeChoice::System, Some(false)));
        // A system that says nothing keeps Snagster dark, as it always was.
        assert!(wants_dark(ThemeChoice::System, None));
        assert!(!choose(ThemeChoice::System, &sys(Some(false))).dark);
        assert!(choose(ThemeChoice::Dark, &sys(Some(false))).dark);
        assert_eq!(choose(ThemeChoice::Light, &sys(None)).panel, LIGHT.panel);
    }

    #[test]
    fn follow_system_is_the_default() {
        assert_eq!(ThemeChoice::default(), ThemeChoice::System);
        assert_eq!(crate::settings::Settings::default().appearance, ThemeChoice::System);
        // Saved as the names list_settings gives.
        for (c, name) in ThemeChoice::ALL.into_iter().zip(["System", "Light", "Dark"]) {
            assert_eq!(serde_json::to_value(c).unwrap(), serde_json::json!(name));
        }
    }

    #[test]
    fn contrast_themes_use_each_system_colour_for_its_job() {
        let p = from_system(night_sky());
        // Hover in the highlight with its text; links in Hotlight; GrayText only when disabled.
        assert_eq!((p.hover, p.on_hover), (night_sky().highlight, night_sky().highlight_text));
        assert_eq!(p.link, night_sky().link);
        assert_eq!((p.faint, p.disabled), (night_sky().window_text, night_sky().gray_text));
        assert_eq!(visuals(&p).window_stroke.width, 2.0);
        assert_eq!(visuals(&DARK).window_stroke.width, 1.0);
    }

    #[test]
    fn a_contrast_theme_wins_over_every_choice() {
        for c in ThemeChoice::ALL {
            let p = choose(c, &SystemLook { dark: Some(false), contrast_colors: Some(night_sky()), ..Default::default() });
            assert!(p.high_contrast && p.dark);
            assert_eq!((p.panel, p.text, p.accent), (Color32::BLACK, Color32::WHITE, night_sky().highlight));
        }
        let p = choose(ThemeChoice::Dark, &SystemLook { contrast_colors: Some(desert()), ..Default::default() });
        assert!(!p.dark, "Desert is a light contrast theme");
    }

    #[test]
    fn more_contrast_makes_our_palette_stronger() {
        for dark in [true, false] {
            let plain = choose(ThemeChoice::System, &sys(Some(dark)));
            let more = choose(ThemeChoice::System, &SystemLook { dark: Some(dark), more_contrast: true, ..Default::default() });
            assert!(more.high_contrast);
            assert!(contrast(more.text, more.panel) > contrast(plain.text, plain.panel));
            assert!(contrast(more.faint, more.band) >= 7.0);
            assert!(contrast(more.link, more.panel) >= 7.0);
        }
    }

    /// Every pair of text and background the interface paints, in a palette.
    fn text_pairs(p: &Palette) -> Vec<(&'static str, Color32, Color32)> {
        vec![
            ("text on panel", p.text, p.panel),
            ("text on button", p.text, p.panel_hi),
            ("text on hovered button", p.on_hover, p.widget_hover),
            ("text on field", p.text, p.bg),
            ("text on band", p.text, p.band),
            ("text on tabs", p.text, p.tabs),
            ("text on hovered tool", p.on_hover, p.hover),
            ("text on chosen tool", p.selected_text, p.pressed),
            ("dim on panel", p.dim, p.panel),
            ("dim on band", p.dim, p.band),
            ("dim on tabs", p.dim, p.tabs),
            ("faint on band", p.faint, p.band),
            ("faint on panel", p.faint, p.panel),
            ("card text", p.card_text, p.card),
            ("card text hovered", p.card_text, p.card_hover),
            ("link on panel", p.link, p.panel),
            ("link on band", p.link, p.band),
            ("button text", p.button_text, p.panel_hi),
            ("text on accent", p.on_accent, p.accent),
            ("text on a delete button", text_on(p.danger), p.danger),
            ("ok on panel", p.ok, p.panel),
            ("warn on panel", p.warn, p.panel),
            ("err on panel", p.err, p.panel),
        ]
    }

    #[test]
    fn text_meets_wcag_aa_in_every_palette() {
        let mut palettes = vec![("dark", choose(ThemeChoice::Dark, &sys(None))), ("light", choose(ThemeChoice::Light, &sys(None)))];
        for dark in [true, false] {
            palettes.push(("stronger", choose(ThemeChoice::System, &SystemLook { dark: Some(dark), more_contrast: true, ..Default::default() })));
        }
        palettes.push(("night sky", choose(ThemeChoice::System, &SystemLook { contrast_colors: Some(night_sky()), ..Default::default() })));
        palettes.push(("desert", choose(ThemeChoice::System, &SystemLook { contrast_colors: Some(desert()), ..Default::default() })));
        for (name, p) in palettes {
            for (what, fg, bg) in text_pairs(&p) {
                let r = contrast(fg, bg);
                assert!(r >= 4.5, "{name}: {what} is {r:.2}:1, below 4.5:1");
            }
            // The focus ring (hovered/focused widget border) against the window: 3:1.
            assert!(contrast(p.border, p.panel) >= 3.0, "{name}: control edges");
            assert!(contrast(p.link, p.panel) >= 3.0, "{name}: focus ring");
        }
    }

    #[test]
    fn a_desktop_accent_is_made_readable() {
        // GNOME's yellow accent on the light palette, and a dark navy on the dark one.
        for (raw, dark) in [(Color32::from_rgb(200, 136, 0), false), (Color32::from_rgb(20, 30, 90), true)] {
            let p = choose(ThemeChoice::System, &SystemLook { dark: Some(dark), accent: Some(raw), ..Default::default() });
            assert_ne!(p.link, raw);
            assert!(contrast(p.link, p.panel) >= 4.5);
            // White on the accent fill, as GNOME and KDE do.
            assert_eq!(p.on_accent, Color32::WHITE);
            assert!(contrast(p.on_accent, p.accent) >= 4.5, "{:?} on {:?}", p.on_accent, p.accent);
        }
        // One that already reads well is kept as it is.
        let blue = Color32::from_rgb(53, 132, 228);
        assert_eq!(readable_on(blue, Color32::WHITE, 3.0), blue);
    }

    #[test]
    fn contrast_ratio_matches_wcag() {
        assert!((contrast(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 0.001);
        // #767676 on white is the classic 4.54:1.
        assert!((contrast(Color32::from_gray(0x76), Color32::WHITE) - 4.54).abs() < 0.02);
        assert_eq!(text_on(Color32::from_rgb(255, 230, 0)), Color32::BLACK);
        assert_eq!(text_on(Color32::from_rgb(20, 40, 120)), Color32::WHITE);
    }

    #[test]
    fn portal_answers() {
        assert_eq!(dark_from_scheme(Some(1)), Some(true));
        assert_eq!(dark_from_scheme(Some(2)), Some(false));
        // GNOME's "Default" is light, as libadwaita apps show it.
        assert_eq!(dark_from_scheme(Some(0)), Some(false));
        assert_eq!(dark_from_scheme(None), None);
        assert_eq!(accent_from(0.0, 0.5, 1.0), Some(Color32::from_rgb(0, 128, 255)));
        assert_eq!(accent_from(0.207, 0.517, 0.894), Some(Color32::from_rgb(53, 132, 228)));
        assert_eq!(accent_from(-1.0, 0.0, 0.0), None);
    }

    #[test]
    fn visuals_carry_the_palette() {
        for p in [DARK, LIGHT] {
            let v = visuals(&p);
            assert_eq!(v.dark_mode, p.dark);
            assert_eq!(v.panel_fill, p.panel);
            assert_eq!(v.selection.bg_fill, p.accent);
            assert_eq!(v.widgets.noninteractive.fg_stroke.color, p.text);
            assert_eq!(v.widgets.hovered.bg_stroke.color, p.link);
            assert_eq!(v.hyperlink_color, p.link);
        }
    }

    #[test]
    fn settings_say_why_the_colours_changed() {
        assert_eq!(note_for(&sys(Some(true))), None);
        assert!(note_for(&SystemLook { more_contrast: true, ..Default::default() }).is_some());
        assert!(note_for(&SystemLook { contrast_colors: Some(desert()), ..Default::default() }).unwrap().contains("contrast theme"));
    }

    #[test]
    fn title_bars_follow_the_system_only_when_asked_to() {
        let s = sys(Some(true));
        assert_eq!(egui_preference(ThemeChoice::Light, &s, false), egui::ThemePreference::Light);
        assert_eq!(egui_preference(ThemeChoice::Dark, &s, true), egui::ThemePreference::Dark);
        let hc = SystemLook { contrast_colors: Some(night_sky()), ..Default::default() };
        assert_eq!(egui_preference(ThemeChoice::System, &hc, true), egui::ThemePreference::Dark);
        let expect = if cfg!(any(target_os = "windows", target_os = "macos")) { egui::ThemePreference::System } else { egui::ThemePreference::Dark };
        assert_eq!(egui_preference(ThemeChoice::System, &s, true), expect);
    }

    /// The black swatch on the dark side panel and the white (and yellow) ones on the light
    /// panel get an edge that stands 3:1; the red one, which stands out on both, gets none.
    /// In high contrast every swatch has an edge.
    #[test]
    fn swatches_that_melt_into_the_panel_get_an_edge() {
        let [r, .., black, white] = crate::settings::DEFAULT_PALETTE.map(|c| Color32::from_rgb(c[0], c[1], c[2]));
        let yellow = Color32::from_rgb(255, 197, 61);
        for (p, faint) in [(DARK, vec![black]), (LIGHT, vec![white, yellow])] {
            for behind in [p.panel, p.panel_hi] {
                for c in &faint {
                    let edge = swatch_edge(*c, behind, &p).unwrap_or_else(|| panic!("{c:?} on {behind:?} has no edge"));
                    assert!(contrast(edge, behind) >= 3.0, "{edge:?} on {behind:?}");
                }
                assert_eq!(swatch_edge(r, behind, &p), None, "red on {behind:?}");
            }
        }
        for p in [stronger(DARK), stronger(LIGHT), from_system(night_sky()), from_system(desert())] {
            for c in [r, black, white] {
                let edge = swatch_edge(c, p.panel, &p).expect("high contrast outlines every swatch");
                assert!(contrast(edge, p.panel) >= 3.0, "{edge:?} on {:?}", p.panel);
            }
        }
    }
}
