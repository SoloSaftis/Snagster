//! Desktop portals (Linux): how Snagster sees the screen where it can't look at it directly.
//!
//! Inside a sandbox (Flatpak) and on Wayland desktops a program isn't allowed to read the screen
//! itself; it asks the desktop through the XDG desktop portal (D-Bus), and the desktop asks the
//! person. Two portals are used:
//!
//! - **Screenshot**: one picture of the whole desktop, saved by the desktop as a file we read and
//!   delete (`screenshot`). Used for still captures when the screens can't be listed directly.
//! - **ScreenCast**: a live screen share. The person picks a screen in the desktop's dialog, and
//!   pictures arrive through PipeWire (`Cast`). Used for recording and live streaming on Wayland.
//!
//! The pixel handling (`copy_rgba`, `crop_in_buffer`, `file_uri_path`) is plain code with tests;
//! the D-Bus and PipeWire parts need a desktop and are checked by hand (`docs/CAPTURE.md`).

use image::RgbaImage;

/// Tell the desktop's portal which app this is, before any other portal call. A Flatpak is
/// known by its sandbox; a program run directly (AppImage, tar.gz, a build) has to say it, or
/// newer portals refuse requests such as GlobalShortcuts with "An app id is required". Older
/// portals without the Registry just answer an error, which is fine. At most a second.
pub fn register_app() {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new().name("snagster-portal-app".into()).spawn(move || {
        let r = pollster::block_on(async {
            let id = ashpd::AppID::try_from(crate::APP_ID).map_err(|e| e.to_string())?;
            ashpd::register_host_app(id).await.map_err(|e| e.to_string())
        });
        let _ = tx.send(r);
    });
    let _ = rx.recv_timeout(std::time::Duration::from_secs(1));
}
use std::sync::{Arc, Mutex};

/// The pixel layouts a screen share may send (4 bytes a pixel).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Px {
    Rgbx,
    Rgba,
    Bgrx,
    Bgra,
}

/// Copy a `w` × `h` picture whose rows are `stride` bytes apart into packed, opaque RGBA in
/// `out`. False when `src` is too short for that size and stride.
pub fn copy_rgba(src: &[u8], w: u32, h: u32, stride: usize, px: Px, out: &mut Vec<u8>) -> bool {
    let row = w as usize * 4;
    if w == 0 || h == 0 || stride < row || src.len() < stride * (h as usize - 1) + row {
        return false;
    }
    out.clear();
    out.reserve(row * h as usize);
    for y in 0..h as usize {
        let line = &src[y * stride..y * stride + row];
        match px {
            Px::Rgbx | Px::Rgba => {
                for p in line.chunks_exact(4) {
                    out.extend_from_slice(&[p[0], p[1], p[2], 255]);
                }
            }
            Px::Bgrx | Px::Bgra => {
                for p in line.chunks_exact(4) {
                    out.extend_from_slice(&[p[2], p[1], p[0], 255]);
                }
            }
        }
    }
    true
}

/// Where the desktop rectangle `rect` (x, y, w, h) lies in a shared screen's pictures: the share
/// shows the screen at `origin` that is `logical` wide and high on the desktop, in pictures of
/// `buffer` pixels (more than `logical` on a scaled screen). The answer is clamped to the
/// picture; None when nothing of `rect` is on that screen.
pub fn crop_in_buffer(rect: (i32, i32, u32, u32), origin: (i32, i32), logical: (u32, u32), buffer: (u32, u32)) -> Option<(u32, u32, u32, u32)> {
    let (bw, bh) = buffer;
    if bw == 0 || bh == 0 {
        return None;
    }
    let (lw, lh) = if logical.0 == 0 || logical.1 == 0 { buffer } else { logical };
    let (sx, sy) = (bw as f64 / lw as f64, bh as f64 / lh as f64);
    let left = (((rect.0 - origin.0) as f64 * sx).round() as i64).clamp(0, bw as i64);
    let top = (((rect.1 - origin.1) as f64 * sy).round() as i64).clamp(0, bh as i64);
    let right = (((rect.0 - origin.0) as f64 + rect.2 as f64) * sx).round().clamp(0.0, bw as f64) as i64;
    let bottom = (((rect.1 - origin.1) as f64 + rect.3 as f64) * sy).round().clamp(0.0, bh as f64) as i64;
    if right - left < 2 || bottom - top < 2 {
        return None;
    }
    Some((left as u32, top as u32, (right - left) as u32, (bottom - top) as u32))
}

/// The local path in a `file://` address (with %XX escapes decoded); None for anything else.
pub fn file_uri_path(uri: &str) -> Option<std::path::PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // "file://host/path": only this computer's files.
    let path = if rest.starts_with('/') { rest } else { rest.strip_prefix("localhost")? };
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 3 <= bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    use std::os::unix::ffi::OsStringExt;
    Some(std::ffi::OsString::from_vec(out).into())
}

/// One picture through the Screenshot portal. Not `interactive`: the whole desktop at once (the
/// desktop may ask the person the first time). `interactive`: the desktop's own screenshot tool,
/// where the person picks an area, a window or a screen; no permission dialog is needed then.
/// The file the desktop wrote is deleted after reading.
pub fn screenshot(interactive: bool) -> Result<RgbaImage, String> {
    let uri = pollster::block_on(async {
        let request = ashpd::desktop::screenshot::Screenshot::request()
            .interactive(interactive)
            .modal(false)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let answer = request.response().map_err(|e| e.to_string())?;
        Ok::<_, String>(answer.uri().to_string())
    })
    .map_err(|e| screenshot_refusal(&e))?;
    let path = file_uri_path(&uri).ok_or_else(|| crate::i18n::tf("The desktop's screenshot portal gave an address Snagster can't open: {address}", &[("address", &uri)]))?;
    let img = image::open(&path).map(|i| i.to_rgba8()).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&path);
    img
}

/// A desktop notification through the Notification portal (works in the Flatpak too), sent
/// from a thread of its own so a slow desktop never holds anything up. `id` replaces an
/// earlier notification with the same id.
pub fn notify(id: &str, title: &str, body: &str) {
    let (id, title, body) = (id.to_string(), title.to_string(), body.to_string());
    let _ = std::thread::Builder::new().name("snagster-notify".into()).spawn(move || {
        let _ = pollster::block_on(async {
            use ashpd::desktop::notification::{Notification, NotificationProxy};
            let proxy = NotificationProxy::new().await?;
            proxy.add_notification(&id, Notification::new(&title).body(body.as_str())).await
        });
    });
}

/// Whether the desktop has a screen-sharing portal at all (a quick D-Bus question, at most a
/// second). WSLg and bare window managers have none; GNOME, KDE and the others do.
pub fn screencast_available() -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new().name("snagster-portal-check".into()).spawn(move || {
        let ok = pollster::block_on(async {
            let proxy = ashpd::desktop::screencast::Screencast::new().await.ok()?;
            proxy.available_source_types().await.ok()
        })
        .is_some();
        let _ = tx.send(ok);
    });
    rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap_or(false)
}

/// What to say when the Screenshot portal said no (translated, worded like the screen share's).
/// Cancel in the desktop's screenshot dialog (GNOME's or KDE's) is said plainly, not as an error.
fn screenshot_refusal(raw: &str) -> String {
    use crate::i18n::{t, tf};
    if raw.trim() == "Cancelled" || raw.contains("Response(Cancelled)") || raw.contains("was cancelled") {
        t("The screenshot was cancelled in the desktop's dialog.").to_string()
    } else {
        tf("The desktop's screenshot portal said no: {error}", &[("error", raw)])
    }
}

/// What to say when the screen share didn't start (translated).
///
/// The portal answers 1 when the person pressed Cancel (GNOME's dialog; ashpd words it "was
/// cancelled") and 2 when "the interaction was ended in some other way" (ashpd: "didn't
/// succeed with no information"). KDE's dialog answers 2 to its Cancel button, but 2 is also
/// how a backend reports a real failure, so that one says both, honestly.
fn share_refusal(raw: &str) -> String {
    use crate::i18n::{t, tf};
    if raw.trim() == "Cancelled" || raw.contains("Response(Cancelled)") || raw.contains("was cancelled") {
        t("Screen sharing was cancelled in the desktop's dialog: nothing was shared.").to_string()
    } else if raw.contains("didn't succeed with no information") {
        t("Screen sharing didn't start: it was cancelled in the desktop's dialog, or the desktop gave no reason. Nothing was shared.").to_string()
    } else {
        tf("The desktop's screen-sharing portal said no: {error}", &[("error", raw)])
    }
}

/// Show a file in the file manager with it selected, through the desktop's OpenURI portal
/// (inside a Flatpak too; the portal asks the file manager over org.freedesktop.FileManager1).
/// Waits for the portal's answer (the file manager may take a moment to start), so call it off
/// the window's thread. An error when there is no portal or it refused, so the caller can open
/// the folder instead.
pub fn show_file(path: &std::path::Path) -> Result<(), String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    pollster::block_on(async {
        let request = ashpd::desktop::open_uri::OpenDirectoryRequest::default().send(&file).await.map_err(|e| e.to_string())?;
        request.response().map_err(|e| e.to_string())
    })
}

/// The pointer mode to ask for: in the picture when wanted and offered, else hidden when
/// offered, else none at all (some desktops offer no modes, and asking for one then fails).
fn cursor_mode(want_pointer: bool, offered: ashpd::desktop::screencast::CursorMode) -> Option<ashpd::desktop::screencast::CursorMode> {
    use ashpd::desktop::screencast::CursorMode;
    if want_pointer && offered.contains(CursorMode::EMBEDDED) {
        Some(CursorMode::EMBEDDED)
    } else if offered.contains(CursorMode::HIDDEN) {
        Some(CursorMode::HIDDEN)
    } else {
        None
    }
}

/// Where a screen-share restore token is kept: next to the settings. Recordings share one
/// screen ("screencast-token"); still pictures share every screen ("screenshot-token"), so each
/// remembers its own choice.
fn token_path(every_screen: bool) -> Option<std::path::PathBuf> {
    let name = if every_screen { "screenshot-token" } else { "screencast-token" };
    Some(crate::settings::settings_path()?.parent()?.join(name))
}

/// One shared screen's picture, where that screen is on the desktop and its size there
/// (0 × 0: the desktop didn't say).
pub struct Part {
    pub image: RgbaImage,
    pub origin: (i32, i32),
    pub logical: (u32, u32),
}

/// The pictures of several shared screens as one picture of the desktop, and where it starts.
///
/// Each screen goes where the desktop says it is. A screen may send more pixels than its
/// desktop size (scaled 2×, say): the whole picture is made at the sharpest screen's scale and
/// the others are enlarged to match, so the screens line up as they do on the desk. When the
/// desktop gives no places (or places that overlap), the screens are set side by side.
pub fn join_screens(parts: Vec<Part>) -> Option<(RgbaImage, (i32, i32))> {
    if parts.len() == 1 {
        let p = parts.into_iter().next()?;
        return Some((p.image, p.origin));
    }
    // Pixels per desktop unit of each screen (1 when its desktop size is unknown).
    let scale_of = |p: &Part| if p.logical.0 > 0 { p.image.width() as f64 / p.logical.0 as f64 } else { 1.0 };
    let scale = parts.iter().map(scale_of).fold(1.0f64, f64::max);
    // Each screen's place on the desktop, in desktop units.
    let mut rects: Vec<(i32, i32, i32, i32)> = parts
        .iter()
        .map(|p| {
            let (w, h) = if p.logical.0 > 0 && p.logical.1 > 0 { p.logical } else { p.image.dimensions() };
            (p.origin.0, p.origin.1, w as i32, h as i32)
        })
        .collect();
    let overlap = |a: &(i32, i32, i32, i32), b: &(i32, i32, i32, i32)| a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3;
    let overlapping = (0..rects.len()).any(|i| (i + 1..rects.len()).any(|j| overlap(&rects[i], &rects[j])));
    if overlapping {
        let mut x = 0;
        for r in &mut rects {
            *r = (x, 0, r.2, r.3);
            x += r.2;
        }
    }
    let min_x = rects.iter().map(|r| r.0).min()?;
    let min_y = rects.iter().map(|r| r.1).min()?;
    let max_x = rects.iter().map(|r| r.0 + r.2).max()?;
    let max_y = rects.iter().map(|r| r.1 + r.3).max()?;
    let px = |v: i32| (v as f64 * scale).round() as i64;
    let (cw, ch) = (px(max_x - min_x).max(1) as u32, px(max_y - min_y).max(1) as u32);
    let mut canvas = RgbaImage::from_pixel(cw, ch, image::Rgba([0, 0, 0, 255]));
    for (p, r) in parts.iter().zip(&rects) {
        let (w, h) = (px(r.2).max(1) as u32, px(r.3).max(1) as u32);
        let at = (px(r.0 - min_x), px(r.1 - min_y));
        if p.image.dimensions() == (w, h) {
            image::imageops::replace(&mut canvas, &p.image, at.0, at.1);
        } else {
            let big = image::imageops::resize(&p.image, w, h, image::imageops::FilterType::Triangle);
            image::imageops::replace(&mut canvas, &big, at.0, at.1);
        }
    }
    Some((canvas, (px(min_x) as i32, px(min_y) as i32)))
}

/// Whether a screen share is remembered (its restore token is saved): opening one then needs
/// no dialog, unless the person took the permission back in the desktop.
pub fn screencast_remembered() -> bool {
    token_path(false).and_then(|p| read_token(&p)).is_some()
}

/// The saved restore token, if it looks like one (the desktop makes them: a UUID or similar).
fn read_token(path: &std::path::Path) -> Option<String> {
    let t = std::fs::read_to_string(path).ok()?;
    let t = t.trim();
    (!t.is_empty() && t.len() <= 200 && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')).then(|| t.to_string())
}

/// Keep the token, readable by this user only (it lets the screen be shared without asking).
fn write_token(path: &std::path::Path, token: &str) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path) {
        let _ = f.write_all(token.as_bytes());
    }
}

/// One still picture of the screen through a screen share that ends at once. On Wayland this
/// asks only the first time (the share is remembered with its restore token), unlike the
/// Screenshot portal, which GNOME confirms for every picture of a program run outside a sandbox.
/// The first time the person picks the screen in the desktop's dialog: up to `limit` to do it.
/// With `every_screen` the desktop is asked for all screens (the person can pick several) and
/// their pictures are joined into one (`join_screens`); without, the recordings' single screen.
pub fn still(cursor: bool, limit: std::time::Duration, every_screen: bool) -> Result<(RgbaImage, (i32, i32)), String> {
    let cast = Cast::open(cursor, every_screen)?;
    let end = std::time::Instant::now() + limit;
    loop {
        if let Some(parts) = cast.first_pictures()? {
            return join_screens(parts).ok_or_else(|| "no screen was chosen".to_string());
        }
        if std::time::Instant::now() >= end {
            return Err("the screen share sent no picture".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
}

/// The newest picture of a screen share, shared between the PipeWire thread and the grabber.
#[derive(Default)]
struct Newest {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    /// Counts up with every picture (0: none yet).
    number: u64,
    /// The share stopped (the person ended it, or PipeWire failed).
    ended: Option<String>,
}

/// A live screen share through the ScreenCast portal: the person picks a screen in the
/// desktop's dialog, then its pictures arrive on a PipeWire thread of its own. Dropping it ends
/// the share.
pub struct Cast {
    /// The newest picture of each shared screen (the first is the one recordings use).
    screens: Vec<Shared>,
    quit: Option<pipewire::channel::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
    session: Option<ashpd::desktop::Session<ashpd::desktop::screencast::Screencast>>,
    /// Where the (first) shared screen is on the desktop, and its size there (0: not told).
    pub origin: (i32, i32),
    pub logical: (u32, u32),
}

/// One shared screen of a `Cast`: its place on the desktop and its newest picture.
struct Shared {
    origin: (i32, i32),
    logical: (u32, u32),
    newest: Arc<Mutex<Newest>>,
}

impl Cast {
    /// Ask the desktop for a screen share, then start receiving. The desktop shows its dialog
    /// the first time; after that the screen chosen then is shared again without asking (a
    /// restore token, kept next to the settings), until the person revokes it in the desktop's
    /// settings or picks again after the token stops working.
    pub fn start(cursor: bool) -> Result<Cast, String> {
        Cast::open(cursor, false)
    }

    /// `start`, or with `every_screen` a share of all screens the person picks (remembered
    /// apart from the recordings' screen).
    fn open(cursor: bool, every_screen: bool) -> Result<Cast, String> {
        use ashpd::desktop::PersistMode;
        use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
        let token_file = token_path(every_screen);
        let saved = token_file.as_deref().and_then(read_token);
        let (session, picked, fd) = pollster::block_on(async {
            let proxy = Screencast::new().await.map_err(|e| e.to_string())?;
            let session = proxy.create_session(Default::default()).await.map_err(|e| e.to_string())?;
            let modes = proxy.available_cursor_modes().await.unwrap_or(CursorMode::empty());
            let mode = cursor_mode(cursor, modes);
            proxy
                .select_sources(
                    &session,
                    SelectSourcesOptions::default()
                        .set_sources(SourceType::MONITOR)
                        .set_multiple(every_screen)
                        .set_cursor_mode(mode)
                        .set_persist_mode(PersistMode::ExplicitlyRevoked)
                        .set_restore_token(saved.as_deref()),
                )
                .await
                .map_err(|e| e.to_string())?;
            let streams = proxy
                .start(&session, None, Default::default())
                .await
                .map_err(|e| e.to_string())?
                .response()
                .map_err(|e| e.to_string())?;
            // A token works once: keep the new one for the next share.
            if let (Some(path), Some(t)) = (token_file.as_deref(), streams.restore_token()) {
                write_token(path, t);
            }
            let streams = streams.streams().to_vec();
            if streams.is_empty() {
                return Err("no screen was chosen".to_string());
            }
            let fd = proxy.open_pipe_wire_remote(&session, Default::default()).await.map_err(|e| e.to_string())?;
            Ok::<_, String>((session, streams, fd))
        })
        .map_err(|e| share_refusal(&e))?;
        let screens: Vec<Shared> = picked
            .iter()
            .map(|s| Shared {
                origin: s.position().unwrap_or((0, 0)),
                logical: s.size().map(|(w, h)| (w.max(0) as u32, h.max(0) as u32)).unwrap_or((0, 0)),
                newest: Arc::new(Mutex::new(Newest::default())),
            })
            .collect();
        let nodes: Vec<(u32, Arc<Mutex<Newest>>)> = picked.iter().zip(&screens).map(|(s, sh)| (s.pipe_wire_node_id(), sh.newest.clone())).collect();
        let (quit_tx, quit_rx) = pipewire::channel::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("snagster-screencast".into())
            .spawn(move || {
                if let Err(e) = receive(fd, &nodes, quit_rx) {
                    for (_, n) in &nodes {
                        n.lock().unwrap_or_else(|p| p.into_inner()).ended = Some(e.clone());
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let (origin, logical) = (screens[0].origin, screens[0].logical);
        Ok(Cast {
            screens,
            quit: Some(quit_tx),
            thread: Some(thread),
            session: Some(session),
            origin,
            logical,
        })
    }

    /// The first picture of every shared screen, once each has sent one. Err when the share
    /// has ended.
    fn first_pictures(&self) -> Result<Option<Vec<Part>>, String> {
        let mut parts = Vec::with_capacity(self.screens.len());
        for s in &self.screens {
            let n = s.newest.lock().unwrap_or_else(|p| p.into_inner());
            if n.number == 0 {
                return match &n.ended {
                    Some(e) => Err(e.clone()),
                    None => Ok(None),
                };
            }
            let image = RgbaImage::from_raw(n.width, n.height, n.rgba.clone()).ok_or("the screen share sent a picture of the wrong size")?;
            parts.push(Part {
                image,
                origin: s.origin,
                logical: s.logical,
            });
        }
        Ok(Some(parts))
    }

    /// The newest picture when it is newer than `seen`: copies it into `out` and returns its
    /// width, height and number. Err when the share has ended.
    pub fn newer_than(&self, seen: u64, out: &mut Vec<u8>) -> Result<Option<(u32, u32, u64)>, String> {
        let n = self.screens[0].newest.lock().unwrap_or_else(|p| p.into_inner());
        if n.number > seen {
            out.clear();
            out.extend_from_slice(&n.rgba);
            return Ok(Some((n.width, n.height, n.number)));
        }
        match &n.ended {
            Some(e) => Err(e.clone()),
            None => Ok(None),
        }
    }
}

impl Drop for Cast {
    fn drop(&mut self) {
        if let Some(q) = self.quit.take() {
            let _ = q.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if let Some(s) = self.session.take() {
            let _ = pollster::block_on(s.close());
        }
    }
}

/// The PipeWire side of a `Cast`: connect through the portal's descriptor, then for each shared
/// screen (`nodes`) ask for raw 4-byte pixels and keep its newest picture until `quit` says stop.
fn receive(fd: std::os::fd::OwnedFd, nodes: &[(u32, Arc<Mutex<Newest>>)], quit: pipewire::channel::Receiver<()>) -> Result<(), String> {
    use pipewire::spa::param::format::{FormatProperties, MediaSubtype, MediaType};
    use pipewire::spa::param::video::{VideoFormat, VideoInfoRaw};
    use pipewire::spa::param::{ParamType, format_utils};
    use pipewire::spa::pod::{self, Pod, serialize::PodSerializer};
    use pipewire::spa::utils::{Direction, Fraction, Rectangle, SpaTypes};
    use pipewire::stream::{StreamFlags, StreamRc};

    pipewire::init();
    let main_loop = pipewire::main_loop::MainLoopRc::new(None).map_err(|e| e.to_string())?;
    let context = pipewire::context::ContextRc::new(&main_loop, None).map_err(|e| e.to_string())?;
    let core = context.connect_fd_rc(fd, None).map_err(|e| e.to_string())?;
    // Each screen's stream and listener, kept until the loop ends.
    let mut kept = Vec::with_capacity(nodes.len());
    for (node, newest) in nodes {
        let (node, newest) = (*node, newest.clone());
        let stream = StreamRc::new(
            core.clone(),
            "Snagster",
            pipewire::properties::properties! {
                *pipewire::keys::MEDIA_TYPE => "Video",
                *pipewire::keys::MEDIA_CATEGORY => "Capture",
                *pipewire::keys::MEDIA_ROLE => "Screen",
            },
        )
        .map_err(|e| e.to_string())?;

        let ended = newest.clone();
        let listener = stream
            .add_local_listener_with_user_data(VideoInfoRaw::default())
            .state_changed(move |_, _, _, now| {
                if let pipewire::stream::StreamState::Error(e) = now {
                    ended.lock().unwrap_or_else(|p| p.into_inner()).ended = Some(e);
                }
            })
            .param_changed(|_, format, id, param| {
                let Some(param) = param else { return };
                if id != ParamType::Format.as_raw() {
                    return;
                }
                if let Ok((MediaType::Video, MediaSubtype::Raw)) = format_utils::parse_format(param) {
                    let _ = format.parse(param);
                }
            })
            .process(move |stream, format| {
                let Some(mut buffer) = stream.dequeue_buffer() else { return };
                let px = match format.format() {
                    VideoFormat::RGBx => Px::Rgbx,
                    VideoFormat::RGBA => Px::Rgba,
                    VideoFormat::BGRx => Px::Bgrx,
                    VideoFormat::BGRA => Px::Bgra,
                    _ => return,
                };
                let size = format.size();
                let datas = buffer.datas_mut();
                let Some(d) = datas.first_mut() else { return };
                let (offset, stride) = (d.chunk().offset() as usize, d.chunk().stride().max(0) as usize);
                let stride = if stride == 0 { size.width as usize * 4 } else { stride };
                let Some(bytes) = d.data() else { return };
                let Some(bytes) = bytes.get(offset..) else { return };
                let mut n = newest.lock().unwrap_or_else(|p| p.into_inner());
                let mut rgba = std::mem::take(&mut n.rgba);
                if copy_rgba(bytes, size.width, size.height, stride, px, &mut rgba) {
                    n.width = size.width;
                    n.height = size.height;
                    n.number += 1;
                }
                n.rgba = rgba;
            })
            .register()
            .map_err(|e| e.to_string())?;

        let wanted = pod::object!(
            SpaTypes::ObjectParamFormat,
            ParamType::EnumFormat,
            pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
            pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
            pod::property!(
                FormatProperties::VideoFormat,
                Choice,
                Enum,
                Id,
                VideoFormat::BGRx,
                VideoFormat::BGRx,
                VideoFormat::RGBx,
                VideoFormat::BGRA,
                VideoFormat::RGBA
            ),
            pod::property!(
                FormatProperties::VideoSize,
                Choice,
                Range,
                Rectangle,
                Rectangle { width: 1920, height: 1080 },
                Rectangle { width: 1, height: 1 },
                Rectangle { width: 8192, height: 8192 }
            ),
            pod::property!(
                FormatProperties::VideoFramerate,
                Choice,
                Range,
                Fraction,
                Fraction { num: 60, denom: 1 },
                Fraction { num: 0, denom: 1 },
                Fraction { num: 240, denom: 1 }
            ),
        );
        let bytes = PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &pod::Value::Object(wanted))
            .map_err(|e| format!("{e:?}"))?
            .0
            .into_inner();
        let mut params = [Pod::from_bytes(&bytes).ok_or("the screen-share format couldn't be written")?];
        stream
            .connect(Direction::Input, Some(node), StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS, &mut params)
            .map_err(|e| e.to_string())?;
        kept.push((stream, listener));
    }

    let ml = main_loop.clone();
    let _quit = quit.attach(main_loop.loop_(), move |()| ml.quit());
    main_loop.run();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancelled_share_is_said_plainly() {
        assert!(share_refusal("Cancelled").starts_with("Screen sharing was cancelled"));
        // ashpd's own wording, and KDE's answer to its dialog's Cancel button.
        assert!(share_refusal("Portal request was cancelled").starts_with("Screen sharing was cancelled"));
        // The portal's "ended some other way": KDE's Cancel, or a failure. Both are said.
        let other_way = share_refusal("Portal request didn't succeed with no information");
        assert!(other_way.starts_with("Screen sharing didn't start") && other_way.contains("cancelled") && other_way.contains("no reason"), "{other_way}");
        let other = share_refusal("Portal request failed: org.freedesktop.portal.Error.NotAllowed");
        assert!(other.contains("NotAllowed") && other.starts_with("The desktop's screen-sharing portal said no: "), "{other}");
    }

    #[test]
    fn a_refused_screenshot_is_said_like_a_refused_share() {
        let raw = "Portal request failed: org.freedesktop.portal.Error.NotAllowed";
        let said = screenshot_refusal(raw);
        assert_eq!(said, format!("The desktop's screenshot portal said no: {raw}"));
        // Through the language packs, like the screen-sharing message beside it.
        assert!(include_str!("../assets/i18n/strings.json").contains("\"The desktop's screenshot portal said no: {error}\""));
        // Cancel in the desktop's dialog isn't an error (ashpd's wording, and the bare answer).
        for raw in ["Portal request was cancelled", "Cancelled"] {
            assert_eq!(screenshot_refusal(raw), "The screenshot was cancelled in the desktop's dialog.");
        }
    }

    #[test]
    fn only_a_pointer_mode_the_desktop_offers_is_asked_for() {
        use ashpd::desktop::screencast::CursorMode;
        let all = CursorMode::HIDDEN | CursorMode::EMBEDDED | CursorMode::METADATA;
        assert_eq!(cursor_mode(true, all), Some(CursorMode::EMBEDDED));
        assert_eq!(cursor_mode(false, all), Some(CursorMode::HIDDEN));
        assert_eq!(cursor_mode(true, CursorMode::HIDDEN), Some(CursorMode::HIDDEN));
        // A desktop that offers none (GNOME in a virtual machine): ask for none.
        assert_eq!(cursor_mode(true, CursorMode::empty()), None);
        assert_eq!(cursor_mode(false, CursorMode::empty()), None);
    }

    #[test]
    fn the_screen_share_token_is_kept_private_and_read_back() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("snagster-token-test-{}", std::process::id()));
        let path = dir.join("screencast-token");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(read_token(&path), None, "nothing saved yet");
        write_token(&path, "3f2a-9b_c1");
        assert_eq!(read_token(&path).as_deref(), Some("3f2a-9b_c1"));
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        // A new token replaces the old one.
        write_token(&path, "next");
        assert_eq!(read_token(&path).as_deref(), Some("next"));
        // Anything that doesn't look like a token is ignored (the dialog then asks again).
        std::fs::write(&path, "not a token\n").unwrap();
        assert_eq!(read_token(&path), None);
        std::fs::write(&path, "  \n").unwrap();
        assert_eq!(read_token(&path), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rows_with_padding_become_packed_opaque_rgba() {
        // 2 × 2 BGRx with 4 bytes of padding at the end of each row.
        let src = [1, 2, 3, 0, 4, 5, 6, 0, 9, 9, 9, 9, 7, 8, 9, 0, 10, 11, 12, 0, 9, 9, 9, 9];
        let mut out = Vec::new();
        assert!(copy_rgba(&src, 2, 2, 12, Px::Bgrx, &mut out));
        assert_eq!(out, vec![3, 2, 1, 255, 6, 5, 4, 255, 9, 8, 7, 255, 12, 11, 10, 255]);
        assert!(copy_rgba(&src, 2, 2, 12, Px::Rgbx, &mut out));
        assert_eq!(&out[..4], &[1, 2, 3, 255]);
    }

    #[test]
    fn a_short_buffer_or_a_stride_smaller_than_a_row_is_refused() {
        let mut out = Vec::new();
        assert!(!copy_rgba(&[0; 15], 2, 2, 8, Px::Rgba, &mut out), "needs 16 bytes");
        assert!(copy_rgba(&[0; 16], 2, 2, 8, Px::Rgba, &mut out));
        assert!(!copy_rgba(&[0; 64], 2, 2, 4, Px::Rgba, &mut out), "stride below a row");
        assert!(!copy_rgba(&[0; 64], 0, 2, 8, Px::Rgba, &mut out));
        // The last row needs only its own pixels, not the padding after it.
        assert!(copy_rgba(&[0; 12 + 8], 2, 2, 12, Px::Rgba, &mut out));
    }

    fn solid(w: u32, h: u32, v: u8) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba([v, v, v, 255]))
    }

    #[test]
    fn one_shared_screen_is_kept_as_it_is() {
        let (img, at) = join_screens(vec![Part { image: solid(4, 3, 7), origin: (1920, 0), logical: (4, 3) }]).unwrap();
        assert_eq!((img.dimensions(), at), ((4, 3), (1920, 0)));
        assert!(join_screens(Vec::new()).is_none());
    }

    #[test]
    fn several_screens_are_joined_where_they_sit_on_the_desk() {
        // A 4 × 3 screen, and a 2 × 2 one to its right, a row lower; a gap stays black.
        let (img, at) = join_screens(vec![
            Part { image: solid(4, 3, 10), origin: (0, 0), logical: (4, 3) },
            Part { image: solid(2, 2, 20), origin: (4, 1), logical: (2, 2) },
        ])
        .unwrap();
        assert_eq!((img.dimensions(), at), ((6, 3), (0, 0)));
        assert_eq!(img.get_pixel(3, 2)[0], 10);
        assert_eq!(img.get_pixel(4, 1)[0], 20);
        assert_eq!(img.get_pixel(5, 0)[0], 0, "nothing above the second screen");
        // A screen left of and above the main one: the picture starts there.
        let (img, at) = join_screens(vec![
            Part { image: solid(4, 3, 10), origin: (0, 0), logical: (4, 3) },
            Part { image: solid(2, 2, 20), origin: (-2, -1), logical: (2, 2) },
        ])
        .unwrap();
        assert_eq!((img.dimensions(), at), ((6, 4), (-2, -1)));
        assert_eq!(img.get_pixel(0, 0)[0], 20);
        assert_eq!(img.get_pixel(2, 1)[0], 10);
    }

    #[test]
    fn a_sharper_screen_sets_the_scale_of_the_whole_picture() {
        // The right screen sends 2 pixels per desktop unit: the left one is enlarged to match.
        let (img, at) = join_screens(vec![
            Part { image: solid(4, 3, 10), origin: (0, 0), logical: (4, 3) },
            Part { image: solid(8, 6, 20), origin: (4, 0), logical: (4, 3) },
        ])
        .unwrap();
        assert_eq!((img.dimensions(), at), ((16, 6), (0, 0)));
        assert_eq!(img.get_pixel(7, 5)[0], 10);
        assert_eq!(img.get_pixel(8, 0)[0], 20);
    }

    #[test]
    fn screens_without_places_are_set_side_by_side() {
        // The desktop told neither place nor size: both say (0, 0), so they would cover each other.
        let (img, at) = join_screens(vec![
            Part { image: solid(4, 3, 10), origin: (0, 0), logical: (0, 0) },
            Part { image: solid(5, 2, 20), origin: (0, 0), logical: (0, 0) },
        ])
        .unwrap();
        assert_eq!((img.dimensions(), at), ((9, 3), (0, 0)));
        assert_eq!(img.get_pixel(3, 0)[0], 10);
        assert_eq!(img.get_pixel(4, 0)[0], 20);
        assert_eq!(img.get_pixel(4, 2)[0], 0);
    }

    #[test]
    fn a_desktop_area_is_found_in_the_shared_screens_pictures() {
        // A 1920 × 1080 screen at (1920, 0), sent at its own size.
        assert_eq!(crop_in_buffer((2000, 100, 400, 300), (1920, 0), (1920, 1080), (1920, 1080)), Some((80, 100, 400, 300)));
        // The same screen scaled 2× (pictures of 3840 × 2160).
        assert_eq!(crop_in_buffer((2000, 100, 400, 300), (1920, 0), (1920, 1080), (3840, 2160)), Some((160, 200, 800, 600)));
        // Size not told: the pictures are the screen.
        assert_eq!(crop_in_buffer((10, 10, 100, 50), (0, 0), (0, 0), (640, 480)), Some((10, 10, 100, 50)));
        // Hanging off the edge is clamped; entirely elsewhere is None.
        assert_eq!(crop_in_buffer((-50, -50, 100, 100), (0, 0), (640, 480), (640, 480)), Some((0, 0, 50, 50)));
        assert_eq!(crop_in_buffer((5000, 0, 100, 100), (0, 0), (640, 480), (640, 480)), None);
        assert_eq!(crop_in_buffer((0, 0, 100, 100), (0, 0), (640, 480), (0, 0)), None);
    }

    #[test]
    fn screenshot_addresses_become_paths() {
        assert_eq!(file_uri_path("file:///home/a/Pictures/Screenshot.png"), Some("/home/a/Pictures/Screenshot.png".into()));
        assert_eq!(file_uri_path("file:///tmp/my%20shot%C3%A6.png"), Some("/tmp/my shotæ.png".into()));
        assert_eq!(file_uri_path("file://localhost/tmp/a.png"), Some("/tmp/a.png".into()));
        assert_eq!(file_uri_path("file://elsewhere/tmp/a.png"), None);
        assert_eq!(file_uri_path("https://example.com/a.png"), None);
        assert_eq!(file_uri_path("file:///tmp/bad%zz.png"), None);
        assert_eq!(file_uri_path("file:///tmp/cut%2"), Some("/tmp/cut%2".into()));
    }
}
