//! Live ink over the desktop on macOS and Linux on X11: a see-through, always-on-top window of
//! the app on every monitor that shows the ink and, while Draw is on, takes the mouse as the pen
//! (clicks pass through while it is off). Windows has native windows of its own
//! (`ink_overlay`), and Wayland draws on the Studio preview only.
//!
//! The code is built everywhere (so the Windows build checks it too) and runs where
//! `uses_app_windows` says.

use eframe::egui::{self, Color32, Pos2, Sense, Shape, Stroke, pos2, vec2};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Whether ink over the screen is drawn by the app's own windows here.
pub(crate) fn uses_app_windows() -> bool {
    !cfg!(windows) && crate::ink_overlay::support() == crate::ink_overlay::Where::Screen
}

/// The monitors, listed again every few seconds (listing them every frame is slow).
static MONITORS: Mutex<Option<(Instant, Vec<crate::capture::MonitorInfo>)>> = Mutex::new(None);

fn monitors() -> Vec<crate::capture::MonitorInfo> {
    let mut g = MONITORS.lock().unwrap_or_else(|e| e.into_inner());
    if g.as_ref().is_none_or(|(t, _)| t.elapsed() > Duration::from_secs(3)) {
        *g = Some((Instant::now(), crate::capture::monitors()));
    }
    g.as_ref().map(|(_, m)| m.clone()).unwrap_or_default()
}

/// Desktop pixels per point in the overlay windows: the Mac lists its screens in points,
/// X11 in pixels.
fn desk_per_point(ppp: f32) -> f32 {
    if cfg!(target_os = "macos") { 1.0 } else { ppp.max(0.25) }
}

/// Show the overlays this frame (only while there is ink or Draw is on; windows not shown in
/// a frame close).
pub(crate) fn overlays(ctx: &egui::Context) {
    if !uses_app_windows() {
        return;
    }
    let now = Instant::now();
    let (on, show) = crate::ink::with(|i| (i.on, i.on || i.any(now)));
    if !show {
        return;
    }
    let k = desk_per_point(ctx.pixels_per_point());
    for m in monitors() {
        let origin = (m.x, m.y);
        let builder = crate::windows::snagster_viewport()
            .with_title("Snagster ink")
            .with_position(pos2(m.x as f32 / k, m.y as f32 / k))
            .with_inner_size(vec2(m.width as f32 / k, m.height as f32 / k))
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_taskbar(false)
            .with_resizable(false)
            .with_active(false)
            .with_mouse_passthrough(!on);
        ctx.show_viewport_deferred(egui::ViewportId::from_hash_of(("ink", m.id)), builder, move |ui, _| overlay(ui, origin));
    }
}

/// One monitor's overlay.
fn overlay(ui: &mut egui::Ui, origin: (i32, i32)) {
    let ctx = ui.ctx().clone();
    let k = desk_per_point(ctx.pixels_per_point());
    let now = Instant::now();
    let to_desk = |p: Pos2| [origin.0 as f32 + p.x * k, origin.1 as f32 + p.y * k];
    let to_local = |p: [f32; 2]| pos2((p[0] - origin.0 as f32) / k, (p[1] - origin.1 as f32) / k);
    let rect = ui.max_rect();
    let (on, snap) = crate::ink::with(|i| (i.on, i.snapshot(now)));
    let resp = ui.allocate_rect(rect, if on { Sense::drag() } else { Sense::hover() });
    if on {
        if resp.drag_started()
            && let Some(p) = resp.interact_pointer_pos()
        {
            crate::ink::with(|i| i.begin(crate::ink::Space::Desktop, to_desk(p), now));
        }
        if resp.dragged()
            && let Some(p) = resp.interact_pointer_pos()
        {
            crate::ink::with(|i| i.extend(to_desk(p)));
        }
        if resp.drag_stopped() {
            crate::ink::with(|i| i.end(now));
        }
        let (esc, undo) = ui.input(|i| (i.key_pressed(egui::Key::Escape), i.modifiers.command && i.key_pressed(egui::Key::Z)));
        if esc {
            crate::ink::with(|i| i.set_on(false, now));
        }
        if undo {
            crate::ink::with(|i| i.undo(now));
        }
        ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    let painter = ui.painter();
    for s in snap.in_space(crate::ink::Space::Desktop) {
        let a = (s.color[3] as f32 * snap.alpha) as u8;
        let c = Color32::from_rgba_unmultiplied(s.color[0], s.color[1], s.color[2], a);
        let w = s.width / k;
        let pts: Vec<Pos2> = s.points.iter().map(|p| to_local(*p)).collect();
        if pts.len() == 1 {
            painter.circle_filled(pts[0], w / 2.0, c);
        } else {
            for p in [pts[0], pts[pts.len() - 1]] {
                painter.circle_filled(p, w / 2.0, c);
            }
            painter.add(Shape::line(pts, Stroke::new(w, c)));
        }
    }
    let fading = crate::ink::with(|i| i.fading(now));
    ctx.request_repaint_after(Duration::from_millis(if on || fading { 16 } else { 100 }));
}

#[cfg(test)]
mod tests {
    #[test]
    fn overlay_windows_measure_the_desktop_as_the_screen_list_does() {
        let k = super::desk_per_point(2.0);
        if cfg!(target_os = "macos") {
            assert_eq!(k, 1.0, "the Mac lists screens in points");
        } else {
            assert_eq!(k, 2.0);
        }
        assert!(super::desk_per_point(0.0) > 0.0);
        if cfg!(windows) {
            assert!(!super::uses_app_windows(), "Windows has native overlay windows");
        }
    }
}
