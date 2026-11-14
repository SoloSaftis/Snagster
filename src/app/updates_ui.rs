//! Checking GitHub for a new Snagster (by hand from Help, or once a day when it starts) and
//! installing it: download, swap in place, start the new version.

use super::*;
use crate::i18n::{t, tf};
use crate::update::Release;
use std::sync::atomic::AtomicU32;

#[derive(Default)]
pub(crate) struct UpdateUi {
    check_rx: Option<Receiver<Result<Release, String>>>,
    /// The person asked (so "you're up to date" is worth saying).
    asked: bool,
    found: Option<Release>,
    pub dialog: bool,
    install_rx: Option<Receiver<Result<PathBuf, String>>>,
    progress: Option<Arc<AtomicU32>>,
    pub message: Option<String>,
}

impl UpdateUi {
    pub fn checking(&self) -> bool {
        self.check_rx.is_some()
    }

    pub fn available(&self) -> Option<&Release> {
        self.found.as_ref().filter(|r| crate::update::is_newer(&r.version, env!("CARGO_PKG_VERSION")))
    }

    /// Ask GitHub in the background.
    pub fn check(&mut self, ctx: &egui::Context, asked: bool) {
        if self.check_rx.is_some() {
            return;
        }
        self.asked = asked;
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new().name("snagster-update-check".into()).spawn(move || {
            let _ = tx.send(crate::update::latest());
            ctx.request_repaint();
        });
        self.check_rx = Some(rx);
    }
}

impl SnagApp {
    /// Once a day at start (unless turned off), and the answers to checks.
    pub(crate) fn poll_updates(&mut self, ctx: &egui::Context) {
        let now = chrono::Utc::now().timestamp();
        if self.persist && self.settings.update_check && now - self.settings.update_checked_at > 24 * 3600 && !UI_TEST.load(Ordering::Relaxed) {
            self.settings.update_checked_at = now;
            self.x.update.check(ctx, false);
        }
        if let Some(rx) = &self.x.update.check_rx
            && let Ok(r) = rx.try_recv()
        {
            self.x.update.check_rx = None;
            let asked = self.x.update.asked;
            match r {
                Ok(rel) => {
                    let newer = crate::update::is_newer(&rel.version, env!("CARGO_PKG_VERSION"));
                    let skipped = !asked && self.settings.update_skip == rel.version;
                    if newer && !skipped {
                        self.status = tf("Snagster {v} is out.", &[("v", &rel.version)]);
                        self.x.update.dialog = true;
                    } else if asked {
                        self.status = tf("You have the newest Snagster ({v}).", &[("v", env!("CARGO_PKG_VERSION"))]);
                    }
                    self.x.update.found = Some(rel);
                }
                Err(e) => {
                    if asked {
                        self.status = tf("Couldn't check for updates: {e}", &[("e", &e)]);
                    }
                }
            }
        }
        if let Some(rx) = &self.x.update.install_rx
            && let Ok(r) = rx.try_recv()
        {
            self.x.update.install_rx = None;
            self.x.update.progress = None;
            match r {
                Ok(program) => match crate::update::restart(&program) {
                    Ok(()) => {
                        self.settings.save();
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    Err(e) => self.x.update.message = Some(tf("Installed. Start Snagster again to use it ({e}).", &[("e", &e)])),
                },
                Err(e) => self.x.update.message = Some(e),
            }
        }
        if self.x.update.install_rx.is_some() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    pub(crate) fn update_window(&mut self, ctx: &egui::Context) {
        if !self.x.update.dialog {
            return;
        }
        let Some(rel) = self.x.update.available().cloned() else {
            self.x.update.dialog = false;
            return;
        };
        let mut open = true;
        egui::Window::new(t("A new Snagster"))
            .id(egui::Id::unique("update-window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.label(RichText::new(tf("Snagster {new} is out. You have {old}.", &[("new", &rel.version), ("old", env!("CARGO_PKG_VERSION"))])).size(15.0).strong());
                ui.add_space(6.0);
                if !rel.notes.trim().is_empty() {
                    egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                        // The first part of the release notes, without links and markup clutter.
                        let notes: String = rel.notes.lines().filter(|l| !l.contains("](http") && !l.starts_with('|')).take(30).collect::<Vec<_>>().join("\n");
                        ui.label(RichText::new(notes).color(TEXT_DIM));
                    });
                }
                ui.add_space(8.0);
                if let Some(p) = &self.x.update.progress {
                    let f = f32::from_bits(p.load(Ordering::Relaxed));
                    ui.add(egui::ProgressBar::new(f).show_percentage().text(t("Downloading...")));
                } else if let Some(m) = &self.x.update.message {
                    ui.label(RichText::new(m).color(Color32::from_rgb(240, 140, 120)));
                }
                ui.horizontal(|ui| {
                    let busy = self.x.update.install_rx.is_some();
                    let can = rel.asset.is_some();
                    if ui
                        .add_enabled(!busy && can, egui::Button::new(RichText::new(t("Update now")).strong().color(Color32::WHITE)).fill(ACCENT))
                        .on_hover_text(t("Download it, put it in place of this version and start it"))
                        .clicked()
                    {
                        let progress = Arc::new(AtomicU32::new(0));
                        let (tx, rx) = channel();
                        let p2 = progress.clone();
                        let c2 = ctx.clone();
                        let rel2 = rel.clone();
                        let _ = std::thread::Builder::new().name("snagster-update".into()).spawn(move || {
                            let r = crate::update::install(&rel2, &|f| {
                                p2.store(f.to_bits(), Ordering::Relaxed);
                                c2.request_repaint();
                            });
                            let _ = tx.send(r);
                            c2.request_repaint();
                        });
                        self.x.update.progress = Some(progress);
                        self.x.update.install_rx = Some(rx);
                        self.x.update.message = None;
                    }
                    if ui.add_enabled(!busy, egui::Button::new(t("Release page"))).on_hover_text(t("See what changed on GitHub")).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(&rel.page));
                    }
                    if ui.add_enabled(!busy, egui::Button::new(t("Skip this version"))).clicked() {
                        self.settings.update_skip = rel.version.clone();
                        self.x.update.dialog = false;
                    }
                    if ui.add_enabled(!busy, egui::Button::new(t("Later"))).clicked() {
                        self.x.update.dialog = false;
                    }
                });
                if !rel.asset.is_some() {
                    ui.label(RichText::new(t("This release has no download for this kind of computer; get it from the release page.")).color(TEXT_DIM));
                }
            });
        if !open {
            self.x.update.dialog = false;
        }
    }
}
