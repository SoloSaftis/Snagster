//! Deleting with an Undo. A deleted capture goes to the Recycle Bin / Trash, and for a short
//! while afterwards an Undo puts it back where it was, with its library tags (Windows, GNOME and
//! macOS all ask for an undo after a destructive action instead of a "Are you sure?").
//!
//! How a file comes back depends on the system (`Way`):
//! - Windows and Linux: the file goes to the Recycle Bin / Trash at once and Undo restores it
//!   from there (the `trash` crate's `os_limited::restore_all`).
//! - macOS (the `trash` crate can't restore there) and the Flatpak (its Trash is the sandbox's
//!   own): the file waits in a hidden `.snagster-deleted` folder next to it while the Undo
//!   shows, and goes to the Trash when the Undo goes away (or Snagster closes). Undo moves it
//!   back. A folder left behind by a crash is sent on to the Trash at the next start.
//!
//! Several deletes while the Undo shows are undone together, and each delete shows it for the
//! full time again.

use crate::library::Meta;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long the window's Undo shows after the last delete (it waits longer while the pointer
/// or the keyboard is on it, or Snagster is in the background: `Pending::keep`). The Mac gets a
/// little longer, as its review asked.
pub const UNDO_FOR: Duration = Duration::from_secs(if cfg!(target_os = "macos") { 15 } else { 10 });
/// After the pointer or keyboard leaves the Undo, it shows at least this much longer.
pub const KEEP_AFTER: Duration = Duration::from_secs(4);
/// How long an AI app has to call `undo_delete` (it reads, thinks and answers slower).
pub const MCP_UNDO_FOR: Duration = Duration::from_secs(120);
/// The hidden folder, next to the file, where it waits on systems that can't restore.
pub const HOLD_DIR: &str = ".snagster-deleted";

/// How a deleted file can be brought back on this system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    /// It goes to the Recycle Bin / Trash at once; Undo restores it from there.
    Restore,
    /// It waits in a hidden folder until the Undo goes away, then goes to the Trash.
    Hold,
}

/// The way that works here.
pub fn way_here() -> Way {
    way_for(cfg!(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), in_flatpak())
}

fn way_for(can_restore: bool, flatpak: bool) -> Way {
    if can_restore && !flatpak { Way::Restore } else { Way::Hold }
}

fn in_flatpak() -> bool {
    cfg!(target_os = "linux") && (std::env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists())
}

/// The system's Recycle Bin / Trash (tests use a folder instead).
pub trait Bin: Send + Sync {
    /// Move `path` to the Recycle Bin / Trash.
    fn delete(&self, path: &Path) -> Result<(), String>;
    /// Bring back the file most recently deleted from `path`.
    fn restore(&self, path: &Path) -> Result<(), String>;
}

/// The real Recycle Bin / Trash.
pub struct SystemBin;

impl Bin for SystemBin {
    fn delete(&self, path: &Path) -> Result<(), String> {
        // The Flatpak's own Trash (in its sandbox) is one the person never sees: the desktop's
        // Trash portal puts it in theirs.
        #[cfg(target_os = "linux")]
        if in_flatpak() {
            return portal_trash(path);
        }
        trash::delete(path).map_err(|e| e.to_string())
    }

    #[cfg(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    fn restore(&self, path: &Path) -> Result<(), String> {
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else { return Err("not a file".into()) };
        let items = trash::os_limited::list().map_err(|e| e.to_string())?;
        let newest = items
            .into_iter()
            .filter(|i| i.name == name && same_dir(&i.original_parent, dir))
            .max_by_key(|i| i.time_deleted)
            .ok_or_else(|| crate::i18n::t(crate::platform::trash_text("it isn't in the Recycle Bin any more", "it isn't in the Trash any more")).to_string())?;
        trash::os_limited::restore_all([newest]).map_err(|e| e.to_string())
    }

    #[cfg(not(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))))]
    fn restore(&self, _path: &Path) -> Result<(), String> {
        Err(crate::i18n::t("this system can't bring files back from the Trash").into())
    }
}

/// The desktop's Trash portal (it takes the open file; it can't restore).
#[cfg(target_os = "linux")]
fn portal_trash(path: &Path) -> Result<(), String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    pollster::block_on(ashpd::desktop::trash::trash_file(&f)).map_err(|e| e.to_string())
}

/// Whether two folders are the same (Windows ignores case and the `\\?\` prefix).
#[cfg(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let p = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let s = p.to_string_lossy().trim_start_matches(r"\\?\").trim_end_matches(['/', '\\']).to_string();
        if cfg!(windows) { s.to_lowercase() } else { s }
    };
    norm(a) == norm(b)
}

/// One deleted file.
#[derive(Clone, Debug)]
pub struct Gone {
    /// Where it was (and goes back to).
    pub path: PathBuf,
    /// Its library title and tags, written back beside it on Undo (None: not a library capture).
    pub meta: Option<Meta>,
    /// Where it waits (`Way::Hold`); None: it is in the Recycle Bin / Trash already.
    held: Option<PathBuf>,
    /// Open it in the editor again when it comes back (it was taken from Recent).
    pub reopen: bool,
}

impl Gone {
    /// A file that went to the Recycle Bin already (tests, and the screenshot view: nothing is
    /// left to finish).
    pub fn in_bin(path: &Path) -> Gone {
        Gone { path: path.to_path_buf(), meta: None, held: None, reopen: false }
    }

    pub fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
    }
}

/// Delete `path` the way `way` says, with the library's `.json` beside it and its thumbnail.
pub fn remove(path: &Path, meta: Option<Meta>, way: Way, bin: &dyn Bin) -> Result<Gone, String> {
    if !path.is_file() {
        return Err(format!("{} is gone", path.display()));
    }
    let held = match way {
        Way::Restore => {
            bin.delete(path)?;
            None
        }
        Way::Hold => {
            let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else { return Err("not a file".into()) };
            let hold = dir.join(HOLD_DIR);
            if std::fs::create_dir_all(&hold).is_err() {
                // A folder that can't take another folder: to the Trash at once (Undo then
                // asks the Trash, which can't always give it back).
                bin.delete(path)?;
                return Ok(tidy(path, meta, None));
            }
            remember_hold_dir(&hold);
            // A file of the same name already waiting (deleted, made again, deleted again).
            let mut to = hold.join(name);
            let mut n = 2;
            while to.exists() {
                to = hold.join(format!("{n}~{}", name.to_string_lossy()));
                n += 1;
            }
            std::fs::rename(path, &to).map_err(|e| e.to_string())?;
            Some(to)
        }
    };
    Ok(tidy(path, meta, held))
}

/// After the file went: its tags file (a library capture) and thumbnail go too.
fn tidy(path: &Path, meta: Option<Meta>, held: Option<PathBuf>) -> Gone {
    if meta.is_some() {
        let _ = std::fs::remove_file(path.with_extension("json"));
    }
    if let (Some(dir), Some(name)) = (path.parent(), path.file_name()) {
        let _ = std::fs::remove_file(dir.join(".thumbs").join(name));
    }
    Gone { path: path.to_path_buf(), meta, held, reopen: false }
}

/// Undo: put the file back where it was, with its tags.
pub fn bring_back(g: &Gone, bin: &dyn Bin) -> Result<(), String> {
    if g.path.exists() {
        return Err(crate::i18n::t("a new file has taken its name").to_string());
    }
    match &g.held {
        None => bin.restore(&g.path)?,
        Some(h) => {
            std::fs::rename(h, &g.path).map_err(|e| e.to_string())?;
            tidy_hold_dir(h);
        }
    }
    if let Some(meta) = &g.meta {
        let item = crate::library::Item { path: g.path.clone(), meta: meta.clone() };
        item.save_meta()?;
    }
    Ok(())
}

/// The Undo went away: a file that waited goes on to the Recycle Bin / Trash (from where it
/// was, so the system's own Restore / Put Back puts it in the right folder).
pub fn finish(g: &Gone, bin: &dyn Bin) -> Result<(), String> {
    let Some(h) = &g.held else { return Ok(()) };
    if !h.exists() {
        return Ok(());
    }
    let r = if g.path.exists() {
        // Something new took its name meanwhile: send it from where it waits.
        bin.delete(h)
    } else {
        std::fs::rename(h, &g.path).map_err(|e| e.to_string()).and_then(|_| bin.delete(&g.path))
    };
    tidy_hold_dir(h);
    r
}

/// Send on to the Trash what an earlier Snagster left waiting (it closed before the Undo went
/// away). Called at the start.
pub fn finish_leftovers(bin: &dyn Bin) {
    let Some(list) = hold_list_path() else { return };
    let dirs = std::fs::read_to_string(&list).unwrap_or_default();
    for d in dirs.lines().filter(|l| !l.trim().is_empty()) {
        finish_hold_dir(Path::new(d), bin);
    }
}

/// Everything waiting in one `.snagster-deleted` folder goes to the Trash.
fn finish_hold_dir(hold: &Path, bin: &dyn Bin) {
    let (Ok(rd), Some(dir)) = (std::fs::read_dir(hold), hold.parent()) else {
        forget_hold_dir(hold);
        return;
    };
    for e in rd.flatten() {
        let held = e.path();
        // "2~name.png" waited beside an earlier "name.png".
        let name = e.file_name().to_string_lossy().to_string();
        let original = name.split_once('~').filter(|(n, _)| n.parse::<u32>().is_ok()).map(|(_, rest)| rest.to_string()).unwrap_or(name);
        let _ = finish(&Gone { path: dir.join(original), meta: None, held: Some(held), reopen: false }, bin);
    }
    tidy_hold_dir(&hold.join("x"));
}

/// Remove the holding folder once it is empty (and forget it).
fn tidy_hold_dir(held_file: &Path) {
    if let Some(hold) = held_file.parent()
        && std::fs::remove_dir(hold).is_ok()
    {
        forget_hold_dir(hold);
    }
}

// The holding folders in use, one per line, so a crash doesn't leave files waiting for ever.
// Tests point it at their own file.
thread_local! {
    static HOLD_LIST: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

fn hold_list_path() -> Option<PathBuf> {
    HOLD_LIST.with(|h| h.borrow().clone()).or_else(|| if cfg!(test) { None } else { Some(crate::userdirs::snagster_dir()?.join("held-deletes.txt")) })
}

fn remember_hold_dir(hold: &Path) {
    let Some(list) = hold_list_path() else { return };
    let s = hold.to_string_lossy().to_string();
    let mut lines: Vec<String> = std::fs::read_to_string(&list).unwrap_or_default().lines().map(String::from).collect();
    if !lines.contains(&s) {
        lines.push(s);
        if let Some(p) = list.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        let _ = std::fs::write(&list, lines.join("\n") + "\n");
    }
}

fn forget_hold_dir(hold: &Path) {
    let Some(list) = hold_list_path() else { return };
    let s = hold.to_string_lossy().to_string();
    let Ok(text) = std::fs::read_to_string(&list) else { return };
    let lines: Vec<&str> = text.lines().filter(|l| *l != s && !l.trim().is_empty()).collect();
    let _ = if lines.is_empty() { std::fs::remove_file(&list) } else { std::fs::write(&list, lines.join("\n") + "\n") };
}

/// The deletes the Undo brings back: it grows while the Undo shows, and each delete shows it
/// for the full time again. What is still waiting goes to the Trash when it is dropped.
pub struct Pending {
    gone: Vec<Gone>,
    until: Option<Instant>,
    /// Whether Ctrl+Z / ⌘Z still means "undo the delete" (until something else is done).
    shortcut: bool,
    lasts: Duration,
    bin: Arc<dyn Bin>,
}

impl Default for Pending {
    fn default() -> Self {
        Self::new(UNDO_FOR, Arc::new(SystemBin))
    }
}

impl Pending {
    pub fn new(lasts: Duration, bin: Arc<dyn Bin>) -> Self {
        Self { gone: Vec::new(), until: None, shortcut: false, lasts, bin }
    }

    pub fn bin(&self) -> Arc<dyn Bin> {
        self.bin.clone()
    }

    /// A delete happened. Gives back what an Undo that timed out leaves to finish.
    pub fn add(&mut self, g: Gone, now: Instant) -> Vec<Gone> {
        let done = self.take_expired(now);
        self.gone.push(g);
        self.until = Some(now + self.lasts);
        self.shortcut = true;
        done
    }

    /// Whether the Undo shows.
    pub fn showing(&self, now: Instant) -> bool {
        self.until.is_some_and(|u| now < u) && !self.gone.is_empty()
    }

    /// How long the Undo still shows.
    pub fn left(&self, now: Instant) -> Option<Duration> {
        self.showing(now).then(|| self.until.unwrap() - now)
    }

    /// The pointer or keyboard is on the Undo, or Snagster is in the background: it doesn't go
    /// meanwhile, and shows a little longer after.
    pub fn keep(&mut self, now: Instant) {
        if self.showing(now)
            && let Some(u) = self.until.as_mut()
        {
            *u = (*u).max(now + KEEP_AFTER);
        }
    }

    /// Something else was done since: Ctrl+Z / ⌘Z undoes that, and only the button undoes the
    /// delete.
    pub fn drop_shortcut(&mut self) {
        self.shortcut = false;
    }

    /// Whether Ctrl+Z / ⌘Z undoes the delete now.
    pub fn owns_shortcut(&self, now: Instant) -> bool {
        self.shortcut && self.showing(now)
    }

    pub fn count(&self) -> usize {
        self.gone.len()
    }

    /// The one file deleted, when it was one.
    pub fn only(&self) -> Option<&Gone> {
        if let [g] = self.gone.as_slice() { Some(g) } else { None }
    }

    /// Closed by hand (or its time is up): what it held is to be finished (`finish`), off the
    /// window's thread.
    pub fn end_now(&mut self) -> Vec<Gone> {
        self.until = None;
        self.shortcut = false;
        std::mem::take(&mut self.gone)
    }

    /// The Undo's time is up: what it held, to finish.
    pub fn take_expired(&mut self, now: Instant) -> Vec<Gone> {
        if self.until.is_some_and(|u| now >= u) {
            return self.end_now();
        }
        Vec::new()
    }

    /// Undo pressed: everything it holds, to `bring_back`. Nothing once its time is up.
    pub fn take_for_undo(&mut self, now: Instant) -> Vec<Gone> {
        if !self.showing(now) {
            return Vec::new();
        }
        self.end_now()
    }

    /// Put the files back now (here, on this thread); the paths that came back, and for each
    /// that didn't, its name and why.
    pub fn undo_now(&mut self, now: Instant) -> (Vec<PathBuf>, Vec<(String, String)>) {
        let bin = self.bin.clone();
        undo_all(self.take_for_undo(now), bin.as_ref())
    }
}

/// `bring_back` each, newest last delete first; the paths that came back, and for each that
/// didn't, its name and why.
pub fn undo_all(gone: Vec<Gone>, bin: &dyn Bin) -> (Vec<PathBuf>, Vec<(String, String)>) {
    let (mut back, mut errors) = (Vec::new(), Vec::new());
    for g in gone.into_iter().rev() {
        match bring_back(&g, bin) {
            Ok(()) => back.push(g.path),
            Err(e) => errors.push((g.name(), e)),
        }
    }
    back.reverse();
    (back, errors)
}

impl Drop for Pending {
    fn drop(&mut self) {
        for g in std::mem::take(&mut self.gone) {
            let _ = finish(&g, self.bin.as_ref());
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A Recycle Bin that is a folder: deleted files are moved into it with where they came
    /// from, and restored from there.
    pub(crate) struct FolderBin {
        pub dir: PathBuf,
        pub log: Mutex<Vec<(PathBuf, PathBuf)>>,
    }

    impl FolderBin {
        pub fn new(dir: PathBuf) -> Self {
            std::fs::create_dir_all(&dir).unwrap();
            Self { dir, log: Mutex::new(Vec::new()) }
        }
        pub fn holds(&self) -> usize {
            self.log.lock().unwrap().len()
        }
    }

    impl Bin for FolderBin {
        fn delete(&self, path: &Path) -> Result<(), String> {
            let mut log = self.log.lock().unwrap();
            let to = self.dir.join(format!("{}-{}", log.len(), path.file_name().unwrap().to_string_lossy()));
            std::fs::rename(path, &to).map_err(|e| e.to_string())?;
            log.push((path.to_path_buf(), to));
            Ok(())
        }
        fn restore(&self, path: &Path) -> Result<(), String> {
            let mut log = self.log.lock().unwrap();
            let i = log.iter().rposition(|(from, _)| from == path).ok_or("not in the bin")?;
            let (from, at) = log.remove(i);
            std::fs::rename(at, from).map_err(|e| e.to_string())
        }
    }

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("snagster-recycle-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        // This thread's own list of holding folders.
        HOLD_LIST.with(|h| *h.borrow_mut() = Some(d.join("held-deletes.txt")));
        d
    }

    /// A library capture: picture, tags beside it, thumbnail.
    fn capture(dir: &Path, name: &str, tags: &[&str]) -> (PathBuf, Meta) {
        let p = dir.join(name);
        image::RgbaImage::from_pixel(5, 4, image::Rgba([1, 2, 3, 255])).save(&p).unwrap();
        let meta = Meta { title: name.into(), tags: tags.iter().map(|t| t.to_string()).collect(), created: "2026-11-18T10:00:00+01:00".into(), ..Default::default() };
        crate::library::Item { path: p.clone(), meta: meta.clone() }.save_meta().unwrap();
        std::fs::create_dir_all(dir.join(".thumbs")).unwrap();
        std::fs::write(dir.join(".thumbs").join(name), b"thumb").unwrap();
        (p, meta)
    }

    fn scanned_tags(dir: &Path) -> Vec<(String, Vec<String>)> {
        crate::library::scan(dir).into_iter().map(|i| (i.path.file_name().unwrap().to_string_lossy().to_string(), i.meta.tags)).collect()
    }

    #[test]
    fn the_way_follows_the_system() {
        assert_eq!(way_for(true, false), Way::Restore);
        assert_eq!(way_for(true, true), Way::Hold, "the Flatpak's Trash is its own, so files wait instead");
        assert_eq!(way_for(false, false), Way::Hold);
        assert_eq!(way_here() == Way::Restore, cfg!(any(windows, target_os = "linux")) && !in_flatpak());
    }

    #[test]
    fn delete_then_undo_gives_back_the_same_file_and_tags_both_ways() {
        for way in [Way::Restore, Way::Hold] {
            let d = temp(&format!("back-{way:?}"));
            let lib = d.join("lib");
            std::fs::create_dir_all(&lib).unwrap();
            let bin = Arc::new(FolderBin::new(d.join("bin")));
            let (p, meta) = capture(&lib, "a.png", &["work", "bug"]);
            let bytes = std::fs::read(&p).unwrap();
            let mut pending = Pending::new(UNDO_FOR, bin.clone());
            let t0 = Instant::now();
            let g = remove(&p, Some(meta.clone()), way, bin.as_ref()).unwrap();
            assert!(!p.exists() && !p.with_extension("json").exists() && !lib.join(".thumbs/a.png").exists());
            assert!(scanned_tags(&lib).is_empty(), "{way:?}: the library doesn't list it, nor the hidden folder");
            assert!(pending.add(g, t0).is_empty());
            assert!(pending.showing(t0 + Duration::from_secs(9)));
            let (back, errors) = pending.undo_now(t0 + Duration::from_secs(9));
            assert!(errors.is_empty(), "{errors:?}");
            assert_eq!(back, vec![p.clone()]);
            assert_eq!(std::fs::read(&p).unwrap(), bytes, "{way:?}: the same file");
            assert_eq!(scanned_tags(&lib), vec![("a.png".to_string(), vec!["work".to_string(), "bug".to_string()])]);
            assert_eq!(crate::library::scan(&lib)[0].meta.title, "a.png");
            assert!(!lib.join(HOLD_DIR).exists(), "the holding folder goes once empty");
            assert_eq!(bin.holds(), 0, "nothing is left in the Recycle Bin");
            assert!(!pending.showing(t0 + Duration::from_secs(9)), "the Undo goes once used");
            drop(pending);
            assert!(p.exists(), "dropping a used Undo leaves the file alone");
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    #[test]
    fn undo_after_the_message_timed_out_does_nothing() {
        for way in [Way::Restore, Way::Hold] {
            let d = temp(&format!("late-{way:?}"));
            let bin = Arc::new(FolderBin::new(d.join("bin")));
            let (p, meta) = capture(&d, "late.png", &["x"]);
            let mut pending = Pending::new(UNDO_FOR, bin.clone());
            let t0 = Instant::now();
            pending.add(remove(&p, Some(meta), way, bin.as_ref()).unwrap(), t0);
            let late = t0 + UNDO_FOR;
            assert!(!pending.showing(late));
            assert_eq!(pending.undo_now(late), (vec![], vec![]), "{way:?}");
            assert!(!p.exists());
            // The window finishes what timed out: a waiting file goes on to the Trash.
            for g in pending.take_expired(late) {
                finish(&g, bin.as_ref()).unwrap();
            }
            assert_eq!(bin.holds(), 1, "{way:?}: in the Recycle Bin either way");
            assert_eq!(bin.log.lock().unwrap()[0].0, p, "sent from where it was, so the system's Restore puts it back there");
            assert!(!d.join(HOLD_DIR).exists());
            assert_eq!(pending.count(), 0);
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    #[test]
    fn several_deletes_undo_together_and_each_starts_the_time_again() {
        for way in [Way::Restore, Way::Hold] {
            let d = temp(&format!("several-{way:?}"));
            let bin = Arc::new(FolderBin::new(d.join("bin")));
            let mut pending = Pending::new(UNDO_FOR, bin.clone());
            let t0 = Instant::now();
            let mut paths = Vec::new();
            for (i, name) in ["one.png", "two.png", "three.png"].iter().enumerate() {
                let (p, meta) = capture(&d, name, &[name]);
                pending.add(remove(&p, Some(meta), way, bin.as_ref()).unwrap(), t0 + Duration::from_secs(6 * i as u64));
                paths.push(p);
            }
            assert_eq!(pending.count(), 3);
            assert!(pending.only().is_none());
            let at = t0 + Duration::from_secs(12 + 9);
            assert!(pending.showing(at), "the last delete started the time again");
            let (back, errors) = pending.undo_now(at);
            assert!(errors.is_empty(), "{errors:?}");
            assert_eq!(back, paths, "{way:?}");
            for p in &paths {
                assert!(p.exists() && p.with_extension("json").exists());
            }
            assert_eq!(scanned_tags(&d).len(), 3);
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    #[test]
    fn the_undo_waits_while_kept_and_ctrl_z_is_its_own_until_something_else_is_done() {
        let d = temp("keep");
        let bin = Arc::new(FolderBin::new(d.join("bin")));
        let mut pending = Pending::new(UNDO_FOR, bin.clone());
        let t0 = Instant::now();
        let (a, _) = capture(&d, "a.png", &[]);
        pending.add(remove(&a, None, Way::Hold, bin.as_ref()).unwrap(), t0);
        assert!(pending.owns_shortcut(t0));
        // Hovered from just before the end until a minute later: still there, and a little after.
        let mut t = t0 + UNDO_FOR - Duration::from_millis(100);
        while t < t0 + Duration::from_secs(70) {
            pending.keep(t);
            t += Duration::from_millis(500);
        }
        let last = t - Duration::from_millis(500);
        assert!(pending.showing(last + KEEP_AFTER - Duration::from_millis(1)));
        assert!(!pending.showing(last + KEEP_AFTER));
        pending.drop_shortcut();
        assert!(!pending.owns_shortcut(last) && pending.showing(last), "the button still works");
        // Keeping it after it went doesn't bring it back.
        let later = last + Duration::from_secs(60);
        pending.keep(later);
        assert!(!pending.showing(later));
        // Closed by hand: finished at once.
        let mut p2 = Pending::new(UNDO_FOR, bin.clone());
        let (b, _) = capture(&d, "b.png", &[]);
        p2.add(remove(&b, None, Way::Hold, bin.as_ref()).unwrap(), t0);
        assert_eq!(p2.end_now().len(), 1);
        assert!(!p2.showing(t0) && !p2.owns_shortcut(t0) && p2.undo_now(t0).0.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_delete_after_the_time_ran_out_starts_a_new_undo() {
        let d = temp("new-undo");
        let bin = Arc::new(FolderBin::new(d.join("bin")));
        let mut pending = Pending::new(UNDO_FOR, bin.clone());
        let t0 = Instant::now();
        let (a, _) = capture(&d, "a.png", &[]);
        let (b, _) = capture(&d, "b.png", &[]);
        pending.add(remove(&a, None, Way::Hold, bin.as_ref()).unwrap(), t0);
        let done = pending.add(remove(&b, None, Way::Hold, bin.as_ref()).unwrap(), t0 + UNDO_FOR);
        assert_eq!(done.iter().map(|g| g.path.clone()).collect::<Vec<_>>(), vec![a.clone()], "the old one is left to finish");
        assert_eq!(pending.only().map(|g| g.name()), Some("b.png".into()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn closing_sends_what_waits_to_the_trash_and_a_crash_is_cleaned_up_next_time() {
        let d = temp("close");
        let bin = Arc::new(FolderBin::new(d.join("bin")));
        let (a, _) = capture(&d, "a.png", &[]);
        let mut pending = Pending::new(UNDO_FOR, bin.clone());
        pending.add(remove(&a, None, Way::Hold, bin.as_ref()).unwrap(), Instant::now());
        drop(pending);
        assert_eq!(bin.holds(), 1, "closing finishes the delete");
        assert!(!d.join(HOLD_DIR).exists());

        // A crash: the file waits, and the list says where.
        let (b, _) = capture(&d, "b.png", &[]);
        let g = remove(&b, None, Way::Hold, bin.as_ref()).unwrap();
        // Deleted again under the same name while the first still waits.
        let (b2, _) = capture(&d, "b.png", &[]);
        let g2 = remove(&b2, None, Way::Hold, bin.as_ref()).unwrap();
        assert_ne!(g.held, g2.held);
        std::mem::forget(g);
        assert!(std::fs::read_to_string(d.join("held-deletes.txt")).unwrap().contains(HOLD_DIR));
        finish_leftovers(bin.as_ref());
        assert_eq!(bin.holds(), 3);
        assert!(bin.log.lock().unwrap().iter().skip(1).all(|(from, _)| *from == b), "both sent from where they were");
        assert!(!d.join(HOLD_DIR).exists());
        assert!(!d.join("held-deletes.txt").exists(), "the list goes once nothing waits");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The computer's own Recycle Bin / Trash, there and back (it touches the real one, so it is
    /// run by hand: `cargo test recycle -- --ignored`).
    #[test]
    #[ignore]
    fn the_systems_recycle_bin_gives_the_file_back() {
        let d = temp("system");
        let (p, meta) = capture(&d, "system.png", &["kept"]);
        let bytes = std::fs::read(&p).unwrap();
        let mut pending = Pending::new(UNDO_FOR, Arc::new(SystemBin));
        let t0 = Instant::now();
        pending.add(remove(&p, Some(meta), way_here(), &SystemBin).unwrap(), t0);
        assert!(!p.exists());
        let (back, errors) = pending.undo_now(t0);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(back, vec![p.clone()]);
        assert_eq!(std::fs::read(&p).unwrap(), bytes);
        assert_eq!(scanned_tags(&d), vec![("system.png".to_string(), vec!["kept".to_string()])]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn undo_doesnt_overwrite_a_new_file_and_a_gone_file_isnt_deleted() {
        let d = temp("clash");
        let bin = Arc::new(FolderBin::new(d.join("bin")));
        assert!(remove(&d.join("missing.png"), None, Way::Restore, bin.as_ref()).is_err());
        let (a, _) = capture(&d, "a.png", &[]);
        let mut pending = Pending::new(UNDO_FOR, bin.clone());
        let t0 = Instant::now();
        pending.add(remove(&a, None, Way::Restore, bin.as_ref()).unwrap(), t0);
        std::fs::write(&a, b"new").unwrap();
        let (back, errors) = pending.undo_now(t0);
        assert!(back.is_empty() && errors.len() == 1, "{errors:?}");
        assert_eq!(std::fs::read(&a).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&d);
    }
}
