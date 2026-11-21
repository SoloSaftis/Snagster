//! Snagster's extra windows: settings (with hotkeys and presets), library, GIF recordings,
//! timer, grabbed text, help, and pinned pictures.

use crate::app::{accent, IntervalJob, SettingsTab, SnagApp, logo, rgb};
use crate::theme::pal;
use crate::capture::CaptureKind;
use crate::doc::{Doc, Tool};
use crate::effects::Effect;
use crate::hotkeys::{self, Target};
use crate::platform::Choice;
use crate::settings::{Action, Combo, ImageFormat, Preset, PresetMode, Settings, Theme};
use crate::{export, library};
use eframe::egui::{self, Color32, CornerRadius, Key, PointerButton, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, TextureOptions, pos2, vec2};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How a settings box or dialog opens: its own window on the desktop, centered on Snagster's.
pub(crate) struct Win<'a> {
    /// Stays the same while the box is open (the window's identity).
    pub key: &'a str,
    pub title: &'a str,
    pub size: egui::Vec2,
    pub resizable: bool,
}

/// Where a `size` box goes so it sits in the middle of `parent` (Snagster's window), kept on the
/// `monitor` (its top-left at 0,0) when that is known.
pub(crate) fn centered_on(parent: Rect, size: egui::Vec2, monitor: Option<egui::Vec2>) -> Pos2 {
    let mut p = parent.center() - size / 2.0;
    if let Some(m) = monitor {
        p.x = p.x.min(m.x - size.x).max(0.0);
        p.y = p.y.min(m.y - size.y).max(0.0);
    }
    p
}

/// Show a settings box or dialog in a window of its own (not inside Snagster's window), opened
/// in the middle of Snagster's window. Closing it (its X or Escape) sets `open` to false. Check
/// runs and systems without extra windows show it inside the main window as before.
pub(crate) fn own_window<R>(ctx: &egui::Context, w: Win, open: &mut bool, add: impl FnOnce(&mut egui::Ui) -> R) -> Option<R> {
    if !*open {
        ctx.data_mut(|d| d.remove::<Pos2>(egui::Id::unique(("own-window-at", w.key))));
        let_go(ctx, w.key);
        // (A box dropped without this call is noticed by the frame it was missing from.)
        #[cfg(target_os = "linux")]
        crate::x11_dialog::closed(w.key);
        return None;
    }
    if in_window(ctx) {
        return in_window_box(ctx, &w, open, add);
    }
    // Back from inside the window (the compact panel opened): it opens afresh there next time.
    let_go(ctx, w.key);
    // Where it opens is worked out once, so moving Snagster's window later doesn't drag it along.
    let at_id = egui::Id::unique(("own-window-at", w.key));
    let at = ctx.data(|d| d.get_temp::<Pos2>(at_id)).unwrap_or_else(|| {
        let (outer, monitor) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().monitor_size));
        let parent = outer.unwrap_or_else(|| ctx.content_rect());
        let p = centered_on(parent, w.size, monitor);
        ctx.data_mut(|d| d.insert_temp(at_id, p));
        p
    });
    let mut builder = own_window_builder(&w, at);
    static ICON: std::sync::OnceLock<std::sync::Arc<egui::IconData>> = std::sync::OnceLock::new();
    builder = builder.with_icon(ICON.get_or_init(|| std::sync::Arc::new(crate::icon())).clone());
    let mut add = Some(add);
    let (out, close) = ctx.show_viewport_immediate(egui::ViewportId::from_hash_of(("own-window", w.key)), builder, |ui, _class| {
        let panel = egui::CentralPanel::default().show(ui, |ui| {
            let r = ui.scope(|ui| add.take().map(|f| f(ui)));
            (r.inner, r.response.rect.height())
        });
        let (out, used) = panel.inner;
        // The Mac menu bar asks whether this window's text box has the keys (`text_has_keys`).
        if ui.input(|i| i.focused) {
            TEXT_IN_OWN_WINDOW.store(ui.ctx().text_edit_focused(), std::sync::atomic::Ordering::Relaxed);
        }
        // A box that can't be resized grows or shrinks to what it holds (not the Mac's
        // Settings, which keeps one size for every pane).
        if fits_content(&w) {
            let want = (used + 16.0).max(80.0).ceil();
            let have = ui.ctx().input(|i| i.viewport().inner_rect.map(|r| r.height()));
            if have.is_some_and(|h| (h - want).abs() > 2.0) {
                let width = ui.ctx().input(|i| i.viewport().inner_rect.map_or(w.size.x, |r| r.width()));
                for cmd in fit_commands(vec2(width, have.unwrap_or(want)), vec2(width, want)) {
                    ui.ctx().send_viewport_cmd(cmd);
                }
            }
        }
        // Esc closes a secondary window everywhere; Ctrl+W (⌘W) too, as GNOME, KDE and macOS
        // expect.
        let close = ui.ctx().input(|i| i.viewport().close_requested() || i.key_pressed(Key::Escape) || (i.modifiers.command && i.key_pressed(Key::W)));
        (out, close)
    });
    // X11: the box belongs to Snagster's window (see `x11_dialog`).
    #[cfg(target_os = "linux")]
    crate::x11_dialog::shown(w.key, w.title, ctx.cumulative_pass_nr());
    if close {
        *open = false;
        ctx.data_mut(|d| d.remove::<Pos2>(at_id));
        #[cfg(target_os = "linux")]
        crate::x11_dialog::closed(w.key);
    }
    out
}

/// The window an `own_window` box opens in, at `at`. A box that can't be resized is a dialog
/// (X11's `_NET_WM_WINDOW_TYPE_DIALOG`: no Minimise or Maximise on KDE once it belongs to
/// Snagster's window); the others are ordinary windows.
fn own_window_builder(w: &Win, at: Pos2) -> egui::ViewportBuilder {
    let builder = snagster_viewport()
        .with_title(w.title)
        .with_inner_size(w.size)
        .with_min_inner_size(vec2(260.0, 120.0))
        .with_resizable(w.resizable)
        .with_maximize_button(w.resizable)
        .with_minimize_button(w.resizable)
        .with_position(at);
    if w.resizable { builder } else { builder.with_window_type(egui::X11WindowType::Dialog) }
}

/// Whether the boxes are drawn inside Snagster's window: on native Wayland (set by the app each
/// frame), where a window of their own can't be given Snagster's window as its parent (so KDE
/// shows Minimise on it, it can fall behind, and it opens anywhere), and in check runs.
pub(crate) static IN_WINDOW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn in_window(ctx: &egui::Context) -> bool {
    ctx.embed_viewports() || IN_WINDOW.load(std::sync::atomic::Ordering::Relaxed)
}

/// The room boxes inside the window have: the window less a margin all round.
const IN_WINDOW_MARGIN: f32 = 16.0;

/// Where the frame a box inside the window had the keys is kept (see `box_has_keys`).
fn keys_id() -> egui::Id {
    egui::Id::unique("own-window-has-keys")
}

/// Which box inside the window has the keys (its `egui::Id`): one at a time, so Esc closes
/// that one and not the box under it.
fn owner_id() -> egui::Id {
    egui::Id::unique("own-window-keys-owner")
}

/// Whether `e` is a key that closes a box: Esc, or Ctrl+W (⌘W), as GNOME, KDE and macOS expect.
fn is_close_key(e: &egui::Event) -> bool {
    matches!(e, egui::Event::Key { key, pressed: true, modifiers, .. }
        if *key == Key::Escape && !modifiers.any() || *key == Key::W && modifiers.command_only())
}

/// Whether a box inside Snagster's window has the keys (a fixed dialog while it is open; a
/// Settings or Library box from its opening until a click outside it), so the editor's
/// shortcuts don't act on the picture behind it: Delete, Esc, Ctrl+Z belong to the box.
pub(crate) fn box_has_keys(ctx: &egui::Context) -> bool {
    let at = ctx.data(|d| d.get_temp::<u64>(keys_id()));
    holds_keys(at, ctx.cumulative_pass_nr())
}

/// Whether a box that last had the keys in frame `at` has them in frame `now` (it is drawn after
/// the shortcuts, so last frame's box counts).
fn holds_keys(at: Option<u64>, now: u64) -> bool {
    at.is_some_and(|at| at.saturating_add(1) >= now)
}

/// The size a box inside the window gets: what it asks for, but never more than the room.
fn clamped(want: egui::Vec2, room: Rect) -> egui::Vec2 {
    want.min(room.size()).max(vec2(0.0, 0.0))
}

/// What dims the window behind a dialog: light enough to still see the picture.
fn backdrop(dark: bool) -> Color32 {
    if dark { Color32::from_black_alpha(110) } else { Color32::from_black_alpha(60) }
}

/// Where the frame a box (by its `egui::Id`) inside the window was last drawn is kept.
fn drawn_id(id: egui::Id) -> egui::Id {
    id.with("drawn")
}

/// The box a box took the keys from when it opened: where that is kept.
fn prev_id(id: egui::Id) -> egui::Id {
    id.with("prev")
}

/// Whether a box drawn last in frame `last` is still there in frame `now`. A box that wasn't
/// drawn in the frame before is gone: callers close a dialog (Cancel, Create, Later) by no
/// longer drawing it.
fn still_there(last: Option<u64>, now: u64) -> bool {
    holds_keys(last, now)
}

/// Once a frame, before any box is drawn or opens: a box that has gone hands the keys back to
/// the box it took them from, and a press decides who has them. A press on a box (a dialog's
/// dimmed backdrop is the dialog's) gives it the keys; a press in an open list or menu (its own
/// layer, over the box it belongs to) changes nothing; a press anywhere else is on the editor
/// and gives the keys back to it. From the events: a press and its release can come in one
/// frame (a quick tap), and then the pointer state has no press left to look at.
fn sort_keys(ctx: &egui::Context, pass: u64) {
    let done = egui::Id::unique("own-window-keys-sorted");
    if ctx.data(|d| d.get_temp::<u64>(done)) == Some(pass) {
        return;
    }
    ctx.data_mut(|d| {
        d.insert_temp(done, pass);
        // (A few steps at most: one box over another over a third.)
        for _ in 0..8 {
            let Some(o) = d.get_temp::<egui::Id>(owner_id()) else { break };
            if still_there(d.get_temp::<u64>(drawn_id(o)), pass) {
                break;
            }
            match d.get_temp::<egui::Id>(prev_id(o)) {
                Some(p) => {
                    d.insert_temp(owner_id(), p);
                }
                None => {
                    d.remove::<egui::Id>(owner_id());
                }
            }
            d.remove::<u64>(drawn_id(o));
            d.remove::<egui::Id>(prev_id(o));
        }
    });
    let press = ctx.input(|i| {
        i.events.iter().find_map(|e| match e {
            egui::Event::PointerButton { pos, pressed: true, .. } => Some(*pos),
            _ => None,
        })
    });
    let Some(p) = press else { return };
    let layer = ctx.layer_id_at(p);
    ctx.data_mut(|d| {
        let on_box = layer.filter(|l| still_there(d.get_temp::<u64>(drawn_id(l.id)), pass));
        if let Some(l) = on_box {
            d.insert_temp(owner_id(), l.id);
        } else if !layer.is_some_and(|l| matches!(l.order, egui::Order::Foreground | egui::Order::Tooltip)) {
            d.remove::<egui::Id>(owner_id());
        }
    });
}

/// A box inside Snagster's window. A fixed dialog is modal: the window behind is dimmed and
/// takes no clicks or keys, it can't be moved, and only its X, Esc, Ctrl+W or its own buttons
/// close it (a click on the dimmed window doesn't, so a typed name isn't lost). Settings, the
/// Library and the other resizable boxes are ordinary windows inside, used next to the editor.
/// Every box fits in the window and follows it when it gets smaller.
fn in_window_box<R>(ctx: &egui::Context, w: &Win, open: &mut bool, add: impl FnOnce(&mut egui::Ui) -> R) -> Option<R> {
    let room = ctx.content_rect().shrink(IN_WINDOW_MARGIN);
    let id = egui::Id::unique(("own-window", w.key));
    let pass = ctx.cumulative_pass_nr();
    sort_keys(ctx, pass);
    // Opening (or back after a frame away): the box takes the keys from whichever box had them
    // (they go back to it when this one closes), and from the control behind that had the focus.
    let first = !still_there(ctx.data(|d| d.get_temp::<u64>(drawn_id(id))), pass);
    if first {
        let focused = ctx.memory(|m| m.focused());
        if let Some(f) = focused {
            ctx.memory_mut(|m| m.surrender_focus(f));
        }
        ctx.data_mut(|d| {
            // A spot worked out for a window of its own (before a switch from the compact
            // panel) means nothing in here.
            d.remove::<Pos2>(egui::Id::unique(("own-window-at", w.key)));
            match d.get_temp::<egui::Id>(owner_id()) {
                Some(prev) if prev != id => {
                    d.insert_temp(prev_id(id), prev);
                }
                _ => {
                    d.remove::<egui::Id>(prev_id(id));
                }
            }
            d.insert_temp(owner_id(), id);
        });
    }
    ctx.data_mut(|d| d.insert_temp(drawn_id(id), pass));
    // Read from this frame's key presses, each with the modifiers held at that moment.
    let close_keys = ctx.input(|i| i.events.iter().any(is_close_key));
    let owns = |ctx: &egui::Context| ctx.data(|d| d.get_temp::<egui::Id>(owner_id())) == Some(id);
    let (out, close) = if !w.resizable {
        let kde = crate::platform::system() == crate::platform::System::Kde;
        let style = ctx.global_style();
        let frame = egui::Frame::window(&style)
            .corner_radius(if kde { 8 } else { 12 })
            .inner_margin(16)
            .shadow(egui::Shadow { offset: [0, 6], blur: 24, spread: 0, color: Color32::from_black_alpha(80) });
        let width = clamped(w.size, room).x - 32.0;
        let r = egui::Modal::new(id).backdrop_color(backdrop(style.visuals.dark_mode)).frame(frame).show(ctx, |ui| {
            ui.set_width(width);
            let x = box_header(ui, w.title, kde);
            // The title row stays; what's under it scrolls when the window is too low for it.
            let body = (room.height() - 32.0 - 48.0).max(80.0);
            let out = egui::ScrollArea::vertical().max_height(body).show(ui, add).inner;
            (out, x)
        });
        // Esc closes an open list in the box first (egui does that), not the box.
        let esc = close_keys && r.is_top_modal && !r.any_popup_open;
        let (out, x) = r.inner;
        (Some(out), x || esc)
    } else {
        // Not while a dialog is over it (that one has the keys).
        let has_keys = owns(ctx) && ctx.memory(|m| m.top_modal_layer()).is_none();
        let mut shown = *open;
        let r = egui::Window::new(w.title)
            .id(id)
            .open(&mut shown)
            .collapsible(false)
            .resizable(true)
            .default_size(clamped(w.size, room))
            .max_size(room.size())
            .constrain_to(room)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(room.center())
            .show(ctx, add);
        let esc = has_keys && close_keys && !egui::Popup::is_any_open(ctx);
        (r.and_then(|r| r.inner), !shown || esc)
    };
    if close {
        *open = false;
        // The key that closed it is used up, so a box drawn after this one doesn't close too.
        if close_keys {
            ctx.input_mut(|i| i.events.retain(|e| !is_close_key(e)));
        }
        let_go(ctx, w.key);
    } else if !w.resizable || owns(ctx) {
        // (Not once it closes: the editor's keys work again from the next frame.)
        ctx.data_mut(|d| d.insert_temp(keys_id(), pass));
    }
    out
}

/// A box inside the window with `key` is gone (closed, or now in a window of its own): the keys
/// go back to the box it took them from, or to the editor. (A box its caller stops drawing is
/// noticed the frame after, by `sort_keys`.)
fn let_go(ctx: &egui::Context, key: &str) {
    let id = egui::Id::unique(("own-window", key));
    if ctx.data(|d| d.get_temp::<u64>(drawn_id(id)).is_none()) {
        return;
    }
    ctx.data_mut(|d| {
        if d.get_temp::<egui::Id>(owner_id()) == Some(id) {
            match d.get_temp::<egui::Id>(prev_id(id)) {
                Some(p) => {
                    d.insert_temp(owner_id(), p);
                }
                None => {
                    d.remove::<egui::Id>(owner_id());
                }
            }
        }
        d.remove::<u64>(drawn_id(id));
        d.remove::<egui::Id>(prev_id(id));
    });
}

/// A dialog's title row inside the window: the title in the middle and a Close button at the
/// end (no Minimise or Maximise), a line under it on KDE. True when Close was clicked.
fn box_header(ui: &mut egui::Ui, title: &str, line: bool) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    // A label, not painted text, so a screen reader reads the dialog's title (on Wayland it has
    // no window title of its own).
    let text = egui::RichText::new(title).size(15.0).strong();
    ui.put(rect.shrink2(vec2(32.0, 0.0)), egui::Label::new(text).selectable(false).truncate());
    let at = Rect::from_center_size(pos2(rect.right() - 12.0, rect.center().y), vec2(28.0, 28.0));
    let x = ui.put(at, |ui: &mut egui::Ui| crate::look::icon_button(ui, crate::look::ph::X, crate::i18n::t("Close")));
    if line {
        ui.separator();
    } else {
        ui.add_space(6.0);
    }
    x.clicked()
}

/// Whether a text box has the keys in the `own_window` that has the keyboard (Settings, the
/// Library, a dialog): set by that window each frame it draws while it is the key window.
pub(crate) static TEXT_IN_OWN_WINDOW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The Settings window's `own_window` key.
const SETTINGS_KEY: &str = "settings";

/// Whether a window without a resize edge takes the height of what it holds.
fn fits_content(w: &Win) -> bool {
    !w.resizable && w.key != SETTINGS_KEY
}

/// What makes a fixed-size window `size` big (from `have`). Its smallest and largest size move
/// with it: on Wayland winit pins both to the size the window opened with, and the desktop then
/// keeps that size (empty space below the buttons on KDE). Smallest = largest also tells KWin and
/// other desktops that it can't be resized, so they show no maximise button. Growing moves the
/// largest first and shrinking the smallest first, so on X11 the smallest is never above the
/// largest in between.
fn fit_commands(have: egui::Vec2, size: egui::Vec2) -> [egui::ViewportCommand; 3] {
    use egui::ViewportCommand::{InnerSize, MaxInnerSize, MinInnerSize};
    if size.y > have.y || size.x > have.x { [MaxInnerSize(size), MinInnerSize(size), InnerSize(size)] } else { [MinInnerSize(size), MaxInnerSize(size), InnerSize(size)] }
}

/// The Settings window's title and whether it can be resized. On a Mac it is titled with the
/// pane shown and keeps a fixed size, as Apple's settings guidelines ask; elsewhere it is
/// "Settings" and resizable.
fn settings_frame(sys: crate::platform::System, pane: &'static str) -> (String, bool) {
    if sys == crate::platform::System::Mac {
        // Window titles are in Title Case on a Mac (in English).
        #[cfg(any(target_os = "macos", test))]
        return (crate::menubar::in_title_case(pane.to_string(), crate::i18n::current()), false);
        #[cfg(not(any(target_os = "macos", test)))]
        return (pane.to_string(), false);
    }
    (crate::i18n::t("Settings").to_string(), true)
}

/// Settings on a Mac: a toolbar of seven icons, as Apple's Settings windows have, each a group
/// of panes that belong together (English name, icon, panes). A group of more than one pane
/// picks between them with a row of buttons under the toolbar.
pub(crate) const MAC_GROUPS: [(&str, &str, &[SettingsTab]); 7] = {
    use crate::look::ph;
    [
        ("General", ph::GEAR, &[SettingsTab::Language, SettingsTab::AfterCapture]),
        ("Capture", ph::CAMERA, &[SettingsTab::Capture, SettingsTab::Presets, SettingsTab::Saving]),
        ("Recording", ph::VIDEO_CAMERA, &[SettingsTab::Recording]),
        ("Editor", ph::PAINT_BRUSH, &[SettingsTab::Editor, SettingsTab::Effects, SettingsTab::Redact]),
        ("Hotkeys", ph::KEYBOARD, &[SettingsTab::Hotkeys]),
        ("Streaming", ph::BROADCAST, &[SettingsTab::Streaming]),
        ("AI apps", ph::ROBOT, &[SettingsTab::Ai]),
    ]
};

/// Text sizes of a Library tile's three lines: title, date and program, tags.
const TILE_FONTS: [f32; 3] = [12.5, 11.0, 11.0];
/// The room round a tile's thumbnail and lines.
const TILE_PAD: f32 = 6.0;

/// Where things go on a Library tile.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TileLayout {
    size: egui::Vec2,
    /// The thumbnail's box, from the tile's top-left corner.
    thumb: Rect,
    /// The top of each line, from the tile's top.
    rows: [f32; 3],
    /// How wide a line may be before it is cut short with "…".
    text_width: f32,
}

/// A Library tile for Windows' Text size `text_scale` (1.0 = 100 %) and the height of a line
/// of each of its texts (`row_heights`, as the fonts measure them at that size). At 100 % it is
/// the classic 192 x 176; bigger text widens it a little (more of the title fits) and the lines
/// stack at their real height, so they never overlap each other or the thumbnail.
fn tile_layout(text_scale: f32, row_heights: [f32; 3]) -> TileLayout {
    let grow = (text_scale - 1.0).max(0.0);
    let width = 192.0 + 64.0 * grow;
    let thumb_w = width - 2.0 * TILE_PAD;
    let thumb = Rect::from_min_size(pos2(TILE_PAD, TILE_PAD), vec2(thumb_w, thumb_w * 110.0 / 180.0));
    let mut y = thumb.bottom() + 4.0;
    let mut rows = [0.0; 3];
    for (k, h) in row_heights.into_iter().enumerate() {
        rows[k] = y;
        y += h + 1.0;
    }
    // The tags line is kept even when a tile has none, so every tile in a row is as tall.
    let height = (y + TILE_PAD).max(176.0);
    TileLayout { size: vec2(width, height), thumb, rows, text_width: width - 2.0 * (TILE_PAD + 2.0) }
}

/// `text` on one line at most `width` wide, cut short with "…" when it is longer.
fn one_line(p: &egui::Painter, text: String, font: egui::FontId, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text, font, color);
    job.wrap.max_width = width;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    p.layout_job(job)
}

/// Whether `one_line` had to cut `text` short (its line ends in an "…" the text didn't have).
fn was_cut(galley: &egui::Galley, text: &str) -> bool {
    galley.rows.last().and_then(|r| r.glyphs.last()).is_some_and(|g| g.chr == '…') && !text.ends_with('…')
}

/// The Library item the delete key acts on: the picked one, when the grid shows it (`order`
/// holds the shown items); a capture hidden by the search is never deleted unseen.
fn picked_index(items: &[library::Item], order: &[usize], picked: Option<&std::path::Path>) -> Option<usize> {
    let picked = picked?;
    order.iter().copied().find(|&i| items.get(i).is_some_and(|it| it.path == picked))
}

/// A translated text in a Mac's Title Case (English only; other languages keep their rules).
fn mac_title(text: &str) -> String {
    #[cfg(any(target_os = "macos", test))]
    return crate::menubar::in_title_case(text.to_string(), crate::i18n::current());
    #[cfg(not(any(target_os = "macos", test)))]
    text.to_string()
}

/// The toolbar group a pane belongs to (`MAC_GROUPS`).
pub(crate) fn mac_group(tab: SettingsTab) -> usize {
    MAC_GROUPS.iter().position(|g| g.2.contains(&tab)).unwrap_or(0)
}

/// A Mac Settings window's toolbar (`MAC_GROUPS`, centred) and, for a group of panes, the
/// row that picks between them. `label` names a pane (translated).
fn mac_settings_toolbar(ui: &mut egui::Ui, tab: &mut SettingsTab, label: &dyn Fn(SettingsTab) -> &'static str) {
    let group = mac_group(*tab);
    let gap = ui.spacing().item_spacing.x;
    let names: Vec<String> = MAC_GROUPS.iter().map(|g| mac_title(crate::i18n::t(g.0))).collect();
    let (size, width) = toolbar_fit(ui.available_width(), gap, |size| names.iter().map(|n| crate::look::pane_tool_width(ui, n, size)).collect());
    centered_row(ui, width, |ui| {
        for (i, (_, icon, tabs)) in MAC_GROUPS.iter().enumerate() {
            if crate::look::pane_tool(ui, icon, &names[i], size, i == group).clicked() && i != group {
                *tab = tabs[0];
            }
        }
    });
    let tabs = MAC_GROUPS[group].2;
    if tabs.len() > 1 {
        ui.add_space(4.0);
        let font = egui::TextStyle::Button.resolve(ui.style());
        let pad = ui.spacing().button_padding.x * 2.0 + gap;
        let width: f32 = tabs.iter().map(|t| ui.fonts_mut(|f| f.layout_no_wrap(label(*t).to_string(), font.clone(), Color32::WHITE).size().x) + pad).sum::<f32>() - gap;
        centered_row(ui, width, |ui| {
            for &t in tabs {
                ui.selectable_value(tab, t, label(t));
            }
        });
    }
}

/// The name size for the Mac Settings toolbar and the row's width: `widths(size)` gives each
/// icon's width for a name size; when the row is wider than `room`, the names get smaller
/// (down to 9 points, Apple's smallest readable size in a toolbar) until it fits.
fn toolbar_fit(room: f32, gap: f32, widths: impl Fn(f32) -> Vec<f32>) -> (f32, f32) {
    let row = |size: f32| {
        let w = widths(size);
        w.iter().sum::<f32>() + gap * (w.len().saturating_sub(1)) as f32
    };
    let mut size = crate::look::PANE_TOOL_TEXT;
    while size > 9.0 && row(size) > room {
        size -= 0.5;
    }
    (size, row(size))
}

/// Lay out a row `width` wide in the middle of `ui`.
fn centered_row(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
        add(ui);
    });
}

/// Modifier boxes and a key list for one hotkey.
fn combo_editor(ui: &mut egui::Ui, id: String, c: &mut Combo) -> bool {
    let before = c.clone();
    let [ctrl, alt, shift, win] = crate::platform::modifier_labels();
    ui.checkbox(&mut c.ctrl, crate::i18n::t(ctrl));
    ui.checkbox(&mut c.alt, crate::i18n::t(alt));
    ui.checkbox(&mut c.shift, crate::i18n::t(shift));
    ui.checkbox(&mut c.win, crate::i18n::t(win));
    egui::ComboBox::from_id_salt(id)
        .width(110.0)
        .selected_text(if c.key.is_empty() { crate::i18n::t("Not set").to_string() } else { c.key.clone() })
        .show_ui(ui, |ui| {
            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                ui.selectable_value(&mut c.key, String::new(), crate::i18n::t("Not set"));
                for (name, _, _) in hotkeys::KEYS {
                    ui.selectable_value(&mut c.key, name.to_string(), *name);
                }
            });
        });
    *c != before
}

fn color_edit(ui: &mut egui::Ui, c: &mut [u8; 3]) {
    let mut col = rgb(*c);
    let r = egui::color_picker::color_edit_button_srgba(ui, &mut col, egui::color_picker::Alpha::Opaque);
    swatch_edge_on(ui, &r, col);
    if r.changed() {
        *c = [col.r(), col.g(), col.b()];
    }
}

/// The same contrast edge the side panel's swatches have, on egui's colour button (a white
/// background colour on a light dialog, a black one on a dark dialog).
fn swatch_edge_on(ui: &egui::Ui, r: &egui::Response, col: Color32) {
    if let Some(edge) = crate::theme::swatch_edge(col.to_opaque(), ui.visuals().window_fill, &pal()) {
        let radius = ui.visuals().widgets.inactive.corner_radius;
        ui.painter().rect_stroke(r.rect, radius, Stroke::new(1.0, edge), StrokeKind::Inside);
    }
}

impl SnagApp {
    pub(crate) fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.settings_open;
        let mut reset = false;
        let mut run_preset = None;
        let panes = [
            (SettingsTab::AfterCapture, crate::i18n::t("After capture")),
            (SettingsTab::Saving, crate::i18n::t("Saving & library")),
            (SettingsTab::Capture, crate::i18n::t("Capture")),
            (SettingsTab::Recording, crate::i18n::t("Recording")),
            (SettingsTab::Editor, crate::i18n::t("Editor & themes")),
            (SettingsTab::Effects, crate::i18n::t("Effects")),
            (SettingsTab::Redact, crate::i18n::t("Smart redact")),
            (SettingsTab::Hotkeys, crate::i18n::t("Hotkeys")),
            (SettingsTab::Presets, crate::i18n::t("Presets")),
            (SettingsTab::Ai, crate::i18n::t("AI apps")),
            (SettingsTab::Streaming, crate::i18n::t("Streaming")),
            (SettingsTab::Language, crate::i18n::t("General")),
        ];
        let mac = crate::platform::system() == crate::platform::System::Mac;
        let label_of = |tab: SettingsTab| panes.iter().find(|p| p.0 == tab).map_or(panes[0].1, |p| p.1);
        // A Mac titles the window with the toolbar icon chosen.
        let pane = if mac { crate::i18n::t(MAC_GROUPS[mac_group(self.settings_tab)].0) } else { label_of(self.settings_tab) };
        let (title, resizable) = settings_frame(crate::platform::system(), pane);
        own_window(ctx, Win { key: SETTINGS_KEY, title: &title, size: vec2(720.0, 600.0), resizable }, &mut open, |ui| {
                if mac {
                    mac_settings_toolbar(ui, &mut self.settings_tab, &label_of);
                } else {
                    ui.horizontal_wrapped(|ui| {
                        for (tab, label) in panes {
                            ui.selectable_value(&mut self.settings_tab, tab, label);
                        }
                    });
                }
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| match self.settings_tab {
                    SettingsTab::AfterCapture => {
                        let s = &mut self.settings;
                        ui.label(crate::i18n::t("When a capture is taken:"));
                        ui.checkbox(&mut s.after_show_editor, crate::i18n::t("Show Snagster with the new picture"));
                        ui.checkbox(&mut s.after_library, crate::i18n::t("Add it to the library"));
                        ui.checkbox(&mut s.after_copy, crate::i18n::t("Copy the picture to the clipboard"));
                        ui.checkbox(&mut s.after_save, crate::i18n::t("Save it to the save folder"));
                        ui.checkbox(&mut s.after_pin, crate::i18n::t("Pin it on top of other windows"));
                        ui.checkbox(&mut s.include_cursor, crate::i18n::t("Include the mouse pointer as a movable mark"));
                        ui.add_space(8.0);
                        use crate::platform::by_system;
                        ui.label(RichText::new(crate::i18n::t(by_system("Taskbar corner", "Menu bar icon", "Tray icon"))).strong());
                        ui.checkbox(
                            &mut s.tray_icon,
                            crate::i18n::t(by_system(
                                "Show Snagster's icon in the taskbar corner (click: capture panel, right-click: quick menu)",
                                "Show Snagster's icon in the menu bar (click: capture panel, right-click: quick menu)",
                                "Show Snagster's icon in the tray (click: capture panel, right-click: quick menu)",
                            )),
                        );
                        ui.add_enabled(s.tray_icon, egui::Checkbox::new(&mut s.close_to_tray, crate::i18n::t("Closing the window keeps Snagster running there")));
                        if !crate::platform::has_tray() {
                            ui.label(RichText::new(crate::i18n::t("This desktop shows tray icons only with an extension, so closing the window ends Snagster.")).color(pal().dim));
                        }
                        ui.add_space(8.0);
                        ui.label(RichText::new(crate::i18n::t("Starting Snagster")).strong());
                        egui::ComboBox::from_id_salt("startmode").selected_text(format!("Start with {}", s.start_mode.label())).show_ui(ui, |ui| {
                            for m in crate::settings::StartMode::ALL {
                                ui.selectable_value(&mut s.start_mode, m, format!("Start with {}", m.label()));
                            }
                        });
                        let mut on = *self.run_at_startup.get_or_insert_with(runs_at_startup);
                        let label = if cfg!(windows) { crate::i18n::t("Start Snagster when Windows starts") } else { crate::i18n::t("Start Snagster when you log in") };
                        if ui.checkbox(&mut on, label).changed() {
                            match set_run_at_startup(on) {
                                Ok(()) => self.run_at_startup = Some(on),
                                Err(e) => self.status = crate::i18n::tf("Couldn't change that: {error}", &[("error", &e.to_string())]),
                            }
                        }
                        ui.checkbox(&mut self.settings.allow_many, crate::i18n::t("Allow more than one Snagster at a time"))
                            .on_hover_text(crate::i18n::t("Off: opening Snagster again brings the running one to the front (and opens the file you chose there)."));
                        let s = &mut self.settings;
                        ui.horizontal(|ui| {
                            ui.label(crate::i18n::t("Menu capture waits"));
                            ui.add(egui::DragValue::new(&mut s.menu_delay_s).range(1..=30).suffix(" s"));
                            ui.label(crate::i18n::t("so you can open the menu"));
                        });
                    }
                    SettingsTab::Saving => self.saving_tab(ui),
                    SettingsTab::Capture => {
                        let s = &mut self.settings;
                        ui.horizontal(|ui| {
                            ui.label(crate::i18n::t("Delay before capturing"));
                            ui.add(egui::DragValue::new(&mut s.delay_s).range(0..=120).suffix(" s"));
                        });
                        ui.checkbox(&mut s.hide_window, crate::i18n::t("Hide Snagster while capturing"));
                        ui.add_enabled(s.hide_window, egui::Slider::new(&mut s.hide_settle_ms, 100..=1500).text("Wait for Snagster to hide (ms)"));
                        ui.add_space(6.0);
                        ui.label(RichText::new(crate::i18n::t("Picking on the screen")).strong());
                        ui.checkbox(&mut s.detect_windows, crate::i18n::t("Highlight windows under the mouse"));
                        ui.checkbox(&mut s.detect_objects, crate::i18n::t("Snap to buttons, menus and panels inside windows (Ctrl = whole window)"));
                        ui.checkbox(&mut s.show_magnifier, crate::i18n::t("Show the magnifier"));
                        ui.add_enabled(s.show_magnifier, egui::Slider::new(&mut s.magnifier_zoom, 2..=20).text("Magnifier zoom"));
                        ui.checkbox(&mut s.show_crosshair, crate::i18n::t("Show crosshair lines"));
                        ui.checkbox(&mut s.show_info, crate::i18n::t("Show position and size"));
                        ui.add(egui::Slider::new(&mut s.dim_alpha, 0..=220).text("Darken the rest of the screen"));
                        ui.add_space(6.0);
                        ui.label(RichText::new(crate::i18n::t("Scrolling capture")).strong());
                        ui.add(egui::Slider::new(&mut s.scroll_max_steps, 5..=150).text(crate::i18n::t("Most scroll steps")));
                        ui.add(egui::Slider::new(&mut s.scroll_wait_ms, 100..=1500).text(crate::i18n::t("Wait after each scroll (ms)")));
                        ui.horizontal(|ui| {
                            ui.label(crate::i18n::t("Scroll by"));
                            ui.add(egui::DragValue::new(&mut s.scroll_notches).range(1..=20));
                            ui.label(crate::i18n::t("wheel notches each time"));
                        });
                        ui.horizontal(|ui| {
                            ui.label(crate::i18n::t("Stop at a height of"));
                            ui.add(egui::DragValue::new(&mut s.scroll_max_height).range(1000..=60000).speed(100).suffix(" px"));
                        });
                        ui.checkbox(&mut s.scroll_from_top, crate::i18n::t("Scroll to the top first"));
                    }
                    SettingsTab::Recording => {
                        let encoders = self.x.encoders.get(Some(ui.ctx()), crate::video::encoders).to_vec();
                        let encoders_known = self.x.encoders.loaded();
                        let pickable = self.x.pickable.get(Some(ui.ctx()), crate::encode::available).to_vec();
                        let s = &mut self.settings;
                        ui.label(RichText::new(crate::i18n::t("Record as")).strong());
                        egui::ComboBox::from_id_salt("rec-format").width(240.0).selected_text(s.rec_format.label()).show_ui(ui, |ui| {
                            for f in crate::video::VideoFormat::ALL {
                                if f.available() {
                                    ui.selectable_value(&mut s.rec_format, f, f.label());
                                }
                            }
                        });
                        ui.add_space(6.0);
                        ui.label(RichText::new(crate::i18n::t("Video (MP4, HEVC, WMV)")).strong());
                        ui.horizontal(|ui| {
                            ui.add(egui::Slider::new(&mut s.video_fps, 1..=120).text("Frames per second"));
                        });
                        ui.horizontal(|ui| {
                            for fps in [15, 30, 60, 120] {
                                if ui.selectable_label(s.video_fps == fps, format!("{fps}")).clicked() {
                                    s.video_fps = fps;
                                }
                            }
                        });
                        egui::ComboBox::from_label(crate::i18n::t("Picture quality"))
                            .selected_text(["", "Small file", "Good", "High", "Best"][s.video_quality.clamp(1, 4) as usize])
                            .show_ui(ui, |ui| {
                                for (q, name) in [(1u8, "Small file"), (2, "Good"), (3, "High"), (4, "Best")] {
                                    ui.selectable_value(&mut s.video_quality, q, name);
                                }
                            });
                        ui.checkbox(&mut s.video_hardware, crate::i18n::t("Use the graphics chip to encode (Intel, AMD or NVIDIA)"));
                        encoder_picker(ui, "rec-encoder", &mut s.video_encoder, &pickable);
                        ui.horizontal(|ui| {
                            ui.label(crate::i18n::t("Videos go to"));
                            ui.add(egui::TextEdit::singleline(&mut s.video_folder).desired_width(300.0));
                            if ui.button(crate::i18n::t("Choose…")).clicked()
                                && let Some(p) = rfd::FileDialog::new().set_directory(&s.video_folder).pick_folder()
                            {
                                s.video_folder = p.display().to_string();
                            }
                        });
                        ui.add_space(4.0);
                        ui.label(RichText::new(crate::i18n::t("Sound")).strong());
                        if crate::audio::AudioSource::supported() {
                            ui.horizontal_wrapped(|ui| {
                                for v in crate::audio::AudioSource::ALL {
                                    ui.radio_value(&mut s.audio_source, v, crate::i18n::t(v.label()));
                                }
                            });
                            let mics = self.x.mics.get(Some(ui.ctx()), crate::audio::microphones).to_vec();
                            let shown = mics.iter().find(|m| m.id == s.audio_mic).map(|m| m.name.clone()).unwrap_or_else(|| crate::i18n::t("The computer's default microphone").to_string());
                            egui::ComboBox::from_label(crate::i18n::t("Microphone")).selected_text(shown).show_ui(ui, |ui| {
                                ui.selectable_value(&mut s.audio_mic, String::new(), crate::i18n::t("The computer's default microphone"));
                                for m in mics.iter() {
                                    ui.selectable_value(&mut s.audio_mic, m.id.clone(), &m.name);
                                }
                            });
                            ui.label(RichText::new(crate::i18n::t("Sound is recorded into MP4 videos (AAC, 48 kHz stereo). The volumes are set in Studio's mixer.")).color(pal().dim));
                        } else {
                            ui.label(RichText::new(crate::i18n::t("Recording sound isn't available on this system yet.")).color(pal().dim));
                        }
                        crate::app::replay_ui::settings_section(ui, s);
                        ui.add_space(4.0);
                        ui.add(egui::Slider::new(&mut s.video_max_s, 0..=7200).suffix(" s").text("Stop by itself after (0: never)"));
                        let (w, h) = (1920, 1080);
                        ui.label(
                            RichText::new(format!(
                                "A full-HD screen at these choices takes about {:.0} MB a minute. Frames above your screen's refresh rate repeat the last picture.",
                                crate::video::bitrate(w, h, s.video_fps, s.rec_format, s.video_quality) as f64 * 60.0 / 8.0 / 1_000_000.0
                            ))
                            .color(pal().dim),
                        );
                        if let Some(list) = encoders_known.then_some(&encoders) {
                            let hw: Vec<String> = list.iter().filter(|e| e.hardware).map(|e| format!("{} ({})", e.name, e.codec)).collect();
                            let text = if hw.is_empty() {
                                "No graphics-chip encoder was found, so videos use the processor.".to_string()
                            } else {
                                format!("Graphics-chip encoders here: {}", hw.join(", "))
                            };
                            ui.label(RichText::new(text).color(pal().dim));
                        }
                        ui.add_space(6.0);
                        ui.label(RichText::new(crate::i18n::t("Animated GIF")).strong());
                        let s = &mut self.settings;
                        ui.add(egui::Slider::new(&mut s.gif_fps, 2..=30).text("Frames per second"));
                        ui.add(egui::Slider::new(&mut s.gif_max_s, 5..=300).suffix(" s").text("Longest recording"));
                        ui.add(egui::Slider::new(&mut s.gif_max_width, 320..=1920).suffix(" px").text("Widest frame"));
                        ui.add_space(6.0);
                        ui.checkbox(&mut s.gif_cursor, crate::i18n::t("Show the mouse pointer in recordings"));
                        ui.add(egui::Slider::new(&mut s.slideshow_s, 0.2..=10.0).suffix(" s").text("Slideshow: time per picture"));
                        ui.label(RichText::new(crate::i18n::t("Videos and GIFs go to the Videos folder; pictures to the Pictures folder.")).color(pal().dim));
                    }
                    SettingsTab::Editor => self.editor_tab(ui),
                    SettingsTab::Effects => {
                        let s = &mut self.settings;
                        ui.add(egui::Slider::new(&mut s.resize_percent, 10..=400).suffix(" %").text("Resize to"));
                        ui.add(egui::Slider::new(&mut s.border_width, 1..=100).suffix(" px").text("Border width"));
                        ui.horizontal(|ui| {
                            ui.label(crate::i18n::t("Border color"));
                            color_edit(ui, &mut s.border_color);
                        });
                        ui.add(egui::Slider::new(&mut s.shadow_size, 4..=80).suffix(" px").text("Drop shadow size"));
                        ui.add(egui::Slider::new(&mut s.rounded_radius, 2..=80).suffix(" px").text("Corner radius"));
                        ui.add(egui::Slider::new(&mut s.tilt_percent, 5..=60).suffix(" %").text("Perspective tilt"));
                        ui.add(egui::Slider::new(&mut s.brightness, -100..=100).text("Brightness"));
                        ui.add(egui::Slider::new(&mut s.contrast, -100..=100).text("Contrast"));
                        ui.add(egui::Slider::new(&mut s.saturation, -100..=100).text("Color strength"));
                        ui.horizontal(|ui| {
                            ui.label(crate::i18n::t("Watermark text"));
                            ui.text_edit_singleline(&mut s.watermark_text);
                        });
                    }
                    SettingsTab::Redact => {
                        let s = &mut self.settings;
                        ui.label(crate::i18n::t("Smart redact reads the picture and blurs what looks private:"));
                        ui.checkbox(&mut s.redact_emails, crate::i18n::t("Email addresses"));
                        ui.checkbox(&mut s.redact_phones, crate::i18n::t("Phone numbers"));
                        ui.checkbox(&mut s.redact_cards, crate::i18n::t("Card and account numbers"));
                        ui.checkbox(&mut s.redact_addresses, crate::i18n::t("Street addresses"));
                        ui.checkbox(&mut s.redact_ips, crate::i18n::t("IP addresses"));
                        ui.checkbox(&mut s.redact_urls, crate::i18n::t("Web addresses"));
                        ui.add(egui::Slider::new(&mut s.blur_block, 4..=48).suffix(" px").text("Blur block size"));
                        ui.label(RichText::new(crate::i18n::t("It uses the text recognition built into Windows; check the result before sharing.")).color(pal().dim));
                    }
                    SettingsTab::Hotkeys => self.hotkeys_tab(ui),
                    SettingsTab::Presets => {
                        run_preset = self.presets_tab(ui);
                    }
                    SettingsTab::Ai => self.ai_tab(ui),
                    SettingsTab::Streaming => self.streaming_tab(ui),
                    SettingsTab::Language => self.language_tab(ui),
                });
                ui.add_space(8.0);
                ui.separator();
                if ui.button(crate::i18n::t("Reset everything to defaults")).clicked() {
                    reset = true;
                }
            });
        self.settings_open = open;
        if reset {
            self.settings = Settings::default();
        }
        // Settings opens on this pane next time, also after a restart.
        if self.settings.settings_pane != self.settings_tab.key() {
            self.settings.settings_pane = self.settings_tab.key().to_string();
        }
        if let Some(i) = run_preset {
            self.run_preset(ctx, i);
        }
    }

    fn saving_tab(&mut self, ui: &mut egui::Ui) {
        let library_before = self.settings.library_folder.clone();
        let s = &mut self.settings;
        ui.label(crate::i18n::t("Save folder"));
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut s.save_folder).desired_width(380.0));
            if ui.button(crate::i18n::t("Browse…")).clicked()
                && let Some(f) = rfd::FileDialog::new().pick_folder()
            {
                s.save_folder = f.display().to_string();
            }
        });
        ui.add_space(6.0);
        ui.label(crate::i18n::t("File name pattern"));
        ui.add(egui::TextEdit::singleline(&mut s.name_pattern).desired_width(380.0));
        ui.label(RichText::new(crate::i18n::t("%Y year  %m month  %d day  %H hour  %M minute  %S second  %n counter  %t title")).color(pal().dim).size(12.0));
        let mut preview = s.clone();
        ui.label(RichText::new(crate::i18n::tf("Example: {name}", &[("name", &format!("{}.{}", preview.file_name("Capture"), s.format.ext()))])).color(pal().dim));
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(crate::i18n::t("Format"));
            for f in ImageFormat::ALL {
                ui.radio_value(&mut s.format, f, f.label());
            }
        });
        ui.add_enabled(matches!(s.format, ImageFormat::Jpeg | ImageFormat::Pdf), egui::Slider::new(&mut s.jpeg_quality, 10..=100).text("JPEG / PDF quality"));
        ui.horizontal(|ui| {
            ui.label(crate::i18n::t("Next counter"));
            ui.add(egui::DragValue::new(&mut s.save_counter).range(1..=99999));
        });
        ui.add_space(10.0);
        ui.label(RichText::new(crate::i18n::t("Library")).strong());
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut s.library_folder).desired_width(380.0));
            if ui.button(crate::i18n::t("Browse…")).clicked()
                && let Some(f) = rfd::FileDialog::new().pick_folder()
            {
                s.library_folder = f.display().to_string();
            }
        });
        ui.label(RichText::new(crate::i18n::t("Keep the library in a cloud folder to have it on every computer:")).color(pal().dim));
        ui.horizontal_wrapped(|ui| {
            let clouds = crate::settings::cloud_folders_now(Some(ui.ctx()));
            if clouds.is_empty() {
                ui.label(RichText::new(crate::i18n::t("No OneDrive, Dropbox, Google Drive, Box or iCloud folder was found.")).color(pal().dim));
            }
            for (name, p) in clouds {
                if ui.button(crate::i18n::tf("Use {name}", &[("name", &name.to_string())])).clicked() {
                    s.library_folder = p.join("Snagster Library").display().to_string();
                }
            }
        });
        // Read the library again only when its folder changed (not every frame this tab shows).
        if self.settings.library_folder != library_before {
            self.lib_loaded = false;
        }
    }

    fn editor_tab(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        ui.horizontal(|ui| {
            ui.label(crate::i18n::t("Drawing color"));
            color_edit(ui, &mut s.color);
        });
        ui.horizontal(|ui| {
            ui.label(crate::i18n::t("Palette"));
            for c in s.palette.iter_mut() {
                color_edit(ui, c);
            }
        });
        ui.add(egui::Slider::new(&mut s.width, 1.0..=24.0).step_by(1.0).text("Line width"));
        ui.add(egui::Slider::new(&mut s.corner_radius, 0.0..=60.0).step_by(1.0).text("Box corner radius"));
        ui.add(egui::Slider::new(&mut s.blur_block, 4..=48).text("Blur block size"));
        ui.add(egui::Slider::new(&mut s.magnify_zoom, 1.5..=6.0).text("Magnify zoom"));
        ui.horizontal(|ui| {
            ui.label(crate::i18n::t("Steps"));
            egui::ComboBox::from_id_salt("st").selected_text(s.step_style.label()).show_ui(ui, |ui| {
                for st in crate::settings::StepStyle::ALL {
                    ui.selectable_value(&mut s.step_style, st, st.label());
                }
            });
            ui.label(crate::i18n::t("start at"));
            ui.add(egui::DragValue::new(&mut s.step_start).range(1..=999));
        });
        ui.checkbox(&mut s.fill, crate::i18n::t("Fill boxes and ovals"));
        ui.checkbox(&mut s.shadow, crate::i18n::t("Drop shadow under marks"));
        ui.checkbox(&mut s.text_background, crate::i18n::t("Text on a colored box"));
        ui.horizontal(|ui| {
            ui.label(crate::i18n::t("Text font"));
            ui.label(RichText::new(if s.font.is_empty() { "Default" } else { &s.font }).strong());
            ui.label(RichText::new(crate::i18n::t("(pick it in the Text tool's panel)")).color(pal().dim));
        });
        ui.add_space(10.0);
        ui.label(RichText::new(crate::i18n::t("Themes")).strong());
        ui.label(RichText::new(crate::i18n::t("A theme keeps the palette, font, text box and shadow choices together.")).color(pal().dim));
        let mut apply = None;
        let mut delete = None;
        for (i, t) in s.themes.iter().enumerate() {
            ui.horizontal(|ui| {
                for c in t.palette {
                    let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                    ui.painter().rect_filled(r, CornerRadius::same(3), rgb(c));
                }
                ui.label(&t.name);
                if ui.small_button(crate::i18n::t("Use")).clicked() {
                    apply = Some(i);
                }
                if ui.small_button(crate::i18n::t("Delete")).clicked() {
                    delete = Some(i);
                }
            });
        }
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.new_theme_name).hint_text(crate::i18n::t("Theme name")).desired_width(180.0));
            if ui.button(crate::i18n::t("Save current as theme")).clicked() && !self.new_theme_name.trim().is_empty() {
                s.themes.push(Theme {
                    name: self.new_theme_name.trim().to_string(),
                    palette: s.palette,
                    font: s.font.clone(),
                    text_background: s.text_background,
                    shadow: s.shadow,
                });
                self.new_theme_name.clear();
            }
        });
        if let Some(i) = apply {
            let t = s.themes[i].clone();
            s.palette = t.palette;
            s.color = t.palette[0];
            s.font = t.font;
            s.text_background = t.text_background;
            s.shadow = t.shadow;
        }
        if let Some(i) = delete {
            s.themes.remove(i);
        }
    }

    fn hotkeys_tab(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(&mut self.settings.hotkeys_enabled, crate::i18n::t("Use global hotkeys (they work while Snagster is in the background)"));
        // On Wayland the desktop holds the hotkeys (and settles clashes itself): nothing to take over.
        if !crate::capture::desktop_screenshots_only() {
            ui.checkbox(&mut self.settings.override_hotkeys, crate::i18n::t("Take over keys another program already uses (for example Print Screen)"));
        }
        ui.label(RichText::new(self.hotkeys.summary()).color(pal().dim));
        let dups = hotkeys::duplicates(&self.settings);
        if !dups.is_empty() {
            ui.label(RichText::new(crate::i18n::tf("Used twice (only the first one works): {keys}", &[("keys", &dups.join(", "))])).color(pal().warn));
        }
        ui.add_space(4.0);
        let statuses: Vec<Option<bool>> = self.settings.bindings.iter().map(|b| self.hotkeys.status_of(Target::Action(b.action))).collect();
        egui::Grid::new("hk").striped(true).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
            for (i, b) in self.settings.bindings.iter_mut().enumerate() {
                ui.label(crate::i18n::tr(b.action.label()));
                ui.horizontal(|ui| {
                    combo_editor(ui, format!("hk{i}"), &mut b.combo);
                });
                let (t, c) = match (b.combo.is_set(), statuses[i]) {
                    (false, _) => ("", pal().dim),
                    (true, Some(true)) => ("Active", pal().ok),
                    (true, Some(false)) => ("Taken by another app", pal().err),
                    (true, None) => ("Applying...", pal().dim),
                };
                ui.label(RichText::new(t).color(c));
                ui.end_row();
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button(crate::i18n::t("Clear all")).clicked() {
                for b in &mut self.settings.bindings {
                    b.combo = Combo::default();
                }
            }
            if ui.button(crate::i18n::t("Back to the standard keys")).clicked() {
                self.settings.bindings = crate::settings::default_bindings();
            }
            let taken: Vec<usize> = self
                .settings
                .bindings
                .iter()
                .enumerate()
                .filter(|(_, b)| b.combo.is_set() && self.hotkeys.status_of(Target::Action(b.action)) == Some(false))
                .map(|(i, _)| i)
                .collect();
            if !taken.is_empty()
                && ui
                    .button(crate::i18n::tf("Give the taken ones free keys: {count}", &[("count", &taken.len().to_string())]))
                    .on_hover_text(crate::i18n::t("Moves them to Ctrl + Shift + F1...F8, skipping keys Snagster already uses"))
                    .clicked()
            {
                let mut free: Vec<Combo> = (1..=8)
                    .map(|n| Combo::new(true, false, true, &format!("F{n}")))
                    .filter(|c| !hotkeys::wanted(&self.settings).iter().any(|(_, w)| w == c))
                    .collect();
                for i in taken {
                    if free.is_empty() {
                        break;
                    }
                    self.settings.bindings[i].combo = free.remove(0);
                }
            }
        });
        ui.label(
            RichText::new(
                "Changes apply at once. If Print Screen shows as taken, turn off \"Use the Print screen key to open screen capture\" in Windows Settings > Accessibility > Keyboard.",
            )
            .color(pal().dim),
        );
    }

    fn presets_tab(&mut self, ui: &mut egui::Ui) -> Option<usize> {
        ui.label(RichText::new(crate::i18n::t("A preset remembers how to capture, which effects to add, where it goes, and its own hotkey.")).color(pal().dim));
        let (mut dup, mut export_one, mut import, mut export_all) = (None, None, false, false);
        let mut run = None;
        let mut delete = None;
        let n = self.settings.presets.len();
        for i in 0..n {
            let status = self.hotkeys.status_of(Target::Preset(i));
            let p = &mut self.settings.presets[i];
            egui::Frame::new().stroke(Stroke::new(1.0, pal().line)).corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(220.0));
                    egui::ComboBox::from_id_salt(("pm", i)).selected_text(p.mode.label()).show_ui(ui, |ui| {
                        for m in PresetMode::ALL {
                            ui.selectable_value(&mut p.mode, m, m.label());
                        }
                    });
                    if p.mode == PresetMode::FixedSize {
                        ui.add(egui::DragValue::new(&mut p.width).range(16..=8000).suffix(" w"));
                        ui.add(egui::DragValue::new(&mut p.height).range(16..=8000).suffix(" h"));
                    }
                    ui.add(egui::DragValue::new(&mut p.delay_s).range(0..=60).suffix(" s delay"));
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(crate::i18n::t("Effects:"));
                    for e in Effect::ALL {
                        let mut on = p.effects.contains(&e);
                        if ui.checkbox(&mut on, e.label()).changed() {
                            if on {
                                p.effects.push(e);
                            } else {
                                p.effects.retain(|x| *x != e);
                            }
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut p.copy, crate::i18n::t("Copy"));
                    ui.checkbox(&mut p.save, crate::i18n::t("Save to folder"));
                    ui.checkbox(&mut p.pin, crate::i18n::t("Pin"));
                });
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::t("Hotkey"));
                    combo_editor(ui, format!("pk{i}"), &mut p.combo);
                    if p.combo.is_set() {
                        let (t, c) = match status {
                            Some(true) => ("Active", pal().ok),
                            Some(false) => ("Taken", pal().err),
                            None => ("", pal().dim),
                        };
                        ui.label(RichText::new(t).color(c));
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button(crate::i18n::t("Capture with this preset")).clicked() {
                        run = Some(i);
                    }
                    if ui.button(crate::i18n::t("Duplicate")).clicked() {
                        dup = Some(i);
                    }
                    if ui.button(crate::i18n::t("Export…")).clicked() {
                        export_one = Some(i);
                    }
                    if ui.button(crate::i18n::t("Delete")).clicked() {
                        delete = Some(i);
                    }
                });
            });
            ui.add_space(4.0);
        }
        if let Some(i) = dup {
            let mut p = self.settings.presets[i].clone();
            p.name = format!("{} copy", p.name);
            p.combo = Combo::default();
            self.settings.presets.insert(i + 1, p);
        }
        if let Some(i) = export_one {
            self.export_presets(Some(i));
        }
        if let Some(i) = delete {
            self.settings.presets.remove(i);
        }
        if ui.button(crate::i18n::t("Add a preset")).clicked() {
            self.settings.presets.push(Preset::default());
        }
        ui.horizontal(|ui| {
            if ui.button(crate::i18n::t("Import presets…")).clicked() {
                import = true;
            }
            if ui.button(crate::i18n::t("Export all presets…")).clicked() {
                export_all = true;
            }
        });
        if import {
            self.import_presets();
        }
        if export_all {
            self.export_presets(None);
        }
        run
    }

    pub(crate) fn help_windows(&mut self, ctx: &egui::Context) {
        let mut open = self.shortcuts_open;
        let s = self.settings.clone();
        own_window(ctx, Win { key: "shortcuts", title: crate::i18n::t("Keyboard shortcuts"), size: vec2(560.0, 560.0), resizable: true }, &mut open, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.label(RichText::new(crate::i18n::t("Anywhere (global hotkeys, change them in Settings > Hotkeys)")).strong());
                egui::Grid::new("gk").striped(true).spacing(vec2(16.0, 4.0)).show(ui, |ui| {
                    for b in s.bindings.iter().filter(|b| b.combo.is_set()) {
                        ui.label(RichText::new(b.combo.label()).strong());
                        ui.label(crate::i18n::tr(b.action.label()));
                        ui.end_row();
                    }
                });
                ui.add_space(8.0);
                ui.label(RichText::new(crate::i18n::t("In Snagster")).strong());
                egui::Grid::new("keys").striped(true).spacing(vec2(16.0, 4.0)).show(ui, |ui| {
                    use crate::platform::{AppKey, app_key_text};
                    let capture_keys = format!("{} / {} / {}", app_key_text(AppKey::AllInOne), app_key_text(AppKey::Window), app_key_text(AppKey::Screen));
                    let print_pin = format!("Ctrl+P, {}", app_key_text(AppKey::Pin));
                    let rows: Vec<(String, String)> = [
                        (capture_keys.as_str(), "All-in-one / window / screen capture"),
                        (if crate::platform::system() == crate::platform::System::Windows { "Ctrl+Z, Ctrl+Y" } else { "Ctrl+Z, Ctrl+Shift+Z" }, "Undo, redo"),
                        ("Ctrl+C, Ctrl+V", "Copy picture, paste a picture"),
                        ("Ctrl+S, Ctrl+Shift+S, Ctrl+Alt+S", "Save, save as, quick save to folder"),
                        ("Ctrl+N, Ctrl+O, Ctrl+L", "New picture, open a picture, open the library"),
                        (print_pin.as_str(), "Print, pin picture to screen"),
                        ("Ctrl+,", "Settings"),
                        ("Delete, [ and ]", "Delete, shrink or grow the selected mark"),
                        ("Ctrl+0 or 0, 1, Ctrl+= or +, Ctrl+- or -", "Fit, actual size, zoom in, zoom out"),
                        ("Ctrl+wheel, middle drag", "Zoom, move around"),
                        ("Shift while drawing", "Straight lines, squares, circles"),
                        ("On the frozen screen", "R E O F P S H T change the shape, Space = full screen, Ctrl = whole window"),
                    ]
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .chain(Tool::ALL.iter().map(|t| (t.key().to_string(), crate::i18n::tf("{tool} tool", &[("tool", &crate::i18n::tr(t.label()))]))))
                    .chain(keyboard_rows())
                    .collect();
                    // The English rows above are translated here, their key notes too ("Shift
                    // while drawing"); a key list or an already translated row stays as it is.
                    for (k, v) in rows {
                        ui.label(RichText::new(crate::platform::keys(&crate::i18n::tr(&k))).strong());
                        ui.label(crate::i18n::tr(&v));
                        ui.end_row();
                    }
                });
            });
        });
        self.shortcuts_open = open;
        let mut open = self.about_open;
        own_window(ctx, Win { key: "about", title: crate::i18n::t("About Snagster"), size: vec2(420.0, 300.0), resizable: false }, &mut open, |ui| {
            ui.horizontal(|ui| {
                logo(ui, 40.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(crate::i18n::t("Snagster")).size(22.0).strong());
                    ui.label(crate::i18n::tf("Version {version}", &[("version", env!("CARGO_PKG_VERSION"))]));
                });
            });
            ui.label(crate::i18n::t("Capture your screen, mark it up, share it. Written in Rust."));
            let ef = crate::emoji::font();
            ui.label(RichText::new(format!(
                "{} installed fonts, {} emojis{}.",
                crate::fonts::families().map(|f| f.len()).unwrap_or(0),
                ef.count(),
                if ef.color { " in color" } else { "" }
            ))
            .color(pal().dim));
        });
        self.about_open = open;
    }

    pub(crate) fn text_window(&mut self, ctx: &egui::Context) {
        let Some(mut text) = self.grabbed_text.take() else { return };
        let mut open = true;
        own_window(ctx, Win { key: "grabbed-text", title: crate::i18n::t("Grabbed text"), size: vec2(560.0, 460.0), resizable: true }, &mut open, |ui| {
            ui.label(RichText::new(crate::i18n::t("Already copied. Edit it here if you like, then copy again.")).color(pal().dim));
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut text).desired_width(f32::INFINITY).desired_rows(12));
            });
            if ui.button(crate::i18n::t("Copy")).clicked() {
                let _ = arboard::Clipboard::new().and_then(|mut c| c.set_text(text.clone()));
            }
        });
        if open {
            self.grabbed_text = Some(text);
        }
    }

    pub(crate) fn timer_window(&mut self, ctx: &egui::Context) {
        let mut open = self.timer_open;
        let mut start_interval = None;
        let mut set_schedule = None;
        let mut stop_interval = false;
        let mut cancel_schedule = false;
        own_window(ctx, Win { key: "timer", title: crate::i18n::t("Timer"), size: vec2(420.0, 360.0), resizable: false }, &mut open, |ui| {
            let s = &mut self.settings;
            ui.label(RichText::new(crate::i18n::t("Countdown")).strong());
            ui.horizontal(|ui| {
                ui.label(crate::i18n::t("Wait"));
                ui.add(egui::DragValue::new(&mut s.delay_s).range(0..=120).suffix(" s"));
                ui.label(crate::i18n::t("before every capture"));
            });
            ui.add_space(8.0);
            ui.label(RichText::new(crate::i18n::t("Repeat")).strong());
            ui.horizontal(|ui| {
                ui.label(crate::i18n::t("Every"));
                ui.add(egui::DragValue::new(&mut s.interval_s).range(2..=3600).suffix(" s"));
                ui.label(crate::i18n::t("for"));
                ui.add(egui::DragValue::new(&mut s.interval_count).range(1..=1000).suffix(" captures"));
            });
            let kinds = [
                ("Screen", CaptureKind::Screen),
                ("All screens", CaptureKind::AllScreens),
                ("Window in front", CaptureKind::ActiveWindow),
            ];
            ui.horizontal(|ui| {
                for (label, k) in &kinds {
                    if ui.button(*label).clicked() {
                        start_interval = Some(k.clone());
                    }
                }
                if let Some((x, y, w, h)) = self.last_region
                    && ui.button(crate::i18n::t("Last area")).clicked()
                {
                    start_interval = Some(CaptureKind::LastRegion(x, y, w, h));
                }
            });
            if let Some(job) = &self.interval {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(crate::i18n::tf("Captures left: {count}", &[("count", &job.left.to_string())])).color(accent()));
                    if ui.button(crate::i18n::t("Stop")).clicked() {
                        stop_interval = true;
                    }
                });
            }
            ui.add_space(8.0);
            ui.label(RichText::new(crate::i18n::t("Schedule")).strong());
            ui.horizontal(|ui| {
                ui.label(crate::i18n::t("At"));
                ui.add(egui::TextEdit::singleline(&mut s.schedule_time).desired_width(60.0));
                ui.label(crate::i18n::t("(hh:mm)"));
                for (label, k) in &kinds {
                    if ui.button(*label).clicked() {
                        set_schedule = Some(k.clone());
                    }
                }
            });
            if let Some((t, _)) = &self.schedule {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(crate::i18n::tf("Capture scheduled at {time}", &[("time", &t.format("%H:%M").to_string())])).color(accent()));
                    if ui.button(crate::i18n::t("Cancel")).clicked() {
                        cancel_schedule = true;
                    }
                });
            }
        });
        self.timer_open = open;
        if stop_interval {
            self.interval = None;
        }
        if cancel_schedule {
            self.schedule = None;
        }
        if let Some(kind) = start_interval {
            self.interval = Some(IntervalJob {
                kind,
                every: Duration::from_secs(self.settings.interval_s.max(2)),
                left: self.settings.interval_count.max(1),
                next: Instant::now() + Duration::from_secs(self.settings.delay_s.max(1)),
            });
            self.status = crate::i18n::tf("Capturing every {seconds} s, {count} times.", &[("seconds", &self.settings.interval_s.to_string()), ("count", &self.settings.interval_count.to_string())]);
        }
        if let Some(kind) = set_schedule {
            match chrono::NaiveTime::parse_from_str(self.settings.schedule_time.trim(), "%H:%M") {
                Ok(t) => {
                    self.schedule = Some((t, kind));
                    self.status = crate::i18n::tf("Capture scheduled at {time}", &[("time", &t.format("%H:%M").to_string())]);
                }
                Err(_) => self.status = "Write the time as hh:mm, for example 14:30.".into(),
            }
        }
    }

    pub(crate) fn library_window(&mut self, ctx: &egui::Context) {
        if !self.library_open {
            self.lib_focused = false;
            return;
        }
        // Reading the library folder (maybe a cloud folder with thousands of files) is done on
        // a worker; the last list stays shown meanwhile.
        if !self.lib_loaded && self.lib_scan_rx.is_none() {
            self.lib_loaded = true;
            self.lib_added.clear();
            let folder = self.settings.library_folder.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            self.lib_scan_rx = Some((folder.clone(), rx));
            let ctx2 = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(library::scan(std::path::Path::new(&folder)));
                ctx2.request_repaint();
            });
        }
        let mut scanned = None;
        if let Some((folder, rx)) = &self.lib_scan_rx {
            match rx.try_recv() {
                Ok(items) => scanned = Some((folder.clone(), Some(items))),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => scanned = Some((folder.clone(), None)),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some((folder, items)) = scanned {
            self.lib_scan_rx = None;
            let added = std::mem::take(&mut self.lib_added);
            match items {
                Some(items) if folder == self.settings.library_folder => {
                    self.lib_items = library::merge_scan(items, added);
                    self.start_thumbnails(ctx);
                }
                // The folder changed while it was read: read the new one.
                _ => self.lib_loaded = false,
            }
        }
        if let Some(rx) = &self.lib_thumb_rx {
            for _ in 0..24 {
                let Ok((p, img)) = rx.try_recv() else { break };
                let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
                self.lib_thumbs.insert(p, ctx.load_texture("lib-thumb", ci, TextureOptions::LINEAR));
            }
        }
        self.library_ui(ctx);
    }

    /// Make the missing thumbnails on a worker; they're turned into textures a few a frame.
    fn start_thumbnails(&mut self, ctx: &egui::Context) {
        {
            let paths: Vec<PathBuf> =
                self.lib_items.iter().map(|i| i.path.clone()).filter(|p| !self.lib_thumbs.contains_key(p)).collect();
            let (tx, rx) = std::sync::mpsc::channel();
            self.lib_thumb_rx = Some(rx);
            let ctx2 = ctx.clone();
            std::thread::spawn(move || {
                for p in paths {
                    if let Some(t) = library::thumbnail(&p, 192) {
                        if tx.send((p, t)).is_err() {
                            break;
                        }
                        ctx2.request_repaint();
                    }
                }
            });
        }
    }

    fn library_ui(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut open_item = None;
        let mut delete_item = None;
        let mut show_item = None;
        let mut save_tags = None;
        let mut undo = false;
        own_window(ctx, Win { key: "library", title: crate::i18n::t("Library"), size: vec2(900.0, 640.0), resizable: true }, &mut open, |ui| {
            self.lib_focused = ui.input(|i| i.focused);
            // Right after a delete: what went, with Undo, at the foot (so the list doesn't move
            // under the pointer when it goes). Ctrl+Z / ⌘Z too, unless a text box has the keys.
            if self.delete_undo_showing() {
                let typing = ui.ctx().egui_wants_keyboard_input();
                undo = !typing && self.deleted.owns_shortcut(std::time::Instant::now()) && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Z));
                egui::Panel::bottom("library-undo").show_separator_line(false).show(ui, |ui| {
                    ui.add_space(6.0);
                    // The button undoes on its own.
                    self.delete_undo_bar(ui, 60);
                });
            }
            ui.horizontal(|ui| {
                ui.label(crate::i18n::t("Search"));
                ui.add(egui::TextEdit::singleline(&mut self.lib_query).hint_text(crate::i18n::t("Title, program, tag or date (2026-11)")).desired_width(260.0));
                let sorts = ["Newest first", "Oldest first", "By program", "By title"].map(crate::i18n::t);
                egui::ComboBox::from_id_salt("libsort")
                    .selected_text(sorts[self.lib_sort.min(sorts.len() - 1)])
                    .show_ui(ui, |ui| {
                        for (i, l) in sorts.iter().enumerate() {
                            ui.selectable_value(&mut self.lib_sort, i, *l);
                        }
                    });
                let mut apps: Vec<String> = self.lib_items.iter().map(|i| i.meta.app.clone()).filter(|a| !a.is_empty()).collect();
                apps.sort_by_key(|a| a.to_lowercase());
                apps.dedup();
                egui::ComboBox::from_id_salt("libapp")
                    .selected_text(if self.lib_app.is_empty() { crate::i18n::t("All programs").to_string() } else { self.lib_app.clone() })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.lib_app, String::new(), crate::i18n::t("All programs"));
                        for a in apps {
                            ui.selectable_value(&mut self.lib_app, a.clone(), a);
                        }
                    });
                if ui.button(crate::i18n::t("Refresh")).clicked() {
                    self.lib_loaded = false;
                }
                if ui.button(crate::i18n::t("Open folder")).clicked() {
                    let f = PathBuf::from(&self.settings.library_folder);
                    self.open_folder(&f);
                }
            });
            ui.horizontal(|ui| {
                let count = crate::i18n::tf("Captures: {count}", &[("count", &self.lib_items.len().to_string())]);
                ui.label(RichText::new(format!("{}  ({count})", self.settings.library_folder)).color(pal().dim).size(12.0));
                if self.lib_scan_rx.is_some() {
                    ui.spinner();
                }
            });
            ui.separator();
            let mut order: Vec<usize> = (0..self.lib_items.len()).filter(|i| self.lib_items[*i].matches(&self.lib_query)).collect();
            match self.lib_sort {
                1 => order.reverse(),
                2 => order.sort_by(|a, b| self.lib_items[*a].meta.app.to_lowercase().cmp(&self.lib_items[*b].meta.app.to_lowercase())),
                3 => order.sort_by(|a, b| self.lib_items[*a].meta.title.to_lowercase().cmp(&self.lib_items[*b].meta.title.to_lowercase())),
                _ => {}
            }
            if !self.lib_app.is_empty() {
                order.retain(|i| self.lib_items[*i].meta.app == self.lib_app);
            }
            // Delete (⌘⌫ on a Mac) moves the picked capture to the Recycle Bin / Trash, unless
            // a text box (the search) has the keys.
            if !ui.ctx().text_edit_focused() && ui.input(crate::platform::trash_key_pressed) {
                delete_item = picked_index(&self.lib_items, &order, self.lib_picked.as_deref());
            }
            if order.is_empty() {
                ui.label(RichText::new(crate::i18n::t("Nothing here yet. Captures are added automatically (Settings > After capture).")).color(pal().dim));
            }
            // Group by month (date order) or by program.
            let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
            for &i in &order {
                let m = &self.lib_items[i].meta;
                let head = match self.lib_sort {
                    0 | 1 => m.created.get(..7).unwrap_or(crate::i18n::t("Unknown date")).to_string(),
                    2 => if m.app.is_empty() { crate::i18n::t("Unknown program").to_string() } else { m.app.clone() },
                    _ => String::new(),
                };
                match groups.last_mut() {
                    Some((h, v)) if *h == head => v.push(i),
                    _ => groups.push((head, vec![i])),
                }
            }
            // Tiles grow with Windows' Text size, and their lines go one under the other at the
            // height their text really takes, so nothing overlaps at 150 % and above.
            let fonts = TILE_FONTS.map(egui::FontId::proportional);
            let rows = fonts.clone().map(|f| ui.fonts_mut(|x| x.row_height(&f)));
            let tile = tile_layout(crate::fonts::text_scale(), rows);
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (head, members) in &groups {
                    if !head.is_empty() {
                        ui.add_space(4.0);
                        ui.label(RichText::new(format!("{head}   ({})", members.len())).strong().color(pal().strong));
                    }
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                    for &i in members {
                        let item = &self.lib_items[i];
                        let (rect, resp) = ui.allocate_exact_size(tile.size, Sense::click());
                        let picked = self.lib_picked.as_ref() == Some(&item.path);
                        let title_for_reader = item.meta.title.clone();
                        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::ListItem, true, picked, &title_for_reader));
                        let p = ui.painter();
                        p.rect_filled(rect, CornerRadius::same(8), if resp.hovered() { pal().card_hover } else { pal().card });
                        if picked {
                            p.rect_stroke(rect, CornerRadius::same(8), Stroke::new(2.0, accent()), StrokeKind::Inside);
                        }
                        // A click picks it (as in Explorer, Files and Finder), a right click too.
                        if resp.clicked() || resp.secondary_clicked() {
                            self.lib_picked = Some(item.path.clone());
                        }
                        let thumb_area = tile.thumb.translate(rect.min.to_vec2());
                        if let Some(t) = self.lib_thumbs.get(&item.path) {
                            let sz = t.size_vec2();
                            let s = (thumb_area.width() / sz.x).min(thumb_area.height() / sz.y);
                            p.image(t.id(), Rect::from_center_size(thumb_area.center(), sz * s), Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
                        }
                        let date = item.meta.created.get(..16).unwrap_or(&item.meta.created).replace('T', " ");
                        let lines = [
                            (item.meta.title.clone(), pal().card_text),
                            (format!("{}  {}", date, item.meta.app), pal().dim),
                            (if item.meta.tags.is_empty() { String::new() } else { format!("# {}", item.meta.tags.join(", ")) }, accent()),
                        ];
                        // The lines the tile had to cut short are shown whole in its tooltip.
                        let mut whole = Vec::new();
                        for (k, (text, color)) in lines.into_iter().enumerate() {
                            if !text.is_empty() {
                                let g = one_line(p, text.clone(), fonts[k].clone(), color, tile.text_width);
                                if was_cut(&g, &text) {
                                    whole.push(text);
                                }
                                p.galley(rect.min + vec2(TILE_PAD + 2.0, tile.rows[k]), g, color);
                            }
                        }
                        if resp.double_clicked() || resp.clicked_by(PointerButton::Primary) && ui.input(|i| i.modifiers.ctrl) {
                            open_item = Some(i);
                        }
                        whole.push(crate::i18n::t("Double-click to open. Right-click for tags and more.").to_string());
                        let resp = resp.on_hover_text(whole.join("\n"));
                        resp.context_menu(|ui| {
                            if ui.button(crate::i18n::t("Open in Snagster")).clicked() {
                                open_item = Some(i);
                                ui.close();
                            }
                            if ui.button(crate::i18n::t("Edit tags…")).clicked() {
                                self.lib_tag_edit = Some((i, self.lib_items[i].meta.tags.join(", ")));
                                ui.close();
                            }
                            if ui.button(crate::i18n::t("Show in folder")).clicked() {
                                show_item = Some(i);
                                ui.close();
                            }
                            if ui.button(crate::i18n::t(crate::platform::trash_text("Move to Recycle Bin", "Move to Trash"))).clicked() {
                                delete_item = Some(i);
                                ui.close();
                            }
                        });
                    }
                });
                }
            });
            if let Some((i, text)) = &mut self.lib_tag_edit {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::t("Tags (comma separated)"));
                    ui.add(egui::TextEdit::singleline(text).desired_width(300.0));
                    if ui.button(crate::i18n::t("Save tags")).clicked() || ui.input(|inp| inp.key_pressed(Key::Enter)) {
                        save_tags = Some((*i, text.clone()));
                    }
                });
            }
        });
        if let Some((i, text)) = save_tags {
            if let Some(item) = self.lib_items.get_mut(i) {
                item.meta.tags = text.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
                if let Err(e) = item.save_meta() {
                    self.status = crate::i18n::tf("Couldn't save the tags: {error}", &[("error", &e.to_string())]);
                }
            }
            self.lib_tag_edit = None;
        }
        if undo {
            self.undo_delete();
        }
        if let Some(i) = open_item {
            let p = self.lib_items[i].path.clone();
            self.open_path(&p);
        }
        if let Some(i) = show_item {
            let p = self.lib_items[i].path.clone();
            self.open_folder(&p);
        }
        if let Some(i) = delete_item {
            // To the Recycle Bin / Trash, where it can be brought back (never deleted for good).
            // An Undo shows right away, here and in the editor.
            match library::delete(&self.lib_items[i]) {
                Ok(g) => {
                    let gone = self.lib_items.remove(i);
                    if self.lib_picked.as_ref() == Some(&gone.path) {
                        self.lib_picked = None;
                    }
                    // The tag box holds an index into the list, which just moved.
                    self.lib_tag_edit = None;
                    self.deleted(g);
                }
                Err(e) => self.status = crate::i18n::tf(crate::platform::trash_text("Couldn't move it to the Recycle Bin: {error}", "Couldn't move it to the Trash: {error}"), &[("error", &e)]),
            }
        }
        self.library_open = open;
    }

    pub(crate) fn gif_window(&mut self, ctx: &egui::Context) {
        if !self.gif_open {
            return;
        }
        let mut open = true;
        let mut load: Option<PathBuf> = None;
        let mut grab = None;
        let mut save_trim = false;
        let mut append = false;
        let mut slideshow = false;
        own_window(ctx, Win { key: "gifs", title: crate::i18n::t("GIF recordings"), size: vec2(740.0, 600.0), resizable: true }, &mut open, |ui| {
            ui.horizontal(|ui| {
                if ui.button(crate::i18n::t("Open a GIF…")).clicked()
                    && let Some(p) = rfd::FileDialog::new().add_filter("GIF", &["gif"]).pick_file()
                {
                    load = Some(p);
                }
                if ui.add_enabled(self.entries.len() >= 2, egui::Button::new(crate::i18n::t("Slideshow from Recent"))).clicked() {
                    slideshow = true;
                }
                ui.label(RichText::new(crate::i18n::t("Record with Record > Record an area as GIF")).color(pal().dim));
            });
            if !self.recordings.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(crate::i18n::t("This session:"));
                    for r in &self.recordings {
                        let name = r.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                        if ui.button(format!("{name} ({} frames, {:.1} s)", r.frames, r.seconds)).clicked() {
                            load = Some(r.path.clone());
                        }
                    }
                });
            }
            ui.separator();
            let Some(ed) = &mut self.gif_editor else {
                ui.label(RichText::new(crate::i18n::t("No GIF open.")).color(pal().dim));
                return;
            };
            let n = ed.frames.len();
            if ed.loading.is_some() {
                // The frames are still being read: show what's here, and that more come.
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(format!("{n}")).color(pal().dim));
                });
            }
            if ed.playing && !ed.frames.is_empty() {
                let ms = ed.frames[ed.index].1.max(20) as u128;
                if ed.shown_at.elapsed().as_millis() >= ms {
                    ed.index = if ed.index >= ed.trim.1 || ed.index < ed.trim.0 { ed.trim.0 } else { ed.index + 1 };
                    ed.shown_at = Instant::now();
                }
                ui.ctx().request_repaint_after(Duration::from_millis(20));
            }
            let avail = vec2(ui.available_width(), 320.0);
            let s = (avail.x / ed.size.x).min(avail.y / ed.size.y).min(1.5);
            let (r, _) = ui.allocate_exact_size(ed.size * s, Sense::hover());
            if let Some((t, _)) = ed.frames.get(ed.index) {
                ui.painter().image(t.id(), r, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
            }
            ui.painter().rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0, pal().border), StrokeKind::Outside);
            ui.horizontal(|ui| {
                if ui.button(if ed.playing { "Pause" } else { "Play" }).clicked() {
                    ed.playing = !ed.playing;
                }
                let mut idx = ed.index;
                if ui.add(egui::Slider::new(&mut idx, 0..=n.saturating_sub(1)).text("frame")).changed() {
                    ed.index = idx;
                    ed.playing = false;
                }
            });
            ui.horizontal(|ui| {
                ui.label(crate::i18n::t("Keep frames"));
                let max = n.saturating_sub(1);
                ui.add(egui::DragValue::new(&mut ed.trim.0).range(0..=max));
                ui.label(crate::i18n::t("to"));
                ui.add(egui::DragValue::new(&mut ed.trim.1).range(0..=max));
                if ui.button(crate::i18n::t("Start here")).clicked() {
                    ed.trim.0 = ed.index;
                }
                if ui.button(crate::i18n::t("End here")).clicked() {
                    ed.trim.1 = ed.index;
                }
                if ed.trim.0 > ed.trim.1 {
                    std::mem::swap(&mut ed.trim.0, &mut ed.trim.1);
                }
            });
            ui.horizontal(|ui| {
                if ui.button(crate::i18n::t("Save trimmed GIF as…")).clicked() {
                    save_trim = true;
                }
                if ui.button(crate::i18n::t("Grab this frame as a picture")).clicked() {
                    grab = Some(ed.index);
                }
                if ui.button(crate::i18n::t("Add another GIF to the end…")).clicked() {
                    append = true;
                }
            });
            ui.label(RichText::new(format!("{}", ed.path.display())).color(pal().dim).size(11.5));
        });
        self.gif_open = open;
        if slideshow {
            self.slideshow(ctx);
        }
        if let Some(p) = load {
            self.open_gif(ctx, p);
        }
        let Some(path) = self.gif_editor.as_ref().map(|e| e.path.clone()) else { return };
        // Reading a whole GIF at full size and writing one take seconds: they run on the job
        // queue and report in the status line.
        if let Some(i) = grab {
            let path = path.clone();
            self.jobs.queue(move || {
                let r = export::gif_frames(&path, 0);
                Box::new(move |app: &mut SnagApp, _: &egui::Context| match r {
                    Ok(frames) => {
                        if let Some((img, _)) = frames.into_iter().nth(i) {
                            let name = app.next_name("Frame");
                            app.add_entry(name, Doc::new(img));
                            app.status = crate::i18n::tf("Frame {number} is now a picture you can mark up.", &[("number", &(i + 1).to_string())]);
                        }
                    }
                    Err(e) => app.status = crate::i18n::tf("Couldn't read the frame: {error}", &[("error", &e.to_string())]),
                })
            });
        }
        if save_trim {
            let (a, b) = self.gif_editor.as_ref().map(|e| e.trim).unwrap_or((0, 0));
            if let Some(out) = rfd::FileDialog::new().add_filter("GIF", &["gif"]).set_file_name("trimmed.gif").save_file() {
                let path = path.clone();
                self.jobs.queue(move || {
                    let r = export::trim_gif(&path, a, b, &out);
                    Box::new(move |app: &mut SnagApp, _: &egui::Context| {
                        app.status = match r {
                            Ok(_) =>format!("Saved frames {}-{} to {}", a + 1, b + 1, out.display()),
                            Err(e) => format!("Couldn't save: {e}"),
                        };
                    })
                });
            }
        }
        if append && let Some(other) = rfd::FileDialog::new().add_filter("GIF", &["gif"]).pick_file() {
            self.jobs.queue(move || {
                let r = export::combine_gifs(&path, &other);
                Box::new(move |app: &mut SnagApp, ctx: &egui::Context| match r {
                    Ok(out) => {
                        app.status = crate::i18n::tf("Combined into {path}", &[("path", &out.display().to_string())]);
                        app.open_gif(ctx, out);
                    }
                    Err(e) => app.status = crate::i18n::tf("Couldn't combine: {error}", &[("error", &e.to_string())]),
                })
            });
        }
    }
}


impl SnagApp {
    pub(crate) fn show_pins(&mut self, ctx: &egui::Context) {
        let ppp = ctx.pixels_per_point();
        for pin in &mut self.pins {
            if pin.close {
                continue;
            }
            let (iw, ih) = (pin.image.width() as f32, pin.image.height() as f32);
            let fit = (900.0 / iw).min(700.0 / ih).min(1.0);
            let size = vec2(iw * fit / ppp, ih * fit / ppp);
            let mut builder = snagster_viewport()
                .with_title(format!("Snagster pin: {}", pin.title))
                .with_inner_size(size)
                .with_decorations(false)
                .with_taskbar(false);
            if crate::app::UI_TEST.load(std::sync::atomic::Ordering::Relaxed) {
                builder = builder.with_active(false).with_window_level(egui::WindowLevel::AlwaysOnBottom);
            } else {
                builder = builder.with_always_on_top();
            }
            let image = &pin.image;
            let tex_slot = &mut pin.tex;
            let mut close = false;
            ctx.show_viewport_immediate(egui::ViewportId::from_hash_of(("pin", pin.id)), builder, |ui, _class| {
                let tex = tex_slot
                    .get_or_insert_with(|| {
                        let ci = egui::ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw());
                        ui.ctx().load_texture("pin", ci, TextureOptions::LINEAR)
                    })
                    .id();
                let rect = ui.max_rect();
                let resp = ui.allocate_rect(rect, Sense::click_and_drag());
                ui.painter().image(tex, rect, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
                ui.painter().rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, accent()), StrokeKind::Inside);
                if resp.drag_started_by(PointerButton::Primary) {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                if resp.double_clicked() || ui.input(|i| i.key_pressed(Key::Escape) || i.viewport().close_requested()) {
                    close = true;
                }
                resp.on_hover_text(crate::i18n::t("Drag to move. Double-click or Esc closes. Right-click for more.")).context_menu(|ui| {
                    if ui.button(crate::i18n::t("Copy")).clicked() {
                        let data = arboard::ImageData {
                            width: image.width() as usize,
                            height: image.height() as usize,
                            bytes: image.as_raw().clone().into(),
                        };
                        let _ = arboard::Clipboard::new().and_then(|mut c| c.set_image(data));
                        ui.close();
                    }
                    if ui.button(crate::i18n::t("Close pin")).clicked() {
                        close = true;
                        ui.close();
                    }
                });
            });
            if close {
                pin.close = true;
            }
        }
        self.pins.retain(|p| !p.close);
    }
}

#[allow(dead_code)]
fn unused(_a: Action) {}

// ---------- small dialogs ----------

pub(crate) enum Dialog {
    NewImage { w: u32, h: u32, color: [u8; 4] },
    Resize { w: u32, h: u32, keep: bool, ratio: f32 },
    Canvas { w: u32, h: u32, color: [u8; 4] },
    Rename(String),
    Export { pick: Vec<bool>, folder: String, format: ImageFormat },
    GettingStarted,
    WhatsNew,
}

impl Dialog {
    pub fn new_image() -> Self {
        Dialog::NewImage { w: 1280, h: 720, color: [255, 255, 255, 255] }
    }
    pub fn export(n: usize, s: &Settings) -> Self {
        Dialog::Export { pick: vec![true; n], folder: s.save_folder.clone(), format: s.format }
    }
}

/// A new window of Snagster's own (dialog, pin, bar). It carries Snagster's app id, so the
/// desktop shows Snagster's icon and groups it with the main window: without it, KDE Plasma
/// guesses and may show another program's icon and a taskbar entry of its own.
pub fn snagster_viewport() -> egui::ViewportBuilder {
    egui::ViewportBuilder::default().with_app_id(crate::APP_ID)
}

/// A dialog's main button (`action`, English) and Cancel, in this system's order.
fn dialog_row(ui: &mut egui::Ui, action: &'static str, enabled: bool) -> Option<Choice> {
    crate::platform::dialog_buttons(ui, Some(crate::i18n::t(action)), crate::i18n::t("Cancel"), crate::platform::Kind::Normal, enabled)
}

fn color4_edit(ui: &mut egui::Ui, c: &mut [u8; 4]) {
    let mut col = Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
    let r = egui::color_picker::color_edit_button_srgba(ui, &mut col, egui::color_picker::Alpha::OnlyBlend);
    swatch_edge_on(ui, &r, col);
    if r.changed() {
        *c = col.to_srgba_unmultiplied();
    }
}

impl SnagApp {
    pub(crate) fn dialog_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.dialog.take() else { return };
        let mut open = true;
        let mut done = false;
        let title = match &d {
            Dialog::NewImage { .. } => crate::i18n::t("New image"),
            Dialog::Resize { .. } => crate::i18n::t("Resize image"),
            Dialog::Canvas { .. } => crate::i18n::t("Canvas size and color"),
            Dialog::Rename(_) => crate::i18n::t("Rename"),
            Dialog::Export { .. } => crate::i18n::t("Export pictures"),
            Dialog::GettingStarted => crate::i18n::t("Getting started"),
            Dialog::WhatsNew => crate::i18n::t("What's new in Snagster"),
        };
        let mut action: Option<Box<dyn FnOnce(&mut SnagApp)>> = None;
        own_window(ctx, Win { key: "dialog", title, size: vec2(440.0, 220.0), resizable: false }, &mut open, |ui| match &mut d {
            Dialog::NewImage { w, h, color } => {
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::t("Width"));
                    ui.add(egui::DragValue::new(w).range(1..=10000));
                    ui.label(crate::i18n::t("Height"));
                    ui.add(egui::DragValue::new(h).range(1..=10000));
                });
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::t("Background"));
                    color4_edit(ui, color);
                    if ui.button(crate::i18n::t("Transparent")).clicked() {
                        *color = [0, 0, 0, 0];
                    }
                });
                let pick = dialog_row(ui, "Create", true);
                done |= pick.is_some();
                if pick == Some(Choice::Action) {
                    let (w, h, c) = (*w, *h, *color);
                    action = Some(Box::new(move |app| {
                        let name = app.next_name("New image");
                        app.add_entry(name, Doc::new(image::RgbaImage::from_pixel(w, h, image::Rgba(c))));
                    }));
                    done = true;
                }
            }
            Dialog::Resize { w, h, keep, ratio } => {
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::t("Width"));
                    if ui.add(egui::DragValue::new(w).range(1..=20000)).changed() && *keep {
                        *h = ((*w as f32 / *ratio).round() as u32).max(1);
                    }
                    ui.label(crate::i18n::t("Height"));
                    if ui.add(egui::DragValue::new(h).range(1..=20000)).changed() && *keep {
                        *w = ((*h as f32 * *ratio).round() as u32).max(1);
                    }
                });
                ui.checkbox(keep, crate::i18n::t("Keep the proportions"));
                let pick = dialog_row(ui, "Resize", true);
                done |= pick.is_some();
                if pick == Some(Choice::Action) {
                    let (w, h) = (*w, *h);
                    action = Some(Box::new(move |app| app.image_op("Resized", |img| crate::effects::resize_exact(img, w, h))));
                    done = true;
                }
            }
            Dialog::Canvas { w, h, color } => {
                ui.label(RichText::new(crate::i18n::t("The picture stays in the middle; the rest gets the color.")).color(pal().dim));
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::t("Width"));
                    ui.add(egui::DragValue::new(w).range(1..=20000));
                    ui.label(crate::i18n::t("Height"));
                    ui.add(egui::DragValue::new(h).range(1..=20000));
                });
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::t("Color"));
                    color4_edit(ui, color);
                    if ui.button(crate::i18n::t("Transparent")).clicked() {
                        *color = [0, 0, 0, 0];
                    }
                });
                let pick = dialog_row(ui, "Apply", true);
                done |= pick.is_some();
                if pick == Some(Choice::Action) {
                    let (w, h, c) = (*w, *h, *color);
                    action = Some(Box::new(move |app| app.image_op("New canvas", |img| crate::effects::canvas(img, w, h, c))));
                    done = true;
                }
            }
            Dialog::Rename(name) => {
                let r = ui.text_edit_singleline(name);
                r.request_focus();
                ui.label(RichText::new(crate::i18n::t("A saved picture's file is renamed too.")).color(pal().dim));
                let mut pick = dialog_row(ui, "Rename", !name.trim().is_empty());
                // The name field has the keyboard, so Return is handled here.
                if pick.is_none() && !name.trim().is_empty() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    pick = Some(Choice::Action);
                }
                done |= pick.is_some();
                if pick == Some(Choice::Action) {
                    let n = name.clone();
                    action = Some(Box::new(move |app| app.rename_current(&n)));
                    done = true;
                }
            }
            Dialog::Export { pick, folder, format } => {
                ui.label(crate::i18n::t("Pictures:"));
                egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                    for (i, p) in pick.iter_mut().enumerate() {
                        let name = self.entries.get(i).map(|e| e.name.clone()).unwrap_or_default();
                        ui.checkbox(p, name);
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button(crate::i18n::t("All")).clicked() {
                        pick.iter_mut().for_each(|p| *p = true);
                    }
                    if ui.button(crate::i18n::t("None")).clicked() {
                        pick.iter_mut().for_each(|p| *p = false);
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::t("Folder"));
                    ui.add(egui::TextEdit::singleline(folder).desired_width(300.0));
                    if ui.button(crate::i18n::t("Browse…")).clicked()
                        && let Some(f) = rfd::FileDialog::new().pick_folder()
                    {
                        *folder = f.display().to_string();
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(crate::i18n::t("Format"));
                    for f in ImageFormat::ALL {
                        ui.radio_value(format, f, f.label());
                    }
                });
                let n = pick.iter().filter(|p| **p).count();
                let choice = crate::platform::dialog_buttons(ui, Some(&crate::i18n::tf("Export {n} pictures", &[("n", &n.to_string())])), crate::i18n::t("Cancel"), crate::platform::Kind::Normal, n > 0);
                done |= choice.is_some();
                if choice == Some(Choice::Action) {
                    let (pick, folder, format) = (pick.clone(), PathBuf::from(folder.clone()), *format);
                    action = Some(Box::new(move |app| app.export_many(&pick, &folder, format)));
                    done = true;
                }
            }
            Dialog::GettingStarted => {
                ui.set_max_width(520.0);
                for (head, body) in [
                    ("1. Capture", "Press Print Screen (or the Capture button). Drag an area, or click a window, button or menu. A toolbar then lets you capture, scroll, record a GIF or grab the text."),
                    ("2. Mark it up", "Pick a tool on the left: arrows, text, callouts, steps, stamps, blur, spotlight, magnify... The options for each tool are on the right."),
                    ("3. Share it", "Copy it (Ctrl+C), save it (Ctrl+S), pin it on screen, email it, or put several into a guide (File > Export guide)."),
                    ("Everything is kept", "Every capture goes into the Library (Ctrl+L), searchable by date, program and tags."),
                    ("From anywhere", "Snagster's icon in the taskbar corner opens the capture panel; right-click it for the quick menu."),
                ] {
                    ui.label(RichText::new(head).strong());
                    ui.label(body);
                    ui.add_space(6.0);
                }
                if ui.button(crate::i18n::t("Try it on the sample picture")).clicked() {
                    action = Some(Box::new(|app| {
                        let name = app.next_name("Sample");
                        app.add_entry(name, crate::sample::sample_doc());
                    }));
                    done = true;
                }
            }
            Dialog::WhatsNew => {
                ui.set_max_width(520.0);
                for line in [
                    "Hotkeys can take over keys another program uses (Settings > Hotkeys).",
                    "New tools: Selection (lift part of the picture and move it), Fill and Eraser.",
                    "Arrows with heads at either end, text alignment and outline, callout shapes, blur types.",
                    "Quick styles, a customizable toolbar, cut / copy / paste of marks.",
                    "Capture several areas at once, menus with their sub-menus, and scrolling you do yourself.",
                    "New image, templates, open recent, rename, Recycle Bin, batch export, print, email, Word documents.",
                    "Presets can be imported, exported and duplicated; Snagster can start with Windows.",
                ] {
                    ui.label(format!("- {line}"));
                }
            }
        });
        if let Some(a) = action {
            a(self);
        }
        if open && !done {
            self.dialog = Some(d);
        }
    }

    /// Save the picked pictures into `folder` on the job queue: encoding many big pictures takes
    /// seconds, and the window must keep answering meanwhile (it used to stop answering). Each
    /// gets a file name of its own, also when two pictures have the same title.
    pub(crate) fn export_many(&mut self, pick: &[bool], folder: &std::path::Path, format: ImageFormat) {
        let mut items = Vec::new();
        for (i, p) in pick.iter().enumerate() {
            if !*p || i >= self.entries.len() {
                continue;
            }
            let title = self.entries[i].name.clone();
            let base = self.settings.file_name(&title);
            let path = crate::app::work::free_path(folder.join(format!("{base}.{}", format.ext())), &self.saving);
            self.saving.insert(path.clone());
            items.push((self.entries[i].doc.clone(), title, path));
        }
        let (settings, folder) = (self.settings.clone(), folder.to_path_buf());
        self.status = crate::i18n::tf("Exporting {n} pictures to {folder}…", &[("n", &items.len().to_string()), ("folder", &folder.display().to_string())]);
        self.jobs.queue(move || {
            let (mut ok, mut last_err, mut paths) = (0, None, Vec::new());
            for (doc, title, path) in items {
                match export::save_doc(&doc, &title, &path, &settings) {
                    Ok(()) => ok += 1,
                    Err(e) => last_err = Some(e),
                }
                paths.push(path);
            }
            Box::new(move |app: &mut SnagApp, _: &egui::Context| {
                for p in &paths {
                    app.saving.remove(p);
                }
                app.status = match last_err {
                    None => format!("Exported {ok} pictures to {}", folder.display()),
                    Some(e) => format!("Exported {ok}; some failed: {e}"),
                };
            })
        });
    }

    pub(crate) fn import_presets(&mut self) {
        let Some(p) = rfd::FileDialog::new().add_filter("Snagster presets", &["json"]).pick_file() else { return };
        match std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|s| serde_json::from_str::<Vec<Preset>>(&s).map_err(|e| e.to_string())) {
            Ok(list) => {
                let n = list.len();
                for mut pr in list {
                    // Imported hotkeys could clash; they start without one.
                    pr.combo = Combo::default();
                    self.settings.presets.push(pr);
                }
                self.status = crate::i18n::tf("Presets imported: {count}", &[("count", &n.to_string())]);
            }
            Err(e) => self.status = crate::i18n::tf("Couldn't import presets: {error}", &[("error", &e.to_string())]),
        }
    }

    pub(crate) fn export_presets(&mut self, only: Option<usize>) {
        let list: Vec<Preset> = match only {
            Some(i) => self.settings.presets.get(i).cloned().into_iter().collect(),
            None => self.settings.presets.clone(),
        };
        let name = if only.is_some() { "snag-preset.json" } else { "snag-presets.json" };
        let Some(p) = rfd::FileDialog::new().add_filter("Snagster presets", &["json"]).set_file_name(name).save_file() else { return };
        self.status = match serde_json::to_string_pretty(&list).map_err(|e| e.to_string()).and_then(|s| std::fs::write(&p, s).map_err(|e| e.to_string())) {
            Ok(()) => format!("Exported {} presets to {}", list.len(), p.display()),
            Err(e) => format!("Couldn't export presets: {e}"),
        };
    }
}

/// The encoders to offer: each name once, in `available`'s order (graphics chip first).
pub(crate) fn encoder_choices(list: &[(crate::encode::Codec, crate::encode::EncoderName)]) -> Vec<crate::encode::EncoderName> {
    let mut out: Vec<crate::encode::EncoderName> = Vec::new();
    for (_, n) in list {
        if !out.iter().any(|o| o.name == n.name) {
            out.push(n.clone());
        }
    }
    out
}

/// The video encoder choice (recording and streaming share it): Automatic, then this
/// computer's encoders. A choice that is gone, or can't start, falls back by itself.
pub(crate) fn encoder_picker(ui: &mut egui::Ui, salt: &str, pick: &mut String, list: &[(crate::encode::Codec, crate::encode::EncoderName)]) {
    use crate::i18n::t;
    let auto = t("Automatic (recommended)");
    let choices = encoder_choices(list);
    ui.horizontal(|ui| {
        ui.label(t("Video encoder"));
        let shown = if pick.is_empty() { auto.to_string() } else { pick.clone() };
        egui::ComboBox::from_id_salt(salt).width(320.0).selected_text(shown).show_ui(ui, |ui| {
            ui.selectable_value(pick, String::new(), auto);
            for n in &choices {
                let kind = if n.hardware { t("graphics chip") } else { t("processor") };
                ui.selectable_value(pick, n.name.clone(), format!("{} ({kind})", n.name));
            }
        });
    })
    .response
    .on_hover_text(t("Automatic uses the best encoder that works. If the one you choose can't start or stops working, Snagster switches to the best one that works and says so."));
    if !pick.is_empty() && !list.is_empty() && !choices.iter().any(|n| n.name == *pick) {
        ui.label(RichText::new(t("The chosen encoder isn't on this computer now, so Automatic is used.")).color(pal().dim));
    }
}

/// Where Windows keeps what starts when you sign in (under HKEY_CURRENT_USER).
#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// Text values under HKEY_CURRENT_USER through the registry API (microseconds; no `reg.exe`).
#[cfg(windows)]
mod registry {
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW};
    use windows::core::HSTRING;

    pub fn get(key: &str, name: &str) -> Option<String> {
        let (k, n) = (HSTRING::from(key), HSTRING::from(name));
        let mut bytes = 0u32;
        unsafe {
            if RegGetValueW(HKEY_CURRENT_USER, &k, &n, RRF_RT_REG_SZ, None, None, Some(&mut bytes)) != ERROR_SUCCESS {
                return None;
            }
            let mut buf = vec![0u16; (bytes as usize).div_ceil(2).max(1)];
            if RegGetValueW(HKEY_CURRENT_USER, &k, &n, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut bytes)) != ERROR_SUCCESS {
                return None;
            }
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(String::from_utf16_lossy(&buf[..len]))
        }
    }

    pub fn set(key: &str, name: &str, value: &str) -> Result<(), String> {
        let data: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
        let r = unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &HSTRING::from(name), REG_SZ.0, Some(data.as_ptr().cast()), (data.len() * 2) as u32) };
        r.ok().map_err(|e| e.to_string())
    }

    /// Remove a value; one that isn't there is fine.
    pub fn delete(key: &str, name: &str) -> Result<(), String> {
        let r = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &HSTRING::from(name)) };
        if r == ERROR_SUCCESS || r == ERROR_FILE_NOT_FOUND { Ok(()) } else { r.ok().map_err(|e| e.to_string()) }
    }

    #[cfg(test)]
    mod tests {
        /// In a key of its own (never the real Run key), removed afterwards.
        #[test]
        fn values_are_written_read_back_and_removed() {
            let key = format!(r"Software\Snagster-test-{}", std::process::id());
            assert_eq!(super::get(&key, "v"), None);
            super::set(&key, "v", "\"C:\\Program Files\\Snagster æøå\\Snagster.exe\"").unwrap();
            assert_eq!(super::get(&key, "v").as_deref(), Some("\"C:\\Program Files\\Snagster æøå\\Snagster.exe\""));
            super::delete(&key, "v").unwrap();
            assert_eq!(super::get(&key, "v"), None);
            super::delete(&key, "v").expect("removing what isn't there is fine");
            unsafe {
                let _ = windows::Win32::System::Registry::RegDeleteKeyW(super::HKEY_CURRENT_USER, &super::HSTRING::from(key.as_str()));
            }
        }
    }
}

/// Start Snagster when Windows starts: the Run key in the registry, or the package's startup
/// task when Snagster was installed from its MSIX (its folder changes with every update).
pub fn set_run_at_startup(on: bool) -> Result<(), String> {
    #[cfg(windows)]
    if crate::packaged::is_packaged() {
        return crate::packaged::set_startup(on);
    }
    #[cfg(windows)]
    {
        // Straight through the registry API: it used to start `reg.exe` (twice) and wait for it
        // on the window's thread.
        // The entry from before the rename.
        let _ = registry::delete(RUN_KEY, "Snag");
        if !on {
            return registry::delete(RUN_KEY, "Snagster");
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        return registry::set(RUN_KEY, "Snagster", &format!("\"{}\"", exe.display()));
    }
    #[cfg(not(windows))]
    {
        let Some(file) = autostart_file() else { return Err("No home folder.".into()) };
        if !on {
            let _ = std::fs::remove_file(&file);
            return Ok(());
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        #[cfg(target_os = "macos")]
        let body = launch_agent(&exe);
        #[cfg(not(target_os = "macos"))]
        let body = desktop_entry(&exe);
        std::fs::write(&file, body).map_err(|e| e.to_string())
    }
}

/// The macOS LaunchAgent that starts `exe` at login.
#[cfg(any(target_os = "macos", test))]
fn launch_agent(exe: &std::path::Path) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>app.snagster</string><key>ProgramArguments</key><array><string>{}</string></array><key>RunAtLoad</key><true/></dict></plist>\n",
        exe.display()
    )
}

/// The Linux autostart entry that starts `exe` at login.
#[cfg(any(all(not(windows), not(target_os = "macos")), test))]
fn desktop_entry(exe: &std::path::Path) -> String {
    format!("[Desktop Entry]\nType=Application\nName=Snagster\nExec=\"{}\"\nX-GNOME-Autostart-enabled=true\n", exe.display())
}

/// Where Linux (autostart entry) or macOS (LaunchAgent) looks for programs to start at login.
#[cfg(not(windows))]
fn autostart_file() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    #[cfg(target_os = "macos")]
    return Some(home.join("Library/LaunchAgents/app.snagster.plist"));
    #[cfg(not(target_os = "macos"))]
    Some(home.join(".config/autostart/snagster.desktop"))
}

pub fn runs_at_startup() -> bool {
    #[cfg(windows)]
    if crate::packaged::is_packaged() {
        return crate::packaged::startup() == Ok(crate::packaged::Startup::On);
    }
    #[cfg(windows)]
    {
        registry::get(RUN_KEY, "Snagster").is_some()
    }
    #[cfg(not(windows))]
    {
        autostart_file().is_some_and(|f| f.exists())
    }
}

impl SnagApp {
    /// How to let AI apps (Claude Code, Claude Desktop, Cursor...) use Snagster over MCP.
    fn ai_tab(&mut self, ui: &mut egui::Ui) {
        use crate::i18n::{t, tf};
        let exe = crate::packaged::mcp_command();
        ui.label(t("Snagster is an MCP server: any AI app that speaks MCP can capture the screen, mark up pictures, read text, record videos and run Studio. Apps on this computer only; nothing goes online."));
        ui.add_space(6.0);
        let copy_row = |ui: &mut egui::Ui, label: &str, text: String| {
            ui.label(RichText::new(label).strong());
            ui.horizontal(|ui| {
                let mut tx = text.clone();
                let rows = tx.lines().count().max(1);
                ui.add(egui::TextEdit::multiline(&mut tx).code_editor().desired_rows(rows).desired_width(470.0));
                if ui.button(t("Copy")).clicked() {
                    let _ = arboard::Clipboard::new().and_then(|mut c| c.set_text(text.clone()));
                }
            });
            ui.add_space(6.0);
        };
        egui::CollapsingHeader::new(RichText::new(t("Apps that start Snagster themselves (stdio)")).strong()).default_open(true).show(ui, |ui| {
            ui.label(RichText::new(t("Claude Desktop, Claude Code, Cursor, VS Code, Windsurf, Zed, LM Studio, Jan, Cline, Continue, Goose and others.")).color(pal().dim));
            copy_row(ui, t("Claude Code (run in a terminal)"), format!("claude mcp add snagster -- \"{exe}\" --mcp"));
            let json = serde_json::to_string_pretty(&serde_json::json!({
                "mcpServers": {"snagster": {"command": exe, "args": ["--mcp"]}}
            }))
            .unwrap_or_default();
            copy_row(ui, t("Claude Desktop, Cursor, LM Studio (mcp.json), Jan and most others"), json);
            let vscode = serde_json::to_string_pretty(&serde_json::json!({
                "servers": {"snagster": {"type": "stdio", "command": exe, "args": ["--mcp"]}}
            }))
            .unwrap_or_default();
            copy_row(ui, t("VS Code (.vscode/mcp.json)"), vscode);
        });
        ui.add_space(4.0);
        egui::CollapsingHeader::new(RichText::new(t("Apps that connect to a server (HTTP)")).strong()).default_open(true).show(ui, |ui| {
            ui.label(RichText::new(t("Open WebUI, AnythingLLM, LibreChat, n8n, LM Studio, your own agent: local AI models (Ollama, llama.cpp, LM Studio) work through these apps.")).color(pal().dim));
            ui.checkbox(&mut self.settings.mcp_http, t("Let AI apps on this computer connect while Snagster runs"));
            ui.horizontal(|ui| {
                ui.label(t("Port"));
                ui.add(egui::DragValue::new(&mut self.settings.mcp_port).range(1024..=65535));
                ui.label(t("Password (optional)"));
                ui.add(egui::TextEdit::singleline(&mut self.settings.mcp_token).password(true).desired_width(140.0));
            });
            match (&self.x.mcp_server, &self.x.mcp_error) {
                (Some(s), _) => {
                    ui.label(RichText::new(tf("Running: {url}", &[("url", &s.url())])).color(pal().ok));
                }
                (None, Some(e)) => {
                    ui.label(RichText::new(tf("Couldn't start: {e}", &[("e", e)])).color(pal().err));
                }
                _ => {
                    ui.label(RichText::new(t("Off")).color(pal().dim));
                }
            }
            let port = self.settings.mcp_port;
            copy_row(ui, t("Streamable HTTP address (Open WebUI, LibreChat, n8n, most new apps)"), format!("http://127.0.0.1:{port}/mcp"));
            copy_row(ui, t("Older apps (HTTP + SSE)"), format!("http://127.0.0.1:{port}/sse"));
            if !self.settings.mcp_token.is_empty() {
                copy_row(ui, t("Header the app must send"), format!("Authorization: Bearer {}", self.settings.mcp_token));
            }
            copy_row(ui, t("Or run the server without the window"), format!("\"{exe}\" mcp-http --port {port}"));
        });
        ui.add_space(4.0);
        copy_row(ui, t("From a terminal"), format!("\"{exe}\" help"));
        ui.label(RichText::new(tf("{n} tools an AI can use", &[("n", &crate::mcp::tool_names().len().to_string())])).strong());
        egui::Grid::new("mcp-tools").striped(true).spacing(vec2(12.0, 3.0)).show(ui, |ui| {
            for (n, d) in crate::mcp::tool_names() {
                ui.label(RichText::new(n).monospace());
                ui.add(egui::Label::new(RichText::new(d).color(pal().dim).size(12.0)).wrap());
                ui.end_row();
            }
        });
    }

    /// Language and updates.
    fn language_tab(&mut self, ui: &mut egui::Ui) {
        use crate::i18n::{t, tf};
        // Light or dark, in each system's words (Windows: App theme / Use system setting).
        ui.label(RichText::new(t(crate::theme::heading())).strong());
        ui.horizontal_wrapped(|ui| {
            for c in crate::theme::ThemeChoice::ALL {
                ui.radio_value(&mut self.settings.appearance, c, t(c.label()));
            }
        });
        if let Some(note) = crate::theme::contrast_note() {
            ui.label(RichText::new(t(note)).color(pal().dim));
        }
        ui.add_space(10.0);
        ui.label(RichText::new(t("Language")).strong());
        let cur = crate::i18n::current();
        ui.horizontal(|ui| {
            let shown = if self.settings.language.is_empty() {
                tf("Automatic: the computer's language ({name})", &[("name", &crate::i18n::name(crate::i18n::system()))])
            } else {
                format!("{}  ·  {}", crate::i18n::name(cur), crate::i18n::native_name(cur))
            };
            ui.label(shown);
            if ui.button(t("Change…")).clicked() {
                self.x.lang_open = true;
            }
        });
        ui.label(RichText::new(t("Snagster speaks 86 languages. Language names are shown in the language you pick.")).color(pal().dim));
        ui.add_space(10.0);
        ui.label(RichText::new(t("Updates")).strong());
        ui.label(tf("This is Snagster {v}.", &[("v", env!("CARGO_PKG_VERSION"))]));
        if let Some(who) = self.x.update.managed() {
            ui.label(RichText::new(crate::app::updates_ui::managed_note(who)).color(pal().dim));
        } else {
            ui.checkbox(&mut self.settings.update_check, t("Look for a new version on GitHub when Snagster starts (once a day)"));
            ui.horizontal(|ui| {
                let checking = self.x.update.checking();
                if ui.add_enabled(!checking, egui::Button::new(if checking { t("Checking…") } else { t("Check now") })).clicked() {
                    self.x.update.check(ui.ctx(), true);
                }
                if let Some(r) = self.x.update.available().cloned()
                    && ui.button(tf("Install {v}", &[("v", &r.version)])).clicked()
                {
                    self.x.update.dialog = true;
                }
            });
        }
        if !self.settings.update_skip.is_empty() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(tf("Skipping version {v}.", &[("v", &self.settings.update_skip)])).color(pal().dim));
                if ui.small_button(t("Don't skip")).clicked() {
                    self.settings.update_skip.clear();
                }
            });
        }
        ui.add_space(10.0);
        ui.label(RichText::new(t("Problems")).strong());
        if let Some(p) = crate::crash_log_path() {
            ui.label(RichText::new(tf("If Snagster ever stops unexpectedly, what happened is written to {file}.", &[("file", &p.display().to_string())])).color(pal().dim));
        }
    }
}

/// The keys that move around the window, for the Keyboard shortcuts window: each system's own
/// way into the ribbon (`app::keys_ui`).
fn keyboard_rows() -> Vec<(String, String)> {
    use crate::app::keys_ui::{TipStyle, tip_style};
    use crate::i18n::t;
    let mut rows = vec![
        ("F6, Shift+F6", t("Move between the ribbon, the tools, the picture, the options and Recent")),
        ("Tab, Shift+Tab", t("The next or the previous control; the arrow keys move within a part")),
        ("Enter, Space", t("Use the control that has the keyboard")),
        ("Esc", t("Back to the picture")),
        ("←  →  ↑  ↓", t("On the picture: move the picked mark (with Shift, ten pixels)")),
    ];
    match tip_style(crate::platform::system()) {
        TipStyle::Office => rows.push(("Alt, F10", t("Show the ribbon's key letters: type one to open a tab or use a tool"))),
        TipStyle::Mnemonics => {
            rows.push(("Alt", t("Hold to see the ribbon tabs' letters; Alt and a letter opens that tab")));
            rows.push(("F10", t("Main menu")));
        }
        TipStyle::Off => {}
    }
    rows.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

#[cfg(test)]
mod tests {
    /// The Keyboard shortcuts window and Settings > Hotkeys show these through the translation
    /// table, so each must be a text there (they were English in every language before).
    #[test]
    fn shortcut_and_hotkey_texts_are_all_translated() {
        let keys: Vec<String> = serde_json::from_str(include_str!("../assets/i18n/strings.json")).unwrap();
        let mut texts: Vec<&str> = crate::settings::Action::ALL.iter().map(|a| a.label()).collect();
        texts.extend(crate::doc::Tool::ALL.iter().map(|t| t.label()));
        // Every English description and key note of the shortcuts window (help_windows).
        let src = include_str!("windows.rs");
        let rows = &src[src.find("let rows: Vec<(String, String)> = [").unwrap()..];
        let rows = &rows[..rows.find("]\n").or_else(|| rows.find("]\r\n")).unwrap()];
        let quoted: Vec<&str> = rows.split('"').skip(1).step_by(2).collect();
        assert!(quoted.len() >= 20, "found {} texts; has the list changed shape?", quoted.len());
        // Plain key lists ("Ctrl+C, Ctrl+V") are shown as they are; anything with a word in it
        // ("Ctrl+0 or 0", "Delete, [ and ]", "Shift while drawing") and the descriptions is translated.
        let has_word = |q: &str| q.split([' ', ',']).any(|w| w.len() >= 2 && w.chars().all(|c| c.is_lowercase()));
        texts.extend(quoted.into_iter().filter(|q| has_word(q)));
        texts.push("{tool} tool");
        let missing: Vec<&&str> = texts.iter().filter(|t| !keys.iter().any(|k| k == *t)).collect();
        assert!(missing.is_empty(), "not in assets/i18n/strings.json: {missing:?}");
    }

    /// No tooltip, hint or chosen-item text is English-only in the source: each goes through the
    /// translation table (`t`, `tf`, `tr`). A literal starting with a capital letter is a sentence
    /// someone forgot to wrap ("rtmp://..." and single letters like "X" are fine).
    #[test]
    fn no_tooltip_or_hint_is_left_in_english() {
        fn files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    files(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let mut all = Vec::new();
        files(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut all);
        let mut found = Vec::new();
        for f in all {
            let src = std::fs::read_to_string(&f).unwrap();
            // Only the window code, not the tests module at the end (a #[cfg(test)] helper higher
            // up doesn't stop the scan).
            let src = src.replace("\r\n", "\n");
            let code = src.split("\n#[cfg(test)]\nmod tests").next().unwrap();
            let calls = ["on_hover_text(\"", "hint_text(\"", "selected_text(\"", "button(format!(\"", "label(format!(\"", "RichText::new(format!(\""];
            // A sentence: starts with a capital and has a word in it ("U+{:04X}" and "X" don't).
            let sentence = |lit: &str| lit.chars().next().is_some_and(|c| c.is_uppercase()) && lit.split(|c: char| !c.is_alphabetic()).any(|w| w.len() >= 2 && w.chars().all(|c| c.is_lowercase()));
            for call in calls {
                for (at, _) in code.match_indices(call) {
                    let rest = &code[at + call.len()..];
                    let lit = &rest[..rest.find('"').unwrap_or(0)];
                    if sentence(lit) {
                        found.push(format!("{}: {call}{lit}\"", f.file_name().unwrap().to_string_lossy()));
                    }
                }
            }
        }
        assert!(found.is_empty(), "wrap these in t(...) and translate them: {found:#?}");
    }

    #[test]
    fn library_tiles_are_the_classic_size_at_normal_text() {
        let t = super::tile_layout(1.0, [15.0, 13.0, 13.0]);
        assert_eq!(t.size, super::vec2(192.0, 176.0));
        assert_eq!(t.thumb.size(), super::vec2(180.0, 110.0));
        assert_eq!(t.text_width, 176.0);
    }

    #[test]
    fn library_tile_lines_never_overlap_at_any_text_size() {
        // The overlap the quality review found: at 150 % the lines sat at fixed heights, so the
        // title ran into the date and the tags fell off the tile.
        for percent in [100u32, 125, 150, 175, 200, 225] {
            let scale = percent as f32 / 100.0;
            let heights = super::TILE_FONTS.map(|f| (f * scale * 1.3).ceil());
            let t = super::tile_layout(scale, heights);
            assert!(t.rows[0] >= t.thumb.bottom(), "{percent} %: title under the thumbnail");
            for k in 0..2 {
                assert!(t.rows[k] + heights[k] <= t.rows[k + 1], "{percent} %: line {k} runs into the next");
            }
            assert!(t.rows[2] + heights[2] <= t.size.y, "{percent} %: the tags fall off the tile");
            assert!(t.thumb.right() <= t.size.x && t.thumb.left() >= 0.0, "{percent} %");
            assert!(t.text_width > 0.0 && t.text_width < t.size.x, "{percent} %");
        }
        let small = super::tile_layout(1.0, [15.0; 3]);
        let big = super::tile_layout(2.25, [40.0; 3]);
        assert!(big.size.x > small.size.x && big.size.y > small.size.y, "bigger text, bigger tile");
    }

    #[test]
    fn a_long_title_is_cut_short_to_the_tiles_width() {
        let ctx = super::egui::Context::default();
        let mut widths = (0.0, 0.0);
        let mut out = ctx.run_ui(Default::default(), |ui| {
            let p = ui.painter();
            let font = super::egui::FontId::proportional(12.5 * 1.5);
            let long = super::one_line(p, "A very long capture title that would never fit on one tile".into(), font.clone(), super::Color32::WHITE, 176.0);
            let short = super::one_line(p, "Notepad".into(), font, super::Color32::WHITE, 176.0);
            assert_eq!(long.rows.len(), 1, "one line only");
            assert!(long.text().ends_with('…') || long.rows[0].glyphs.last().is_some_and(|g| g.chr == '…'), "cut short with …");
            assert!(!short.rows[0].glyphs.iter().any(|g| g.chr == '…'), "a short title stays whole");
            // Only what was cut goes to the tooltip; a title that itself ends in "…" wasn't cut.
            assert!(super::was_cut(&long, "A very long capture title that would never fit on one tile"));
            assert!(!super::was_cut(&short, "Notepad"));
            let dots = super::one_line(p, "Wait…".into(), super::egui::FontId::proportional(12.5), super::Color32::WHITE, 176.0);
            assert!(!super::was_cut(&dots, "Wait…"));
            widths = (long.size().x, short.size().x);
        });
        out.textures_delta.clear();
        assert!(widths.0 <= 176.0 + 0.5, "{widths:?}");
    }

    #[test]
    fn the_shortcuts_window_lists_how_to_move_around_by_keyboard() {
        let rows = super::keyboard_rows();
        assert!(rows.iter().any(|(k, _)| k.starts_with("F6")));
        let tips = rows.iter().any(|(k, _)| k.starts_with("Alt"));
        assert_eq!(tips, crate::platform::system() != crate::platform::System::Mac, "{rows:?}");
    }

    #[test]
    fn every_window_of_snagsters_own_says_it_is_snagster() {
        // KDE Plasma shows another program's icon for a window without it.
        assert_eq!(super::snagster_viewport().app_id.as_deref(), Some(crate::APP_ID));
    }

    #[test]
    fn no_window_is_made_without_snagsters_app_id() {
        // Every window but the main one (src/main.rs sets its own app id) comes from
        // snagster_viewport(); a plain builder anywhere else is how the KDE icon bug started.
        let plain = ["ViewportBuilder", "::default()"].concat();
        let mut found = Vec::new();
        let mut dirs = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = entry.path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&p).unwrap();
                    let n = text.matches(&plain).count();
                    let allowed = match p.file_name().and_then(|f| f.to_str()) {
                        Some("main.rs") | Some("windows.rs") => 1,
                        _ => 0,
                    };
                    if n > allowed {
                        found.push(format!("{} ({n})", p.display()));
                    }
                }
            }
        }
        assert!(found.is_empty(), "use windows::snagster_viewport() for new windows: {found:?}");
    }

    use super::*;
    use std::path::Path;

    #[test]
    fn mac_settings_is_fixed_and_named_after_its_pane() {
        use crate::platform::System;
        assert_eq!(settings_frame(System::Mac, "Hotkeys"), ("Hotkeys".to_string(), false));
        if crate::i18n::current() == "en" {
            assert_eq!(settings_frame(System::Mac, "Saving & library").0, "Saving & Library");
        }
        for sys in [System::Windows, System::Gnome, System::Kde, System::OtherLinux] {
            let (title, resizable) = settings_frame(sys, "Hotkeys");
            assert!(resizable, "{sys:?}");
            assert_eq!(title, crate::i18n::t("Settings"));
        }
        // A fixed Settings window keeps its size for every pane; other fixed boxes fit what
        // they hold.
        let settings = Win { key: SETTINGS_KEY, title: "Hotkeys", size: vec2(720.0, 600.0), resizable: false };
        assert!(!fits_content(&settings));
        let dialog = Win { key: "rename", title: "Rename", size: vec2(400.0, 160.0), resizable: false };
        assert!(fits_content(&dialog));
        let big = Win { key: "library", title: "Library", size: vec2(900.0, 600.0), resizable: true };
        assert!(!fits_content(&big));
    }

    #[test]
    fn the_delete_key_takes_the_picked_capture_only_when_it_is_shown() {
        let item = |p: &str| library::Item { path: PathBuf::from(p), meta: Default::default() };
        let items = vec![item("a.png"), item("b.png"), item("c.png")];
        assert_eq!(picked_index(&items, &[0, 1, 2], Some(Path::new("b.png"))), Some(1));
        assert_eq!(picked_index(&items, &[2, 0], Some(Path::new("c.png"))), Some(2), "in any order");
        assert_eq!(picked_index(&items, &[0, 2], Some(Path::new("b.png"))), None, "hidden by the search: never deleted unseen");
        assert_eq!(picked_index(&items, &[0, 1, 2], None), None, "nothing picked");
        assert_eq!(picked_index(&items, &[0, 1, 2], Some(Path::new("gone.png"))), None);
        assert_eq!(picked_index(&items, &[7], Some(Path::new("a.png"))), None);
    }

    #[test]
    fn the_mac_settings_toolbar_holds_every_pane_once() {
        assert!(MAC_GROUPS.len() <= 7, "about seven icons, as Apple's Settings windows have");
        for tab in SettingsTab::ALL {
            let n = MAC_GROUPS.iter().filter(|g| g.2.contains(&tab)).count();
            assert_eq!(n, 1, "{:?} is in {n} groups", tab.key());
            assert!(MAC_GROUPS[mac_group(tab)].2.contains(&tab));
        }
        let en: std::collections::HashSet<String> = serde_json::from_str::<Vec<String>>(include_str!("../assets/i18n/strings.json")).unwrap().into_iter().collect();
        let mut icons = std::collections::HashSet::new();
        for (name, icon, tabs) in MAC_GROUPS {
            assert!(!tabs.is_empty());
            assert!(en.contains(name), "{name} isn't in strings.json");
            assert!(icons.insert(icon), "{name}: an icon of its own");
        }
        // The toolbar fits the fixed 720-point window.
        assert!(MAC_GROUPS.len() as f32 * (crate::look::PANE_TOOL_W + 8.0) < 700.0);
    }

    #[test]
    fn long_toolbar_names_show_whole_and_the_row_fits_the_window() {
        // Shrinks the type only when it must.
        assert_eq!(toolbar_fit(700.0, 8.0, |_| vec![76.0; 7]), (crate::look::PANE_TOOL_TEXT, 76.0 * 7.0 + 48.0));
        let (size, w) = toolbar_fit(500.0, 8.0, |s| vec![6.0 * s; 7]);
        assert!(size < crate::look::PANE_TOOL_TEXT && size > 9.0 && w <= 500.0, "{size} {w}");
        assert_eq!(toolbar_fit(10.0, 8.0, |s| vec![10.0 * s; 7]).0, 9.0, "never below 9 points");
        // The longest real names: every icon is wider than its name, and the row fits the
        // fixed 720-point window (less its margins).
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/i18n");
        let ctx = egui::Context::default();
        for code in ["de", "et", "ar", "fr-CA", "fi", "hu", "ta"] {
            let pack: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join(format!("{code}.json"))).unwrap()).unwrap();
            let names: Vec<String> = MAC_GROUPS.iter().map(|g| pack["strings"][g.0].as_str().unwrap_or(g.0).to_string()).collect();
            ctx.run_ui(Default::default(), |ui| {
                let (size, width) = toolbar_fit(704.0, 8.0, |s| names.iter().map(|n| crate::look::pane_tool_width(ui, n, s)).collect());
                assert!(width <= 704.0 || size == 9.0, "{code}: {width}");
                for n in &names {
                    let text = ui.fonts_mut(|f| f.layout_no_wrap(n.clone(), egui::FontId::proportional(size), Color32::WHITE).size().x);
                    assert!(crate::look::pane_tool_width(ui, n, size) > text, "{code}: {n} is cut off");
                }
            }).textures_delta.clear();
        }
    }

    #[test]
    fn the_mac_settings_toolbar_draws_every_group_and_keeps_the_pane() {
        let ctx = egui::Context::default();
        for start in SettingsTab::ALL {
            let mut tab = start;
            let mut height = 0.0;
            ctx.run_ui(Default::default(), |ui| {
                let r = ui.scope(|ui| mac_settings_toolbar(ui, &mut tab, &|t| t.key()));
                height = r.response.rect.height();
            }).textures_delta.clear();
            assert!(tab == start, "drawing alone never changes the pane ({})", start.key());
            let row = MAC_GROUPS[mac_group(start)].2.len() > 1;
            assert!(height >= 54.0, "{}: the icons are drawn", start.key());
            assert!(!row || height > 70.0, "{}: a group of panes gets its row", start.key());
        }
    }

    #[test]
    fn settings_reopens_on_the_pane_shown_last_after_a_restart() {
        for tab in SettingsTab::ALL {
            assert!(SettingsTab::from_key(tab.key()) == tab, "{}", tab.key());
        }
        assert!(SettingsTab::from_key("") == SettingsTab::AfterCapture, "a new settings file starts on the first pane");
        assert!(SettingsTab::from_key("gone") == SettingsTab::AfterCapture);
        // Every key is one the catalogue (and so AI apps) accept.
        let k = crate::catalog::SETTINGS.iter().find(|k| k.key == "settings_pane").expect("in the catalogue");
        let crate::catalog::Kind::Choice(names) = k.kind else { panic!("a choice") };
        assert_eq!(names.len(), SettingsTab::ALL.len());
        for tab in SettingsTab::ALL {
            assert!(names.contains(&tab.key()), "{}", tab.key());
        }
        // The window writes the pane it shows into the settings, which are saved to disk.
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(true);
        let mut s = Settings::default();
        s.settings_pane = "hotkeys".into();
        let mut a = SnagApp::blank(s, false, None);
        assert!(a.settings_tab == SettingsTab::Hotkeys, "started on the pane saved last time");
        a.settings_open = true;
        a.settings_tab = SettingsTab::Streaming;
        ctx.run_ui(Default::default(), |ui| a.settings_window(ui.ctx())).textures_delta.clear();
        assert_eq!(a.settings.settings_pane, "streaming");
        let json = serde_json::to_string(&a.settings).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert!(SettingsTab::from_key(&back.settings_pane) == SettingsTab::Streaming);
    }

    /// A box opens in the middle of Snagster's window, but never off the screen.
    #[test]
    fn boxes_open_in_the_middle_of_the_window_and_on_the_screen() {
        let main = Rect::from_min_size(pos2(100.0, 50.0), vec2(1280.0, 800.0));
        assert_eq!(centered_on(main, vec2(400.0, 200.0), None), pos2(540.0, 350.0));
        assert_eq!(centered_on(main, vec2(400.0, 200.0), Some(vec2(1920.0, 1080.0))), pos2(540.0, 350.0));
        // Snagster pushed far to the right: the box stays on the monitor.
        let right = Rect::from_min_size(pos2(1700.0, 900.0), vec2(600.0, 400.0));
        assert_eq!(centered_on(right, vec2(700.0, 600.0), Some(vec2(1920.0, 1080.0))), pos2(1220.0, 480.0));
        // A box bigger than the monitor starts at its corner.
        assert_eq!(centered_on(main, vec2(3000.0, 2000.0), Some(vec2(1920.0, 1080.0))), pos2(0.0, 0.0));
    }

    /// In check runs (no extra windows) a box shows inside the main window, and its content runs.
    #[test]
    fn a_box_shows_inside_the_window_when_windows_are_embedded() {
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(true);
        let mut open = true;
        let mut ran = false;
        let mut out = ctx.run_ui(Default::default(), |ui| {
            let r = own_window(ui.ctx(), Win { key: "t", title: "Test", size: vec2(300.0, 200.0), resizable: false }, &mut open, |ui| {
                ui.label("hello");
                ran = true;
                7
            });
            assert_eq!(r, Some(7));
        });
        out.textures_delta.clear();
        assert!(ran && open);
        let mut closed = false;
        let mut out = ctx.run_ui(Default::default(), |ui| {
            assert_eq!(own_window(ui.ctx(), Win { key: "t", title: "Test", size: vec2(300.0, 200.0), resizable: false }, &mut closed, |_| 1), None);
        });
        out.textures_delta.clear();
    }

    /// One frame of a box inside an 800x600 window, with `events` (and the pointer at `at`).
    fn box_frame(ctx: &egui::Context, key: &'static str, resizable: bool, open: &mut bool, events: Vec<egui::Event>) -> Option<Rect> {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))), events, ..Default::default() };
        let mut drawn = None;
        let mut out = ctx.run_ui(input, |ui| {
            let size = if resizable { vec2(900.0, 640.0) } else { vec2(440.0, 220.0) };
            own_window(ui.ctx(), Win { key, title: "Box", size, resizable }, open, |ui| drawn = Some(ui.min_rect()));
        });
        out.textures_delta.clear();
        drawn
    }

    fn key(k: Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers }
    }

    fn click(at: Pos2) -> Vec<egui::Event> {
        let b = |pressed| egui::Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        vec![egui::Event::PointerMoved(at), b(true), b(false)]
    }

    /// Wayland: a fixed dialog inside the window is modal. It has the keys (the picture behind
    /// isn't touched by Delete or Ctrl+Z), a click on the dimmed window doesn't close it (a typed
    /// name isn't lost), and Esc or Ctrl+W does.
    #[test]
    fn a_dialog_inside_the_window_holds_the_keys_and_closes_on_esc_not_on_a_click_behind() {
        for close in [key(Key::Escape, Default::default()), key(Key::W, egui::Modifiers::COMMAND)] {
            let ctx = egui::Context::default();
            ctx.set_embed_viewports(true);
            let mut open = true;
            assert!(!box_has_keys(&ctx), "no box yet");
            box_frame(&ctx, "dialog", false, &mut open, vec![]);
            assert!(box_has_keys(&ctx), "the dialog has the keys");
            box_frame(&ctx, "dialog", false, &mut open, click(pos2(20.0, 580.0)));
            box_frame(&ctx, "dialog", false, &mut open, vec![]);
            assert!(open && box_has_keys(&ctx), "a click behind the dialog leaves it open");
            box_frame(&ctx, "dialog", false, &mut open, vec![close]);
            assert!(!open, "Esc / Ctrl+W closes it");
            // Closed: from the frame after next the editor has its keys again.
            box_frame(&ctx, "dialog", false, &mut open, vec![]);
            box_frame(&ctx, "dialog", false, &mut open, vec![]);
            assert!(!box_has_keys(&ctx));
        }
    }

    /// Wayland: Settings and the Library are boxes beside the editor: bigger than the window
    /// they are made to fit, they have the keys when they open, and a click on the editor gives
    /// the keys back (then Esc is the editor's, not a close).
    #[test]
    fn a_big_box_fits_in_the_window_and_gives_the_keys_back_on_a_click_outside() {
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(true);
        let mut open = true;
        let mut inner = None;
        for _ in 0..3 {
            inner = box_frame(&ctx, "library", true, &mut open, vec![]);
        }
        let inner = inner.expect("drawn");
        let room = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0)).shrink(IN_WINDOW_MARGIN);
        assert!(room.contains_rect(inner), "{inner:?} inside {room:?}");
        assert!(box_has_keys(&ctx), "opened: it has the keys");
        box_frame(&ctx, "library", true, &mut open, click(pos2(4.0, 4.0)));
        box_frame(&ctx, "library", true, &mut open, vec![]);
        box_frame(&ctx, "library", true, &mut open, vec![]);
        assert!(!box_has_keys(&ctx), "a click on the editor gives the keys back");
        box_frame(&ctx, "library", true, &mut open, vec![key(Key::Escape, Default::default())]);
        assert!(open, "Esc is the editor's now");
        box_frame(&ctx, "library", true, &mut open, click(inner.center()));
        box_frame(&ctx, "library", true, &mut open, vec![key(Key::Escape, Default::default())]);
        assert!(!open, "a click inside gives it the keys back, and Esc closes it");
    }

    /// One frame of two boxes inside an 800x600 window: a big one `a` and `b` over it (a second
    /// big box, or a dialog), each drawn in the order given.
    fn two_frame(ctx: &egui::Context, a: &mut bool, b: &mut bool, b_resizable: bool, b_first: bool, events: Vec<egui::Event>) {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| {
            let size = if b_resizable { vec2(440.0, 600.0) } else { vec2(440.0, 220.0) };
            let draw_b = |b: &mut bool| {
                own_window(ui.ctx(), Win { key: "b", title: "B", size, resizable: b_resizable }, b, |ui| {
                    ui.label("b");
                });
            };
            if b_first {
                draw_b(b);
            }
            own_window(ui.ctx(), Win { key: "a", title: "A", size: vec2(720.0, 600.0), resizable: true }, a, |ui| {
                ui.label("a");
            });
            if !b_first {
                draw_b(b);
            }
        });
        out.textures_delta.clear();
    }

    /// Settings, then Language from its General pane: Esc closes Language and leaves Settings,
    /// which has the keys again (a second Esc closes it), whichever is drawn first.
    #[test]
    fn esc_in_a_second_box_closes_only_that_one_and_the_keys_go_back() {
        for b_first in [false, true] {
            let ctx = egui::Context::default();
            ctx.set_embed_viewports(true);
            let (mut a, mut b) = (true, false);
            for _ in 0..3 {
                two_frame(&ctx, &mut a, &mut b, true, b_first, vec![]);
            }
            two_frame(&ctx, &mut a, &mut b, true, b_first, click(pos2(400.0, 300.0)));
            b = true;
            for _ in 0..3 {
                two_frame(&ctx, &mut a, &mut b, true, b_first, vec![]);
            }
            two_frame(&ctx, &mut a, &mut b, true, b_first, vec![key(Key::Escape, Default::default())]);
            assert!(!b, "Esc closes the box on top");
            assert!(a, "and leaves the one under it open (drawn first: {b_first})");
            two_frame(&ctx, &mut a, &mut b, true, b_first, vec![]);
            assert!(box_has_keys(&ctx), "the first box has the keys again");
            two_frame(&ctx, &mut a, &mut b, true, b_first, vec![key(Key::Escape, Default::default())]);
            assert!(!a, "so the next Esc closes it");
        }
    }

    /// The Library, then New image over it: a click into the dialog doesn't give the Library the
    /// keys, and Esc closes the dialog alone.
    #[test]
    fn esc_in_a_dialog_over_a_big_box_leaves_the_box_open() {
        for b_first in [false, true] {
            let ctx = egui::Context::default();
            ctx.set_embed_viewports(true);
            let (mut a, mut b) = (true, false);
            for _ in 0..3 {
                two_frame(&ctx, &mut a, &mut b, false, b_first, vec![]);
            }
            two_frame(&ctx, &mut a, &mut b, false, b_first, click(pos2(4.0, 4.0)));
            b = true;
            for _ in 0..3 {
                two_frame(&ctx, &mut a, &mut b, false, b_first, vec![]);
            }
            two_frame(&ctx, &mut a, &mut b, false, b_first, click(pos2(400.0, 300.0)));
            two_frame(&ctx, &mut a, &mut b, false, b_first, vec![key(Key::Escape, Default::default())]);
            assert!(!b, "Esc closes the dialog");
            assert!(a, "and leaves the box under it open (drawn first: {b_first})");
            for _ in 0..2 {
                two_frame(&ctx, &mut a, &mut b, false, b_first, vec![]);
            }
            assert!(!box_has_keys(&ctx), "the Library had given the keys back before, so the editor has them");
        }
    }

    /// One frame of an 800x600 window running `ui` (which draws whatever boxes it likes).
    fn any_frame(ctx: &egui::Context, events: Vec<egui::Event>, ui: impl FnMut(&mut egui::Ui)) {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(input, ui);
        out.textures_delta.clear();
    }

    fn settings_box(ui: &mut egui::Ui, open: &mut bool) {
        own_window(ui.ctx(), Win { key: "settings-t", title: "Settings", size: vec2(720.0, 560.0), resizable: true }, open, |ui| {
            ui.label("settings");
        });
    }

    fn a_dialog(ui: &mut egui::Ui) {
        own_window(ui.ctx(), Win { key: "dialog", title: "New image", size: vec2(440.0, 220.0), resizable: false }, &mut true, |ui| {
            ui.label("dialog");
        });
    }

    /// A dialog closed by its own buttons (Cancel, Create, Later: its caller just stops drawing
    /// it) gives the keys back to the box it took them from, and takes them again when it opens
    /// next time.
    #[test]
    fn a_dialog_cancelled_by_its_button_gives_the_keys_back_to_the_box_under_it() {
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(true);
        let mut settings = true;
        for _ in 0..3 {
            any_frame(&ctx, vec![], |ui| settings_box(ui, &mut settings));
        }
        // The update box (or MCP's new_image) opens on its own, over Settings.
        for _ in 0..3 {
            any_frame(&ctx, vec![], |ui| {
                settings_box(ui, &mut settings);
                a_dialog(ui);
            });
        }
        assert!(box_has_keys(&ctx));
        // Cancel: the dialog isn't drawn any more.
        for _ in 0..2 {
            any_frame(&ctx, vec![], |ui| settings_box(ui, &mut settings));
        }
        assert!(box_has_keys(&ctx), "Settings has the keys again");
        any_frame(&ctx, vec![key(Key::Escape, Default::default())], |ui| settings_box(ui, &mut settings));
        assert!(!settings, "so Esc closes it");
        // Alone: Cancel gives the keys to the editor, and the dialog has them when it reopens.
        for _ in 0..2 {
            any_frame(&ctx, vec![], a_dialog);
        }
        for _ in 0..3 {
            any_frame(&ctx, vec![], |_| {});
        }
        assert!(!box_has_keys(&ctx), "the editor has the keys");
        let mut open = true;
        box_frame(&ctx, "dialog", false, &mut open, vec![]);
        assert!(box_has_keys(&ctx), "reopened, it has them again");
        box_frame(&ctx, "dialog", false, &mut open, vec![key(Key::W, egui::Modifiers::COMMAND)]);
        assert!(!open, "and Ctrl+W closes it");
    }

    /// Picking from a list in Settings (Theme, Language...) leaves Settings with the keys, where
    /// the list lies inside the box and where it hangs out of it.
    #[test]
    fn picking_from_a_list_in_a_big_box_leaves_it_the_keys() {
        for low in [false, true] {
            let ctx = egui::Context::default();
            ctx.set_embed_viewports(true);
            let mut open = true;
            let mut pick = 0;
            let mut button = Rect::NOTHING;
            // A low box at the bottom of the room: its list hangs out under it.
            let size = if low { vec2(300.0, 120.0) } else { vec2(500.0, 500.0) };
            let mut frame = |events: Vec<egui::Event>, pick: &mut i32, button: &mut Rect| {
                any_frame(&ctx, events, |ui| {
                    own_window(ui.ctx(), Win { key: "settings-l", title: "Settings", size, resizable: true }, &mut open, |ui| {
                        let r = egui::ComboBox::from_id_salt("theme").selected_text(format!("{pick}")).show_ui(ui, |ui| {
                            for n in 0..4 {
                                ui.selectable_value(pick, n, format!("choice {n}"));
                            }
                        });
                        *button = r.response.rect;
                    });
                });
            };
            for _ in 0..3 {
                frame(vec![], &mut pick, &mut button);
            }
            frame(click(button.center()), &mut pick, &mut button);
            for _ in 0..2 {
                frame(vec![], &mut pick, &mut button);
            }
            // The list opens under the button, one row per choice.
            let row = button.height().max(18.0);
            let choice = pos2(button.center().x, button.bottom() + 4.0 + row * 2.5);
            frame(click(choice), &mut pick, &mut button);
            for _ in 0..2 {
                frame(vec![], &mut pick, &mut button);
            }
            assert_ne!(pick, 0, "a choice was picked (low: {low})");
            assert!(box_has_keys(&ctx), "Settings kept the keys (low: {low})");
            frame(vec![key(Key::Escape, Default::default())], &mut pick, &mut button);
            assert!(!open, "and Esc closes it");
        }
    }

    /// The Library is open; a click on the editor's ribbon opens New image the same frame.
    /// Closing the dialog then leaves the keys with the editor, which the click gave them to,
    /// not with the Library.
    #[test]
    fn a_dialog_opened_by_a_click_on_the_editor_gives_the_keys_to_the_editor_when_it_closes() {
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(true);
        let mut library = true;
        for _ in 0..3 {
            any_frame(&ctx, vec![], |ui| settings_box(ui, &mut library));
        }
        assert!(box_has_keys(&ctx));
        // The dialog is drawn before the Library, as `dialog_window` is.
        let mut dialog = true;
        let draw = |ui: &mut egui::Ui, dialog: &mut bool, library: &mut bool| {
            own_window(ui.ctx(), Win { key: "dialog", title: "New image", size: vec2(440.0, 220.0), resizable: false }, dialog, |ui| {
                ui.label("dialog");
            });
            settings_box(ui, library);
        };
        any_frame(&ctx, click(pos2(4.0, 4.0)), |ui| draw(ui, &mut dialog, &mut library));
        any_frame(&ctx, vec![], |ui| draw(ui, &mut dialog, &mut library));
        any_frame(&ctx, vec![key(Key::Escape, Default::default())], |ui| draw(ui, &mut dialog, &mut library));
        assert!(!dialog && library, "Esc closed the dialog only");
        for _ in 0..2 {
            any_frame(&ctx, vec![], |ui| draw(ui, &mut dialog, &mut library));
        }
        assert!(!box_has_keys(&ctx), "the editor has the keys");
    }

    #[test]
    fn keys_belong_to_a_box_drawn_this_frame_or_the_one_before() {
        assert!(!holds_keys(None, 5));
        assert!(holds_keys(Some(5), 5) && holds_keys(Some(4), 5));
        assert!(!holds_keys(Some(3), 5), "a box not drawn last frame is gone");
        assert_eq!(clamped(vec2(900.0, 640.0), Rect::from_min_size(Pos2::ZERO, vec2(768.0, 568.0))), vec2(768.0, 568.0));
        assert_eq!(clamped(vec2(440.0, 220.0), Rect::from_min_size(Pos2::ZERO, vec2(768.0, 568.0))), vec2(440.0, 220.0));
        assert!(backdrop(true).a() > backdrop(false).a(), "dims more on a dark window");
    }

    /// A fixed box that fits its content moves its smallest and largest size along, so the
    /// desktop can't keep it at the size it opened with (KDE on Wayland) and shows no maximise.
    #[test]
    fn a_fitted_box_pins_its_smallest_and_largest_size_to_the_new_size() {
        use egui::ViewportCommand::{InnerSize, MaxInnerSize, MinInnerSize};
        let (open, size) = (vec2(440.0, 220.0), vec2(440.0, 132.0));
        // Shrinking: the smallest size comes down first, then the largest, then the size itself.
        assert_eq!(fit_commands(open, size), [MinInnerSize(size), MaxInnerSize(size), InnerSize(size)]);
        // Growing: the largest goes up first, so the smallest never passes it.
        assert_eq!(fit_commands(size, open), [MaxInnerSize(open), MinInnerSize(open), InnerSize(open)]);
    }

    /// A fixed box is a dialog to X11 (with no Minimise or Maximise button asked for); one that
    /// can be resized is an ordinary window with both.
    #[test]
    fn a_fixed_box_opens_as_a_dialog_window() {
        let dialog = own_window_builder(&Win { key: "dialog", title: "New image", size: vec2(440.0, 220.0), resizable: false }, pos2(10.0, 20.0));
        assert_eq!(dialog.window_type, Some(egui::X11WindowType::Dialog));
        assert_eq!((dialog.minimize_button, dialog.maximize_button, dialog.resizable), (Some(false), Some(false), Some(false)));
        assert_eq!(dialog.app_id.as_deref(), Some(crate::APP_ID), "grouped with Snagster on the task bar");
        let library = own_window_builder(&Win { key: "library", title: "Library", size: vec2(900.0, 640.0), resizable: true }, pos2(0.0, 0.0));
        assert_eq!(library.window_type, None);
        assert_eq!((library.minimize_button, library.maximize_button), (Some(true), Some(true)));
    }

    /// The New image dialog's content is only as tall as its rows: the buttons don't take the
    /// space left in the window (which kept the dialog from shrinking to fit).
    #[test]
    fn dialog_content_is_only_as_tall_as_its_rows() {
        let ctx = egui::Context::default();
        let mut used = 0.0;
        let mut out = ctx.run_ui(Default::default(), |ui| {
            let r = ui.scope(|ui| {
                ui.label("Width");
                dialog_row(ui, "Create", true);
            });
            used = r.response.rect.height();
        });
        out.textures_delta.clear();
        assert!(used < 80.0, "the dialog's rows took {used} points");
    }

    #[test]
    fn new_image_dialog_starts_white_720p() {
        match Dialog::new_image() {
            Dialog::NewImage { w, h, color } => assert_eq!((w, h, color), (1280, 720, [255, 255, 255, 255])),
            _ => panic!("wrong dialog"),
        }
    }

    #[test]
    fn export_dialog_picks_everything_and_uses_the_settings() {
        let mut s = Settings::default();
        s.save_folder = "/tmp/pictures".into();
        match Dialog::export(3, &s) {
            Dialog::Export { pick, folder, format } => {
                assert_eq!(pick, vec![true, true, true]);
                assert_eq!(folder, "/tmp/pictures");
                assert_eq!(format, s.format);
            }
            _ => panic!("wrong dialog"),
        }
        match Dialog::export(0, &s) {
            Dialog::Export { pick, .. } => assert!(pick.is_empty()),
            _ => panic!("wrong dialog"),
        }
    }

    #[test]
    fn linux_autostart_entry_runs_the_program() {
        let e = desktop_entry(Path::new("/opt/Snag ster/snagster"));
        assert!(e.starts_with("[Desktop Entry]\n"));
        assert!(e.contains("\nType=Application\n"));
        assert!(e.contains("\nName=Snagster\n"));
        // Quoted, so a folder with a space still works.
        assert!(e.contains("\nExec=\"/opt/Snag ster/snagster\"\n"));
        assert!(e.ends_with('\n'));
    }

    #[test]
    fn mac_launch_agent_runs_the_program_at_login() {
        let p = launch_agent(Path::new("/Applications/Snagster.app/Contents/MacOS/snagster"));
        assert!(p.starts_with("<?xml version=\"1.0\""));
        assert!(p.contains("<key>Label</key><string>app.snagster</string>"));
        assert!(p.contains("<array><string>/Applications/Snagster.app/Contents/MacOS/snagster</string></array>"));
        assert!(p.contains("<key>RunAtLoad</key><true/>"));
        assert!(p.trim_end().ends_with("</plist>"));
    }

    #[cfg(not(windows))]
    #[test]
    fn autostart_file_is_in_the_home_folder() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let f = autostart_file().unwrap();
        assert!(f.starts_with(home));
        #[cfg(target_os = "macos")]
        assert!(f.ends_with("Library/LaunchAgents/app.snagster.plist"));
        #[cfg(not(target_os = "macos"))]
        assert!(f.ends_with(".config/autostart/snagster.desktop"));
    }
}
