//! The bounded queue between the encoders and the network. When the network is slower than the
//! stream, video is dropped (never the capture held up): the first dropped frame starts a gap
//! that lasts until the next keyframe, since frames after a missing one can't be decoded. Sound
//! is kept as long as possible, because gaps in sound are what people notice most.

use crate::encode::{AudioPacket, VideoPacket};
use std::collections::VecDeque;

/// One encoded piece on its way to the server (and to the recording hook).
#[derive(Clone, Debug, PartialEq)]
pub enum Packet {
    Video(VideoPacket),
    Audio(AudioPacket),
}

impl Packet {
    /// Send order time (decode time for video), microseconds.
    pub fn time_us(&self) -> i64 {
        match self {
            Packet::Video(v) => v.dts_us,
            Packet::Audio(a) => a.pts_us,
        }
    }

    fn len(&self) -> usize {
        match self {
            Packet::Video(v) => v.data.len(),
            Packet::Audio(a) => a.data.len(),
        }
    }
}

pub struct SendQueue {
    items: VecDeque<Packet>,
    bytes: usize,
    /// Video waits for the next keyframe.
    drop_until_key: bool,
    /// Above this much queued time, frames are dropped.
    pub max_ms: u32,
    /// Never more than this many bytes queued (sound is dropped from the front past it).
    pub max_bytes: usize,
    pub dropped_video: u64,
    pub dropped_audio: u64,
    /// The stream has ended (no more packets will come).
    pub ended: bool,
}

impl SendQueue {
    pub fn new(max_ms: u32) -> SendQueue {
        SendQueue { items: VecDeque::new(), bytes: 0, drop_until_key: true, max_ms: max_ms.max(50), max_bytes: 64 << 20, dropped_video: 0, dropped_audio: 0, ended: false }
    }

    /// Milliseconds between the oldest and newest packet waiting.
    pub fn duration_ms(&self) -> u32 {
        match (self.items.front(), self.items.back()) {
            (Some(a), Some(b)) => ((b.time_us() - a.time_us()).max(0) / 1000).min(u32::MAX as i64) as u32,
            _ => 0,
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    fn push(&mut self, p: Packet) {
        self.bytes += p.len();
        self.items.push_back(p);
        while self.bytes > self.max_bytes {
            match self.items.pop_front() {
                Some(old) => {
                    self.bytes -= old.len();
                    match old {
                        Packet::Video(_) => self.dropped_video += 1,
                        Packet::Audio(_) => self.dropped_audio += 1,
                    }
                }
                None => break,
            }
        }
    }

    /// Queue a video frame. False when it was dropped.
    pub fn push_video(&mut self, v: VideoPacket) -> bool {
        let congested = self.duration_ms() > self.max_ms;
        if !v.keyframe {
            if self.drop_until_key || congested {
                self.drop_until_key = true;
                self.dropped_video += 1;
                return false;
            }
        } else if self.duration_ms() > self.max_ms.saturating_mul(2) {
            // Far behind: throw away the video still waiting and start again from this keyframe.
            self.purge_video();
        }
        self.drop_until_key = false;
        self.push(Packet::Video(v));
        true
    }

    /// Queue sound; only dropped (oldest first) when the queue is far past its limit.
    pub fn push_audio(&mut self, a: AudioPacket) {
        self.push(Packet::Audio(a));
        while self.duration_ms() > self.max_ms.saturating_mul(4) {
            if let Some(old) = self.items.pop_front() {
                self.bytes -= old.len();
                match old {
                    Packet::Video(_) => self.dropped_video += 1,
                    Packet::Audio(_) => self.dropped_audio += 1,
                }
            }
        }
    }

    fn purge_video(&mut self) {
        let before = self.items.len();
        self.items.retain(|p| matches!(p, Packet::Audio(_)));
        self.dropped_video += (before - self.items.len()) as u64;
        self.bytes = self.items.iter().map(Packet::len).sum();
    }

    pub fn pop(&mut self) -> Option<Packet> {
        let p = self.items.pop_front()?;
        self.bytes -= p.len();
        Some(p)
    }

    /// A new connection: what was waiting is stale (its video frames count as dropped); video
    /// restarts at a keyframe.
    pub fn restart(&mut self) {
        self.dropped_video += self.items.iter().filter(|p| matches!(p, Packet::Video(_))).count() as u64;
        self.items.clear();
        self.bytes = 0;
        self.drop_until_key = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(ms: i64, key: bool) -> VideoPacket {
        VideoPacket { data: vec![0; 10], pts_us: ms * 1000, dts_us: ms * 1000, keyframe: key }
    }
    fn a(ms: i64) -> AudioPacket {
        AudioPacket { data: vec![1; 4], pts_us: ms * 1000 }
    }

    #[test]
    fn video_starts_at_a_keyframe() {
        let mut q = SendQueue::new(500);
        assert!(!q.push_video(v(0, false)));
        assert!(q.push_video(v(33, true)));
        assert!(q.push_video(v(66, false)));
        assert_eq!(q.len(), 2);
        assert_eq!(q.dropped_video, 1);
    }

    #[test]
    fn a_slow_network_drops_inter_frames_until_the_next_keyframe_and_keeps_sound() {
        let mut q = SendQueue::new(200);
        assert!(q.push_video(v(0, true)));
        // Nothing is sent: the queue grows.
        let mut t = 33;
        while t <= 300 {
            q.push_audio(a(t));
            q.push_video(v(t, false));
            t += 33;
        }
        assert!(q.dropped_video > 0);
        let dropped = q.dropped_video;
        // The network catches up a little; inter frames still wait for a keyframe.
        while q.duration_ms() > 50 {
            q.pop();
        }
        assert!(!q.push_video(v(330, false)));
        assert_eq!(q.dropped_video, dropped + 1);
        assert!(q.push_video(v(363, true)));
        assert!(q.push_video(v(396, false)));
        // Every sound packet that was queued is still there or was sent; none dropped.
        assert_eq!(q.dropped_audio, 0);
    }

    #[test]
    fn far_behind_a_keyframe_replaces_the_waiting_video() {
        let mut q = SendQueue::new(100);
        q.push_video(v(0, true));
        for t in 1..=10 {
            q.push_audio(a(t * 30));
        }
        assert!(q.push_video(v(301, true)));
        let videos = q.items.iter().filter(|p| matches!(p, Packet::Video(_))).count();
        assert_eq!(videos, 1);
        assert_eq!(q.dropped_video, 1);
    }

    #[test]
    fn the_queue_never_grows_without_bound() {
        let mut q = SendQueue::new(100);
        for t in 0..10_000 {
            q.push_audio(a(t * 21));
        }
        assert!(q.duration_ms() <= 400);
        assert!(q.dropped_audio > 0);
        let mut q = SendQueue::new(100_000);
        q.max_bytes = 100;
        for t in 0..100 {
            q.push_audio(a(t));
        }
        assert!(q.bytes <= 100);
    }

    #[test]
    fn restart_clears_and_waits_for_a_keyframe() {
        let mut q = SendQueue::new(500);
        q.push_video(v(0, true));
        q.push_audio(a(0));
        q.restart();
        assert_eq!(q.len(), 0);
        assert_eq!(q.dropped_video, 1, "the frame that was waiting is lost");
        assert!(!q.push_video(v(33, false)));
        assert!(q.pop().is_none());
    }
}
