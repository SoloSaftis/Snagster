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
