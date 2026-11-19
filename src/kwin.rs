//! KDE Plasma on Wayland: the recording bar's place and the editor's return, through KWin.
//!
//! A Wayland program can't place its windows or keep one above the others, and KWin opens the
//! recording bar in the middle of the screen, over what is being recorded. KWin offers its
//! scripting interface on the session bus (`org.kde.KWin`, `/Scripting`), and a small script
//! can do what the program can't: keep the bar above other windows, move it to a corner, and
//! un-minimize the editor after a recording that was stopped without a click on the bar (a
//! hotkey, the desktop's own "stop sharing"). Where the bus or KWin isn't there (another
//! desktop, a Flatpak without access to KWin), every call fails quietly and Snagster carries on
//! without it.

use std::path::PathBuf;

/// The space between the bar and the screen's edges (the panel's side excluded by KWin).
pub const MARGIN: i32 = 24;

/// Whether the desktop is KDE Plasma (where KWin's scripting is worth asking).
pub fn on_kde() -> bool {
    is_kde(&std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default())
}

fn is_kde(current_desktop: &str) -> bool {
    current_desktop.split(':').any(|d| d.eq_ignore_ascii_case("kde"))
}

/// A string as a JavaScript literal (JSON's rules are JavaScript's).
fn js_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// The script that keeps the bar (Snagster's window titled `title`) above the others and puts
/// it in the bottom-right corner of the screen it opened on, once: the person may drag it
/// after. It also watches for the bar while it opens (KWin may see the window before its title).
pub fn bar_script(pid: u32, title: &str, margin: i32) -> String {
    format!(
        r#"(function () {{
    const pid = {pid}, title = {title}, margin = {margin};
    const placed = new Set();
    function place(w) {{
        if (!w || w.pid !== pid || w.caption !== title) return;
        const id = String(w.internalId);
        if (placed.has(id)) return;
        placed.add(id);
        w.keepAbove = true;
        const a = workspace.clientArea(KWin.PlacementArea, w);
        const g = w.frameGeometry;
        w.frameGeometry = {{ x: a.x + a.width - g.width - margin, y: a.y + a.height - g.height - margin, width: g.width, height: g.height }};
    }}
    function watch(w) {{
        if (!w || w.pid !== pid) return;
        place(w);
        w.captionChanged.connect(function () {{ place(w); }});
    }}
    workspace.windowList().forEach(watch);
    workspace.windowAdded.connect(watch);
}})();
"#,
        title = js_string(title)
    )
}

/// The script that brings back Snagster's minimized windows other than the bar (the editor)
/// and gives the editor the focus.
pub fn bring_back_script(pid: u32, bar_title: &str) -> String {
    format!(
        r#"(function () {{
    const pid = {pid}, bar = {bar};
    for (const w of workspace.windowList()) {{
        if (w.pid === pid && w.caption !== bar && w.minimized && w.normalWindow) {{
            w.minimized = false;
            workspace.activeWindow = w;
        }}
    }}
}})();
"#,
        bar = js_string(bar_title)
    )
}

/// The name a script of this Snagster is loaded under (one per kind, per running Snagster).
pub fn script_name(kind: &str) -> String {
    format!("snagster-{kind}-{}", std::process::id())
}

/// Load `js` into KWin under `name` (replacing one loaded before under that name) and run it.
/// It stays loaded (its handlers keep working) until `unload`.
pub fn run(name: &str, js: &str) -> Result<(), String> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let path = dir.join(format!("{name}.js"));
    std::fs::write(&path, js).map_err(|e| e.to_string())?;
    let file = path.to_string_lossy().to_string();
    let result = pollster::block_on(async {
        use ashpd::zbus::Connection;
        let conn = Connection::session().await.map_err(|e| e.to_string())?;
        let scripting = |method: &'static str| (Some("org.kde.KWin"), "/Scripting", Some("org.kde.kwin.Scripting"), method);
        let (dest, path, iface, method) = scripting("unloadScript");
        let _ = conn.call_method(dest, path, iface, method, &(name,)).await;
        let (dest, path, iface, method) = scripting("loadScript");
        let reply = conn.call_method(dest, path, iface, method, &(file.as_str(), name)).await.map_err(|e| e.to_string())?;
        let id: i32 = reply.body().deserialize().map_err(|e| e.to_string())?;
        if id < 0 {
            return Err("KWin didn't load the script".to_string());
        }
        let script = format!("/Scripting/Script{id}");
        conn.call_method(Some("org.kde.KWin"), script.as_str(), Some("org.kde.kwin.Script"), "run", &())
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    });
    // KWin has read it by now (loading reads the file).
    let _ = std::fs::remove_file(&path);
    result
}

/// Take a script loaded with `run` out of KWin (its handlers stop).
pub fn unload(name: &str) {
    let _ = pollster::block_on(async {
        let conn = ashpd::zbus::Connection::session().await?;
        conn.call_method(Some("org.kde.KWin"), "/Scripting", Some("org.kde.kwin.Scripting"), "unloadScript", &(name,)).await
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plasma_counts_as_kde() {
        assert!(is_kde("KDE"));
        assert!(is_kde("kde"));
        assert!(is_kde("ubuntu:KDE"));
        assert!(!is_kde("ubuntu:GNOME"));
        assert!(!is_kde(""));
        assert!(!is_kde("KDEish"));
    }

    #[test]
    fn the_scripts_name_this_snagster_and_its_bar() {
        let s = bar_script(4242, "Recording", MARGIN);
        assert!(s.contains("const pid = 4242, title = \"Recording\", margin = 24;"), "{s}");
        assert!(s.contains("w.keepAbove = true"));
        assert!(s.contains("KWin.PlacementArea"), "the corner of the free area, panels left out");
        assert!(s.contains("workspace.windowAdded.connect"), "a bar that opens after the script runs");
        let b = bring_back_script(4242, "Recording");
        assert!(b.contains("w.minimized = false") && b.contains("workspace.activeWindow = w"));
        assert!(b.contains("w.caption !== bar"), "the bar itself stays as it is");
        assert!(script_name("bar").starts_with("snagster-bar-"));
    }

    #[test]
    fn a_title_can_never_break_out_of_the_script() {
        // Translated titles may hold quotes or backslashes.
        let s = bar_script(1, "Rec\"ord'\\ing\n});evil();", MARGIN);
        assert!(s.contains(r#"title = "Rec\"ord'\\ing\n});evil();""#), "{s}");
        assert!(!s.contains("\n});evil();"));
    }
}
