//! A small AAC-LC decoder for the tests: it reads what the encoder writes (one channel pair
//! element per frame, long and short windows, mid/side) and turns it back into sound, so the
//! tests can check the encoder without any outside tool. It follows the standard's decoding
//! process on its own: Huffman decoding, inverse quantisation `q^(4/3) * 2^((sf-100)/4)`,
//! mid/side, inverse MDCT, windowing and overlap-add.

use super::mdct::{self, Mdct, Sequence, Shape, Windows};
use super::tables::{BOOKS, SF_BITS, SF_CODES, SWB_LONG, SWB_SHORT};
use std::collections::HashMap;

pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn bit(&mut self) -> Result<u32, String> {
        let byte = self.data.get(self.pos / 8).ok_or("read past the end of the frame")?;
        let b = (byte >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Ok(b as u32)
    }

    pub fn read(&mut self, n: u32) -> Result<u32, String> {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | self.bit()?;
        }
        Ok(v)
    }

    pub fn pos(&self) -> usize {
        self.pos
    }
}

/// Bit-by-bit Huffman decoding through a map of (length, code) to index.
pub struct Huffman {
    map: HashMap<(u32, u32), usize>,
}

impl Huffman {
    pub fn new(codes: &[u32], bits: &[u8]) -> Self {
        Self { map: codes.iter().zip(bits).enumerate().map(|(i, (&c, &b))| ((b as u32, c), i)).collect() }
    }

    pub fn decode(&self, r: &mut BitReader) -> Result<usize, String> {
        let mut code = 0;
        for len in 1..=19 {
            code = (code << 1) | r.bit()?;
            if let Some(&i) = self.map.get(&(len, code)) {
                return Ok(i);
            }
        }
        Err("no such codeword".into())
    }
}

/// Values of one tuple read from the bitstream with codebook `book`.
pub fn read_tuple(r: &mut BitReader, huff: &Huffman, book: usize) -> Result<Vec<i32>, String> {
    let cb = &BOOKS[book];
    let idx = huff.decode(r)? as i32;
    let m = match book {
        1..=4 => 3,
        5 | 6 => 9,
        7 | 8 => 8,
        9 | 10 => 13,
        _ => 17,
    };
    let mut v = vec![0i32; cb.dim];
    let mut rest = idx;
    for i in (0..cb.dim).rev() {
        v[i] = rest % m;
        rest /= m;
    }
    if cb.signed {
        for x in v.iter_mut() {
            *x -= (m - 1) / 2;
        }
        return Ok(v);
    }
    for x in v.iter_mut() {
        if *x != 0 && r.bit()? == 1 {
            *x = -*x;
        }
    }
    if book == 11 {
        for x in v.iter_mut() {
            if x.abs() == 16 {
                let mut n = 0;
                while r.bit()? == 1 {
                    n += 1;
                }
                let a = (1 << (n + 4)) + r.read(n + 4)? as i32;
                *x = if *x < 0 { -a } else { a };
            }
        }
    }
    Ok(v)
}

/// What one channel of a frame holds after parsing.
#[derive(Clone, Default)]
pub struct ChannelData {
    pub books: Vec<u8>,
    pub sf: Vec<i32>,
    /// Quantised values in window order (window w, line k at `w * 128 + k` for short windows).
    pub q: Vec<i32>,
}

pub struct Frame {
    pub seq: Sequence,
    pub shape: Shape,
    pub max_sfb: usize,
    pub groups: Vec<usize>,
    pub ms: Vec<bool>,
    pub ch: [ChannelData; 2],
}

pub struct Decoder {
    huff: Vec<Huffman>,
    sf: Huffman,
    win: Windows,
    long: Mdct,
    short: Mdct,
    prev_shape: Shape,
    overlap: [Vec<f32>; 2],
}

impl Decoder {
    pub fn new() -> Self {
        Self {
            huff: BOOKS.iter().map(|b| Huffman::new(b.codes, b.bits)).collect(),
            sf: Huffman::new(&SF_CODES, &SF_BITS),
            win: Windows::new(),
            long: Mdct::new(2048),
            short: Mdct::new(256),
            prev_shape: Shape::Sine,
            overlap: [vec![0.0; 1024], vec![0.0; 1024]],
        }
    }

    pub fn parse(&self, data: &[u8]) -> Result<Frame, String> {
        let mut r = BitReader::new(data);
        if r.read(3)? != 1 {
            return Err("expected a channel pair element".into());
        }
        r.read(4)?;
        if r.read(1)? != 1 {
            return Err("expected a common window".into());
        }
        if r.read(1)? != 0 {
            return Err("reserved bit set".into());
        }
        let seq = match r.read(2)? {
            0 => Sequence::OnlyLong,
            1 => Sequence::LongStart,
            2 => Sequence::EightShort,
            _ => Sequence::LongStop,
        };
        let shape = if r.read(1)? == 1 { Shape::Kbd } else { Shape::Sine };
        let short = seq == Sequence::EightShort;
        let (max_sfb, groups) = if short {
            let m = r.read(4)? as usize;
            let g = r.read(7)?;
            let mut groups = vec![1usize];
            for i in (0..7).rev() {
                if (g >> i) & 1 == 1 {
                    *groups.last_mut().unwrap() += 1;
                } else {
                    groups.push(1);
                }
            }
            (m, groups)
        } else {
            let m = r.read(6)? as usize;
            if r.read(1)? != 0 {
                return Err("prediction isn't AAC-LC".into());
            }
            (m, vec![1])
        };
        if max_sfb > if short { 14 } else { 49 } {
            return Err("max_sfb too big".into());
        }
        let nb = groups.len() * max_sfb;
        let ms = match r.read(2)? {
            0 => vec![false; nb],
            1 => (0..nb).map(|_| r.read(1).map(|b| b == 1)).collect::<Result<_, _>>()?,
            2 => vec![true; nb],
            _ => return Err("reserved ms_mask_present".into()),
        };
        let swb: &[u16] = if short { &SWB_SHORT } else { &SWB_LONG };
        let mut chans: [ChannelData; 2] = Default::default();
        for ch in chans.iter_mut() {
            let mut sf = r.read(8)? as i32;
            let (len_bits, esc) = if short { (3, 7) } else { (5, 31) };
            let mut books = vec![0u8; nb];
            for g in 0..groups.len() {
                let mut k = 0;
                while k < max_sfb {
                    let cb = r.read(4)? as u8;
                    if cb > 11 {
                        return Err(format!("codebook {cb} isn't used by this encoder"));
                    }
                    let mut len = 0;
                    loop {
                        let inc = r.read(len_bits)? as usize;
                        len += inc;
                        if inc != esc {
                            break;
                        }
                    }
                    if len == 0 || k + len > max_sfb {
                        return Err("bad section length".into());
                    }
                    for b in k..k + len {
                        books[g * max_sfb + b] = cb;
                    }
                    k += len;
                }
            }
            let mut sfs = vec![0i32; nb];
            for b in 0..nb {
                if books[b] != 0 {
                    sf += self.sf.decode(&mut r)? as i32 - 60;
                    if !(0..=255).contains(&sf) {
                        return Err("scalefactor out of range".into());
                    }
                    sfs[b] = sf;
                }
            }
            if r.read(1)? != 0 || r.read(1)? != 0 || r.read(1)? != 0 {
                return Err("pulse, TNS or gain control data isn't expected".into());
            }
            let mut q = vec![0i32; 1024];
            let mut w0 = 0;
            for (g, &glen) in groups.iter().enumerate() {
                for sfb in 0..max_sfb {
                    let book = books[g * max_sfb + sfb] as usize;
                    let width = (swb[sfb + 1] - swb[sfb]) as usize;
                    if book == 0 {
                        continue;
                    }
                    let mut vals = Vec::with_capacity(width * glen);
                    while vals.len() < width * glen {
                        vals.extend(read_tuple(&mut r, &self.huff[book], book)?);
                    }
                    for (wi, chunk) in vals.chunks(width).enumerate() {
                        let at = if short { (w0 + wi) * 128 } else { 0 } + swb[sfb] as usize;
                        q[at..at + width].copy_from_slice(chunk);
                    }
                }
                w0 += glen;
            }
            *ch = ChannelData { books, sf: sfs, q };
        }
        if r.read(3)? != 7 {
            return Err("expected the end element".into());
        }
        if r.pos().div_ceil(8) != data.len() {
            return Err(format!("frame has {} bytes but the data ends after {} bits", data.len(), r.pos()));
        }
        Ok(Frame { seq, shape, max_sfb, groups, ms, ch: chans })
    }

    /// Decode one frame to 1024 interleaved stereo samples.
    pub fn decode(&mut self, data: &[u8]) -> Result<Vec<i16>, String> {
        let f = self.parse(data)?;
        let short = f.seq == Sequence::EightShort;
        let swb: &[u16] = if short { &SWB_SHORT } else { &SWB_LONG };
        // Inverse quantisation.
        let mut spec = [vec![0.0f32; 1024], vec![0.0f32; 1024]];
        for c in 0..2 {
            let d = &f.ch[c];
            let mut w0 = 0;
            for (g, &glen) in f.groups.iter().enumerate() {
                for sfb in 0..f.max_sfb {
                    let b = g * f.max_sfb + sfb;
                    if d.books[b] == 0 {
                        continue;
                    }
                    let gain = 2f64.powf(0.25 * (d.sf[b] - 100) as f64);
                    for w in w0..w0 + glen {
                        let base = if short { w * 128 } else { 0 };
                        for k in base + swb[sfb] as usize..base + swb[sfb + 1] as usize {
                            let q = d.q[k];
                            spec[c][k] = ((q.unsigned_abs() as f64).powf(4.0 / 3.0) * gain * q.signum() as f64) as f32;
                        }
                    }
                }
                w0 += glen;
            }
        }
        // Mid/side back to left/right.
        let mut w0 = 0;
        for (g, &glen) in f.groups.iter().enumerate() {
            for sfb in 0..f.max_sfb {
                if !f.ms[g * f.max_sfb + sfb] {
                    continue;
                }
                for w in w0..w0 + glen {
                    let base = if short { w * 128 } else { 0 };
                    for k in base + swb[sfb] as usize..base + swb[sfb + 1] as usize {
                        let (m, s) = (spec[0][k], spec[1][k]);
                        spec[0][k] = m + s;
                        spec[1][k] = m - s;
                    }
                }
            }
            w0 += glen;
        }
        // Synthesis.
        let mut out = vec![0i16; 2048];
        for c in 0..2 {
            let mut y = vec![0.0f32; 2048];
            if short {
                let mut win = vec![0.0; 256];
                let mut t = vec![0.0; 256];
                for w in 0..8 {
                    mdct::short_window(&self.win, w, self.prev_shape, f.shape, &mut win);
                    self.short.inverse(&spec[c][w * 128..(w + 1) * 128], &mut t);
                    for i in 0..256 {
                        y[448 + w * 128 + i] += t[i] * win[i];
                    }
                }
            } else {
                let mut win = vec![0.0; 2048];
                mdct::long_window(&self.win, f.seq, self.prev_shape, f.shape, &mut win);
                let mut t = vec![0.0; 2048];
                self.long.inverse(&spec[c], &mut t);
                for i in 0..2048 {
                    y[i] = t[i] * win[i];
                }
            }
            for i in 0..1024 {
                let v = (self.overlap[c][i] + y[i]).round().clamp(-32768.0, 32767.0);
                out[2 * i + c] = v as i16;
            }
            self.overlap[c].copy_from_slice(&y[1024..]);
        }
        self.prev_shape = f.shape;
        Ok(out)
    }
}

/// Decode a whole stream (without the priming samples), interleaved stereo.
pub fn decode_all(frames: &[Vec<u8>]) -> Result<Vec<i16>, String> {
    let mut d = Decoder::new();
    let mut out = Vec::new();
    for (i, f) in frames.iter().enumerate() {
        out.extend(d.decode(f).map_err(|e| format!("frame {i}: {e}"))?);
    }
    let skip = 2 * super::AacEncoder::priming_samples() as usize;
    Ok(out.split_off(skip.min(out.len())))
}
