//! Snagster's own AAC-LC audio encoder (48 kHz stereo), written from the description in
//! ISO/IEC 14496-3. Linux has no permissively licensed AAC encoder, and streaming services
//! want AAC sound, so this is the fallback on every system and the only encoder on Linux.
//!
//! What it does, frame by frame (1024 samples per channel):
//! - a transient detector on the next block chooses long windows or eight short windows
//!   (with the start/stop windows between them); long windows use the sine shape, short ones
//!   the Kaiser-Bessel-derived shape;
//! - the MDCT turns the windowed block into 1024 (or 8 x 128) coefficients;
//! - a simple masking model (band energy, spreading to neighbouring bands, threshold in quiet)
//!   says how much noise each scalefactor band may carry, mid/side stereo is chosen per band
//!   when it needs fewer bits;
//! - one global offset on all scalefactors is searched so the frame fits its bit budget, which
//!   comes from the average bitrate and a bit reservoir (never more than 6144 bits per channel);
//! - the quantised values (`|x|^(3/4)`) are Huffman coded with the standard's codebooks, the
//!   best codebook per band and the sections between them chosen by dynamic programming.
//!
//! The output is raw AAC frames (no ADTS header) plus the two-byte AudioSpecificConfig; the
//! first [`AacEncoder::priming_samples`] decoded samples are the encoder delay.

mod bits;
mod mdct;
mod tables;
#[cfg(test)]
mod testdec;

use bits::BitWriter;
use mdct::{Mdct, Sequence, Shape, Windows};
use tables::{BOOKS, SF_BITS, SF_CODES, SWB_LONG, SWB_SHORT};

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
const FRAME: usize = 1024;
/// The decoder's buffer, and so the largest frame: 6144 bits per channel.
const MAX_BITS_PER_CHANNEL: usize = 6144;
const SF_OFFSET: i32 = 100;
const MAX_Q: i32 = 8191;
/// Rounding offset of the quantiser (less than 0.5 because of the `x^(3/4)` companding).
const ROUND: f32 = 0.4054;

pub struct AacEncoder {
    bitrate: u32,
    mean_bits: f64,
    reservoir: f64,
    reservoir_max: f64,
    /// Samples per channel, starting at the current block (2048 for this frame plus 1024 to
    /// look ahead for attacks).
    buf: [Vec<f32>; CHANNELS],
    /// A left sample whose right partner hasn't arrived yet.
    odd: Option<i16>,
    samples_in: u64,
    frames_out: u64,
    finished: bool,
    prev_seq: Sequence,
    prev_shape: Shape,
    attack_next: bool,
    max_sfb_long: usize,
    max_sfb_short: usize,
    ath_long: Vec<f32>,
    ath_short: Vec<f32>,
    last_delta: i32,
    win: Windows,
    mdct_long: Mdct,
    mdct_short: Mdct,
}

impl AacEncoder {
    /// A 48 kHz stereo AAC-LC encoder aiming at `bitrate_bps` on average (64 to 320 kbit/s).
    pub fn new(bitrate_bps: u32) -> Result<Self, String> {
        if !(64_000..=320_000).contains(&bitrate_bps) {
            return Err(format!("AAC bitrate must be between 64 and 320 kbit/s, not {}", bitrate_bps / 1000));
        }
        let mean_bits = bitrate_bps as f64 * FRAME as f64 / SAMPLE_RATE as f64;
        let cutoff = cutoff_hz(bitrate_bps);
        let max_sfb_long = bands_below(&SWB_LONG, cutoff, FRAME);
        let max_sfb_short = bands_below(&SWB_SHORT, cutoff, FRAME / 8);
        Ok(Self {
            bitrate: bitrate_bps,
            mean_bits,
            reservoir: 0.0,
            reservoir_max: (MAX_BITS_PER_CHANNEL * CHANNELS) as f64 - mean_bits,
            buf: [vec![0.0; FRAME], vec![0.0; FRAME]],
            odd: None,
            samples_in: 0,
            frames_out: 0,
            finished: false,
            prev_seq: Sequence::OnlyLong,
            prev_shape: Shape::Sine,
            attack_next: false,
            max_sfb_long,
            max_sfb_short,
            ath_long: ath_bands(&SWB_LONG, FRAME),
            ath_short: ath_bands(&SWB_SHORT, FRAME / 8),
            last_delta: 0,
            win: Windows::new(),
            mdct_long: Mdct::new(2 * FRAME),
            mdct_short: Mdct::new(2 * FRAME / 8),
        })
    }

    /// The average bitrate this encoder aims at.
    pub fn bitrate(&self) -> u32 {
        self.bitrate
    }

    /// AudioSpecificConfig: AAC-LC, 48 kHz, two channels.
    pub fn audio_specific_config() -> [u8; 2] {
        // 5 bits object type 2, 4 bits frequency index 3, 4 bits channel configuration 2, 3 zero bits.
        let v: u16 = (2 << 11) | (3 << 7) | (2 << 3);
        v.to_be_bytes()
    }

    /// How many samples (per channel) at the start of the decoded sound are encoder delay.
    #[cfg(test)]
    pub fn priming_samples() -> u32 {
        FRAME as u32
    }

    /// Feed interleaved stereo samples (any number); returns the frames that are complete.
    pub fn encode(&mut self, pcm: &[i16]) -> Vec<Vec<u8>> {
        if self.finished {
            return Vec::new();
        }
        let mut it = pcm.iter().copied();
        if let Some(l) = self.odd.take() {
            match it.next() {
                Some(r) => self.push(l, r),
                None => {
                    self.odd = Some(l);
                    return Vec::new();
                }
            }
        }
        loop {
            match (it.next(), it.next()) {
                (Some(l), Some(r)) => self.push(l, r),
                (Some(l), None) => {
                    self.odd = Some(l);
                    break;
                }
                _ => break,
            }
        }
        let mut out = Vec::new();
        while self.buf[0].len() >= 3 * FRAME {
            out.push(self.frame());
        }
        out
    }

    /// Pad with silence and return the remaining frames, so every sample fed in comes out of a
    /// decoder (after the priming samples).
    pub fn finish(&mut self) -> Vec<Vec<u8>> {
        if self.finished {
            return Vec::new();
        }
        if let Some(l) = self.odd.take() {
            self.push(l, 0);
        }
        let needed = (self.samples_in + FRAME as u64).div_ceil(FRAME as u64);
        let mut out = Vec::new();
        while self.frames_out < needed {
            for b in self.buf.iter_mut() {
                b.resize(b.len().max(3 * FRAME), 0.0);
            }
            out.push(self.frame());
        }
        self.finished = true;
        out
    }

    fn push(&mut self, l: i16, r: i16) {
        self.buf[0].push(l as f32);
        self.buf[1].push(r as f32);
        self.samples_in += 1;
    }

    /// Encode the block at the start of the buffer and move on by one frame.
    fn frame(&mut self) -> Vec<u8> {
        let attack = self.attack_next;
        self.attack_next = detect_attack(&self.buf, FRAME);
        let seq = match self.prev_seq {
            Sequence::LongStart => Sequence::EightShort,
            Sequence::EightShort if attack || self.attack_next => Sequence::EightShort,
            Sequence::EightShort => Sequence::LongStop,
            _ if self.attack_next => Sequence::LongStart,
            _ => Sequence::OnlyLong,
        };
        let shape = if seq == Sequence::EightShort { Shape::Kbd } else { Shape::Sine };
        let bytes = self.encode_block(seq, shape);
        self.prev_seq = seq;
        self.prev_shape = shape;
        for b in self.buf.iter_mut() {
            b.drain(..FRAME);
        }
        self.frames_out += 1;
        bytes
    }

    fn encode_block(&mut self, seq: Sequence, shape: Shape) -> Vec<u8> {
        let short = seq == Sequence::EightShort;
        // Transform.
        let mut spec = [vec![0.0f32; FRAME], vec![0.0f32; FRAME]];
        let mut z = vec![0.0f32; 2 * FRAME];
        let mut w = vec![0.0f32; 2 * FRAME];
        for c in 0..CHANNELS {
            let x = &self.buf[c];
            if short {
                for i in 0..8 {
                    mdct::short_window(&self.win, i, self.prev_shape, shape, &mut w[..256]);
                    let at = 448 + i * 128;
                    for k in 0..256 {
                        z[k] = x[at + k] * w[k];
                    }
                    self.mdct_short.forward(&z[..256], &mut spec[c][i * 128..(i + 1) * 128]);
                }
            } else {
                mdct::long_window(&self.win, seq, self.prev_shape, shape, &mut w);
                for k in 0..2 * FRAME {
                    z[k] = x[k] * w[k];
                }
                self.mdct_long.forward(&z, &mut spec[c]);
            }
        }
        let groups = if short { group_windows(&spec) } else { vec![1] };
        let layout = Layout::new(short, &groups, if short { self.max_sfb_short } else { self.max_sfb_long });
        let mut coef = [layout.order(&spec[0]), layout.order(&spec[1])];
        let ath = if short { &self.ath_short } else { &self.ath_long };
        let mut thr = [thresholds(&layout, &coef[0], ath), thresholds(&layout, &coef[1], ath)];
        let ms = choose_ms(&layout, &mut coef, &mut thr);
        let chans = [Channel::analyse(&layout, &coef[0], &thr[0]), Channel::analyse(&layout, &coef[1], &thr[1])];

        // Budget from the average and the reservoir.
        let max_frame = (self.mean_bits + self.reservoir).min((MAX_BITS_PER_CHANNEL * CHANNELS) as f64);
        let share = if short { 0.5 } else { 0.2 };
        let budget = (self.mean_bits + share * self.reservoir).min(max_frame) as usize;

        // Finest common scalefactor offset that fits the budget.
        let fixed = fixed_bits(&layout, &ms);
        let bits_at = |d: i32| -> usize { fixed + chans.iter().map(|c| c.quantise(&layout, d).bits).sum::<usize>() };
        let (mut lo, mut hi) = (-120i32, 120i32);
        let start = self.last_delta.clamp(lo + 1, hi - 1);
        if bits_at(start) <= budget {
            hi = start;
        } else {
            lo = start;
        }
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if bits_at(mid) <= budget {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        self.last_delta = hi;
        let q = [chans[0].quantise(&layout, hi), chans[1].quantise(&layout, hi)];
        let bytes = write_frame(&layout, seq, shape, &groups, &ms, &q);
        let used = bytes.len() as f64 * 8.0;
        debug_assert!(used <= max_frame + 8.0, "frame of {used} bits over {max_frame}");
        self.reservoir = (self.reservoir + self.mean_bits - used).clamp(0.0, self.reservoir_max);
        bytes
    }
}

/// ADTS header (7 bytes, no CRC) for a raw AAC-LC frame of `payload_len` bytes.
/// `freq_index` is the standard's sampling frequency index (3 = 48 kHz).
#[cfg(test)]
pub fn adts_header(payload_len: usize, channels: u8, freq_index: u8) -> [u8; 7] {
    let len = payload_len + 7;
    let mut w = BitWriter::new();
    w.put(0xFFF, 12); // syncword
    w.put(0, 1); // MPEG-4
    w.put(0, 2); // layer
    w.put(1, 1); // no CRC
    w.put(1, 2); // profile: AAC LC (object type 2) minus one
    w.put(freq_index as u32, 4);
    w.put(0, 1); // private
    w.put(channels as u32, 3);
    w.put(0, 4); // original/copy, home, copyright bits
    w.put(len as u32, 13);
    w.put(0x7FF, 11); // buffer fullness: variable bitrate
    w.put(0, 2); // one raw data block
    let v = w.finish();
    [v[0], v[1], v[2], v[3], v[4], v[5], v[6]]
}

/// Audio bandwidth for a bitrate: lower rates leave out the top octave.
fn cutoff_hz(bitrate: u32) -> f32 {
    let pts = [(64_000.0, 12_000.0), (96_000.0, 15_000.0), (128_000.0, 16_500.0), (160_000.0, 18_000.0), (192_000.0, 19_500.0), (256_000.0, 20_000.0)];
    let b = bitrate as f32;
    if b <= pts[0].0 {
        return pts[0].1;
    }
    for p in pts.windows(2) {
        if b <= p[1].0 {
            let t = (b - p[0].0) / (p[1].0 - p[0].0);
            return p[0].1 + t * (p[1].1 - p[0].1);
        }
    }
    pts[pts.len() - 1].1
}

/// Number of scalefactor bands that start below `hz`.
fn bands_below(swb: &[u16], hz: f32, lines: usize) -> usize {
    let hz_per_line = SAMPLE_RATE as f32 / 2.0 / lines as f32;
    (0..swb.len() - 1).take_while(|&b| (swb[b] as f32 * hz_per_line) < hz).count()
}

/// Threshold in quiet per band, in the encoder's MDCT units (energy over the band).
fn ath_bands(swb: &[u16], lines: usize) -> Vec<f32> {
    // A full-scale sine gives coefficients of about 32768 * lines * 0.6.
    let full = 32768.0 * lines as f64 * 0.6;
    let hz_per_line = SAMPLE_RATE as f64 / 2.0 / lines as f64;
    (0..swb.len() - 1)
        .map(|b| {
            let (lo, hi) = (swb[b] as usize, swb[b + 1] as usize);
            let db = (lo..hi).map(|k| ath_db((k as f64 + 0.5) * hz_per_line)).fold(f64::MAX, f64::min);
            (full * full * 10f64.powf((db - 95.0) / 10.0) * (hi - lo) as f64) as f32
        })
        .collect()
}

/// Terhardt's approximation of the threshold in quiet, in dB SPL.
fn ath_db(hz: f64) -> f64 {
    let f = (hz / 1000.0).max(0.02);
    3.64 * f.powf(-0.8) - 6.5 * (-0.6 * (f - 3.3) * (f - 3.3)).exp() + 1e-3 * f.powi(4)
}

/// Does the block one frame ahead start with a sudden rise in loudness?
/// Looks at the high-passed signal in 128-sample pieces where its short windows sit.
fn detect_attack(buf: &[Vec<f32>; CHANNELS], ahead: usize) -> bool {
    // Pieces from 512 before the first short window of the next block to its last one.
    let from = ahead + 448 - 512;
    let pieces = (1600 + 512 - 448) / 128;
    let mut e = vec![0.0f32; pieces];
    for x in buf {
        for (p, ep) in e.iter_mut().enumerate() {
            let s = from + p * 128;
            let mut acc = 0.0;
            for i in s..s + 128 {
                let d = x[i] - x[i - 1];
                acc += d * d;
            }
            *ep = ep.max(acc);
        }
    }
    (4..pieces).any(|p| {
        let before = e[p - 4..p].iter().sum::<f32>() / 4.0;
        e[p] > 10.0 * before && e[p] > 128.0 * 400.0
    })
}

/// Group consecutive short windows of similar loudness (each group shares scalefactors).
fn group_windows(spec: &[Vec<f32>; CHANNELS]) -> Vec<usize> {
    let e: Vec<f32> = (0..8).map(|w| spec.iter().map(|s| s[w * 128..(w + 1) * 128].iter().map(|v| v * v).sum::<f32>()).sum::<f32>() + 1.0).collect();
    let mut groups = vec![1usize];
    for w in 1..8 {
        let r = e[w] / e[w - 1];
        if (0.25..=4.0).contains(&r) {
            *groups.last_mut().unwrap() += 1;
        } else {
            groups.push(1);
        }
    }
    groups
}

/// The scalefactor bands of one frame in bitstream order. For short windows each band of a
/// group holds that band of every window in the group, one window after the other.
struct Layout {
    short: bool,
    max_sfb: usize,
    /// (group, sfb, start, end) in the ordered coefficient array.
    bands: Vec<(usize, usize, usize, usize)>,
    groups: Vec<usize>,
}

impl Layout {
    fn new(short: bool, groups: &[usize], max_sfb: usize) -> Self {
        let swb: &[u16] = if short { &SWB_SHORT } else { &SWB_LONG };
        let mut bands = Vec::new();
        let mut at = 0;
        for (g, &len) in groups.iter().enumerate() {
            for b in 0..max_sfb {
                let width = (swb[b + 1] - swb[b]) as usize * len;
                bands.push((g, b, at, at + width));
                at += width;
            }
        }
        Self { short, max_sfb, bands, groups: groups.to_vec() }
    }

    /// Coefficients reordered into bitstream order (and cut at `max_sfb`).
    fn order(&self, spec: &[f32]) -> Vec<f32> {
        if !self.short {
            return spec[..SWB_LONG[self.max_sfb] as usize].to_vec();
        }
        let mut out = Vec::with_capacity(FRAME);
        let mut w0 = 0;
        for &len in &self.groups {
            for b in 0..self.max_sfb {
                for w in w0..w0 + len {
                    out.extend_from_slice(&spec[w * 128 + SWB_SHORT[b] as usize..w * 128 + SWB_SHORT[b + 1] as usize]);
                }
            }
            w0 += len;
        }
        out
    }

    fn width(&self, b: usize) -> usize {
        self.bands[b].3 - self.bands[b].2
    }
}

/// Noise allowed in each band before the global offset: the band's energy spread a little to
/// its neighbours (a loud band hides noise just above and below it) some way down, never
/// below the threshold in quiet. 0 marks a band that can't be heard at all.
fn thresholds(l: &Layout, coef: &[f32], ath: &[f32]) -> Vec<f32> {
    // Noise this far below the (spread) band energy at offset 0; the rate control moves it.
    const BASE: f32 = 0.0158; // -18 dB
    let e: Vec<f32> = l.bands.iter().map(|&(_, _, s, t)| coef[s..t].iter().map(|v| v * v).sum()).collect();
    let n = e.len();
    let mut out = vec![0.0; n];
    for b in 0..n {
        let (g, sfb, _, _) = l.bands[b];
        let per = |i: usize| e[i] / l.width(i) as f32 * l.width(b) as f32;
        let mut m = e[b];
        if b > 0 && l.bands[b - 1].0 == g {
            m = m.max(per(b - 1) * 0.03);
        }
        if b + 1 < n && l.bands[b + 1].0 == g {
            m = m.max(per(b + 1) * 0.01);
        }
        let quiet = ath[sfb] * l.groups[g] as f32;
        let (_, _, s, t) = l.bands[b];
        // Noise hides noise better than a tone does: up to 6 dB more for a flat band.
        let noisy = 4f32.powf(1.0 - tonality(&coef[s..t], e[b]));
        out[b] = if e[b] <= quiet { 0.0 } else { (m * BASE * noisy).max(quiet) };
    }
    out
}

/// 1 for a band whose energy sits in a few lines (a tone), 0 for an even spread (noise),
/// from the spectral flatness (geometric over arithmetic mean of the line energies).
fn tonality(x: &[f32], energy: f32) -> f32 {
    if energy <= 0.0 || x.len() < 2 {
        return 1.0;
    }
    let floor = energy / x.len() as f32 * 1e-6;
    let mean_log = x.iter().map(|v| (v * v + floor).ln()).sum::<f32>() / x.len() as f32;
    let flatness_db = 10.0 / std::f32::consts::LN_10 * (mean_log - (energy / x.len() as f32).ln());
    // Gaussian noise measures about -5.5 dB (the lines' energies scatter), a clear tone -18 or less.
    ((-flatness_db - 6.0) / 12.0).clamp(0.0, 1.0)
}

/// Turn bands into mid/side where that should need fewer bits (common window only).
/// Returns the per-band flags.
fn choose_ms(l: &Layout, coef: &mut [Vec<f32>; CHANNELS], thr: &mut [Vec<f32>; CHANNELS]) -> Vec<bool> {
    let mut flags = vec![false; l.bands.len()];
    for (b, &(_, _, s, t)) in l.bands.iter().enumerate() {
        let (mut el, mut er, mut em, mut es) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for i in s..t {
            let (a, c) = (coef[0][i], coef[1][i]);
            el += a * a;
            er += c * c;
            let (m, d) = ((a + c) * 0.5, (a - c) * 0.5);
            em += m * m;
            es += d * d;
        }
        let tl = thr[0][b];
        let tr = thr[1][b];
        if tl <= 0.0 || tr <= 0.0 {
            continue;
        }
        let tm = tl.min(tr) * 0.5;
        // Roughly the bits a band needs: more the further its energy is above the allowed noise.
        let cost = |e: f32, t: f32| (1.0 + e / t).log2();
        if cost(em, tm) + cost(es, tm) < cost(el, tl) + cost(er, tr) {
            flags[b] = true;
            for i in s..t {
                let (a, c) = (coef[0][i], coef[1][i]);
                coef[0][i] = (a + c) * 0.5;
                coef[1][i] = (a - c) * 0.5;
            }
            thr[0][b] = tm;
            thr[1][b] = tm;
        }
    }
    flags
}

/// One channel's coefficients ready to be quantised at any global offset.
struct Channel {
    /// `|x|^(3/4)` and the sign of every coefficient.
    a34: Vec<f32>,
    neg: Vec<bool>,
    /// Scalefactor per band before the global offset (None: the band is masked or silent).
    base_sf: Vec<Option<f32>>,
    /// Smallest scalefactor that keeps every value of the band at or below 8191.
    min_sf: Vec<i32>,
}

/// One channel quantised at one global offset.
struct Quantised {
    q: Vec<i32>,
    sf: Vec<i32>,
    books: Vec<u8>,
    bits: usize,
}

impl Channel {
    fn analyse(l: &Layout, coef: &[f32], thr: &[f32]) -> Self {
        let a34: Vec<f32> = coef.iter().map(|v| v.abs().powf(0.75)).collect();
        let neg = coef.iter().map(|v| *v < 0.0).collect();
        let mut base_sf = Vec::with_capacity(l.bands.len());
        let mut min_sf = Vec::with_capacity(l.bands.len());
        for (b, &(_, _, s, t)) in l.bands.iter().enumerate() {
            let mut e = 0.0f32;
            let mut root = 0.0f32;
            let mut peak = 0.0f32;
            for v in &coef[s..t] {
                let a = v.abs();
                e += a * a;
                root += a.sqrt();
                peak = peak.max(a);
            }
            // The quantiser adds about (4/27) * A^1.5 * sum(sqrt|x|) of noise with step A.
            base_sf.push(if thr[b] > 0.0 && e > 0.0 {
                let a15 = thr[b] * 27.0 / (4.0 * root);
                Some(SF_OFFSET as f32 + 8.0 / 3.0 * a15.log2())
            } else {
                None
            });
            let need = if peak > 0.0 { SF_OFFSET as f32 + 4.0 * (peak / (MAX_Q as f32).powf(4.0 / 3.0)).log2() } else { 0.0 };
            min_sf.push((need.ceil() as i32 + 1).max(0));
        }
        Self { a34, neg, base_sf, min_sf }
    }

    fn quantise(&self, l: &Layout, delta: i32) -> Quantised {
        let n = l.bands.len();
        let mut q = vec![0i32; self.a34.len()];
        let mut sf = vec![0i32; n];
        let mut zero = vec![true; n];
        let mut prev: Option<i32> = None;
        for b in 0..n {
            let Some(base) = self.base_sf[b] else { continue };
            let mut s = (base.round() as i32 + delta).clamp(self.min_sf[b], 255);
            if let Some(p) = prev {
                s = s.clamp(p - 60, p + 60);
            }
            let (_, _, st, en) = l.bands[b];
            let scale = 2f32.powf(-0.1875 * (s - SF_OFFSET) as f32);
            let mut any = false;
            for i in st..en {
                let v = ((self.a34[i] * scale + ROUND) as i32).min(MAX_Q);
                q[i] = if self.neg[i] { -v } else { v };
                any |= v != 0;
            }
            if any {
                zero[b] = false;
                sf[b] = s;
                prev = Some(s);
            }
        }
        let (books, spectral) = choose_books(l, &q, &zero);
        // A band coded with a real codebook but holding only zeros repeats the scalefactor
        // before it (or, at the start, the first real one), so it costs one bit.
        let mut cur = (0..n).find(|&b| !zero[b]).map(|b| sf[b]).unwrap_or(SF_OFFSET);
        for b in 0..n {
            if books[b] == 0 {
                continue;
            }
            if zero[b] {
                sf[b] = cur;
            }
            cur = sf[b];
        }
        let bits = spectral + sf_bits(&sf, &books) + section_bits(l, &books) + 8 + 3;
        Quantised { q, sf, books, bits }
    }
}

/// The global gain: the scalefactor of the first band that is sent.
fn first_sf(sf: &[i32], books: &[u8]) -> i32 {
    (0..sf.len()).find(|&b| books[b] != 0).map(|b| sf[b]).unwrap_or(SF_OFFSET)
}

/// Bits of the spectral data of `q` in `book` (None: a value is too big for the book).
fn book_bits(book: usize, q: &[i32]) -> Option<usize> {
    let cb = &BOOKS[book];
    let mut bits = 0usize;
    for t in q.chunks_exact(cb.dim) {
        let (idx, extra) = cb.index(t)?;
        bits += cb.bits[idx] as usize + extra;
    }
    Some(bits)
}

/// Pick a codebook per band and the sections between them (dynamic programming over bands),
/// returning the books and the bits of the spectral data.
fn choose_books(l: &Layout, q: &[i32], zero: &[bool]) -> (Vec<u8>, usize) {
    let n = l.bands.len();
    let mut books = vec![0u8; n];
    let mut total = 0;
    let len_bits = if l.short { 3 } else { 5 };
    let switch = 4 + len_bits;
    const INF: usize = usize::MAX / 4;
    let per_group = l.max_sfb;
    for g in 0..l.groups.len() {
        let range = g * per_group..(g + 1) * per_group;
        // cost[b][k]: bits of bands up to b with band b in book k.
        let mut cost: Vec<[usize; 12]> = Vec::with_capacity(per_group);
        let mut from: Vec<[u8; 12]> = Vec::with_capacity(per_group);
        let mut own: Vec<[usize; 12]> = Vec::with_capacity(per_group);
        for b in range.clone() {
            let (_, _, s, t) = l.bands[b];
            let band = &q[s..t];
            let mut c = [INF; 12];
            if zero[b] {
                c[0] = 0;
                for (k, ck) in c.iter_mut().enumerate().skip(1) {
                    // Zeros in a real codebook, plus the scalefactor that then must be sent.
                    *ck = BOOKS[k].zero_bits() * band.len() / BOOKS[k].dim + 1;
                }
            } else {
                let peak = band.iter().map(|v| v.abs()).max().unwrap_or(0);
                let first = BOOKS.iter().position(|cb| cb.dim > 0 && cb.lav >= peak).unwrap_or(11);
                // The smallest books that fit, and the escape book.
                for k in (first..12).take(4).chain(std::iter::once(11)) {
                    if let Some(bits) = book_bits(k, band) {
                        c[k] = bits;
                    }
                }
            }
            own.push(c);
            let i = b - range.start;
            let mut row = [INF; 12];
            let mut back = [0u8; 12];
            for k in 0..12 {
                if c[k] >= INF {
                    continue;
                }
                if i == 0 {
                    row[k] = c[k] + switch;
                    back[k] = k as u8;
                } else {
                    let prev = &cost[i - 1];
                    let (mut best, mut bk) = (prev[k], k);
                    for j in 0..12 {
                        if prev[j] + switch < best {
                            best = prev[j] + switch;
                            bk = j;
                        }
                    }
                    row[k] = best.saturating_add(c[k]);
                    back[k] = bk as u8;
                }
            }
            cost.push(row);
            from.push(back);
        }
        if per_group == 0 {
            continue;
        }
        let last = cost.len() - 1;
        let mut k = (0..12).min_by_key(|&k| cost[last][k]).unwrap();
        for i in (0..=last).rev() {
            books[range.start + i] = k as u8;
            total += own[i][k];
            k = from[i][k] as usize;
        }
    }
    // The DP added one scalefactor bit for zero bands in real books; sf_bits counts it exactly.
    for b in 0..n {
        if zero[b] && books[b] != 0 {
            total -= 1;
        }
    }
    (books, total)
}

/// Runs of equal codebooks within each group: (group, book, first band, count).
fn sections(l: &Layout, books: &[u8]) -> Vec<(usize, u8, usize, usize)> {
    let mut out = Vec::new();
    for g in 0..l.groups.len() {
        let base = g * l.max_sfb;
        let mut b = 0;
        while b < l.max_sfb {
            let k = books[base + b];
            let mut e = b + 1;
            while e < l.max_sfb && books[base + e] == k {
                e += 1;
            }
            out.push((g, k, b, e - b));
            b = e;
        }
    }
    out
}

fn section_bits(l: &Layout, books: &[u8]) -> usize {
    let (len_bits, esc) = if l.short { (3, 7) } else { (5, 31) };
    sections(l, books).iter().map(|&(_, _, _, n)| 4 + len_bits * (1 + n / esc)).sum()
}

fn sf_bits(sf: &[i32], books: &[u8]) -> usize {
    let mut prev = None;
    let mut bits = 0;
    for b in 0..sf.len() {
        if books[b] == 0 {
            continue;
        }
        let p = prev.unwrap_or(sf[b]);
        bits += SF_BITS[(sf[b] - p + 60) as usize] as usize;
        prev = Some(sf[b]);
    }
    bits
}

/// Bits of the frame outside the two channels' own data.
fn fixed_bits(l: &Layout, ms: &[bool]) -> usize {
    let ics_info = if l.short { 1 + 2 + 1 + 4 + 7 } else { 1 + 2 + 1 + 6 + 1 };
    let ms_bits = 2 + if ms.iter().all(|m| *m) || !ms.iter().any(|m| *m) { 0 } else { ms.len() };
    3 + 4 + 1 + ics_info + ms_bits + 3 + 7
}

/// The seven `scale_factor_grouping` bits: the bit for window w (1..8, first bit = window 1)
/// is set when w joins the group of window w - 1.
fn grouping_bits(groups: &[usize]) -> u32 {
    let mut bits = 0u32;
    let mut win = 0;
    for &len in groups {
        for i in 0..len {
            if win > 0 {
                bits = (bits << 1) | (i > 0) as u32;
            }
            win += 1;
        }
    }
    bits
}

fn write_frame(l: &Layout, seq: Sequence, shape: Shape, groups: &[usize], ms: &[bool], q: &[Quantised; CHANNELS]) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.put(1, 3); // ID_CPE
    w.put(0, 4); // element instance tag
    w.put(1, 1); // common window
    // ics_info
    w.put(0, 1);
    w.put(seq as u32, 2);
    w.put(shape as u32, 1);
    if l.short {
        w.put(l.max_sfb as u32, 4);
        w.put(grouping_bits(groups), 7);
    } else {
        w.put(l.max_sfb as u32, 6);
        w.put(0, 1); // no prediction
    }
    // Mid/side mask.
    let all = ms.iter().all(|m| *m) && !ms.is_empty();
    let none = !ms.iter().any(|m| *m);
    if none {
        w.put(0, 2);
    } else if all {
        w.put(2, 2);
    } else {
        w.put(1, 2);
        for &m in ms {
            w.put(m as u32, 1);
        }
    }
    for ch in q {
        write_channel(&mut w, l, ch);
    }
    w.put(7, 3); // ID_END
    w.finish()
}

fn write_channel(w: &mut BitWriter, l: &Layout, ch: &Quantised) {
    let gg = first_sf(&ch.sf, &ch.books);
    w.put(gg as u32, 8);
    // Section data.
    let (len_bits, esc) = if l.short { (3, 7) } else { (5, 31) };
    for (_, book, _, mut n) in sections(l, &ch.books) {
        w.put(book as u32, 4);
        while n >= esc {
            w.put(esc as u32, len_bits);
            n -= esc;
        }
        w.put(n as u32, len_bits);
    }
    // Scalefactors, each as the difference to the one before (the first to the global gain).
    let mut prev = gg;
    for b in 0..ch.sf.len() {
        if ch.books[b] == 0 {
            continue;
        }
        let d = (ch.sf[b] - prev + 60) as usize;
        w.put(SF_CODES[d], SF_BITS[d] as u32);
        prev = ch.sf[b];
    }
    w.put(0, 1); // pulse data
    w.put(0, 1); // TNS
    w.put(0, 1); // gain control
    // Spectral data.
    for (b, &(_, _, s, t)) in l.bands.iter().enumerate() {
        let book = ch.books[b] as usize;
        if book == 0 {
            continue;
        }
        let cb = &BOOKS[book];
        for tuple in ch.q[s..t].chunks_exact(cb.dim) {
            cb.write(w, tuple);
        }
    }
}

#[cfg(test)]
mod tests;
