//! One Snagster at a time: opening it again (the Start menu, the Dock, a file double-clicked)
//! brings the running window to the front instead of a second copy, and a file to open goes to
//! that window. People who want more copies switch it on in Settings ("Allow more than one
//! Snagster") or start with `--new-window`; command-line commands, AI apps and the check runs
//! never ask.
//!
//! The running Snagster listens on 127.0.0.1 at a port the system picks and writes
//! `instance.lock` next to the settings: the port and a random secret (readable by this user
//! only). A new Snagster reads it, connects, sends the secret and what it wants, gets "ok" and
//! quits. No answer (the old one crashed and left the file) and the new one carries on as the
//! first. Nothing listens beyond this computer, and a message without the secret is ignored.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// What another start of Snagster asked the running one to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Ask {
    /// Come to the front.
    Show,
    /// Come to the front and open this file.
    Open(PathBuf),
    /// Do this capture or recording (an `Action` by name, e.g. "Area"): `snagster --action Area`,
    /// for a keyboard shortcut set in the desktop's own settings.
    Action(String),
    /// Run a command from `catalog::COMMANDS` (an AI app's `app_command`), with its value.
    Command(String, Option<String>),
}

impl Ask {
    /// What a start with these command-line choices asks for.
    pub fn from_args(open: Option<&Path>, action: Option<&str>) -> Ask {
        match (action, open) {
            (Some(a), _) => Ask::Action(a.to_string()),
            (None, Some(p)) => Ask::Open(p.to_path_buf()),
            (None, None) => Ask::Show,
        }
    }
}

/// An action name as the command line and the messages carry it: letters only.
fn action_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 40 && name.chars().all(|c| c.is_ascii_alphabetic())
}

/// Sends a command to the running Snagster window. Fails when no window listens (none is
/// open, or "Allow more than one Snagster" is on).
pub fn send_command(id: &str, value: Option<&str>) -> Result<(), String> {
    let lock = lock_path().ok_or("Snagster has no settings folder here")?;
    send_command_at(&lock, id, value)
}

pub(crate) fn send_command_at(lock: &Path, id: &str, value: Option<&str>) -> Result<(), String> {
    let (port, secret) = std::fs::read_to_string(lock).ok().as_deref().and_then(parse_lock).ok_or("Snagster's window isn't open")?;
    send(port, &command_message(&secret, id, value)).map_err(|_| "Snagster's window isn't open (or doesn't answer)".to_string())
}

/// What the running window answers to a question (`query`): set once by the window, called on
/// the listening thread, so it must not wait on the window's frame. None: not a question it
/// knows.
pub type Answer = fn(&str) -> Option<String>;

static ANSWER: OnceLock<Answer> = OnceLock::new();

/// The window says how it answers questions from other Snagsters (an AI app's own server).
pub fn set_answer(f: Answer) {
    let _ = ANSWER.set(f);
}

/// Ask the running Snagster window a question (e.g. "replay_status") and get its one-line
/// answer. Fails when no window listens or it doesn't know the question. (Tests use
/// `query_at` with a lock file of their own.)
#[cfg(not(test))]
pub fn query(id: &str) -> Result<String, String> {
    let lock = lock_path().ok_or("Snagster has no settings folder here")?;
    query_at(&lock, id)
}

pub(crate) fn query_at(lock: &Path, id: &str) -> Result<String, String> {
    let (port, secret) = std::fs::read_to_string(lock).ok().as_deref().and_then(parse_lock).ok_or("Snagster's window isn't open")?;
    let reply = exchange(port, &query_message(&secret, id)).map_err(|_| "Snagster's window isn't open (or doesn't answer)".to_string())?;
    match reply.strip_prefix("ok ") {
        Some(a) => Ok(a.to_string()),
        None => Err(format!("The Snagster window doesn't answer {id}")),
    }
}

/// The secret, then QUERY and the question's id.
pub(crate) fn query_message(secret: &str, id: &str) -> String {
    format!("{secret}\nQUERY {}\n", id.replace(['\r', '\n', '\t'], " "))
}

/// The question in a message, if it carries the secret.
pub(crate) fn parse_query(text: &str, secret: &str) -> Option<String> {
    let mut lines = text.lines();
    if lines.next()? != secret {
        return None;
    }
    let id = lines.next()?.strip_prefix("QUERY ")?.trim();
    (!id.is_empty() && id.len() <= 40).then(|| id.to_string())
}

/// The line sent back for a question: "ok " and the answer on one line, or "unknown".
pub(crate) fn query_reply(answer: Option<String>) -> String {
    match answer {
        Some(a) => format!("ok {}\n", a.replace(['\r', '\n'], " ")),
        None => "unknown\n".to_string(),
    }
}

/// The secret, then COMMAND, the id and the value on one line (tab between; no new lines).
pub(crate) fn command_message(secret: &str, id: &str, value: Option<&str>) -> String {
    let clean = |s: &str| s.replace(['\r', '\n', '\t'], " ");
    match value {
        Some(v) => format!("{secret}\nCOMMAND {}\t{}\n", clean(id), clean(v)),
        None => format!("{secret}\nCOMMAND {}\n", clean(id)),
    }
}

/// How a start went.
#[derive(Debug, PartialEq)]
pub enum Claim {
    /// This is the first Snagster (and now listens for the next ones).
    First,
    /// A running Snagster took over; this one should quit.
    Handed,
}

type Inbox = Arc<Mutex<Vec<Ask>>>;

fn inbox() -> &'static Inbox {
    static INBOX: OnceLock<Inbox> = OnceLock::new();
    INBOX.get_or_init(Inbox::default)
}

static WAKE: OnceLock<eframe::egui::Context> = OnceLock::new();

/// Where the running Snagster says where it listens.
fn lock_path() -> Option<PathBuf> {
    Some(crate::settings::settings_path()?.parent()?.join("instance.lock"))
}

/// Be the first Snagster, or hand what this start asks for to the running one. The first one
/// keeps a file or an action for itself, to do once its window is up.
pub fn claim(ask: Ask) -> Claim {
    let claim = match lock_path() {
        Some(p) => claim_at(&p, &ask, inbox().clone()),
        None => Claim::First,
    };
    if claim == Claim::First && matches!(ask, Ask::Action(_)) && let Ok(mut g) = inbox().lock() {
        g.push(ask);
    }
    claim
}

/// Do this once the window is up (an action asked for on this start's own command line).
pub fn keep(ask: Ask) {
    if let Ok(mut g) = inbox().lock() {
        g.push(ask);
    }
}

/// Repaint this window when another start asks for something.
pub fn set_wake(ctx: &eframe::egui::Context) {
    let _ = WAKE.set(ctx.clone());
}

/// What other starts have asked for since the last look (the window takes them every frame).
pub fn take() -> Vec<Ask> {
    inbox().lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default()
}

pub(crate) fn claim_at(lock: &Path, ask: &Ask, inbox: Inbox) -> Claim {
    if let Some((port, secret)) = std::fs::read_to_string(lock).ok().as_deref().and_then(parse_lock)
        && hand_over(port, &secret, ask).is_ok()
    {
        return Claim::Handed;
    }
    // Nobody answered: this is the first. A failure to listen just means no handing over.
    let _ = listen(lock, inbox);
    Claim::First
}

/// "port secret" from the lock file.
pub(crate) fn parse_lock(text: &str) -> Option<(u16, String)> {
    let mut it = text.split_whitespace();
    let port = it.next()?.parse().ok()?;
    let secret = it.next()?.to_string();
    (port > 0 && secret.len() >= 16).then_some((port, secret))
}

/// The message to the running Snagster: the secret, then SHOW, OPEN and a path, or ACTION and
/// a name.
pub(crate) fn message(secret: &str, ask: &Ask) -> String {
    match ask {
        Ask::Open(p) => format!("{secret}\nOPEN {}\n", p.display()),
        Ask::Action(a) => format!("{secret}\nACTION {a}\n"),
        Ask::Command(id, value) => command_message(secret, id, value.as_deref()),
        Ask::Show => format!("{secret}\nSHOW\n"),
    }
}

/// What a message asks, if it carries the secret.
pub(crate) fn parse_message(text: &str, secret: &str) -> Option<Ask> {
    let mut lines = text.lines();
    if lines.next()? != secret {
        return None;
    }
    let what = lines.next()?;
    if what == "SHOW" {
        Some(Ask::Show)
    } else if let Some(a) = what.strip_prefix("ACTION ") {
        action_name(a).then(|| Ask::Action(a.to_string()))
    } else if let Some(c) = what.strip_prefix("COMMAND ") {
        let (id, value) = match c.split_once('\t') {
            Some((id, v)) => (id, Some(v.to_string())),
            None => (c, None),
        };
        (!id.trim().is_empty()).then(|| Ask::Command(id.trim().to_string(), value))
    } else {
        what.strip_prefix("OPEN ").map(|p| Ask::Open(PathBuf::from(p))).filter(|a| !matches!(a, Ask::Open(p) if p.as_os_str().is_empty()))
    }
}

fn hand_over(port: u16, secret: &str, ask: &Ask) -> Result<(), String> {
    send(port, &message(secret, ask))
}

/// Sends one message and waits for "ok".
fn send(port: u16, msg: &str) -> Result<(), String> {
    let reply = exchange(port, msg)?;
    if reply == "ok" { Ok(()) } else { Err(format!("unexpected answer {reply:?}")) }
}

/// Sends one message and returns the one line that comes back (trimmed).
fn exchange(port: u16, msg: &str) -> Result<String, String> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(500)).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(2))).map_err(|e| e.to_string())?;
    s.write_all(msg.as_bytes()).map_err(|e| e.to_string())?;
    let mut reply = String::new();
    BufReader::new(s).read_line(&mut reply).map_err(|e| e.to_string())?;
    Ok(reply.trim().to_string())
}

/// A secret nobody else can guess: two random 64-bit numbers from the standard library's
/// randomly keyed hasher (no extra dependency).
pub(crate) fn new_secret() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for i in 0..2u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i ^ std::process::id() as u64);
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

fn listen(lock: &Path, inbox: Inbox) -> Result<(), String> {
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|e| e.to_string())?;
    let port = l.local_addr().map_err(|e| e.to_string())?.port();
    let secret = new_secret();
    write_private(lock, &format!("{port} {secret}\n")).map_err(|e| e.to_string())?;
    std::thread::Builder::new()
        .name("snagster-instance".into())
        .spawn(move || {
            for s in l.incoming() {
                let Ok(mut s) = s else { continue };
                let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                // At most 4 KB: a secret and a path.
                let mut text = String::new();
                let mut buf = [0u8; 4096];
                let mut got = 0;
                while got < buf.len() {
                    match s.read(&mut buf[got..]) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            got += n;
                            text = String::from_utf8_lossy(&buf[..got]).into_owned();
                            if text.lines().count() >= 2 && text.ends_with('\n') {
                                break;
                            }
                        }
                    }
                }
                if let Some(id) = parse_query(&text, &secret) {
                    let answer = ANSWER.get().and_then(|f| f(&id));
                    let _ = s.write_all(query_reply(answer).as_bytes());
                } else if let Some(ask) = parse_message(&text, &secret) {
                    if let Ok(mut g) = inbox.lock() {
                        g.push(ask);
                    }
                    let _ = s.write_all(b"ok\n");
                    if let Some(c) = WAKE.get() {
                        c.request_repaint();
                    }
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(unix)]
fn write_private(p: &Path, text: &str) -> std::io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(p)?;
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.write_all(text.as_bytes())
}

#[cfg(not(unix))]
fn write_private(p: &Path, text: &str) -> std::io::Result<()> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(p, text)
}

/// Fuzz target (`src/fuzz.rs`): the lock file and what arrives on the local port, then what
/// the window makes of a message (a command and its value are checked the same way).
#[cfg(test)]
pub(crate) mod fuzzing {
    use super::*;
    use crate::fuzz::Target;

    const SECRET: &str = "0123456789abcdef0123";

    pub fn targets() -> Vec<Target> {
        vec![Target {
            name: "instance",
            seeds: || {
                let mut out: Vec<Vec<u8>> = [Ask::Show, Ask::Open(PathBuf::from("C:/x y/shot.png")), Ask::Action("Area".into()), Ask::Command("zoom".into(), Some("150".into())), Ask::Command("ribbon_tab".into(), None)]
                    .iter()
                    .map(|a| message(SECRET, a).into_bytes())
                    .collect();
                out.push(format!("45831 {SECRET}\n").into_bytes());
                out
            },
            words: || {
                let mut w: Vec<String> = ["SHOW", "OPEN ", "ACTION ", "COMMAND ", "\t", "\n", "\r\n", SECRET].map(String::from).to_vec();
                w.extend(crate::catalog::COMMANDS.iter().take(80).map(|c| c.id.to_string()));
                w.extend(crate::settings::Action::ALL.iter().map(|a| format!("{a:?}")));
                w
            },
            run: |b| {
                // At most 4 KB arrive, as text.
                let text = String::from_utf8_lossy(&b[..b.len().min(4096)]);
                let _ = parse_lock(&text);
                match parse_message(&text, SECRET) {
                    Some(Ask::Command(id, value)) => {
                        let _ = crate::app::commands_ui::parse(&id, value.as_deref());
                    }
                    Some(Ask::Action(name)) => {
                        assert!(action_name(&name), "{name:?}");
                        let _ = crate::settings::Action::ALL.iter().find(|a| format!("{a:?}") == name);
                    }
                    Some(Ask::Open(p)) => assert!(!p.as_os_str().is_empty()),
                    Some(Ask::Show) | None => {}
                }
                // Never without the secret.
                assert!(parse_message(&text, "another secret, also long").is_none() || text.starts_with("another secret, also long"));
            },
            budget: |n| 64 * 1024 + 8 * n,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_action_on_the_command_line_wins_over_a_file() {
        let p = Path::new("/tmp/a.png");
        assert_eq!(Ask::from_args(None, None), Ask::Show);
        assert_eq!(Ask::from_args(Some(p), None), Ask::Open(p.to_path_buf()));
        assert_eq!(Ask::from_args(Some(p), Some("Area")), Ask::Action("Area".into()));
        assert!(action_name("AllInOne"));
        assert!(!action_name("All In One"));
        assert!(!action_name(""));
    }

    #[test]
    fn messages_need_the_secret() {
        let secret = new_secret();
        assert_eq!(secret.len(), 32);
        assert_ne!(secret, new_secret(), "a new secret each time");
        assert_eq!(parse_message(&message(&secret, &Ask::Show), &secret), Some(Ask::Show));
        let p = Path::new("C:/Pictures/a b.png");
        assert_eq!(parse_message(&message(&secret, &Ask::Open(p.to_path_buf())), &secret), Some(Ask::Open(p.to_path_buf())));
        assert_eq!(parse_message(&message(&secret, &Ask::Action("Area".into())), &secret), Some(Ask::Action("Area".into())));
        // Only a plain action name gets through.
        assert_eq!(parse_message(&format!("{secret}\nACTION \n"), &secret), None);
        assert_eq!(parse_message(&format!("{secret}\nACTION ../../x\n"), &secret), None);
        assert_eq!(parse_message(&message("wrong-secret-0000", &Ask::Show), &secret), None);
        assert_eq!(parse_message("", &secret), None);
        assert_eq!(parse_message(&format!("{secret}\nDELETE everything\n"), &secret), None);
        assert_eq!(parse_message(&format!("{secret}\nOPEN \n"), &secret), None);
        assert_eq!(parse_message(&command_message(&secret, "zoom_in", None), &secret), Some(Ask::Command("zoom_in".into(), None)));
        assert_eq!(parse_message(&command_message(&secret, "open", Some("C:/a\nb.png")), &secret), Some(Ask::Command("open".into(), Some("C:/a b.png".into()))));
        assert_eq!(parse_message(&format!("{secret}\nCOMMAND \n"), &secret), None);
        assert_eq!(parse_message(&command_message("wrong-secret-0000", "exit", None), &secret), None);
        assert_eq!(parse_lock("45831 0123456789abcdef0123\n"), Some((45831, "0123456789abcdef0123".into())));
        assert_eq!(parse_lock("0 0123456789abcdef"), None);
        assert_eq!(parse_lock("80 short"), None);
        assert_eq!(parse_lock("garbage"), None);
    }

    /// The first start listens; the next hands its file over and is told to quit; the first
    /// gets the file. A lock left by a Snagster that is gone doesn't stop a new one.
    #[test]
    fn a_second_start_hands_over_to_the_first() {
        let dir = std::env::temp_dir().join(format!("snagster-instance-test-{}", std::process::id()));
        let lock = dir.join("instance.lock");
        let _ = std::fs::remove_dir_all(&dir);
        let first: Inbox = Inbox::default();
        assert_eq!(claim_at(&lock, &Ask::Show, first.clone()), Claim::First);
        assert!(lock.exists());
        let second: Inbox = Inbox::default();
        assert_eq!(claim_at(&lock, &Ask::Open(PathBuf::from("/tmp/shot.png")), second.clone()), Claim::Handed);
        assert_eq!(claim_at(&lock, &Ask::Show, second.clone()), Claim::Handed);
        assert_eq!(claim_at(&lock, &Ask::Action("Area".into()), second.clone()), Claim::Handed);
        assert_eq!(*first.lock().unwrap(), vec![Ask::Open(PathBuf::from("/tmp/shot.png")), Ask::Show, Ask::Action("Area".into())]);
        assert!(second.lock().unwrap().is_empty());
        // A stale lock (nothing listens on its port): the new start is the first.
        let stale = dir.join("stale.lock");
        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port();
        std::fs::write(&stale, format!("{port} 0123456789abcdef0123456789abcdef\n")).unwrap();
        assert_eq!(claim_at(&stale, &Ask::Show, Inbox::default()), Claim::First);
        assert_ne!(std::fs::read_to_string(&stale).unwrap().split_whitespace().next(), Some(port.to_string().as_str()), "it now holds the new port");
        // A wrong secret gets no answer, so it isn't handed over.
        let real = std::fs::read_to_string(&lock).unwrap();
        let (p, _) = parse_lock(&real).unwrap();
        assert!(hand_over(p, "not-the-secret-not-the-secret-00", &Ask::Show).is_err());
        // An AI app's command reaches the first one, with its value.
        first.lock().unwrap().clear();
        send_command_at(&lock, "zoom_in", None).unwrap();
        send_command_at(&lock, "effect", Some("Border")).unwrap();
        assert_eq!(*first.lock().unwrap(), vec![Ask::Command("zoom_in".into(), None), Ask::Command("effect".into(), Some("Border".into()))]);
        assert!(send_command_at(&dir.join("none.lock"), "zoom_in", None).unwrap_err().contains("isn't open"));
        // A question is answered at once on the listening thread (the window's replay state for
        // an AI app's own server), never put in the inbox; one it doesn't know says so.
        set_answer(|id| (id == "replay_status").then(|| "{\"phase\": \"on\",\n\"seconds\": 60}".to_string()));
        assert_eq!(query_at(&lock, "replay_status").unwrap(), "{\"phase\": \"on\", \"seconds\": 60}", "one line");
        assert!(query_at(&lock, "what_time").unwrap_err().contains("doesn't answer what_time"));
        assert!(query_at(&dir.join("none.lock"), "replay_status").unwrap_err().contains("isn't open"));
        assert!(first.lock().unwrap().len() == 2, "questions aren't commands");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn questions_need_the_secret_and_a_short_id() {
        let secret = new_secret();
        assert_eq!(parse_query(&query_message(&secret, "replay_status"), &secret).as_deref(), Some("replay_status"));
        assert_eq!(parse_query(&query_message("wrong-secret-0000", "replay_status"), &secret), None);
        assert_eq!(parse_query(&format!("{secret}\nQUERY \n"), &secret), None);
        assert_eq!(parse_query(&format!("{secret}\nQUERY {}\n", "x".repeat(41)), &secret), None);
        assert_eq!(parse_query(&command_message(&secret, "zoom_in", None), &secret), None, "a command isn't a question");
        assert_eq!(parse_message(&query_message(&secret, "replay_status"), &secret), None, "a question isn't a command");
        assert_eq!(query_reply(Some("a\nb".into())), "ok a b\n");
        assert_eq!(query_reply(None), "unknown\n");
    }
}
