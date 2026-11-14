//! Keeping slow work off the window's thread, so Windows never calls Snagster "Not responding":
//! a background job queue, a cached list that refreshes itself on a worker, a wait-until-idle
//! timer, a per-frame budget for drawing many small pictures, and the slow-frame watchdog.
//! Nothing here needs a screen, so it is all unit tested.

use eframe::egui;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type Task<T> = Box<dyn FnOnce() -> T + Send>;

/// Work done in the background whose results come back to the window's thread.
///
/// `queue` runs jobs one after another, in the order given, on one worker thread (so a
/// capture is added to the library before it is copied and saved, as before). `spawn` runs a
/// job on its own thread at once, for things that may block for a long time (reading the
/// clipboard from a program that doesn't answer). A job that panics is dropped; the queue
/// carries on.
pub(crate) struct Jobs<T: Send + 'static> {
    queue: Option<Sender<Task<T>>>,
    done_tx: Sender<T>,
    done_rx: Receiver<T>,
    busy: Arc<AtomicUsize>,
    wake: Arc<Mutex<Option<egui::Context>>>,
}

impl<T: Send + 'static> Jobs<T> {
    pub fn new() -> Self {
        let (done_tx, done_rx) = channel();
        Self { queue: None, done_tx, done_rx, busy: Arc::new(AtomicUsize::new(0)), wake: Arc::new(Mutex::new(None)) }
    }

    /// Repaint this window when a job finishes, so its result is shown straight away.
    pub fn wake(&mut self, ctx: &egui::Context) {
        if let Ok(mut w) = self.wake.lock()
            && w.is_none()
        {
            *w = Some(ctx.clone());
        }
    }

    fn runner(&self) -> impl Fn(Task<T>) + Send + 'static {
        let tx = self.done_tx.clone();
        let busy = self.busy.clone();
        let wake = self.wake.clone();
        move |task: Task<T>| {
            let r = catch_unwind(AssertUnwindSafe(task));
            if let Ok(v) = r {
                let _ = tx.send(v);
            }
            busy.fetch_sub(1, Ordering::SeqCst);
            let c = wake.lock().ok().and_then(|w| w.clone());
            if let Some(c) = c {
                c.request_repaint();
            }
        }
    }

    /// Run `f` after the jobs queued before it.
    pub fn queue(&mut self, f: impl FnOnce() -> T + Send + 'static) {
        self.busy.fetch_add(1, Ordering::SeqCst);
        let mut task: Task<T> = Box::new(f);
        if let Some(q) = &self.queue {
            match q.send(task) {
                Ok(()) => return,
                // The worker is gone (it can't be, but never lose a save): start a new one.
                Err(e) => task = e.0,
            }
        }
        let (tx, rx) = channel::<Task<T>>();
        let run = self.runner();
        let started = std::thread::Builder::new().name("snagster-jobs".into()).spawn(move || {
            for t in rx {
                run(t);
            }
        });
        if started.is_err() {
            // No thread could be started: do it here rather than not at all.
            self.runner()(task);
            return;
        }
        let _ = tx.send(task);
        self.queue = Some(tx);
    }

    /// Run `f` on a thread of its own, now.
    pub fn spawn(&mut self, f: impl FnOnce() -> T + Send + 'static) {
        self.busy.fetch_add(1, Ordering::SeqCst);
        let run = self.runner();
        let task: Task<T> = Box::new(f);
        let _ = std::thread::Builder::new().name("snagster-job".into()).spawn(move || run(task));
    }

    /// The results that have arrived, at most `max` of them.
    pub fn finished(&self, max: usize) -> Vec<T> {
        let mut out = Vec::new();
        while out.len() < max {
            match self.done_rx.try_recv() {
                Ok(v) => out.push(v),
                Err(_) => break,
            }
        }
        out
    }

    /// How many jobs haven't finished yet.
    pub fn busy(&self) -> usize {
        self.busy.load(Ordering::SeqCst)
    }

    /// Wait (at most `limit`) until every job has finished.
    pub fn wait_idle(&self, limit: Duration) -> bool {
        let until = Instant::now() + limit;
        while self.busy() > 0 {
            if Instant::now() >= until {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        true
    }
}

impl<T: Send + 'static> Drop for Jobs<T> {
    /// Snagster is closing: let saves that are under way finish first.
    fn drop(&mut self) {
        self.queue = None;
        self.wait_idle(Duration::from_secs(15));
    }
}

/// Whether a cached list should be fetched again.
pub(crate) fn refresh_due(fetched: Option<Instant>, now: Instant, every: Duration, in_flight: bool) -> bool {
    !in_flight && fetched.is_none_or(|t| now.saturating_duration_since(t) >= every)
}

/// A list (windows, screens) that is slow to make: it is made on a worker thread, at most
/// once every `every`, and the last one is shown meanwhile.
pub(crate) struct ListCache<T: Send + 'static> {
    items: Vec<T>,
    fetched: Option<Instant>,
    rx: Option<Receiver<Vec<T>>>,
    every: Duration,
    loaded: bool,
}

impl<T: Send + 'static> ListCache<T> {
    pub fn new(every: Duration) -> Self {
        Self { items: Vec::new(), fetched: None, rx: None, every, loaded: false }
    }

    /// Take a finished list, if one arrived.
    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        match rx.try_recv() {
            Ok(v) => {
                self.items = v;
                self.loaded = true;
                self.rx = None;
            }
            Err(TryRecvError::Disconnected) => self.rx = None,
            Err(TryRecvError::Empty) => {}
        }
    }

    /// The list as it is now, starting a new one in the background when it is due.
    pub fn get(&mut self, ctx: Option<&egui::Context>, fetch: fn() -> Vec<T>) -> &[T] {
        self.poll();
        if refresh_due(self.fetched, Instant::now(), self.every, self.rx.is_some()) {
            self.fetched = Some(Instant::now());
            let (tx, rx) = channel();
            self.rx = Some(rx);
            let wake = ctx.cloned();
            let _ = std::thread::Builder::new().name("snagster-list".into()).spawn(move || {
                let _ = tx.send(fetch());
                if let Some(c) = wake {
                    c.request_repaint();
                }
            });
        }
        if self.rx.is_some()
            && let Some(c) = ctx
        {
            c.request_repaint_after(Duration::from_millis(50));
        }
        &self.items
    }

    /// The list as it is now, without starting a new one.
    pub fn items(&self) -> &[T] {
        &self.items
    }

    /// Whether a list has arrived at least once.
    pub fn loaded(&self) -> bool {
        self.loaded
    }

    /// Make the next `get` fetch a new list (unless one is being made).
    pub fn expire(&mut self) {
        self.fetched = None;
    }
}

/// Apply a change once it has stopped changing: the HTTP server for AI apps isn't restarted
/// on every key typed into its port or password.
pub(crate) struct Debounce<V> {
    pending: Option<(V, Instant)>,
}

impl<V: PartialEq + Clone> Debounce<V> {
    pub fn new() -> Self {
        Self { pending: None }
    }

    /// `applied` is in effect, `want` is asked for now. Gives the value to apply now: at once
    /// when `at_once`, otherwise after `want` stayed the same for `wait`.
    pub fn check(&mut self, applied: &V, want: &V, now: Instant, wait: Duration, at_once: bool) -> Option<V> {
        if want == applied {
            self.pending = None;
            return None;
        }
        if at_once {
            self.pending = None;
            return Some(want.clone());
        }
        match &self.pending {
            Some((v, since)) if v == want => {
                if now.saturating_duration_since(*since) >= wait {
                    self.pending = None;
                    Some(want.clone())
                } else {
                    None
                }
            }
            _ => {
                self.pending = Some((want.clone(), now));
                None
            }
        }
    }

    /// Whether a change is waiting to be applied.
    pub fn waiting(&self) -> bool {
        self.pending.is_some()
    }
}

/// How much slow drawing one frame may do: always `min` items, then more until `until`.
pub(crate) struct Budget {
    until: Instant,
    min: usize,
    used: usize,
    refused: bool,
}

impl Budget {
    pub fn new(time: Duration, min: usize) -> Self {
        Self { until: Instant::now() + time, min, used: 0, refused: false }
    }

    /// Whether one more slow item may be made in this frame.
    pub fn allow(&mut self) -> bool {
        if self.used < self.min || Instant::now() < self.until {
            self.used += 1;
            true
        } else {
            self.refused = true;
            false
        }
    }

    /// Whether something was left for a later frame.
    pub fn left_some(&self) -> bool {
        self.refused
    }
}

/// A frame slower than this is written to crash.log.
pub(crate) const SLOW_FRAME: Duration = Duration::from_secs(1);

/// The crash.log line for a slow frame, or nothing when it wasn't slow.
pub(crate) fn slow_frame_line(part: &str, took: Duration, open: &[&str]) -> Option<String> {
    if took < SLOW_FRAME {
        return None;
    }
    let open = if open.is_empty() { "nothing else".to_string() } else { open.join(", ") };
    Some(format!(
        "{} Snagster {} ({} {}) slow frame: {} took {:.1} s; open: {}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        part,
        took.as_secs_f32(),
        open
    ))
}

/// Whether a picture's texture hasn't been drawn for so long that it can be let go (it is
/// made again when it is shown).
pub(crate) fn texture_stale(last_drawn: Instant, now: Instant, keep: Duration) -> bool {
    now.saturating_duration_since(last_drawn) >= keep
}

/// `path`, or `path` with " (2)", " (3)"... before the extension when that one is already
/// taken: on disk, or by a save that is still under way.
pub(crate) fn free_path(path: std::path::PathBuf, taken: &std::collections::HashSet<std::path::PathBuf>) -> std::path::PathBuf {
    if !taken.contains(&path) && !path.exists() {
        return path;
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let mut k = 2;
    loop {
        let p = path.with_file_name(format!("{stem} ({k}){ext}"));
        if !taken.contains(&p) && !p.exists() {
            return p;
        }
        k += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect<T: Send + 'static>(jobs: &Jobs<T>, n: usize) -> Vec<T> {
        let until = Instant::now() + Duration::from_secs(10);
        let mut out = Vec::new();
        while out.len() < n && Instant::now() < until {
            out.extend(jobs.finished(usize::MAX));
            std::thread::sleep(Duration::from_millis(2));
        }
        out
    }

    #[test]
    fn queued_jobs_run_in_order_off_this_thread() {
        let mut jobs = Jobs::new();
        let here = std::thread::current().id();
        for i in 0..20 {
            jobs.queue(move || {
                assert_ne!(std::thread::current().id(), here);
                if i == 0 {
                    std::thread::sleep(Duration::from_millis(30));
                }
                i
            });
        }
        assert_eq!(collect(&jobs, 20), (0..20).collect::<Vec<_>>());
        assert!(jobs.wait_idle(Duration::from_secs(5)));
        assert_eq!(jobs.busy(), 0);
    }

    #[test]
    fn queueing_doesnt_wait_for_the_job() {
        // The job can't finish before this test lets it (no sleeps: a slow machine can't
        // turn the order around), so if queue() waited for it, it would never return.
        let (go, gate) = channel::<()>();
        let mut jobs = Jobs::new();
        jobs.queue(move || {
            let _ = gate.recv_timeout(Duration::from_secs(30));
            1
        });
        assert_eq!(jobs.busy(), 1);
        assert!(jobs.finished(10).is_empty());
        go.send(()).unwrap();
        assert_eq!(collect(&jobs, 1), vec![1]);
    }

    #[test]
    fn a_job_that_panics_doesnt_stop_the_queue() {
        let mut jobs = Jobs::new();
        jobs.queue(|| panic!("a broken job (expected in this test)"));
        jobs.queue(|| 2);
        assert_eq!(collect(&jobs, 1), vec![2]);
        assert!(jobs.wait_idle(Duration::from_secs(5)));
    }

    #[test]
    fn spawned_jobs_dont_wait_behind_the_queue() {
        // The queued job waits until the spawned one has finished (or 30 s, so a broken
        // spawn fails the test instead of hanging it).
        let (go, gate) = channel::<()>();
        let mut jobs = Jobs::new();
        jobs.queue(move || {
            let _ = gate.recv_timeout(Duration::from_secs(30));
            "slow"
        });
        jobs.spawn(|| "quick");
        let mut got = collect(&jobs, 1);
        go.send(()).unwrap();
        got.extend(collect(&jobs, 1));
        assert_eq!(got, vec!["quick", "slow"]);
    }

    #[test]
    fn finished_gives_at_most_max() {
        let mut jobs = Jobs::new();
        for i in 0..5 {
            jobs.queue(move || i);
        }
        assert!(jobs.wait_idle(Duration::from_secs(5)));
        assert_eq!(jobs.finished(2).len(), 2);
        assert_eq!(jobs.finished(10).len(), 3);
    }

    #[test]
    fn dropping_waits_for_jobs_under_way() {
        let done = Arc::new(Mutex::new(false));
        {
            let mut jobs = Jobs::new();
            let d = done.clone();
            jobs.queue(move || {
                std::thread::sleep(Duration::from_millis(100));
                *d.lock().unwrap() = true;
            });
        }
        assert!(*done.lock().unwrap(), "closing lost a job");
    }

    #[test]
    fn lists_refresh_only_when_due_and_not_twice_at_once() {
        let t0 = Instant::now();
        let every = Duration::from_secs(3);
        assert!(refresh_due(None, t0, every, false));
        assert!(!refresh_due(None, t0, every, true), "one fetch at a time");
        assert!(!refresh_due(Some(t0), t0 + Duration::from_secs(1), every, false));
        assert!(refresh_due(Some(t0), t0 + every, every, false));
        assert!(!refresh_due(Some(t0), t0 + every * 2, every, true));
        // A clock that went backwards doesn't refresh every frame.
        assert!(!refresh_due(Some(t0 + every), t0, every, false));
    }

    static FETCHES: AtomicUsize = AtomicUsize::new(0);
    /// `slow_list` doesn't finish until the test opens this (no timing to race against).
    static GATE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    fn slow_list() -> Vec<u32> {
        FETCHES.fetch_add(1, Ordering::SeqCst);
        let until = Instant::now() + Duration::from_secs(30);
        while !GATE.load(Ordering::SeqCst) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(2));
        }
        vec![1, 2, 3]
    }

    #[test]
    fn list_cache_fetches_in_the_background_once() {
        let mut c = ListCache::new(Duration::from_secs(60));
        // The list can't be made before the gate opens, so getting here proves get() didn't wait.
        assert!(c.get(None, slow_list).is_empty(), "the first call doesn't wait for the list");
        assert!(c.get(None, slow_list).is_empty());
        assert!(!c.loaded());
        GATE.store(true, Ordering::SeqCst);
        let until = Instant::now() + Duration::from_secs(10);
        while !c.loaded() && Instant::now() < until {
            c.get(None, slow_list);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(c.items(), &[1, 2, 3]);
        assert_eq!(FETCHES.load(Ordering::SeqCst), 1, "asked every frame, fetched once");
        c.expire();
        c.get(None, slow_list);
        let until = Instant::now() + Duration::from_secs(10);
        while FETCHES.load(Ordering::SeqCst) < 2 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(FETCHES.load(Ordering::SeqCst), 2, "expire fetches again");
        assert_eq!(c.items(), &[1, 2, 3], "the old list is kept meanwhile");
    }

    #[test]
    fn debounce_waits_until_typing_stops() {
        let mut d = Debounce::new();
        let t0 = Instant::now();
        let wait = Duration::from_secs(1);
        let applied = Some(45831u16);
        // Typing 4, 45, 458: each key restarts the wait.
        assert_eq!(d.check(&applied, &Some(4), t0, wait, false), None);
        assert!(d.waiting());
        assert_eq!(d.check(&applied, &Some(45), t0 + Duration::from_millis(300), wait, false), None);
        assert_eq!(d.check(&applied, &Some(458), t0 + Duration::from_millis(600), wait, false), None);
        assert_eq!(d.check(&applied, &Some(458), t0 + Duration::from_millis(1500), wait, false), None);
        assert_eq!(d.check(&applied, &Some(458), t0 + Duration::from_millis(1600), wait, false), Some(Some(458)));
        assert!(!d.waiting());
    }

    #[test]
    fn debounce_applies_at_once_when_asked_and_forgets_undone_changes() {
        let mut d = Debounce::new();
        let t0 = Instant::now();
        let wait = Duration::from_secs(1);
        assert_eq!(d.check(&1, &2, t0, wait, true), Some(2));
        assert_eq!(d.check(&1, &2, t0, wait, false), None);
        // Changed back before the wait ended: nothing to do.
        assert_eq!(d.check(&1, &1, t0, wait, false), None);
        assert!(!d.waiting());
        assert_eq!(d.check(&1, &1, t0 + wait * 5, wait, true), None);
    }

    #[test]
    fn budget_always_allows_the_minimum() {
        let mut b = Budget::new(Duration::ZERO, 3);
        assert!(b.allow() && b.allow() && b.allow());
        assert!(!b.left_some());
        assert!(!b.allow());
        assert!(b.left_some());
        let mut roomy = Budget::new(Duration::from_secs(60), 0);
        assert!((0..100).all(|_| roomy.allow()));
        assert!(!roomy.left_some());
    }

    #[test]
    fn only_slow_frames_are_logged() {
        assert!(slow_frame_line("ui", Duration::from_millis(999), &["Library"]).is_none());
        let line = slow_frame_line("ui", Duration::from_millis(2500), &["Library", "GIF recordings"]).unwrap();
        assert!(line.contains("slow frame: ui took 2.5 s"), "{line}");
        assert!(line.contains("open: Library, GIF recordings"), "{line}");
        assert!(line.ends_with('\n') && line.matches('\n').count() == 1, "one line");
        assert!(slow_frame_line("logic", SLOW_FRAME, &[]).unwrap().contains("open: nothing else"));
    }

    #[test]
    fn textures_go_after_a_while_unseen() {
        let t0 = Instant::now();
        let keep = Duration::from_secs(60);
        assert!(!texture_stale(t0, t0 + Duration::from_secs(59), keep));
        assert!(texture_stale(t0, t0 + keep, keep));
        assert!(!texture_stale(t0 + keep, t0, keep));
    }

    #[test]
    fn free_path_skips_files_and_saves_under_way() {
        let dir = std::env::temp_dir().join(format!("snagster-work-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("shot.png");
        let mut taken = std::collections::HashSet::new();
        assert_eq!(free_path(p.clone(), &taken), p);
        taken.insert(p.clone());
        assert_eq!(free_path(p.clone(), &taken), dir.join("shot (2).png"));
        std::fs::write(dir.join("shot (2).png"), b"x").unwrap();
        assert_eq!(free_path(p.clone(), &taken), dir.join("shot (3).png"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
