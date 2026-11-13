//! Everything the person can set, saved as JSON in their app-data folder.

use crate::effects::Effect;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Bmp,
    Gif,
    Webp,
    Tiff,
    Pdf,
    Svg,
}

impl ImageFormat {
    pub const ALL: [ImageFormat; 8] = [
        ImageFormat::Png,
        ImageFormat::Jpeg,
        ImageFormat::Bmp,
        ImageFormat::Gif,
        ImageFormat::Webp,
        ImageFormat::Tiff,
        ImageFormat::Pdf,
        ImageFormat::Svg,
    ];
    pub fn ext(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Bmp => "bmp",
            ImageFormat::Gif => "gif",
            ImageFormat::Webp => "webp",
            ImageFormat::Tiff => "tiff",
            ImageFormat::Pdf => "pdf",
            ImageFormat::Svg => "svg",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ImageFormat::Png => "PNG",
            ImageFormat::Jpeg => "JPEG",
            ImageFormat::Bmp => "BMP",
            ImageFormat::Gif => "GIF",
            ImageFormat::Webp => "WebP",
            ImageFormat::Tiff => "TIFF",
            ImageFormat::Pdf => "PDF",
            ImageFormat::Svg => "SVG",
        }
    }
}

/// A key combination for a global hotkey. An empty `key` means "not set".
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Combo {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    pub key: String,
}

impl Combo {
    pub fn new(ctrl: bool, alt: bool, shift: bool, key: &str) -> Self {
        Self { ctrl, alt, shift, win: false, key: key.into() }
    }
    pub fn is_set(&self) -> bool {
        !self.key.is_empty()
    }
    pub fn label(&self) -> String {
        if !self.is_set() {
            return "Not set".into();
        }
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.win {
            parts.push("Win");
        }
        parts.push(&self.key);
        parts.join(" + ")
    }
}

/// Things a global hotkey can do.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum Action {
    AllInOne,
    Area,
    Window,
    Screen,
    ActiveWindow,
    AllScreens,
    LastArea,
    Scrolling,
    Panorama,
    Circle,
    Freehand,
    Polygon,
    MultiArea,
    MenuCapture,
    ManualScroll,
    GrabText,
    ColorPicker,
    Ruler,
    RecordGif,
    RecordSteps,
    ShowSnag,
}

impl Action {
    pub const ALL: [Action; 21] = [
        Action::AllInOne,
        Action::Area,
        Action::Window,
        Action::Screen,
        Action::ActiveWindow,
        Action::AllScreens,
        Action::LastArea,
        Action::Scrolling,
        Action::Panorama,
        Action::Circle,
        Action::Freehand,
        Action::Polygon,
        Action::MultiArea,
        Action::MenuCapture,
        Action::ManualScroll,
        Action::GrabText,
        Action::ColorPicker,
        Action::Ruler,
        Action::RecordGif,
        Action::RecordSteps,
        Action::ShowSnag,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Action::AllInOne => "All-in-one capture",
            Action::Area => "Capture an area",
            Action::Window => "Capture a window (click it)",
            Action::Screen => "Capture the screen under the mouse",
            Action::ActiveWindow => "Capture the window in front",
            Action::AllScreens => "Capture all screens",
            Action::LastArea => "Repeat the last area",
            Action::Scrolling => "Scrolling capture (down)",
            Action::Panorama => "Panoramic capture (sideways)",
            Action::Circle => "Circle capture",
            Action::Freehand => "Freehand capture",
            Action::Polygon => "Polygon capture",
            Action::MultiArea => "Capture several areas at once",
            Action::MenuCapture => "Capture an open menu (with sub-menus)",
            Action::ManualScroll => "Scrolling capture (you scroll)",
            Action::GrabText => "Grab text from the screen",
            Action::ColorPicker => "Screen color picker",
            Action::Ruler => "Screen ruler",
            Action::RecordGif => "Start / stop GIF recording",
            Action::RecordSteps => "Start / stop step recording",
            Action::ShowSnag => "Show Snagster",
        }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Binding {
    pub action: Action,
    pub combo: Combo,
}

pub fn default_bindings() -> Vec<Binding> {
    let ps = "Print Screen";
    let b = |action, combo| Binding { action, combo };
    vec![
        b(Action::AllInOne, Combo::new(false, false, false, ps)),
        b(Action::Screen, Combo::new(true, false, false, ps)),
        b(Action::ActiveWindow, Combo::new(false, true, false, ps)),
        b(Action::AllScreens, Combo::new(false, false, true, ps)),
        b(Action::LastArea, Combo::new(true, false, true, ps)),
        b(Action::GrabText, Combo::new(true, false, true, "F9")),
        b(Action::Scrolling, Combo::new(true, false, true, "F10")),
        b(Action::RecordGif, Combo::new(true, false, true, "F11")),
        b(Action::RecordSteps, Combo::new(true, false, true, "F12")),
        b(Action::Area, Combo::default()),
        b(Action::Window, Combo::default()),
        b(Action::Panorama, Combo::default()),
        b(Action::Circle, Combo::default()),
        b(Action::Freehand, Combo::default()),
        b(Action::Polygon, Combo::default()),
        b(Action::MultiArea, Combo::default()),
        b(Action::MenuCapture, Combo::default()),
        b(Action::ManualScroll, Combo::default()),
        b(Action::ColorPicker, Combo::default()),
        b(Action::Ruler, Combo::default()),
        b(Action::ShowSnag, Combo::default()),
    ]
}

/// How a preset captures.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum PresetMode {
    Area,
    Window,
    Screen,
    AllScreens,
    FixedSize,
    Circle,
    Freehand,
    Star,
    Heart,
    Scrolling,
}

impl PresetMode {
    pub const ALL: [PresetMode; 10] = [
        PresetMode::Area,
        PresetMode::Window,
        PresetMode::Screen,
        PresetMode::AllScreens,
        PresetMode::FixedSize,
        PresetMode::Circle,
        PresetMode::Freehand,
        PresetMode::Star,
        PresetMode::Heart,
        PresetMode::Scrolling,
    ];
    pub fn label(self) -> &'static str {
        match self {
            PresetMode::Area => "Area",
            PresetMode::Window => "Window",
            PresetMode::Screen => "Screen",
            PresetMode::AllScreens => "All screens",
            PresetMode::FixedSize => "Fixed size",
            PresetMode::Circle => "Circle",
            PresetMode::Freehand => "Freehand",
            PresetMode::Star => "Star",
            PresetMode::Heart => "Heart",
            PresetMode::Scrolling => "Scrolling",
        }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Preset {
    pub name: String,
    pub mode: PresetMode,
    pub delay_s: u64,
    pub width: u32,
    pub height: u32,
    pub effects: Vec<Effect>,
    pub copy: bool,
    pub save: bool,
    pub pin: bool,
    pub combo: Combo,
}

impl Default for Preset {
    fn default() -> Self {
        Self {
            name: "New preset".into(),
            mode: PresetMode::Area,
            delay_s: 0,
            width: 1280,
            height: 720,
            effects: Vec::new(),
            copy: true,
            save: false,
            pin: false,
            combo: Combo::default(),
        }
    }
}

pub fn default_presets() -> Vec<Preset> {
    vec![
        Preset {
            name: "Area with border and shadow".into(),
            effects: vec![Effect::Border, Effect::DropShadow],
            ..Default::default()
        },
        Preset {
            name: "1280 x 720 frame".into(),
            mode: PresetMode::FixedSize,
            ..Default::default()
        },
        Preset { name: "Round avatar".into(), mode: PresetMode::Circle, ..Default::default() },
    ]
}

/// A named set of colours and a font, so marks look the same every time.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub name: String,
    pub palette: [[u8; 3]; 8],
    pub font: String,
    pub text_background: bool,
    pub shadow: bool,
}

pub const DEFAULT_PALETTE: [[u8; 3]; 8] = [
    [229, 72, 77],
    [247, 107, 21],
    [255, 197, 61],
    [48, 164, 108],
    [0, 144, 255],
    [142, 78, 198],
    [28, 28, 32],
    [255, 255, 255],
];

impl Default for Theme {
    fn default() -> Self {
        Self { name: "Snagster".into(), palette: DEFAULT_PALETTE, font: String::new(), text_background: false, shadow: false }
    }
}

pub fn default_themes() -> Vec<Theme> {
    vec![
        Theme::default(),
        Theme {
            name: "Calm blue".into(),
            palette: [
                [0, 102, 204],
                [0, 153, 204],
                [102, 178, 255],
                [0, 170, 136],
                [255, 170, 0],
                [90, 90, 110],
                [20, 30, 50],
                [255, 255, 255],
            ],
            font: String::new(),
            text_background: true,
            shadow: true,
        },
        Theme {
            name: "Neon".into(),
            palette: [
                [255, 0, 128],
                [255, 230, 0],
                [0, 255, 170],
                [0, 200, 255],
                [170, 0, 255],
                [255, 100, 0],
                [10, 10, 10],
                [255, 255, 255],
            ],
            font: String::new(),
            text_background: false,
            shadow: true,
        },
    ]
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum StepStyle {
    Numbers,
    Upper,
    Lower,
    Roman,
}

impl StepStyle {
    pub const ALL: [StepStyle; 4] = [StepStyle::Numbers, StepStyle::Upper, StepStyle::Lower, StepStyle::Roman];
    pub fn label(self) -> &'static str {
        match self {
            StepStyle::Numbers => "1, 2, 3",
            StepStyle::Upper => "A, B, C",
            StepStyle::Lower => "a, b, c",
            StepStyle::Roman => "i, ii, iii",
        }
    }
    pub fn format(self, n: u32) -> String {
        match self {
            StepStyle::Numbers => n.to_string(),
            StepStyle::Upper | StepStyle::Lower => {
                let mut n = n.max(1);
                let mut s = Vec::new();
                while n > 0 {
                    n -= 1;
                    s.push((b'A' + (n % 26) as u8) as char);
                    n /= 26;
                }
                let s: String = s.into_iter().rev().collect();
                if self == StepStyle::Lower { s.to_lowercase() } else { s }
            }
            StepStyle::Roman => {
                let mut n = n.clamp(1, 3999);
                let table = [
                    (1000, "m"),
                    (900, "cm"),
                    (500, "d"),
                    (400, "cd"),
                    (100, "c"),
                    (90, "xc"),
                    (50, "l"),
                    (40, "xl"),
                    (10, "x"),
                    (9, "ix"),
                    (5, "v"),
                    (4, "iv"),
                    (1, "i"),
                ];
                let mut s = String::new();
                for (v, r) in table {
                    while n >= v {
                        s.push_str(r);
                        n -= v;
                    }
                }
                s
            }
        }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    // After capture
    pub after_show_editor: bool,
    pub after_copy: bool,
    pub after_save: bool,
    pub after_pin: bool,
    pub after_library: bool,
    // Saving
    pub save_folder: String,
    pub name_pattern: String,
    pub format: ImageFormat,
    pub jpeg_quality: u8,
    pub save_counter: u32,
    pub library_folder: String,
    // Capture
    pub delay_s: u64,
    pub hide_window: bool,
    pub hide_settle_ms: u64,
    pub show_magnifier: bool,
    pub magnifier_zoom: u32,
    pub show_crosshair: bool,
    pub show_info: bool,
    pub dim_alpha: u8,
    pub detect_windows: bool,
    pub detect_objects: bool,
    pub include_cursor: bool,
    pub scroll_max_steps: u32,
    pub scroll_wait_ms: u64,
    // Timer
    pub interval_s: u64,
    pub interval_count: u32,
    pub schedule_time: String,
    // Recording
    pub gif_fps: u32,
    pub gif_max_s: u32,
    pub gif_max_width: u32,
    pub gif_cursor: bool,
    pub slideshow_s: f32,
    // Editor defaults
    pub color: [u8; 3],
    pub palette: [[u8; 3]; 8],
    pub font: String,
    pub width: f32,
    pub fill: bool,
    pub corner_radius: f32,
    pub shadow: bool,
    pub text_background: bool,
    pub blur_block: u32,
    pub step_start: u32,
    pub step_style: StepStyle,
    pub magnify_zoom: f32,
    pub themes: Vec<Theme>,
    // Image effects
    pub border_width: u32,
    pub border_color: [u8; 3],
    pub shadow_size: u32,
    pub rounded_radius: u32,
    pub resize_percent: u32,
    pub watermark_text: String,
    pub tilt_percent: u32,
    pub brightness: i32,
    pub contrast: i32,
    pub saturation: i32,
    // Smart redact
    pub redact_emails: bool,
    pub redact_phones: bool,
    pub redact_cards: bool,
    pub redact_addresses: bool,
    pub redact_ips: bool,
    pub redact_urls: bool,
    // Hotkeys
    pub hotkeys_enabled: bool,
    pub bindings: Vec<Binding>,
    pub presets: Vec<Preset>,
    // View
    pub show_side_panel: bool,
    pub show_tray: bool,
    // Tray and capture panel
    pub tray_icon: bool,
    pub close_to_tray: bool,
    pub panel: PanelSettings,
    // Round 5
    pub override_hotkeys: bool,
    pub start_mode: StartMode,
    pub blur_mode: BlurMode,
    pub arrow_heads: u8,
    pub text_outline: bool,
    pub text_align: u8,
    pub callout_shape: CalloutShape,
    pub callout_outline: bool,
    pub quick_styles: Vec<QuickStyle>,
    pub hidden_tools: Vec<String>,
    pub recent_files: Vec<String>,
    pub menu_delay_s: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            after_show_editor: true,
            after_copy: false,
            after_save: false,
            after_pin: false,
            after_library: true,
            save_folder: pictures().join("Snagster").to_string_lossy().to_string(),
            name_pattern: "Snagster_%Y-%m-%d_%H-%M-%S".into(),
            format: ImageFormat::Png,
            jpeg_quality: 90,
            save_counter: 1,
            library_folder: pictures().join("Snagster").join("Library").to_string_lossy().to_string(),
            delay_s: 0,
            hide_window: true,
            hide_settle_ms: 350,
            show_magnifier: true,
            magnifier_zoom: 8,
            show_crosshair: true,
            show_info: true,
            dim_alpha: 110,
            detect_windows: true,
            detect_objects: true,
            include_cursor: false,
            scroll_max_steps: 40,
            scroll_wait_ms: 350,
            interval_s: 10,
            interval_count: 6,
            schedule_time: "12:00".into(),
            gif_fps: 10,
            gif_max_s: 30,
            gif_max_width: 1280,
            gif_cursor: true,
            slideshow_s: 2.0,
            color: DEFAULT_PALETTE[0],
            palette: DEFAULT_PALETTE,
            font: String::new(),
            width: 6.0,
            fill: false,
            corner_radius: 0.0,
            shadow: false,
            text_background: false,
            blur_block: 12,
            step_start: 1,
            step_style: StepStyle::Numbers,
            magnify_zoom: 2.0,
            themes: default_themes(),
            border_width: 8,
            border_color: [28, 28, 32],
            shadow_size: 24,
            rounded_radius: 16,
            resize_percent: 50,
            watermark_text: "Made with Snagster".into(),
            tilt_percent: 20,
            brightness: 10,
            contrast: 10,
            saturation: 20,
            redact_emails: true,
            redact_phones: true,
            redact_cards: true,
            redact_addresses: true,
            redact_ips: true,
            redact_urls: false,
            hotkeys_enabled: true,
            bindings: default_bindings(),
            presets: default_presets(),
            show_side_panel: true,
            show_tray: true,
            tray_icon: true,
            close_to_tray: true,
            panel: PanelSettings::default(),
            override_hotkeys: true,
            start_mode: StartMode::Editor,
            blur_mode: BlurMode::Pixelate,
            arrow_heads: 1,
            text_outline: true,
            text_align: 0,
            callout_shape: CalloutShape::Rounded,
            callout_outline: false,
            quick_styles: default_quick_styles(),
            hidden_tools: Vec::new(),
            recent_files: Vec::new(),
            menu_delay_s: 3,
        }
    }
}

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn pictures() -> PathBuf {
    home().join("Pictures")
}

/// Folders that sync to the cloud, when the person has them.
pub fn cloud_folders() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for var in ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"] {
        if let Some(p) = std::env::var_os(var).map(PathBuf::from)
            && p.is_dir()
            && !out.iter().any(|(_, q): &(String, PathBuf)| *q == p)
        {
            out.push(("OneDrive".to_string(), p));
        }
    }
    let h = home();
    for (name, p) in [
        ("Dropbox", h.join("Dropbox")),
        ("Google Drive", h.join("Google Drive")),
        ("Google Drive", PathBuf::from("G:\\My Drive")),
        ("Box", h.join("Box")),
        ("iCloud Drive", h.join("iCloudDrive")),
    ] {
        if p.is_dir() {
            out.push((name.to_string(), p));
        }
    }
    out
}

fn settings_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("Snagster").join("settings.json"))
}

impl Settings {
    /// Load the saved settings. `fresh` skips the file (used for the check run).
    pub fn load(fresh: bool) -> Self {
        if fresh {
            return Self::default();
        }
        // Settings from before the rename (the "Snag" folder) are carried over.
        let old = settings_path().and_then(|p| Some(p.parent()?.parent()?.join("Snag").join("settings.json")));
        let mut s: Settings = settings_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .or_else(|| old.and_then(|p| std::fs::read_to_string(p).ok()))
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        // Settings saved by an older Snagster may lack newer actions.
        for d in default_bindings() {
            if !s.bindings.iter().any(|b| b.action == d.action) {
                s.bindings.push(d);
            }
        }
        s
    }

    pub fn save(&self) {
        if let Some(p) = settings_path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(s) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(p, s);
            }
        }
    }

    pub fn binding(&self, a: Action) -> Combo {
        self.bindings.iter().find(|b| b.action == a).map(|b| b.combo.clone()).unwrap_or_default()
    }

    /// Fill in the file-name pattern: %Y %m %d %H %M %S date and time, %n counter, %t title.
    pub fn file_name(&mut self, title: &str) -> String {
        let now = chrono::Local::now();
        let mut out = String::new();
        let mut chars = self.name_pattern.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some(t @ ('Y' | 'm' | 'd' | 'H' | 'M' | 'S')) => out.push_str(&now.format(&format!("%{t}")).to_string()),
                Some('n') => {
                    out.push_str(&format!("{:03}", self.save_counter));
                    self.save_counter += 1;
                }
                Some('t') => out.push_str(title),
                Some(other) => {
                    out.push('%');
                    out.push(other);
                }
                None => out.push('%'),
            }
        }
        let cleaned: String = out
            .chars()
            .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
            .collect();
        if cleaned.trim().is_empty() { "Snagster".into() } else { cleaned }
    }

    /// A path in the save folder that doesn't exist yet.
    pub fn next_save_path(&mut self, title: &str) -> PathBuf {
        let folder = PathBuf::from(&self.save_folder);
        let base = self.file_name(title);
        let ext = self.format.ext();
        unique_path(&folder, &base, ext)
    }
}

pub fn unique_path(folder: &std::path::Path, base: &str, ext: &str) -> PathBuf {
    let mut p = folder.join(format!("{base}.{ext}"));
    let mut k = 2;
    while p.exists() {
        p = folder.join(format!("{base} ({k}).{ext}"));
        k += 1;
    }
    p
}

/// What the capture panel captures.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum PanelSelection {
    Region,
    Window,
    Screen,
    AllScreens,
    Scrolling,
    Panorama,
    Menu,
    Circle,
    Freehand,
    Polygon,
    Star,
    Heart,
    FixedSize,
    Multi,
    MenuDelayed,
    ScrollYourself,
    GrabText,
}

impl PanelSelection {
    pub const IMAGE: [PanelSelection; 17] = [
        PanelSelection::Region,
        PanelSelection::Window,
        PanelSelection::Screen,
        PanelSelection::AllScreens,
        PanelSelection::Scrolling,
        PanelSelection::Panorama,
        PanelSelection::Menu,
        PanelSelection::Circle,
        PanelSelection::Freehand,
        PanelSelection::Polygon,
        PanelSelection::Star,
        PanelSelection::Heart,
        PanelSelection::FixedSize,
        PanelSelection::Multi,
        PanelSelection::MenuDelayed,
        PanelSelection::ScrollYourself,
        PanelSelection::GrabText,
    ];
    pub const VIDEO: [PanelSelection; 3] = [PanelSelection::Region, PanelSelection::Window, PanelSelection::Screen];
    pub fn label(self) -> &'static str {
        match self {
            PanelSelection::Region => "Region",
            PanelSelection::Window => "Window",
            PanelSelection::Screen => "Full screen",
            PanelSelection::AllScreens => "All screens",
            PanelSelection::Scrolling => "Scrolling (down)",
            PanelSelection::Panorama => "Panoramic (sideways)",
            PanelSelection::Menu => "Menu or button",
            PanelSelection::Circle => "Circle",
            PanelSelection::Freehand => "Freehand",
            PanelSelection::Polygon => "Polygon",
            PanelSelection::Star => "Star",
            PanelSelection::Heart => "Heart",
            PanelSelection::FixedSize => "Fixed size",
            PanelSelection::Multi => "Multiple areas",
            PanelSelection::MenuDelayed => "Menu with sub-menus",
            PanelSelection::ScrollYourself => "Scrolling (you scroll)",
            PanelSelection::GrabText => "Grab text",
        }
    }
}

/// Where the capture panel sends a capture.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum PanelShare {
    None,
    Clipboard,
    SaveFolder,
    Pin,
    OpenWith,
    Cloud,
}

impl PanelShare {
    pub const ALL: [PanelShare; 6] =
        [PanelShare::None, PanelShare::Clipboard, PanelShare::SaveFolder, PanelShare::Pin, PanelShare::OpenWith, PanelShare::Cloud];
    pub fn label(self) -> &'static str {
        match self {
            PanelShare::None => "None",
            PanelShare::Clipboard => "Clipboard",
            PanelShare::SaveFolder => "Save to folder",
            PanelShare::Pin => "Pin to screen",
            PanelShare::OpenWith => "Open in another program",
            PanelShare::Cloud => "Cloud folder",
        }
    }
}

/// The capture panel's own choices.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelSettings {
    pub tab: u8,
    pub selection: PanelSelection,
    pub video_selection: PanelSelection,
    pub effect: Option<Effect>,
    pub share: PanelShare,
    pub delay_on: bool,
    pub delay_s: u64,
    pub fixed_w: u32,
    pub fixed_h: u32,
}

impl Default for PanelSettings {
    fn default() -> Self {
        Self {
            tab: 0,
            selection: PanelSelection::Region,
            video_selection: PanelSelection::Region,
            effect: None,
            share: PanelShare::None,
            delay_on: false,
            delay_s: 5,
            fixed_w: 1280,
            fixed_h: 720,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum StartMode {
    Editor,
    Panel,
    Tray,
}

impl StartMode {
    pub const ALL: [StartMode; 3] = [StartMode::Editor, StartMode::Panel, StartMode::Tray];
    pub fn label(self) -> &'static str {
        match self {
            StartMode::Editor => "the editor",
            StartMode::Panel => "the capture panel",
            StartMode::Tray => "only the icon in the taskbar corner",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum BlurMode {
    Pixelate,
    Blur,
    Solid,
}

impl BlurMode {
    pub const ALL: [BlurMode; 3] = [BlurMode::Pixelate, BlurMode::Blur, BlurMode::Solid];
    pub fn label(self) -> &'static str {
        match self {
            BlurMode::Pixelate => "Pixelate",
            BlurMode::Blur => "Blur",
            BlurMode::Solid => "Solid color",
        }
    }
}

/// A saved look for marks: colour, width and the switches, one click away.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct QuickStyle {
    pub color: [u8; 3],
    pub width: f32,
    pub shadow: bool,
    pub fill: bool,
    pub text_background: bool,
}

impl Default for QuickStyle {
    fn default() -> Self {
        Self { color: DEFAULT_PALETTE[0], width: 6.0, shadow: false, fill: false, text_background: false }
    }
}

pub fn default_quick_styles() -> Vec<QuickStyle> {
    vec![
        QuickStyle { color: [229, 72, 77], width: 6.0, shadow: true, fill: false, text_background: false },
        QuickStyle { color: [0, 144, 255], width: 4.0, shadow: false, fill: false, text_background: true },
        QuickStyle { color: [255, 197, 61], width: 8.0, shadow: true, fill: false, text_background: true },
        QuickStyle { color: [28, 28, 32], width: 3.0, shadow: false, fill: false, text_background: false },
        QuickStyle { color: [48, 164, 108], width: 10.0, shadow: true, fill: true, text_background: true },
    ]
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
pub enum CalloutShape {
    #[default]
    Rounded,
    Rectangle,
    Oval,
}
