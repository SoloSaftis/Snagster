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
            return crate::i18n::t("Not set").into();
        }
        // Written the way this system writes shortcuts (Super on Linux, ⌃⌥⇧⌘ on a Mac).
        crate::platform::hotkey_label(self.ctrl, self.alt, self.shift, self.win, &self.key)
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
    SaveReplay,
    ToggleReplay,
    DrawInk,
    ClearInk,
}

impl Action {
    pub const ALL: [Action; 25] = [
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
        Action::SaveReplay,
        Action::ToggleReplay,
        Action::DrawInk,
        Action::ClearInk,
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
            Action::RecordGif => "Start / stop recording (video or GIF)",
            Action::RecordSteps => "Start / stop step recording",
            Action::ShowSnag => "Show Snagster",
            Action::SaveReplay => "Save replay",
            Action::ToggleReplay => "Turn replay on / off",
            Action::DrawInk => "Draw on screen",
            Action::ClearInk => "Clear ink",
        }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Binding {
    pub action: Action,
    pub combo: Combo,
}

pub fn default_bindings() -> Vec<Binding> {
    default_bindings_for(crate::platform::system() == crate::platform::System::Mac)
}

/// The hotkeys Snagster starts with. A Mac keyboard has no Print Screen key and Command is
/// its main modifier, so the Mac gets Command-based keys that leave the system's own
/// screenshot keys (Shift-Command-3, 4 and 5) alone.
fn default_bindings_for(mac: bool) -> Vec<Binding> {
    let ps = "Print Screen";
    let b = |action, combo| Binding { action, combo };
    // Shift-Command (and Option-Shift-Command) with a key, for the Mac.
    let cmd = |alt: bool, key: &str| Combo { ctrl: false, alt, shift: true, win: true, key: key.into() };
    let first: Vec<Binding> = if mac {
        vec![
            b(Action::AllInOne, cmd(false, "2")),
            b(Action::Screen, cmd(true, "3")),
            b(Action::ActiveWindow, cmd(true, "4")),
            b(Action::AllScreens, Combo::default()),
            b(Action::LastArea, cmd(true, "2")),
            b(Action::GrabText, cmd(true, "T")),
            b(Action::Scrolling, cmd(true, "S")),
            b(Action::RecordGif, cmd(true, "G")),
            b(Action::RecordSteps, cmd(true, "R")),
            b(Action::SaveReplay, cmd(true, "B")),
            b(Action::DrawInk, cmd(true, "D")),
            b(Action::ClearInk, cmd(true, "X")),
        ]
    } else {
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
            b(Action::SaveReplay, Combo::new(true, false, true, "F8")),
            b(Action::DrawInk, Combo::new(true, false, true, "F7")),
            b(Action::ClearInk, Combo::new(true, false, true, "F6")),
        ]
    };
    first.into_iter().chain([
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
        b(Action::ToggleReplay, Combo::default()),
    ])
    .collect()
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
    /// Mouse wheel notches per scroll.
    pub scroll_notches: u32,
    /// The longest scrolling picture, in pixels.
    pub scroll_max_height: u32,
    /// Scroll back to the top (or the left end) before capturing.
    pub scroll_from_top: bool,
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
    // Video
    pub rec_format: crate::video::VideoFormat,
    pub video_fps: u32,
    pub video_quality: u8,
    pub video_hardware: bool,
    /// The video encoder chosen for recording and streaming, by its name (`encode::available`);
    /// empty: automatic (the best that works). One that can't start falls back by itself.
    pub video_encoder: String,
    pub video_max_s: u32,
    /// Where videos and GIF recordings go (Videos/Snagster).
    pub video_folder: String,
    /// Sound recorded with videos.
    pub audio_source: crate::audio::AudioSource,
    /// The microphone (its id; empty: the computer's default).
    pub audio_mic: String,
    /// The replay buffer: how many seconds it keeps (30-120).
    pub replay_s: u32,
    /// Sound kept with replays (the microphone only when chosen).
    pub replay_audio: crate::audio::AudioSource,
    /// Turn replay on when Snagster starts (off: it starts only when turned on).
    pub replay_at_start: bool,
    /// Interface language code; empty follows the computer.
    pub language: String,
    /// Light or dark: follow the system (the default), or always one of them.
    pub appearance: crate::theme::ThemeChoice,
    /// Look for a new Snagster on GitHub when it starts (at most once a day).
    pub update_check: bool,
    /// When it last looked (Unix seconds).
    pub update_checked_at: i64,
    /// A version the person chose to skip.
    pub update_skip: String,
    /// Studio mode's scenes.
    pub studio: crate::studio::StudioSettings,
    /// Live streaming (the stream key is kept apart, see `stream::keystore`).
    pub stream: StreamSettings,
    /// The ribbon is folded away.
    pub ribbon_folded: bool,
    /// The Settings pane shown last (`SettingsTab::key`), so Settings opens on it again.
    pub settings_pane: String,
    /// Let AI apps on this computer connect over HTTP while Snagster runs.
    pub mcp_http: bool,
    pub mcp_port: u16,
    /// Optional password AI apps must send (Authorization: Bearer ...).
    pub mcp_token: String,
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
    /// Opening Snagster again starts another copy instead of bringing the running one forward.
    pub allow_many: bool,
    pub blur_mode: BlurMode,
    pub arrow_heads: u8,
    /// The highlighter draws see-through boxes (true) or freehand strokes (false).
    pub highlight_box: bool,
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
            scroll_notches: 3,
            scroll_max_height: 20000,
            scroll_from_top: true,
            interval_s: 10,
            interval_count: 6,
            schedule_time: "12:00".into(),
            gif_fps: 10,
            gif_max_s: 30,
            gif_max_width: 1280,
            gif_cursor: true,
            slideshow_s: 2.0,
            rec_format: crate::video::VideoFormat::Mp4,
            video_fps: 30,
            video_quality: 2,
            video_hardware: true,
            video_encoder: String::new(),
            video_max_s: 0,
            video_folder: videos().join("Snagster").to_string_lossy().to_string(),
            audio_source: crate::audio::AudioSource::System,
            audio_mic: String::new(),
            replay_s: crate::replay::DEFAULT_S,
            replay_audio: crate::audio::AudioSource::System,
            replay_at_start: false,
            language: String::new(),
            appearance: crate::theme::ThemeChoice::System,
            update_check: true,
            update_checked_at: 0,
            update_skip: String::new(),
            studio: Default::default(),
            stream: StreamSettings::default(),
            ribbon_folded: false,
            settings_pane: String::new(),
            mcp_http: false,
            mcp_port: 45831,
            mcp_token: String::new(),
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
            close_to_tray: crate::platform::close_to_tray_by_default(),
            panel: PanelSettings::default(),
            override_hotkeys: true,
            start_mode: StartMode::Editor,
            allow_many: false,
            blur_mode: BlurMode::Pixelate,
            arrow_heads: 1,
            highlight_box: true,
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
    crate::userdirs::home().unwrap_or_else(|| PathBuf::from("."))
}

/// Live streaming choices. The stream key is secret and is deliberately not here: it is kept by
/// `stream::keystore` (DPAPI, the Keychain, or a private file), never in settings.json.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct StreamSettings {
    pub service: crate::stream::Service,
    /// The server address; empty: the service's usual one.
    pub url: String,
    /// What goes out live.
    pub source: crate::stream::Source,
    /// The tallest picture sent (0: the source's own size); the service's limit still applies.
    pub max_height: u32,
    pub fps: u32,
    /// Video bitrate in kbit/s (0: recommended for the size); at most what the service takes.
    pub kbps: u32,
    /// Encode on the graphics chip when there is one.
    pub hardware: bool,
    /// Send HEVC (H.265) where the service takes it (Enhanced RTMP) and the computer can
    /// encode it; H.264 otherwise.
    pub hevc: bool,
    /// Seconds between keyframes (services ask for 2).
    pub keyframe_s: u32,
    pub audio: crate::audio::AudioSource,
    /// Connection drops are retried this many times in a row.
    pub reconnects: u32,
    pub cursor: bool,
    /// Also write what is streamed to an FLV file in the recordings folder.
    pub record_copy: bool,
    /// More services streamed to at the same time, from the same encode (multistreaming).
    /// Their keys are kept like the main one (`keystore::load_for(id)`).
    pub also: Vec<ExtraService>,
}

/// At most this many services besides the main one.
pub const MAX_ALSO: usize = 4;

/// A service streamed to besides the main one.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ExtraService {
    /// Names its stream key in the key store (1 and up, never reused while in the list).
    pub id: u32,
    pub service: crate::stream::Service,
    /// The server address; empty: the service's usual one.
    pub url: String,
    /// Streamed to when going live (off: kept in the list for later).
    pub on: bool,
}

impl Default for ExtraService {
    fn default() -> Self {
        ExtraService { id: 0, service: crate::stream::Service::YouTube, url: String::new(), on: true }
    }
}

impl Default for StreamSettings {
    fn default() -> Self {
        StreamSettings {
            service: crate::stream::Service::Twitch,
            url: String::new(),
            source: crate::stream::Source::Screen,
            max_height: 1080,
            fps: 30,
            kbps: 0,
            hardware: true,
            hevc: false,
            keyframe_s: 2,
            audio: crate::audio::AudioSource::System,
            reconnects: 5,
            cursor: true,
            record_copy: false,
            also: Vec::new(),
        }
    }
}

impl StreamSettings {
    /// Every number brought within what the chosen service takes.
    pub fn clamped(&self) -> StreamSettings {
        let p = self.service.preset();
        let mut s = self.clone();
        s.max_height = if s.max_height == 0 { 0 } else { s.max_height.clamp(144, p.max_height) };
        s.fps = s.fps.clamp(1, p.max_fps);
        // A size chosen: no faster than the service takes at that size (4K at 60 at most).
        if s.max_height > 0 && !p.offers(s.max_height, s.fps) {
            let h = s.max_height.min(p.max_height);
            s.fps = p.fps_for((h * 16 / 9) & !1, h, s.fps);
        }
        s.kbps = if s.kbps == 0 { 0 } else { s.kbps.clamp(300, p.max_video_kbps) };
        s.keyframe_s = s.keyframe_s.clamp(1, 4);
        s.reconnects = s.reconnects.min(50);
        s.also.truncate(MAX_ALSO);
        // Every extra service a key name of its own (a hand-edited file may repeat one or leave it 0).
        let mut seen = Vec::new();
        for i in 0..s.also.len() {
            if s.also[i].id == 0 || seen.contains(&s.also[i].id) {
                s.also[i].id = s.free_id_besides(&seen);
            }
            seen.push(s.also[i].id);
        }
        s
    }

    /// An id for a new extra service that none in the list has.
    pub fn new_also_id(&self) -> u32 {
        self.free_id_besides(&[])
    }

    fn free_id_besides(&self, taken: &[u32]) -> u32 {
        (1..).find(|id| !taken.contains(id) && !self.also.iter().any(|a| a.id == *id)).unwrap_or(1)
    }

    /// The extra services that are on.
    pub fn also_on(&self) -> impl Iterator<Item = &ExtraService> {
        self.also.iter().filter(|a| a.on)
    }
}

pub fn pictures() -> PathBuf {
    crate::userdirs::pictures().unwrap_or_else(|| home().join("Pictures"))
}

/// The person's Videos folder (Movies on macOS).
pub fn videos() -> PathBuf {
    crate::userdirs::videos().unwrap_or_else(|| home().join(if cfg!(target_os = "macos") { "Movies" } else { "Videos" }))
}

/// The cloud folders as last found, never waiting: the window asks every frame, and looking
/// can take seconds (`G:` may be a network drive that is gone, and each check then waits for
/// it), so they are looked for on a thread of their own at most once a minute. Empty until
/// the first look is done; `repaint` is asked to draw again when it is.
pub fn cloud_folders_now(repaint: Option<&egui::Context>) -> Vec<(String, PathBuf)> {
    static CACHE: std::sync::Mutex<CloudCache> = std::sync::Mutex::new(CloudCache { found: Vec::new(), looked: None, looking: false });
    cached_look(&CACHE, std::time::Instant::now(), repaint.cloned(), cloud_folders)
}

pub(crate) struct CloudCache {
    found: Vec<(String, PathBuf)>,
    looked: Option<std::time::Instant>,
    looking: bool,
}

/// What `cache` holds now, starting a look on its own thread with `look` when the last one is a
/// minute old (or there was none) and none is under way.
pub(crate) fn cached_look(cache: &'static std::sync::Mutex<CloudCache>, now: std::time::Instant, repaint: Option<egui::Context>, look: fn() -> Vec<(String, PathBuf)>) -> Vec<(String, PathBuf)> {
    let mut c = cache.lock().unwrap_or_else(|p| p.into_inner());
    let due = c.looked.is_none_or(|t| now.saturating_duration_since(t) >= std::time::Duration::from_secs(60));
    if due && !c.looking {
        c.looking = true;
        c.looked = Some(now);
        let started = std::thread::Builder::new().name("snagster-cloud-folders".into()).spawn(move || {
            let found = look();
            let mut c = cache.lock().unwrap_or_else(|p| p.into_inner());
            c.found = found;
            c.looking = false;
            drop(c);
            if let Some(r) = repaint {
                r.request_repaint();
            }
        });
        if started.is_err() {
            c.looking = false;
        }
    }
    c.found.clone()
}

/// Folders that sync to the cloud, when the person has them. Slow when a drive doesn't
/// answer: the window uses `cloud_folders_now`.
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

/// `settings.json` in Snagster's folder (`userdirs::snagster_dir`: `%APPDATA%\Snagster`,
/// `~/Library/Application Support/Snagster`, `$XDG_CONFIG_HOME/Snagster` or
/// `~/.config/Snagster`). crash.log, the stream key file and the font cache sit beside it.
pub fn settings_path() -> Option<PathBuf> {
    Some(crate::userdirs::snagster_dir()?.join("settings.json"))
}

impl Settings {
    /// Load the saved settings. `fresh` skips the file (used for the check run).
    pub fn load(fresh: bool) -> Self {
        if fresh {
            return Self::default();
        }
        // Settings from before the rename (the "Snag" folder) are carried over.
        let old = settings_path().and_then(|p| Some(p.parent()?.parent()?.join("Snag").join("settings.json")));
        let text = settings_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .or_else(|| old.and_then(|p| std::fs::read_to_string(p).ok()));
        let mut s = Self::from_saved(text.as_deref());
        s.move_old_folder();
        s
    }

    /// Settings from a saved file's text (the defaults when there is none or it can't be read).
    fn from_saved(text: Option<&str>) -> Self {
        let mut s: Settings = text.and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
        // Settings saved by an older Snagster may lack newer actions.
        for d in default_bindings() {
            if !s.bindings.iter().any(|b| b.action == d.action) {
                s.bindings.push(d);
            }
        }
        // Studio from before per-scene sound: the old Sound choice becomes its shared sound.
        let (old, mic) = (s.audio_source, s.audio_mic.clone());
        s.studio.ready_sounds(old, &mic);
        s.studio.tidy_sounds();
        s
    }

    /// Pictures saved before the rename went to Pictures/Snag: move that folder to
    /// Pictures/Snagster (library included) and point the settings there.
    fn move_old_folder(&mut self) {
        let pics = pictures();
        let old = pics.join("Snag");
        let new = pics.join("Snagster");
        let old_s = old.to_string_lossy().to_string();
        let uses_old = |p: &str| p == old_s || p.starts_with(&format!("{old_s}{}", std::path::MAIN_SEPARATOR));
        if !uses_old(&self.save_folder) && !uses_old(&self.library_folder) {
            return;
        }
        if old.is_dir() && !new.exists() && std::fs::rename(&old, &new).is_err() {
            return;
        }
        if old.is_dir() && new.exists() {
            // Both exist: leave the old one where it is, so nothing goes missing.
            return;
        }
        let new_s = new.to_string_lossy().to_string();
        for f in [&mut self.save_folder, &mut self.library_folder] {
            if uses_old(f) {
                *f = format!("{new_s}{}", &f[old_s.len()..]);
            }
        }
        self.save();
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

    /// Where Share > Save to the cloud puts a picture: `<cloud folder>/Snagster/<file name>`,
    /// never over an existing file. The window and the MCP server's save_to_cloud both use it.
    pub fn cloud_save_path(&mut self, cloud: &std::path::Path, title: &str) -> PathBuf {
        let base = self.file_name(title);
        unique_path(&cloud.join("Snagster"), &base, self.format.ext())
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

    /// The effect settings (border, shadow, watermark...), with `info_text` for the capture-info
    /// effect.
    pub fn effect_options(&self, info_text: String) -> crate::effects::EffectOptions {
        crate::effects::EffectOptions {
            resize_percent: self.resize_percent,
            border_width: self.border_width,
            border_color: self.border_color,
            shadow_size: self.shadow_size,
            rounded_radius: self.rounded_radius,
            watermark_text: self.watermark_text.clone(),
            tilt_percent: self.tilt_percent,
            brightness: self.brightness,
            contrast: self.contrast,
            saturation: self.saturation,
            info_text,
        }
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

/// Fuzz target (`src/fuzz.rs`): a settings.json cut short, edited by hand or written by
/// another version.
#[cfg(test)]
pub(crate) mod fuzzing {
    use super::*;
    use crate::fuzz::Target;

    /// Every key in the settings, at any depth.
    fn keys(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::Object(m) => {
                for (k, c) in m {
                    out.push(k.clone());
                    keys(c, out);
                }
            }
            serde_json::Value::Array(a) => a.iter().for_each(|c| keys(c, out)),
            _ => {}
        }
    }

    pub fn targets() -> Vec<Target> {
        vec![Target {
            name: "settings",
            seeds: || {
                let mut s = Settings::default();
                s.studio.current = 1;
                s.jpeg_quality = 40;
                vec![serde_json::to_vec_pretty(&s).unwrap(), b"{}".to_vec()]
            },
            words: || {
                let mut w = Vec::new();
                keys(&serde_json::to_value(Settings::default()).unwrap(), &mut w);
                w.sort();
                w.dedup();
                w
            },
            run: |b| {
                let loaded = Settings::from_saved(Some(&String::from_utf8_lossy(b)));
                // What a window does with settings it opened.
                for a in Action::ALL {
                    let _ = loaded.binding(a);
                }
                let _ = loaded.effect_options("info".into());
                let _ = loaded.studio.scene();
                for i in 0..loaded.studio.scenes.len() + 1 {
                    let _ = loaded.studio.scene_sounds(i);
                }
                let again = serde_json::to_string(&loaded).unwrap();
                let _ = Settings::from_saved(Some(&again));
            },
            budget: |n| (1 << 20) + 64 * n,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// A new, empty folder of its own under the temp folder.
    fn temp_folder(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("snagster-settings-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A drive that doesn't answer used to hold up every frame of the Share tab. The cache
    /// answers at once, looks once a minute on its own thread, and never twice at a time.
    #[test]
    fn cloud_folders_are_looked_for_in_the_background_and_never_waited_for() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CACHE: std::sync::Mutex<CloudCache> = std::sync::Mutex::new(CloudCache { found: Vec::new(), looked: None, looking: false });
        static LOOKS: AtomicUsize = AtomicUsize::new(0);
        fn slow_look() -> Vec<(String, PathBuf)> {
            LOOKS.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(400));
            vec![("Dropbox".into(), PathBuf::from("/d"))]
        }
        let t0 = std::time::Instant::now();
        assert!(cached_look(&CACHE, t0, None, slow_look).is_empty(), "nothing yet, and no waiting");
        assert!(cached_look(&CACHE, t0, None, slow_look).is_empty());
        assert!(t0.elapsed() < std::time::Duration::from_millis(200), "it waited {:?}", t0.elapsed());
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while cached_look(&CACHE, t0, None, slow_look).is_empty() && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(cached_look(&CACHE, t0, None, slow_look), vec![("Dropbox".to_string(), PathBuf::from("/d"))]);
        assert_eq!(LOOKS.load(Ordering::SeqCst), 1, "one look, however often it is asked");
        // A minute later it looks again (and still answers with the last list meanwhile).
        assert_eq!(cached_look(&CACHE, t0 + std::time::Duration::from_secs(61), None, slow_look).len(), 1);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while LOOKS.load(Ordering::SeqCst) < 2 && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(LOOKS.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn extra_stream_services_get_ids_of_their_own_and_old_files_load_without_them() {
        let mut s = StreamSettings::default();
        assert!(s.also.is_empty());
        let x = |id| ExtraService { id, ..Default::default() };
        s.also = vec![x(0), x(2), x(2), x(5), x(1), x(9)];
        let c = s.clamped();
        assert_eq!(c.also.len(), MAX_ALSO);
        let ids: Vec<u32> = c.also.iter().map(|a| a.id).collect();
        assert!(ids.iter().all(|&i| i > 0), "{ids:?}");
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len(), "no id twice: {ids:?}");
        assert_eq!(ids[1], 2, "ids that were fine are kept (their keys are saved under them)");
        let mut c = c;
        assert!(!c.also.iter().any(|a| a.id == c.new_also_id()));
        c.also[1].on = false;
        assert_eq!(c.also_on().count(), MAX_ALSO - 1);
        // Settings saved before multistreaming have no "also".
        let old: StreamSettings = serde_json::from_str(r#"{"fps":60}"#).unwrap();
        assert_eq!((old.fps, old.also.len()), (60, 0));
        let back: StreamSettings = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn defaults_are_sensible() {
        let s = Settings::default();
        assert!(s.after_show_editor && s.after_library);
        assert_eq!(s.format, ImageFormat::Png);
        assert_eq!(s.jpeg_quality, 90);
        assert_eq!(s.save_counter, 1);
        assert_eq!(s.mcp_port, 45831);
        assert!(!s.mcp_http, "AI apps can't connect over HTTP until the person allows it");
        assert!(s.mcp_token.is_empty());
        assert!(s.language.is_empty(), "the interface follows the computer by default");
        assert_eq!(s.color, DEFAULT_PALETTE[0]);
        assert_eq!(s.palette, DEFAULT_PALETTE);
        assert!(s.save_folder.ends_with("Snagster"));
        assert!(PathBuf::from(&s.library_folder).ends_with(PathBuf::from("Snagster").join("Library")));
        assert!(s.video_folder.ends_with("Snagster"));
        assert_eq!(s.bindings, default_bindings());
        assert_eq!(s.presets, default_presets());
        assert_eq!(s.themes, default_themes());
        assert_eq!(s.quick_styles, default_quick_styles());
        assert_eq!(s.panel, PanelSettings::default());
        assert_eq!(s.callout_shape, CalloutShape::Rounded);
    }

    #[test]
    fn every_action_has_exactly_one_default_binding() {
        let b = default_bindings();
        assert_eq!(b.len(), Action::ALL.len());
        let actions: HashSet<Action> = b.iter().map(|b| b.action).collect();
        assert_eq!(actions.len(), Action::ALL.len());
        for a in Action::ALL {
            assert!(actions.contains(&a), "{a:?} has no default binding");
            assert!(!a.label().is_empty());
        }
        // No two default key combinations are the same.
        let set: Vec<&Combo> = b.iter().map(|b| &b.combo).filter(|c| c.is_set()).collect();
        for (i, c) in set.iter().enumerate() {
            assert!(!set[..i].contains(c), "{} is bound twice", c.label());
        }
        if !cfg!(target_os = "macos") {
            assert_eq!(Settings::default().binding(Action::AllInOne), Combo::new(false, false, false, "Print Screen"));
        }
        assert!(!Settings::default().binding(Action::Area).is_set());
    }

    /// The Mac's defaults: every action once, no Print Screen (Mac keyboards have none), and
    /// never the system's own Shift-Command-3, 4 or 5.
    #[test]
    fn mac_default_hotkeys_use_command_and_leave_the_system_keys_alone() {
        for mac in [true, false] {
            let b = default_bindings_for(mac);
            assert_eq!(b.len(), Action::ALL.len());
            let set: Vec<&Combo> = b.iter().map(|b| &b.combo).filter(|c| c.is_set()).collect();
            for (i, c) in set.iter().enumerate() {
                assert!(!set[..i].contains(c), "bound twice");
                assert!(crate::hotkeys::to_hotkey(c).is_some(), "{} isn't a known key", c.key);
                if mac {
                    assert!(c.win && c.key != "Print Screen", "{c:?}");
                    let system_key = !c.alt && !c.ctrl && c.shift && ["3", "4", "5"].contains(&c.key.as_str());
                    assert!(!system_key, "{c:?} is the Mac's own screenshot key");
                }
            }
        }
    }

    #[test]
    fn binding_of_a_missing_action_is_not_set() {
        let mut s = Settings::default();
        s.bindings.clear();
        assert_eq!(s.binding(Action::Screen), Combo::default());
    }

    #[test]
    fn combo_labels() {
        assert_eq!(Combo::default().label(), "Not set");
        assert!(!Combo::default().is_set());
        assert_eq!(Combo::new(false, false, false, "F9").label(), "F9");
        assert_eq!(Combo::new(true, false, true, "F9").label(), crate::platform::hotkey_label(true, false, true, false, "F9"));
        let all = Combo { ctrl: true, alt: true, shift: true, win: true, key: "A".into() };
        assert_eq!(all.label(), crate::platform::hotkey_label(true, true, true, true, "A"));
        if cfg!(windows) {
            assert_eq!(all.label(), "Ctrl + Alt + Shift + Win + A");
        }
        // Modifiers without a key are still "not set".
        let mods_only = Combo { ctrl: true, alt: true, shift: false, win: false, key: String::new() };
        assert_eq!(mods_only.label(), "Not set");
    }

    #[test]
    fn image_formats_have_distinct_extensions_and_labels() {
        let exts: HashSet<&str> = ImageFormat::ALL.iter().map(|f| f.ext()).collect();
        let labels: HashSet<&str> = ImageFormat::ALL.iter().map(|f| f.label()).collect();
        assert_eq!(exts.len(), ImageFormat::ALL.len());
        assert_eq!(labels.len(), ImageFormat::ALL.len());
        assert_eq!(ImageFormat::Jpeg.ext(), "jpg");
        for f in ImageFormat::ALL {
            assert!(f.ext().chars().all(|c| c.is_ascii_lowercase()));
            assert_eq!(f.label().to_ascii_lowercase().replace("jpeg", "jpg"), f.ext());
        }
    }

    #[test]
    fn every_enum_lists_all_its_values_with_labels() {
        fn distinct(labels: Vec<&'static str>) {
            let set: HashSet<&str> = labels.iter().copied().collect();
            assert_eq!(set.len(), labels.len(), "duplicate label in {labels:?}");
            assert!(labels.iter().all(|l| !l.is_empty()));
        }
        distinct(PresetMode::ALL.iter().map(|m| m.label()).collect());
        distinct(StepStyle::ALL.iter().map(|m| m.label()).collect());
        distinct(PanelSelection::IMAGE.iter().map(|m| m.label()).collect());
        distinct(PanelShare::ALL.iter().map(|m| m.label()).collect());
        distinct(StartMode::ALL.iter().map(|m| m.label()).collect());
        distinct(BlurMode::ALL.iter().map(|m| m.label()).collect());
        for v in PanelSelection::VIDEO {
            assert!(PanelSelection::IMAGE.contains(&v));
        }
    }

    #[test]
    fn step_numbers() {
        let n = StepStyle::Numbers;
        assert_eq!(n.format(0), "0");
        assert_eq!(n.format(1), "1");
        assert_eq!(n.format(42), "42");
    }

    #[test]
    fn step_letters_run_like_spreadsheet_columns() {
        let u = StepStyle::Upper;
        assert_eq!(u.format(0), "A", "0 counts as the first step");
        assert_eq!(u.format(1), "A");
        assert_eq!(u.format(2), "B");
        assert_eq!(u.format(26), "Z");
        assert_eq!(u.format(27), "AA");
        assert_eq!(u.format(52), "AZ");
        assert_eq!(u.format(53), "BA");
        assert_eq!(u.format(702), "ZZ");
        assert_eq!(u.format(703), "AAA");
        assert_eq!(StepStyle::Lower.format(28), "ab");
    }

    #[test]
    fn step_roman_numerals() {
        let r = StepStyle::Roman;
        assert_eq!(r.format(1), "i");
        assert_eq!(r.format(4), "iv");
        assert_eq!(r.format(9), "ix");
        assert_eq!(r.format(14), "xiv");
        assert_eq!(r.format(40), "xl");
        assert_eq!(r.format(90), "xc");
        assert_eq!(r.format(400), "cd");
        assert_eq!(r.format(1994), "mcmxciv");
        assert_eq!(r.format(3999), "mmmcmxcix");
        assert_eq!(r.format(0), "i", "clamped to 1");
        assert_eq!(r.format(10_000), "mmmcmxcix", "clamped to 3999");
    }

    #[test]
    fn streaming_settings_round_trip_and_never_hold_the_key() {
        let mut s = Settings::default();
        s.stream = StreamSettings {
            service: crate::stream::Service::YouTube,
            url: "rtmps://b.rtmps.youtube.com:443/live2".into(),
            source: crate::stream::Source::Window("Notepad".into()),
            max_height: 720,
            fps: 60,
            kbps: 4500,
            hardware: false,
            hevc: true,
            keyframe_s: 2,
            audio: crate::audio::AudioSource::Both,
            reconnects: 3,
            cursor: false,
            record_copy: true,
            also: vec![ExtraService { id: 1, service: crate::stream::Service::Kick, url: String::new(), on: true }],
        };
        let text = serde_json::to_string_pretty(&s).unwrap();
        assert_eq!(Settings::from_saved(Some(&text)), s);
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let keys: Vec<&String> = v["stream"].as_object().unwrap().keys().collect();
        assert!(!keys.iter().any(|k| k.contains("key") && *k != "keyframe_s"), "no stream key in settings.json: {keys:?}");
        // Older settings files have no stream section: the defaults.
        let old = Settings::from_saved(Some(r#"{ "jpeg_quality": 70 }"#));
        assert_eq!(old.stream, StreamSettings::default());
        let d = StreamSettings::default();
        assert_eq!((d.service, d.fps, d.keyframe_s, d.kbps, d.max_height), (crate::stream::Service::Twitch, 30, 2, 0, 1080));
        assert!(d.hardware && d.cursor && !d.record_copy);
    }

    #[test]
    fn streaming_numbers_are_kept_within_the_services_limits() {
        let s = StreamSettings { service: crate::stream::Service::Twitch, max_height: 2160, fps: 144, kbps: 20_000, keyframe_s: 30, reconnects: 1000, ..Default::default() }.clamped();
        assert_eq!((s.max_height, s.fps, s.kbps, s.keyframe_s, s.reconnects), (1080, 60, 6000, 4, 50));
        let s = StreamSettings { service: crate::stream::Service::Custom, max_height: 0, kbps: 0, fps: 0, keyframe_s: 0, ..Default::default() }.clamped();
        assert_eq!((s.max_height, s.kbps, s.fps, s.keyframe_s), (0, 0, 1, 1), "0 keeps meaning the source size and the recommended bitrate");
        let custom = |max_height, fps| StreamSettings { service: crate::stream::Service::Custom, max_height, fps, ..Default::default() }.clamped();
        assert_eq!(custom(2160, 120).fps, 60, "4K at 60 at most");
        assert_eq!(custom(1440, 120).fps, 120);
        assert_eq!(custom(1080, 120).fps, 120);
        assert_eq!(custom(0, 120).fps, 120, "the source's size: decided when going live");
        let yt = StreamSettings { service: crate::stream::Service::YouTube, max_height: 2160, fps: 120, kbps: 60_000, ..Default::default() }.clamped();
        assert_eq!((yt.max_height, yt.fps, yt.kbps), (2160, 60, 51_000));
        assert_eq!(StreamSettings { kbps: 10, ..Default::default() }.clamped().kbps, 300);
    }

    #[test]
    fn settings_survive_a_save_and_load_round_trip() {
        let mut s = Settings::default();
        s.format = ImageFormat::Webp;
        s.jpeg_quality = 55;
        s.name_pattern = "shot_%n".into();
        s.language = "de".into();
        s.mcp_token = "secret".into();
        s.recent_files = vec!["a.png".into(), "b.png".into()];
        s.bindings[0].combo = Combo { ctrl: true, alt: false, shift: false, win: true, key: "Z".into() };
        s.presets.push(Preset { name: "Mine".into(), mode: PresetMode::Heart, delay_s: 3, ..Default::default() });
        s.panel.effect = Some(Effect::DropShadow);
        s.step_style = StepStyle::Roman;
        s.callout_shape = CalloutShape::Oval;
        let text = serde_json::to_string_pretty(&s).unwrap();
        assert_eq!(Settings::from_saved(Some(&text)), s);
    }

    #[test]
    fn missing_or_unreadable_file_gives_defaults() {
        assert_eq!(Settings::from_saved(None), Settings::default());
        assert_eq!(Settings::from_saved(Some("")), Settings::default());
        assert_eq!(Settings::from_saved(Some("{ not json")), Settings::default());
        assert_eq!(Settings::from_saved(Some("[1, 2, 3]")), Settings::default());
    }

    #[test]
    fn old_files_keep_what_they_have_and_get_defaults_for_the_rest() {
        // A file from an early version: a few fields only.
        let s = Settings::from_saved(Some(r#"{ "jpeg_quality": 70, "format": "Jpeg", "after_copy": true }"#));
        assert_eq!(s.jpeg_quality, 70);
        assert_eq!(s.format, ImageFormat::Jpeg);
        assert!(s.after_copy);
        assert_eq!(s.mcp_port, 45831);
        assert_eq!(s.themes, default_themes());
        assert_eq!(s.panel, PanelSettings::default());
    }

    #[test]
    fn scrolling_capture_settings_get_defaults_and_keep_what_was_saved() {
        // Saved before these existed: the defaults.
        let s = Settings::from_saved(Some(r#"{ "scroll_wait_ms": 500 }"#));
        assert_eq!(s.scroll_wait_ms, 500);
        assert_eq!((s.scroll_notches, s.scroll_max_height, s.scroll_from_top), (3, 20000, true));
        let s = Settings::from_saved(Some(r#"{ "scroll_notches": 8, "scroll_max_height": 5000, "scroll_from_top": false }"#));
        assert_eq!((s.scroll_notches, s.scroll_max_height, s.scroll_from_top), (8, 5000, false));
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let s = Settings::from_saved(Some(r#"{ "gif_fps": 15, "a_setting_from_the_future": [1, 2], "panel": { "tab": 1, "zzz": 3 } }"#));
        assert_eq!(s.gif_fps, 15);
        assert_eq!(s.panel.tab, 1);
        assert_eq!(s.panel.fixed_w, 1280);
    }

    #[test]
    fn old_files_get_bindings_for_newer_actions() {
        let s = Settings::from_saved(Some(
            r#"{ "bindings": [ { "action": "Screen", "combo": { "ctrl": false, "alt": true, "key": "F1" } } ] }"#,
        ));
        assert_eq!(s.bindings.len(), Action::ALL.len());
        // The saved binding wins over the default one and stays first.
        assert_eq!(s.bindings[0].action, Action::Screen);
        assert_eq!(s.binding(Action::Screen), Combo::new(false, true, false, "F1"));
        // The others get this system's defaults (Print Screen; Shift-Command-2 on a Mac).
        let default_all_in_one = default_bindings().into_iter().find(|b| b.action == Action::AllInOne).map(|b| b.combo);
        assert_eq!(Some(s.binding(Action::AllInOne)), default_all_in_one);
        for a in Action::ALL {
            assert_eq!(s.bindings.iter().filter(|b| b.action == a).count(), 1, "{a:?}");
        }
    }

    #[test]
    fn partial_nested_values_take_defaults() {
        let s = Settings::from_saved(Some(r#"{ "presets": [ { "name": "Only a name" } ], "quick_styles": [ { "width": 2.5 } ] }"#));
        assert_eq!(s.presets.len(), 1);
        assert_eq!(s.presets[0].name, "Only a name");
        assert_eq!(s.presets[0].mode, PresetMode::Area);
        assert_eq!(s.presets[0].width, 1280);
        assert!(s.presets[0].copy);
        assert_eq!(s.quick_styles[0].width, 2.5);
        assert_eq!(s.quick_styles[0].color, DEFAULT_PALETTE[0]);
    }

    #[test]
    fn settings_live_in_snagsters_folder() {
        if let (Some(p), Some(dir)) = (settings_path(), crate::userdirs::snagster_dir()) {
            assert_eq!(p, dir.join("settings.json"));
        }
    }

    #[test]
    fn file_name_fills_in_counter_and_title() {
        let mut s = Settings { name_pattern: "shot_%n_%t".into(), save_counter: 7, ..Default::default() };
        assert_eq!(s.file_name("Notes"), "shot_007_Notes");
        assert_eq!(s.save_counter, 8);
        assert_eq!(s.file_name("Notes"), "shot_008_Notes");
        s.name_pattern = "%n%n".into();
        assert_eq!(s.file_name(""), "009010");
        assert_eq!(s.save_counter, 11);
    }

    #[test]
    fn file_name_dates_have_the_right_shape() {
        let mut s = Settings::default();
        let name = s.file_name("x");
        // Snagster_YYYY-MM-DD_HH-MM-SS
        let rest = name.strip_prefix("Snagster_").unwrap();
        assert_eq!(rest.len(), 19, "{name}");
        for (i, c) in rest.chars().enumerate() {
            match i {
                4 | 7 | 13 | 16 => assert_eq!(c, '-', "{name}"),
                10 => assert_eq!(c, '_', "{name}"),
                _ => assert!(c.is_ascii_digit(), "{name}"),
            }
        }
        assert_eq!(s.save_counter, 1, "no %n, so the counter doesn't move");
    }

    #[test]
    fn file_name_keeps_unknown_codes_and_cleans_bad_characters() {
        let mut s = Settings { name_pattern: "a%qb%%c%".into(), ..Default::default() };
        assert_eq!(s.file_name(""), "a%qb%%c%");
        s.name_pattern = "%t".into();
        assert_eq!(s.file_name(r#"a\b/c:d*e?f"g<h>i|j"#), "a_b_c_d_e_f_g_h_i_j");
        assert_eq!(s.file_name(""), "Snagster", "an empty name falls back");
        assert_eq!(s.file_name("   "), "Snagster", "a blank name falls back");
        s.name_pattern = String::new();
        assert_eq!(s.file_name("ignored"), "Snagster");
    }

    #[test]
    fn unique_path_counts_up_past_existing_files() {
        let dir = temp_folder("unique");
        assert_eq!(unique_path(&dir, "pic", "png"), dir.join("pic.png"));
        std::fs::write(dir.join("pic.png"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "pic", "png"), dir.join("pic (2).png"));
        std::fs::write(dir.join("pic (2).png"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "pic", "png"), dir.join("pic (3).png"));
        // Another extension is another name.
        assert_eq!(unique_path(&dir, "pic", "jpg"), dir.join("pic.jpg"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn next_save_path_uses_folder_pattern_and_format() {
        let dir = temp_folder("next");
        let mut s = Settings {
            save_folder: dir.to_string_lossy().to_string(),
            name_pattern: "cap_%n".into(),
            format: ImageFormat::Jpeg,
            ..Default::default()
        };
        assert_eq!(s.next_save_path("t"), dir.join("cap_001.jpg"));
        std::fs::write(dir.join("cap_002.jpg"), b"x").unwrap();
        assert_eq!(s.next_save_path("t"), dir.join("cap_002 (2).jpg"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn effect_options_carry_the_effect_settings() {
        let s = Settings { border_width: 3, border_color: [1, 2, 3], shadow_size: 7, rounded_radius: 9, resize_percent: 25, watermark_text: "W".into(), tilt_percent: 4, brightness: -5, contrast: 6, saturation: -7, ..Default::default() };
        let o = s.effect_options("info".into());
        assert_eq!((o.border_width, o.border_color, o.shadow_size, o.rounded_radius, o.resize_percent), (3, [1, 2, 3], 7, 9, 25));
        assert_eq!((o.watermark_text.as_str(), o.tilt_percent, o.brightness, o.contrast, o.saturation), ("W", 4, -5, 6, -7));
        assert_eq!(o.info_text, "info");
    }

    #[test]
    fn default_themes_and_presets_have_names() {
        let themes = default_themes();
        assert_eq!(themes[0], Theme::default());
        let names: HashSet<&str> = themes.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names.len(), themes.len());
        let presets = default_presets();
        assert!(presets.iter().all(|p| !p.name.is_empty() && !p.combo.is_set()));
        assert_eq!(Preset::default().mode, PresetMode::Area);
    }

    /// A settings file that was cut short, edited by hand or overwritten with junk opens as
    /// something (the defaults at worst), never a crash, and saves again.
    #[test]
    fn broken_settings_files_load_as_something() {
        let mut s = Settings::default();
        s.studio.current = 7;
        s.jpeg_quality = 40;
        let good = serde_json::to_string_pretty(&s).unwrap();
        let mut g = crate::testalloc::Bytes::new(7);
        for i in 0..1_500 {
            let input = match i % 4 {
                3 => g.noise(300),
                // Hand edits: a number where something else belongs.
                2 => good.replacen("40", &format!("{}", g.next() as i64), 1).into_bytes(),
                _ => g.mutate(good.as_bytes()),
            };
            let text = String::from_utf8_lossy(&input);
            let (loaded, largest, _) = crate::testalloc::measure(|| Settings::from_saved(Some(&text)));
            assert!(largest <= (1 << 20) + 64 * input.len(), "input {i}: {largest} bytes");
            // What it opened as can be used and saved.
            let _ = loaded.studio.scene();
            let _ = loaded.binding(Action::Area);
            let again = serde_json::to_string(&loaded).unwrap();
            let _ = Settings::from_saved(Some(&again));
        }
    }
}
