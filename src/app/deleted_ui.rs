//! The Undo after a delete: "Moved … to the Recycle Bin. Undo" in the editor's status area and
//! at the foot of the Library window, and Ctrl+Z / ⌘Z until something else is done
//! (`crate::recycle` does the files). The Windows, GNOME/KDE and macOS reviews asked for: no
//! "Are you sure?", an Undo that waits while the pointer or keyboard is on it (or Snagster is in
//! the background), that a screen reader announces, that can be closed, and one Undo action for
//! the key, the Edit menu and the ribbon.

use super::*;
use crate::i18n::{t, tf};
use crate::recycle::{self, Gone};

/// What the pictures' histories look like; when it changes, something else was done and
/// Ctrl+Z / ⌘Z no longer undoes the delete.
pub(crate) fn edits_mark(entries: &[Entry]) -> (usize, usize, usize, u64) {
    let undo = entries.iter().map(|e| e.undo.len()).sum();
    let redo = entries.iter().map(|e| e.redo.len()).sum();
    // A history at its cap keeps its length: the newest step tells it apart.
    let last = entries.iter().map(|e| e.undo.last().map_or(0, |d| d.shapes.len() as u64 + 1)).sum();
    (entries.len(), undo, redo, last)
}

/// A long file name, shortened in the middle (the end and extension stay readable).
pub(crate) fn short_name(name: &str, max: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max || max < 5 {
        return name.to_string();
    }
    let tail = (max - 1) / 2;
    let head = max - 1 - tail;
    format!("{}…{}", chars[..head].iter().collect::<String>(), chars[chars.len() - tail..].iter().collect::<String>())
}

impl SnagApp {
    /// A delete went through (Library or File menu): the Undo shows, and takes in this one too
    /// when it was showing already.
    pub(crate) fn deleted(&mut self, g: Gone) {
        let done = self.deleted.add(g, Instant::now());
        self.finish_deletes(done);
        self.deleted_edits = edits_mark(&self.entries);
        self.status = self.deleted_text();
    }

    /// What the Undo says: the file's name, or how many.
    pub(crate) fn deleted_text(&self) -> String {
        match self.deleted.only() {
            Some(g) => tf(crate::platform::trash_text("Moved {path} to the Recycle Bin.", "Moved {path} to the Trash."), &[("path", &short_name(&g.name(), 40))]),
            None => tf(
                crate::platform::trash_text("Moved {count} captures to the Recycle Bin.", "Moved {count} captures to the Trash."),
                &[("count", &self.deleted.count().to_string())],
            ),
        }
    }

    /// Whether the Undo shows now.
    pub(crate) fn delete_undo_showing(&self) -> bool {
        self.deleted.showing(Instant::now())
    }

    /// Undo for Ctrl+Z / ⌘Z, the Edit menu and the ribbon: the delete right after it, else the
    /// picture's last change.
    pub(crate) fn undo_any(&mut self) {
        if self.deleted.owns_shortcut(Instant::now()) {
            self.undo_delete();
        } else {
            self.undo();
        }
    }

    /// Whether `undo_any` has something to undo (`can_undo`: the picture's history has).
    pub(crate) fn can_undo_any(&self, can_undo: bool) -> bool {
        can_undo || self.deleted.owns_shortcut(Instant::now())
    }

    /// Every frame: once the Undo's time is up, what waited goes on to the Trash; it waits while
    /// Snagster is in the background; Ctrl+Z / ⌘Z goes back to the picture once it changes.
    pub(crate) fn tick_deleted(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        if !self.deleted.showing(now) {
            let done = self.deleted.take_expired(now);
            self.finish_deletes(done);
            return;
        }
        if edits_mark(&self.entries) != self.deleted_edits {
            self.deleted.drop_shortcut();
        }
        let away = !ctx.input(|i| i.focused) && !self.lib_focused;
        if away {
            self.deleted.keep(now);
        }
        if let Some(left) = self.deleted.left(now) {
            ctx.request_repaint_after(left.min(Duration::from_secs(1)));
        }
    }

    fn finish_deletes(&mut self, done: Vec<Gone>) {
        if done.is_empty() {
            return;
        }
        let bin = self.deleted.bin();
        self.jobs.queue(move || {
            let failed: Vec<String> = done.iter().filter_map(|g| recycle::finish(g, bin.as_ref()).err().map(|e| format!("{}: {e}", g.name()))).collect();
            Box::new(move |app: &mut SnagApp, _: &egui::Context| {
                // Said, so nobody thinks it is in the Trash when it isn't.
                if !failed.is_empty() {
                    app.status = tf(
                        crate::platform::trash_text("Couldn't move it to the Recycle Bin: {error}", "Couldn't move it to the Trash: {error}"),
                        &[("error", &failed.join("; "))],
                    );
                }
            })
        });
    }

    /// Undo (the button, Ctrl+Z / ⌘Z, or app_command undo_delete): everything deleted while the
    /// Undo showed comes back where it was, with its tags. Restoring from the Recycle Bin can
    /// take a moment, so it runs on the job queue.
    pub(crate) fn undo_delete(&mut self) {
        let gone = self.deleted.take_for_undo(Instant::now());
        if gone.is_empty() {
            return;
        }
        let reopen: Vec<PathBuf> = gone.iter().filter(|g| g.reopen).map(|g| g.path.clone()).collect();
        let bin = self.deleted.bin();
        self.jobs.queue(move || {
            let (back, errors) = recycle::undo_all(gone, bin.as_ref());
            Box::new(move |app: &mut SnagApp, _: &egui::Context| app.undid_delete(back, errors, reopen))
        });
    }

    /// Closed by hand: the Undo goes, and what waited goes on to the Trash.
    pub(crate) fn close_delete_undo(&mut self) {
        let done = self.deleted.end_now();
        self.finish_deletes(done);
    }

    fn undid_delete(&mut self, back: Vec<PathBuf>, errors: Vec<(String, String)>, reopen: Vec<PathBuf>) {
        // The Library reads its folder again (with the tags), and a file taken from Recent
        // opens again.
        self.lib_loaded = false;
        for p in back.iter().filter(|p| reopen.contains(p)) {
            self.open_path(p);
        }
        self.status = undo_message(&back, &errors);
    }

    /// The Undo itself: what was deleted, Undo and Close. While the pointer or the keyboard is
    /// on it, it doesn't go.
    pub(crate) fn delete_undo_bar(&mut self, ui: &mut egui::Ui, name_room: usize) {
        if !self.delete_undo_showing() {
            return;
        }
        let text = match self.deleted.only() {
            Some(g) => tf(crate::platform::trash_text("Moved {path} to the Recycle Bin.", "Moved {path} to the Trash."), &[("path", &short_name(&g.name(), name_room))]),
            None => self.deleted_text(),
        };
        let (mut undo, mut close) = (false, false);
        let bar = egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(crate::look::ph::INFO).family(egui::FontFamily::Name("icons".into())).size(16.0).color(accent()));
                    let label = ui.add(egui::Label::new(RichText::new(text).size(12.5)).wrap());
                    // Screen readers say it when it appears (Narrator, Orca, VoiceOver).
                    ui.ctx().accesskit_node_builder(label.id, |n| n.set_live(egui::accesskit::Live::Polite));
                    let b = ui.button(t("Undo")).on_hover_text(crate::platform::keys("Ctrl+Z"));
                    undo = b.clicked();
                    let c = crate::look::icon_button(ui, crate::look::ph::X, t("Close"));
                    close = c.clicked();
                    b.has_focus() || c.has_focus()
                })
                .inner
            });
        let focused = bar.inner;
        if focused || ui.rect_contains_pointer(bar.response.rect) {
            self.deleted.keep(Instant::now());
        }
        if undo {
            self.undo_delete();
        } else if close {
            self.close_delete_undo();
        }
    }
}

/// What the status line says after an Undo: what came back, and what didn't and why.
pub(crate) fn undo_message(back: &[PathBuf], errors: &[(String, String)]) -> String {
    let name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let failed = errors.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ");
    let why = errors.iter().map(|(_, e)| e.as_str()).collect::<Vec<_>>();
    let mut why = why.clone();
    why.dedup();
    let why = why.join("; ");
    match (back, errors.is_empty()) {
        ([p], true) => tf("Restored {name}.", &[("name", &name(p))]),
        (_, true) => tf("Restored {count} captures.", &[("count", &back.len().to_string())]),
        ([], false) => tf("Couldn't restore {name}: {error}", &[("name", &failed), ("error", &why)]),
        (_, false) => tf(
            "Restored {count} of {total} captures. Couldn't restore {name}: {error}",
            &[("count", &back.len().to_string()), ("total", &(back.len() + errors.len()).to_string()), ("name", &failed), ("error", &why)],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recycle::{Pending, UNDO_FOR, Way, tests::FolderBin};

    fn app_with_bin(dir: &std::path::Path) -> (SnagApp, Arc<FolderBin>) {
        let mut app = SnagApp::blank(Settings::default(), false, None);
        let bin = Arc::new(FolderBin::new(dir.join("bin")));
        app.deleted = Pending::new(UNDO_FOR, bin.clone());
        (app, bin)
    }

    fn settle(app: &mut SnagApp) {
        let ctx = egui::Context::default();
        assert!(app.jobs.wait_idle(Duration::from_secs(20)), "the jobs never finished");
        for apply in app.jobs.finished(usize::MAX) {
            apply(app, &ctx);
        }
    }

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("snagster-deleted-ui-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_undo_says_what_went_and_brings_it_back() {
        let dir = temp("back");
        let (mut app, bin) = app_with_bin(&dir);
        assert!(!app.delete_undo_showing());
        app.undo_delete();
        assert_eq!(app.jobs.busy(), 0, "nothing to undo does nothing");
        let mut paths = Vec::new();
        for name in ["a.png", "b.png"] {
            let p = dir.join(name);
            std::fs::write(&p, name).unwrap();
            app.deleted(recycle::remove(&p, None, Way::Restore, bin.as_ref()).unwrap());
            paths.push(p);
        }
        assert!(app.delete_undo_showing());
        assert!(app.deleted_text().contains('2'), "{}", app.deleted_text());
        assert_eq!(app.status, app.deleted_text(), "the status line keeps it after the Undo goes");
        app.lib_loaded = true;
        assert!(app.can_undo_any(false), "the Edit menu and ribbon Undo are on");
        app.undo_any();
        assert!(!app.delete_undo_showing(), "pressed once, it goes");
        settle(&mut app);
        assert!(paths.iter().all(|p| p.exists()));
        assert_eq!(app.status, "Restored 2 captures.");
        assert!(!app.lib_loaded, "the Library reads its folder again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ctrl_z_goes_back_to_the_picture_once_it_changes_and_close_finishes_the_delete() {
        let dir = temp("edit");
        let (mut app, bin) = app_with_bin(&dir);
        app.add_entry("Pic".into(), Doc::new(RgbaImage::new(8, 8)));
        let p = dir.join("a.png");
        std::fs::write(&p, "a").unwrap();
        app.deleted(recycle::remove(&p, None, Way::Hold, bin.as_ref()).unwrap());
        let ctx = egui::Context::default();
        app.tick_deleted(&ctx);
        assert!(app.deleted.owns_shortcut(Instant::now()));
        // A mark is drawn: Ctrl+Z now undoes the mark, and only the button the delete.
        let i = app.current.unwrap();
        app.entries[i].push_undo();
        app.tick_deleted(&ctx);
        assert!(!app.deleted.owns_shortcut(Instant::now()) && app.delete_undo_showing());
        assert!(!app.can_undo_any(false));
        app.undo_any();
        assert!(app.delete_undo_showing(), "Ctrl+Z undid the mark, not the delete");
        app.close_delete_undo();
        assert!(!app.delete_undo_showing());
        settle(&mut app);
        assert_eq!(bin.holds(), 1, "closed: it went on to the Recycle Bin");
        assert!(!dir.join(recycle::HOLD_DIR).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_undo_message_says_what_came_back_and_what_didnt() {
        let p = |n: &str| PathBuf::from(n);
        let e = |n: &str| (n.to_string(), "a new file has taken its name".to_string());
        assert_eq!(undo_message(&[p("a.png")], &[]), "Restored a.png.");
        assert_eq!(undo_message(&[p("a.png"), p("b.png")], &[]), "Restored 2 captures.");
        assert_eq!(undo_message(&[], &[e("a.png")]), "Couldn't restore a.png: a new file has taken its name");
        assert_eq!(
            undo_message(&[p("a.png"), p("b.png")], &[e("c.png"), e("d.png")]),
            "Restored 2 of 4 captures. Couldn't restore c.png, d.png: a new file has taken its name"
        );
    }

    #[test]
    fn long_names_are_shortened_in_the_middle() {
        assert_eq!(short_name("short.png", 20), "short.png");
        let s = short_name("2026-11-19_10-00-00 a very long title indeed.png", 20);
        assert_eq!(s.chars().count(), 20);
        assert!(s.starts_with("2026-11-19") && s.ends_with("ed.png") && s.contains('…'), "{s}");
    }
}
