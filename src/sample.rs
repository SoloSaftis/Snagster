//! A made-up sample picture (a plain app mock-up drawn from shapes) to try the tools on.

use crate::doc::{self, Doc, Shape};
use eframe::egui::{Color32, Rect, pos2};
use image::RgbaImage;
use tiny_skia as ts;

fn fill(pm: &mut ts::Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, c: (u8, u8, u8)) {
    let mut paint = ts::Paint::default();
    paint.set_color_rgba8(c.0, c.1, c.2, 255);
    paint.anti_alias = true;
    let Some(rect) = ts::Rect::from_xywh(x, y, w, h) else { return };
    let path = if r <= 0.0 {
        ts::PathBuilder::from_rect(rect)
    } else {
        let mut pb = ts::PathBuilder::new();
        let (l, t, rr, b) = (x, y, x + w, y + h);
        pb.move_to(l + r, t);
        pb.line_to(rr - r, t);
        pb.quad_to(rr, t, rr, t + r);
        pb.line_to(rr, b - r);
        pb.quad_to(rr, b, rr - r, b);
        pb.line_to(l + r, b);
        pb.quad_to(l, b, l, b - r);
        pb.line_to(l, t + r);
        pb.quad_to(l, t, l + r, t);
        pb.close();
        match pb.finish() {
            Some(p) => p,
            None => return,
        }
    };
    pm.fill_path(&path, &paint, ts::FillRule::Winding, ts::Transform::identity(), None);
}

fn circle(pm: &mut ts::Pixmap, x: f32, y: f32, r: f32, c: (u8, u8, u8)) {
    let mut paint = ts::Paint::default();
    paint.set_color_rgba8(c.0, c.1, c.2, 255);
    paint.anti_alias = true;
    if let Some(p) = ts::PathBuilder::from_circle(x, y, r) {
        pm.fill_path(&p, &paint, ts::FillRule::Winding, ts::Transform::identity(), None);
    }
}

pub fn sample_image(variant: u32) -> RgbaImage {
    let (w, h) = (1280u32, 800u32);
    let mut pm = ts::Pixmap::new(w, h).unwrap();
    // Soft diagonal backdrop.
    let (c0, c1) = if variant == 0 { ((88, 112, 220), (176, 120, 214)) } else { ((44, 150, 140), (90, 190, 120)) };
    {
        let data = pm.data_mut();
        for y in 0..h {
            for x in 0..w {
                let t = (x as f32 / w as f32 * 0.6 + y as f32 / h as f32 * 0.4).clamp(0.0, 1.0);
                let i = ((y * w + x) * 4) as usize;
                data[i] = (c0.0 as f32 + (c1.0 as f32 - c0.0 as f32) * t) as u8;
                data[i + 1] = (c0.1 as f32 + (c1.1 as f32 - c0.1 as f32) * t) as u8;
                data[i + 2] = (c0.2 as f32 + (c1.2 as f32 - c0.2 as f32) * t) as u8;
                data[i + 3] = 255;
            }
        }
    }
    // App window mock-up.
    fill(&mut pm, 96.0, 70.0, 1088.0, 660.0, 14.0, (40, 44, 56));
    fill(&mut pm, 100.0, 74.0, 1080.0, 652.0, 12.0, (248, 249, 252));
    fill(&mut pm, 100.0, 74.0, 1080.0, 48.0, 12.0, (232, 235, 242));
    fill(&mut pm, 100.0, 104.0, 1080.0, 18.0, 0.0, (232, 235, 242));
    circle(&mut pm, 126.0, 98.0, 7.0, (240, 98, 92));
    circle(&mut pm, 150.0, 98.0, 7.0, (246, 190, 79));
    circle(&mut pm, 174.0, 98.0, 7.0, (98, 196, 98));
    fill(&mut pm, 300.0, 88.0, 560.0, 20.0, 10.0, (255, 255, 255));
    // Sidebar.
    fill(&mut pm, 100.0, 122.0, 220.0, 604.0, 0.0, (238, 240, 246));
    for i in 0..7 {
        let y = 150.0 + i as f32 * 44.0;
        let c = if i == 1 { (200, 212, 250) } else { (238, 240, 246) };
        fill(&mut pm, 114.0, y - 8.0, 192.0, 34.0, 8.0, c);
        circle(&mut pm, 136.0, y + 9.0, 8.0, if i == 1 { (80, 110, 230) } else { (170, 176, 192) });
        fill(&mut pm, 154.0, y + 4.0, 90.0 + (i * 17 % 50) as f32, 10.0, 5.0, (170, 176, 192));
    }
    // Header and cards.
    fill(&mut pm, 352.0, 150.0, 300.0, 22.0, 6.0, (60, 64, 80));
    fill(&mut pm, 352.0, 184.0, 460.0, 12.0, 6.0, (180, 186, 200));
    for k in 0..3 {
        let x = 352.0 + k as f32 * 264.0;
        fill(&mut pm, x, 222.0, 244.0, 150.0, 12.0, (255, 255, 255));
        fill(&mut pm, x + 18.0, 242.0, 60.0, 60.0, 12.0, [(255, 206, 120), (150, 210, 255), (180, 230, 170)][k]);
        fill(&mut pm, x + 18.0, 318.0, 160.0, 12.0, 6.0, (90, 96, 112));
        fill(&mut pm, x + 18.0, 340.0, 200.0, 10.0, 5.0, (196, 200, 212));
    }
    // A form with a private-looking field.
    fill(&mut pm, 352.0, 400.0, 776.0, 290.0, 12.0, (255, 255, 255));
    for r in 0..3 {
        let y = 428.0 + r as f32 * 70.0;
        fill(&mut pm, 380.0, y, 120.0, 10.0, 5.0, (120, 126, 140));
        fill(&mut pm, 380.0, y + 20.0, 420.0, 32.0, 6.0, (240, 242, 247));
        fill(&mut pm, 392.0, y + 31.0, 140.0 + r as f32 * 60.0, 10.0, 5.0, (60, 64, 80));
    }
    fill(&mut pm, 880.0, 620.0, 210.0, 44.0, 10.0, (80, 110, 230));
    fill(&mut pm, 930.0, 637.0, 110.0, 10.0, 5.0, (255, 255, 255));
    RgbaImage::from_raw(w, h, pm.take()).unwrap()
}

/// The sample picture with a few marks on it, to show what the tools do.
pub fn sample_doc() -> Doc {
    let mut d = Doc::new(sample_image(0));
    // Blur the third field.
    if let Some(b) = d.blur(Rect::from_min_max(pos2(380.0, 568.0), pos2(800.0, 600.0)), 12) {
        d = b;
    }
    let red = Color32::from_rgb(229, 72, 77);
    let yellow = Color32::from_rgb(255, 197, 61);
    d.shapes = vec![
        Shape::Highlight {
            points: vec![pos2(352.0, 161.0), pos2(652.0, 161.0)],
            color: yellow,
            width: 6.0,
        },
        Shape::Rect { rect: Rect::from_min_max(pos2(870.0, 610.0), pos2(1100.0, 674.0)), color: red, width: 5.0, fill: false, radius: 10.0, shadow: true },
        Shape::Arrow { a: pos2(900.0, 552.0), b: pos2(962.0, 606.0), color: red, width: 6.0, shadow: true, heads: 1 },
        Shape::Step { center: pos2(352.0, 222.0), n: 1, color: red, radius: doc::step_radius(6.0), shadow: true, style: Default::default() },
        Shape::Step { center: pos2(1100.0, 610.0), n: 2, color: red, radius: doc::step_radius(6.0), shadow: true, style: Default::default() },
        Shape::Text { pos: pos2(826.0, 494.0), text: "Press Save when done".into(), color: red, size: 26.0, bg: true, shadow: true, font: String::new(), bold: false, italic: false, outline: true, align: 0 },
        Shape::Stamp { center: pos2(1150.0, 250.0), emoji: "\u{1F44D}".into(), size: 64.0, color: red },
        Shape::Pointer { pos: pos2(1010.0, 640.0), size: 30.0 },
    ];
    d
}
