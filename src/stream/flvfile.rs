//! "Record a copy while streaming": the stream's own packets (the ones going to the server)
//! written to an FLV file, so the copy costs no second encoder. FLV, not MP4, because it is
//! written as it goes: a stream cut off by a crash or a full disk still plays up to that point.
//!
//! Layout: the 9-byte header, then tags (type, size, time, body) each followed by its size.
//! The bodies are the same ones RTMP carries (`flv` builds them).

use super::amf::{self, Amf};
use super::flv::{self, Meta, ParamSets};
use super::queue::Packet;
use crate::encode::Codec;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

const TAG_AUDIO: u8 = 8;
const TAG_VIDEO: u8 = 9;
const TAG_SCRIPT: u8 = 18;

pub struct FlvWriter<W: Write> {
    out: W,
    codec: Codec,
    asc: Option<Vec<u8>>,
    params: ParamSets,
    /// The first keyframe's time: the file starts there.
    base_us: Option<i64>,
    audio_header: bool,
    pub video_frames: u64,
    pub audio_frames: u64,
    pub bytes: u64,
}

impl<W: Write> FlvWriter<W> {
    /// The header and the stream description. `asc` is the AAC configuration (None: no sound).
    pub fn new(mut out: W, meta: &Meta, asc: Option<Vec<u8>>) -> std::io::Result<FlvWriter<W>> {
        let flags = if asc.is_some() { 0x05 } else { 0x01 };
        out.write_all(&[b'F', b'L', b'V', 1, flags, 0, 0, 0, 9, 0, 0, 0, 0])?;
        let mut w = FlvWriter { out, codec: meta.codec, asc, params: ParamSets::default(), base_us: None, audio_header: false, video_frames: 0, audio_frames: 0, bytes: 13 };
        // A file's metadata is onMetaData without the "@setDataFrame" in front that RTMP uses.
        let body = flv::metadata(meta);
        let skip = amf::encode(&[Amf::str("@setDataFrame")]).len();
        w.tag(TAG_SCRIPT, 0, body.get(skip..).unwrap_or(&body))?;
        Ok(w)
    }

    fn tag(&mut self, kind: u8, ms: u32, body: &[u8]) -> std::io::Result<()> {
        let n = body.len() as u32;
        let mut h = [0u8; 11];
        h[0] = kind;
        h[1..4].copy_from_slice(&n.to_be_bytes()[1..]);
        h[4..7].copy_from_slice(&ms.to_be_bytes()[1..]);
        h[7] = (ms >> 24) as u8;
        self.out.write_all(&h)?;
        self.out.write_all(body)?;
        self.out.write_all(&(n + 11).to_be_bytes())?;
        self.bytes += n as u64 + 15;
        Ok(())
    }

    fn ms(&self, t_us: i64) -> u32 {
        ((t_us - self.base_us.unwrap_or(0)).max(0) / 1000) as u32
    }

    /// One packet. The file starts at the first keyframe (with the decoder configuration);
    /// what comes before it is skipped.
    pub fn write(&mut self, p: &Packet) -> std::io::Result<()> {
        match p {
            Packet::Video(v) => {
                if self.base_us.is_none() {
                    if !v.keyframe {
                        return Ok(());
                    }
                    let found = ParamSets::find(self.codec, &v.data);
                    if !found.complete(self.codec) {
                        return Ok(());
                    }
                    self.params = found;
                    let Ok(h) = flv::video_sequence_header(self.codec, &self.params) else { return Ok(()) };
                    self.base_us = Some(v.dts_us);
                    self.tag(TAG_VIDEO, 0, &h)?;
                }
                let cts = ((v.pts_us - v.dts_us) / 1000) as i32;
                let ms = self.ms(v.dts_us);
                self.tag(TAG_VIDEO, ms, &flv::video_frame(self.codec, &v.data, v.keyframe, cts))?;
                self.video_frames += 1;
            }
            Packet::Audio(a) => {
                let (Some(base), Some(asc)) = (self.base_us, self.asc.clone()) else { return Ok(()) };
                if a.pts_us < base {
                    return Ok(());
                }
                if !self.audio_header {
                    self.audio_header = true;
                    self.tag(TAG_AUDIO, 0, &flv::audio_sequence_header(&asc))?;
                }
                let ms = self.ms(a.pts_us);
                self.tag(TAG_AUDIO, ms, &flv::audio_frame(&a.data))?;
                self.audio_frames += 1;
            }
        }
        Ok(())
    }

    pub fn finish(mut self) -> std::io::Result<W> {
        self.out.flush()?;
        Ok(self.out)
    }
}

/// What the copy came to.
#[derive(Clone, Debug, PartialEq)]
pub struct Copy {
    pub path: PathBuf,
    pub video_frames: u64,
    pub bytes: u64,
}

/// The copy being written on its own thread: packets go in through `hook`'s channel.
pub struct Recorder {
    tx: Option<Sender<Packet>>,
    thread: Option<JoinHandle<Result<Copy, String>>>,
}

impl Recorder {
    pub fn start(path: &Path, meta: Meta, asc: Option<Vec<u8>>) -> Result<Recorder, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't make the folder for the copy: {e}"))?;
        }
        let f = std::fs::File::create(path).map_err(|e| format!("Couldn't make {}: {e}", path.display()))?;
        let mut w = FlvWriter::new(BufWriter::new(f), &meta, asc).map_err(|e| e.to_string())?;
        let (tx, rx): (Sender<Packet>, Receiver<Packet>) = channel();
        let path = path.to_path_buf();
        let thread = std::thread::Builder::new()
            .name("snagster-stream-copy".into())
            .spawn(move || {
                let mut err = None;
                for p in rx {
                    if err.is_none()
                        && let Err(e) = w.write(&p)
                    {
                        err = Some(format!("Writing the copy stopped: {e}"));
                    }
                }
                let (frames, bytes) = (w.video_frames, w.bytes);
                w.finish().map_err(|e| e.to_string())?;
                match err {
                    Some(e) => Err(e),
                    None => Ok(Copy { path, video_frames: frames, bytes }),
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Recorder { tx: Some(tx), thread: Some(thread) })
    }

    /// The hook for `StreamConfig::on_packet`.
    pub fn hook(&self) -> super::PacketHook {
        let tx = std::sync::Mutex::new(self.tx.clone());
        std::sync::Arc::new(move |p: &Packet| {
            if let Ok(g) = tx.lock()
                && let Some(t) = g.as_ref()
            {
                let _ = t.send(p.clone());
            }
        })
    }

    /// Wait for the file to be complete (call after the stream has stopped, so no packets
    /// are still coming).
    pub fn finish(mut self) -> Result<Copy, String> {
        self.tx = None;
        match self.thread.take() {
            Some(t) => t.join().map_err(|_| "Writing the copy failed".to_string())?,
            None => Err("Writing the copy failed".into()),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::encode::{AudioPacket, VideoPacket};

    /// The tags of an FLV file: (type, time in ms, body).
    pub(crate) fn tags(data: &[u8]) -> Vec<(u8, u32, Vec<u8>)> {
        assert_eq!(&data[..3], b"FLV");
        let mut out = Vec::new();
        let mut i = 13;
        while i + 11 <= data.len() {
            let n = u32::from_be_bytes([0, data[i + 1], data[i + 2], data[i + 3]]) as usize;
            let ms = u32::from_be_bytes([data[i + 7], data[i + 4], data[i + 5], data[i + 6]]);
            let body = data[i + 11..i + 11 + n].to_vec();
            let prev = u32::from_be_bytes(data[i + 11 + n..i + 15 + n].try_into().unwrap()) as usize;
            assert_eq!(prev, n + 11, "previous tag size");
            out.push((data[i], ms, body));
            i += 15 + n;
        }
        assert_eq!(i, data.len(), "trailing bytes");
        out
    }

    fn key(ms: i64) -> Packet {
        let d = vec![0, 0, 0, 1, 0x67, 0x42, 0xC0, 0x1F, 0, 0, 0, 1, 0x68, 0xCE, 0, 0, 0, 1, 0x65, 0xAA];
        Packet::Video(VideoPacket { data: d, pts_us: ms * 1000, dts_us: ms * 1000, keyframe: true })
    }

    fn inter(ms: i64) -> Packet {
        Packet::Video(VideoPacket { data: vec![0, 0, 1, 0x41, 0xBB], pts_us: ms * 1000, dts_us: ms * 1000, keyframe: false })
    }

    fn sound(ms: i64) -> Packet {
        Packet::Audio(AudioPacket { data: vec![0x21, 0x10], pts_us: ms * 1000 })
    }

    fn meta(sound: bool) -> Meta {
        Meta { width: 64, height: 64, fps: 30, video_kbps: 1000, codec: Codec::H264, audio_kbps: sound.then_some(128) }
    }

    #[test]
    fn a_file_starts_at_the_first_keyframe_with_its_headers() {
        let mut w = FlvWriter::new(Vec::new(), &meta(true), Some(vec![0x11, 0x90])).unwrap();
        for p in [sound(0), inter(10), key(100), sound(110), inter(133), sound(90), sound(150)] {
            w.write(&p).unwrap();
        }
        assert_eq!((w.video_frames, w.audio_frames), (2, 2));
        let bytes = w.bytes;
        let data = w.finish().unwrap();
        assert_eq!(bytes, data.len() as u64);
        assert_eq!(data[4], 0x05, "audio and video");
        let t = tags(&data);
        let kinds: Vec<u8> = t.iter().map(|x| x.0).collect();
        assert_eq!(kinds, [TAG_SCRIPT, TAG_VIDEO, TAG_VIDEO, TAG_AUDIO, TAG_AUDIO, TAG_VIDEO, TAG_AUDIO]);
        // onMetaData without @setDataFrame.
        let m = amf::decode_all(&t[0].2).unwrap();
        assert_eq!(m[0].as_str(), Some("onMetaData"));
        assert_eq!(m[1].get("width").and_then(Amf::as_number), Some(64.0));
        assert_eq!(&t[1].2[..2], &[0x17, 0], "AVC sequence header");
        assert_eq!(&t[2].2[..2], &[0x17, 1]);
        assert_eq!(&t[3].2, &[0xAF, 0, 0x11, 0x90], "AAC sequence header");
        // Times count from the keyframe.
        let times: Vec<u32> = t.iter().map(|x| x.1).collect();
        assert_eq!(times, [0, 0, 0, 0, 10, 33, 50]);
    }

    #[test]
    fn without_sound_no_audio_is_written() {
        let mut w = FlvWriter::new(Vec::new(), &meta(false), None).unwrap();
        for p in [key(0), sound(5), inter(33)] {
            w.write(&p).unwrap();
        }
        let data = w.finish().unwrap();
        assert_eq!(data[4], 0x01);
        assert!(tags(&data).iter().all(|t| t.0 != TAG_AUDIO));
    }

    #[test]
    fn long_times_use_the_extended_byte() {
        let mut w = FlvWriter::new(Vec::new(), &meta(false), None).unwrap();
        w.write(&key(0)).unwrap();
        // 5 hours: past 24 bits of milliseconds.
        w.write(&inter(5 * 3600 * 1000)).unwrap();
        let t = tags(&w.finish().unwrap());
        assert_eq!(t[3].1, 5 * 3600 * 1000);
    }

    #[test]
    fn the_recorder_writes_on_its_own_thread() {
        let p = std::env::temp_dir().join(format!("snagster-copy-test-{}.flv", std::process::id()));
        let r = Recorder::start(&p, meta(false), None).unwrap();
        let hook = r.hook();
        for i in 0..10 {
            hook(&if i == 0 { key(0) } else { inter(i * 33) });
        }
        drop(hook);
        let c = r.finish().unwrap();
        assert_eq!(c.video_frames, 10);
        let data = std::fs::read(&p).unwrap();
        assert_eq!(c.bytes, data.len() as u64);
        assert_eq!(tags(&data).len(), 12);
        let _ = std::fs::remove_file(&p);
    }
}
