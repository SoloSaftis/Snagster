//! The Undo after a delete: "Moved … to the Recycle Bin. Undo" in the editor's status area and
//! in the Library window, and Ctrl+Z / ⌘Z while it shows (`crate::recycle` does the files).

use super::*;
use crate::i18n::{t, tf};
use crate::recycle::{self, Gone};

impl SnagApp {
    /// A delete went through (Library or File menu): the Undo shows, and takes in this one too
    /// when it was showing already.
    pub(crate) fn deleted(&mut self, g: Gone) {
        let done = self.deleted.add(g, Instant::now());
        self.finish_deletes(done);
        self.status = self.deleted_text();
    }

    /// What the Undo says: the file's name, or how many.
    pub(crate) fn deleted_text(&self) -> String {
        match self.deleted.only() {
            Some(g) => tf(crate::platform::trash_text("Moved {path} to the Recycle Bin.", "Moved {path} to the Trash."), &[("path", &g.name())]),
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

    /// Every frame: once the Undo's time is up, what waited goes on to the Trash; while it
    /// shows, a frame is drawn when it goes.
    pub(crate) fn tick_deleted(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let done = self.deleted.take_expired(now);
        self.finish_deletes(done);
        if let Some(left) = self.deleted.left(now) {
            ctx.request_repaint_after(left);
        }
    }

    fn finish_deletes(&mut self, done: Vec<Gone>) {
        if done.is_empty() {
            return;
        }
        let bin = self.deleted.bin();
        self.jobs.queue(move || {
            for g in &done {
                let _ = recycle::finish(g, bin.as_ref());
            }
            Box::new(|_: &mut SnagApp, _: &egui::Context| {})
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

    fn undid_delete(&mut self, back: Vec<PathBuf>, errors: Vec<String>, reopen: Vec<PathBuf>) {
        // The Library reads its folder again (with the tags), and a file taken from Recent
        // opens again.
        self.lib_loaded = false;
        for p in back.iter().filter(|p| reopen.contains(p)) {
            self.open_path(p);
        }
        self.status = if !errors.is_empty() {
            tf("Couldn't bring it back: {error}", &[("error", &errors.join("; "))])
        } else if let [p] = back.as_slice() {
            tf("Brought back {path}.", &[("path", &p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())])
        } else {
            tf("Brought back {count} captures.", &[("count", &back.len().to_string())])
        };
    }

    /// The Undo itself: what was deleted and the button. Gives true when it was pressed.
    pub(crate) fn delete_undo_bar(&mut self, ui: &mut egui::Ui) -> bool {
        if !self.delete_undo_showing() {
            return false;
        }
        let text = self.deleted_text();
        let mut pressed = false;
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::Label::new(RichText::new(text).size(12.5)).wrap());
            let undo = ui.button(t("Undo")).on_hover_text(if cfg!(target_os = "macos") { "⌘Z" } else { "Ctrl+Z" });
            pressed = undo.clicked();
        });
        if pressed {
            self.undo_delete();
        }
        pressed
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

    fn wait_for_jobs(app: &mut SnagApp) {
        let ctx = egui::Context::default();
        assert!(app.jobs.wait_idle(Duration::from_secs(20)), "the jobs never finished");
        for apply in app.jobs.finished(usize::MAX) {
            apply(app, &ctx);
        }
    }

    #[test]
    fn the_undo_says_what_went_and_brings_it_back() {
        let dir = std::env::temp_dir().join(format!("snagster-deleted-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
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
        app.undo_delete();
        assert!(!app.delete_undo_showing(), "pressed once, it goes");
        wait_for_jobs(&mut app);
        assert!(paths.iter().all(|p| p.exists()));
        assert!(app.status.contains('2'), "{}", app.status);
        assert!(!app.lib_loaded, "the Library reads its folder again");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
