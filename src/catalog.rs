//! The catalogue of everything a person can do in Snagster, shared by the window and the MCP
//! server: every ribbon and menu command (with the MCP tool, setting or window command that does
//! the same), every setting (with its kind, range and whether an AI app may change it), and how
//! hotkeys, editor tools, effects and command-line commands reach MCP.
//!
//! The tests at the bottom keep it complete: a new ribbon or menu button, setting, hotkey,
//! editor tool or command-line command without an MCP route fails `cargo test`.

use crate::doc::Tool;
use crate::settings::Action;

/// What a command or setting does beyond the current picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Risk {
    /// Stays inside Snagster (the picture, the window, a setting).
    Safe,
    /// Opens a dialog or window where the person decides (save as, print, install an update).
    Asks,
    /// Reaches outside Snagster: other programs, folders, email, the network, the cloud.
    Outside,
    /// Deletes or ends something (a file to the Recycle Bin, a recording, Snagster itself).
    Destructive,
}

impl Risk {
    pub fn name(self) -> &'static str {
        match self {
            Risk::Safe => "safe",
            Risk::Asks => "asks the person",
            Risk::Outside => "reaches outside Snagster",
            Risk::Destructive => "deletes or ends something",
        }
    }
}

/// One thing a person can press or tick.
pub struct Command {
    /// The id `app_command` takes.
    pub id: &'static str,
    /// Where it is: "Ribbon tab > group" or "menu".
    pub place: &'static str,
    /// The English texts on its buttons and menu items (as written in the source, before
    /// translation). Several places can show one command.
    pub labels: &'static [&'static str],
    /// The MCP tool that does the same to the AI app's own picture or files ("" none: the
    /// command only makes sense in the window).
    pub tool: &'static str,
    /// The setting it changes, when it is a tick box or a choice ("" none).
    pub setting: &'static str,
    /// Whether `app_command` runs it in the open Snagster window.
    pub window: bool,
    /// What `value` means for it ("" takes none).
    pub value: &'static str,
    pub risk: Risk,
}

const fn c(id: &'static str, place: &'static str, labels: &'static [&'static str], tool: &'static str, window: bool, value: &'static str, risk: Risk) -> Command {
    Command { id, place, labels, tool, setting: "", window, value, risk }
}

/// A tick box or choice: changed with `set_setting` (and in the window with `app_command`
/// "setting" when the person should see it change).
const fn s(id: &'static str, place: &'static str, labels: &'static [&'static str], setting: &'static str) -> Command {
    Command { id, place, labels, tool: "set_setting", setting, window: false, value: "", risk: Risk::Safe }
}

use Risk::{Asks, Destructive, Outside, Safe};

/// Every ribbon, menu, Stream and Studio command.
pub const COMMANDS: &[Command] = &[
    // ---- capture (the hotkey actions) ----
    c("capture_all_in_one", "Home > Capture, Capture > Capture", &["Capture", "All-in-one", "All-in-one capture"], "capture_region", true, "", Safe),
    c("capture_area", "Home > Capture, Capture > Capture", &["Area", "Capture an area"], "capture_region", true, "", Safe),
    c("capture_window_click", "Home > Capture, Capture > Capture", &["Window", "Click a window on the screen", "Pick a window from the list, or click one on the screen"], "capture_window", true, "", Safe),
    c("capture_window_list", "Capture > Capture > Window, menu Window list", &["Window list"], "capture_window", true, "window title (part of it)", Safe),
    c("capture_screen", "Home > Capture, Capture > Capture", &["Screen", "Capture the screen under the mouse"], "capture_screen", true, "", Safe),
    c("capture_monitor", "Capture menu > Screens", &["Screens"], "capture_screen", true, "screen number from list_monitors", Safe),
    c("capture_all_screens", "Capture > Capture", &["All screens", "Capture all screens"], "capture_screen", true, "", Safe),
    c("capture_active_window", "Capture > Capture", &["Window in front", "Capture the window in front"], "capture_active_window", true, "", Safe),
    c("capture_last_area", "Capture > Capture", &["Last area", "Repeat the last area"], "capture_last_area", true, "", Safe),
    c("capture_scrolling", "Home > Capture, Capture > Long pages", &["Scrolling", "Scrolling capture (down)"], "capture_scrolling", true, "", Safe),
    c("capture_panorama", "Capture > Long pages", &["Panorama", "Panoramic capture (sideways)"], "capture_scrolling", true, "", Safe),
    c("capture_manual_scroll", "Capture > Long pages", &["Manual"], "capture_scrolling", true, "", Safe),
    c("capture_circle", "Capture > Shapes", &["Circle", "Circle capture"], "capture_region", true, "", Safe),
    c("capture_freehand", "Capture > Shapes", &["Freehand", "Freehand capture"], "capture_region", true, "", Safe),
    c("capture_polygon", "Capture > Shapes", &["Polygon", "Polygon capture"], "capture_region", true, "", Safe),
    c("capture_multi_area", "Capture > Shapes", &["Several areas", "Capture several areas at once"], "combine_images", true, "", Safe),
    c("capture_menu", "Capture > Shapes", &["Menu"], "capture_screen", true, "", Safe),
    c("capture_shape", "Capture > Shapes > More shapes, menu Fun shapes", &["More shapes", "Fun shapes"], "capture_region", true, "shape: rectangle, ellipse, rounded, triangle, star, heart", Safe),
    c("capture_fixed_size", "Capture > Shapes > More shapes, menu Fixed size", &["Fixed size"], "capture_region", true, "WIDTHxHEIGHT, e.g. 1280x720", Safe),
    c("grab_text_screen", "Capture > Tools, Tools menu", &["Grab text", "Grab text from the screen..."], "read_screen_text", true, "", Safe),
    c("color_picker", "Capture > Tools, Tools menu", &["Color picker", "Screen color picker"], "pick_color", true, "", Safe),
    c("ruler", "Capture > Tools, Tools menu", &["Ruler", "Screen ruler"], "", true, "", Safe),
    c("show_window", "hotkey", &["Show Snagster"], "open_in_snagster", true, "", Safe),
    c("run_preset", "Capture > Timing > Presets", &["Presets"], "run_preset", true, "preset number from list_presets", Safe),
    c("import_presets", "Capture menu > Presets", &["Import presets..."], "set_setting", true, "", Asks),
    c("export_presets", "Capture menu > Presets", &["Export all presets..."], "get_settings", true, "", Asks),
    c("timer", "Home > Delay, Capture > Timing", &["Timer", "Timer: scheduled and repeated captures..."], "set_setting", true, "", Safe),
    c("capture_from_clipboard", "Home > Clipboard, File menu", &["Paste", "New from clipboard", "Capture from the clipboard"], "paste_from_clipboard", true, "", Safe),
    // ---- File ----
    c("new_image", "File menu", &["New image..."], "editor_apply", true, "", Asks),
    c("new_from_template", "File menu > New from template", &["New from template", "Step-by-step guide (one page)"], "combine_images", true, "page, side_by_side, grid2, grid3, before_after, gallery", Safe),
    c("open", "Home > File, File menu", &["Open", "Open picture or project...", "Capture from a file..."], "open_image", true, "a file to open (no value: the Open dialog)", Asks),
    c("import_pictures", "File menu", &["Import pictures..."], "open_image", true, "", Asks),
    c("open_recent", "File menu > Open recent", &["Open recent"], "open_image", true, "a path from get_settings recent_files", Safe),
    c("library", "Home > File, View > Windows, File menu", &["Library", "Library..."], "library_search", true, "", Safe),
    c("save", "Home > File, File menu", &["Save"], "save", true, "", Asks),
    c("save_as", "Share > Save, File menu", &["Save as", "Save as..."], "save", true, "", Asks),
    c("save_project_as", "File menu", &["Save as Snagster project (editable marks)..."], "save", true, "", Asks),
    c("quick_save", "Share > Save, File and Share menus", &["Quick save", "Quick save to folder", "Save to the save folder"], "quick_save", true, "", Safe),
    c("rename", "File menu", &["Rename..."], "library_tag", true, "", Asks),
    c("recycle", "File menu", &["Move to Recycle Bin"], "library_delete", true, "", Destructive),
    c("undo_delete", "Library and the status line, for a few seconds after a delete", &["Undo"], "undo_delete", true, "", Safe),
    c("export_pictures", "File menu", &["Export pictures..."], "save", true, "", Asks),
    c("print", "Share > Send, File and Share menus", &["Print", "Print..."], "", true, "", Outside),
    c("export_guide", "Share > Documents, File menu > Export guide", &["PDF guide", "Web page guide", "Export guide", "PDF document...", "Web page...", "One-page picture", "One-page guide from Recent"], "make_guide", true, "pdf, html or page", Asks),
    c("copy_file_path", "File menu", &["Copy file path"], "picture_info", true, "", Safe),
    c("open_save_folder", "Share > Save, File menu", &["Save folder", "Open save folder"], "", true, "", Outside),
    c("settings", "File menu, the settings buttons", &["Settings", "Settings...", "Editor preferences...", "Video settings", "Video settings...", "Effect settings", "Effect settings...", "Smart redact settings...", "Stream key", "Stream settings", "Stream settings...", "Edit presets..."], "set_setting", true, "page: after_capture, saving, capture, recording, editor, effects, redact, hotkeys, presets, ai, language, streaming", Safe),
    c("exit", "File menu, the Linux main menu", &["Exit", "Quit", "Quit Snagster"], "", true, "", Destructive),
    c("panel", "Home > File, View > Windows, View menu", &["Panel", "Capture panel"], "", true, "", Safe),
    // ---- clipboard and history ----
    c("copy", "Home > Clipboard, Share > Send, Share menu", &["Copy", "Copy picture"], "copy_to_clipboard", true, "", Safe),
    c("pin", "Home > Clipboard, Tools menu", &["Pin", "Pin picture to screen"], "", true, "", Safe),
    c("close_pins", "Tools menu", &["Close all pins"], "", true, "", Safe),
    c("copy_color", "Tools menu", &["Copy color"], "pick_color", true, "", Safe),
    c("undo", "Home > History, Edit > History", &["Undo"], "undo", true, "", Safe),
    c("redo", "Home > History, Edit > History", &["Redo"], "redo", true, "", Safe),
    // ---- recording ----
    c("record", "Home > Record, Record > Record", &["Record", "Record an area"], "record_video", true, "", Safe),
    c("record_stop", "Home > Record, Record > Record", &["Stop"], "record_video", true, "", Safe),
    c("record_whole_screen", "Record > Record, Record menu", &["Whole screen", "Record the whole screen"], "record_video", true, "", Safe),
    c("record_pause", "Record > Record, Record menu", &["Pause", "Resume", "Resume recording", "Pause recording"], "", true, "", Safe),
    c("record_steps", "Record > Guides", &["Steps", "Stop steps", "Record steps"], "make_guide", true, "", Safe),
    c("play_last_video", "Record > Recordings, Record menu", &["Play last video"], "list_recordings", true, "", Outside),
    c("show_last_video", "Record menu", &["Show last video in its folder"], "list_recordings", true, "", Outside),
    c("recordings_folder", "Record > Recordings, Studio", &["Recordings folder"], "list_recordings", true, "", Outside),
    c("gif_editor", "Record > Recordings, Record menu", &["GIF editor", "GIF recordings: trim, frames, combine..."], "gif_edit", true, "", Safe),
    c("replay_toggle", "Record > Replay, tray menu, hotkey", &["Replay", "Stop replay", "Turn replay on / off"], "replay_start", true, "", Safe),
    c("replay_save", "Record > Replay, tray menu, hotkey", &["Save replay"], "replay_save", true, "", Safe),
    c("gif_slideshow", "Record > Recordings, Record menu, GIF editor", &["GIF slideshow", "Make a GIF slideshow from Recent", "Slideshow from Recent"], "gif_edit", true, "", Safe),
    c("microphones_refresh", "Record > Video > sound", &["Look for microphones again"], "list_audio_devices", false, "", Safe),
    // ---- Edit ----
    c("delete_mark", "Edit > Marks, Edit menu", &["Delete mark"], "remove_mark", true, "", Safe),
    c("mark_bigger", "Edit > Marks, Edit menu", &["Bigger", "Make mark bigger"], "editor_apply", true, "", Safe),
    c("mark_smaller", "Edit > Marks, Edit menu", &["Smaller", "Make mark smaller"], "editor_apply", true, "", Safe),
    c("clear_marks", "Edit > Marks, Edit menu", &["Clear all marks"], "clear_marks", true, "", Safe),
    c("cut_mark", "Edit > Marks, Edit menu", &["Cut mark"], "remove_mark", true, "", Safe),
    c("copy_mark", "Edit > Marks, Edit menu", &["Copy mark"], "editor_apply", true, "", Safe),
    c("paste_mark", "Edit > Marks, Edit menu", &["Paste mark"], "editor_apply", true, "", Safe),
    c("paste_onto", "Edit > Marks, Edit menu", &["Paste picture onto", "Paste a picture onto this one"], "add_mark", true, "", Safe),
    c("duplicate", "Edit > Picture, Edit menu", &["Duplicate", "Duplicate picture"], "", true, "", Safe),
    c("remove_from_recent", "Edit > Picture, Edit menu", &["Remove from Recent", "Remove picture from Recent"], "", true, "", Safe),
    c("grab_text_picture", "Edit > Picture, Edit and Tools menus", &["Grab text", "Grab text...", "Grab text from this picture"], "read_text", true, "", Safe),
    c("select_tool", "Edit menu > Tool, the tool bar", &["Tool"], "editor_apply", true, "tool name, e.g. Arrow, Text, Blur, Crop", Safe),
    // ---- Image ----
    c("effect", "Image > Turn, Image > Effects, Image menu", &["Effects"], "apply_effect", true, "effect name from apply_effect", Safe),
    c("resize", "Image > Size, Image menu", &["Resize", "Resize image..."], "resize", true, "", Asks),
    c("crop", "Image > Size", &["Crop"], "crop", true, "", Safe),
    c("trim", "Image > Size, Image menu", &["Trim edges", "Trim flat edges"], "trim", true, "", Safe),
    c("canvas", "Image > Size, Image menu", &["Canvas", "Canvas size and color..."], "set_canvas", true, "", Asks),
    c("smart_redact", "Image > Privacy, Image menu", &["Smart redact", "Smart redact (blur emails, phone numbers...)"], "smart_redact", true, "", Safe),
    c("simplify", "Image > Privacy, Image menu", &["Simplify", "Simplify (text becomes bars)"], "simplify", true, "", Safe),
    c("combine", "Tools menu", &["Combine Recent side by side", "Combine Recent top to bottom"], "combine_images", true, "side or top", Safe),
    // ---- Share ----
    c("email", "Share > Send, Share menu", &["Email", "Email..."], "", true, "", Outside),
    c("word_document", "Share > Documents, Share menu", &["Word document", "Microsoft Word document..."], "make_guide", true, "", Asks),
    c("open_with", "Share > Save, Share menu", &["Open with", "Open in the default picture program"], "", true, "", Outside),
    c("show_in_folder", "Share menu", &["Show the file in its folder"], "", true, "", Outside),
    c("save_to_cloud", "Share > Cloud, Share menu", &["Save to the cloud", "Save to {name}"], "save_to_cloud", true, "cloud folder name: OneDrive, Dropbox, Google Drive or iCloud", Outside),
    // ---- View ----
    c("zoom_fit", "View > Zoom, View menu", &["Fit", "Fit to window"], "", true, "", Safe),
    c("zoom_actual", "View > Zoom, View menu", &["Actual size"], "", true, "", Safe),
    c("zoom_in", "View > Zoom, View menu", &["Zoom in"], "", true, "", Safe),
    c("zoom_out", "View > Zoom, View menu", &["Zoom out"], "", true, "", Safe),
    c("language", "View > Language", &["Change the language of Snagster's buttons and menus"], "set_language", true, "language code from list_languages (no value: the language list)", Safe),
    // ---- Help ----
    c("getting_started", "Help > Help", &["Getting started"], "", true, "", Safe),
    c("shortcuts", "Help > Help, the Linux main menu", &["Shortcuts", "Keyboard shortcuts"], "", true, "", Safe),
    c("whats_new", "Help > Help", &["What's new"], "app_info", true, "", Safe),
    c("check_updates", "Help > Updates", &["Check for updates", "Checking..."], "check_for_update", true, "", Asks),
    c("ai_apps", "Help > Snagster", &["AI apps"], "app_info", true, "", Safe),
    c("about", "Help > Snagster, the Linux main menu", &["About", "About Snagster"], "app_info", true, "", Safe),
    c("problem_reports", "Help > Snagster", &["Problem reports"], "", true, "", Outside),
    c("all_commands", "every tab > Commands", &["All commands"], "list_commands", false, "", Safe),
    c("tools_menu", "Capture, Edit, Image > Commands", &["Tools"], "list_commands", false, "", Safe),
    // The menus a narrow window folds the tabs and groups that don't fit into.
    c("overflow_menus", "the tab row and the band, in a narrow window", &["More", "More tabs"], "list_commands", false, "", Safe),
    // ---- Stream ----
    c("go_live", "Stream > Go live, Studio", &["Go live"], "stream_start", true, "", Outside),
    c("stream_stop", "Stream > Go live, Studio", &["Stop", "Cancel", "Stop the stream"], "stream_stop", true, "", Destructive),
    c("stream_source", "Stream > Source", &["Whole screen", "A window", "An area", "Pick an area...", "Studio scene"], "stream_start", true, "screen, window:TITLE, area (pick it) or studio", Safe),
    c("stream_details", "Stream > Status", &["Stream details"], "stream_stats", false, "", Safe),
    c("stream_edit_in_studio", "Stream > Scenes", &["Edit in Studio"], "studio_scenes", true, "", Safe),
    c("stream_forget_key", "Settings > Streaming", &["Forget"], "", true, "", Asks),
    c("stream_also", "Settings > Streaming", &["Add a service", "Stream to this service too"], "set_setting", false, "", Safe),
    // ---- Studio ----
    c("studio", "Studio tab", &["Studio"], "studio_scenes", true, "", Safe),
    c("studio_record", "Studio > Record", &["Start recording"], "studio_record", true, "", Safe),
    c("studio_stop", "Studio > Record", &["Stop"], "studio_record", true, "", Safe),
    c("studio_pause", "Studio > Record", &["Pause", "Resume"], "", true, "", Safe),
    c("studio_new_scene", "Studio > Scenes", &["New scene"], "studio_add_scene", true, "", Safe),
    c("studio_copy_scene", "Studio > Scenes", &["Copy scene"], "studio_edit", true, "", Safe),
    c("studio_delete_scene", "Studio > Scenes", &["Delete scene"], "studio_remove_scene", true, "", Destructive),
    c("studio_ink", "Studio > Ink, hotkey", &["Draw", "Draw on screen"], "", true, "", Safe),
    c("studio_ink_clear", "Studio > Ink, hotkey", &["Clear", "Clear ink"], "ink_clear", true, "", Destructive),
    s("studio_ink_color", "Studio > Ink", &["Color", "Ink color", "More colors…"], "studio.ink_color"),
    s("studio_ink_width", "Studio > Ink", &["Pen width"], "studio.ink_width"),
    s("studio_ink_hold", "Studio > Ink", &["How long ink stays", "Stay until cleared"], "studio.ink_mode"),
    c("studio_add_source", "Studio > Sources", &["Add source", "Screen area", "Picture...", "Text", "Color", "Pick a window..."], "studio_add_source", false, "", Safe),
    c("studio_source", "Studio > the source's properties", &["Shown", "Show this scene (also while recording)", "Fill the video", "Fit, keep shape", "Center", "Choose another picture...", "Move up (in front)", "Move down (behind)"], "studio_update_source", false, "", Safe),
    c("studio_sound", "Studio > Sound", &["Add sound", "Sound file (WAV)...", "The computer's default microphone", "Mute Computer sound"], "studio_edit", false, "", Safe),
    // ---- tick boxes and choices ----
    s("after_copy", "Capture > After capture, Capture menu", &["Copy to clipboard"], "after_copy"),
    s("after_save", "Capture > After capture, Capture menu", &["Save to folder", "Save to the save folder"], "after_save"),
    s("after_show_editor", "Capture menu > After capture", &["Show Snagster with the picture"], "after_show_editor"),
    s("after_library", "Capture menu > After capture", &["Add to the library"], "after_library"),
    s("after_pin", "Capture menu > After capture", &["Pin to screen"], "after_pin"),
    s("include_cursor", "Capture > After capture, Capture menu", &["Mouse pointer", "Include the mouse pointer (movable)"], "include_cursor"),
    s("hide_window", "Capture menu", &["Hide Snagster while capturing"], "hide_window"),
    s("delay", "Home > Delay, Capture > Timing", &["Delay", "No delay", "Wait before capturing (to open a menu first)"], "delay_s"),
    s("rec_format", "Record > Video, Record menu, Studio", &["Record as", "What to record as: MP4, HEVC, WMV or GIF", "Video format"], "rec_format"),
    s("video_fps", "Record > Video, Record menu", &["Frames per second", "Frames per second (1-120)", "fps"], "video_fps"),
    s("video_quality", "Record > Video", &["Picture quality (and file size)"], "video_quality"),
    s("video_hardware", "Record > Video", &["Graphics chip"], "video_hardware"),
    s("gif_cursor", "Record > Video", &["Mouse pointer"], "gif_cursor"),
    s("audio_source", "Record > Video", &["Sound to record with videos"], "audio_source"),
    s("audio_mic", "Record > Video > sound", &["Microphone", "The computer's default microphone"], "audio_mic"),
    s("replay_s", "Record > Replay, Settings > Recording", &["How much to keep"], "replay_s"),
    s("replay_audio", "Settings > Recording > Replay", &["Sound kept with replays"], "replay_audio"),
    s("replay_at_start", "Settings > Recording > Replay", &["Turn replay on when Snagster starts"], "replay_at_start"),
    s("show_side_panel", "View > Show, View menu", &["Options panel", "Show options panel"], "show_side_panel"),
    s("show_tray", "View > Show, View menu", &["Recent pictures", "Show Recent pictures"], "show_tray"),
    s("toolbar", "View menu > Customize toolbar", &["Customize toolbar", "Show all"], "hidden_tools"),
    s("stream_record_copy", "Stream > Go live, Settings > Streaming", &["Record a copy", "Also record what is streamed (an FLV video in the recordings folder)"], "stream.record_copy"),
    s("stream_cursor", "Stream > Go live, Settings > Streaming", &["Mouse pointer", "Show the mouse pointer"], "stream.cursor"),
    s("stream_hardware", "Settings > Streaming", &["Use the graphics chip to encode (Intel, AMD or NVIDIA)"], "stream.hardware"),
    s("stream_hevc", "Settings > Streaming", &["Send HEVC (H.265) where the service takes it"], "stream.hevc"),
    s("stream_service", "Stream > Go live", &["Service", "Where the stream goes"], "stream.service"),
    s("stream_url", "Settings > Streaming", &["Usual server"], "stream.url"),
    s("stream_audio", "Stream > Sound", &["Sound to stream"], "stream.audio"),
    s("studio_fps", "Studio > Video", &["Frames per second while recording"], "studio.fps"),
    s("studio_size", "Studio > Video", &["The size of the video"], "studio.width"),
];

/// The command a hotkey action runs (every action has one: the compiler checks).
pub fn action_command(a: Action) -> &'static str {
    match a {
        Action::AllInOne => "capture_all_in_one",
        Action::Area => "capture_area",
        Action::Window => "capture_window_click",
        Action::Screen => "capture_screen",
        Action::ActiveWindow => "capture_active_window",
        Action::AllScreens => "capture_all_screens",
        Action::LastArea => "capture_last_area",
        Action::Scrolling => "capture_scrolling",
        Action::Panorama => "capture_panorama",
        Action::Circle => "capture_circle",
        Action::Freehand => "capture_freehand",
        Action::Polygon => "capture_polygon",
        Action::MultiArea => "capture_multi_area",
        Action::MenuCapture => "capture_menu",
        Action::ManualScroll => "capture_manual_scroll",
        Action::GrabText => "grab_text_screen",
        Action::ColorPicker => "color_picker",
        Action::Ruler => "ruler",
        Action::RecordGif => "record",
        Action::RecordSteps => "record_steps",
        Action::ShowSnag => "show_window",
        Action::SaveReplay => "replay_save",
        Action::ToggleReplay => "replay_toggle",
        Action::DrawInk => "studio_ink",
        Action::ClearInk => "studio_ink_clear",
    }
}

/// The hotkey action a command id runs, if it is one.
pub fn command_action(id: &str) -> Option<Action> {
    Action::ALL.into_iter().find(|a| action_command(*a) == id)
}

/// The MCP tool (and how) an editor tool's work is done with.
pub fn editor_tool_route(t: Tool) -> (&'static str, &'static str) {
    match t {
        Tool::Select => ("update_mark", "move, resize or restyle a mark by its index"),
        Tool::Selection => ("editor_apply", "action lift: lift a piece of the picture out as a movable mark"),
        Tool::Arrow => ("add_mark", "kind arrow"),
        Tool::Line => ("add_mark", "kind line"),
        Tool::Rect => ("add_mark", "kind box"),
        Tool::Ellipse => ("add_mark", "kind oval"),
        Tool::Pen => ("add_mark", "kind pen"),
        Tool::Highlight => ("add_mark", "kind highlight"),
        Tool::Text => ("add_mark", "kind text"),
        Tool::Callout => ("add_mark", "kind callout"),
        Tool::Step => ("add_mark", "kind step"),
        Tool::Stamp => ("add_mark", "kind stamp"),
        Tool::Magnify => ("add_mark", "kind magnify"),
        Tool::Spotlight => ("add_mark", "kind spotlight"),
        Tool::Blur => ("blur_area", "pixelate, blur or cover an area"),
        Tool::CutOut => ("editor_apply", "action cut_out: remove a band and close the gap"),
        Tool::Fill => ("editor_apply", "action fill: flood-fill from a point"),
        Tool::Eraser => ("editor_apply", "action erase: rub out along points"),
        Tool::Crop => ("crop", "keep a rectangle"),
    }
}

/// Command-line commands (`snagster help`) and the MCP tool each one runs.
pub const CLI: &[(&str, &str)] = &[
    ("capture", "capture_screen"),
    ("ocr", "read_text"),
    ("redact", "smart_redact"),
    ("effect", "apply_effect"),
    ("convert", "save"),
    ("record", "record_video"),
    ("encoders", "list_video_encoders"),
    ("windows", "list_windows"),
    ("monitors", "list_monitors"),
    ("tools", "list_commands"),
    ("call", "list_commands"),
    ("help", "list_commands"),
    ("--help", "list_commands"),
    ("-h", "list_commands"),
    ("--hw-report", "list_video_encoders"),
    ("mcp", "list_commands"),
    ("--mcp", "list_commands"),
    ("mcp-http", "list_commands"),
];

pub fn command(id: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.id == id)
}

/// What kind of value a setting holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Bool,
    /// A whole number from..to.
    Int(i64, i64),
    /// A number from..to.
    Num(f64, f64),
    Text,
    /// One of these names.
    Choice(&'static [&'static str]),
    /// [red, green, blue], 0-255 each.
    Color,
    /// A list or object (its shape is the one get_settings shows).
    List,
}

impl Kind {
    pub fn describe(self) -> String {
        match self {
            Kind::Bool => "true or false".into(),
            Kind::Int(a, b) => format!("a whole number {a}-{b}"),
            Kind::Num(a, b) => format!("a number {a}-{b}"),
            Kind::Text => "text".into(),
            Kind::Choice(v) => format!("one of {}", v.join(", ")),
            Kind::Color => "[red, green, blue], 0-255 each".into(),
            Kind::List => "a list or object shaped like get_settings shows it".into(),
        }
    }
}

/// Who may change a setting over MCP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    /// An AI app may change it.
    Anyone,
    /// An AI app may change it, but it reaches outside Snagster (where files go, the network,
    /// starting with the computer...): the tool result says so.
    Outside,
    /// Only the person, in Settings: an AI app can't widen its own access.
    Person,
    /// Kept by Snagster itself (counters, timestamps): read only.
    Internal,
}

#[derive(Debug)]
pub struct SettingKey {
    /// The key: a top-level name, or `stream.fps`, `studio.width`, `panel.tab`.
    pub key: &'static str,
    pub kind: Kind,
    pub who: Who,
    pub help: &'static str,
}

const fn k(key: &'static str, kind: Kind, help: &'static str) -> SettingKey {
    SettingKey { key, kind, who: Who::Anyone, help }
}
const fn ko(key: &'static str, kind: Kind, help: &'static str) -> SettingKey {
    SettingKey { key, kind, who: Who::Outside, help }
}

const AUDIO: &[&str] = &["None", "System", "Microphone", "Both"];

/// Every setting (keep in step with `settings::Settings`; a test checks).
pub const SETTINGS: &[SettingKey] = &[
    k("after_show_editor", Kind::Bool, "After capture: show Snagster with the picture."),
    k("after_copy", Kind::Bool, "After capture: copy it to the clipboard."),
    k("after_save", Kind::Bool, "After capture: save it in the save folder."),
    k("after_pin", Kind::Bool, "After capture: pin it on top of every window."),
    k("after_library", Kind::Bool, "After capture: keep it in the library."),
    ko("save_folder", Kind::Text, "Where pictures are saved."),
    k("name_pattern", Kind::Text, "File names (strftime, e.g. Snag_%Y-%m-%d_%H-%M-%S)."),
    k("format", Kind::Choice(&["Png", "Jpeg", "Bmp", "Gif", "Webp", "Tiff", "Pdf", "Svg"]), "The picture format for saving."),
    k("jpeg_quality", Kind::Int(1, 100), "JPEG quality."),
    SettingKey { key: "save_counter", kind: Kind::Int(1, 99999), who: Who::Internal, help: "The number in the next file name (Snagster counts)." },
    ko("library_folder", Kind::Text, "Where the library keeps pictures."),
    k("delay_s", Kind::Int(0, 120), "Seconds to wait before capturing."),
    k("hide_window", Kind::Bool, "Hide Snagster while capturing."),
    k("hide_settle_ms", Kind::Int(0, 1500), "Milliseconds to wait after hiding."),
    k("show_magnifier", Kind::Bool, "Show the magnifier while picking an area."),
    k("magnifier_zoom", Kind::Int(2, 20), "The magnifier's zoom."),
    k("show_crosshair", Kind::Bool, "Show the crosshair while picking."),
    k("show_info", Kind::Bool, "Show the size and position while picking."),
    k("dim_alpha", Kind::Int(0, 220), "How dark the screen gets while picking."),
    k("detect_windows", Kind::Bool, "Snap to windows while picking."),
    k("detect_objects", Kind::Bool, "Snap to buttons, menus and other parts of windows."),
    k("include_cursor", Kind::Bool, "Add the mouse pointer to captures (movable)."),
    k("scroll_max_steps", Kind::Int(5, 150), "Scrolling capture: at most this many scrolls."),
    k("scroll_wait_ms", Kind::Int(0, 1500), "Scrolling capture: milliseconds to wait after each scroll."),
    k("scroll_notches", Kind::Int(1, 20), "Scrolling capture: wheel notches per scroll."),
    k("scroll_max_height", Kind::Int(0, 60000), "Scrolling capture: the tallest picture (0 no limit)."),
    k("scroll_from_top", Kind::Bool, "Scrolling capture: go to the top first."),
    k("interval_s", Kind::Int(2, 3600), "Timer: seconds between repeated captures."),
    k("interval_count", Kind::Int(1, 1000), "Timer: how many repeated captures."),
    k("schedule_time", Kind::Text, "Timer: the time of day (hh:mm) for a scheduled capture."),
    k("gif_fps", Kind::Int(2, 30), "GIF recording frames per second."),
    k("gif_max_s", Kind::Int(5, 300), "The longest GIF recording in seconds."),
    k("gif_max_width", Kind::Int(0, 7680), "The widest GIF (0 as recorded)."),
    k("gif_cursor", Kind::Bool, "Show the mouse pointer in recordings."),
    k("slideshow_s", Kind::Num(0.5, 10.0), "Seconds per picture in a GIF slideshow."),
    k("rec_format", Kind::Choice(&["Mp4", "Hevc", "Wmv", "Gif"]), "What Record makes."),
    k("video_fps", Kind::Int(1, 120), "Video frames per second."),
    k("video_quality", Kind::Int(1, 4), "Video quality: 1 small file, 2 good, 3 high, 4 best."),
    k("video_hardware", Kind::Bool, "Encode videos on the graphics chip."),
    k("video_encoder", Kind::Text, "The video encoder by name from list_video_encoders (\"\" automatic)."),
    k("video_max_s", Kind::Int(0, 7200), "The longest video in seconds (0 no limit)."),
    ko("video_folder", Kind::Text, "Where videos are saved."),
    k("audio_source", Kind::Choice(AUDIO), "Sound recorded with videos."),
    k("audio_mic", Kind::Text, "The microphone's id from list_audio_devices (\"\" the default)."),
    k("replay_s", Kind::Int(30, 120), "Replay buffer: how many seconds it keeps (30, 60, 90 or 120)."),
    k("replay_audio", Kind::Choice(AUDIO), "Sound kept with replays."),
    k("replay_at_start", Kind::Bool, "Turn the replay buffer on when Snagster starts (on Wayland only when the shared screen is remembered)."),
    k("language", Kind::Text, "The interface language code (\"\" the computer's); set_language checks it."),
    k("appearance", Kind::Choice(&["System", "Light", "Dark"]), "Light or dark: follow the system (System), or always Light or Dark. A high-contrast theme on the system wins."),
    ko("update_check", Kind::Bool, "Ask GitHub for a newer Snagster now and then (uses the network)."),
    SettingKey { key: "update_checked_at", kind: Kind::Int(0, i64::MAX), who: Who::Internal, help: "When updates were last checked (Snagster keeps it)." },
    k("update_skip", Kind::Text, "A version the person chose to skip."),
    k("studio.scenes", Kind::List, "Studio's scenes and sources (studio_* tools change them one by one)."),
    k("studio.current", Kind::Int(0, 1000), "The current Studio scene."),
    k("studio.width", Kind::Int(16, 7680), "Studio video width."),
    k("studio.height", Kind::Int(16, 4320), "Studio video height."),
    k("studio.fps", Kind::Int(1, 120), "Studio frames per second."),
    k("studio.shared_sounds", Kind::List, "Sounds in every scene (studio_edit changes them)."),
    SettingKey { key: "studio.sounds_ready", kind: Kind::Bool, who: Who::Internal, help: "Studio's sound lists were set up (Snagster keeps it)." },
    k("studio.system_volume", Kind::Num(0.0, 2.0), "The computer sound's volume in Studio."),
    k("studio.system_muted", Kind::Bool, "The computer sound is muted in Studio."),
    k("studio.mic_volume", Kind::Num(0.0, 2.0), "The microphone's volume in Studio."),
    k("studio.mic_muted", Kind::Bool, "The microphone is muted in Studio."),
    k("studio.ink_mode", Kind::Choice(&["Fade", "Stay"]), "Live ink: Fade (goes studio.ink_fade_s seconds after the last stroke) or Stay (until cleared with ink_clear)."),
    k("studio.ink_fade_s", Kind::Num(0.5, 600.0), "Seconds live ink holds before it fades out (Fade mode)."),
    k("studio.ink_color", Kind::Color, "The live ink colour."),
    k("studio.ink_width", Kind::Num(1.0, 64.0), "The live ink pen width in screen pixels."),
    k("stream.service", Kind::Choice(&["Twitch", "YouTube", "Kick", "Facebook", "Restream", "Custom"]), "Where the stream goes."),
    ko("stream.url", Kind::Text, "The server address (\"\" the service's usual one)."),
    k("stream.source", Kind::List, "What is streamed: \"Screen\", \"Studio\", {\"Window\": title} or {\"Area\": [x, y, w, h]}."),
    k("stream.max_height", Kind::Int(144, 4320), "The stream's tallest picture."),
    k("stream.fps", Kind::Int(1, 120), "Stream frames per second."),
    k("stream.kbps", Kind::Int(0, 100000), "Stream video bitrate (0 the service's best)."),
    k("stream.hardware", Kind::Bool, "Encode the stream on the graphics chip."),
    k("stream.hevc", Kind::Bool, "Send HEVC where the service takes it."),
    k("stream.keyframe_s", Kind::Int(1, 4), "Seconds between keyframes."),
    k("stream.audio", Kind::Choice(AUDIO), "Sound to stream."),
    k("stream.reconnects", Kind::Int(0, 50), "Reconnect this many times when the connection drops."),
    k("stream.cursor", Kind::Bool, "Show the mouse pointer in the stream."),
    k("stream.record_copy", Kind::Bool, "Also record what is streamed."),
    ko("stream.also", Kind::List, "More services to stream to at once (up to 4; their keys are set by the person)."),
    k("ribbon_folded", Kind::Bool, "The ribbon is folded away."),
    // Kept by the window as the person moves between panes; AI apps open a pane with the
    // `settings` window command instead.
    SettingKey {
        key: "settings_pane",
        kind: Kind::Choice(&["after_capture", "saving", "capture", "recording", "editor", "effects", "redact", "hotkeys", "presets", "ai", "general", "streaming"]),
        who: Who::Person,
        help: "The Settings pane shown last (Settings opens on it).",
    },
    SettingKey { key: "mcp_http", kind: Kind::Bool, who: Who::Person, help: "The HTTP server for AI apps (only the person changes it)." },
    SettingKey { key: "mcp_port", kind: Kind::Int(1024, 65535), who: Who::Person, help: "The HTTP server's port (only the person changes it)." },
    SettingKey { key: "mcp_token", kind: Kind::Text, who: Who::Person, help: "The HTTP server's access token (only the person changes it; never shown)." },
    k("color", Kind::Color, "The mark colour."),
    k("palette", Kind::List, "The eight colours in the palette."),
    k("font", Kind::Text, "The text font (\"\" Snagster's)."),
    k("width", Kind::Num(1.0, 60.0), "Line width of new marks."),
    k("fill", Kind::Bool, "Fill boxes and ovals."),
    k("corner_radius", Kind::Num(0.0, 60.0), "Box corner rounding."),
    k("shadow", Kind::Bool, "Marks have a shadow."),
    k("text_background", Kind::Bool, "Text has a background."),
    k("blur_block", Kind::Int(4, 48), "Pixelate/blur strength."),
    k("step_start", Kind::Int(1, 999), "The first step number."),
    k("step_style", Kind::Choice(&["Numbers", "Upper", "Lower", "Roman"]), "How steps are numbered."),
    k("magnify_zoom", Kind::Num(1.0, 8.0), "The magnify mark's zoom."),
    k("themes", Kind::List, "Saved themes (palette and font)."),
    k("border_width", Kind::Int(1, 100), "Border effect width."),
    k("border_color", Kind::Color, "Border effect colour."),
    k("shadow_size", Kind::Int(4, 80), "Drop shadow effect size."),
    k("rounded_radius", Kind::Int(2, 80), "Rounded corners effect radius."),
    k("resize_percent", Kind::Int(1, 400), "Resize effect percentage."),
    k("watermark_text", Kind::Text, "Watermark effect text."),
    k("tilt_percent", Kind::Int(5, 60), "Perspective effect strength."),
    k("brightness", Kind::Int(-100, 100), "Brightness effect amount."),
    k("contrast", Kind::Int(-100, 100), "Contrast effect amount."),
    k("saturation", Kind::Int(-100, 100), "Saturation effect amount."),
    k("redact_emails", Kind::Bool, "Smart redact hides email addresses."),
    k("redact_phones", Kind::Bool, "Smart redact hides phone numbers."),
    k("redact_cards", Kind::Bool, "Smart redact hides card numbers."),
    k("redact_addresses", Kind::Bool, "Smart redact hides street addresses."),
    k("redact_ips", Kind::Bool, "Smart redact hides IP addresses."),
    k("redact_urls", Kind::Bool, "Smart redact hides web addresses."),
    k("hotkeys_enabled", Kind::Bool, "Hotkeys work."),
    k("bindings", Kind::List, "Hotkeys: each action and its key combination."),
    k("presets", Kind::List, "Capture presets (run_preset runs one)."),
    k("show_side_panel", Kind::Bool, "Show the options panel on the right."),
    k("show_tray", Kind::Bool, "Show the Recent pictures row."),
    ko("tray_icon", Kind::Bool, "An icon in the taskbar corner."),
    k("close_to_tray", Kind::Bool, "Closing the window keeps Snagster in the taskbar corner."),
    k("panel.tab", Kind::Int(0, 2), "The capture panel's tab: 0 image, 1 video, 2 more."),
    k("panel.selection", Kind::Choice(&["Region", "Window", "Screen", "AllScreens", "Scrolling", "Panorama", "Menu", "Circle", "Freehand", "Polygon", "Star", "Heart", "FixedSize", "Multi"]), "The capture panel's picture selection."),
    k("panel.video_selection", Kind::Choice(&["Region", "Window", "Screen", "AllScreens", "Scrolling", "Panorama", "Menu", "Circle", "Freehand", "Polygon", "Star", "Heart", "FixedSize", "Multi"]), "The capture panel's video selection."),
    k("panel.effect", Kind::List, "The capture panel's effect (an effect name or null)."),
    k("panel.share", Kind::Choice(&["None", "Clipboard", "SaveFolder", "Pin", "OpenWith", "Cloud"]), "Where the capture panel sends captures."),
    k("panel.delay_on", Kind::Bool, "The capture panel waits first."),
    k("panel.delay_s", Kind::Int(1, 120), "The capture panel's delay."),
    k("panel.fixed_w", Kind::Int(6, 8000), "The capture panel's fixed width."),
    k("panel.fixed_h", Kind::Int(6, 8000), "The capture panel's fixed height."),
    ko("override_hotkeys", Kind::Bool, "Snagster takes Print Screen from the system."),
    ko("start_mode", Kind::Choice(&["Editor", "Panel", "Tray"]), "How Snagster starts."),
    k("allow_many", Kind::Bool, "Allow more than one Snagster (then app_command can't reach the window)."),
    k("blur_mode", Kind::Choice(&["Pixelate", "Blur", "Solid"]), "How the blur tool hides things."),
    k("arrow_heads", Kind::Int(0, 3), "Arrow heads: 0 none, 1 end, 2 start, 3 both."),
    k("highlight_box", Kind::Bool, "The highlighter draws see-through boxes (true) or freehand strokes (false)."),
    k("text_outline", Kind::Bool, "Text has an outline."),
    k("text_align", Kind::Int(0, 2), "Text alignment: 0 left, 1 center, 2 right."),
    k("callout_shape", Kind::Choice(&["Rounded", "Rectangle", "Oval"]), "Callout bubble shape."),
    k("callout_outline", Kind::Bool, "Callouts have an outline."),
    k("quick_styles", Kind::List, "Saved quick styles."),
    k("hidden_tools", Kind::List, "Editor tools hidden from the tool bar (by name)."),
    SettingKey { key: "recent_files", kind: Kind::List, who: Who::Internal, help: "Files opened or saved lately (Snagster keeps them)." },
    k("menu_delay_s", Kind::Int(1, 30), "Seconds before a menu capture."),
];

pub fn setting(key: &str) -> Option<&'static SettingKey> {
    SETTINGS.iter().find(|s| s.key == key)
}

/// Checks a new value against the catalogue: who may change it and its range. The type itself
/// is checked by reading the settings back (serde).
pub fn check_setting(key: &str, v: &serde_json::Value) -> Result<&'static SettingKey, String> {
    let spec = setting(key).ok_or(format!("there's no setting {key} (list_settings shows them all)"))?;
    match spec.who {
        Who::Person => return Err(format!("{key} can only be changed by the person, in Settings > AI apps")),
        Who::Internal => return Err(format!("{key} is kept by Snagster itself")),
        _ => {}
    }
    let bad = || format!("{key} takes {}", spec.kind.describe());
    match spec.kind {
        Kind::Bool if !v.is_boolean() => return Err(bad()),
        Kind::Int(a, b) => {
            let n = v.as_f64().ok_or_else(bad)?;
            if n.fract() != 0.0 || n < a as f64 || n > b as f64 {
                return Err(bad());
            }
        }
        Kind::Num(a, b) => {
            let n = v.as_f64().ok_or_else(bad)?;
            if !(a..=b).contains(&n) {
                return Err(bad());
            }
        }
        Kind::Text if !v.is_string() => return Err(bad()),
        Kind::Choice(names) => {
            let s = v.as_str().ok_or_else(bad)?;
            if !names.contains(&s) {
                return Err(bad());
            }
        }
        Kind::Color => {
            let ok = v.as_array().is_some_and(|a| a.len() == 3 && a.iter().all(|c| c.as_u64().is_some_and(|c| c <= 255)));
            if !ok {
                return Err(bad());
            }
        }
        _ => {}
    }
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::collections::HashSet;

    /// Every key of the settings, nested ones (studio, stream, panel) as `outer.inner`.
    fn setting_keys() -> Vec<String> {
        let v = serde_json::to_value(crate::settings::Settings::default()).unwrap();
        let mut out = Vec::new();
        for (k, val) in v.as_object().unwrap() {
            if matches!(k.as_str(), "studio" | "stream" | "panel") {
                for k2 in val.as_object().unwrap().keys() {
                    out.push(format!("{k}.{k2}"));
                }
            } else {
                out.push(k.clone());
            }
        }
        out
    }

    #[test]
    fn every_setting_is_in_the_catalogue_and_nothing_else() {
        let keys: HashSet<String> = setting_keys().into_iter().collect();
        for k in &keys {
            assert!(setting(k).is_some(), "the setting {k} has no entry in catalog::SETTINGS, so MCP can't describe or check it");
        }
        for s in SETTINGS {
            assert!(keys.contains(s.key), "catalog::SETTINGS lists {}, which isn't a setting", s.key);
        }
        let unique: HashSet<_> = SETTINGS.iter().map(|s| s.key).collect();
        assert_eq!(unique.len(), SETTINGS.len(), "a setting is listed twice");
    }

    #[test]
    fn every_default_fits_its_kind_and_every_choice_is_a_real_value() {
        let v = serde_json::to_value(crate::settings::Settings::default()).unwrap();
        for s in SETTINGS {
            let cur = s.key.split('.').fold(&v, |v, part| &v[part]);
            if matches!(s.who, Who::Anyone | Who::Outside) {
                check_setting(s.key, cur).unwrap_or_else(|e| panic!("default of {}: {e}", s.key));
            }
            if let Kind::Choice(names) = s.kind {
                for n in names {
                    let mut w = v.clone();
                    let mut slot = &mut w;
                    for part in s.key.split('.') {
                        slot = &mut slot[part];
                    }
                    *slot = json!(n);
                    assert!(serde_json::from_value::<crate::settings::Settings>(w).is_ok(), "{} isn't a value of {}", n, s.key);
                }
            }
        }
    }

    #[test]
    fn checks_refuse_what_an_ai_app_must_not_set() {
        assert!(check_setting("mcp_token", &json!("x")).unwrap_err().contains("only be changed by the person"));
        assert!(check_setting("mcp_http", &json!(true)).is_err());
        assert!(check_setting("save_counter", &json!(5)).unwrap_err().contains("kept by Snagster"));
        assert!(check_setting("video_fps", &json!(500)).unwrap_err().contains("1-120"));
        assert!(check_setting("video_fps", &json!(30.5)).is_err());
        assert!(check_setting("video_fps", &json!(30)).is_ok());
        assert!(check_setting("format", &json!("Tga")).is_err());
        assert!(check_setting("color", &json!([1, 2, 300])).is_err());
        assert!(check_setting("color", &json!([1, 2, 3])).is_ok());
        assert!(check_setting("stream.fps", &json!(60)).is_ok());
        assert_eq!(check_setting("save_folder", &json!("C:/x")).unwrap().who, Who::Outside);
        assert!(check_setting("nope", &Value::Null).unwrap_err().contains("no setting nope"));
    }

    #[test]
    fn command_ids_are_unique_and_every_route_exists() {
        let tools: HashSet<&str> = crate::mcp::tool_names().into_iter().map(|t| t.0).collect();
        let mut seen = HashSet::new();
        for c in COMMANDS {
            assert!(seen.insert(c.id), "command {} is listed twice", c.id);
            assert!(!c.labels.is_empty(), "{} has no label", c.id);
            assert!(c.window || !c.tool.is_empty(), "{} can't be reached: no tool and not a window command", c.id);
            assert!(c.tool.is_empty() || tools.contains(c.tool), "{} names the MCP tool {}, which doesn't exist", c.id, c.tool);
            if !c.setting.is_empty() {
                assert!(setting(c.setting).is_some(), "{} changes {}, which isn't in catalog::SETTINGS", c.id, c.setting);
            }
        }
    }

    #[test]
    fn every_hotkey_action_and_editor_tool_and_effect_reaches_mcp() {
        let tools: HashSet<&str> = crate::mcp::tool_names().into_iter().map(|t| t.0).collect();
        for a in Action::ALL {
            let c = command(action_command(a)).unwrap_or_else(|| panic!("{a:?} runs {}, which isn't a command", action_command(a)));
            assert!(c.window, "{a:?} must run in the window");
            assert_eq!(command_action(c.id), Some(a));
        }
        for t in Tool::ALL {
            let (tool, _) = editor_tool_route(t);
            assert!(tools.contains(tool), "editor tool {t:?} goes to {tool}, which doesn't exist");
        }
        let list = crate::mcp::tool_list();
        let effect_enum = list["tools"].as_array().unwrap().iter().find(|t| t["name"] == "apply_effect").unwrap()["inputSchema"]["properties"]["effect"]["enum"].clone();
        for e in crate::effects::Effect::ALL {
            assert!(effect_enum.as_array().unwrap().contains(&json!(format!("{e:?}"))), "apply_effect doesn't offer {e:?}");
        }
    }

    #[test]
    fn every_command_line_command_has_a_tool() {
        let tools: HashSet<&str> = crate::mcp::tool_names().into_iter().map(|t| t.0).collect();
        for (_, t) in CLI {
            assert!(tools.contains(t), "{t} isn't a tool");
        }
        // The words `run_cli` answers to.
        let src = include_str!("mcp.rs");
        let body = &src[src.find("pub fn run_cli").unwrap()..];
        let body = &body[..body.find("#[cfg(test)]").unwrap()];
        let start = body.find("let code = match cmd {").unwrap();
        for line in body[start..].lines() {
            let l = line.trim();
            // Only the arms of the outer match: 8 spaces in.
            if !line.starts_with("        \"") || !l.contains("=>") {
                continue;
            }
            for w in l.split("=>").next().unwrap().split('|') {
                let w = w.trim().trim_matches('"');
                assert!(CLI.iter().any(|c| c.0 == w), "the command-line command {w} has no entry in catalog::CLI");
            }
        }
    }

    /// The English texts of the buttons, menu items and tick boxes in `src`: for each line that
    /// makes one, the translated strings on it (its label and tooltip).
    fn button_lines(src: &str) -> Vec<(usize, Vec<String>)> {
        const MAKERS: [&str; 10] = ["small_tool(", "big_tool(", "small_menu(", "big_menu(", "capture_tool(", ".checkbox(", ".button(", "small_button(", "item(ui", "item_on(ui"];
        let mut out = Vec::new();
        for (n, line) in src.lines().enumerate() {
            if line.trim_start().starts_with("//") || !MAKERS.iter().any(|m| line.contains(m)) {
                continue;
            }
            let mut texts = Vec::new();
            // t("..."), crate::i18n::t("...") and the plain "..." that item() translates.
            let mut rest = line;
            while let Some(i) = rest.find('"') {
                let after = &rest[i + 1..];
                let Some(j) = after.find('"') else { break };
                let text = &after[..j];
                let before = &rest[..i];
                let quoted_arg = before.ends_with("t(") || before.ends_with("item(ui, ") || (before.contains("item_on(ui,") && before.trim_end().ends_with(','));
                if quoted_arg && !text.is_empty() && !text.contains("Ctrl+") {
                    texts.push(text.to_string());
                }
                rest = &after[j + 1..];
            }
            if !texts.is_empty() {
                out.push((n + 1, texts));
            }
        }
        out
    }

    /// The source from the method `first` to the end of the method `last` (its closing `    }`).
    fn methods<'a>(src: &'a str, first: &str, last: &str) -> &'a str {
        let a = src.find(first).unwrap_or_else(|| panic!("{first} not found"));
        let l = src.find(last).unwrap_or_else(|| panic!("{last} not found"));
        let b = l + src[l..].find("\n    }\n").unwrap();
        &src[a..b]
    }

    #[test]
    fn every_ribbon_and_menu_button_has_an_mcp_route() {
        // "…" and "..." count as the same (menu texts use the one character since 0.10.6).
        let same = |t: &str| t.replace('…', "...");
        let labels: HashSet<String> = COMMANDS.iter().flat_map(|c| c.labels.iter().map(|l| same(l))).collect();
        // Checked out with Windows line ends or not, the same text.
        let lf = |s: &str| s.replace("\r\n", "\n");
        let app = lf(include_str!("app.rs"));
        let sources = [
            ("src/app/ribbon.rs", lf(include_str!("app/ribbon.rs"))),
            ("src/app.rs menus", methods(&app, "    fn file_menu(", "    fn view_menu(").to_string()),
            ("src/app.rs template menu", methods(&app, "    fn template_menu(", "    fn template_menu(").to_string()),
            ("src/app/stream_ui.rs", lf(include_str!("app/stream_ui.rs"))),
            ("src/app/studio_ui.rs", lf(include_str!("app/studio_ui.rs"))),
        ];
        let mut missing = Vec::new();
        let mut scanned = 0;
        for (name, src) in &sources {
            // Only the code that builds the window (tests below it build nothing).
            let src = src.split("#[cfg(test)]").next().unwrap();
            let found = button_lines(src);
            assert!(!found.is_empty(), "{name}: only {} buttons found; has the way buttons are written changed?", found.len());
            scanned += found.len();
            for (line, texts) in found {
                if !texts.iter().any(|t| labels.contains(&same(t))) {
                    missing.push(format!("{name}:{line} {texts:?}"));
                }
            }
        }
        assert!(scanned > 200, "only {scanned} buttons found in all");
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "these buttons have no entry in catalog::COMMANDS (add one with its MCP tool, setting or window command):\n{}", missing.join("\n"));
    }

    #[test]
    fn the_button_scanner_finds_labels() {
        let src = "if look::small_tool(ui, ph::COPY, t(\"Copy\"), t(\"tip\"), has).clicked() {\nif item_on(ui, has, \"Print...\", \"Ctrl+P\") {\n// look::big_tool(ui, x, t(\"Old\"))\nui.label(t(\"Not a button\"));";
        let found = button_lines(src);
        assert_eq!(found, vec![(1, vec!["Copy".to_string(), "tip".to_string()]), (2, vec!["Print...".to_string()])]);
    }
}
