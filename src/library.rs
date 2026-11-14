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
pub fn add(folder: &Path, img: &RgbaImage, mut meta: Meta) -> Result<PathBuf, String> {
    let now = chrono::Local::now();
    let day = folder.join(now.format("%Y-%m").to_string());
    std::fs::create_dir_all(&day).map_err(|e| e.to_string())?;
    let base = now.format("%Y-%m-%d_%H-%M-%S").to_string();
    let path = crate::settings::unique_path(&day, &base, "png");
    img.save(&path).map_err(|e| e.to_string())?;
    meta.created = now.to_rfc3339();
    meta.width = img.width();
    meta.height = img.height();
    let item = Item { path: path.clone(), meta };
    item.save_meta()?;
    Ok(path)
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
    let img = image::open(path).ok()?.thumbnail(size, size).to_rgba8();
    let _ = std::fs::create_dir_all(&dir);
    let _ = img.save(&thumb);
    Some(img)
}

pub fn delete(item: &Item) -> Result<(), String> {
    std::fs::remove_file(&item.path).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(item.meta_path());
    if let (Some(dir), Some(name)) = (item.path.parent(), item.path.file_name()) {
        let _ = std::fs::remove_file(dir.join(".thumbs").join(name));
    }
    Ok(())
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

    #[test]
    fn delete_removes_picture_meta_and_thumbnail() {
        let dir = temp_folder("delete");
        let p = add(&dir, &picture(30, 30), Meta::default()).unwrap();
        thumbnail(&p, 10).unwrap();
        let it = scan(&dir).pop().unwrap();
        let thumb = p.parent().unwrap().join(".thumbs").join(p.file_name().unwrap());
        assert!(thumb.exists() && it.meta_path().exists());
        delete(&it).unwrap();
        assert!(!p.exists() && !it.meta_path().exists() && !thumb.exists());
        assert!(scan(&dir).is_empty());
        assert!(delete(&it).is_err(), "deleting twice says so");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
