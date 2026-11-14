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
    /// Seconds between keyframes the service asks for.
    pub keyframe_s: u32,
    pub audio_kbps: u32,
    /// Takes HEVC over Enhanced RTMP.
    pub hevc: bool,
}

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
        keyframe_s: 2,
        audio_kbps: 160,
        hevc: false,
    },
    Preset {
        service: Service::YouTube,
        name: "YouTube",
        url: "rtmps://a.rtmps.youtube.com:443/live2",
        key_help: "YouTube Studio > Go live > Stream > Stream key",
        max_video_kbps: 12000,
        max_width: 1920,
        max_height: 1080,
        max_fps: 60,
        keyframe_s: 2,
        audio_kbps: 128,
        hevc: true,
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
        keyframe_s: 2,
        audio_kbps: 160,
        hevc: false,
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
        keyframe_s: 2,
        audio_kbps: 128,
        hevc: false,
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
        keyframe_s: 2,
        audio_kbps: 160,
        hevc: false,
    },
    Preset {
        service: Service::Custom,
        name: "Custom server",
        url: "",
        key_help: "The server's address (rtmp:// or rtmps://) and, if it uses one, the stream key",
        max_video_kbps: 50_000,
        max_width: 3840,
        max_height: 2160,
        max_fps: 120,
        keyframe_s: 2,
        audio_kbps: 160,
        hevc: true,
    },
];

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
