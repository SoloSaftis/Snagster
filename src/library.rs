//! The capture library: every capture saved with when it was taken, which program it came
//! from and the person's tags, searchable and sortable. Point it at a OneDrive, Dropbox or
//! Google Drive folder and it syncs across computers.

use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Meta {
    pub title: String,
    pub app: String,
    pub kind: String,
    pub created: String,
    pub tags: Vec<String>,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub path: PathBuf,
    pub meta: Meta,
}

impl Item {
    pub fn meta_path(&self) -> PathBuf {
        self.path.with_extension("json")
    }

    pub fn matches(&self, q: &str) -> bool {
        if q.trim().is_empty() {
            return true;
        }
        let hay = format!(
            "{} {} {} {} {}",
            self.meta.title,
            self.meta.app,
            self.meta.kind,
            self.meta.created,
            self.meta.tags.join(" ")
        )
        .to_lowercase();
        q.to_lowercase().split_whitespace().all(|w| hay.contains(w))
    }

    pub fn save_meta(&self) -> Result<(), String> {
        let s = serde_json::to_string_pretty(&self.meta).map_err(|e| e.to_string())?;
        std::fs::write(self.meta_path(), s).map_err(|e| e.to_string())
    }
}

/// Add a capture to the library.
pub fn add(folder: &Path, img: &RgbaImage, meta: Meta) -> Result<PathBuf, String> {
    add_item(folder, img, meta).map(|it| it.path)
}

/// Add a capture to the library and give back its entry (for the open library window).
pub fn add_item(folder: &Path, img: &RgbaImage, mut meta: Meta) -> Result<Item, String> {
    let now = chrono::Local::now();
    let day = folder.join(now.format("%Y-%m").to_string());
    std::fs::create_dir_all(&day).map_err(|e| e.to_string())?;
    let base = now.format("%Y-%m-%d_%H-%M-%S").to_string();
    let path = crate::settings::unique_path(&day, &base, "png");
    img.save(&path).map_err(|e| e.to_string())?;
    meta.created = now.to_rfc3339();
    meta.width = img.width();
    meta.height = img.height();
    let item = Item { path, meta };
    item.save_meta()?;
    Ok(item)
}

/// Put `item` where it belongs in a newest-first list (a new capture goes at the top).
pub fn insert_newest(items: &mut Vec<Item>, item: Item) {
    if items.iter().any(|i| i.path == item.path) {
        return;
    }
    let at = items.iter().position(|i| i.meta.created < item.meta.created).unwrap_or(items.len());
    items.insert(at, item);
}

/// A finished scan with the captures added while it ran (that it may have missed).
pub fn merge_scan(mut scanned: Vec<Item>, added: Vec<Item>) -> Vec<Item> {
    for it in added {
        insert_newest(&mut scanned, it);
    }
    scanned
}

/// Everything in the library, newest first.
pub fn scan(folder: &Path) -> Vec<Item> {
    let mut out = Vec::new();
    let mut stack = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
                    stack.push(p);
                }
                continue;
            }
            if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("png")) {
                let meta = std::fs::read_to_string(p.with_extension("json"))
                    .ok()
                    .and_then(|s| serde_json::from_str::<Meta>(&s).ok())
                    .unwrap_or_else(|| Meta {
                        title: p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                        created: std::fs::metadata(&p)
                            .and_then(|m| m.modified())
                            .map(|t| chrono::DateTime::<chrono::Local>::from(t).to_rfc3339())
                            .unwrap_or_default(),
                        ..Default::default()
                    });
                out.push(Item { path: p, meta });
            }
        }
    }
    out.sort_by(|a, b| b.meta.created.cmp(&a.meta.created));
    out
}

/// A small preview, cached next to the picture in a hidden folder.
pub fn thumbnail(path: &Path, size: u32) -> Option<RgbaImage> {
    let dir = path.parent()?.join(".thumbs");
    let thumb = dir.join(path.file_name()?);
    if let Ok(t) = image::open(&thumb) {
        return Some(t.to_rgba8());
    }
    let img = image::DynamicImage::ImageRgba8(crate::export::open_picture(path).ok()?).thumbnail(size, size).to_rgba8();
    let _ = std::fs::create_dir_all(&dir);
    let _ = img.save(&thumb);
    Some(img)
}

/// The preview of a capture that is still in memory (no need to read the file back),
/// cached the same way as `thumbnail`.
pub fn thumbnail_of(path: &Path, img: &RgbaImage, size: u32) -> Option<RgbaImage> {
    let dir = path.parent()?.join(".thumbs");
    // Fit inside size x size, keeping the shape (never larger than the picture).
    let s = (size as f32 / img.width().max(1) as f32).min(size as f32 / img.height().max(1) as f32).min(1.0);
    let (w, h) = (((img.width() as f32 * s).round() as u32).max(1), ((img.height() as f32 * s).round() as u32).max(1));
    let thumb = image::imageops::thumbnail(img, w, h);
    let _ = std::fs::create_dir_all(&dir);
    let _ = thumb.save(dir.join(path.file_name()?));
    Some(thumb)
}

/// Move a capture to the Recycle Bin / Trash (so it can be brought back) and drop its tags
/// and thumbnail. Every system's guidelines ask for this instead of deleting for good; the
/// answer is what an Undo needs to bring it back with its tags (`recycle::Pending`).
pub fn delete(item: &Item) -> Result<crate::recycle::Gone, String> {
    crate::recycle::remove(&item.path, Some(item.meta.clone()), crate::recycle::way_here(), &crate::recycle::SystemBin)
}

/// Fuzz target (`src/fuzz.rs`): a library folder whose tag files were edited by hand, synced
/// half-way or overwritten with junk.
#[cfg(test)]
pub(crate) mod fuzzing {
    use super::*;
    use crate::fuzz::Target;

    pub fn targets() -> Vec<Target> {
        vec![Target {
            name: "library",
            seeds: || {
                let m = Meta { title: "Shot ü".into(), app: "Firefox".into(), kind: "Area".into(), created: "2026-11-19T10:00:00+01:00".into(), tags: vec!["bug".into(), "ui".into()], width: 4, height: 3 };
                vec![serde_json::to_vec_pretty(&m).unwrap(), b"{}".to_vec()]
            },
            words: || ["title", "app", "kind", "created", "tags", "width", "height"].iter().map(|s| s.to_string()).collect(),
            run: |b| {
                let dir = crate::fuzz::scratch("library");
                let pic = dir.join("shot.png");
                if !pic.exists() {
                    RgbaImage::from_pixel(4, 3, image::Rgba([9, 9, 9, 255])).save(&pic).unwrap();
                }
                std::fs::write(dir.join("shot.json"), b).unwrap();
                let items = scan(&dir);
                assert_eq!(items.len(), 1, "the picture is listed whatever its tag file says");
                let q = String::from_utf8_lossy(&b[..b.len().min(12)]).into_owned();
                for i in &items {
                    let _ = i.matches(&q);
                    let _ = i.matches("shot bug");
                }
                let _ = merge_scan(items.clone(), items);
                assert!(thumbnail(&pic, 64).is_some());
            },
            budget: |n| (1 << 20) + 64 * n,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_folder(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("snagster-library-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn picture(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]))
    }

    fn put(path: &Path, created: Option<&str>) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        picture(4, 3).save(path).unwrap();
        if let Some(c) = created {
            let item = Item { path: path.to_path_buf(), meta: Meta { title: c.into(), created: c.into(), ..Default::default() } };
            item.save_meta().unwrap();
        }
    }

    fn item(title: &str, app: &str, tags: &[&str]) -> Item {
        Item {
            path: PathBuf::from("x.png"),
            meta: Meta {
                title: title.into(),
                app: app.into(),
                kind: "Area".into(),
                created: "2026-03-04T10:00:00+01:00".into(),
                tags: tags.iter().map(|t| t.to_string()).collect(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn search_matches_every_word_anywhere_ignoring_case() {
        let it = item("Login page", "Firefox", &["bug", "urgent"]);
        assert!(it.matches(""));
        assert!(it.matches("   "));
        assert!(it.matches("login"));
        assert!(it.matches("FIREFOX"));
        assert!(it.matches("urgent login"));
        assert!(it.matches("area"));
        assert!(it.matches("2026-03"));
        assert!(it.matches("  bug   page "));
        assert!(!it.matches("chrome"));
        assert!(!it.matches("login chrome"), "every word must match");
    }

    #[test]
    fn meta_sits_next_to_the_picture() {
        let it = Item { path: PathBuf::from("a").join("shot.png"), meta: Meta::default() };
        assert_eq!(it.meta_path(), PathBuf::from("a").join("shot.json"));
    }

    #[test]
    fn meta_reads_old_and_partial_files() {
        let m: Meta = serde_json::from_str(r#"{ "title": "t", "future": true }"#).unwrap();
        assert_eq!(m.title, "t");
        assert!(m.tags.is_empty());
        assert_eq!(m.width, 0);
    }

    #[test]
    fn scan_finds_pictures_newest_first() {
        let dir = temp_folder("scan");
        put(&dir.join("2026-01").join("a.png"), Some("2026-01-05T10:00:00+00:00"));
        put(&dir.join("2026-03").join("b.png"), Some("2026-03-01T10:00:00+00:00"));
        put(&dir.join("2026-02").join("c.PNG"), Some("2026-02-01T10:00:00+00:00"));
        put(&dir.join("top.png"), Some("2025-12-31T23:59:59+00:00"));
        let titles: Vec<String> = scan(&dir).into_iter().map(|i| i.meta.title).collect();
        assert_eq!(
            titles,
            ["2026-03-01T10:00:00+00:00", "2026-02-01T10:00:00+00:00", "2026-01-05T10:00:00+00:00", "2025-12-31T23:59:59+00:00"]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_skips_hidden_folders_and_other_files() {
        let dir = temp_folder("hidden");
        put(&dir.join("keep.png"), Some("2026-01-01T00:00:00+00:00"));
        put(&dir.join(".thumbs").join("keep.png"), None);
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        std::fs::write(dir.join("shot.jpg"), b"x").unwrap();
        let items = scan(&dir);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].path, dir.join("keep.png"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_without_meta_uses_the_file_name_and_date() {
        let dir = temp_folder("nometa");
        put(&dir.join("plain shot.png"), None);
        put(&dir.join("broken.png"), None);
        std::fs::write(dir.join("broken.json"), b"{ not json").unwrap();
        let mut items = scan(&dir);
        items.sort_by(|a, b| a.meta.title.cmp(&b.meta.title));
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].meta.title, "broken");
        assert_eq!(items[1].meta.title, "plain shot");
        for it in &items {
            assert!(chrono::DateTime::parse_from_rfc3339(&it.meta.created).is_ok(), "{}", it.meta.created);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_of_a_missing_folder_is_empty() {
        assert!(scan(&std::env::temp_dir().join("snagster-library-test-does-not-exist-xyz")).is_empty());
    }

    #[test]
    fn add_saves_picture_and_meta_in_a_month_folder() {
        let dir = temp_folder("add");
        let meta = Meta { title: "Hello".into(), app: "Editor".into(), tags: vec!["t1".into()], ..Default::default() };
        let p1 = add(&dir, &picture(5, 7), meta.clone()).unwrap();
        let p2 = add(&dir, &picture(5, 7), meta).unwrap();
        assert_ne!(p1, p2, "two captures in the same second get different names");
        let month = p1.parent().unwrap().file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(month.len(), 7);
        assert_eq!(&month[4..5], "-");
        assert_eq!(p1.parent().unwrap().parent().unwrap(), dir.as_path());
        let items = scan(&dir);
        assert_eq!(items.len(), 2);
        for it in &items {
            assert_eq!(it.meta.title, "Hello");
            assert_eq!(it.meta.tags, vec!["t1".to_string()]);
            assert_eq!((it.meta.width, it.meta.height), (5, 7));
            assert!(chrono::DateTime::parse_from_rfc3339(&it.meta.created).is_ok());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edited_meta_is_kept() {
        let dir = temp_folder("edit");
        let p = add(&dir, &picture(2, 2), Meta::default()).unwrap();
        let mut it = scan(&dir).pop().unwrap();
        assert_eq!(it.path, p);
        it.meta.tags = vec!["changed".into()];
        it.save_meta().unwrap();
        assert_eq!(scan(&dir)[0].meta.tags, vec!["changed".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn thumbnails_are_made_once_and_cached() {
        let dir = temp_folder("thumb");
        let p = dir.join("big.png");
        picture(200, 100).save(&p).unwrap();
        let t = thumbnail(&p, 50).unwrap();
        assert!(t.width() <= 50 && t.height() <= 50);
        assert_eq!(t.width(), 50);
        let cached = dir.join(".thumbs").join("big.png");
        assert!(cached.exists());
        // The cached one is used, even when a different size is asked for.
        assert_eq!(thumbnail(&p, 20).unwrap().dimensions(), t.dimensions());
        assert!(thumbnail(&dir.join("missing.png"), 50).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn dated(path: &str, created: &str) -> Item {
        Item { path: PathBuf::from(path), meta: Meta { created: created.into(), ..Default::default() } }
    }

    fn paths(items: &[Item]) -> Vec<String> {
        items.iter().map(|i| i.path.display().to_string()).collect()
    }

    #[test]
    fn add_item_gives_the_entry_a_scan_would_find() {
        let dir = temp_folder("additem");
        let meta = Meta { title: "New".into(), app: "Paint".into(), ..Default::default() };
        let it = add_item(&dir, &picture(6, 4), meta).unwrap();
        assert!(it.path.exists() && it.meta_path().exists());
        assert_eq!((it.meta.width, it.meta.height), (6, 4));
        let found = scan(&dir);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, it.path);
        assert_eq!(found[0].meta.created, it.meta.created);
        assert_eq!(found[0].meta.title, "New");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_capture_goes_on_top_once() {
        let mut items = vec![dated("b.png", "2026-02-01T00:00:00+00:00"), dated("a.png", "2026-01-01T00:00:00+00:00")];
        insert_newest(&mut items, dated("c.png", "2026-03-01T00:00:00+00:00"));
        assert_eq!(paths(&items), ["c.png", "b.png", "a.png"]);
        insert_newest(&mut items, dated("c.png", "2026-03-01T00:00:00+00:00"));
        assert_eq!(items.len(), 3, "the same file isn't listed twice");
        insert_newest(&mut items, dated("old.png", "2025-01-01T00:00:00+00:00"));
        assert_eq!(paths(&items).last().unwrap(), "old.png");
        let mut empty = Vec::new();
        insert_newest(&mut empty, dated("x.png", "2026-01-01T00:00:00+00:00"));
        assert_eq!(empty.len(), 1);
    }

    #[test]
    fn captures_added_during_a_scan_are_kept() {
        let scanned = vec![dated("b.png", "2026-02-01T00:00:00+00:00"), dated("a.png", "2026-01-01T00:00:00+00:00")];
        // One the scan already found, one it missed.
        let added = vec![dated("b.png", "2026-02-01T00:00:00+00:00"), dated("new.png", "2026-04-01T00:00:00+00:00")];
        assert_eq!(paths(&merge_scan(scanned, added)), ["new.png", "b.png", "a.png"]);
    }

    #[test]
    fn thumbnail_of_a_picture_in_memory_keeps_its_shape_and_is_cached() {
        let dir = temp_folder("thumbof");
        let p = dir.join("wide.png");
        let t = thumbnail_of(&p, &picture(400, 100), 192).unwrap();
        assert_eq!(t.dimensions(), (192, 48));
        assert!(dir.join(".thumbs").join("wide.png").exists());
        // Small pictures aren't blown up.
        assert_eq!(thumbnail_of(&p, &picture(20, 10), 192).unwrap().dimensions(), (20, 10));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_removes_picture_meta_and_thumbnail() {
        let dir = temp_folder("delete");
        let p = add(&dir, &picture(30, 30), Meta::default()).unwrap();
        thumbnail(&p, 10).unwrap();
        let it = scan(&dir).pop().unwrap();
        let thumb = p.parent().unwrap().join(".thumbs").join(p.file_name().unwrap());
        assert!(thumb.exists() && it.meta_path().exists());
        let bin = crate::recycle::tests::FolderBin::new(dir.join(".bin"));
        let remove = |it: &Item| crate::recycle::remove(&it.path, Some(it.meta.clone()), crate::recycle::Way::Restore, &bin);
        remove(&it).unwrap();
        assert!(!p.exists() && !it.meta_path().exists() && !thumb.exists());
        assert!(scan(&dir).is_empty());
        assert!(remove(&it).is_err(), "deleting twice says so");
        assert!(delete(&it).is_err(), "a capture that is gone isn't sent to the Trash");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
