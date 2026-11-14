//! Live streaming to RTMP/RTMPS services (Twitch, YouTube, Kick, Facebook Live, Restream, any
//! custom server). Pure Rust on every system: our own RTMP client (`rtmp`), FLV packaging of
//! H.264, HEVC (Enhanced RTMP) and AAC (`flv`), AMF0 (`amf`), TLS through rustls.
//!
//! Using it:
//!
//! ```ignore
//! let target = StreamTarget { service: Service::Twitch, url: String::new(), key };
//! let cfg = StreamConfig::for_target(target, 1920, 1080, 30, Codec::H264);
//! let s = Streamer::start(cfg, FrameSource::studio(&engine), SoundSource::Capture(cap), Encoders::system())?;
//! // every frame in the window:
//! let st = s.stats();          // never waits
//! // at the end:
//! let last = s.stop();         // flushes, unpublishes, closes
//! ```
//!
//! The stream key is secret: `StreamTarget`'s `Debug`, every error and `Stats::error` show it as
//! `****`.

// The engine is ready but the window and MCP don't call it yet; they will, and then this goes.
#![allow(dead_code, unused_imports)]

pub mod amf;
pub mod engine;
pub mod flv;
pub mod queue;
pub mod rtmp;
pub mod services;
#[cfg(test)]
mod tests;

pub use engine::{AudioFeed, Encoders, FrameSource, PacketHook, RgbaFeed, SoundSource, State, Stats, StreamConfig, Streamer, recommended_kbps};
pub use queue::Packet;
pub use services::{PRESETS, Preset, Service, StreamTarget, mask};
