//! Encoder tests: everything is decoded again by `testdec` and compared with the input.

use super::bits::BitWriter;
use super::mdct::Sequence;
use super::tables::{BOOKS, SF_BITS, SF_CODES};
use super::testdec::{self, BitReader, Decoder, Huffman};
use super::*;

const RATE: f64 = SAMPLE_RATE as f64;

fn encode_all(bitrate: u32, pcm: &[i16]) -> Vec<Vec<u8>> {
    let mut e = AacEncoder::new(bitrate).unwrap();
    let mut frames = e.encode(pcm);
    frames.extend(e.finish());
    frames
}

/// Signal-to-noise ratio in dB of `got` against `want` over `range` (interleaved samples).
fn snr(want: &[i16], got: &[i16], range: std::ops::Range<usize>) -> f64 {
    let (mut s, mut n) = (0.0f64, 0.0f64);
    for i in range {
        let (a, b) = (want[i] as f64, got[i] as f64);
        s += a * a;
        n += (a - b) * (a - b);
    }
    10.0 * (s / n.max(1e-9)).log10()
}

fn stereo(len: usize, mut f: impl FnMut(usize) -> (f64, f64)) -> Vec<i16> {
    let mut v = Vec::with_capacity(len * 2);
    for i in 0..len {
        let (l, r) = f(i);
        v.push(l.round().clamp(-32768.0, 32767.0) as i16);
        v.push(r.round().clamp(-32768.0, 32767.0) as i16);
    }
    v
}

struct Rng {
    s: u32,
    a: f64,
    b: f64,
}

impl Rng {
    fn new(seed: u32) -> Self {
        Self { s: seed, a: 0.0, b: 0.0 }
    }

    /// White noise in -1..1.
    fn next(&mut self) -> f64 {
        self.s = self.s.wrapping_mul(1664525).wrapping_add(1013904223);
        (self.s >> 8) as f64 / (1u32 << 24) as f64 * 2.0 - 1.0
    }

    /// Noise with little above 10 kHz (two one-pole low-passes), so the encoder's audio
    /// bandwidth doesn't count as error.
    fn soft(&mut self) -> f64 {
        let x = self.next();
        self.a += 0.3 * (x - self.a);
        self.b += 0.3 * (self.a - self.b);
        self.b * 3.0
    }
}

/// Something like music: a melody of harmonic notes with vibrato, a bass line, a little
/// noise, drum hits every half second, and different mixes left and right.
fn music_like(secs: f64) -> Vec<i16> {
    let len = (secs * RATE) as usize;
    let mut rng = Rng::new(12345);
    let notes = [220.0, 261.6, 329.6, 392.0, 440.0, 349.2, 293.7, 246.9];
    let mut drum = 0.0f64;
    let mut out = Vec::with_capacity(len * 2);
    for i in 0..len {
        let t = i as f64 / RATE;
        let note = notes[(t * 2.0) as usize % notes.len()];
        let f = note * (1.0 + 0.004 * (2.0 * std::f64::consts::PI * 5.0 * t).sin());
        let env = (-(t * 2.0).fract() * 3.0).exp();
        let mut mel = 0.0;
        for h in 1..8 {
            mel += (2.0 * std::f64::consts::PI * f * h as f64 * t).sin() / h as f64;
        }
        let bass = (2.0 * std::f64::consts::PI * note / 4.0 * t).sin();
        if i % (RATE as usize / 2) == 0 {
            drum = 1.0;
        }
        let hit = drum * rng.soft();
        drum *= 0.9993;
        let hiss = rng.next() * 0.02;
        let l = 6000.0 * mel * env + 5000.0 * bass + 9000.0 * hit + 2000.0 * hiss;
        let r = 3000.0 * mel * env + 5000.0 * bass + 7000.0 * hit - 2000.0 * hiss;
        out.push(l.round().clamp(-32768.0, 32767.0) as i16);
        out.push(r.round().clamp(-32768.0, 32767.0) as i16);
    }
    out
}

#[test]
fn config_and_header() {
    assert_eq!(AacEncoder::audio_specific_config(), [0x11, 0x90]);
    assert_eq!(AacEncoder::priming_samples(), 1024);
    let h = adts_header(100, 2, 3);
    assert_eq!(h[0], 0xFF);
    assert_eq!(h[1], 0xF1); // MPEG-4, no CRC
    assert_eq!(h[2] >> 6, 1); // LC
    assert_eq!((h[2] >> 2) & 15, 3); // 48 kHz
    let ch = ((h[2] & 1) << 2) | (h[3] >> 6);
    assert_eq!(ch, 2);
    let len = ((h[3] as usize & 3) << 11) | ((h[4] as usize) << 3) | (h[5] as usize >> 5);
    assert_eq!(len, 107);
    assert!(AacEncoder::new(63_999).is_err());
    assert!(AacEncoder::new(320_001).is_err());
    assert_eq!(AacEncoder::new(128_000).unwrap().bitrate(), 128_000);
}

/// Every codeword of every codebook, with signs and escapes, reads back as written.
#[test]
fn huffman_round_trips_every_codeword() {
    let mut rng = Rng::new(9);
    for book in 1..12 {
        let cb = &BOOKS[book];
        let m = cb.codes.len() as f64;
        let m = if cb.dim == 4 { m.powf(0.25).round() as i32 } else { m.sqrt().round() as i32 };
        let huff = Huffman::new(cb.codes, cb.bits);
        let mut tuples = Vec::new();
        for idx in 0..cb.codes.len() as i32 {
            let mut v = vec![0i32; cb.dim];
            let mut rest = idx;
            for i in (0..cb.dim).rev() {
                v[i] = rest % m;
                rest /= m;
                if cb.signed {
                    v[i] -= (m - 1) / 2;
                } else if rng.next() < 0.0 {
                    v[i] = -v[i];
                }
            }
            if book == 11 {
                for x in v.iter_mut() {
                    if x.abs() == 16 {
                        let big = 16 + ((rng.next() + 1.0) * 4000.0) as i32;
                        *x = x.signum() * big.min(8191);
                    }
                }
            }
            tuples.push(v);
        }
        tuples.push(vec![8191, -16]);
        if book != 11 {
            tuples.pop();
        }
        let mut w = BitWriter::new();
        let mut expected_bits = 0;
        for t in &tuples {
            let (i, extra) = cb.index(t).unwrap();
            expected_bits += cb.bits[i] as usize + extra;
            cb.write(&mut w, t);
        }
        assert_eq!(w.len(), expected_bits, "book {book}");
        let data = w.finish();
        let mut r = BitReader::new(&data);
        for t in &tuples {
            assert_eq!(&testdec::read_tuple(&mut r, &huff, book).unwrap(), t, "book {book}");
        }
    }
    // Scalefactor differences.
    let huff = Huffman::new(&SF_CODES, &SF_BITS);
    let mut w = BitWriter::new();
    for d in 0..121 {
        w.put(SF_CODES[d], SF_BITS[d] as u32);
    }
    let data = w.finish();
    let mut r = BitReader::new(&data);
    for d in 0..121 {
        assert_eq!(huff.decode(&mut r).unwrap(), d);
    }
}

#[test]
fn sine_round_trip_keeps_the_tone() {
    let len = 48_000 * 2;
    let pcm = stereo(len, |i| {
        let t = i as f64 / RATE;
        let s = (2.0 * std::f64::consts::PI * 1000.0 * t).sin();
        (10_000.0 * s, 8_000.0 * (2.0 * std::f64::consts::PI * 1000.0 * t + 0.5).sin())
    });
    let frames = encode_all(128_000, &pcm);
    let out = testdec::decode_all(&frames).unwrap();
    let db = snr(&pcm, &out, 2 * 4096..pcm.len());
    println!("1 kHz sine at 128 kbit/s: SNR {db:.1} dB");
    assert!(db > 20.0, "SNR {db:.1} dB");
}

#[test]
fn music_like_round_trip() {
    let pcm = music_like(3.0);
    for rate in [64_000, 128_000, 256_000] {
        let frames = encode_all(rate, &pcm);
        let out = testdec::decode_all(&frames).unwrap();
        let db = snr(&pcm, &out, 0..pcm.len());
        println!("music-like at {} kbit/s: SNR {db:.1} dB", rate / 1000);
        assert!(db > if rate >= 128_000 { 15.0 } else { 8.0 }, "{rate}: SNR {db:.1} dB");
    }
}

#[test]
fn silence_makes_tiny_frames() {
    let frames = encode_all(128_000, &vec![0i16; 2 * 48_000]);
    assert!(frames.len() >= 47);
    for f in &frames {
        assert!(f.len() <= 12, "silent frame of {} bytes", f.len());
    }
    assert!(testdec::decode_all(&frames).unwrap().iter().all(|s| *s == 0));
}

#[test]
fn bitrate_is_met_on_average() {
    let secs = 10.0;
    let music = music_like(secs);
    let mut rng = Rng::new(77);
    let noise = stereo((secs * RATE) as usize, |_| (6000.0 * rng.next(), 6000.0 * rng.next()));
    for (name, pcm) in [("music-like", &music), ("noise", &noise)] {
        for rate in [64_000u32, 128_000, 192_000] {
            let frames = encode_all(rate, pcm);
            let bytes: usize = frames.iter().map(|f| f.len()).sum();
            let got = bytes as f64 * 8.0 / secs;
            println!("{name} at {} kbit/s: {:.1} kbit/s", rate / 1000, got / 1000.0);
            assert!((got / rate as f64 - 1.0).abs() < 0.15, "{name} at {rate}: {got}");
        }
    }
}

#[test]
fn frames_never_exceed_the_decoder_buffer() {
    // Loud full-band noise with silent gaps (the reservoir fills, then gets spent) at the top rate.
    let mut rng = Rng::new(5);
    let pcm = stereo(48_000 * 4, |i| {
        let on = (i / 12_000) % 2 == 0;
        if on { (30_000.0 * rng.next(), 30_000.0 * rng.next()) } else { (0.0, 0.0) }
    });
    for rate in [64_000, 320_000] {
        let frames = encode_all(rate, &pcm);
        let max = frames.iter().map(|f| f.len()).max().unwrap();
        assert!(max * 8 <= 6144 * 2, "{rate}: frame of {max} bytes");
        testdec::decode_all(&frames).unwrap();
    }
}

#[test]
fn finish_flushes_every_sample() {
    let len = 12_345;
    let pcm = stereo(len, |i| {
        let t = i as f64 / RATE;
        (8000.0 * (2.0 * std::f64::consts::PI * 440.0 * t).sin(), 6000.0 * (2.0 * std::f64::consts::PI * 660.0 * t).sin())
    });
    let frames = encode_all(128_000, &pcm);
    // (1024 priming + 12345) samples need 14 frames of 1024.
    assert_eq!(frames.len(), 14);
    let out = testdec::decode_all(&frames).unwrap();
    assert!(out.len() >= pcm.len());
    let tail = snr(&pcm, &out, pcm.len() - 2000..pcm.len());
    assert!(tail > 20.0, "end of the sound: SNR {tail:.1} dB");
    // Nothing more after finishing.
    let mut e = AacEncoder::new(128_000).unwrap();
    e.encode(&pcm);
    e.finish();
    assert!(e.finish().is_empty());
    assert!(e.encode(&pcm).is_empty());
}

#[test]
fn chunk_sizes_dont_change_the_stream() {
    let pcm = music_like(1.0);
    let whole = encode_all(128_000, &pcm);
    let mut e = AacEncoder::new(128_000).unwrap();
    let mut parts = Vec::new();
    // Odd chunk lengths split stereo pairs.
    let mut at = 0;
    let mut n = 1;
    while at < pcm.len() {
        let end = (at + n).min(pcm.len());
        parts.extend(e.encode(&pcm[at..end]));
        at = end;
        n = n * 7 % 3001 + 1;
    }
    parts.extend(e.finish());
    assert_eq!(whole, parts);
}

#[test]
fn attacks_switch_to_short_windows() {
    // Silence, then sharp clicks of noise.
    let mut rng = Rng::new(3);
    let pcm = stereo(48_000, |i| {
        let k = i % 12_000;
        if (6000..6600).contains(&k) {
            let a = 20_000.0 * (-((k - 6000) as f64) / 150.0).exp();
            (a * rng.soft(), a * rng.soft())
        } else {
            (0.0, 0.0)
        }
    });
    let frames = encode_all(128_000, &pcm);
    let d = Decoder::new();
    let seqs: Vec<Sequence> = frames.iter().map(|f| d.parse(f).unwrap().seq).collect();
    assert!(seqs.contains(&Sequence::EightShort));
    for w in seqs.windows(2) {
        let ok = match w[0] {
            Sequence::OnlyLong | Sequence::LongStop => matches!(w[1], Sequence::OnlyLong | Sequence::LongStart),
            Sequence::LongStart => w[1] == Sequence::EightShort,
            Sequence::EightShort => matches!(w[1], Sequence::EightShort | Sequence::LongStop),
        };
        assert!(ok, "{:?} then {:?}", w[0], w[1]);
    }
    let out = testdec::decode_all(&frames).unwrap();
    let db = snr(&pcm, &out, 0..pcm.len());
    assert!(db > 15.0, "clicks: SNR {db:.1} dB");
    // Ahead of each click stays nearly silent: the noise spreads at most over the short
    // windows (256 samples) holding the attack, not over a long window (2048).
    for c in 0..4 {
        let start = c * 12_000 + 5000;
        let peak = out[2 * start..2 * (start + 1000 - 300)].iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!(peak < 300, "pre-echo of {peak} before click {c}");
    }
}

#[test]
fn centred_sound_uses_mid_side() {
    let pcm = stereo(48_000, |i| {
        let t = i as f64 / RATE;
        let v = 9000.0 * (2.0 * std::f64::consts::PI * 300.0 * t).sin() + 3000.0 * (2.0 * std::f64::consts::PI * 2500.0 * t).sin();
        (v, v)
    });
    let frames = encode_all(128_000, &pcm);
    let d = Decoder::new();
    let f = d.parse(&frames[20]).unwrap();
    let coded: Vec<usize> = (0..f.ms.len()).filter(|&b| f.ch[0].books[b] != 0).collect();
    assert!(coded.len() > 5);
    assert!(coded.iter().all(|&b| f.ms[b]), "every band with sound is mid/side");
    assert!(f.ch[1].books.iter().all(|b| *b == 0), "nothing left in the side channel");
    let out = testdec::decode_all(&frames).unwrap();
    assert!(snr(&pcm, &out, 0..pcm.len()) > 25.0);
}

/// Speed: run with `cargo test --release aac::tests::speed -- --ignored --nocapture`.
#[test]
#[ignore]
fn speed() {
    let pcm = music_like(20.0);
    let t = std::time::Instant::now();
    let frames = encode_all(128_000, &pcm);
    let secs = t.elapsed().as_secs_f64();
    println!("{} frames; 20 s of sound in {secs:.3} s = {:.0}x real time", frames.len(), 20.0 / secs);
}

/// Writes ADTS files of the test signals for checking with other decoders by hand.
#[test]
#[ignore]
fn write_adts_files() {
    let dir = std::env::temp_dir();
    for (name, pcm) in [("music", music_like(10.0)), ("clicks", {
        let mut rng = Rng::new(3);
        stereo(48_000 * 3, |i| {
            let k = i % 12_000;
            if (6000..6600).contains(&k) {
                let a = 20_000.0 * (-((k - 6000) as f64) / 150.0).exp();
                (a * rng.soft(), a * rng.soft())
            } else {
                (0.0, 0.0)
            }
        })
    })] {
        let frames = encode_all(128_000, &pcm);
        let mut file = Vec::new();
        for f in &frames {
            file.extend_from_slice(&adts_header(f.len(), 2, 3));
            file.extend_from_slice(f);
        }
        let p = dir.join(format!("snagster-aac-{name}.aac"));
        std::fs::write(&p, file).unwrap();
        let raw: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        std::fs::write(dir.join(format!("snagster-aac-{name}.s16")), raw).unwrap();
        let dec: Vec<u8> = testdec::decode_all(&frames).unwrap().iter().flat_map(|s| s.to_le_bytes()).collect();
        std::fs::write(dir.join(format!("snagster-aac-{name}.dec.s16")), dec).unwrap();
        println!("{}", p.display());
    }
}

#[test]
fn grouping_and_bandwidth() {
    // Windows 1 and 5 join the group before them.
    assert_eq!(grouping_bits(&[2, 1, 1, 2, 1, 1]), 0b100_0100);
    assert_eq!(grouping_bits(&[8]), 0b111_1111);
    assert_eq!(grouping_bits(&[1; 8]), 0);
    assert_eq!(cutoff_hz(64_000), 12_000.0);
    assert_eq!(cutoff_hz(320_000), 20_000.0);
    assert!(cutoff_hz(128_000) > cutoff_hz(96_000));
    // 16.5 kHz at 128 kbit/s: long bands up to line 704 (16.5 kHz), short ones to line 88.
    assert_eq!(super::tables::SWB_LONG[bands_below(&super::tables::SWB_LONG, 16_500.0, 1024)], 704);
    assert_eq!(bands_below(&super::tables::SWB_SHORT, 16_500.0, 128), 12);
}

#[test]
fn detects_attacks_but_not_steady_sound() {
    let block = |f: &dyn Fn(usize) -> f32| [(0..3072).map(f).collect::<Vec<f32>>(), vec![0.0; 3072]];
    let steady = block(&|i| 8000.0 * (i as f32 * 0.07).sin());
    assert!(!detect_attack(&steady, 1024));
    let click = block(&|i| if i > 2200 { 8000.0 * (i as f32 * 1.3).sin() } else { 0.0 });
    assert!(detect_attack(&click, 1024));
    assert!(!detect_attack(&block(&|_| 0.0), 1024));
}
