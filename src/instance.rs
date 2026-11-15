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

/// Be the first Snagster, or hand `open` (or just "show") to the running one.
pub fn claim(open: Option<&Path>) -> Claim {
    match lock_path() {
        Some(p) => claim_at(&p, open, inbox().clone()),
        None => Claim::First,
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

pub(crate) fn claim_at(lock: &Path, open: Option<&Path>, inbox: Inbox) -> Claim {
    if let Some((port, secret)) = std::fs::read_to_string(lock).ok().as_deref().and_then(parse_lock)
        && hand_over(port, &secret, open).is_ok()
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

/// The message to the running Snagster: the secret, then SHOW or OPEN and a path.
pub(crate) fn message(secret: &str, open: Option<&Path>) -> String {
    match open {
        Some(p) => format!("{secret}\nOPEN {}\n", p.display()),
        None => format!("{secret}\nSHOW\n"),
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
    } else {
        what.strip_prefix("OPEN ").map(|p| Ask::Open(PathBuf::from(p))).filter(|a| !matches!(a, Ask::Open(p) if p.as_os_str().is_empty()))
    }
}

fn hand_over(port: u16, secret: &str, open: Option<&Path>) -> Result<(), String> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(500)).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(2))).map_err(|e| e.to_string())?;
    s.write_all(message(secret, open).as_bytes()).map_err(|e| e.to_string())?;
    let mut reply = String::new();
    BufReader::new(s).read_line(&mut reply).map_err(|e| e.to_string())?;
    if reply.trim() == "ok" { Ok(()) } else { Err(format!("unexpected answer {reply:?}")) }
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
                if let Some(ask) = parse_message(&text, &secret) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_need_the_secret() {
        let secret = new_secret();
        assert_eq!(secret.len(), 32);
        assert_ne!(secret, new_secret(), "a new secret each time");
        assert_eq!(parse_message(&message(&secret, None), &secret), Some(Ask::Show));
        let p = Path::new("C:/Pictures/a b.png");
        assert_eq!(parse_message(&message(&secret, Some(p)), &secret), Some(Ask::Open(p.to_path_buf())));
        assert_eq!(parse_message(&message("wrong-secret-0000", None), &secret), None);
        assert_eq!(parse_message("", &secret), None);
        assert_eq!(parse_message(&format!("{secret}\nDELETE everything\n"), &secret), None);
        assert_eq!(parse_message(&format!("{secret}\nOPEN \n"), &secret), None);
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
        assert_eq!(claim_at(&lock, None, first.clone()), Claim::First);
        assert!(lock.exists());
        let second: Inbox = Inbox::default();
        assert_eq!(claim_at(&lock, Some(Path::new("/tmp/shot.png")), second.clone()), Claim::Handed);
        assert_eq!(claim_at(&lock, None, second.clone()), Claim::Handed);
        assert_eq!(*first.lock().unwrap(), vec![Ask::Open(PathBuf::from("/tmp/shot.png")), Ask::Show]);
        assert!(second.lock().unwrap().is_empty());
        // A stale lock (nothing listens on its port): the new start is the first.
        let stale = dir.join("stale.lock");
        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port();
        std::fs::write(&stale, format!("{port} 0123456789abcdef0123456789abcdef\n")).unwrap();
        assert_eq!(claim_at(&stale, None, Inbox::default()), Claim::First);
        assert_ne!(std::fs::read_to_string(&stale).unwrap().split_whitespace().next(), Some(port.to_string().as_str()), "it now holds the new port");
        // A wrong secret gets no answer, so it isn't handed over.
        let real = std::fs::read_to_string(&lock).unwrap();
        let (p, _) = parse_lock(&real).unwrap();
        assert!(hand_over(p, "not-the-secret-not-the-secret-00", None).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
