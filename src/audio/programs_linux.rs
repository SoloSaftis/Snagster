//! Linux: one program's sound through PipeWire, written from PipeWire's documentation. Every
//! program that plays sound has an output stream node ("Stream/Output/Audio"); a lane connects
//! one capture stream to each of the program's nodes (`target.object` = the node's serial, never
//! falling back to a microphone) and adds them together. A program is its binary
//! (`application.process.binary`), else its application name; people read the application name.
//!
//! In the Flatpak, Snagster reaches PipeWire only when the sandbox lets it
//! (`--filesystem=xdg-run/pipewire-0`); the screen-share portal's PipeWire is for pictures only.

use super::programs::{Gather, Program, Unsupported, key, same};
use super::{CHANNELS, Feed, RATE};
use pipewire as pw;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn in_flatpak() -> bool {
    std::env::var_os("FLATPAK_ID").is_some() || std::path::Path::new("/.flatpak-info").exists()
}

pub fn support() -> Result<(), Unsupported> {
    static OK: OnceLock<bool> = OnceLock::new();
    if *OK.get_or_init(|| nodes().is_ok()) {
        Ok(())
    } else if in_flatpak() {
        Err(Unsupported::Flatpak)
    } else {
        Err(Unsupported::NoPipeWire)
    }
}

/// A program's output stream node: (node id, serial, key, label).
type Node = (u32, String, String, String);

/// The program an output node belongs to, from its properties: (key, label). Not a program's
/// output (a device, an input, Snagster's own stream): None.
fn node_program<'a>(get: impl Fn(&str) -> Option<&'a str>, own_pid: u32) -> Option<(String, String)> {
    if get("media.class") != Some("Stream/Output/Audio") {
        return None;
    }
    if get("application.process.id").and_then(|p| p.trim().parse::<u32>().ok()) == Some(own_pid) {
        return None;
    }
    let name = get("application.name").map(str::trim).filter(|s| !s.is_empty());
    let binary = get("application.process.binary").map(str::trim).filter(|s| !s.is_empty());
    let k = key(binary.or(name).or(get("node.name"))?);
    if k.is_empty() {
        return None;
    }
    Some((k, name.map(String::from).unwrap_or_default()))
}

/// Every program output node, read once (fails when there is no PipeWire to reach).
fn nodes() -> Result<Vec<Node>, String> {
    pw::init();
    let main_loop = pw::main_loop::MainLoopRc::new(None).map_err(|e| e.to_string())?;
    let context = pw::context::ContextRc::new(&main_loop, None).map_err(|e| e.to_string())?;
    let core = context.connect_rc(None).map_err(|e| e.to_string())?;
    let registry = core.get_registry_rc().map_err(|e| e.to_string())?;
    let found: Rc<RefCell<Vec<Node>>> = Rc::new(RefCell::new(Vec::new()));
    let done = Rc::new(std::cell::Cell::new(false));
    let own = std::process::id();
    let f = found.clone();
    let _reg = registry
        .add_listener_local()
        .global(move |g| {
            if g.type_ != pw::types::ObjectType::Node {
                return;
            }
            let Some(props) = g.props else { return };
            if let Some((k, label)) = node_program(|x| props.get(x), own) {
                f.borrow_mut().push((g.id, props.get("object.serial").unwrap_or_default().to_string(), k, label));
            }
        })
        .register();
    let pending = core.sync(0).map_err(|e| e.to_string())?;
    let (d, ml) = (done.clone(), main_loop.clone());
    let _core = core
        .add_listener_local()
        .done(move |id, seq| {
            if id == pw::core::PW_ID_CORE && seq == pending {
                d.set(true);
                ml.quit();
            }
        })
        .register();
    // Never wait more than a second for the answer.
    let ml = main_loop.clone();
    let timer = main_loop.loop_().add_timer(move |_| ml.quit());
    let _ = timer.update_timer(Some(Duration::from_secs(1)), None);
    main_loop.run();
    if !done.get() {
        return Err("PipeWire didn't answer".into());
    }
    Ok(found.take())
}

pub fn list() -> Vec<Program> {
    nodes().unwrap_or_default().into_iter().map(|(_, _, name, label)| Program { name, label, playing: true }).collect()
}

/// One capture stream on one of the program's nodes.
type Linked = (u32, pw::stream::StreamListener<()>, pw::stream::StreamRc);

/// Open a capture stream on output node `serial`, its samples going to `gather` as copy `id`.
fn link(core: &pw::core::CoreRc, id: u32, serial: &str, gather: Rc<RefCell<Gather>>) -> Result<Linked, String> {
    use pw::spa::param::audio::{AudioFormat, AudioInfoRaw};
    use pw::spa::pod::{Object, Pod, Value, serialize::PodSerializer};
    let props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Production",
        "target.object" => serial,
        *pw::keys::NODE_DONT_RECONNECT => "true",
        "node.dont-fallback" => "true",
        "node.passive" => "true",
        *pw::keys::APP_NAME => "Snagster",
    };
    let stream = pw::stream::StreamRc::new(core.clone(), "Snagster program sound", props).map_err(|e| e.to_string())?;
    let mut buf: Vec<i16> = Vec::new();
    let listener = stream
        .add_local_listener_with_user_data(())
        .process(move |stream, _| {
            let Some(mut b) = stream.dequeue_buffer() else { return };
            let Some(d) = b.datas_mut().first_mut() else { return };
            let (offset, size) = (d.chunk().offset() as usize, d.chunk().size() as usize);
            let Some(bytes) = d.data() else { return };
            let Some(bytes) = bytes.get(offset..offset.saturating_add(size)) else { return };
            buf.clear();
            buf.extend(bytes.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])));
            let n = buf.len() - buf.len() % CHANNELS as usize;
            if let Ok(mut g) = gather.try_borrow_mut() {
                g.put(id as u64, &buf[..n]);
            }
        })
        .register()
        .map_err(|e| e.to_string())?;
    let mut info = AudioInfoRaw::new();
    info.set_format(AudioFormat::S16LE);
    info.set_rate(RATE);
    info.set_channels(CHANNELS);
    let mut pos = [0u32; 64];
    pos[0] = pw::spa::sys::SPA_AUDIO_CHANNEL_FL;
    pos[1] = pw::spa::sys::SPA_AUDIO_CHANNEL_FR;
    info.set_position(pos);
    let obj = Object { type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(), id: pw::spa::param::ParamType::EnumFormat.as_raw(), properties: info.into() };
    let bytes = PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(obj)).map_err(|e| format!("{e:?}"))?.0.into_inner();
    let mut params = [Pod::from_bytes(&bytes).ok_or("the sound format couldn't be written")?];
    stream
        .connect(pw::spa::utils::Direction::Input, None, pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS, &mut params)
        .map_err(|e| e.to_string())?;
    Ok((id, listener, stream))
}

pub fn run(name: &str, feed: &Arc<Feed>, stop: &Arc<AtomicBool>) -> Result<(), String> {
    pw::init();
    let main_loop = pw::main_loop::MainLoopRc::new(None).map_err(|e| e.to_string())?;
    let context = pw::context::ContextRc::new(&main_loop, None).map_err(|e| e.to_string())?;
    let core = context.connect_rc(None).map_err(|e| e.to_string())?;
    let registry = core.get_registry_rc().map_err(|e| e.to_string())?;
    // Nodes of this program that came (id, serial) and went (id), handled on the timer.
    let came: Rc<RefCell<Vec<(u32, String)>>> = Rc::new(RefCell::new(Vec::new()));
    let went: Rc<RefCell<Vec<u32>>> = Rc::new(RefCell::new(Vec::new()));
    let own = std::process::id();
    let want = name.to_string();
    let (c, w) = (came.clone(), went.clone());
    let _reg = registry
        .add_listener_local()
        .global(move |g| {
            if g.type_ != pw::types::ObjectType::Node {
                return;
            }
            let Some(props) = g.props else { return };
            if let Some((k, _)) = node_program(|x| props.get(x), own)
                && same(&want, &k)
            {
                c.borrow_mut().push((g.id, props.get("object.serial").map(String::from).unwrap_or_else(|| g.id.to_string())));
            }
        })
        .global_remove(move |id| w.borrow_mut().push(id))
        .register();

    let gather = Rc::new(RefCell::new(Gather::new(Instant::now())));
    let linked: Rc<RefCell<Vec<Linked>>> = Rc::new(RefCell::new(Vec::new()));
    let (feed, stop) = (feed.clone(), stop.clone());
    let ml = main_loop.clone();
    let (core2, g2, l2) = (core.clone(), gather.clone(), linked.clone());
    let timer = main_loop.loop_().add_timer(move |_| {
        if stop.load(Ordering::Relaxed) {
            ml.quit();
            return;
        }
        for id in went.borrow_mut().drain(..) {
            l2.borrow_mut().retain(|(i, ..)| *i != id);
            g2.borrow_mut().end(id as u64);
        }
        for (id, serial) in came.borrow_mut().drain(..) {
            if l2.borrow().iter().any(|(i, ..)| *i == id) {
                continue;
            }
            if let Ok(l) = link(&core2, id, &serial, g2.clone()) {
                l2.borrow_mut().push(l);
            }
        }
        feed.waiting.store(l2.borrow().is_empty(), Ordering::Relaxed);
        g2.borrow_mut().tick(Instant::now(), &feed);
    });
    let _ = timer.update_timer(Some(Duration::from_millis(10)), Some(Duration::from_millis(10)));
    main_loop.run();
    // The streams go before the loop and the context.
    linked.borrow_mut().clear();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn program(props: &[(&str, &str)], own: u32) -> Option<(String, String)> {
        let m: HashMap<&str, &str> = props.iter().copied().collect();
        node_program(|k| m.get(k).copied(), own)
    }

    #[test]
    fn a_program_is_known_by_its_binary_and_read_by_its_name() {
        let firefox = [("media.class", "Stream/Output/Audio"), ("application.name", "Firefox"), ("application.process.binary", "firefox"), ("application.process.id", "4242")];
        assert_eq!(program(&firefox, 1), Some(("firefox".into(), "Firefox".into())));
        assert_eq!(program(&firefox, 4242), None, "Snagster's own stream is left out");
        // No binary: the application name; neither: the node name.
        assert_eq!(program(&[("media.class", "Stream/Output/Audio"), ("application.name", "Spotify")], 1), Some(("spotify".into(), "Spotify".into())));
        assert_eq!(program(&[("media.class", "Stream/Output/Audio"), ("node.name", "mpv")], 1), Some(("mpv".into(), String::new())));
        assert_eq!(program(&[("media.class", "Stream/Output/Audio")], 1), None, "nothing to know it by");
        // Not a program's sound: a microphone stream, a device.
        assert_eq!(program(&[("media.class", "Stream/Input/Audio"), ("application.name", "Zoom")], 1), None);
        assert_eq!(program(&[("media.class", "Audio/Sink"), ("node.name", "alsa_output")], 1), None);
        assert_eq!(program(&[("media.class", "Stream/Output/Audio"), ("application.name", "  "), ("application.process.binary", "")], 1), None);
    }
}
