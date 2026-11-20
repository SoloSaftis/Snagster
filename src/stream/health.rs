//! How well a live stream is going, from what it really does: the stream's threads only add to
//! counters (atomics, no locks); `Sampler` keeps snapshots of those running totals every quarter
//! of a second for the last ten seconds, and `Sampler::health` turns them into rates and
//! plain-words warnings.
//!
//! What is told apart:
//! - **Upload**: frames each service's send queue threw away because the upload fell behind,
//!   the bitrate it really sent, and how far its queue is behind.
//! - **Computer**: pictures the encoder threw away or frame slots the picture thread missed
//!   (it couldn't keep up), and how much of each frame's time the encoder takes.
//! - **Connection**: how long the current connection has been up, and recent reconnects.
//!
//! `Steady` is what the window shows of the warnings: one at a time, without flickering.

use std::collections::VecDeque;

/// Rates cover this much of the recent past.
pub const RATE_WINDOW_MS: u64 = 10_000;
/// Reconnects count as recent for this long.
pub const RECONNECT_WINDOW_MS: u64 = 600_000;
/// No rates before this much has been seen (on the stream, or on a service's connection).
pub const MIN_WINDOW_MS: u64 = 5_000;
/// No upload or computer warnings in the first seconds after going live or reconnecting (the
/// first keyframe, the connection settling).
pub const QUIET_START_MS: u64 = 10_000;
/// At most one snapshot this often.
pub const SAMPLE_EVERY_MS: u64 = 250;

/// Dropped frames over the window: a warning from this share on…
pub const DROP_WARN: f32 = 0.01;
/// …and bad from this one.
pub const DROP_BAD: f32 = 0.05;
/// An upload queue this far behind is a warning even before frames are dropped, and bad from
/// where frames start being dropped (`StreamConfig::max_queue_ms`, 2 s).
pub const QUEUE_WARN_MS: u32 = 1000;
pub const QUEUE_BAD_MS: u32 = 2000;
/// An encoder taking this share of each frame's time has no room left.
pub const LOAD_WARN: f32 = 0.9;
/// Reconnects in the last ten minutes that make a warning.
pub const RECONNECTS_WARN: u32 = 2;
/// A warning shows once it has held this long (a bad one at once)…
pub const HOLD_MS: u64 = 3_000;
/// …and stays at least this long after it clears.
pub const KEEP_MS: u64 = 10_000;

/// One service's running totals.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OutTotals {
    /// Bytes written to the server.
    pub bytes: u64,
    /// Video frames sent.
    pub sent: u64,
    /// Video frames its queue threw away (the upload fell behind, or a reconnect).
    pub dropped: u64,
    pub reconnects: u32,
}

/// The whole stream's running totals at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Totals {
    /// Milliseconds since the stream started.
    pub at_ms: u64,
    /// Pictures handed to the video encoder.
    pub frames_in: u64,
    /// Pictures the encoder threw away plus frame slots missed (the computer fell behind).
    pub encoder_lost: u64,
    /// Of those, the frame slots missed (they never reached the encoder, so they aren't in
    /// `frames_in`; the pictures the encoder threw away are).
    pub late: u64,
    /// Microseconds spent inside the video encoder.
    pub encode_us: u64,
    /// Each service, the main one first.
    pub outputs: Vec<OutTotals>,
}

/// One service as it is right now (not a total).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutNow {
    pub service: String,
    /// Sending (not connecting, reconnecting or failed).
    pub live: bool,
    /// How much is waiting to be sent.
    pub queue_ms: u32,
    /// How long the current connection has been up (0 when it isn't).
    pub connected_ms: u64,
    /// When it went live, ms since the stream started (0: it isn't). Its rates never reach back
    /// before this: the frames thrown away at a reconnect belong to the break, not the upload.
    pub live_since_ms: u64,
}

/// What the stream was asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Asked {
    pub fps: u32,
    /// Video plus sound, kbit/s.
    pub kbps: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Warning,
    Bad,
}

/// In the order they are shown when several are up at the same level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Problem {
    /// The upload can't carry the stream.
    Upload,
    /// The computer can't encode fast enough.
    Computer,
    /// The connection keeps dropping.
    Connection,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Warning {
    pub problem: Problem,
    pub level: Level,
    /// The service it is about (None: the computer, for every service).
    pub service: Option<String>,
}

impl Warning {
    /// A few words for the band and the panel (English with `{service}`; the window translates
    /// it before filling it in).
    pub fn short(&self) -> &'static str {
        match self.problem {
            Problem::Upload => "Upload to {service} too slow",
            Problem::Computer => "Computer too slow to encode",
            Problem::Connection => "{service} keeps reconnecting",
        }
    }

    /// In plain words, with what to do (English with `{service}`).
    pub fn advice(&self) -> &'static str {
        match self.problem {
            Problem::Upload => "Your upload to {service} can't keep up: lower the bitrate or the resolution.",
            Problem::Computer => "Your computer can't encode fast enough: lower the resolution or the frame rate.",
            Problem::Connection => "The connection to {service} keeps dropping: check your network.",
        }
    }

    /// `text` with the service filled in.
    pub fn fill(&self, text: &str) -> String {
        text.replace("{service}", self.service.as_deref().unwrap_or_default())
    }

    /// The same problem with the same service (the level may differ).
    fn same(&self, o: &Warning) -> bool {
        self.problem == o.problem && self.service == o.service
    }
}

/// One service's health.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutHealth {
    pub service: String,
    /// Its rates are known (enough seen on this connection); else they read "measuring".
    pub measured: bool,
    /// What it really sent over the window, kbit/s.
    pub sent_kbps: u32,
    /// Frames its queue threw away since the stream started, and the share over the window.
    pub dropped: u64,
    pub dropped_share: f32,
    pub queue_ms: u32,
    /// How long the current connection has been up.
    pub connected_s: f64,
    pub reconnects: u32,
    /// Reconnects in the last ten minutes.
    pub reconnects_recent: u32,
}

/// How the stream is going.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Health {
    /// The seconds the rates cover (0: not enough seen yet; the rates are 0 and only the
    /// connection can warn).
    pub window_s: f32,
    /// What was asked for: video plus sound, kbit/s.
    pub set_kbps: u32,
    /// The time one frame may take at the stream's frame rate.
    pub frame_budget_ms: f32,
    /// The encoder's average time per picture over the window.
    pub encode_ms: f32,
    /// The share of the time the encoder was busy over the window (above 1 when it can't keep up).
    pub encoder_load: f32,
    /// Pictures the computer lost (encoder threw away, slots missed) since the start, and the
    /// share over the window.
    pub encoder_lost: u64,
    pub encoder_lost_share: f32,
    pub outputs: Vec<OutHealth>,
    /// Worst first.
    pub warnings: Vec<Warning>,
}

impl Health {
    /// The worst warning's level (None: all is well).
    pub fn level(&self) -> Option<Level> {
        self.warnings.iter().map(|w| w.level).max()
    }
}

/// The snapshots of the last ten seconds, and when services reconnected in the last ten
/// minutes.
#[derive(Debug, Default)]
pub struct Sampler {
    samples: VecDeque<Totals>,
    /// (ms, service index) of each reconnect seen.
    reconnects: VecDeque<(u64, usize)>,
}

/// `part` of `part + rest`, 0 when both are 0.
fn share(part: u64, rest: u64) -> f32 {
    if part + rest == 0 { 0.0 } else { part as f32 / (part + rest) as f32 }
}

fn level_of(share: f32) -> Option<Level> {
    if share >= DROP_BAD {
        Some(Level::Bad)
    } else if share >= DROP_WARN {
        Some(Level::Warning)
    } else {
        None
    }
}

impl Sampler {
    /// Whether a snapshot is due at `at_ms` (so a caller can skip reading the counters).
    pub fn due(&self, at_ms: u64) -> bool {
        self.samples.back().is_none_or(|l| at_ms >= l.at_ms + SAMPLE_EVERY_MS)
    }

    /// Keep a snapshot (ignored when one was kept less than `SAMPLE_EVERY_MS` ago).
    pub fn push(&mut self, t: Totals) {
        if !self.due(t.at_ms) {
            return;
        }
        let newest = t.at_ms;
        if let Some(last) = self.samples.back() {
            for (i, o) in t.outputs.iter().enumerate() {
                let before = last.outputs.get(i).map_or(0, |l| l.reconnects);
                for _ in before..o.reconnects {
                    self.reconnects.push_back((newest, i));
                }
            }
        }
        self.samples.push_back(t);
        // Keep one snapshot at or before the rate window's start.
        while self.samples.len() > 2 && self.samples[1].at_ms + RATE_WINDOW_MS <= newest {
            self.samples.pop_front();
        }
        while self.reconnects.front().is_some_and(|r| r.0 + RECONNECT_WINDOW_MS <= newest) {
            self.reconnects.pop_front();
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// The newest snapshot at least `ms` before the newest one, else the oldest.
    fn back_by(&self, ms: u64) -> Option<&Totals> {
        let newest = self.samples.back()?.at_ms;
        self.samples.iter().rev().find(|s| s.at_ms + ms <= newest).or(self.samples.front())
    }

    /// Rates over the last `RATE_WINDOW_MS` and the warnings they call for; `now` is how each
    /// service is right now (in the same order as the totals).
    pub fn health(&self, asked: Asked, now: &[OutNow]) -> Health {
        let budget = 1000.0 / asked.fps.max(1) as f32;
        let mut h = Health { set_kbps: asked.kbps, frame_budget_ms: budget, ..Default::default() };
        let (Some(new), Some(old)) = (self.samples.back(), self.back_by(RATE_WINDOW_MS)) else {
            h.outputs = now.iter().map(|n| OutHealth { service: n.service.clone(), queue_ms: n.queue_ms, connected_s: n.connected_ms as f64 / 1000.0, ..Default::default() }).collect();
            return h;
        };
        let span_ms = new.at_ms.saturating_sub(old.at_ms);
        let enough = span_ms >= MIN_WINDOW_MS;
        h.encoder_lost = new.encoder_lost;
        if enough {
            h.window_s = span_ms as f32 / 1000.0;
            let frames = new.frames_in.saturating_sub(old.frames_in);
            let lost = new.encoder_lost.saturating_sub(old.encoder_lost);
            let enc_us = new.encode_us.saturating_sub(old.encode_us);
            h.encode_ms = if frames == 0 { 0.0 } else { enc_us as f32 / frames as f32 / 1000.0 };
            h.encoder_load = enc_us as f32 / (span_ms as f32 * 1000.0);
            // Of every slot due: the pictures handed over plus the slots missed.
            let late = new.late.saturating_sub(old.late);
            h.encoder_lost_share = share(lost, (frames + late).saturating_sub(lost));
            let mut level = level_of(h.encoder_lost_share);
            if h.encoder_load >= LOAD_WARN {
                level = level.max(Some(Level::Warning));
            }
            // Not in the stream's first seconds.
            if let Some(level) = level.filter(|_| new.at_ms >= QUIET_START_MS) {
                h.warnings.push(Warning { problem: Problem::Computer, level, service: None });
            }
        }
        for (i, n) in now.iter().enumerate() {
            let get = |t: &Totals| t.outputs.get(i).copied().unwrap_or_default();
            // This service's window: the last ten seconds, but none of it before this
            // connection went live (strictly after: the frames lost at the reconnect were
            // counted just before it went live, possibly in the same millisecond).
            let since = self.samples.iter().find(|s| s.at_ms >= old.at_ms && s.at_ms > n.live_since_ms).unwrap_or(new);
            let span_ms = new.at_ms.saturating_sub(since.at_ms);
            let (a, b) = (get(since), get(new));
            let mut o = OutHealth {
                service: n.service.clone(),
                dropped: b.dropped,
                queue_ms: n.queue_ms,
                connected_s: n.connected_ms as f64 / 1000.0,
                reconnects: b.reconnects,
                reconnects_recent: self.reconnects.iter().filter(|r| r.1 == i).count() as u32,
                ..Default::default()
            };
            let service = Some(n.service.clone());
            if n.live && span_ms >= MIN_WINDOW_MS {
                o.measured = true;
                o.sent_kbps = (b.bytes.saturating_sub(a.bytes) as f64 * 8.0 / span_ms as f64) as u32;
                o.dropped_share = share(b.dropped.saturating_sub(a.dropped), b.sent.saturating_sub(a.sent));
                let mut level = level_of(o.dropped_share);
                if n.queue_ms >= QUEUE_BAD_MS {
                    level = Some(Level::Bad);
                } else if n.queue_ms >= QUEUE_WARN_MS {
                    level = level.max(Some(Level::Warning));
                }
                // Not in this connection's first seconds.
                if let Some(level) = level.filter(|_| n.connected_ms >= QUIET_START_MS) {
                    h.warnings.push(Warning { problem: Problem::Upload, level, service: service.clone() });
                }
            }
            if o.reconnects_recent >= RECONNECTS_WARN {
                h.warnings.push(Warning { problem: Problem::Connection, level: Level::Warning, service });
            }
            h.outputs.push(o);
        }
        // Worst first; at the same level the upload, then the computer, then the connection.
        h.warnings.sort_by(|a, b| b.level.cmp(&a.level).then(a.problem.cmp(&b.problem)));
        h
    }
}

/// The one warning the window shows, steady: a warning appears once it has held for `HOLD_MS`
/// (a bad one at once), a worse one replaces it at once, and it stays `KEEP_MS` after it
/// clears, so nothing flickers.
#[derive(Clone, Debug, Default)]
pub struct Steady {
    shown: Option<Warning>,
    /// When `shown` was last true, and last true at its level or worse (a milder level of the
    /// same problem takes its place only `KEEP_MS` after that).
    seen_ms: u64,
    level_seen_ms: u64,
    /// The top warning now, and since when it has held.
    held: Option<(Warning, u64)>,
}

impl Steady {
    /// Take the warnings at `at_ms` (worst first, as `Health::warnings`); returns what to show.
    pub fn update(&mut self, at_ms: u64, warnings: &[Warning]) -> Option<&Warning> {
        match warnings.first() {
            Some(c) => {
                if !self.held.as_ref().is_some_and(|h| h.0.same(c)) {
                    self.held = Some((c.clone(), at_ms));
                }
                let since = self.held.as_ref().map_or(at_ms, |h| h.1);
                let ready = c.level == Level::Bad || at_ms >= since + HOLD_MS;
                let take = match &self.shown {
                    None => ready,
                    // The same problem: worse at once, milder once the worse level has gone.
                    Some(s) if s.same(c) => c.level >= s.level || at_ms >= self.level_seen_ms + KEEP_MS,
                    Some(s) => ready && (c.level > s.level || at_ms >= self.seen_ms + KEEP_MS),
                };
                if take {
                    self.shown = Some(c.clone());
                    self.seen_ms = at_ms;
                    self.level_seen_ms = at_ms;
                }
            }
            None => self.held = None,
        }
        // The one shown is still true further down the list: it stays.
        if let Some(s) = &self.shown {
            for w in warnings.iter().filter(|w| w.same(s)) {
                self.seen_ms = at_ms;
                if w.level >= s.level {
                    self.level_seen_ms = at_ms;
                }
            }
        }
        if self.shown.is_some() && at_ms >= self.seen_ms + KEEP_MS {
            self.shown = None;
        }
        self.shown.as_ref()
    }

    /// Forget everything (the stream ended).
    pub fn reset(&mut self) {
        *self = Steady::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(bytes: u64, sent: u64, dropped: u64, reconnects: u32) -> OutTotals {
        OutTotals { bytes, sent, dropped, reconnects }
    }

    /// Totals whose lost pictures are all late slots (see `the_computers_share_counts_every_slot`
    /// for pictures the encoder threw away).
    fn tot(at_ms: u64, frames_in: u64, encoder_lost: u64, encode_us: u64, outputs: Vec<OutTotals>) -> Totals {
        Totals { at_ms, frames_in, encoder_lost, late: encoder_lost, encode_us, outputs }
    }

    fn now(service: &str, queue_ms: u32) -> OutNow {
        OutNow { service: service.into(), live: true, queue_ms, connected_ms: 60_000, live_since_ms: 0 }
    }

    /// The reviewer's case: a one-second break at 50 s; the 60 frames still queued are thrown
    /// away at the reconnect (51.01 s), live again at 51.03 s with an upload that keeps up.
    /// Ten seconds on (the end of the quiet start) nothing may warn, and the numbers are this
    /// connection's own.
    #[test]
    fn frames_lost_at_a_reconnect_are_not_blamed_on_the_upload() {
        let mut s = Sampler::default();
        let (mut sent, mut dropped, mut bytes, mut reconnects) = (0u64, 0u64, 0u64, 0u32);
        let live_since = 51_030;
        for ms in (0..=70_000u64).step_by(10) {
            if (50_000..51_030).contains(&ms) {
                // The break: nothing goes out.
            } else if ms % 100 == 0 {
                sent += 3;
                bytes += 75_000;
            }
            if ms == 51_010 {
                dropped += 60;
                reconnects += 1;
            }
            s.push(tot(ms, ms * 30 / 1000, 0, 0, vec![out(bytes, sent, dropped, reconnects)]));
            if ms >= live_since + QUIET_START_MS {
                let n = OutNow { service: "Twitch".into(), live: true, queue_ms: 0, connected_ms: ms - live_since, live_since_ms: live_since };
                let h = s.health(ASKED, &[n]);
                assert!(h.warnings.is_empty(), "at {ms}: {:?} {:?}", h.warnings, h.outputs[0]);
                assert_eq!(h.outputs[0].dropped_share, 0.0, "at {ms}");
                assert_eq!(h.outputs[0].dropped, 60, "the total still counts them");
            }
        }
        // Seven seconds after the reconnect: measured over those seven seconds only.
        let mut s2 = Sampler::default();
        let (mut sent, mut bytes) = (0u64, 0u64);
        for ms in (0..=58_000u64).step_by(250) {
            let up = !(50_000..51_030).contains(&ms);
            if up {
                sent += 7;
                bytes += 187_500;
            }
            s2.push(tot(ms, ms * 30 / 1000, 0, 0, vec![out(bytes, sent, if ms >= 51_010 { 60 } else { 0 }, 0)]));
        }
        let n = OutNow { service: "Twitch".into(), live: true, queue_ms: 0, connected_ms: 6970, live_since_ms: live_since };
        let h = s2.health(ASKED, &[n]);
        assert!(h.outputs[0].measured);
        assert_eq!((h.outputs[0].sent_kbps, h.outputs[0].dropped_share), (6000, 0.0), "{:?}", h.outputs[0]);
    }

    /// Pictures the encoder threw away were handed to it (they are in `frames_in`); slots missed
    /// were not. Both count against every slot that was due.
    #[test]
    fn the_computers_share_counts_every_slot() {
        let mut s = Sampler::default();
        s.push(Totals { at_ms: 10_000, outputs: vec![out(0, 0, 0, 0)], ..Default::default() });
        // 300 pictures handed over, 15 of them thrown away by the encoder: 5 %.
        s.push(Totals { at_ms: 20_000, frames_in: 300, encoder_lost: 15, late: 0, encode_us: 0, outputs: vec![out(7_500_000, 285, 0, 0)] });
        let h = s.health(ASKED, &[now("Twitch", 0)]);
        assert!((h.encoder_lost_share - 0.05).abs() < 1e-6, "{}", h.encoder_lost_share);
        assert_eq!(h.level(), Some(Level::Bad));
        // 285 handed over and 15 slots missed: also 5 %.
        let mut s = Sampler::default();
        s.push(Totals { at_ms: 10_000, outputs: vec![out(0, 0, 0, 0)], ..Default::default() });
        s.push(Totals { at_ms: 20_000, frames_in: 285, encoder_lost: 15, late: 15, encode_us: 0, outputs: vec![out(7_500_000, 285, 0, 0)] });
        assert!((s.health(ASKED, &[now("Twitch", 0)]).encoder_lost_share - 0.05).abs() < 1e-6);
    }

    fn warn(problem: Problem, level: Level, service: Option<&str>) -> Warning {
        Warning { problem, level, service: service.map(str::to_string) }
    }

    const ASKED: Asked = Asked { fps: 30, kbps: 6160 };

    /// A steady 30 fps stream at `kbps`, every frame `encode_ms`, for `secs` seconds, sampled
    /// every 500 ms; `drop_every` throws away one frame in that many on the network (0: none).
    fn steady(secs: u64, kbps: u64, encode_ms: u64, drop_every: u64) -> Sampler {
        let mut s = Sampler::default();
        for i in 0..=secs * 2 {
            let ms = i * 500;
            let frames = ms * 30 / 1000;
            let dropped = if drop_every == 0 { 0 } else { frames / drop_every };
            s.push(tot(ms, frames, 0, frames * encode_ms * 1000, vec![out(ms * kbps / 8, frames - dropped, dropped, 0)]));
        }
        s
    }

    #[test]
    fn snapshots_are_kept_at_most_every_quarter_second_for_ten_seconds() {
        let mut s = Sampler::default();
        s.push(tot(0, 0, 0, 0, vec![]));
        s.push(tot(100, 0, 0, 0, vec![]));
        assert_eq!(s.len(), 1, "too soon");
        assert!(!s.due(249) && s.due(250));
        for ms in (250..=200_000).step_by(250) {
            s.push(tot(ms, 0, 0, 0, vec![]));
        }
        // Ten seconds plus the one snapshot at their start.
        assert_eq!(s.len(), (RATE_WINDOW_MS / SAMPLE_EVERY_MS) as usize + 1);
        assert_eq!(s.back_by(RATE_WINDOW_MS).unwrap().at_ms, 190_000);
    }

    #[test]
    fn nothing_is_measured_before_five_seconds() {
        let h = Sampler::default().health(ASKED, &[now("Twitch", 5000)]);
        assert_eq!((h.window_s, h.outputs.len(), h.outputs[0].queue_ms, h.outputs[0].measured), (0.0, 1, 5000, false));
        assert!(h.warnings.is_empty());
        // Four seconds of a stream dropping everything: too early to say.
        let mut s = Sampler::default();
        s.push(tot(0, 0, 0, 0, vec![out(0, 0, 0, 0)]));
        s.push(tot(4000, 120, 120, 0, vec![out(1000, 0, 120, 0)]));
        let h = s.health(ASKED, &[now("Twitch", 3000)]);
        assert_eq!((h.window_s, h.outputs[0].measured), (0.0, false));
        assert!(h.warnings.is_empty(), "{:?}", h.warnings);
        assert_eq!((h.outputs[0].dropped, h.encoder_lost), (120, 120), "the totals are shown from the start");
    }

    /// The first ten seconds of the stream, or of a service's connection, warn of nothing but
    /// reconnects: the first keyframe and the connection settling look like trouble.
    #[test]
    fn the_first_ten_seconds_are_quiet() {
        let s = steady(9, 6000, 31, 10);
        let mut n = now("Twitch", 2500);
        n.connected_ms = 9000;
        assert!(s.health(ASKED, &[n]).warnings.is_empty());
        let s = steady(30, 6000, 5, 10);
        let mut n = now("Twitch", 0);
        let connected = |n: &mut OutNow, ms: u64| {
            n.connected_ms = ms;
            n.live_since_ms = 30_000 - ms;
        };
        connected(&mut n, 4000);
        let h = s.health(ASKED, &[n.clone()]);
        assert!(!h.outputs[0].measured && h.warnings.is_empty(), "just reconnected: measuring");
        connected(&mut n, 8000);
        let h = s.health(ASKED, &[n.clone()]);
        assert!(h.outputs[0].measured && h.warnings.is_empty(), "measured, not yet judged");
        connected(&mut n, 10_000);
        assert_eq!(s.health(ASKED, &[n]).level(), Some(Level::Bad));
    }

    #[test]
    fn rates_cover_the_last_ten_seconds() {
        let s = steady(30, 6000, 5, 0);
        let h = s.health(ASKED, &[now("Twitch", 100)]);
        assert_eq!(h.window_s, 10.0);
        assert_eq!(h.outputs[0].sent_kbps, 6000);
        assert!((h.encode_ms - 5.0).abs() < 0.01, "{}", h.encode_ms);
        // 5 ms of every 33.3 ms: 15 % busy.
        assert!((h.encoder_load - 0.15).abs() < 0.001, "{}", h.encoder_load);
        assert!((h.frame_budget_ms - 33.33).abs() < 0.01);
        assert_eq!((h.set_kbps, h.outputs[0].connected_s), (6160, 60.0));
        assert!(h.warnings.is_empty() && h.level().is_none(), "{:?}", h.warnings);
    }

    /// The rates follow the last ten seconds, not the whole stream: a stream that was fine for
    /// a minute and then started dropping warns at once, and clears ten seconds after it stops.
    #[test]
    fn the_window_slides() {
        let mut s = Sampler::default();
        let (mut sent, mut dropped) = (0u64, 0u64);
        let mut push = |s: &mut Sampler, ms: u64, bad: bool| {
            // 15 frames each half second; 3 of them dropped while bad.
            if bad {
                sent += 12;
                dropped += 3;
            } else {
                sent += 15;
            }
            s.push(tot(ms, sent + dropped, 0, 0, vec![out(ms * 750, sent, dropped, 0)]));
        };
        for i in 0..=120 {
            push(&mut s, i * 500, false);
        }
        assert!(s.health(ASKED, &[now("Twitch", 0)]).warnings.is_empty());
        for i in 121..=126 {
            push(&mut s, i * 500, true);
        }
        let h = s.health(ASKED, &[now("Twitch", 0)]);
        assert_eq!(h.warnings, vec![warn(Problem::Upload, Level::Bad, Some("Twitch"))]);
        assert!((h.outputs[0].dropped_share - 18.0 / 300.0).abs() < 0.001, "{}", h.outputs[0].dropped_share);
        for i in 127..=146 {
            push(&mut s, i * 500, false);
        }
        assert!(s.health(ASKED, &[now("Twitch", 0)]).warnings.is_empty(), "ten good seconds later it clears");
        assert_eq!(s.health(ASKED, &[now("Twitch", 0)]).outputs[0].dropped, 18, "the total stays");
    }

    #[test]
    fn network_drops_warn_from_one_percent_and_are_bad_from_five() {
        let w = |drop_every| steady(20, 6000, 5, drop_every).health(ASKED, &[now("Twitch", 200)]).warnings;
        assert!(w(200).is_empty(), "0.5 % is fine");
        assert_eq!(w(50), vec![warn(Problem::Upload, Level::Warning, Some("Twitch"))]);
        assert_eq!(w(10)[0].level, Level::Bad);
        assert_eq!(w(10)[0].fill(w(10)[0].advice()), "Your upload to Twitch can't keep up: lower the bitrate or the resolution.");
        assert_eq!(w(10)[0].fill(w(10)[0].short()), "Upload to Twitch too slow");
    }

    #[test]
    fn a_queue_falling_behind_warns_before_anything_is_dropped() {
        let s = steady(20, 6000, 5, 0);
        assert!(s.health(ASKED, &[now("Twitch", 999)]).warnings.is_empty());
        assert_eq!(s.health(ASKED, &[now("Twitch", 1000)]).level(), Some(Level::Warning));
        assert_eq!(s.health(ASKED, &[now("Twitch", 2000)]).level(), Some(Level::Bad), "frames go from here");
        // Not while reconnecting (the queue is stale then, and the state says so).
        let mut n = now("Twitch", 1500);
        n.live = false;
        assert!(s.health(ASKED, &[n]).warnings.is_empty());
    }

    #[test]
    fn a_computer_that_falls_behind_is_told_apart_from_the_upload() {
        // 30 ms of a 33 ms budget: busy 90 %.
        let h = steady(20, 6000, 30, 0).health(ASKED, &[now("Twitch", 100)]);
        assert_eq!(h.warnings, vec![warn(Problem::Computer, Level::Warning, None)]);
        assert_eq!(h.warnings[0].fill(h.warnings[0].advice()), "Your computer can't encode fast enough: lower the resolution or the frame rate.");
        // Pictures lost by the encoder or missed slots: 6 of 300 over ten seconds is 2 %.
        let mut s = Sampler::default();
        s.push(tot(10_000, 0, 0, 0, vec![out(0, 0, 0, 0)]));
        s.push(tot(20_000, 294, 6, 294_000, vec![out(7_500_000, 294, 0, 0)]));
        let h = s.health(ASKED, &[now("Twitch", 0)]);
        assert!((h.encoder_lost_share - 0.02).abs() < 0.0001, "{}", h.encoder_lost_share);
        assert_eq!((h.encoder_lost, h.warnings.len(), h.warnings[0].problem), (6, 1, Problem::Computer));
        assert!(h.outputs[0].dropped_share == 0.0, "the network dropped nothing");
    }

    #[test]
    fn two_reconnects_in_ten_minutes_warn() {
        let mut s = Sampler::default();
        let at = |s: &mut Sampler, sec: u64, r: u32| s.push(tot(sec * 1000, sec * 30, 0, 0, vec![out(sec * 750_000, sec * 30, 0, r)]));
        for i in 0..=40 {
            at(&mut s, i, if i >= 30 { 1 } else { 0 });
        }
        let h = s.health(ASKED, &[now("Twitch", 0)]);
        assert_eq!((h.outputs[0].reconnects, h.outputs[0].reconnects_recent), (1, 1));
        assert!(h.warnings.is_empty(), "one is bad luck");
        for i in 41..=300 {
            at(&mut s, i, if i >= 290 { 2 } else { 1 });
        }
        let h = s.health(ASKED, &[now("Twitch", 0)]);
        assert_eq!((h.outputs[0].reconnects, h.outputs[0].reconnects_recent), (2, 2));
        assert_eq!(h.warnings, vec![warn(Problem::Connection, Level::Warning, Some("Twitch"))]);
        assert_eq!(h.warnings[0].fill(h.warnings[0].short()), "Twitch keeps reconnecting");
        // Ten minutes after the first, only one is recent.
        for i in 301..=631 {
            at(&mut s, i, 2);
        }
        let h = s.health(ASKED, &[now("Twitch", 0)]);
        assert_eq!(h.outputs[0].reconnects_recent, 1);
        assert!(h.warnings.is_empty());
    }

    #[test]
    fn several_services_are_judged_each_on_its_own_and_named() {
        let mut s = Sampler::default();
        for i in 0..=20u64 {
            let f = i * 30;
            // Twitch keeps up; YouTube drops one frame in ten.
            s.push(tot(i * 1000, f, 0, f * 5000, vec![out(i * 750_000, f, 0, 0), out(i * 700_000, f - f / 10, f / 10, 0)]));
        }
        let h = s.health(ASKED, &[now("Twitch", 50), now("YouTube", 1800)]);
        assert_eq!(h.outputs.len(), 2);
        assert_eq!((h.outputs[0].sent_kbps, h.outputs[1].sent_kbps), (6000, 5600));
        assert_eq!(h.warnings, vec![warn(Problem::Upload, Level::Bad, Some("YouTube"))]);
        // The worst comes first, then upload before the computer.
        let mut s2 = Sampler::default();
        s2.push(tot(10_000, 0, 0, 0, vec![out(0, 0, 0, 0), out(0, 0, 0, 0)]));
        s2.push(tot(20_000, 300, 0, 9_500_000, vec![out(7_500_000, 300, 0, 0), out(7_000_000, 297, 3, 0)]));
        let h = s2.health(ASKED, &[now("Twitch", 0), now("YouTube", 0)]);
        assert_eq!(h.warnings.iter().map(|w| (w.problem, w.level)).collect::<Vec<_>>(), vec![(Problem::Upload, Level::Warning), (Problem::Computer, Level::Warning)]);
        let mut s3 = Sampler::default();
        s3.push(tot(10_000, 0, 0, 0, vec![out(0, 0, 0, 0)]));
        s3.push(tot(20_000, 300, 30, 0, vec![out(7_500_000, 297, 3, 0)]));
        assert_eq!(s3.health(ASKED, &[now("Twitch", 0)]).warnings[0].problem, Problem::Computer, "bad comes before a warning");
    }

    #[test]
    fn counters_that_go_backwards_never_wrap() {
        let mut s = Sampler::default();
        s.push(tot(0, 100, 10, 1000, vec![out(1000, 100, 10, 2)]));
        s.push(tot(15_000, 50, 5, 500, vec![out(500, 50, 5, 1)]));
        let h = s.health(ASKED, &[now("Twitch", 0)]);
        assert_eq!((h.outputs[0].sent_kbps, h.outputs[0].reconnects_recent, h.encoder_lost_share), (0, 0, 0.0));
        assert!(h.warnings.is_empty());
        // Totals missing for a service read as zero rather than panicking.
        let h = s.health(ASKED, &[now("Twitch", 0), now("Kick", 0)]);
        assert_eq!(h.outputs[1].sent_kbps, 0);
    }

    #[test]
    fn a_warning_shows_after_three_seconds_a_bad_one_at_once_and_both_stay_ten() {
        let w = [warn(Problem::Upload, Level::Warning, Some("Twitch"))];
        let mut st = Steady::default();
        assert_eq!(st.update(0, &w), None);
        assert_eq!(st.update(2000, &w), None, "not held long enough");
        assert_eq!(st.update(3000, &w), Some(&w[0]));
        // A blip that clears doesn't make it flicker: it stays ten seconds.
        assert_eq!(st.update(4000, &[]), Some(&w[0]));
        assert_eq!(st.update(12_999, &[]), Some(&w[0]));
        assert_eq!(st.update(13_000, &[]), None);
        // A warning that comes and goes never shows.
        let mut st = Steady::default();
        for t in (0..20_000).step_by(2000) {
            let now: &[Warning] = if (t / 2000) % 2 == 0 { &w } else { &[] };
            assert_eq!(st.update(t, now), None, "at {t}");
        }
        // Bad shows at once.
        let bad = [warn(Problem::Computer, Level::Bad, None)];
        let mut st = Steady::default();
        assert_eq!(st.update(0, &bad), Some(&bad[0]));
    }

    #[test]
    fn a_worse_warning_replaces_the_shown_one_at_once_a_milder_one_waits() {
        let up = warn(Problem::Upload, Level::Warning, Some("Twitch"));
        let bad = warn(Problem::Computer, Level::Bad, None);
        let mut st = Steady::default();
        st.update(0, std::slice::from_ref(&up));
        assert_eq!(st.update(3000, std::slice::from_ref(&up)), Some(&up));
        assert_eq!(st.update(3500, &[bad.clone(), up.clone()]), Some(&bad));
        // The bad one clears; the warning still holds but waits until the bad one has gone.
        assert_eq!(st.update(4000, std::slice::from_ref(&up)), Some(&bad));
        assert_eq!(st.update(13_499, std::slice::from_ref(&up)), Some(&bad));
        assert_eq!(st.update(13_500, std::slice::from_ref(&up)), Some(&up));
        // The same problem getting worse shows at once…
        let up_bad = Warning { level: Level::Bad, ..up.clone() };
        assert_eq!(st.update(14_000, std::slice::from_ref(&up_bad)), Some(&up_bad));
        // …milder only once it has stayed milder for ten seconds (no red, yellow, red).
        assert_eq!(st.update(15_000, std::slice::from_ref(&up)), Some(&up_bad));
        assert_eq!(st.update(16_000, std::slice::from_ref(&up_bad)), Some(&up_bad));
        for t in (17_000..26_000).step_by(1000) {
            assert_eq!(st.update(t, std::slice::from_ref(&up)), Some(&up_bad), "at {t}");
        }
        assert_eq!(st.update(26_000, std::slice::from_ref(&up)), Some(&up));
        st.reset();
        assert_eq!(st.update(15_000, &[]), None);
    }
}
