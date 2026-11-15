//! WAV files for Studio's sound-file inputs: read, then brought to 48 kHz stereo 16-bit (what
//! the sound mixer works in). Snagster's own reader: RIFF/WAVE with PCM (8, 16, 24 and 32 bits),
//! IEEE float (32 and 64 bits) and WAVE_FORMAT_EXTENSIBLE around either; any number of channels
//! (mono is put on both sides, more than two keep the first two) and any rate (linear
//! resampling). At most `MAX_SECONDS` are kept, so a huge file can't use up the memory.

/// The longest sound kept from a file.
pub const MAX_SECONDS: u32 = 600;
/// A bigger file isn't read at all.
const MAX_BYTES: u64 = 512 * 1024 * 1024;

/// A file's sound as it is: interleaved samples in -1..1.
#[derive(Debug, PartialEq)]
pub struct Wav {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

/// Read a WAV file's bytes.
pub fn parse(bytes: &[u8]) -> Result<Wav, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("it isn't a WAV file".into());
    }
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32_at(bytes, at + 4).unwrap_or(0) as usize;
        let body = &bytes[at + 8..(at + 8).saturating_add(size).min(bytes.len())];
        match id {
            b"fmt " => {
                let mut tag = u16_at(body, 0).ok_or("the WAV format is cut short")?;
                let channels = u16_at(body, 2).ok_or("the WAV format is cut short")?;
                let rate = u32_at(body, 4).ok_or("the WAV format is cut short")?;
                let bits = u16_at(body, 14).ok_or("the WAV format is cut short")?;
                // WAVE_FORMAT_EXTENSIBLE: the real format is the start of the sub-format GUID.
                if tag == 0xFFFE {
                    tag = u16_at(body, 24).ok_or("the WAV format is cut short")?;
                }
                fmt = Some((tag, channels, rate, bits));
            }
            b"data" => {
                let (tag, channels, rate, bits) = fmt.ok_or("the WAV file has no format before its sound")?;
                if !(1..=16).contains(&channels) {
                    return Err(format!("the WAV file has {channels} channels"));
                }
                if !(1000..=384_000).contains(&rate) {
                    return Err(format!("the WAV file's rate ({rate} Hz) isn't usable"));
                }
                let samples = decode(tag, bits, body)?;
                return Ok(Wav { rate, channels, samples });
            }
            _ => {}
        }
        // Chunks are padded to an even size.
        at = at.saturating_add(8).saturating_add(size).saturating_add(size & 1);
    }
    Err("the WAV file has no sound in it".into())
}

fn decode(tag: u16, bits: u16, d: &[u8]) -> Result<Vec<f32>, String> {
    Ok(match (tag, bits) {
        (1, 8) => d.iter().map(|&b| (b as f32 - 128.0) / 128.0).collect(),
        (1, 16) => d.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect(),
        (1, 24) => d.chunks_exact(3).map(|c| (i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 8) as f32 / 8_388_608.0).collect(),
        (1, 32) => d.chunks_exact(4).map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32 / 2_147_483_648.0).collect(),
        (3, 32) => d.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]).clamp(-1.0, 1.0)).collect(),
        (3, 64) => d.chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap_or([0; 8])).clamp(-1.0, 1.0) as f32).collect(),
        (1 | 3, b) => return Err(format!("{b}-bit WAV sound isn't supported")),
        (t, _) => return Err(format!("this WAV file is compressed (format {t}); save it as plain PCM")),
    })
}

/// `w` as 48 kHz stereo 16-bit, at most `max_secs` long.
pub fn to_48k_stereo(w: &Wav, max_secs: u32) -> Vec<i16> {
    let ch = w.channels.max(1) as usize;
    let frames_in = w.samples.len() / ch;
    if frames_in == 0 {
        return Vec::new();
    }
    let side = |f: usize, c: usize| w.samples[f * ch + c.min(ch - 1)];
    let frames_out = ((frames_in as u64 * 48_000 / w.rate as u64) as usize).min(48_000 * max_secs as usize);
    let mut out = Vec::with_capacity(frames_out * 2);
    let step = w.rate as f64 / 48_000.0;
    for i in 0..frames_out {
        let pos = i as f64 * step;
        let f = (pos as usize).min(frames_in - 1);
        let next = (f + 1).min(frames_in - 1);
        let t = (pos - f as f64) as f32;
        for c in 0..2 {
            let v = side(f, c) + (side(next, c) - side(f, c)) * t;
            out.push((v * 32767.0).round().clamp(-32768.0, 32767.0) as i16);
        }
    }
    out
}

/// Read a WAV file as 48 kHz stereo 16-bit (at most `MAX_SECONDS`).
pub fn read_48k_stereo(path: &std::path::Path) -> Result<Vec<i16>, String> {
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_BYTES {
        return Err(format!("the file is too big ({} MB)", size / 1_048_576));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    Ok(to_48k_stereo(&parse(&bytes)?, MAX_SECONDS))
}

/// `secs` seconds of a `hz` tone as a mono 48 kHz 16-bit WAV file (the button test's and the
/// README pictures' sound file).
pub fn tone(secs: u32, hz: f32) -> Vec<u8> {
    let samples: Vec<i16> = (0..48_000 * secs).map(|i| ((i as f32 * hz * std::f32::consts::TAU / 48_000.0).sin() * 12_000.0) as i16).collect();
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut b = Vec::with_capacity(44 + data.len());
    b.extend(b"RIFF");
    b.extend((36 + data.len() as u32).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes()); // PCM
    b.extend(1u16.to_le_bytes()); // mono
    b.extend(48_000u32.to_le_bytes());
    b.extend((48_000u32 * 2).to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend((data.len() as u32).to_le_bytes());
    b.extend(data);
    b
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A PCM WAV file of `samples` (16-bit), or of `bits`-bit samples built from them.
    pub(crate) fn wav_bytes(channels: u16, rate: u32, bits: u16, samples: &[i16]) -> Vec<u8> {
        let data: Vec<u8> = match bits {
            8 => samples.iter().map(|&s| ((s >> 8) + 128) as u8).collect(),
            24 => samples.iter().flat_map(|&s| { let v = (s as i32) << 8; [v as u8, (v >> 8) as u8, (v >> 16) as u8] }).collect(),
            _ => samples.iter().flat_map(|s| s.to_le_bytes()).collect(),
        };
        with_format(1, channels, rate, bits, &data, false)
    }

    fn with_format(tag: u16, channels: u16, rate: u32, bits: u16, data: &[u8], extensible: bool) -> Vec<u8> {
        let mut fmt = Vec::new();
        fmt.extend((if extensible { 0xFFFEu16 } else { tag }).to_le_bytes());
        fmt.extend(channels.to_le_bytes());
        fmt.extend(rate.to_le_bytes());
        fmt.extend((rate * channels as u32 * bits as u32 / 8).to_le_bytes());
        fmt.extend((channels * bits / 8).to_le_bytes());
        fmt.extend(bits.to_le_bytes());
        if extensible {
            fmt.extend(22u16.to_le_bytes());
            fmt.extend(bits.to_le_bytes());
            fmt.extend(3u32.to_le_bytes());
            fmt.extend(tag.to_le_bytes());
            fmt.extend([0u8; 14]);
        }
        let mut b = b"RIFF\0\0\0\0WAVE".to_vec();
        // A chunk Snagster doesn't know, with an odd size and its pad byte, comes first.
        b.extend(b"LIST");
        b.extend(3u32.to_le_bytes());
        b.extend([1, 2, 3, 0]);
        b.extend(b"fmt ");
        b.extend((fmt.len() as u32).to_le_bytes());
        b.extend(fmt);
        b.extend(b"data");
        b.extend((data.len() as u32).to_le_bytes());
        b.extend(data);
        let total = (b.len() - 8) as u32;
        b[4..8].copy_from_slice(&total.to_le_bytes());
        b
    }

    #[test]
    fn pcm_of_every_size_reads_back() {
        let s = [0i16, 16384, -16384, 32767];
        for bits in [8u16, 16, 24] {
            let w = parse(&wav_bytes(1, 48_000, bits, &s)).unwrap();
            assert_eq!((w.rate, w.channels, w.samples.len()), (48_000, 1, 4), "{bits}-bit");
            assert!((w.samples[1] - 0.5).abs() < 0.01 && (w.samples[2] + 0.5).abs() < 0.01, "{bits}-bit: {:?}", w.samples);
        }
        let floats: Vec<u8> = [0.25f32, -1.5].iter().flat_map(|f| f.to_le_bytes()).collect();
        let w = parse(&with_format(3, 1, 44_100, 32, &floats, false)).unwrap();
        assert_eq!(w.samples, vec![0.25, -1.0], "float, clamped");
        let w = parse(&with_format(3, 2, 44_100, 32, &floats, true)).unwrap();
        assert_eq!((w.channels, w.samples.len()), (2, 2), "WAVE_FORMAT_EXTENSIBLE");
    }

    #[test]
    fn mono_goes_to_both_sides_and_other_rates_are_resampled() {
        // 24 kHz mono, 100 frames: 200 frames of 48 kHz stereo, same on both sides.
        let s: Vec<i16> = (0..100).map(|i| (i * 100) as i16).collect();
        let out = to_48k_stereo(&parse(&wav_bytes(1, 24_000, 16, &s)).unwrap(), 60);
        assert_eq!(out.len(), 400);
        assert_eq!((out[0], out[1]), (0, 0));
        assert_eq!(out[4], out[5], "both sides alike");
        assert!((out[2] - 50).abs() <= 1, "halfway between two samples: {}", out[2]);
        // Four channels keep the first two.
        let w = Wav { rate: 48_000, channels: 4, samples: vec![0.5, -0.5, 0.9, 0.9] };
        assert_eq!(to_48k_stereo(&w, 60), vec![16384, -16384]);
        // At most `max_secs`.
        let long = Wav { rate: 48_000, channels: 1, samples: vec![0.0; 48_000 * 3] };
        assert_eq!(to_48k_stereo(&long, 2).len(), 48_000 * 2 * 2);
        assert!(to_48k_stereo(&Wav { rate: 48_000, channels: 2, samples: vec![] }, 2).is_empty());
    }

    #[test]
    fn what_isnt_plain_wav_says_why() {
        assert_eq!(parse(b"ID3 an mp3").unwrap_err(), "it isn't a WAV file");
        let adpcm = with_format(2, 1, 22_050, 4, &[0; 8], false);
        assert!(parse(&adpcm).unwrap_err().contains("compressed (format 2)"));
        assert!(parse(&with_format(1, 1, 48_000, 12, &[0; 6], false)).unwrap_err().contains("12-bit"));
        assert!(parse(&wav_bytes(0, 48_000, 16, &[0])).unwrap_err().contains("0 channels"));
        assert!(parse(&wav_bytes(1, 10, 16, &[0])).unwrap_err().contains("isn't usable"));
        let mut no_data = wav_bytes(1, 48_000, 16, &[]);
        no_data.truncate(no_data.len() - 8);
        assert!(parse(&no_data).is_err());
    }

    /// A data chunk that says it is longer than the file (a cut-off download) uses what's there;
    /// random bytes never panic.
    #[test]
    fn broken_files_never_panic() {
        let mut b = wav_bytes(2, 48_000, 16, &[1000; 8]);
        let n = b.len();
        b.truncate(n - 3);
        assert_eq!(parse(&b).unwrap().samples.len(), 6);
        let mut seed = 7u32;
        for len in [0usize, 4, 12, 20, 44, 100] {
            let mut junk: Vec<u8> = (0..len)
                .map(|_| {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    (seed >> 16) as u8
                })
                .collect();
            if len >= 12 {
                junk[0..4].copy_from_slice(b"RIFF");
                junk[8..12].copy_from_slice(b"WAVE");
            }
            let _ = parse(&junk);
        }
    }

    #[test]
    fn the_tone_reads_back_as_a_second_of_sound() {
        let w = parse(&tone(1, 440.0)).unwrap();
        assert_eq!((w.rate, w.channels, w.samples.len()), (48_000, 1, 48_000));
        assert!(w.samples.iter().any(|s| *s > 0.3));
    }

    #[test]
    fn missing_and_huge_files_are_refused() {
        assert!(read_48k_stereo(std::path::Path::new("/no/such/file.wav")).is_err());
    }
}
