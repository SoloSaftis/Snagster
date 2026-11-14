//! The modified discrete cosine transform (MDCT) and the window shapes AAC uses.
//!
//! The forward transform follows the standard's definition
//! `X[k] = 2 * sum z[n] cos(2*pi/N * (n + n0) * (k + 1/2))`, `n0 = (N/2 + 1) / 2`,
//! so a decoder's inverse (`2/N * sum ...`) with overlap-add gives the input back.
//! It is computed as a fold to a DCT-IV of half the length, done with a complex FFT
//! of a quarter of the length.

use std::f64::consts::PI;

/// In-place radix-2 complex FFT (forward, `exp(-2*pi*i*n*k/len)`).
struct Fft {
    len: usize,
    rev: Vec<u32>,
    tw: Vec<(f32, f32)>,
}

impl Fft {
    fn new(len: usize) -> Self {
        assert!(len.is_power_of_two());
        let bits = len.trailing_zeros();
        let rev = (0..len as u32).map(|i| if bits == 0 { 0 } else { i.reverse_bits() >> (32 - bits) }).collect();
        let tw = (0..len / 2)
            .map(|k| {
                let a = -2.0 * PI * k as f64 / len as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .collect();
        Self { len, rev, tw }
    }

    fn run(&self, re: &mut [f32], im: &mut [f32]) {
        let n = self.len;
        for i in 0..n {
            let j = self.rev[i] as usize;
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut size = 2;
        while size <= n {
            let half = size / 2;
            let step = n / size;
            for start in (0..n).step_by(size) {
                for k in 0..half {
                    let (wr, wi) = self.tw[k * step];
                    let a = start + k;
                    let b = a + half;
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            size *= 2;
        }
    }
}

/// DCT-IV of length `m`: `X[k] = sum v[n] cos(pi/m * (n + 1/2) * (k + 1/2))`.
struct Dct4 {
    m: usize,
    fft: Fft,
    pre: Vec<(f32, f32)>,
    post: Vec<(f32, f32)>,
}

impl Dct4 {
    fn new(m: usize) -> Self {
        let h = m / 2;
        let pre = (0..h).map(|n| angle(-PI * n as f64 / m as f64)).collect();
        let post = (0..h).map(|k| angle(-PI * (k as f64 + 0.25) / m as f64)).collect();
        Self { m, fft: Fft::new(h), pre, post }
    }

    fn run(&self, v: &[f32], out: &mut [f32], re: &mut Vec<f32>, im: &mut Vec<f32>) {
        let m = self.m;
        let h = m / 2;
        re.resize(h, 0.0);
        im.resize(h, 0.0);
        for n in 0..h {
            let (a, b) = (v[2 * n], v[m - 1 - 2 * n]);
            let (c, s) = self.pre[n];
            re[n] = a * c - b * s;
            im[n] = a * s + b * c;
        }
        self.fft.run(re, im);
        for k in 0..h {
            let (c, s) = self.post[k];
            let yr = re[k] * c - im[k] * s;
            let yi = re[k] * s + im[k] * c;
            out[2 * k] = yr;
            out[m - 1 - 2 * k] = -yi;
        }
    }
}

fn angle(a: f64) -> (f32, f32) {
    (a.cos() as f32, a.sin() as f32)
}

/// Forward MDCT of `n` windowed samples to `n/2` coefficients.
pub struct Mdct {
    n: usize,
    dct: Dct4,
    fold: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
}

impl Mdct {
    pub fn new(n: usize) -> Self {
        Self { n, dct: Dct4::new(n / 2), fold: vec![0.0; n / 2], re: Vec::new(), im: Vec::new() }
    }

    /// `z` holds `n` already windowed samples; `out` receives `n/2` coefficients.
    pub fn forward(&mut self, z: &[f32], out: &mut [f32]) {
        let n = self.n;
        let q = n / 4;
        // (a, b, c, d) quarters -> (-c_reversed - d, a - b_reversed)
        for i in 0..q {
            self.fold[i] = -z[3 * q - 1 - i] - z[3 * q + i];
            self.fold[q + i] = z[i] - z[2 * q - 1 - i];
        }
        self.dct.run(&self.fold, out, &mut self.re, &mut self.im);
        for x in out[..n / 2].iter_mut() {
            *x *= 2.0;
        }
    }

    /// Inverse MDCT with the decoder's `2/N` scaling: `n/2` coefficients to `n` samples
    /// (not yet windowed).
    #[cfg(test)]
    pub fn inverse(&mut self, x: &[f32], out: &mut [f32]) {
        let n = self.n;
        let q = n / 4;
        let mut w = vec![0.0f32; n / 2];
        self.dct.run(x, &mut w, &mut self.re, &mut self.im);
        let s = 2.0 / n as f32;
        // (v1, v2) -> (v2, -v2_reversed, -v1_reversed, -v1)
        for i in 0..q {
            out[i] = w[q + i] * s;
            out[q + i] = -w[2 * q - 1 - i] * s;
            out[2 * q + i] = -w[q - 1 - i] * s;
            out[3 * q + i] = -w[i] * s;
        }
    }
}

/// Window shape, as the `window_shape` bit says.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    Sine = 0,
    Kbd = 1,
}

/// Rising halves of the four windows (long and short, sine and KBD); the falling half is the
/// same table read backwards.
pub struct Windows {
    long: [Vec<f32>; 2],
    short: [Vec<f32>; 2],
}

impl Windows {
    pub fn new() -> Self {
        Self { long: [sine_half(2048), kbd_half(2048, 4.0)], short: [sine_half(256), kbd_half(256, 6.0)] }
    }

    /// Rising half of length 1024 (long) or 128 (short).
    pub fn rise(&self, shape: Shape, short: bool) -> &[f32] {
        if short { &self.short[shape as usize] } else { &self.long[shape as usize] }
    }
}

fn sine_half(n: usize) -> Vec<f32> {
    (0..n / 2).map(|i| (PI / n as f64 * (i as f64 + 0.5)).sin() as f32).collect()
}

/// Kaiser-Bessel-derived window (rising half), as the standard defines it.
fn kbd_half(n: usize, alpha: f64) -> Vec<f32> {
    let h = n / 2;
    let q = n as f64 / 4.0;
    let kaiser: Vec<f64> = (0..=h)
        .map(|p| {
            let t = (p as f64 - q) / q;
            bessel_i0(PI * alpha * (1.0 - t * t).max(0.0).sqrt())
        })
        .collect();
    let total: f64 = kaiser.iter().sum();
    let mut acc = 0.0;
    (0..h)
        .map(|i| {
            acc += kaiser[i];
            (acc / total).sqrt() as f32
        })
        .collect()
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let y = x * x / 4.0;
    for k in 1..60 {
        term *= y / (k * k) as f64;
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// What the frame's window looks like over its 2048 samples.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sequence {
    OnlyLong = 0,
    LongStart = 1,
    EightShort = 2,
    LongStop = 3,
}

/// The full 2048-sample window of a long-type frame (`OnlyLong`, `LongStart`, `LongStop`):
/// the left half uses the previous frame's shape, the right half this frame's.
pub fn long_window(w: &Windows, seq: Sequence, prev: Shape, cur: Shape, out: &mut [f32]) {
    debug_assert!(seq != Sequence::EightShort);
    let (lr, sr) = (w.rise(prev, false), w.rise(prev, true));
    let (lc, sc) = (w.rise(cur, false), w.rise(cur, true));
    match seq {
        Sequence::LongStop => {
            out[..448].fill(0.0);
            out[448..576].copy_from_slice(sr);
            out[576..1024].fill(1.0);
        }
        _ => out[..1024].copy_from_slice(lr),
    }
    match seq {
        Sequence::LongStart => {
            out[1024..1472].fill(1.0);
            for i in 0..128 {
                out[1472 + i] = sc[127 - i];
            }
            out[1600..].fill(0.0);
        }
        _ => {
            for i in 0..1024 {
                out[1024 + i] = lc[1023 - i];
            }
        }
    }
}

/// The 256-sample window of short window `index` (0..8) in an `EightShort` frame.
pub fn short_window(w: &Windows, index: usize, prev: Shape, cur: Shape, out: &mut [f32]) {
    let left = w.rise(if index == 0 { prev } else { cur }, true);
    let right = w.rise(cur, true);
    out[..128].copy_from_slice(left);
    for i in 0..128 {
        out[128 + i] = right[127 - i];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn direct_mdct(z: &[f32]) -> Vec<f64> {
        let n = z.len();
        let n0 = (n as f64 / 2.0 + 1.0) / 2.0;
        (0..n / 2)
            .map(|k| 2.0 * (0..n).map(|i| z[i] as f64 * (2.0 * PI / n as f64 * (i as f64 + n0) * (k as f64 + 0.5)).cos()).sum::<f64>())
            .collect()
    }

    fn noise(len: usize, seed: u32) -> Vec<f32> {
        let mut s = seed;
        (0..len)
            .map(|_| {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                (s >> 8) as f32 / (1 << 24) as f32 * 2.0 - 1.0
            })
            .collect()
    }

    #[test]
    fn fast_mdct_matches_the_definition() {
        for n in [256usize, 2048] {
            let z = noise(n, 7);
            let mut m = Mdct::new(n);
            let mut out = vec![0.0; n / 2];
            m.forward(&z, &mut out);
            let want = direct_mdct(&z);
            let peak = want.iter().fold(0.0f64, |a, b| a.max(b.abs()));
            for k in 0..n / 2 {
                assert!((out[k] as f64 - want[k]).abs() < peak * 1e-5, "n={n} k={k}: {} vs {}", out[k], want[k]);
            }
        }
    }

    #[test]
    fn windows_meet_princen_bradley() {
        let w = Windows::new();
        for shape in [Shape::Sine, Shape::Kbd] {
            for short in [false, true] {
                let r = w.rise(shape, short);
                let h = r.len();
                for i in 0..h {
                    // w(i)^2 + w(i + N/2)^2 = 1, where the falling half is the rise reversed.
                    let s = r[i] * r[i] + r[h - 1 - i] * r[h - 1 - i];
                    assert!((s - 1.0).abs() < 1e-5, "{shape:?} short={short} i={i}: {s}");
                }
            }
        }
    }

    /// Window, transform, inverse transform, window again and overlap-add: the signal comes
    /// back (time-domain aliasing cancellation), across every allowed sequence change and both
    /// window shapes.
    #[test]
    fn tdac_reconstructs_across_block_switches() {
        use Sequence::*;
        let seqs = [OnlyLong, LongStart, EightShort, EightShort, LongStop, OnlyLong, LongStart, EightShort, LongStop, OnlyLong];
        let shapes = [Shape::Sine, Shape::Kbd, Shape::Kbd, Shape::Sine, Shape::Sine, Shape::Kbd, Shape::Sine, Shape::Kbd, Shape::Kbd, Shape::Sine];
        let frames = seqs.len();
        let x = noise(1024 * (frames + 1), 3);
        let w = Windows::new();
        let mut long = Mdct::new(2048);
        let mut short = Mdct::new(256);
        let mut out = vec![0.0f32; 1024 * (frames + 1)];
        let mut prev = Shape::Sine;
        for f in 0..frames {
            let block = &x[f * 1024..f * 1024 + 2048];
            let mut y = vec![0.0f32; 2048];
            if seqs[f] == EightShort {
                for wi in 0..8 {
                    let mut win = vec![0.0; 256];
                    short_window(&w, wi, prev, shapes[f], &mut win);
                    let z: Vec<f32> = (0..256).map(|i| block[448 + wi * 128 + i] * win[i]).collect();
                    let mut c = vec![0.0; 128];
                    short.forward(&z, &mut c);
                    let mut t = vec![0.0; 256];
                    short.inverse(&c, &mut t);
                    for i in 0..256 {
                        y[448 + wi * 128 + i] += t[i] * win[i];
                    }
                }
            } else {
                let mut win = vec![0.0; 2048];
                long_window(&w, seqs[f], prev, shapes[f], &mut win);
                let z: Vec<f32> = (0..2048).map(|i| block[i] * win[i]).collect();
                let mut c = vec![0.0; 1024];
                long.forward(&z, &mut c);
                let mut t = vec![0.0; 2048];
                long.inverse(&c, &mut t);
                for i in 0..2048 {
                    y[i] = t[i] * win[i];
                }
            }
            for i in 0..2048 {
                out[f * 1024 + i] += y[i];
            }
            prev = shapes[f];
        }
        // Every sample covered by two frames is reconstructed.
        for i in 1024..1024 * frames {
            assert!((out[i] - x[i]).abs() < 1e-4, "sample {i}: {} vs {}", out[i], x[i]);
        }
    }
}
