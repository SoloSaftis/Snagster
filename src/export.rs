//! Writing pictures and guides to files: PNG, JPEG, BMP, GIF, WebP, TIFF, PDF, SVG, HTML
//! guides, and Snagster's own layered project file (.snag) that keeps marks editable.

use crate::doc::{Doc, Shape};
use base64::Engine as _;
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn png_bytes(img: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

fn jpeg_bytes(img: &RgbaImage, quality: u8) -> Result<Vec<u8>, String> {
    let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
    let mut out = Vec::new();
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
    rgb.write_with_encoder(enc).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Save a flattened picture in the format its file name asks for.
pub fn save_image(img: &RgbaImage, path: &Path, jpeg_quality: u8) -> Result<(), String> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => std::fs::write(path, jpeg_bytes(img, jpeg_quality)?).map_err(|e| e.to_string()),
        "bmp" => image::DynamicImage::ImageRgba8(img.clone()).to_rgb8().save(path).map_err(|e| e.to_string()),
        "pdf" => std::fs::write(path, pdf(&[(None, img.clone())], false, jpeg_quality)?).map_err(|e| e.to_string()),
        "svg" => std::fs::write(path, svg(img)?).map_err(|e| e.to_string()),
        "gif" | "webp" | "tiff" | "tif" | "png" | "ico" => img.save(path).map_err(|e| e.to_string()),
        _ => img.save(path).map_err(|e| e.to_string()),
    }
}

fn svg(img: &RgbaImage) -> Result<String, String> {
    let (w, h) = img.dimensions();
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">\n  <image width=\"{w}\" height=\"{h}\" href=\"data:image/png;base64,{}\"/>\n</svg>\n",
        b64(&png_bytes(img)?)
    ))
}

fn pdf_text(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 128 && !c.is_control() => out.push(c),
            'æ' | 'ø' | 'å' | 'Æ' | 'Ø' | 'Å' | 'é' | 'è' | 'ä' | 'ö' | 'ü' | 'Ä' | 'Ö' | 'Ü' | 'ß' => {
                out.push_str(&format!("\\{:03o}", c as u32));
            }
            _ => out.push('?'),
        }
    }
    out
}

/// A PDF with one picture per page. `a4` lays each out on an A4 page under its caption;
/// otherwise each page is exactly the picture's size.
pub fn pdf(pages: &[(Option<String>, RgbaImage)], a4: bool, quality: u8) -> Result<Vec<u8>, String> {
    let mut objects: Vec<Vec<u8>> = Vec::new();
    // 1: catalog, 2: pages, 3: font; then per page: page, contents, image.
    let n = pages.len();
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + i * 3)).collect();
    objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    objects.push(format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")).into_bytes());
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec());
    for (i, (caption, img)) in pages.iter().enumerate() {
        let (iw, ih) = (img.width() as f32, img.height() as f32);
        let page_obj = 4 + i * 3;
        let (pw, ph, draw_w, draw_h, x, y, text) = if a4 {
            let (pw, ph) = (595.0f32, 842.0f32);
            let margin = 40.0;
            let top_text = 40.0;
            let max_w = pw - margin * 2.0;
            let max_h = ph - margin * 2.0 - top_text;
            let s = (max_w / iw).min(max_h / ih).min(0.75);
            let (dw, dh) = (iw * s, ih * s);
            let text = caption
                .as_ref()
                .map(|c| format!("BT /F1 16 Tf {margin} {} Td ({}) Tj ET\n", ph - margin - 16.0, pdf_text(c)))
                .unwrap_or_default();
            (pw, ph, dw, dh, margin, ph - margin - top_text - dh, text)
        } else {
            let (pw, ph) = (iw * 0.75, ih * 0.75);
            (pw, ph, pw, ph, 0.0, 0.0, String::new())
        };
        let content = format!("{text}q {draw_w:.2} 0 0 {draw_h:.2} {x:.2} {y:.2} cm /Im{i} Do Q\n");
        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.2} {ph:.2}] /Resources << /Font << /F1 3 0 R >> /XObject << /Im{i} {} 0 R >> >> /Contents {} 0 R >>",
                page_obj + 2,
                page_obj + 1
            )
            .into_bytes(),
        );
        let mut c = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
        c.extend_from_slice(content.as_bytes());
        c.extend_from_slice(b"\nendstream");
        objects.push(c);
        let jpg = jpeg_bytes(img, quality.max(60))?;
        let mut im = format!(
            "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
            img.width(),
            img.height(),
            jpg.len()
        )
        .into_bytes();
        im.extend_from_slice(&jpg);
        im.extend_from_slice(b"\nendstream");
        objects.push(im);
    }
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        write!(out, "{} 0 obj\n", i + 1).ok();
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    write!(out, "xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).ok();
    for off in offsets {
        write!(out, "{off:010} 00000 n \n").ok();
    }
    write!(out, "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).ok();
    Ok(out)
}

/// A single-file web page guide: title, then each step's caption and picture.
pub fn html_guide(title: &str, steps: &[(String, RgbaImage)]) -> Result<String, String> {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let mut html = format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>{t}</title>\n<style>body{{font-family:Segoe UI,Arial,sans-serif;max-width:960px;margin:40px auto;color:#1c1e26;padding:0 20px}}h1{{font-size:30px}}h2{{font-size:20px;margin-top:36px}}img{{max-width:100%;border:1px solid #d2d6de;border-radius:6px}}</style></head><body>\n<h1>{t}</h1>\n",
        t = esc(title)
    );
    for (i, (cap, img)) in steps.iter().enumerate() {
        html.push_str(&format!(
            "<h2>Step {}: {}</h2>\n<img alt=\"Step {}\" src=\"data:image/png;base64,{}\">\n",
            i + 1,
            esc(cap),
            i + 1,
            b64(&png_bytes(img)?)
        ));
    }
    html.push_str("</body></html>\n");
    Ok(html)
}

// ---------- Snagster project files ----------

#[derive(Serialize, Deserialize)]
struct Project {
    format: String,
    version: u32,
    name: String,
    image_png: String,
    shapes: Vec<Shape>,
}

/// Save the picture and its marks so they open again with the marks still editable.
pub fn save_project(d: &Doc, name: &str, path: &Path) -> Result<(), String> {
    let p = Project {
        format: "snag-project".into(),
        version: 1,
        name: name.into(),
        image_png: b64(&png_bytes(&d.image)?),
        shapes: d.shapes.clone(),
    };
    let json = serde_json::to_string(&p).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

pub fn load_project(path: &Path) -> Result<(String, Doc), String> {
    let s = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let p: Project = serde_json::from_str(&s).map_err(|e| format!("not a Snagster project: {e}"))?;
    if p.format != "snag-project" {
        return Err("not a Snagster project".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(p.image_png).map_err(|e| e.to_string())?;
    let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?.to_rgba8();
    let mut d = Doc::new(img);
    d.shapes = p.shapes;
    Ok((p.name, d))
}

// ---------- animated GIFs ----------

/// Read the frames of an animated GIF, shrunk to at most `max_w` wide (0 keeps full size).
pub fn gif_frames(path: &Path, max_w: u32) -> Result<Vec<(RgbaImage, u32)>, String> {
    use image::AnimationDecoder;
    let file = std::io::BufReader::new(std::fs::File::open(path).map_err(|e| e.to_string())?);
    let dec = image::codecs::gif::GifDecoder::new(file).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for f in dec.into_frames() {
        let f = f.map_err(|e| e.to_string())?;
        let (n, d) = f.delay().numer_denom_ms();
        let ms = if d == 0 { 100 } else { (n / d).max(20) };
        let mut img = f.into_buffer();
        if max_w > 0 && img.width() > max_w {
            let h = (img.height() as f32 * max_w as f32 / img.width() as f32).round().max(1.0) as u32;
            img = image::imageops::resize(&img, max_w, h, image::imageops::FilterType::Triangle);
        }
        out.push((img, ms));
        if out.len() > 3000 {
            break;
        }
    }
    Ok(out)
}

/// Write frames as a looping GIF.
pub fn write_gif(path: &Path, frames: &[(RgbaImage, u32)]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = image::codecs::gif::GifEncoder::new_with_speed(std::io::BufWriter::new(file), 15);
    enc.set_repeat(image::codecs::gif::Repeat::Infinite).map_err(|e| e.to_string())?;
    for (img, ms) in frames {
        enc.encode_frame(image::Frame::from_parts(img.clone(), 0, 0, image::Delay::from_numer_denom_ms(*ms, 1)))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Fit pictures onto one canvas size (centred on dark grey) so they can be frames of one GIF.
pub fn letterbox(images: &[RgbaImage], max_w: u32) -> Vec<RgbaImage> {
    let w = images.iter().map(|i| i.width()).max().unwrap_or(1).min(max_w.max(64));
    let h = images
        .iter()
        .map(|i| (i.height() as f32 * (w as f32 / i.width() as f32).min(1.0)) as u32)
        .max()
        .unwrap_or(1)
        .max(1);
    images
        .iter()
        .map(|img| {
            let s = (w as f32 / img.width() as f32).min(h as f32 / img.height() as f32).min(1.0);
            let (iw, ih) = (((img.width() as f32 * s) as u32).max(1), ((img.height() as f32 * s) as u32).max(1));
            let scaled = image::imageops::resize(img, iw, ih, image::imageops::FilterType::Triangle);
            let mut canvas = RgbaImage::from_pixel(w, h, image::Rgba([30, 32, 38, 255]));
            image::imageops::overlay(&mut canvas, &scaled, ((w - iw) / 2) as i64, ((h - ih) / 2) as i64);
            canvas
        })
        .collect()
}

// ---------- Word documents ----------

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// A Word document (.docx): a title, then each picture under its caption, sized to the page.
pub fn docx(title: &str, items: &[(String, RgbaImage)], path: &Path) -> Result<(), String> {
    use zip::write::SimpleFileOptions;
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut z = zip::ZipWriter::new(file);
    let opt = SimpleFileOptions::default();
    let err = |e: zip::result::ZipError| e.to_string();
    z.start_file("[Content_Types].xml", opt).map_err(err)?;
    z.write_all(
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#,
    )
    .map_err(|e| e.to_string())?;
    z.start_file("_rels/.rels", opt).map_err(err)?;
    z.write_all(
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
    )
    .map_err(|e| e.to_string())?;
    let mut rels = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    let mut body = format!(
        r#"<w:p><w:pPr><w:spacing w:after="240"/></w:pPr><w:r><w:rPr><w:b/><w:sz w:val="40"/></w:rPr><w:t xml:space="preserve">{}</w:t></w:r></w:p>"#,
        xml_escape(title)
    );
    // Usable page width: 6 inches in EMUs.
    let max_w = 6.0 * 914_400.0;
    for (i, (cap, img)) in items.iter().enumerate() {
        let n = i + 1;
        z.start_file(format!("word/media/image{n}.png"), opt).map_err(err)?;
        z.write_all(&png_bytes(img)?).map_err(|e| e.to_string())?;
        rels.push_str(&format!(
            r#"<Relationship Id="rImg{n}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image{n}.png"/>"#
        ));
        let px_w = img.width() as f64 * 9525.0;
        let s = (max_w / px_w).min(1.0);
        let (cx, cy) = ((px_w * s) as u64, (img.height() as f64 * 9525.0 * s) as u64);
        body.push_str(&format!(
            r#"<w:p><w:pPr><w:spacing w:before="240" w:after="120"/></w:pPr><w:r><w:rPr><w:b/><w:sz w:val="26"/></w:rPr><w:t xml:space="preserve">{n}. {cap}</w:t></w:r></w:p><w:p><w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{cx}" cy="{cy}"/><wp:docPr id="{n}" name="Picture {n}"/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:nvPicPr><pic:cNvPr id="{n}" name="image{n}.png"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="rImg{n}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#,
            cap = xml_escape(cap)
        ));
    }
    rels.push_str("</Relationships>");
    z.start_file("word/_rels/document.xml.rels", opt).map_err(err)?;
    z.write_all(rels.as_bytes()).map_err(|e| e.to_string())?;
    let doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"><w:body>{body}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1134" w:right="1134" w:bottom="1134" w:left="1134"/></w:sectPr></w:body></w:document>"#
    );
    z.start_file("word/document.xml", opt).map_err(err)?;
    z.write_all(doc.as_bytes()).map_err(|e| e.to_string())?;
    z.finish().map_err(err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Color32, Rect, pos2};
    use std::io::Read as _;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// A fresh folder under the system's temp folder, removed again when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!("snagster-export-test-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn gradient(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x * 25) as u8, (y * 25) as u8, 128, 255]))
    }

    fn solid(w: u32, h: u32, c: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba(c))
    }

    fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    fn count(hay: &str, needle: &str) -> usize {
        hay.matches(needle).count()
    }

    #[test]
    fn png_bytes_round_trip_exactly() {
        let img = RgbaImage::from_fn(7, 5, |x, y| image::Rgba([x as u8 * 30, y as u8 * 50, 9, (x * y * 6) as u8]));
        let bytes = png_bytes(&img).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(image::load_from_memory(&bytes).unwrap().to_rgba8(), img);
    }

    #[test]
    fn jpeg_bytes_are_a_jpeg_of_the_same_size() {
        let img = gradient(16, 8);
        let bytes = jpeg_bytes(&img, 90).unwrap();
        assert_eq!(&bytes[..2], &[0xFF, 0xD8]);
        let back = image::load_from_memory(&bytes).unwrap();
        assert_eq!((back.width(), back.height()), (16, 8));
        // Quality is clamped, so 0 still works, and lower quality is not bigger.
        let low = jpeg_bytes(&img, 0).unwrap();
        assert!(low.len() <= bytes.len());
    }

    #[test]
    fn save_image_writes_every_format() {
        let dir = TempDir::new("formats");
        let img = gradient(10, 6);
        for name in ["a.png", "b.jpg", "c.jpeg", "d.bmp", "e.gif", "f.webp", "g.tiff", "h.tif", "i.ico", "UPPER.PNG"] {
            let path = dir.file(name);
            save_image(&img, &path, 85).unwrap_or_else(|e| panic!("{name}: {e}"));
            let back = image::open(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!((back.width(), back.height()), (10, 6), "{name}");
        }
        // Lossless formats keep every pixel.
        for name in ["a.png", "g.tiff"] {
            assert_eq!(image::open(dir.file(name)).unwrap().to_rgba8(), img, "{name}");
        }
        let svg_text = std::fs::read_to_string({
            let p = dir.file("j.svg");
            save_image(&img, &p, 85).unwrap();
            p
        })
        .unwrap();
        assert!(svg_text.starts_with("<svg ") && svg_text.contains("width=\"10\" height=\"6\""));
        let p = dir.file("k.pdf");
        save_image(&img, &p, 85).unwrap();
        assert!(std::fs::read(&p).unwrap().starts_with(b"%PDF-1.4"));
        // Folders on the way are made.
        let nested = dir.file("x").join("y").join("z.png");
        save_image(&img, &nested, 85).unwrap();
        assert!(nested.exists());
        // An unknown extension is an error, not a panic.
        assert!(save_image(&img, &dir.file("nope.xyz"), 85).is_err());
    }

    #[test]
    fn svg_embeds_the_picture() {
        let img = gradient(5, 3);
        let s = svg(&img).unwrap();
        assert!(s.contains("viewBox=\"0 0 5 3\""));
        let start = s.find("base64,").unwrap() + 7;
        let end = s[start..].find('"').unwrap() + start;
        let bytes = base64::engine::general_purpose::STANDARD.decode(&s[start..end]).unwrap();
        assert_eq!(image::load_from_memory(&bytes).unwrap().to_rgba8(), img);
    }

    #[test]
    fn pdf_text_escapes() {
        assert_eq!(pdf_text("a(b)\\c"), "a\\(b\\)\\\\c");
        assert_eq!(pdf_text("Hello, world!"), "Hello, world!");
        assert_eq!(pdf_text("é"), "\\351");
        assert_eq!(pdf_text("Æøå"), "\\306\\370\\345");
        assert_eq!(pdf_text("€\n"), "??");
        assert_eq!(pdf_text(""), "");
    }

    /// Check the cross-reference table points at each object.
    fn check_xref(pdf: &[u8], objects: usize) {
        // Work on bytes: the pictures are binary, so text offsets would not match.
        assert!(pdf.starts_with(b"%PDF-1.4\n"));
        assert!(pdf.ends_with(b"%%EOF\n"));
        let sx = pdf.windows(10).rposition(|w| w == b"startxref\n").unwrap();
        let tail = std::str::from_utf8(&pdf[sx + 10..]).unwrap();
        let xref_at: usize = tail.lines().next().unwrap().parse().unwrap();
        assert_eq!(&pdf[xref_at..xref_at + 5], b"xref\n");
        let table = std::str::from_utf8(&pdf[xref_at..]).unwrap();
        let mut lines = table.lines().skip(1);
        assert_eq!(lines.next().unwrap(), format!("0 {}", objects + 1));
        assert!(lines.next().unwrap().starts_with("0000000000 65535 f"));
        for i in 1..=objects {
            let line = lines.next().unwrap();
            let off: usize = line[..10].parse().unwrap();
            let head = format!("{i} 0 obj\n");
            assert_eq!(&pdf[off..off + head.len()], head.as_bytes(), "object {i}");
        }
        assert!(table.contains(&format!("/Size {}", objects + 1)));
    }

    #[test]
    fn pdf_one_page_per_picture() {
        let pages = vec![(Some("First (one)".to_string()), gradient(40, 20)), (None, gradient(20, 40))];
        let a4 = pdf(&pages, true, 80).unwrap();
        check_xref(&a4, 3 + 2 * 3);
        let t = String::from_utf8_lossy(&a4);
        assert!(t.contains("/Count 2"));
        assert!(t.contains("/Kids [4 0 R 7 0 R]"));
        assert_eq!(count(&t, "/MediaBox [0 0 595.00 842.00]"), 2);
        assert!(t.contains("(First \\(one\\)) Tj"), "the caption is escaped");
        assert_eq!(count(&t, " Tj "), 1, "only the first page has a caption");
        assert!(t.contains("/Width 40 /Height 20"));
        // Each picture is a JPEG stream.
        assert_eq!(a4.windows(3).filter(|w| *w == [0xFF, 0xD8, 0xFF]).count(), 2);

        let exact = pdf(&[(Some("ignored".into()), gradient(100, 40))], false, 80).unwrap();
        check_xref(&exact, 6);
        let t = String::from_utf8_lossy(&exact);
        assert!(t.contains("/MediaBox [0 0 75.00 30.00]"), "pixels become points at 0.75");
        assert!(!t.contains("Tj"));
        // The content stream's length is right.
        let at = find(&exact, b"<< /Length ").unwrap();
        let digits: Vec<u8> = exact[at + 11..].iter().copied().take_while(u8::is_ascii_digit).collect();
        let len: usize = String::from_utf8(digits).unwrap().parse().unwrap();
        let s = find(&exact[at..], b"stream\n").unwrap() + at + 7;
        assert_eq!(&exact[s + len..s + len + 10], b"\nendstream");

        let empty = pdf(&[], true, 80).unwrap();
        check_xref(&empty, 3);
    }

    #[test]
    fn html_guide_escapes_and_embeds() {
        let steps = vec![("Click <OK> & wait".to_string(), gradient(4, 4)), ("Done".to_string(), gradient(3, 2))];
        let h = html_guide("Fish & <chips>", &steps).unwrap();
        assert!(h.starts_with("<!doctype html>"));
        assert!(h.contains("<title>Fish &amp; &lt;chips&gt;</title>"));
        assert!(h.contains("<h2>Step 1: Click &lt;OK&gt; &amp; wait</h2>"));
        assert!(h.contains("<h2>Step 2: Done</h2>"));
        assert_eq!(count(&h, "<img "), 2);
        assert!(h.trim_end().ends_with("</body></html>"));
        assert_eq!(count(&html_guide("x", &[]).unwrap(), "<img "), 0);
    }

    #[test]
    fn project_round_trip_keeps_marks() {
        let dir = TempDir::new("project");
        let path = dir.file("one.snag");
        let mut d = Doc::new(gradient(9, 7));
        d.shapes.push(Shape::Rect {
            rect: Rect::from_min_max(pos2(1.0, 1.0), pos2(5.0, 5.0)),
            color: Color32::RED,
            width: 3.0,
            fill: false,
            radius: 2.0,
            shadow: true,
        });
        d.shapes.push(Shape::Image {
            rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(2.0, 2.0)),
            image: crate::doc::ImgData(Arc::new(solid(2, 2, [1, 2, 3, 4]))),
        });
        save_project(&d, "My shot", &path).unwrap();
        let (name, back) = load_project(&path).unwrap();
        assert_eq!(name, "My shot");
        assert_eq!(*back.image, *d.image);
        assert_eq!(back.shapes.len(), 2);
        assert!(matches!(back.shapes[0], Shape::Rect { width: 3.0, radius: 2.0, shadow: true, .. }));
        let Shape::Image { image, .. } = &back.shapes[1] else { panic!("image mark lost") };
        assert_eq!(*image.0, solid(2, 2, [1, 2, 3, 4]));
        let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(json["format"], "snag-project");
        assert_eq!(json["version"], 1);
    }

    #[test]
    fn load_project_rejects_other_files() {
        let dir = TempDir::new("badproject");
        let wrong = dir.file("wrong.snag");
        std::fs::write(&wrong, r#"{"format":"other","version":1,"name":"x","image_png":"","shapes":[]}"#).unwrap();
        assert_eq!(load_project(&wrong).err().unwrap(), "not a Snagster project");
        let junk = dir.file("junk.snag");
        std::fs::write(&junk, "hello").unwrap();
        assert!(load_project(&junk).err().unwrap().starts_with("not a Snagster project"));
        let bad_png = dir.file("badpng.snag");
        std::fs::write(&bad_png, r#"{"format":"snag-project","version":1,"name":"x","image_png":"aGVsbG8=","shapes":[]}"#).unwrap();
        assert!(load_project(&bad_png).is_err());
        assert!(load_project(&dir.file("missing.snag")).is_err());
    }

    #[test]
    fn gif_round_trip() {
        let dir = TempDir::new("gif");
        let path = dir.file("sub").join("anim.gif");
        let frames = vec![(solid(8, 6, [255, 0, 0, 255]), 100), (solid(8, 6, [0, 0, 255, 255]), 250)];
        write_gif(&path, &frames).unwrap();
        let back = gif_frames(&path, 0).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].1, 100);
        assert_eq!(back[1].1, 250);
        assert_eq!(back[0].0.dimensions(), (8, 6));
        let p = back[0].0.get_pixel(3, 3).0;
        assert!(p[0] > 240 && p[1] < 16 && p[2] < 16, "{p:?}");
        let p = back[1].0.get_pixel(3, 3).0;
        assert!(p[2] > 240 && p[0] < 16, "{p:?}");
        // Shrunk to a maximum width, keeping the shape.
        let small = gif_frames(&path, 4).unwrap();
        assert_eq!(small[0].0.dimensions(), (4, 3));
        assert!(gif_frames(&dir.file("missing.gif"), 0).is_err());
        let not_gif = dir.file("not.gif");
        std::fs::write(&not_gif, b"nope").unwrap();
        assert!(gif_frames(&not_gif, 0).is_err());
    }

    #[test]
    fn letterbox_puts_every_frame_on_one_canvas() {
        let imgs = vec![solid(100, 50, [255, 0, 0, 255]), solid(50, 100, [0, 255, 0, 255])];
        let out = letterbox(&imgs, 1000);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|i| i.dimensions() == (100, 100)));
        assert_eq!(out[0].get_pixel(50, 50).0, [255, 0, 0, 255]);
        assert_eq!(out[0].get_pixel(50, 5).0, [30, 32, 38, 255], "bars above and below");
        assert_eq!(out[1].get_pixel(5, 50).0, [30, 32, 38, 255], "bars left and right");
        assert_eq!(out[1].get_pixel(50, 50).0, [0, 255, 0, 255]);
        // The width limit is never below 64.
        let capped = letterbox(&imgs, 10);
        assert_eq!(capped[0].width(), 64);
        assert!(letterbox(&[], 100).is_empty());
    }

    #[test]
    fn xml_escape_covers_markup() {
        assert_eq!(xml_escape(r#"a<b>&"c""#), "a&lt;b&gt;&amp;&quot;c&quot;");
        assert_eq!(xml_escape("plain"), "plain");
    }

    #[test]
    fn docx_is_a_valid_package() {
        let dir = TempDir::new("docx");
        let path = dir.file("guide.docx");
        let items = vec![("Open <menu>".to_string(), gradient(100, 50)), ("Wide".to_string(), gradient(1000, 10))];
        docx("Guide & notes", &items, &path).unwrap();
        let mut z = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let names: Vec<String> = z.file_names().map(|n| n.unwrap().to_string()).collect();
        for n in ["[Content_Types].xml", "_rels/.rels", "word/_rels/document.xml.rels", "word/document.xml", "word/media/image1.png", "word/media/image2.png"] {
            assert!(names.iter().any(|x| x == n), "missing {n}");
        }
        let mut doc = String::new();
        z.by_name("word/document.xml").unwrap().read_to_string(&mut doc).unwrap();
        assert!(doc.contains("Guide &amp; notes"));
        assert!(doc.contains("1. Open &lt;menu&gt;"));
        assert!(doc.contains(r#"<wp:extent cx="952500" cy="476250"/>"#), "small pictures keep their size");
        // Wide pictures are scaled down to fit 6 inches (5486400 EMU), give or take rounding.
        let widths: Vec<u64> = doc
            .split(r#"<wp:extent cx=""#)
            .skip(1)
            .map(|s| s[..s.find('"').unwrap()].parse().unwrap())
            .collect();
        assert_eq!(widths.len(), 2);
        assert!(widths[1].abs_diff(5_486_400) <= 1, "{widths:?}");
        let mut rels = String::new();
        z.by_name("word/_rels/document.xml.rels").unwrap().read_to_string(&mut rels).unwrap();
        assert_eq!(count(&rels, "<Relationship "), 2);
        let mut png = Vec::new();
        z.by_name("word/media/image1.png").unwrap().read_to_end(&mut png).unwrap();
        assert_eq!(image::load_from_memory(&png).unwrap().to_rgba8(), gradient(100, 50));
    }
}
