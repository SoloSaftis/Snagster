//! Test builds only: the fuzz harness. Everything Snagster reads that it didn't write itself
//! (pictures, `.snag` projects, settings.json, MCP messages and every MCP tool's arguments,
//! translations, the single-instance messages, the updater's SHA256SUMS and GitHub's answer,
//! video files, what a streaming server sends) is a target here: a function that reads one
//! input, with good inputs to start from. The harness breaks those inputs over and over,
//! from a seed, and fails on a panic, a single allocation over the target's budget, or an
//! input that takes too long. Whatever failed is saved, so it can be replayed.
//!
//! `cargo test` runs a short pass of every target (the `short_*` tests below). The nightly CI
//! job (`.github/workflows/nightly.yml`) runs each target for minutes:
//!
//! ```text
//! SNAGSTER_FUZZ_TARGET=picture SNAGSTER_FUZZ_SECS=300 cargo test fuzz_nightly -- --ignored --nocapture
//! SNAGSTER_FUZZ_TARGET=picture SNAGSTER_FUZZ_INPUT=target/fuzz/crash-picture-....bin cargo test fuzz_replay -- --ignored --nocapture
//! cargo test fuzz_list -- --ignored --nocapture
//! ```
//!
//! No coverage feedback (no libFuzzer, no nightly Rust): a seeded random walk from good
//! inputs, with a mutator that knows JSON, which is most of what Snagster reads. Our own code
//! (MIT/Apache-2.0), nothing new in the dependency tree.

use crate::testalloc::{Bytes, measure};
use serde_json::Value;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// One place Snagster reads outside input.
pub struct Target {
    pub name: &'static str,
    /// Good inputs to start from (at least one).
    pub seeds: fn() -> Vec<Vec<u8>>,
    /// Words worth putting in: field names, command names, values from a list.
    pub words: fn() -> Vec<String>,
    /// Reads one input. A panic (a bug, or an assert about the result) is a failure.
    pub run: fn(&[u8]),
    /// The largest single allocation reading an input of this many bytes may make.
    pub budget: fn(usize) -> usize,
}

/// Every target, from the modules that read the input.
pub fn targets() -> Vec<Target> {
    let mut all = Vec::new();
    all.extend(crate::export::fuzzing::targets());
    all.extend(crate::library::fuzzing::targets());
    all.extend(crate::settings::fuzzing::targets());
    all.extend(crate::mcp::fuzzing::targets());
    all.extend(crate::mcp_http::fuzzing::targets());
    all.extend(crate::i18n::fuzzing::targets());
    all.extend(crate::instance::fuzzing::targets());
    all.extend(crate::update::fuzzing::targets());
    all.extend(crate::video::fuzzing::targets());
    all.extend(crate::stream::fuzzing::targets());
    all
}

pub fn target(name: &str) -> Target {
    targets().into_iter().find(|t| t.name == name).unwrap_or_else(|| panic!("no fuzz target {name} (cargo test fuzz_list -- --ignored --nocapture)"))
}

/// A folder of the target's own for files it reads (removed when the run ends).
pub fn scratch(target: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("snagster-fuzz-{}", std::process::id())).join(target);
    let _ = std::fs::create_dir_all(&p);
    p
}

/// How long a run lasts.
#[derive(Clone, Copy, Debug)]
pub enum Stop {
    Count(u64),
    Time(Duration),
}

/// One input slower than this is a failure (a parser that loops, or work that grows too fast).
const SLOW: Duration = Duration::from_secs(30);
/// No answer for this long: the watchdog saves the input and ends the process.
const HANG: Duration = Duration::from_secs(120);

/// Where failing inputs go: `SNAGSTER_FUZZ_OUT`, else `target/fuzz`.
pub fn out_dir() -> PathBuf {
    std::env::var_os("SNAGSTER_FUZZ_OUT").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("fuzz"))
}

/// How a run went.
#[derive(Debug)]
pub struct Report {
    pub inputs: u64,
    pub seconds: f64,
}

/// Feed `t` broken inputs from `seed` until `stop`. With `keep_current`, each input is written
/// to `<out>/<target>.current` before it is read, so one that aborts the whole process (out of
/// memory) or hangs is still on disk. The error names the saved file and how to replay it.
pub fn run(t: &Target, seed: u64, stop: Stop, out: &Path, keep_current: bool) -> Result<Report, String> {
    let words = (t.words)();
    let seeds = (t.seeds)();
    assert!(!seeds.is_empty(), "fuzz target {} has no seeds", t.name);
    let mut g = Bytes::new(seed);
    let mut pool = seeds.clone();
    let t0 = Instant::now();
    let _ = std::fs::create_dir_all(out);
    let current = out.join(format!("{}.current", t.name));
    let watch = keep_current.then(|| Watchdog::start(t.name, current.clone()));
    let mut n: u64 = 0;
    let result = loop {
        let done = match stop {
            Stop::Count(c) => n >= c,
            Stop::Time(d) => n >= seeds.len() as u64 && t0.elapsed() >= d,
        };
        if done {
            break Ok(Report { inputs: n, seconds: t0.elapsed().as_secs_f64() });
        }
        let input = if (n as usize) < seeds.len() {
            seeds[n as usize].clone()
        } else {
            // Half the time from a good input, else from one made earlier (breaks pile up).
            let base = if g.below(2) == 0 { &seeds[g.below(seeds.len())] } else { &pool[g.below(pool.len())] };
            mutate(&mut g, base, &pool, &words)
        };
        if keep_current {
            let _ = std::fs::write(&current, &input);
        }
        if let Some(w) = &watch {
            w.started();
        }
        let budget = (t.budget)(input.len());
        let t1 = Instant::now();
        let r = catch_unwind(AssertUnwindSafe(|| measure(|| (t.run)(&input))));
        let took = t1.elapsed();
        let problem = match r {
            Err(p) => Some(format!("panicked: {}", panic_text(&p))),
            Ok(((), largest, _)) if largest > budget => Some(format!("asked for {largest} bytes at once (the budget for {} bytes of input is {budget})", input.len())),
            Ok(_) if took > SLOW => Some(format!("took {:.1} s", took.as_secs_f64())),
            Ok(_) => None,
        };
        if let Some(problem) = problem {
            let file = out.join(format!("crash-{}-{seed}-{n}.bin", t.name));
            let _ = std::fs::write(&file, &input);
            break Err(format!(
                "fuzz target {} (seed {seed}, input {n}, {} bytes) {problem}.\nSaved as {}. Replay: SNAGSTER_FUZZ_TARGET={} SNAGSTER_FUZZ_INPUT={} cargo test fuzz_replay -- --ignored --nocapture",
                t.name,
                input.len(),
                file.display(),
                t.name,
                file.display()
            ));
        }
        if pool.len() < 1000 {
            pool.push(input);
        } else {
            let i = g.below(pool.len());
            pool[i] = input;
        }
        n += 1;
    };
    drop(watch);
    if keep_current && result.is_ok() {
        let _ = std::fs::remove_file(&current);
    }
    let _ = std::fs::remove_dir_all(scratch(t.name));
    result
}

fn panic_text(p: &Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_else(|| "(no message)".into())
}

/// Ends the process when one input runs for too long (an endless loop can't be caught).
struct Watchdog {
    /// When the input now being read started, in ms since the watchdog began.
    since: Arc<AtomicU64>,
    t0: Instant,
    quit: Arc<AtomicBool>,
}

impl Watchdog {
    fn start(name: &'static str, current: PathBuf) -> Watchdog {
        let since = Arc::new(AtomicU64::new(0));
        let quit = Arc::new(AtomicBool::new(false));
        let t0 = Instant::now();
        let (s, q) = (since.clone(), quit.clone());
        std::thread::spawn(move || {
            while !q.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(500));
                let now = t0.elapsed().as_millis() as u64;
                if now.saturating_sub(s.load(Ordering::Relaxed)) > HANG.as_millis() as u64 {
                    let hang = current.with_file_name(format!("hang-{name}.bin"));
                    let _ = std::fs::copy(&current, &hang);
                    eprintln!("fuzz target {name}: one input has run for over {} s; saved as {}", HANG.as_secs(), hang.display());
                    std::process::exit(3);
                }
            }
        });
        Watchdog { since, t0, quit }
    }

    fn started(&self) {
        self.since.store(self.t0.elapsed().as_millis() as u64, Ordering::Relaxed);
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
    }
}

// ---- breaking inputs ----

/// Numbers that find edges: zero, signs, the limits of each integer size, huge and tiny floats.
const NUMBERS: [f64; 26] = [
    0.0,
    -0.0,
    1.0,
    -1.0,
    2.0,
    0.5,
    -0.5,
    7.0,
    255.0,
    256.0,
    1000.0,
    16384.0,
    20000.0,
    65535.0,
    65536.0,
    100_000.0,
    2147483647.0,
    2147483648.0,
    -2147483649.0,
    4294967295.0,
    4294967296.0,
    9007199254740993.0,
    1.8446744073709552e19,
    1e308,
    -1e308,
    1e-308,
];

/// Strings that find edges: empty, long, right-to-left marks, nulls, paths, placeholders.
fn odd_string(g: &mut Bytes, words: &[String]) -> String {
    match g.below(16) {
        0 => String::new(),
        1 => "a".repeat(1 + g.below(5000)),
        2 => "ü😀\u{202E}ﷺ\u{0301}x".into(),
        3 => "\u{0}".into(),
        4 => "../../..".into(),
        5 => "{name}{".into(),
        6 => "#".into(),
        7 => "#zz00GG".into(),
        8 => format!("{}", NUMBERS[g.below(NUMBERS.len())]),
        9 => "\r\n\t ".into(),
        10 => "-".into(),
        11 => String::from_utf8_lossy(&g.noise(24)).into_owned(),
        _ if !words.is_empty() => words[g.below(words.len())].clone(),
        _ => "x".into(),
    }
}

fn odd_value(g: &mut Bytes, words: &[String], depth: usize) -> Value {
    match g.below(12) {
        0 | 1 | 2 => serde_json::Number::from_f64(NUMBERS[g.below(NUMBERS.len())]).map(Value::Number).unwrap_or(Value::Null),
        3 => Value::from(g.next() as i64 >> g.below(64)),
        4 | 5 | 6 => Value::String(odd_string(g, words)),
        7 => Value::Bool(g.below(2) == 0),
        8 => Value::Null,
        9 if depth < 4 => Value::Array((0..g.below(4)).map(|_| odd_value(g, words, depth + 1)).collect()),
        10 if depth < 4 => {
            let mut m = serde_json::Map::new();
            for _ in 0..g.below(3) {
                m.insert(odd_string(g, words), odd_value(g, words, depth + 1));
            }
            Value::Object(m)
        }
        _ => Value::Array(Vec::new()),
    }
}

/// Every place in a JSON value, as JSON pointers.
fn places(v: &Value, at: String, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, c) in m {
                places(c, format!("{at}/{}", k.replace('~', "~0").replace('/', "~1")), out);
            }
        }
        Value::Array(a) => {
            for (i, c) in a.iter().enumerate() {
                places(c, format!("{at}/{i}"), out);
            }
        }
        _ => {}
    }
    out.push(at);
}

/// One to three changes to the structure of a JSON value: values swapped for odd ones, keys
/// added or removed, list items repeated, numbers pushed past their range.
fn mutate_json(g: &mut Bytes, v: &mut Value, words: &[String]) {
    for _ in 0..1 + g.below(3) {
        let mut all = Vec::new();
        places(v, String::new(), &mut all);
        let at = all[g.below(all.len())].clone();
        let Some(node) = v.pointer_mut(&at) else { continue };
        match (node, g.below(4)) {
            (Value::Object(m), 0) if !m.is_empty() => {
                let k = m.keys().nth(g.below(m.len())).cloned().unwrap_or_default();
                m.remove(&k);
            }
            (Value::Object(m), 1 | 2) => {
                let k = if !words.is_empty() && g.below(3) > 0 { words[g.below(words.len())].clone() } else { odd_string(g, words) };
                let val = odd_value(g, words, 1);
                m.insert(k, val);
            }
            (Value::Array(a), 0) if !a.is_empty() => {
                let i = g.below(a.len());
                a.remove(i);
            }
            (Value::Array(a), 1 | 2) if !a.is_empty() => {
                let i = g.below(a.len());
                let copy = a[i].clone();
                let most = if g.below(8) == 0 { 300 } else { 3 };
                for _ in 0..1 + g.below(most) {
                    a.push(copy.clone());
                }
            }
            (Value::Number(n), 0 | 1) => {
                let x = n.as_f64().unwrap_or(0.0);
                let y = match g.below(5) {
                    0 => x + 1.0,
                    1 => -x,
                    2 => x * 1000.0,
                    3 => x - 1.0,
                    _ => x / 3.0,
                };
                if let Some(y) = serde_json::Number::from_f64(y) {
                    *n = y;
                }
            }
            (Value::String(s), 0) => {
                let extra = odd_string(g, words);
                s.push_str(&extra);
            }
            (node, _) => *node = odd_value(g, words, 0),
        }
    }
}

/// `base` broken in one of several ways (structure-aware when it is JSON).
pub fn mutate(g: &mut Bytes, base: &[u8], pool: &[Vec<u8>], words: &[String]) -> Vec<u8> {
    let looks_json = base.iter().find(|b| !b.is_ascii_whitespace()).is_some_and(|b| *b == b'{' || *b == b'[');
    match g.below(10) {
        0..=4 if looks_json => {
            if let Ok(mut v) = serde_json::from_slice::<Value>(base) {
                mutate_json(g, &mut v, words);
                let mut out = serde_json::to_vec(&v).unwrap_or_default();
                // Now and then broken as bytes as well.
                if g.below(6) == 0 {
                    out = g.mutate(&out);
                }
                return out;
            }
            g.mutate(base)
        }
        5 if !pool.is_empty() => {
            // The start of one input and the end of another.
            let other = &pool[g.below(pool.len())];
            let mut v = base[..g.below(base.len() + 1)].to_vec();
            v.extend_from_slice(&other[g.below(other.len() + 1)..]);
            v
        }
        6 if !words.is_empty() => {
            let mut v = base.to_vec();
            let i = g.below(v.len() + 1);
            let w = words[g.below(words.len())].clone().into_bytes();
            v.splice(i..i, w);
            v
        }
        7 if !base.is_empty() => {
            // A stretch repeated many times: long lines, long lists, deep nesting.
            let i = g.below(base.len());
            let n = (1 + g.below(16)).min(base.len() - i);
            let mut v = base[..i].to_vec();
            for _ in 0..1 + g.below(400) {
                v.extend_from_slice(&base[i..i + n]);
            }
            v.extend_from_slice(&base[i..]);
            v
        }
        8 if g.below(4) == 0 => g.noise(512),
        _ => g.mutate(base),
    }
}

/// Settings for a target that must not touch the real folders: every folder in `dir`.
pub fn sandboxed_settings(dir: &Path) -> crate::settings::Settings {
    let mut s = crate::settings::Settings::default();
    pin_folders(&mut s, dir);
    s
}

/// Every folder setting back inside `dir` (after a call that may have changed one).
pub fn pin_folders(s: &mut crate::settings::Settings, dir: &Path) {
    let d = |n: &str| dir.join(n).to_string_lossy().into_owned();
    s.save_folder = d("pictures");
    s.library_folder = d("library");
    s.video_folder = d("videos");
}

/// A file name made from anything: only letters, digits and a few marks, never a path out of
/// the folder (the fuzz inputs choose names, never places).
pub fn safe_name(s: &str) -> String {
    let mut n: String = s.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ' ')).take(60).collect();
    n = n.trim_start_matches(['.', ' ']).trim_end_matches(['.', ' ']).to_string();
    if n.is_empty() { "x".into() } else { n }
}

fn seed_from_env() -> u64 {
    std::env::var("SNAGSTER_FUZZ_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or_else(|| SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1))
}

fn chosen() -> Vec<Target> {
    match std::env::var("SNAGSTER_FUZZ_TARGET") {
        Ok(list) if !list.trim().is_empty() && list.trim() != "all" => list.split(',').map(|n| target(n.trim())).collect(),
        _ => targets(),
    }
}

/// A short pass of one target, for `cargo test` (the same seed every time).
fn short(name: &str) {
    let t = target(name);
    if let Err(e) = run(&t, 1, Stop::Count(250), &out_dir(), false) {
        panic!("{e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The nightly run: `SNAGSTER_FUZZ_SECS` (default 60) for each target in
    /// `SNAGSTER_FUZZ_TARGET` (a comma list, default all), from `SNAGSTER_FUZZ_SEED` (default
    /// the clock; printed, so a run can be repeated).
    #[test]
    #[ignore]
    fn fuzz_nightly() {
        let secs: u64 = std::env::var("SNAGSTER_FUZZ_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(60);
        let seed = seed_from_env();
        let mut failed = Vec::new();
        for t in chosen() {
            println!("fuzz {}: {secs} s from seed {seed}", t.name);
            match run(&t, seed, Stop::Time(Duration::from_secs(secs)), &out_dir(), true) {
                Ok(r) => println!("fuzz {}: {} inputs in {:.0} s, no problems", t.name, r.inputs, r.seconds),
                Err(e) => {
                    println!("{e}");
                    failed.push(e);
                }
            }
        }
        assert!(failed.is_empty(), "{} fuzz target(s) failed:\n{}", failed.len(), failed.join("\n"));
    }

    /// One saved input again: `SNAGSTER_FUZZ_TARGET` and `SNAGSTER_FUZZ_INPUT` (a file).
    #[test]
    #[ignore]
    fn fuzz_replay() {
        let t = target(&std::env::var("SNAGSTER_FUZZ_TARGET").expect("set SNAGSTER_FUZZ_TARGET"));
        let input = std::fs::read(std::env::var("SNAGSTER_FUZZ_INPUT").expect("set SNAGSTER_FUZZ_INPUT")).expect("the input file");
        // Where any allocation over the budget comes from.
        crate::testalloc::trace_from((t.budget)(input.len()) + 1);
        let t0 = Instant::now();
        let ((), largest, total) = measure(|| (t.run)(&input));
        println!("{}: {} bytes read in {:.2} s; largest allocation {largest} bytes (budget {}), {total} bytes in all", t.name, input.len(), t0.elapsed().as_secs_f64(), (t.budget)(input.len()));
        assert!(largest <= (t.budget)(input.len()), "over the budget");
    }

    /// The names, one per line, for the nightly workflow.
    #[test]
    #[ignore]
    fn fuzz_list() {
        for t in targets() {
            println!("fuzz-target: {}", t.name);
        }
    }

    #[test]
    fn every_target_has_a_unique_name_and_a_short_test() {
        let names: Vec<&str> = targets().iter().map(|t| t.name).collect();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "{names:?}");
        let me = include_str!("fuzz.rs");
        for n in names {
            assert!(me.contains(&format!("short!({n})")), "add short!({n}) to src/fuzz.rs so cargo test runs it");
            assert!(n.chars().all(|c| c.is_ascii_lowercase() || c == '_'), "{n}: the workflow uses it in file names");
        }
    }

    #[test]
    fn the_harness_finds_a_panic_and_a_big_allocation_and_saves_them() {
        let out = scratch("harness-test");
        let panics = Target { name: "test_panics", seeds: || vec![b"{\"a\":1}".to_vec()], words: || vec!["boom".into()], run: |b| assert!(!String::from_utf8_lossy(b).contains("boom"), "found it"), budget: |_| usize::MAX };
        let e = run(&panics, 3, Stop::Count(20_000), &out, false).unwrap_err();
        assert!(e.contains("panicked: found it") && e.contains("Replay:"), "{e}");
        let saved = out.join(e.split("Saved as ").nth(1).unwrap().split(". Replay").next().unwrap());
        assert!(String::from_utf8_lossy(&std::fs::read(saved).unwrap()).contains("boom"));
        let greedy = Target { name: "test_greedy", seeds: || vec![vec![1, 2, 3]], words: Vec::new, run: |b| drop(vec![0u8; b.len() * 1000]), budget: |n| n * 100 + 64 };
        let e = run(&greedy, 3, Stop::Count(10), &out, false).unwrap_err();
        assert!(e.contains("asked for 3000 bytes at once"), "{e}");
        let fine = Target { name: "test_fine", seeds: || vec![b"[1,2]".to_vec()], words: Vec::new, run: |_| {}, budget: |_| usize::MAX };
        assert_eq!(run(&fine, 3, Stop::Count(50), &out, true).unwrap().inputs, 50);
        assert!(!out.join("test_fine.current").exists(), "a clean run leaves no current input");
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn json_mutations_stay_json_and_reach_odd_values() {
        let mut g = Bytes::new(11);
        let words = vec!["width".to_string()];
        let base = br#"{"a":[1,{"b":"x"}],"c":2}"#;
        let (mut json, mut extreme, mut keys) = (0, 0, 0);
        for _ in 0..2000 {
            let m = mutate(&mut g, base, &[], &words);
            if let Ok(v) = serde_json::from_slice::<Value>(&m) {
                json += 1;
                let s = v.to_string();
                extreme += (s.contains("e308") || s.contains("4294967296")) as u32;
                keys += s.contains("\"width\"") as u32;
            }
        }
        assert!(json > 800 && extreme > 5 && keys > 20, "{json} {extreme} {keys}");
    }

    #[test]
    fn names_from_inputs_never_leave_the_folder() {
        for bad in ["../../etc/passwd", "C:\\Windows\\x", "/", "..", "a/../../b", "", " . "] {
            let n = safe_name(bad);
            assert!(!n.contains(['/', '\\', ':']) && n != ".." && !n.starts_with('.') && !n.is_empty(), "{bad} -> {n}");
        }
        assert_eq!(safe_name("shot 1.png"), "shot 1.png");
    }

    macro_rules! short {
        ($name:ident) => {
            #[test]
            fn $name() {
                short(stringify!($name));
            }
        };
    }

    mod short_runs {
        use super::short;
        short!(picture);
        short!(gif);
        short!(project);
        short!(library);
        short!(settings);
        short!(mcp_jsonrpc);
        short!(mcp_tools);
        short!(mcp_http);
        short!(language);
        short!(instance);
        short!(update);
        short!(video_probe);
        short!(stream_amf);
        short!(stream_rtmp);
        short!(stream_flv);
    }
}
