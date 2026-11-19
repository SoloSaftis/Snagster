//! Live streaming to RTMP/RTMPS services (Twitch, YouTube, Kick, Facebook Live, Restream, any
//! custom server). Pure Rust on every system: our own RTMP client (`rtmp`), FLV packaging of
//! H.264, HEVC (Enhanced RTMP) and AAC (`flv`), AMF0 (`amf`), TLS through rustls.
//!
//! Using it:
//!
//! ```ignore
//! let target = StreamTarget { service: Service::Twitch, url: String::new(), key };
//! let cfg = StreamConfig::for_target(target, 1920, 1080, 30, Codec::H264);
//! let s = Streamer::start(cfg, source::studio_frames(engine.shared.clone(), w, h), SoundSource::Capture(cap), Encoders::system())?;
//! // every frame in the window:
//! let st = s.stats();          // never waits
//! // at the end:
//! let last = s.stop();         // flushes, unpublishes, closes
//! ```
//!
//! The stream key is secret: `StreamTarget`'s `Debug`, every error and `Stats::error` show it as
//! `****`.
//!
//! The window and AI apps go live through `live` (`begin`, `snapshot`, `end`): one stream at a
//! time, started and stopped on threads of its own. `source` copies the screen, a window or an
//! area (or takes Studio's scene); `flvfile` writes a copy of the stream; `keystore` keeps the
//! key out of settings.json; `testserver` is a local RTMP server for the button test.

pub mod amf;
pub mod engine;
pub mod flv;
pub mod flvfile;
pub mod keystore;
pub mod live;
pub mod queue;
pub mod rtmp;
pub mod services;
pub mod source;
pub mod testserver;
#[cfg(test)]
mod tests;

pub use engine::{Encoders, PacketHook, State};
pub use services::{PRESETS, Service, StreamTarget, mask};
pub use source::Source;

/// Fuzz targets (`src/fuzz.rs`): what servers send (`rtmp`), and what encoders hand over to be
/// packed into FLV tags.
#[cfg(test)]
pub(crate) mod fuzzing {
    use crate::encode::Codec;
    use crate::fuzz::Target;

    pub fn targets() -> Vec<Target> {
        let mut all = super::rtmp::fuzzing::targets();
        all.push(Target {
            name: "stream_flv",
            seeds: || vec![[&[0u8, 0, 0, 1, 0x67, 0x42, 0xC0, 0x1F, 0xDA][..], &[0, 0, 1, 0x68, 0xCE, 0x3C, 0x80], &[0, 0, 0, 1, 0x65, 1, 2, 3], &[0, 0, 1, 0x40, 1, 0x0C, 0, 0, 0, 1, 0x42, 1, 1, 1, 0x60, 0, 0, 3, 0, 0, 1, 0x44]].concat()],
            words: || ["\u{0}\u{0}\u{1}", "\u{0}\u{0}\u{0}\u{1}", "\u{0}\u{0}\u{3}", "g", "h", "e", "@", "B", "D"].iter().map(|s| s.to_string()).collect(),
            run: |au| {
                let nals = super::flv::split_annexb(au);
                assert!(nals.iter().map(|n| n.len()).sum::<usize>() <= au.len());
                for codec in [Codec::H264, Codec::Hevc] {
                    let p = super::flv::ParamSets::find(codec, au);
                    let _ = super::flv::video_sequence_header(codec, &p);
                    let _ = super::flv::annexb_to_length_prefixed(codec, au);
                    assert!(super::flv::video_frame(codec, au, au.len() % 2 == 0, au.len() as i32 - 100).len() <= 9 + au.len() + 4 * nals.len());
                }
                for n in nals {
                    let _ = super::flv::unescape(n);
                }
                let _ = super::flv::audio_sequence_header(au);
                let _ = super::flv::audio_frame(au);
            },
            budget: |n| 64 * 1024 + 16 * n,
        });
        all
    }
}
