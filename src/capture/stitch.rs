//! Putting scrolled frames together into one long picture.
//!
//! Each frame is compared with the one before by row: every row gets a small hash of its
//! pixels (colours rounded down, so a little noise doesn't matter), the columns at the left and
//! right edges are left out (that's where scroll bars sit), and rows that are one plain colour
//! don't count when scoring, because they match anywhere. Rows that stay put at the top and
//! bottom of both frames are a sticky header or footer: the offset is searched between them
//! only, new rows are taken from just above the footer, and the footer is kept once, at the
//! bottom. A frame that matches the last one exactly means the end was reached.
//!
//! Frames are always "tall": sideways scrolling turns its frames a quarter turn first, so
//! the same code stitches both directions.

use image::RgbaImage;

/// How much of each side is left out of the row hashes (scroll bars, and the focus ring
/// some programs draw around a scrolling list).
fn edge_columns(width: u32) -> u32 {
    (width / 12).min(24)
}

/// Row hashes of a frame, and which rows are one plain colour.
#[derive(Clone, Debug, PartialEq)]
pub struct Rows {
    pub sig: Vec<u64>,
    pub plain: Vec<bool>,
}

pub fn rows(img: &RgbaImage) -> Rows {
    let w = img.width();
    let edge = edge_columns(w);
    let (x0, x1) = if w > edge * 2 + 4 { (edge, w - edge) } else { (0, w) };
    let mut sig = Vec::with_capacity(img.height() as usize);
    let mut plain = Vec::with_capacity(img.height() as usize);
    for y in 0..img.height() {
        let mut h: u64 = 1469598103934665603;
        let first = img.get_pixel(x0.min(w.saturating_sub(1)), y).0;
        let q = |p: [u8; 4]| [p[0] >> 3, p[1] >> 3, p[2] >> 3];
        let fq = q(first);
        let mut same = true;
        let mut x = x0;
        while x < x1 {
            let p = q(img.get_pixel(x, y).0);
            if p != fq {
                same = false;
            }
            for c in p {
                h ^= c as u64;
                h = h.wrapping_mul(1099511628211);
            }
            x += 1;
        }
        sig.push(h);
        plain.push(same);
    }
    Rows { sig, plain }
}

/// Rows that are identical, at the same place, at the top and at the bottom of two frames.
/// Each is held to a third of the height, so a page that is mostly blank still leaves room
/// to compare.
pub fn sticky(prev: &Rows, next: &Rows) -> (usize, usize) {
    let h = prev.sig.len().min(next.sig.len());
    let cap = h / 3;
    let top = (0..h).take_while(|&i| prev.sig[i] == next.sig[i]).count().min(cap);
    let bottom = (0..h).rev().take_while(|&i| prev.sig[i] == next.sig[i]).count().min(cap);
    (top, bottom)
}

/// How a new frame relates to the last one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Match {
    /// How many rows the content moved up.
    pub dy: usize,
    /// The share of informative rows that matched at that offset (0 to 1).
    pub score: f32,
    /// Sticky rows at the top and bottom.
    pub header: usize,
    pub footer: usize,
}

/// Find how far `next` moved compared with `prev`, between the sticky header and footer.
/// When two offsets score about the same (a list of look-alike rows), the one nearest
/// `expected` (the last step) wins, or else the smaller one.
pub fn find(prev: &Rows, next: &Rows, expected: Option<usize>) -> Match {
    let h = prev.sig.len().min(next.sig.len());
    let (header, footer) = sticky(prev, next);
    let none = Match { dy: 0, score: 0.0, header, footer };
    if header + footer >= h {
        return Match { dy: 0, score: 1.0, header, footer };
    }
    let mid = h - header - footer;
    let min_overlap = (mid / 8).max(8);
    if mid <= min_overlap {
        return none;
    }
    let mut scores: Vec<(usize, f32)> = Vec::new();
    for dy in 0..mid - min_overlap {
        let n = mid - dy;
        let (mut same, mut counted) = (0usize, 0usize);
        for i in 0..n {
            let a = header + dy + i;
            let b = header + i;
            if next.plain[b] && prev.plain[a] {
                continue;
            }
            counted += 1;
            if prev.sig[a] == next.sig[b] {
                same += 1;
            }
        }
        // Too few rows with something on them to judge this offset.
        if counted < (n / 10).max(4) {
            continue;
        }
        scores.push((dy, same as f32 / counted as f32));
    }
    let Some(best) = scores.iter().map(|s| s.1).max_by(|a, b| a.total_cmp(b)) else {
        // Nothing but plain rows: it can't have moved in a way we can see.
        return Match { dy: 0, score: if mid > 0 { 1.0 } else { 0.0 }, header, footer };
    };
    let near: Vec<&(usize, f32)> = scores.iter().filter(|s| s.1 >= best - 0.02).collect();
    let pick = match expected {
        Some(e) => near.iter().min_by_key(|s| (s.0 as i64 - e as i64).abs()),
        None => near.first(),
    };
    let (dy, score) = pick.map(|s| (s.0, s.1)).unwrap_or((0, 0.0));
    Match { dy, score, header, footer }
}

/// A match is trusted from this score up; below it the content jumped (it scrolled further
/// than the frame is tall, or the page redrew), and the whole frame is kept instead.
pub const TRUSTED: f32 = 0.6;

/// What one more frame did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// This many new rows were added.
    Added(u32),
    /// The picture didn't change (twice in a row means the end was reached).
    Same,
    /// The picture is as tall as allowed; stop.
    Full,
}

/// Rows to add for a match: the offset when it is trusted, else everything between the
/// sticky header and footer.
pub fn rows_to_add(m: &Match, frame_height: u32) -> u32 {
    if m.score >= TRUSTED { m.dy as u32 } else { frame_height.saturating_sub((m.header + m.footer) as u32) }
}

/// Keep a picture within `max_height` rows: how many of `add` fit, and whether that's the end.
pub fn cap(height: u32, add: u32, max_height: u32) -> (u32, bool) {
    let room = max_height.saturating_sub(height);
    if add >= room { (room, true) } else { (add, false) }
}

/// Builds the long picture one frame at a time.
pub struct Stitcher {
    out: RgbaImage,
    prev: Rows,
    /// How many rows at the bottom of `out` are the last frame's bottom rows.
    tail: u32,
    last_dy: Option<usize>,
    still: u32,
    max_height: u32,
    pub frames: u32,
}

impl Stitcher {
    pub fn new(first: RgbaImage, max_height: u32) -> Self {
        let max_height = max_height.max(first.height());
        let prev = rows(&first);
        let tail = first.height();
        Self { out: first, prev, tail, last_dy: None, still: 0, max_height, frames: 1 }
    }

    pub fn height(&self) -> u32 {
        self.out.height()
    }

    /// How many frames in a row haven't changed the picture.
    pub fn still(&self) -> u32 {
        self.still
    }

    pub fn push(&mut self, frame: &RgbaImage) -> Step {
        if frame.width() != self.out.width() || frame.height() == 0 {
            // The area changed size (a window was resized): nothing sensible to add.
            self.still += 1;
            return Step::Same;
        }
        let next = rows(frame);
        let m = find(&self.prev, &next, self.last_dy);
        if m.dy == 0 && m.score > 0.97 {
            self.still += 1;
            return Step::Same;
        }
        self.still = 0;
        let fh = frame.height();
        let add = rows_to_add(&m, fh);
        if add == 0 {
            self.still += 1;
            return Step::Same;
        }
        let footer = (m.footer as u32).min(self.tail).min(self.out.height()).min(fh);
        // The new rows sit just above the footer.
        let start = fh.saturating_sub(footer + add);
        let add = add.min(fh - footer - start);
        let keep = self.out.height() - footer;
        let (add, full) = cap(keep, add, self.max_height.saturating_sub(footer));
        // The old footer comes off, the new rows (the first of them, when the picture is
        // full) and the new footer go on.
        let w = frame.width();
        let mut grown = RgbaImage::new(w, keep + add + footer);
        image::imageops::replace(&mut grown, &image::imageops::crop_imm(&self.out, 0, 0, w, keep).to_image(), 0, 0);
        image::imageops::replace(&mut grown, &image::imageops::crop_imm(frame, 0, start, w, add).to_image(), 0, keep as i64);
        image::imageops::replace(&mut grown, &image::imageops::crop_imm(frame, 0, fh - footer, w, footer).to_image(), 0, (keep + add) as i64);
        self.out = grown;
        self.tail = add + footer;
        self.prev = next;
        if m.score >= TRUSTED {
            self.last_dy = Some(m.dy);
        }
        self.frames += 1;
        if full { Step::Full } else { Step::Added(add) }
    }

    pub fn finish(self) -> RgbaImage {
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    /// A tall "page" whose every row has its own colour, so rows can be told apart.
    fn page(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| Rgba([((y % 32) * 8) as u8, ((y / 32) * 8) as u8, if x % 2 == 0 { 40 } else { 200 }, 255]))
    }

    fn cut(img: &RgbaImage, top: u32, h: u32) -> RgbaImage {
        image::imageops::crop_imm(img, 0, top, img.width(), h).to_image()
    }

    /// What a window shows when its page is scrolled to `top`: a sticky header bar and footer
    /// bar that never move, and the page between them.
    fn window(p: &RgbaImage, top: u32, h: u32, header: u32, footer: u32) -> RgbaImage {
        let w = p.width();
        let mut f = RgbaImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let px = if y < header {
                    Rgba([250, (x * 3) as u8, 10, 255])
                } else if y >= h - footer {
                    Rgba([10, (x * 5) as u8, 250, 255])
                } else {
                    *p.get_pixel(x, top + y - header)
                };
                f.put_pixel(x, y, px);
            }
        }
        f
    }

    #[test]
    fn rows_are_told_apart_and_noise_is_ignored() {
        let img = page(60, 100);
        let r = rows(&img);
        assert_eq!(r.sig.len(), 100);
        let distinct: std::collections::HashSet<_> = r.sig.iter().collect();
        assert_eq!(distinct.len(), 100);
        let mut noisy = img.clone();
        for p in noisy.pixels_mut() {
            p.0[0] |= 3;
        }
        assert_eq!(rows(&noisy), r);
        assert!(r.plain.iter().all(|p| !p));
        assert!(rows(&RgbaImage::from_pixel(60, 5, Rgba([9, 9, 9, 255]))).plain.iter().all(|p| *p));
    }

    #[test]
    fn the_scroll_bar_columns_are_left_out() {
        let img = page(120, 80);
        let mut moved = img.clone();
        // A scroll bar thumb on the right edge moves between frames.
        for y in 0..40 {
            for x in 112..120 {
                moved.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
        assert_eq!(rows(&moved).sig, rows(&img).sig);
    }

    #[test]
    fn offsets_are_found() {
        let p = page(40, 600);
        let a = rows(&cut(&p, 0, 150));
        for step in [1, 7, 20, 60, 110] {
            let m = find(&a, &rows(&cut(&p, step, 150)), None);
            assert_eq!(m.dy, step as usize, "step {step}");
            assert!(m.score > 0.99);
        }
    }

    #[test]
    fn sticky_header_and_footer_are_found_and_skipped() {
        let p = page(40, 1000);
        let a = window(&p, 0, 200, 30, 20);
        let b = window(&p, 45, 200, 30, 20);
        let (ra, rb) = (rows(&a), rows(&b));
        assert_eq!(sticky(&ra, &rb), (30, 20));
        let m = find(&ra, &rb, None);
        assert_eq!((m.dy, m.header, m.footer), (45, 30, 20));
        assert!(m.score > 0.99);
    }

    #[test]
    fn stitching_with_a_sticky_header_and_footer_keeps_each_once() {
        let p = page(40, 2000);
        let (h, head, foot) = (220, 30, 24);
        let mut st = Stitcher::new(window(&p, 0, h, head, foot), 100_000);
        let mut top = 0;
        for step in [60, 60, 33, 80, 60] {
            top += step;
            assert_eq!(st.push(&window(&p, top, h, head, foot)), Step::Added(step));
        }
        let out = st.finish();
        // header + every page row seen + footer
        let want = window(&p, 0, head + (top + h - head - foot) + foot, head, foot);
        assert_eq!(out.dimensions(), want.dimensions());
        assert!(out == want, "the stitched page differs from the real one");
    }

    #[test]
    fn identical_frames_mean_the_end() {
        let p = page(40, 400);
        let mut st = Stitcher::new(cut(&p, 0, 120), 10_000);
        assert_eq!(st.push(&cut(&p, 0, 120)), Step::Same);
        assert_eq!(st.still(), 1);
        assert_eq!(st.push(&cut(&p, 0, 120)), Step::Same);
        assert_eq!(st.still(), 2);
        assert_eq!(st.height(), 120);
        // A change starts the count again.
        assert_eq!(st.push(&cut(&p, 30, 120)), Step::Added(30));
        assert_eq!(st.still(), 0);
    }

    #[test]
    fn a_blank_area_is_the_same_frame() {
        let blank = RgbaImage::from_pixel(50, 120, Rgba([255, 255, 255, 255]));
        let m = find(&rows(&blank), &rows(&blank), None);
        assert_eq!((m.dy, m.score), (0, 1.0));
        let mut st = Stitcher::new(blank.clone(), 1000);
        assert_eq!(st.push(&blank), Step::Same);
    }

    #[test]
    fn a_short_last_page_adds_only_what_is_new() {
        // The page ends 70 rows after the last full step: the last scroll moves less.
        let p = page(40, 400);
        let fh = 150;
        let mut st = Stitcher::new(cut(&p, 0, fh), 10_000);
        assert_eq!(st.push(&cut(&p, 90, fh)), Step::Added(90));
        assert_eq!(st.push(&cut(&p, 180, fh)), Step::Added(90));
        assert_eq!(st.push(&cut(&p, 250, fh)), Step::Added(70));
        assert_eq!(st.push(&cut(&p, 250, fh)), Step::Same);
        assert_eq!(st.push(&cut(&p, 250, fh)), Step::Same);
        let out = st.finish();
        assert_eq!(out, cut(&p, 0, 400));
    }

    #[test]
    fn the_height_is_capped() {
        let p = page(40, 2000);
        let fh = 150;
        let mut st = Stitcher::new(cut(&p, 0, fh), 400);
        assert_eq!(st.push(&cut(&p, 100, fh)), Step::Added(100));
        assert_eq!(st.push(&cut(&p, 200, fh)), Step::Added(100));
        assert_eq!(st.push(&cut(&p, 300, fh)), Step::Full);
        let out = st.finish();
        assert_eq!(out.height(), 400);
        assert_eq!(out, cut(&p, 0, 400));
        assert_eq!(cap(390, 30, 400), (10, true));
        assert_eq!(cap(100, 30, 400), (30, false));
        assert_eq!(cap(500, 30, 400), (0, true));
    }

    #[test]
    fn a_jump_keeps_the_whole_frame() {
        let p = page(40, 1200);
        let a = rows(&cut(&p, 0, 120));
        let m = find(&a, &rows(&cut(&p, 600, 120)), None);
        assert!(m.score < TRUSTED, "{}", m.score);
        assert_eq!(rows_to_add(&m, 120), 120 - (m.header + m.footer) as u32);
        let mut st = Stitcher::new(cut(&p, 0, 120), 10_000);
        assert_eq!(st.push(&cut(&p, 600, 120)), Step::Added(120 - (m.header + m.footer) as u32));
    }

    #[test]
    fn small_differences_are_tolerated() {
        // A blinking caret and a changed clock: a few rows differ, the match still holds.
        let p = page(40, 600);
        let a = cut(&p, 0, 160);
        let mut b = cut(&p, 50, 160);
        for y in 70..74 {
            for x in 10..30 {
                b.put_pixel(x, y, Rgba([255, 0, 255, 255]));
            }
        }
        let m = find(&rows(&a), &rows(&b), None);
        assert_eq!(m.dy, 50);
        assert!(m.score > 0.9 && m.score < 1.0, "{}", m.score);
    }

    #[test]
    fn look_alike_rows_follow_the_last_step() {
        // Stripes repeat every 20 rows, so 10, 30 and 50 match equally well.
        let stripes = RgbaImage::from_fn(40, 600, |x, y| Rgba([((y % 20) * 12) as u8, (x * 6) as u8, 0, 255]));
        let a = rows(&cut(&stripes, 0, 200));
        let b = rows(&cut(&stripes, 30, 200));
        assert_eq!(find(&a, &b, Some(29)).dy % 20, 10);
        assert_eq!(find(&a, &b, Some(29)).dy, 30);
        assert_eq!(find(&a, &b, Some(52)).dy, 50);
        assert_eq!(find(&a, &b, None).dy, 10);
    }

    #[test]
    fn sideways_pages_stitch_turned() {
        // A wide page seen through a 120-pixel-wide window, scrolled right.
        let wide = image::imageops::rotate270(&page(90, 900));
        assert_eq!(wide.dimensions(), (900, 90));
        let view = |left: u32| image::imageops::crop_imm(&wide, left, 0, 120, 90).to_image();
        let turn = |img: RgbaImage| image::imageops::rotate90(&img);
        let mut st = Stitcher::new(turn(view(0)), 10_000);
        for left in [40, 80, 150, 200] {
            assert!(matches!(st.push(&turn(view(left))), Step::Added(_)));
        }
        let out = image::imageops::rotate270(&st.finish());
        assert_eq!(out, image::imageops::crop_imm(&wide, 0, 0, 320, 90).to_image());
    }

    #[test]
    fn a_frame_of_another_size_adds_nothing() {
        let p = page(40, 400);
        let mut st = Stitcher::new(cut(&p, 0, 100), 10_000);
        assert_eq!(st.push(&page(41, 100)), Step::Same);
        assert_eq!(st.height(), 100);
    }

    #[test]
    fn edges_are_narrow_and_capped() {
        assert_eq!(edge_columns(12), 1);
        assert_eq!(edge_columns(120), 10);
        assert_eq!(edge_columns(2000), 24);
    }
}
