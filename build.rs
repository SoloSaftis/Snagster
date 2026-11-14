//! Puts Snagster's icon and version details into Snagster.exe on Windows, so Explorer, the Start
//! menu, the taskbar and Alt+Tab show the icon even before the window sets its own. Linux and
//! macOS get their icon from the .desktop file and the app bundle (see the CI packaging steps).

fn main() {
    println!("cargo:rerun-if-changed=assets/snagster.ico");
    println!("cargo:rerun-if-changed=build.rs");
    // The target, not the computer building it: cross-builds for Windows still get the icon.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/snagster.ico")
            .set("ProductName", "Snagster")
            .set("FileDescription", "Snagster - capture your screen, mark it up, share it")
            .set("LegalCopyright", "MIT or Apache-2.0");
        if let Err(e) = res.compile() {
            // A missing resource compiler shouldn't stop the build; the program works without
            // the embedded icon (the window still sets its own).
            println!("cargo:warning=Snagster.exe is built without its icon: {e}");
        }
    }
}
