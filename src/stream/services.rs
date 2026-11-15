//! Streaming services: where each one takes the stream, what it recommends, and how the server
//! address and the stream key become an RTMP connection (host, port, app, stream name).

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Service {
    #[default]
    Twitch,
    YouTube,
    Kick,
    Facebook,
    Restream,
    /// Any RTMP/RTMPS server (your own, OBS-style relays, other sites).
    Custom,
}

/// What a service takes and recommends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Preset {
    pub service: Service,
    pub name: &'static str,
    /// The usual ingest address (the person can change it, e.g. to a nearer server).
    pub url: &'static str,
    /// Where the stream key is found, shown next to the key box.
    pub key_help: &'static str,
    /// Highest video bitrate the service takes (kbit/s).
    pub max_video_kbps: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub max_fps: u32,
    /// Most pixels a second it takes (width × height × frame rate). The custom server's is
    /// 3840 × 2160 at 60: 4K stays at 60 fps, while 120 fps goes up to 1440p (and 2560 × 1600,
    /// just under). What graphics-chip encoders manage in real time is about the same.
    pub max_pixel_rate: u64,
    /// Seconds between keyframes the service asks for.
    pub keyframe_s: u32,
    pub audio_kbps: u32,
    /// Takes HEVC over Enhanced RTMP.
    pub hevc: bool,
    /// From this picture height on HEVC is chosen by itself (when this computer can encode
    /// it), because the service wants much less bitrate for it there; 0: only when asked for.
    pub hevc_from_height: u32,
    /// Recommended bitrates by size and frame rate; empty: `engine::recommended_kbps`.
    pub rates: &'static [RateRow],
}

/// One row of a bitrate table: pictures up to `height` tall at up to `fps` frames a second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RateRow {
    pub height: u32,
    pub fps: u32,
    /// Recommended kbit/s for H.264 and for HEVC.
    pub h264: u32,
    pub hevc: u32,
}

const fn row(height: u32, fps: u32, h264: u32, hevc: u32) -> RateRow {
    RateRow { height, fps, h264, hevc }
}

/// YouTube's "recommended" column in kbit/s, H.264 and HEVC/AV1, from YouTube Help "Choose live
/// encoder settings, bitrates, and resolutions" (support.google.com/youtube/answer/2853702, read
/// 2026-11-15): 2160p60 50 / 35 Mbit/s, 2160p30 42 / 30, 1440p60 34 / 24, 1440p30 21 / 15,
/// 1080p60 17 / 12, 1080p30 14 / 10, 720p (30 and 60) 8 / 6, 480p and 360p 4 / 3. The same page
/// asks for keyframes every 2 s (never more than 4) and AAC stereo at 128 kbit/s.
const YOUTUBE_RATES: &[RateRow] = &[
    row(480, 60, 4_000, 3_000),
    row(720, 60, 8_000, 6_000),
    row(1080, 30, 14_000, 10_000),
    row(1080, 60, 17_000, 12_000),
    row(1440, 30, 21_000, 15_000),
    row(1440, 60, 34_000, 24_000),
    row(2160, 30, 42_000, 30_000),
    row(2160, 60, 50_000, 35_000),
];

/// Custom servers: YouTube's table (the best-known published one) with 120 fps rows up to 1440p
/// (and 1600 rows for 2560 × 1600 screens, between 1440p and 2160p) at about 1.4 times the 60 fps rate (twice the pictures, each a smaller change from the last).
const CUSTOM_RATES: &[RateRow] = &[
    row(480, 60, 4_000, 3_000),
    row(480, 120, 6_000, 4_000),
    row(720, 60, 8_000, 6_000),
    row(720, 120, 11_000, 8_000),
    row(1080, 30, 14_000, 10_000),
    row(1080, 60, 17_000, 12_000),
    row(1080, 120, 24_000, 17_000),
    row(1440, 30, 21_000, 15_000),
    row(1440, 60, 34_000, 24_000),
    row(1440, 120, 48_000, 34_000),
    row(1600, 30, 24_000, 17_000),
    row(1600, 60, 38_000, 27_000),
    row(1600, 120, 50_000, 36_000),
    row(2160, 30, 42_000, 30_000),
    row(2160, 60, 50_000, 35_000),
];

/// 1920 × 1080 at 60 fps, and 3840 × 2160 at 60 fps, in pixels a second.
pub const PX_1080P60: u64 = 1920 * 1080 * 60;
pub const PX_2160P60: u64 = 3840 * 2160 * 60;

/// The frame rates offered (Settings > Streaming), and what a too-fast rate steps down to.
pub const FRAME_RATES: [u32; 6] = [24, 25, 30, 50, 60, 120];

/// The picture heights offered (Settings > Streaming).
pub const HEIGHTS: [u32; 6] = [2160, 1440, 1080, 720, 480, 360];

/// Twitch: "Broadcasting Guidelines" (help.twitch.tv): up to 1080p60 and 6000 kbit/s for H.264
/// over RTMP, keyframes every 2 s, AAC up to 160 kbit/s. (Twitch's 1440p and HEVC need its
/// multi-track "Enhanced Broadcasting", which plain RTMP doesn't do.)
/// Kick: up to 1080p60, 8000 kbit/s. Facebook Live: "Live Video Specs": up to 1080p60, at most
/// 9000 kbit/s, keyframes every 2 s, AAC 128 kbit/s. Restream: 1080p60 on every plan.
pub const PRESETS: [Preset; 6] = [
    Preset {
        service: Service::Twitch,
        name: "Twitch",
        url: "rtmps://live.twitch.tv:443/app",
        key_help: "Twitch: Creator Dashboard > Settings > Stream > Primary Stream key",
        max_video_kbps: 6000,
        max_width: 1920,
        max_height: 1080,
        max_fps: 60,
        max_pixel_rate: PX_1080P60,
        keyframe_s: 2,
        audio_kbps: 160,
        hevc: false,
        hevc_from_height: 0,
        rates: &[],
    },
    Preset {
        service: Service::YouTube,
        name: "YouTube",
        url: "rtmps://a.rtmps.youtube.com:443/live2",
        key_help: "YouTube Studio > Go live > Stream > Stream key",
        // 2160p60 H.264 is recommended at 50 Mbit/s; a little room above it.
        max_video_kbps: 51_000,
        max_width: 3840,
        max_height: 2160,
        max_fps: 60,
        max_pixel_rate: PX_2160P60,
        keyframe_s: 2,
        audio_kbps: 128,
        hevc: true,
        // At 4K YouTube recommends 35 Mbit/s HEVC against 50 Mbit/s H.264.
        hevc_from_height: 2160,
        rates: YOUTUBE_RATES,
    },
    Preset {
        service: Service::Kick,
        name: "Kick",
        url: "rtmps://fa723fc1b171.global-contribute.live-video.net:443/app",
        key_help: "Kick: Dashboard > Settings > Stream URL & Key",
        max_video_kbps: 8000,
        max_width: 1920,
        max_height: 1080,
        max_fps: 60,
        max_pixel_rate: PX_1080P60,
        keyframe_s: 2,
        audio_kbps: 160,
        hevc: false,
        hevc_from_height: 0,
        rates: &[],
    },
    Preset {
        service: Service::Facebook,
        name: "Facebook Live",
        url: "rtmps://live-api-s.facebook.com:443/rtmp/",
        key_help: "Facebook: Live Producer > Streaming software > Stream key",
        max_video_kbps: 9000,
        max_width: 1920,
        max_height: 1080,
        max_fps: 60,
        max_pixel_rate: PX_1080P60,
        keyframe_s: 2,
        audio_kbps: 128,
        hevc: false,
        hevc_from_height: 0,
        rates: &[],
    },
    Preset {
        service: Service::Restream,
        name: "Restream",
        url: "rtmp://live.restream.io/live",
        key_help: "Restream: Dashboard > Streaming setup > Stream key",
        max_video_kbps: 8000,
        max_width: 1920,
        max_height: 1080,
        max_fps: 60,
        max_pixel_rate: PX_1080P60,
        keyframe_s: 2,
        audio_kbps: 160,
        hevc: false,
        hevc_from_height: 0,
        rates: &[],
    },
    Preset {
        service: Service::Custom,
        name: "Custom server",
        url: "",
        key_help: "The server's address (rtmp:// or rtmps://) and, if it uses one, the stream key",
        max_video_kbps: 80_000,
        max_width: 3840,
        max_height: 2160,
        max_fps: 120,
        max_pixel_rate: PX_2160P60,
        keyframe_s: 2,
        audio_kbps: 160,
        hevc: true,
        hevc_from_height: 0,
        rates: CUSTOM_RATES,
    },
];

impl Preset {
    /// `w` × `h` brought within the service's largest picture, in proportion, even numbers.
    pub fn fit_size(&self, w: u32, h: u32) -> (u32, u32) {
        let (mut w, mut h) = (w.max(2), h.max(2));
        if w > self.max_width || h > self.max_height {
            let s = (self.max_width as f64 / w as f64).min(self.max_height as f64 / h as f64);
            w = ((w as f64 * s) as u32).max(2);
            h = ((h as f64 * s) as u32).max(2);
        }
        (w & !1, h & !1)
    }

    /// Whether `fps` frames a second of `w` × `h` are within the service's limits.
    pub fn takes(&self, w: u32, h: u32, fps: u32) -> bool {
        w <= self.max_width && h <= self.max_height && self.rate_ok(w, h, fps)
    }

    /// The frame rate and the pixel rate alone.
    fn rate_ok(&self, w: u32, h: u32, fps: u32) -> bool {
        fps >= 1 && fps <= self.max_fps && w as u64 * h as u64 * fps as u64 <= self.max_pixel_rate
    }

    /// The frame rate to send for `w` × `h` when `want` is asked for: `want` when the service
    /// takes it, else the fastest of `FRAME_RATES` below it that fits (else the most that fits).
    pub fn fps_for(&self, w: u32, h: u32, want: u32) -> u32 {
        let want = want.clamp(1, self.max_fps);
        if self.rate_ok(w, h, want) {
            return want;
        }
        if let Some(&f) = FRAME_RATES.iter().rev().find(|&&f| f < want && self.rate_ok(w, h, f)) {
            return f;
        }
        let px = (w as u64 * h as u64).max(1);
        (self.max_pixel_rate / px).clamp(1, want as u64) as u32
    }

    /// Whether `fps` can be picked with pictures `height` tall (16:9 assumed, as the size list
    /// gives no width); 0: the source's own size, decided when going live.
    pub fn offers(&self, height: u32, fps: u32) -> bool {
        if height == 0 {
            return fps <= self.max_fps;
        }
        let h = height.min(self.max_height);
        self.takes((h * 16 / 9) & !1, h, fps)
    }

    /// The recommended bitrate for `w` × `h` at `fps` with `codec`: the service's table row for
    /// the smallest height and frame rate at least as big (the biggest row beyond the table),
    /// else about 0.1 bit per pixel (`engine::recommended_kbps`, HEVC 60 % of that); never above
    /// what the service takes.
    pub fn recommended_kbps(&self, w: u32, h: u32, fps: u32, codec: crate::encode::Codec) -> u32 {
        let hevc = codec == crate::encode::Codec::Hevc;
        let kbps = match rate_row(self.rates, h, fps) {
            Some(r) => if hevc { r.hevc } else { r.h264 },
            None => {
                let k = super::engine::recommended_kbps(w, h, fps);
                if hevc { (k as u64 * 3 / 5).max(500) as u32 } else { k }
            }
        };
        kbps.min(self.max_video_kbps)
    }
}

/// The row of `rows` for pictures `h` tall at `fps`: the first (rows go smallest first) whose
/// height and frame rate both reach them; past the end, the row with the most pixels a second.
pub fn rate_row(rows: &[RateRow], h: u32, fps: u32) -> Option<RateRow> {
    rows.iter()
        .filter(|r| r.height >= h && r.fps >= fps)
        .min_by_key(|r| (r.height, r.fps))
        .or_else(|| rows.iter().max_by_key(|r| r.height as u64 * r.fps as u64))
        .copied()
}

impl Service {
    pub fn preset(self) -> &'static Preset {
        PRESETS.iter().find(|p| p.service == self).unwrap_or(&PRESETS[PRESETS.len() - 1])
    }

    /// Parse a service name ("twitch", "youtube", "fb", ...) as used on the command line and by MCP.
    pub fn parse(s: &str) -> Option<Service> {
        Some(match s.trim().to_ascii_lowercase().replace([' ', '-', '_'], "").as_str() {
            "twitch" => Service::Twitch,
            "youtube" | "yt" => Service::YouTube,
            "kick" => Service::Kick,
            "facebook" | "facebooklive" | "fb" => Service::Facebook,
            "restream" => Service::Restream,
            "custom" | "rtmp" | "rtmps" => Service::Custom,
            _ => return None,
        })
    }
}

/// Where to stream: a service, its server address and the stream key. The key is secret: it
/// never shows in `Debug` output, errors or logs.
#[derive(Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct StreamTarget {
    pub service: Service,
    /// Empty: the service's usual address.
    pub url: String,
    pub key: String,
}

impl std::fmt::Debug for StreamTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamTarget")
            .field("service", &self.service)
            .field("url", &mask(&self.url, &self.key))
            .field("key", &if self.key.is_empty() { "" } else { "****" })
            .finish()
    }
}

/// `text` with every copy of the secret `key` replaced by `****` (for errors and logs).
pub fn mask(text: &str, key: &str) -> String {
    let key = key.trim();
    if key.len() < 2 { text.to_string() } else { text.replace(key, "****") }
}

/// An RTMP address taken apart.
#[derive(Clone, PartialEq)]
pub struct Endpoint {
    pub tls: bool,
    pub host: String,
    pub port: u16,
    /// The application, e.g. `app` or `live2` (may hold a query such as `app?token=...`).
    pub app: String,
    /// What `connect` sends as tcUrl: `rtmp://host:port/app`.
    pub tc_url: String,
    /// The stream name to publish (the key). Secret.
    pub stream: String,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("tls", &self.tls)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("app", &mask(&self.app, &self.stream))
            .field("stream", &"****")
            .finish()
    }
}

impl StreamTarget {
    /// The address to use: the one given, or the service's usual one.
    pub fn effective_url(&self) -> &str {
        let u = self.url.trim();
        if u.is_empty() { self.service.preset().url } else { u }
    }

    /// Host, port, app and stream name. Rules:
    /// - `rtmp://` defaults to port 1935, `rtmps://` to 443.
    /// - With a key, the whole path is the app and the key is the stream name
    ///   (`rtmp://host/live2` + `abcd` → app `live2`, stream `abcd`).
    /// - Without a key, the last part of the path is the stream name
    ///   (`rtmp://host/live/abcd` → app `live`, stream `abcd`), for servers whose address
    ///   includes the key.
    pub fn endpoint(&self) -> Result<Endpoint, String> {
        let key = self.key.trim();
        let url = self.effective_url();
        let err = |m: &str| Err(format!("{m}: {}", mask(url, key)));
        if url.is_empty() {
            return err("Type the server address (rtmp://... or rtmps://...)");
        }
        let (tls, rest) = if let Some(r) = strip_prefix_ci(url, "rtmps://") {
            (true, r)
        } else if let Some(r) = strip_prefix_ci(url, "rtmp://") {
            (false, r)
        } else {
            return err("The server address must start with rtmp:// or rtmps://");
        };
        let (hostport, path) = rest.split_once('/').unwrap_or((rest, ""));
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) if !h.ends_with(']') || hostport.starts_with('[') => match p.parse::<u16>() {
                Ok(p) if p > 0 => (h, p),
                _ => return err("The port in the server address isn't a number"),
            },
            _ => (hostport, if tls { 443 } else { 1935 }),
        };
        let host = host.trim_start_matches('[').trim_end_matches(']');
        if host.is_empty() {
            return err("The server address has no host name");
        }
        let path = path.trim_matches('/');
        let (app, stream) = if !key.is_empty() {
            (path.to_string(), key.to_string())
        } else {
            match path.rsplit_once('/') {
                Some((a, s)) if !a.is_empty() && !s.is_empty() => (a.to_string(), s.to_string()),
                _ => return err("Type the stream key (or add it to the end of the server address)"),
            }
        };
        if app.is_empty() {
            return err("The server address needs an application, e.g. rtmp://server/live");
        }
        let scheme = if tls { "rtmps" } else { "rtmp" };
        let tc_url = format!("{scheme}://{hostport}/{app}");
        Ok(Endpoint { tls, host: host.to_string(), port, app, tc_url, stream })
    }

    /// The address with the key on the end, masked: what to show the person.
    pub fn display(&self) -> String {
        match self.endpoint() {
            Ok(e) => mask(&format!("{}/****", e.tc_url), &e.stream),
            Err(_) => mask(self.effective_url(), &self.key),
        }
    }
}

fn strip_prefix_ci<'a>(s: &'a str, p: &str) -> Option<&'a str> {
    if s.len() >= p.len() && s.is_char_boundary(p.len()) && s[..p.len()].eq_ignore_ascii_case(p) { Some(&s[p.len()..]) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(service: Service, url: &str, key: &str) -> StreamTarget {
        StreamTarget { service, url: url.into(), key: key.into() }
    }

    #[test]
    fn every_service_has_a_preset_and_a_parseable_address() {
        for p in PRESETS {
            assert_eq!(p.service.preset().name, p.name);
            assert_eq!(p.keyframe_s, 2);
            if p.service != Service::Custom {
                let e = t(p.service, "", "live_123").endpoint().unwrap();
                assert!(!e.app.is_empty(), "{}", p.name);
                assert_eq!(e.stream, "live_123");
            }
        }
        assert_eq!(Service::parse("YouTube"), Some(Service::YouTube));
        assert_eq!(Service::parse("facebook-live"), Some(Service::Facebook));
        assert_eq!(Service::parse("nope"), None);
    }

    #[test]
    fn limits_youtube_to_4k60_and_the_others_to_1080p60() {
        let yt = Service::YouTube.preset();
        assert_eq!((yt.max_width, yt.max_height, yt.max_fps, yt.max_video_kbps), (3840, 2160, 60, 51_000));
        assert!(yt.takes(3840, 2160, 60));
        assert!(!yt.takes(1920, 1080, 120), "YouTube takes at most 60 fps");
        for s in [Service::Twitch, Service::Kick, Service::Facebook, Service::Restream] {
            let p = s.preset();
            assert_eq!((p.max_height, p.max_fps, p.max_pixel_rate), (1080, 60, PX_1080P60), "{}", p.name);
            assert_eq!(p.fit_size(3840, 2160), (1920, 1080));
            assert!(p.takes(1920, 1080, 60) && !p.takes(2560, 1440, 30));
            assert!(!p.hevc && p.hevc_from_height == 0);
        }
        assert_eq!(yt.fit_size(5120, 2880), (3840, 2160));
        assert_eq!(yt.fit_size(3841, 2161), (3838, 2160), "in proportion, rounded down to even");
        assert_eq!(yt.fit_size(2561, 1601), (2560, 1600), "fits already: only made even");
    }

    #[test]
    fn the_custom_servers_pixel_rate_keeps_4k_at_60_and_allows_120_up_to_1440p() {
        let c = Service::Custom.preset();
        assert_eq!(c.fps_for(3840, 2160, 120), 60, "4K at most 60");
        assert_eq!(c.fps_for(3840, 2160, 60), 60);
        assert_eq!(c.fps_for(2560, 1440, 120), 120);
        assert_eq!(c.fps_for(2560, 1600, 120), 120, "2560 x 1600 x 120 is just under 3840 x 2160 x 60");
        assert_eq!(c.fps_for(1920, 1080, 120), 120);
        assert_eq!(c.fps_for(1920, 1080, 500), 120, "never over the most it takes");
        assert_eq!(c.fps_for(3840, 2400, 120), 50, "steps down to a usual rate that fits");
        assert_eq!(c.fps_for(7680, 4320, 60), 15, "no usual rate fits: the most that does");
        assert_eq!(Service::Twitch.preset().fps_for(1920, 1080, 120), 60);
        assert_eq!(c.fps_for(16, 16, 0), 1);
        // What the Settings page offers.
        assert!(c.offers(1440, 120) && c.offers(1080, 120) && c.offers(2160, 60));
        assert!(!c.offers(2160, 120), "4K at 120 isn't offered");
        assert!(c.offers(0, 120), "the source's size: decided when going live");
        assert!(!Service::YouTube.preset().offers(720, 120));
        assert!(Service::YouTube.preset().offers(2160, 60));
        assert!(!Service::Twitch.preset().offers(0, 120));
    }

    #[test]
    fn bitrates_follow_youtubes_table_with_less_for_hevc() {
        use crate::encode::Codec::{H264, Hevc};
        let yt = Service::YouTube.preset();
        assert_eq!(yt.recommended_kbps(3840, 2160, 60, H264), 50_000);
        assert_eq!(yt.recommended_kbps(3840, 2160, 60, Hevc), 35_000);
        assert_eq!(yt.recommended_kbps(3840, 2160, 30, H264), 42_000);
        assert_eq!(yt.recommended_kbps(2560, 1440, 60, H264), 34_000);
        assert_eq!(yt.recommended_kbps(1920, 1080, 60, H264), 17_000);
        assert_eq!(yt.recommended_kbps(1920, 1080, 30, Hevc), 10_000);
        assert_eq!(yt.recommended_kbps(1280, 720, 30, H264), 8_000);
        assert_eq!(yt.recommended_kbps(854, 480, 30, H264), 4_000);
        assert_eq!(yt.recommended_kbps(1700, 900, 25, H264), 14_000, "between rows: the next bigger one");
        let c = Service::Custom.preset();
        assert_eq!(c.recommended_kbps(1920, 1080, 120, H264), 24_000);
        assert_eq!(c.recommended_kbps(2560, 1440, 120, H264), 48_000);
        assert_eq!(c.recommended_kbps(2560, 1600, 60, H264), 38_000);
        assert_eq!(c.recommended_kbps(3840, 2160, 60, Hevc), 35_000);
        // Services without a table: about 0.1 bit a pixel, never over their maximum.
        let tw = Service::Twitch.preset();
        assert_eq!(tw.recommended_kbps(1920, 1080, 60, H264), 6000);
        assert_eq!(tw.recommended_kbps(1280, 720, 30, H264), super::super::engine::recommended_kbps(1280, 720, 30));
        assert!(Service::Kick.preset().recommended_kbps(1280, 720, 30, Hevc) < Service::Kick.preset().recommended_kbps(1280, 720, 30, H264));
        // Every table goes smallest first, and HEVC always needs less.
        for p in PRESETS {
            assert!(p.rates.iter().all(|r| r.hevc < r.h264 && r.h264 <= p.max_video_kbps), "{}", p.name);
            assert!(p.rates.windows(2).all(|w| (w[0].height, w[0].fps) < (w[1].height, w[1].fps)), "{}", p.name);
        }
        assert_eq!(rate_row(&[], 1080, 30), None);
        assert_eq!(rate_row(YOUTUBE_RATES, 4320, 60).map(|r| r.height), Some(2160), "past the end: the biggest row");
    }

    #[test]
    fn key_is_the_stream_name_and_the_path_is_the_app() {
        let e = t(Service::YouTube, "", "abcd-efgh").endpoint().unwrap();
        assert!(e.tls);
        assert_eq!((e.host.as_str(), e.port, e.app.as_str()), ("a.rtmps.youtube.com", 443, "live2"));
        assert_eq!(e.tc_url, "rtmps://a.rtmps.youtube.com:443/live2");
        let e = t(Service::Facebook, "", "FB-1-2?s_bl=1").endpoint().unwrap();
        assert_eq!((e.app.as_str(), e.stream.as_str()), ("rtmp", "FB-1-2?s_bl=1"));
        let e = t(Service::Custom, "rtmp://127.0.0.1/live/sub/", " k ").endpoint().unwrap();
        assert_eq!((e.port, e.app.as_str(), e.stream.as_str()), (1935, "live/sub", "k"));
        assert_eq!(e.tc_url, "rtmp://127.0.0.1/live/sub");
    }

    #[test]
    fn without_a_key_the_last_path_part_is_the_stream() {
        let e = t(Service::Custom, "RTMP://example.com:1936/live/secret", "").endpoint().unwrap();
        assert!(!e.tls);
        assert_eq!((e.port, e.app.as_str(), e.stream.as_str()), (1936, "live", "secret"));
        assert!(t(Service::Custom, "rtmp://example.com/live", "").endpoint().is_err());
    }

    #[test]
    fn bad_addresses_are_explained() {
        assert!(t(Service::Custom, "", "k").endpoint().unwrap_err().contains("server address"));
        assert!(t(Service::Custom, "http://x/app", "k").endpoint().is_err());
        assert!(t(Service::Custom, "rtmp://x:abc/app", "k").endpoint().is_err());
        assert!(t(Service::Custom, "rtmp:///app", "k").endpoint().is_err());
        assert!(t(Service::Custom, "rtmp://host", "k").endpoint().is_err());
        let e = t(Service::Custom, "rtmp://[::1]:1937/app", "k").endpoint().unwrap();
        assert_eq!((e.host.as_str(), e.port), ("::1", 1937));
    }

    #[test]
    fn the_key_never_shows() {
        let target = t(Service::Custom, "rtmp://h/live/sk_secret_99", "sk_secret_99");
        let dbg = format!("{target:?}");
        assert!(!dbg.contains("sk_secret_99"), "{dbg}");
        let e = target.endpoint().unwrap();
        assert!(!format!("{e:?}").contains("sk_secret_99"));
        assert!(!target.display().contains("sk_secret_99"));
        let bad = t(Service::Custom, "ftp://h/sk_secret_99", "sk_secret_99");
        assert!(!bad.endpoint().unwrap_err().contains("sk_secret_99"));
        assert!(!bad.display().contains("sk_secret_99"));
        assert_eq!(mask("bad key sk_secret_99!", "sk_secret_99"), "bad key ****!");
        assert_eq!(mask("abc", ""), "abc");
    }
}
