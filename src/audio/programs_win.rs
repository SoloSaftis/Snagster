//! Windows: one program's sound through process loopback (`ActivateAudioInterfaceAsync` on the
//! virtual process-loopback device with `AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS`, Windows 10
//! version 2004 and later), written from Microsoft's documentation. A program is its exe name;
//! each running copy (a process of that exe whose parent isn't the same exe) is captured with its
//! whole process tree, so a browser's or game's helper processes are heard too.
//!
//! Which programs have sound: the audio sessions of every active output device
//! (`IAudioSessionManager2`), active ones playing; the name people read is the exe's
//! FileDescription.

use super::programs::{Gather, LOOK_EVERY, Program, Unsupported, key, label_from_key, same};
use super::{CHANNELS, Feed, RATE};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0};
use windows::Win32::System::Com::{BLOB, CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, IAgileObject, IAgileObject_Impl};
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
use windows::Win32::System::Threading::{CreateEventW, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW, WaitForSingleObject};
use windows::Win32::System::Variant::VT_BLOB;
use windows::core::{HRESULT, IUnknown, Interface, PWSTR, Ref, implement};

/// The first Windows build with process loopback (Windows 10 version 2004).
const FIRST_BUILD: u32 = 19041;

pub fn support() -> Result<(), Unsupported> {
    static BUILD: OnceLock<u32> = OnceLock::new();
    if build_ok(*BUILD.get_or_init(build)) { Ok(()) } else { Err(Unsupported::OldWindows) }
}

fn build_ok(build: u32) -> bool {
    build >= FIRST_BUILD
}

/// This Windows' build number (0 when it can't be read).
fn build() -> u32 {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    let mut buf = [0u16; 32];
    let mut len = (buf.len() * 2) as u32;
    let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, windows::core::w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"), windows::core::w!("CurrentBuildNumber"), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut len)) };
    if r.is_err() {
        return 0;
    }
    String::from_utf16_lossy(&buf[..(len as usize / 2).min(buf.len())]).trim_end_matches('\0').trim().parse().unwrap_or(0)
}

/// A process: (id, parent id, exe name).
type Proc = (u32, u32, String);

/// Every running process.
fn processes() -> Vec<Proc> {
    let mut out = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let len = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len());
            out.push((e.th32ProcessID, e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..len])));
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// The copies of program `name` to capture: its processes whose parent isn't the same program
/// (each is captured with its process tree, so the children would be heard twice).
fn roots(procs: &[Proc], name: &str) -> Vec<u32> {
    procs
        .iter()
        .filter(|(pid, parent, exe)| *pid != 0 && same(name, exe) && !procs.iter().any(|(p, _, e)| p == parent && p != pid && same(name, e)))
        .map(|(pid, ..)| *pid)
        .collect()
}

/// The exe name of process `pid` (None: not in the snapshot).
fn exe_of(procs: &[Proc], pid: u32) -> Option<&str> {
    procs.iter().find(|(p, ..)| *p == pid).map(|(_, _, e)| e.as_str())
}

pub fn list() -> Vec<Program> {
    let procs = processes();
    let me = std::process::id();
    let mut out = Vec::new();
    for (pid, playing) in sessions() {
        if pid == me || pid == 0 {
            continue;
        }
        let Some(exe) = exe_of(&procs, pid) else { continue };
        out.push(Program { name: key(exe), label: describe(pid).unwrap_or_else(|| label_from_key(exe)), playing });
    }
    out
}

/// (process id, playing) of every program's audio session on every active output device;
/// Windows' own "System sounds" left out.
fn sessions() -> Vec<(u32, bool)> {
    let mut out = Vec::new();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return out };
        let Ok(devices) = en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else { return out };
        for d in 0..devices.GetCount().unwrap_or(0) {
            let Ok(dev) = devices.Item(d) else { continue };
            let Ok(mgr) = dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { continue };
            let Ok(list) = mgr.GetSessionEnumerator() else { continue };
            for i in 0..list.GetCount().unwrap_or(0) {
                let Ok(ctl) = list.GetSession(i) else { continue };
                let Ok(ctl2) = ctl.cast::<IAudioSessionControl2>() else { continue };
                if ctl2.IsSystemSoundsSession() == HRESULT(0) {
                    continue;
                }
                let Ok(pid) = ctl2.GetProcessId() else { continue };
                let playing = ctl.GetState().is_ok_and(|s| s == AudioSessionStateActive);
                out.push((pid, playing));
            }
        }
    }
    out
}

/// The FileDescription (else ProductName) of process `pid`'s exe.
fn describe(pid: u32) -> Option<String> {
    use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut path = vec![0u16; 1024];
        let mut len = path.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(path.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return None;
        }
        path.truncate(len as usize);
        path.push(0);
        let file = windows::core::PCWSTR(path.as_ptr());
        let size = GetFileVersionInfoSizeW(file, None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(file, None, size, data.as_mut_ptr().cast()).ok()?;
        let mut p = std::ptr::null_mut();
        let mut n = 0u32;
        // The first language and code page the file lists.
        let langs = if VerQueryValueW(data.as_ptr().cast(), windows::core::w!("\\VarFileInfo\\Translation"), &mut p, &mut n).as_bool() && n >= 4 {
            let w = std::slice::from_raw_parts(p as *const u16, 2);
            vec![format!("{:04x}{:04x}", w[0], w[1]), "040904b0".into(), "040904e4".into()]
        } else {
            vec!["040904b0".into(), "040904e4".into()]
        };
        for field in ["FileDescription", "ProductName"] {
            for lang in &langs {
                let q: Vec<u16> = format!("\\StringFileInfo\\{lang}\\{field}").encode_utf16().chain(std::iter::once(0)).collect();
                if VerQueryValueW(data.as_ptr().cast(), windows::core::PCWSTR(q.as_ptr()), &mut p, &mut n).as_bool() && n > 1 {
                    let s = String::from_utf16_lossy(std::slice::from_raw_parts(p as *const u16, n as usize - 1)).trim_end_matches('\0').trim().to_string();
                    if !s.is_empty() {
                        return Some(s);
                    }
                }
            }
        }
        None
    }
}

/// The completion handler `ActivateAudioInterfaceAsync` calls (it must be agile).
#[implement(IActivateAudioInterfaceCompletionHandler, IAgileObject)]
struct Activated {
    done: Mutex<Option<Sender<()>>>,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for Activated_Impl {
    fn ActivateCompleted(&self, _op: Ref<IActivateAudioInterfaceAsyncOperation>) -> windows::core::Result<()> {
        if let Some(tx) = self.done.lock().ok().and_then(|mut d| d.take()) {
            let _ = tx.send(());
        }
        Ok(())
    }
}

impl IAgileObject_Impl for Activated_Impl {}

/// A tap on one running copy of the program: its sound client and the process, to see when
/// it ends.
struct Tap {
    pid: u32,
    client: IAudioClient,
    cap: IAudioCaptureClient,
    process: HANDLE,
    event: HANDLE,
}

impl Drop for Tap {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
            let _ = CloseHandle(self.process);
            let _ = CloseHandle(self.event);
        }
    }
}

/// Start capturing process `pid` and its children.
fn open(pid: u32) -> Result<Tap, String> {
    unsafe {
        let params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 { ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS { TargetProcessId: pid, ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE } },
        };
        // Never dropped: PROPVARIANT's Drop would free the blob, which points at `params` here.
        let pv = std::mem::ManuallyDrop::new(PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_BLOB,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: PROPVARIANT_0_0_0 { blob: BLOB { cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32, pBlobData: &params as *const _ as *mut u8 } },
                }),
            },
        });
        let (tx, rx) = channel();
        let handler: IActivateAudioInterfaceCompletionHandler = Activated { done: Mutex::new(Some(tx)) }.into();
        let op = ActivateAudioInterfaceAsync(VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, &IAudioClient::IID, Some(&*pv), &handler).map_err(|e| e.message())?;
        rx.recv_timeout(Duration::from_secs(5)).map_err(|_| "Windows didn't answer".to_string())?;
        let mut hr = HRESULT(0);
        let mut unk: Option<IUnknown> = None;
        op.GetActivateResult(&mut hr, &mut unk).map_err(|e| e.message())?;
        hr.ok().map_err(|e| e.message())?;
        let client: IAudioClient = unk.ok_or("no sound client")?.cast().map_err(|e| e.message())?;
        let fmt = WAVEFORMATEX { wFormatTag: 1, nChannels: CHANNELS as u16, nSamplesPerSec: RATE, nAvgBytesPerSec: RATE * CHANNELS * 2, nBlockAlign: (CHANNELS * 2) as u16, wBitsPerSample: 16, cbSize: 0 };
        let flags = AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, &fmt, None).map_err(|e| e.message())?;
        let event = CreateEventW(None, false, false, None).map_err(|e| e.message())?;
        let _ = client.SetEventHandle(event);
        let cap: IAudioCaptureClient = client.GetService().map_err(|e| e.message())?;
        let process = OpenProcess(PROCESS_SYNCHRONIZE, false, pid).unwrap_or_default();
        client.Start().map_err(|e| e.message())?;
        Ok(Tap { pid, client, cap, process, event })
    }
}

/// Whether tap `c`'s process has ended (seen through its handle, when Windows gave one).
fn ended(c: &Tap) -> bool {
    !c.process.is_invalid() && unsafe { WaitForSingleObject(c.process, 0) } == WAIT_OBJECT_0
}

/// What tap `c` has captured since last time.
fn read(c: &Tap, buf: &mut Vec<i16>) {
    buf.clear();
    unsafe {
        while c.cap.GetNextPacketSize().unwrap_or(0) > 0 {
            let mut data = std::ptr::null_mut();
            let (mut frames, mut flags) = (0u32, 0u32);
            if c.cap.GetBuffer(&mut data, &mut frames, &mut flags, None, None).is_err() {
                break;
            }
            let n = (frames * CHANNELS) as usize;
            if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                buf.resize(buf.len() + n, 0);
            } else {
                buf.extend_from_slice(std::slice::from_raw_parts(data as *const i16, n));
            }
            let _ = c.cap.ReleaseBuffer(frames);
        }
    }
}

/// How long a copy Windows wouldn't let Snagster hear waits before it is tried again.
const RETRY: Duration = Duration::from_secs(10);

/// A copy Windows refused `since` ago is tried again.
fn retry_due(since: Duration) -> bool {
    since >= RETRY
}

/// The lane waits for the program ("Not running") only when no copy of it runs at all: one
/// running but refused keeps the row on, silent.
fn waiting(tapped: usize, refused: usize) -> bool {
    tapped == 0 && refused == 0
}

pub fn run(name: &str, feed: &std::sync::Arc<Feed>, stop: &std::sync::Arc<AtomicBool>) -> Result<(), String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let mut gather = Gather::new(Instant::now());
    let mut copies: Vec<Tap> = Vec::new();
    // Copies that couldn't be tapped and when: tried again every RETRY, not every look.
    let mut refused: Vec<(u32, Instant)> = Vec::new();
    let mut looked: Option<Instant> = None;
    let mut buf = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        if looked.is_none_or(|t| t.elapsed() >= LOOK_EVERY) {
            looked = Some(Instant::now());
            let procs = processes();
            copies.retain(|c| {
                // Without a handle, a process missing from the snapshot has ended.
                let gone = ended(c) || !procs.iter().any(|(p, ..)| *p == c.pid);
                if gone {
                    gather.end(c.pid as u64);
                }
                !gone
            });
            let now = roots(&procs, name);
            refused.retain(|(p, at)| now.contains(p) && !retry_due(at.elapsed()));
            for pid in now {
                if copies.iter().any(|c| c.pid == pid) || refused.iter().any(|(p, _)| *p == pid) {
                    continue;
                }
                match open(pid) {
                    Ok(c) => copies.push(c),
                    Err(_) => refused.push((pid, Instant::now())),
                }
            }
            feed.waiting.store(waiting(copies.len(), refused.len()), Ordering::Relaxed);
        }
        for c in &copies {
            read(c, &mut buf);
            if !buf.is_empty() {
                gather.put(c.pid as u64, &buf);
            }
        }
        gather.tick(Instant::now(), feed);
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_copy_is_tried_again_and_keeps_the_row_on() {
        assert!(!retry_due(Duration::from_secs(9)));
        assert!(retry_due(RETRY));
        assert!(waiting(0, 0), "nothing running: waits");
        assert!(!waiting(0, 1), "running but refused: not \"Not running\"");
        assert!(!waiting(2, 0));
    }

    fn p(pid: u32, parent: u32, exe: &str) -> Proc {
        (pid, parent, exe.to_string())
    }

    /// A browser's helpers are part of its tree: only the top process of each copy is captured.
    #[test]
    fn each_copy_is_captured_once_from_its_top_process() {
        let procs = vec![p(4, 0, "System"), p(100, 50, "explorer.exe"), p(200, 100, "chrome.exe"), p(201, 200, "chrome.exe"), p(202, 200, "Chrome.exe"), p(300, 100, "Spotify.exe"), p(301, 300, "Spotify.exe"), p(400, 999, "chrome.exe"), p(500, 300, "crashpad.exe")];
        assert_eq!(roots(&procs, "chrome"), [200, 400], "two copies of Chrome");
        assert_eq!(roots(&procs, "spotify"), [300]);
        assert!(roots(&procs, "zoom").is_empty(), "not running");
        assert!(roots(&procs, "").is_empty());
        // A process that is its own parent (it happens with reused ids) still counts.
        assert_eq!(roots(&[p(7, 7, "game.exe")], "game"), [7]);
        assert_eq!(exe_of(&procs, 301), Some("Spotify.exe"));
        assert_eq!(exe_of(&procs, 12345), None);
    }

    /// On a real computer with speakers (not in CI): a PowerShell plays one of Windows' own
    /// sounds, and a lane for "powershell" hears it while one for a program that isn't running
    /// waits. `cargo test -- --ignored one_programs_sound_is_heard`.
    #[test]
    #[ignore = "needs speakers and plays a sound"]
    fn one_programs_sound_is_heard() {
        use crate::audio::{Capture, SoundInput};
        let ps = SoundInput::Program { name: "powershell".into() };
        let none = SoundInput::Program { name: "no-such-program-here".into() };
        let cap = Capture::start_inputs(&[ps.clone(), none.clone()]);
        assert!(cap.notes().is_empty(), "{:?}", cap.notes());
        let mut child = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", "$p = New-Object Media.SoundPlayer 'C:\\Windows\\Media\\chimes.wav'; $p.PlaySync(); $p.PlaySync(); $p.PlaySync()"])
            .spawn()
            .unwrap();
        let start = Instant::now();
        let mut loudest = 0f32;
        while start.elapsed() < Duration::from_secs(10) && loudest < 0.02 {
            std::thread::sleep(Duration::from_millis(50));
            cap.take(2_400);
            loudest = loudest.max(cap.level_of(&ps).unwrap_or(0.0));
        }
        let _ = child.kill();
        let _ = child.wait();
        assert!(loudest >= 0.02, "PowerShell's sound wasn't heard (loudest {loudest})");
        assert!(!cap.waiting(&ps));
        assert!(cap.waiting(&none), "a program that isn't running waits");
        assert_eq!(cap.level_of(&none), Some(0.0));
    }

    #[test]
    fn process_loopback_needs_windows_10_2004() {
        assert!(!build_ok(18363), "1909");
        assert!(build_ok(19041) && build_ok(22631) && build_ok(26100));
        assert!(!build_ok(0), "unreadable: not offered");
    }

    /// This computer's build is read (every CI and development machine runs a recent Windows).
    #[test]
    fn this_windows_reports_its_build() {
        assert!(build() >= 17763, "{}", build());
    }
}
