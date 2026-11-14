//! What is inside the windows on screen, for the picker's snapping: buttons, lists, panels and
//! the rest, nested as the program lays them out.
//!
//! Reading a program's controls asks that program, which can be slow, so it happens on a
//! worker while the frozen screen is already up: the window under the mouse first, then the
//! others front to back, each within a time and size budget. The picker uses each window's
//! controls as soon as they arrive and snaps to the window itself until then.
//!
//! - Windows: UI Automation (the control view of the tree, read with a cache request so each
//!   element costs one call). Child windows (`EnumChildWindows`) are what the picker has until
//!   then, and stay if UI Automation can't read a program.
//! - macOS: the Accessibility API, when Snagster is allowed to use it; window bounds otherwise.
//! - Linux: window bounds (from the window list).

use super::Object;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

/// From the coordinates a system reports to desktop pixels. macOS reports points, which are
/// 2 pixels on a Retina screen; Windows (Snagster is DPI-aware there) and Linux report pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Space {
    /// The picture's top-left, in both systems (the scale is taken around it).
    pub origin: (i32, i32),
    pub scale: f32,
}

impl Default for Space {
    fn default() -> Self {
        Space { origin: (0, 0), scale: 1.0 }
    }
}

impl Space {
    /// The scale of a screen whose picture is `image_w` pixels wide and whose bounds are
    /// `monitor_w` units wide.
    pub fn scale_for(image_w: u32, monitor_w: u32) -> f32 {
        if monitor_w == 0 || image_w == 0 { 1.0 } else { image_w as f32 / monitor_w as f32 }
    }

    pub fn to_pixels(&self, x: f64, y: f64, w: f64, h: f64) -> (i32, i32, u32, u32) {
        let s = self.scale as f64;
        let (ox, oy) = (self.origin.0 as f64, self.origin.1 as f64);
        let x0 = (ox + (x - ox) * s).round();
        let y0 = (oy + (y - oy) * s).round();
        let x1 = (ox + (x + w - ox) * s).round();
        let y1 = (oy + (y + h - oy) * s).round();
        (x0 as i32, y0 as i32, (x1 - x0).max(0.0) as u32, (y1 - y0).max(0.0) as u32)
    }

    pub fn from_pixels(&self, x: i32, y: i32, w: u32, h: u32) -> (f64, f64, f64, f64) {
        let s = if self.scale > 0.0 { self.scale as f64 } else { 1.0 };
        let (ox, oy) = (self.origin.0 as f64, self.origin.1 as f64);
        (ox + (x as f64 - ox) / s, oy + (y as f64 - oy) / s, w as f64 / s, h as f64 / s)
    }

    /// Move a tree of objects reported in system units into pixels.
    pub fn scale_objects(&self, objs: &mut [Object]) {
        if self.scale == 1.0 {
            return;
        }
        for o in objs {
            let (x, y, w, h) = self.to_pixels(o.x as f64, o.y as f64, o.width as f64, o.height as f64);
            (o.x, o.y, o.width, o.height) = (x, y, w, h);
            self.scale_objects(&mut o.children);
        }
    }
}

/// An element as a system reports it, in its own units.
#[derive(Clone, Debug, PartialEq)]
pub struct Info {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub label: String,
    /// Off screen (scrolled out of view) or hidden.
    pub hidden: bool,
}

/// A tree of elements a system can be asked about, one level at a time.
pub trait Tree {
    type Node;
    /// The children of `n` (at most `max` of them).
    fn children(&mut self, n: &Self::Node, max: usize) -> Vec<(Self::Node, Info)>;
}

/// How much one window may cost.
pub struct Budget {
    nodes: usize,
    deadline: Instant,
    cancel: Arc<AtomicBool>,
}

impl Budget {
    pub fn new(nodes: usize, time: Duration, cancel: Arc<AtomicBool>) -> Self {
        Budget { nodes, deadline: Instant::now() + time, cancel }
    }
    pub fn left(&self) -> usize {
        self.nodes
    }
    /// Spend one element; false once it is all spent, the time is up or the picker closed.
    pub fn take(&mut self) -> bool {
        if self.nodes == 0 || self.cancel.load(Ordering::Relaxed) || Instant::now() >= self.deadline {
            return false;
        }
        self.nodes -= 1;
        true
    }
}

/// Deeper than this is never a control anyone wants to capture on its own.
const MAX_DEPTH: usize = 16;

/// An element's rectangle, cut to its parent's, or None when it shouldn't be offered:
/// hidden, empty or tiny, or outside its parent.
pub fn keep(r: (i32, i32, u32, u32), hidden: bool, clip: (i32, i32, u32, u32)) -> Option<(i32, i32, u32, u32)> {
    if hidden || r.2 < 2 || r.3 < 2 {
        return None;
    }
    let x0 = r.0.max(clip.0);
    let y0 = r.1.max(clip.1);
    let x1 = (r.0 + r.2 as i32).min(clip.0 + clip.2 as i32);
    let y1 = (r.1 + r.3 as i32).min(clip.1 + clip.3 as i32);
    (x1 - x0 >= 4 && y1 - y0 >= 4).then(|| (x0, y0, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// Read a window's elements, widest first (so a budget that runs out leaves the big parts,
/// not one corner in full detail), and nest them as the system does.
pub fn walk<T: Tree>(tree: &mut T, root: T::Node, window: (i32, i32, u32, u32), space: Space, budget: &mut Budget) -> Vec<Object> {
    // (object without its children, parent index)
    let mut flat: Vec<(Object, Option<usize>)> = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back((root, window, None::<usize>, 0usize));
    'outer: while let Some((node, clip, parent, depth)) = queue.pop_front() {
        if depth >= MAX_DEPTH {
            continue;
        }
        if budget.left() == 0 {
            break;
        }
        for (child, info) in tree.children(&node, budget.left()) {
            if !budget.take() {
                break 'outer;
            }
            let px = space.to_pixels(info.x, info.y, info.w, info.h);
            let Some(r) = keep(px, info.hidden, clip) else { continue };
            flat.push((Object { x: r.0, y: r.1, width: r.2, height: r.3, label: info.label, children: Vec::new(), handle: 0, pid: 0 }, parent));
            queue.push_back((child, r, Some(flat.len() - 1), depth + 1));
        }
    }
    nest(flat)
}

/// Turn a flat list with parent indices (parents before children) into a tree.
fn nest(mut flat: Vec<(Object, Option<usize>)>) -> Vec<Object> {
    let mut roots = Vec::new();
    while let Some((o, parent)) = flat.pop() {
        match parent {
            Some(p) if p < flat.len() => flat[p].0.children.insert(0, o),
            _ => roots.insert(0, o),
        }
    }
    roots
}

/// The name shown for an element: its kind, and its name when it has one (cut short).
pub fn element_label(kind: &str, name: &str) -> String {
    let name = name.split(['\r', '\n']).next().unwrap_or("").trim();
    let kind = if kind.is_empty() { "Element" } else { kind };
    if name.is_empty() {
        return kind.to_string();
    }
    let short: String = if name.chars().count() > 40 { name.chars().take(37).collect::<String>() + "..." } else { name.to_string() };
    format!("{kind}: {short}")
}

/// UI Automation's control types, by number.
#[cfg(any(windows, test))]
pub fn uia_kind(id: i32) -> &'static str {
    const KINDS: [&str; 41] = [
        "Button", "Calendar", "Check box", "Combo box", "Edit", "Link", "Image", "List item", "List", "Menu", "Menu bar",
        "Menu item", "Progress bar", "Radio button", "Scroll bar", "Slider", "Spinner", "Status bar", "Tabs", "Tab", "Text",
        "Toolbar", "Tooltip", "Tree", "Tree item", "Custom", "Group", "Thumb", "Data grid", "Data item", "Document",
        "Split button", "Window", "Pane", "Header", "Header item", "Table", "Title bar", "Separator", "Semantic zoom", "App bar",
    ];
    id.checked_sub(50000).and_then(|i| KINDS.get(i as usize)).copied().unwrap_or("")
}

/// The accessibility role of a macOS element, readable: "AXButton" is "Button".
#[cfg(any(target_os = "macos", test))]
pub fn ax_kind(role: &str) -> String {
    let r = role.strip_prefix("AX").unwrap_or(role);
    let mut out = String::new();
    for (i, c) in r.chars().enumerate() {
        if i > 0 && c.is_uppercase() {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// A window whose inside is to be read.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    /// Its place in the picker's list of windows.
    pub index: usize,
    pub handle: u64,
    pub pid: u32,
    /// In desktop pixels.
    pub rect: (i32, i32, u32, u32),
}

/// Which windows to read, in order: the one under the mouse, then the others front to back,
/// leaving out those not on the frozen screen (`view`) and any past `limit`.
pub fn order(objs: &[Object], cursor: Option<(i32, i32)>, view: (i32, i32, u32, u32), limit: usize) -> Vec<Target> {
    let on_view = |o: &Object| keep((o.x, o.y, o.width, o.height), false, view).is_some();
    let under = cursor.and_then(|(cx, cy)| {
        objs.iter().position(|o| cx >= o.x && cy >= o.y && cx < o.x + o.width as i32 && cy < o.y + o.height as i32)
    });
    let mut idx: Vec<usize> = under.into_iter().collect();
    idx.extend((0..objs.len()).filter(|i| Some(*i) != under && on_view(&objs[*i])));
    idx.into_iter()
        .filter(|i| objs[*i].handle != 0 || objs[*i].pid != 0)
        .take(limit)
        .map(|i| {
            let o = &objs[i];
            Target { index: i, handle: o.handle, pid: o.pid, rect: (o.x, o.y, o.width, o.height) }
        })
        .collect()
}

/// The controls found inside one window.
pub struct Detail {
    pub index: usize,
    pub children: Vec<Object>,
}

/// Elements one window may give, and how long reading it may take.
const NODES_PER_WINDOW: usize = 4000;
const TIME_PER_WINDOW: Duration = Duration::from_millis(1500);
/// Windows read per picker.
pub const WINDOWS: usize = 12;

/// Read the windows' insides on a worker; results arrive one window at a time. Setting
/// `cancel` (the picker closed) stops it soon after.
pub fn spawn(targets: Vec<Target>, space: Space, cancel: Arc<AtomicBool>, ctx: Option<eframe::egui::Context>) -> Receiver<Detail> {
    let (tx, rx) = channel();
    if targets.is_empty() || !available() {
        return rx;
    }
    std::thread::spawn(move || {
        super::background_thread(false);
        let mut reader = Reader::new();
        for t in targets {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let mut budget = Budget::new(NODES_PER_WINDOW, TIME_PER_WINDOW, cancel.clone());
            let children = reader.read(&t, space, &mut budget);
            if children.is_empty() {
                continue;
            }
            if tx.send(Detail { index: t.index, children }).is_err() {
                break;
            }
            if let Some(c) = &ctx {
                c.request_repaint();
            }
        }
    });
    rx
}

/// Whether this system can say what's inside other programs' windows.
pub fn available() -> bool {
    #[cfg(windows)]
    {
        true
    }
    #[cfg(target_os = "macos")]
    {
        super::mac::trusted()
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        x11_available()
    }
}

struct Reader {
    #[cfg(windows)]
    uia: Option<uia::Uia>,
}

impl Reader {
    fn new() -> Self {
        Reader {
            #[cfg(windows)]
            uia: uia::Uia::new(),
        }
    }

    fn read(&mut self, t: &Target, space: Space, budget: &mut Budget) -> Vec<Object> {
        #[cfg(windows)]
        {
            let Some(u) = &mut self.uia else { return Vec::new() };
            let Some(root) = u.root(t.handle) else { return Vec::new() };
            walk(u, root, t.rect, space, budget)
        }
        #[cfg(target_os = "macos")]
        {
            let (x, y, w, h) = space.from_pixels(t.rect.0, t.rect.1, t.rect.2, t.rect.3);
            let Some(root) = super::mac::window_element(t.pid, (x, y, w, h)) else { return Vec::new() };
            walk(&mut super::mac::Ax, root, t.rect, space, budget)
        }
        #[cfg(all(not(windows), not(target_os = "macos")))]
        {
            let Some(text) = x11_tree(t.handle) else { return Vec::new() };
            let mut tree = parse_xwininfo(&text);
            walk(&mut tree, 0, t.rect, space, budget)
        }
    }
}

/// A tree that was read in one go, element by number (0 is the window itself).
#[cfg(any(all(unix, not(target_os = "macos")), test))]
pub struct Listed {
    pub items: Vec<(Info, Vec<usize>)>,
}

#[cfg(any(all(unix, not(target_os = "macos")), test))]
impl Tree for Listed {
    type Node = usize;
    fn children(&mut self, n: &usize, max: usize) -> Vec<(usize, Info)> {
        let Some((_, kids)) = self.items.get(*n) else { return Vec::new() };
        kids.iter().take(max).map(|c| (*c, self.items[*c].0.clone())).collect()
    }
}

/// X11: the child windows of a window, as `xwininfo -tree` lists them (one line per window,
/// indented by depth, with its size and its position on the desktop).
#[cfg(any(all(unix, not(target_os = "macos")), test))]
pub fn parse_xwininfo(text: &str) -> Listed {
    static LINE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = LINE.get_or_init(|| regex::Regex::new(r"^(\s*)0x[0-9a-fA-F]+ (.*?)\s+(\d+)x(\d+)[+-]-?\d+[+-]-?\d+\s+\+(-?\d+)\+(-?\d+)\s*$").unwrap());
    let root = Info { x: 0.0, y: 0.0, w: 0.0, h: 0.0, label: String::new(), hidden: false };
    let mut items: Vec<(Info, Vec<usize>)> = vec![(root, Vec::new())];
    // (indent, item)
    let mut stack: Vec<(usize, usize)> = vec![(0, 0)];
    let mut started = false;
    for line in text.lines() {
        if line.trim_end().ends_with("children:") || line.trim_end().ends_with("child:") {
            started = true;
            continue;
        }
        let Some(c) = re.captures(line) else { continue };
        if !started {
            continue;
        }
        let indent = c[1].len();
        while stack.len() > 1 && stack.last().is_some_and(|s| s.0 >= indent) {
            stack.pop();
        }
        let parent = stack.last().map(|s| s.1).unwrap_or(0);
        let num = |i: usize| c[i].parse::<f64>().unwrap_or(0.0);
        let label = element_label("Window", &x11_name(&c[2]));
        items.push((Info { x: num(5), y: num(6), w: num(3), h: num(4), label, hidden: false }, Vec::new()));
        let me = items.len() - 1;
        items[parent].1.push(me);
        stack.push((indent, me));
    }
    Listed { items }
}

/// The name in an `xwininfo` line: the window's title, else its class.
#[cfg(any(all(unix, not(target_os = "macos")), test))]
fn x11_name(mid: &str) -> String {
    if let Some(rest) = mid.strip_prefix('"')
        && let Some(end) = rest.rfind("\": (")
    {
        return rest[..end].to_string();
    }
    // ("resource" "Class")
    let parts: Vec<&str> = mid.rsplit_once(": (").map(|(_, p)| p.split('"').collect()).unwrap_or_default();
    parts.get(3).map(|s| s.to_string()).unwrap_or_default()
}

/// `xwininfo -tree` for one window, if X11 and the tool are there.
#[cfg(all(unix, not(target_os = "macos")))]
fn x11_tree(id: u64) -> Option<String> {
    let out = std::process::Command::new("xwininfo").args(["-tree", "-id", &format!("{id:#x}")]).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Whether `xwininfo` can be run (it comes with X11's basic tools).
#[cfg(all(unix, not(target_os = "macos")))]
fn x11_available() -> bool {
    static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OK.get_or_init(|| {
        std::env::var_os("DISPLAY").is_some()
            && std::process::Command::new("xwininfo").arg("-version").output().is_ok_and(|o| o.status.success())
    })
}

#[cfg(windows)]
mod uia {
    use super::{Info, Tree};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
    use windows::Win32::UI::Accessibility::*;

    pub struct Uia {
        auto: IUIAutomation,
        walker: IUIAutomationTreeWalker,
        cache: IUIAutomationCacheRequest,
    }

    impl Uia {
        pub fn new() -> Option<Self> {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                let auto: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
                let walker = auto.ControlViewWalker().ok()?;
                let cache = auto.CreateCacheRequest().ok()?;
                for p in [UIA_BoundingRectanglePropertyId, UIA_NamePropertyId, UIA_ControlTypePropertyId, UIA_IsOffscreenPropertyId] {
                    cache.AddProperty(p).ok()?;
                }
                Some(Uia { auto, walker, cache })
            }
        }

        pub fn root(&self, handle: u64) -> Option<IUIAutomationElement> {
            unsafe { self.auto.ElementFromHandle(HWND(handle as usize as *mut core::ffi::c_void)).ok() }
        }
    }

    fn info(e: &IUIAutomationElement) -> Option<Info> {
        unsafe {
            let r = e.CachedBoundingRectangle().ok()?;
            let name = e.CachedName().map(|b| b.to_string()).unwrap_or_default();
            let kind = e.CachedControlType().map(|t| t.0).unwrap_or(0);
            let hidden = e.CachedIsOffscreen().map(|b| b.as_bool()).unwrap_or(false);
            Some(Info {
                x: r.left as f64,
                y: r.top as f64,
                w: (r.right - r.left) as f64,
                h: (r.bottom - r.top) as f64,
                label: super::element_label(super::uia_kind(kind), &name),
                hidden,
            })
        }
    }

    impl Tree for Uia {
        type Node = IUIAutomationElement;
        fn children(&mut self, n: &IUIAutomationElement, max: usize) -> Vec<(IUIAutomationElement, Info)> {
            let mut out = Vec::new();
            unsafe {
                let mut next = self.walker.GetFirstChildElementBuildCache(n, &self.cache);
                while let Ok(e) = next {
                    if out.len() >= max {
                        break;
                    }
                    next = self.walker.GetNextSiblingElementBuildCache(&e, &self.cache);
                    if let Some(i) = info(&e) {
                        out.push((e, i));
                    }
                }
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(x: f64, y: f64, w: f64, h: f64, label: &str) -> Info {
        Info { x, y, w, h, label: label.into(), hidden: false }
    }

    /// A made-up program: each element by number, with its children.
    fn app() -> Listed {
        // 0 window, 1 toolbar (2, 3 buttons), 4 list (5 visible row, 6 scrolled-out row, 7 hidden), 8 empty pane
        let mut hidden = info(20.0, 60.0, 50.0, 10.0, "Hidden");
        hidden.hidden = true;
        Listed {
            items: vec![
                (info(0.0, 0.0, 400.0, 300.0, "Window"), vec![1, 4, 8]),
                (info(0.0, 0.0, 400.0, 40.0, "Toolbar"), vec![2, 3]),
                (info(5.0, 5.0, 30.0, 30.0, "Button: Save"), vec![]),
                (info(40.0, 5.0, 30.0, 30.0, "Button: Open"), vec![]),
                (info(0.0, 40.0, 200.0, 260.0, "List"), vec![5, 6, 7]),
                (info(0.0, 40.0, 200.0, 20.0, "List item: One"), vec![]),
                (info(0.0, 900.0, 200.0, 20.0, "List item: Far"), vec![]),
                (hidden, vec![]),
                (info(300.0, 100.0, 0.0, 0.0, "Empty"), vec![]),
            ],
        }
    }

    fn budget(n: usize) -> Budget {
        Budget::new(n, Duration::from_secs(60), Arc::new(AtomicBool::new(false)))
    }

    #[test]
    fn a_window_is_read_into_a_nested_tree() {
        let mut f = app();
        let kids = walk(&mut f, 0, (0, 0, 400, 300), Space::default(), &mut budget(100));
        let labels: Vec<&str> = kids.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(labels, ["Toolbar", "List"], "the empty pane is left out");
        assert_eq!(kids[0].children.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(), ["Button: Save", "Button: Open"]);
        // The row scrolled out of the list and the hidden one are left out.
        assert_eq!(kids[1].children.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(), ["List item: One"]);
        let b = &kids[0].children[1];
        assert_eq!((b.x, b.y, b.width, b.height), (40, 5, 30, 30));
    }

    #[test]
    fn the_budget_keeps_the_widest_parts() {
        let mut f = app();
        // Three elements: the window's three children are looked at first (breadth first).
        let kids = walk(&mut f, 0, (0, 0, 400, 300), Space::default(), &mut budget(3));
        assert_eq!(kids.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(), ["Toolbar", "List"]);
        assert!(kids.iter().all(|k| k.children.is_empty()));
        // A cancelled picker reads nothing more.
        let cancel = Arc::new(AtomicBool::new(true));
        let mut b = Budget::new(100, Duration::from_secs(60), cancel);
        assert!(walk(&mut app(), 0, (0, 0, 400, 300), Space::default(), &mut b).is_empty());
        // Out of time.
        let mut b = Budget::new(100, Duration::ZERO, Arc::new(AtomicBool::new(false)));
        assert!(!b.take());
    }

    #[test]
    fn children_are_cut_to_their_parent() {
        let mut f = Listed {
            items: vec![
                (info(0.0, 0.0, 100.0, 100.0, "Window"), vec![1]),
                (info(50.0, 50.0, 200.0, 200.0, "Panel"), vec![2]),
                (info(-20.0, 90.0, 400.0, 30.0, "Wide"), vec![]),
            ],
        };
        let kids = walk(&mut f, 0, (0, 0, 100, 100), Space::default(), &mut budget(10));
        assert_eq!((kids[0].x, kids[0].y, kids[0].width, kids[0].height), (50, 50, 50, 50));
        let w = &kids[0].children[0];
        assert_eq!((w.x, w.y, w.width, w.height), (50, 90, 50, 10));
    }

    #[test]
    fn deep_trees_stop_at_the_depth_limit() {
        let n = 40;
        let items = (0..n).map(|i| (info(0.0, 0.0, 100.0, 100.0, "Pane"), if i + 1 < n { vec![i + 1] } else { vec![] })).collect();
        let mut f = Listed { items };
        let mut kids = walk(&mut f, 0, (0, 0, 100, 100), Space::default(), &mut budget(1000));
        let mut depth = 0;
        while let Some(k) = kids.pop() {
            depth += 1;
            kids = k.children;
        }
        assert_eq!(depth, MAX_DEPTH);
    }

    #[test]
    fn keep_drops_hidden_empty_and_outside_elements() {
        let clip = (0, 0, 100, 100);
        assert_eq!(keep((10, 10, 20, 20), false, clip), Some((10, 10, 20, 20)));
        assert_eq!(keep((10, 10, 20, 20), true, clip), None);
        assert_eq!(keep((10, 10, 0, 20), false, clip), None);
        assert_eq!(keep((10, 10, 20, 1), false, clip), None);
        assert_eq!(keep((200, 10, 20, 20), false, clip), None);
        assert_eq!(keep((98, 10, 20, 20), false, clip), None, "a 2-pixel sliver isn't worth offering");
        assert_eq!(keep((-10, -10, 30, 200), false, clip), Some((0, 0, 20, 100)));
    }

    #[test]
    fn points_become_pixels_on_a_retina_screen() {
        let s = Space { origin: (0, 0), scale: 2.0 };
        assert_eq!(s.to_pixels(10.0, 20.0, 100.0, 50.0), (20, 40, 200, 100));
        assert_eq!(s.from_pixels(20, 40, 200, 100), (10.0, 20.0, 100.0, 50.0));
        // A second screen to the right, at 1440 points: the scale is taken from its left edge.
        let s = Space { origin: (1440, 0), scale: 2.0 };
        assert_eq!(s.to_pixels(1450.0, 0.0, 10.0, 10.0), (1460, 0, 20, 20));
        assert_eq!(s.from_pixels(1460, 0, 20, 20), (1450.0, 0.0, 10.0, 10.0));
        // Fractional scales round to whole pixels without gaps between neighbours.
        let s = Space { origin: (0, 0), scale: 1.5 };
        let a = s.to_pixels(0.0, 0.0, 3.0, 3.0);
        let b = s.to_pixels(3.0, 0.0, 3.0, 3.0);
        assert_eq!(a.0 + a.2 as i32, b.0);
        assert_eq!(Space::scale_for(2880, 1440), 2.0);
        assert_eq!(Space::scale_for(1920, 1920), 1.0);
        assert_eq!(Space::scale_for(1920, 0), 1.0);
    }

    #[test]
    fn whole_trees_are_scaled() {
        let mut objs = vec![Object {
            x: 10,
            y: 10,
            width: 100,
            height: 50,
            label: "W".into(),
            children: vec![Object { x: 20, y: 20, width: 10, height: 10, label: "B".into(), children: Vec::new(), handle: 0, pid: 0 }],
            handle: 1,
            pid: 2,
        }];
        Space { origin: (0, 0), scale: 2.0 }.scale_objects(&mut objs);
        assert_eq!((objs[0].x, objs[0].width), (20, 200));
        assert_eq!((objs[0].children[0].x, objs[0].children[0].height), (40, 20));
        assert_eq!(objs[0].handle, 1);
    }

    fn top(x: i32, y: i32, w: u32, h: u32, handle: u64) -> Object {
        Object { x, y, width: w, height: h, label: String::new(), children: Vec::new(), handle, pid: 0 }
    }

    #[test]
    fn the_window_under_the_mouse_is_read_first() {
        let objs = vec![
            top(0, 0, 100, 100, 1),
            top(200, 0, 100, 100, 2),
            top(5000, 0, 100, 100, 3),
            top(0, 0, 800, 600, 4),
            top(10, 10, 50, 50, 0),
        ];
        let view = (0, 0, 1920, 1080);
        let t = order(&objs, Some((250, 50)), view, 10);
        assert_eq!(t.iter().map(|t| t.handle).collect::<Vec<_>>(), [2, 1, 4], "off-screen and handle-less windows are left out");
        assert_eq!(t[0].index, 1);
        assert_eq!(t[0].rect, (200, 0, 100, 100));
        // The front window wins where windows overlap.
        assert_eq!(order(&objs, Some((50, 50)), view, 10)[0].handle, 1);
        assert_eq!(order(&objs, None, view, 2).len(), 2);
    }

    #[test]
    fn element_names_read_well() {
        assert_eq!(element_label("Button", "Save"), "Button: Save");
        assert_eq!(element_label("Pane", ""), "Pane");
        assert_eq!(element_label("", "  "), "Element");
        assert_eq!(element_label("Edit", "first line\nsecond"), "Edit: first line");
        let long = element_label("Text", &"\u{e9}".repeat(60));
        assert_eq!(long.chars().count(), "Text: ".len() + 40);
        assert_eq!(uia_kind(50000), "Button");
        assert_eq!(uia_kind(50033), "Pane");
        assert_eq!(uia_kind(50040), "App bar");
        assert_eq!(uia_kind(50041), "");
        assert_eq!(uia_kind(7), "");
        assert_eq!(ax_kind("AXButton"), "Button");
        assert_eq!(ax_kind("AXScrollArea"), "Scroll area");
        assert_eq!(ax_kind("Plain"), "Plain");
    }

    #[test]
    fn nesting_keeps_order() {
        let o = |l: &str| Object { x: 0, y: 0, width: 1, height: 1, label: l.into(), children: Vec::new(), handle: 0, pid: 0 };
        let t = nest(vec![(o("a"), None), (o("b"), None), (o("a1"), Some(0)), (o("a2"), Some(0)), (o("a1x"), Some(2))]);
        assert_eq!(t.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(t[0].children.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(), ["a1", "a2"]);
        assert_eq!(t[0].children[0].children[0].label, "a1x");
    }

    #[test]
    fn x11_child_windows_are_read_from_xwininfo() {
        let text = r#"
xwininfo: Window id: 0x3a00007 "Editor"

  Root window id: 0x1e5 (the root window) (has no name)
  Parent window id: 0x1e5 (the root window) (has no name)
     3 children:
     0x3a00008 (has no name): ()  1x1+-1+-1  +-1+-1
     0x3a0000a "Main view": ("editor" "Editor")  800x560+0+40  +100+140
        2 children:
        0x3a0000b (has no name): ("scroll" "Scrollbar")  16x560+784+0  +884+140
        0x3a0000c "Canvas": ()  784x560+0+0  +100+140
     0x3a0000d (has no name): ("toolbar" "Toolbar")  800x40+0+0  +100+100
"#;
        let mut t = parse_xwininfo(text);
        assert_eq!(t.items.len(), 6);
        let top: Vec<&str> = t.items[0].1.iter().map(|i| t.items[*i].0.label.as_str()).collect();
        assert_eq!(top, ["Window", "Window: Main view", "Window: Toolbar"]);
        let view = &t.items[2];
        assert_eq!((view.0.x, view.0.y, view.0.w, view.0.h), (100.0, 140.0, 800.0, 560.0));
        assert_eq!(view.1.iter().map(|i| t.items[*i].0.label.as_str()).collect::<Vec<_>>(), ["Window: Scrollbar", "Window: Canvas"]);
        // Walked like any other tree: the 1x1 window is dropped, the rest nests.
        let kids = walk(&mut t, 0, (100, 100, 800, 600), Space::default(), &mut budget(100));
        assert_eq!(kids.len(), 2);
        assert_eq!(kids[0].children.len(), 2);
        assert_eq!(x11_name("\"A \\\"quoted\\\" title\": (\"a\" \"B\")"), "A \\\"quoted\\\" title");
        assert_eq!(x11_name("(has no name): ()"), "");
        assert!(parse_xwininfo("xwininfo: error").items[0].1.is_empty());
    }
}