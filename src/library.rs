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
