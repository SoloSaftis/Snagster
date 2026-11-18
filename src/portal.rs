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
        let request = ashpd::desktop::screenshot::Screenshot::request().interactive(interactive).modal(false).send().await.map_err(|e| e.to_string())?;
        let answer = request.response().map_err(|e| e.to_string())?;
        Ok::<_, String>(answer.uri().to_string())
    })
    .map_err(|e| format!("the desktop's screenshot portal said no ({e})"))?;
    let path = file_uri_path(&uri).ok_or_else(|| format!("the screenshot portal gave an address Snagster can't open ({uri})"))?;
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

/// What to say when the screen share didn't start: plainly when the person pressed Cancel in
/// the desktop's dialog, else the portal's own words.
fn share_refusal(raw: &str) -> String {
    if raw.trim() == "Cancelled" || raw.contains("Response(Cancelled)") {
        "Screen sharing was cancelled in the desktop's dialog: nothing was shared.".into()
    } else {
        format!("the desktop's screen-sharing portal said no ({raw})")
    }
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

/// Where the screen-share restore token is kept: next to the settings.
fn token_path() -> Option<std::path::PathBuf> {
    Some(crate::settings::settings_path()?.parent()?.join("screencast-token"))
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
pub fn still(cursor: bool, limit: std::time::Duration) -> Result<(RgbaImage, (i32, i32)), String> {
    let cast = Cast::start(cursor)?;
    let end = std::time::Instant::now() + limit;
    let mut rgba = Vec::new();
    loop {
        if let Some((w, h, _)) = cast.newer_than(0, &mut rgba)? {
            let img = RgbaImage::from_raw(w, h, std::mem::take(&mut rgba)).ok_or("the screen share sent a picture of the wrong size")?;
            return Ok((img, cast.origin));
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
    newest: Arc<Mutex<Newest>>,
    quit: Option<pipewire::channel::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
    session: Option<ashpd::desktop::Session<ashpd::desktop::screencast::Screencast>>,
    /// Where the shared screen is on the desktop, and its size there (0: not told).
    pub origin: (i32, i32),
    pub logical: (u32, u32),
}

impl Cast {
    /// Ask the desktop for a screen share, then start receiving. The desktop shows its dialog
    /// the first time; after that the screen chosen then is shared again without asking (a
    /// restore token, kept next to the settings), until the person revokes it in the desktop's
    /// settings or picks again after the token stops working.
    pub fn start(cursor: bool) -> Result<Cast, String> {
        use ashpd::desktop::PersistMode;
        use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
        let token_file = token_path();
        let saved = token_file.as_deref().and_then(read_token);
        let (session, stream, fd) = pollster::block_on(async {
            let proxy = Screencast::new().await.map_err(|e| e.to_string())?;
            let session = proxy.create_session(Default::default()).await.map_err(|e| e.to_string())?;
            let modes = proxy.available_cursor_modes().await.unwrap_or(CursorMode::empty());
            let mode = cursor_mode(cursor, modes);
            proxy
                .select_sources(
                    &session,
                    SelectSourcesOptions::default()
                        .set_sources(SourceType::MONITOR)
                        .set_multiple(false)
                        .set_cursor_mode(mode)
                        .set_persist_mode(PersistMode::ExplicitlyRevoked)
                        .set_restore_token(saved.as_deref()),
                )
                .await
                .map_err(|e| e.to_string())?;
            let streams = proxy.start(&session, None, Default::default()).await.map_err(|e| e.to_string())?.response().map_err(|e| e.to_string())?;
            // A token works once: keep the new one for the next share.
            if let (Some(path), Some(t)) = (token_file.as_deref(), streams.restore_token()) {
                write_token(path, t);
            }
            let stream = streams.streams().first().cloned().ok_or("no screen was chosen")?;
            let fd = proxy.open_pipe_wire_remote(&session, Default::default()).await.map_err(|e| e.to_string())?;
            Ok::<_, String>((session, stream, fd))
        })
        .map_err(|e| share_refusal(&e))?;
        let origin = stream.position().unwrap_or((0, 0));
        let logical = stream.size().map(|(w, h)| (w.max(0) as u32, h.max(0) as u32)).unwrap_or((0, 0));
        let node = stream.pipe_wire_node_id();
        let newest = Arc::new(Mutex::new(Newest::default()));
        let (quit_tx, quit_rx) = pipewire::channel::channel::<()>();
        let shared = newest.clone();
        let thread = std::thread::Builder::new()
            .name("snagster-screencast".into())
            .spawn(move || {
                if let Err(e) = receive(fd, node, shared.clone(), quit_rx) {
                    shared.lock().unwrap_or_else(|p| p.into_inner()).ended = Some(e);
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Cast { newest, quit: Some(quit_tx), thread: Some(thread), session: Some(session), origin, logical })
    }

    /// The newest picture when it is newer than `seen`: copies it into `out` and returns its
    /// width, height and number. Err when the share has ended.
    pub fn newer_than(&self, seen: u64, out: &mut Vec<u8>) -> Result<Option<(u32, u32, u64)>, String> {
        let n = self.newest.lock().unwrap_or_else(|p| p.into_inner());
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

/// The PipeWire side of a `Cast`: connect through the portal's descriptor, ask for raw 4-byte
/// pixels, and keep the newest picture until `quit` says stop.
fn receive(fd: std::os::fd::OwnedFd, node: u32, newest: Arc<Mutex<Newest>>, quit: pipewire::channel::Receiver<()>) -> Result<(), String> {
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
    let stream = StreamRc::new(
        core,
        "Snagster",
        pipewire::properties::properties! {
            *pipewire::keys::MEDIA_TYPE => "Video",
            *pipewire::keys::MEDIA_CATEGORY => "Capture",
            *pipewire::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(|e| e.to_string())?;

    let ended = newest.clone();
    let _listener = stream
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
        pod::property!(FormatProperties::VideoFormat, Choice, Enum, Id, VideoFormat::BGRx, VideoFormat::BGRx, VideoFormat::RGBx, VideoFormat::BGRA, VideoFormat::RGBA),
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
    let bytes = PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &pod::Value::Object(wanted)).map_err(|e| format!("{e:?}"))?.0.into_inner();
    let mut params = [Pod::from_bytes(&bytes).ok_or("the screen-share format couldn't be written")?];
    stream.connect(Direction::Input, Some(node), StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS, &mut params).map_err(|e| e.to_string())?;

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
        let other = share_refusal("Portal request failed: org.freedesktop.portal.Error.NotAllowed");
        assert!(other.contains("NotAllowed") && other.starts_with("the desktop's screen-sharing portal said no"));
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
