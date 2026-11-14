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
