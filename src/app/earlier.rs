//! "Earlier" in the Recent strip: the pictures and videos already on this computer (the
//! library, the save folder and the videos folder), newest first, not only this session's.
//! The folders are read and the thumbnails made on worker threads; the window only shows what
//! has arrived (rule: the window never waits).

use super::*;
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::SystemTime;

/// At most this many earlier items in the strip.
pub(crate) const MAX_EARLIER: usize = 60;
/// Never look at more files than this while looking for them (a cloud folder may hold thousands).
const MAX_FILES: usize = 20_000;
/// The folders are read again this often.
const EVERY: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EarlierItem {
    pub path: PathBuf,
    pub video: bool,
    pub modified: SystemTime,
}

impl EarlierItem {
    pub fn name(&self) -> String {
        self.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
    }
}

/// What kind of file `p` is for the strip: Some(false) a picture, Some(true) a video, None
/// neither.
pub(crate) fn kind_of(p: &Path) -> Option<bool> {
    let ext = p.extension()?.to_string_lossy().to_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "bmp" | "webp" => Some(false),
        "mp4" | "wmv" | "flv" | "mov" | "mkv" | "gif" => Some(true),
        _ => None,
    }
}

/// The newest pictures and videos in `folders` (and their folders, four deep; hidden ones and
/// thumbnail caches skipped), at most `limit`, each file once.
pub(crate) fn scan(folders: &[PathBuf], limit: usize) -> Vec<EarlierItem> {
    let mut out: Vec<EarlierItem> = Vec::new();
    let mut seen_dirs: Vec<PathBuf> = Vec::new();
    let mut looked = 0;
    for root in folders {
        let mut stack = vec![(root.clone(), 0)];
        while let Some((dir, depth)) = stack.pop() {
            let canon = std::fs::canonicalize(&dir).unwrap_or(dir.clone());
            if seen_dirs.contains(&canon) {
                continue;
            }
            seen_dirs.push(canon);
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                looked += 1;
                if looked > MAX_FILES {
                    break;
                }
                let p = e.path();
                let hidden = p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
                if hidden {
                    continue;
                }
                let Ok(meta) = e.metadata() else { continue };
                if meta.is_dir() {
                    if depth < 4 {
                        stack.push((p, depth + 1));
                    }
                    continue;
                }
                if let Some(video) = kind_of(&p)
                    && !out.iter().any(|x| x.path == p)
                {
                    out.push(EarlierItem { path: p, video, modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH) });
                }
            }
        }
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out.truncate(limit);
    out
}

/// The strip's earlier items and their thumbnails.
#[derive(Default)]
pub(crate) struct Earlier {
    pub items: Vec<EarlierItem>,
    rx: Option<Receiver<Vec<EarlierItem>>>,
    fetched: Option<Instant>,
    pub thumbs: HashMap<PathBuf, TextureHandle>,
    thumb_rx: Option<Receiver<(PathBuf, RgbaImage)>>,
}

impl Earlier {
    /// Read the folders again soon (after a capture was saved, a folder was changed).
    pub fn expire(&mut self) {
        self.fetched = None;
    }

    /// Every frame: start reading the folders when due, take a finished list, turn a few
    /// finished thumbnails into textures.
    pub fn poll(&mut self, ctx: &egui::Context, folders: Vec<PathBuf>) {
        if self.rx.is_none() && self.fetched.is_none_or(|t| t.elapsed() >= EVERY) {
            self.fetched = Some(Instant::now());
            let (tx, rx) = channel();
            self.rx = Some(rx);
            let c = ctx.clone();
            let _ = std::thread::Builder::new().name("snagster-earlier".into()).spawn(move || {
                let _ = tx.send(scan(&folders, MAX_EARLIER));
                c.request_repaint();
            });
        }
        if let Some(rx) = &self.rx {
            match rx.try_recv() {
                Ok(items) => {
                    self.items = items;
                    self.rx = None;
                    self.start_thumbs(ctx);
                }
                Err(TryRecvError::Disconnected) => self.rx = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(rx) = &self.thumb_rx {
            for _ in 0..6 {
                let Ok((p, img)) = rx.try_recv() else { break };
                let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
                self.thumbs.insert(p, ctx.load_texture("earlier-thumb", ci, TextureOptions::LINEAR));
            }
        }
        // Thumbnails of files no longer listed go.
        let listed: Vec<&PathBuf> = self.items.iter().map(|i| &i.path).collect();
        self.thumbs.retain(|p, _| listed.contains(&p));
    }

    fn start_thumbs(&mut self, ctx: &egui::Context) {
        let paths: Vec<PathBuf> = self.items.iter().filter(|i| !i.video && !self.thumbs.contains_key(&i.path)).map(|i| i.path.clone()).collect();
        if paths.is_empty() {
            return;
        }
        let (tx, rx) = channel();
        self.thumb_rx = Some(rx);
        let c = ctx.clone();
        let _ = std::thread::Builder::new().name("snagster-earlier-thumbs".into()).spawn(move || {
            for p in paths {
                // Made in memory: nothing is written into the person's folders.
                let Some(img) = image::open(&p).ok().map(|i| i.thumbnail(232, 116).to_rgba8()) else { continue };
                if tx.send((p, img)).is_err() {
                    break;
                }
                c.request_repaint();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_and_videos_are_told_apart_by_their_ending() {
        assert_eq!(kind_of(Path::new("a/shot.PNG")), Some(false));
        assert_eq!(kind_of(Path::new("shot.jpeg")), Some(false));
        assert_eq!(kind_of(Path::new("clip.mp4")), Some(true));
        assert_eq!(kind_of(Path::new("rec.gif")), Some(true));
        assert_eq!(kind_of(Path::new("shot.json")), None);
        assert_eq!(kind_of(Path::new("noending")), None);
    }

    /// The newest first, each file once even when two folders overlap, hidden folders and
    /// other files left out, at most `limit`.
    #[test]
    fn the_folders_give_the_newest_pictures_and_videos() {
        let root = std::env::temp_dir().join(format!("snagster-earlier-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (pics, vids) = (root.join("Pictures"), root.join("Videos"));
        std::fs::create_dir_all(pics.join("2026-11").join(".thumbs")).unwrap();
        std::fs::create_dir_all(&vids).unwrap();
        let write = |p: PathBuf, age_s: u64| {
            std::fs::write(&p, b"x").unwrap();
            let t = SystemTime::now() - Duration::from_secs(age_s);
            std::fs::File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
        };
        write(pics.join("old.png"), 500);
        write(pics.join("2026-11").join("new.png"), 10);
        write(pics.join("2026-11").join(".thumbs").join("new.png"), 1);
        write(pics.join("2026-11").join("new.json"), 1);
        write(vids.join("clip.mp4"), 100);
        let got = scan(&[pics.clone(), vids.clone(), pics.clone()], 10);
        let names: Vec<String> = got.iter().map(|i| i.name()).collect();
        assert_eq!(names, vec!["new", "clip", "old"]);
        assert_eq!(got.iter().map(|i| i.video).collect::<Vec<_>>(), vec![false, true, false]);
        assert_eq!(scan(&[pics, vids], 2).len(), 2);
        assert!(scan(&[root.join("missing")], 10).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
