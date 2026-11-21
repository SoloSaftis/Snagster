//! The main window from the keyboard, the way each system does it.
//!
//! - Everywhere: F6 / Shift+F6 move between the panes (ribbon, tools, picture, options, Recent).
//!   Tab / Shift+Tab move from control to control, the arrow keys move within a pane (they never
//!   jump into another one), Enter or Space runs the focused control, Esc goes back to the
//!   picture. A ring shows which control has the keyboard. egui hands its focus to AccessKit,
//!   so screen readers follow every move.
//! - Windows: Alt (pressed and let go) or F10 shows key tips on the ribbon's tabs and buttons, as
//!   in Office. A tab's letter opens it and shows its tools' letters; a tool's letters run it; Esc
//!   goes back a level; Alt or F10 again hides them. Alt+letter opens a tab at once.
//! - Linux (GNOME, KDE): F10 opens the main menu (see `ribbon::main_menu`); holding Alt shows the
//!   tabs' letters and Alt+letter opens one, like GTK's and Qt's mnemonics.
//! - macOS: the menu bar covers this (Ctrl+F2 reaches it, the system's own key), so no key tips.
//!
//! The ribbon's, tool column's and Recent strip's widgets are noted in a registry while they're
//! drawn ([`note`]); the keys pressed in the next frame act on that list.

use super::*;
use crate::app::ribbon::RibbonTab;
use egui::{Event, FocusDirection, Id, Key};
use std::cell::RefCell;

/// The parts of the main window F6 moves between, in order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Pane {
    Ribbon,
    Tools,
    Canvas,
    Options,
    Recent,
}

/// Where a noted widget is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Scope {
    /// File, the tabs and the buttons at the end of the tab row.
    TabRow,
    /// The tools of the open tab.
    Band,
    Tools,
    Recent,
}

impl Scope {
    pub(crate) fn pane(self) -> Pane {
        match self {
            Scope::TabRow | Scope::Band => Pane::Ribbon,
            Scope::Tools => Pane::Tools,
            Scope::Recent => Pane::Recent,
        }
    }
}

/// A widget the keyboard can reach, as it was drawn.
#[derive(Clone, Debug)]
pub(crate) struct Item {
    pub id: Id,
    pub rect: Rect,
    pub label: String,
    pub enabled: bool,
    /// The open tab, the chosen tool, the picture shown.
    pub on: bool,
    pub scope: Scope,
    /// A ribbon tab: its key tip opens it instead of clicking it.
    pub tab: Option<RibbonTab>,
}

#[derive(Default)]
struct Registry {
    scope: Option<(Scope, egui::LayerId)>,
    items: Vec<Item>,
}

thread_local! {
    static REG: RefCell<Registry> = RefCell::new(Registry::default());
}

/// Note the widgets drawn from now on as being in `scope` (None: stop noting).
pub(crate) fn set_scope(ui: &egui::Ui, scope: Option<Scope>) {
    REG.with(|r| r.borrow_mut().scope = scope.map(|s| (s, ui.layer_id())));
}

/// Note a widget the keyboard can reach. Widgets in a popup (the band's More menu, a tool's
/// menu) aren't noted: they're on another layer.
pub(crate) fn note(ui: &egui::Ui, resp: &egui::Response, label: &str, enabled: bool, on: bool) {
    REG.with(|r| {
        let mut r = r.borrow_mut();
        if let Some((scope, layer)) = r.scope
            && layer == ui.layer_id()
        {
            r.items.push(Item { id: resp.id, rect: resp.rect, label: label.to_string(), enabled, on, scope, tab: None });
        }
    });
}

/// The widget just noted is the tab `tab`.
pub(crate) fn note_tab(resp: &egui::Response, tab: RibbonTab) {
    REG.with(|r| {
        if let Some(it) = r.borrow_mut().items.iter_mut().rev().find(|i| i.id == resp.id) {
            it.tab = Some(tab);
        }
    });
}

fn take_items() -> Vec<Item> {
    REG.with(|r| {
        let mut r = r.borrow_mut();
        r.scope = None;
        std::mem::take(&mut r.items)
    })
}

fn peek_items() -> Vec<Item> {
    REG.with(|r| r.borrow().items.clone())
}

// ---------- key tips ----------

/// The keys a key tip can use: letters first, then digits. Every keyboard layout has them (the
/// egui fork maps other scripts' keys onto these), so a tip can always be typed. Last, and never
/// picked for a name's own letter: those easily mistaken for others (I and 1, O and 0, L) and
/// those GNOME's guidelines leave out because they hang below the line (G, Y).
const POOL: &[char] = &[
    'A', 'B', 'C', 'D', 'E', 'F', 'H', 'J', 'K', 'M', 'N', 'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Z', '2', '3', '4', '5', '6', '7', '8', '9', 'G', 'I', 'L', 'O', 'Y', '1', '0',
];
const AVOID: &[char] = &['G', 'I', 'L', 'O', 'Y', '1', '0'];

/// Key tips for widgets named `labels` (in the language shown), in their order: unique, and
/// the same every time for the same names. Each takes the first free letter of its own name
/// (its first letter, then the first letters of its other words, then the rest), as Office does;
/// names in other scripts get free letters in turn. With more widgets than keys, the last keys
/// become two-key tips ("Z1"), and no tip is the start of another.
pub(crate) fn assign(labels: &[&str]) -> Vec<String> {
    let n = labels.len();
    // How many keys must start two-key tips: singles + prefixes x pool must cover n.
    let mut prefixes = 0;
    while POOL.len() - prefixes + prefixes * POOL.len() < n {
        prefixes += 1;
    }
    let singles: Vec<char> = POOL[..POOL.len() - prefixes].to_vec();
    let lead: Vec<char> = POOL[POOL.len() - prefixes..].iter().rev().copied().collect();
    let mut used = vec![false; singles.len()];
    let mut out: Vec<Option<String>> = vec![None; n];
    // 1. Letters of the widget's own name.
    for (i, label) in labels.iter().enumerate() {
        let wanted = preferred(label);
        if let Some(k) = wanted.iter().find_map(|c| singles.iter().position(|s| s == c).filter(|&k| !used[k])) {
            used[k] = true;
            out[i] = Some(singles[k].to_string());
        }
    }
    // 2. Any free key, in order.
    for slot in out.iter_mut().filter(|s| s.is_none()) {
        if let Some(k) = used.iter().position(|u| !u) {
            used[k] = true;
            *slot = Some(singles[k].to_string());
        }
    }
    // 3. Two-key tips for the rest.
    let mut combos = lead.iter().flat_map(|a| POOL.iter().map(move |b| format!("{a}{b}")));
    out.into_iter().map(|s| s.or_else(|| combos.next()).unwrap_or_default()).collect()
}

/// The keys a name would like, best first: its first letter, its words' first letters, then
/// its other letters and digits (only those a key gives: A-Z and 0-9).
fn preferred(label: &str) -> Vec<char> {
    let key = |c: char| (c.is_ascii_alphanumeric() && !AVOID.contains(&c.to_ascii_uppercase())).then(|| c.to_ascii_uppercase());
    let mut v: Vec<char> = Vec::new();
    let mut push = |c: Option<char>| {
        if let Some(c) = c
            && !v.contains(&c)
        {
            v.push(c);
        }
    };
    let words: Vec<&str> = label.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    for w in &words {
        push(w.chars().next().and_then(key));
    }
    for c in label.chars() {
        push(key(c));
    }
    v
}

/// What the keys typed so far mean among `tips`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Typed {
    /// This tip is typed in full.
    Is(usize),
    /// The start of one or more tips: wait for the next key.
    Starts,
    /// No tip.
    Nothing,
}

pub(crate) fn typed(tips: &[String], keys: &str) -> Typed {
    if keys.is_empty() {
        return Typed::Nothing;
    }
    if let Some(i) = tips.iter().position(|t| t == keys) {
        return Typed::Is(i);
    }
    if tips.iter().any(|t| t.starts_with(keys)) { Typed::Starts } else { Typed::Nothing }
}

/// The character a key types into a key tip.
fn tip_char(key: Key) -> Option<char> {
    let name = key.name();
    let mut c = name.chars();
    match (c.next(), c.next()) {
        (Some(ch), None) if ch.is_ascii_alphanumeric() => Some(ch.to_ascii_uppercase()),
        _ => None,
    }
}

/// Which level of key tips is shown.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub(crate) enum Tips {
    #[default]
    Off,
    /// On the tab row.
    Tabs,
    /// On the open tab's tools.
    Band,
}

/// Alt pressed and let go with nothing in between (Windows shows key tips then), and Alt held
/// on its own (Linux shows mnemonics meanwhile).
#[derive(Default, Debug)]
pub(crate) struct AltTap {
    down: bool,
    clean: bool,
}

impl AltTap {
    /// Feed one input event; true when it ends a clean tap of Alt.
    pub(crate) fn feed(&mut self, e: &Event) -> bool {
        match e {
            Event::ModifiersChanged(m) => {
                if !m.alt {
                    let tap = self.down && self.clean;
                    self.down = false;
                    self.clean = false;
                    return tap;
                }
                // With Ctrl (AltGr on many keyboards) or Shift (switching layout) it's a
                // combination, until Alt is let go.
                let alone = alt_only(*m);
                if !self.down {
                    self.down = true;
                    self.clean = alone;
                } else if !alone {
                    self.clean = false;
                }
                false
            }
            // Alt+key, a click, or the window coming to the front with Alt+Tab: not a tap.
            Event::Key { .. } | Event::PointerButton { .. } | Event::WindowFocused(_) | Event::MouseWheel { .. } => {
                self.clean = false;
                false
            }
            _ => false,
        }
    }

    /// Alt is held on its own.
    pub(crate) fn held(&self) -> bool {
        self.down && self.clean
    }
}

/// Which systems show key tips, and how.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TipStyle {
    /// Alt or F10 toggles them, Alt+letter opens a tab.
    Office,
    /// Holding Alt shows the tabs' letters, Alt+letter opens a tab.
    Mnemonics,
    /// None (the menu bar has the keyboard).
    Off,
}

pub(crate) fn tip_style(sys: crate::platform::System) -> TipStyle {
    use crate::platform::System;
    match sys {
        System::Windows => TipStyle::Office,
        System::Gnome | System::Kde | System::OtherLinux => TipStyle::Mnemonics,
        System::Mac => TipStyle::Off,
    }
}

// ---------- moving around ----------

/// The pane after (or before, `back`) the one with the keyboard, among those shown. From
/// nowhere, F6 goes to the first and Shift+F6 to the last.
pub(crate) fn next_pane(shown: &[Pane], at: Option<Pane>, back: bool) -> Option<Pane> {
    if shown.is_empty() {
        return None;
    }
    let n = shown.len();
    let i = match at.and_then(|p| shown.iter().position(|s| *s == p)) {
        Some(i) if back => (i + n - 1) % n,
        Some(i) => (i + 1) % n,
        None if back => n - 1,
        None => 0,
    };
    Some(shown[i])
}

/// The widget an arrow key moves to from `rects[from]`: the nearest one in that direction
/// (within 45 degrees of it), or none at the edge of the pane.
pub(crate) fn step(rects: &[Rect], from: usize, dir: Vec2) -> Option<usize> {
    // How far apart two ranges are; nothing when they overlap well (a row, a column).
    fn apart(a: egui::Rangef, b: egui::Rangef) -> f32 {
        if a.intersection(b).span() >= 0.5 * b.span().min(a.span()) { 0.0 } else { a.center() - b.center() }
    }
    let here = *rects.get(from)?;
    // Ties (a column beside a big tool) go to the one whose middle is nearest.
    let mut best: Option<((f32, f32), usize)> = None;
    for (i, r) in rects.iter().enumerate() {
        if i == from {
            continue;
        }
        let to = vec2(apart(r.x_range(), here.x_range()), apart(r.y_range(), here.y_range()));
        if to == Vec2::ZERO {
            continue;
        }
        let cos = to.normalized().dot(dir);
        if cos < 0.5f32.sqrt() {
            continue;
        }
        let score = (to.length() / (cos * cos), (r.center() - here.center()).length());
        if best.is_none_or(|(s, _)| score < s) {
            best = Some((score, i));
        }
    }
    best.map(|(_, i)| i)
}

fn arrow(key: Key) -> Option<Vec2> {
    match key {
        Key::ArrowLeft => Some(Vec2::LEFT),
        Key::ArrowRight => Some(Vec2::RIGHT),
        Key::ArrowUp => Some(Vec2::UP),
        Key::ArrowDown => Some(Vec2::DOWN),
        _ => None,
    }
}

/// The keyboard's state in the main window.
#[derive(Default)]
pub(crate) struct KeyNav {
    /// The widgets noted while the last frame was drawn.
    last: Vec<Item>,
    /// The panes shown last frame and where; those of this frame.
    panes: Vec<(Pane, Rect)>,
    panes_now: Vec<(Pane, Rect)>,
    pub tips: Tips,
    keys: String,
    alt: AltTap,
    /// A pane to give the keyboard to as it's drawn (its first control).
    enter: Option<Pane>,
    /// The picture's canvas, when one was drawn last frame / this frame.
    canvas: Option<Id>,
    canvas_now: Option<Id>,
    /// What had the keyboard at the end of the last frame.
    prev_focus: Option<Id>,
    /// What had the keyboard before Alt or F10 showed the key tips.
    before_tips: Option<Option<Id>>,
    /// The mark the arrows moved last, and when: holding an arrow is one undo step.
    nudged: Option<(usize, usize, f64)>,
    /// The control an arrow left to egui last frame, and its pane.
    arrowed: Option<(Id, Pane)>,
    /// A control the keyboard went to, to scroll into the middle of its pane's scroll area.
    scroll: Option<Id>,
}

impl KeyNav {
    /// Give a widget the keyboard, scrolled into view (a picture far along Recent).
    fn focus(&mut self, ctx: &egui::Context, id: Id) {
        ctx.memory_mut(|m| m.request_focus(id));
        self.scroll = Some(id);
    }

    /// Inside pane `pane`'s scroll area: scroll to the control the keyboard went to, if it's
    /// there. (egui only scrolls an area for a request made inside it.)
    pub(crate) fn scroll_here(&mut self, ctx: &egui::Context, pane: Pane) {
        if let Some(id) = self.scroll
            && self.pane_of(ctx, Some(id)) == Some(pane)
            // The tab row isn't in the band's scroll area.
            && !self.last.iter().any(|i| i.id == id && i.scope == Scope::TabRow)
            && let Some(r) = ctx.read_response(id)
        {
            self.scroll = None;
            r.scroll_to_me(Some(egui::Align::Center));
        }
    }

    /// Undo and redo start a new step for the next arrow on the picture.
    pub(crate) fn forget_nudge(&mut self) {
        self.nudged = None;
    }

    /// The canvas widget, so its keys still work while it has the keyboard.
    pub(crate) fn is_canvas(&self, id: Option<Id>) -> bool {
        id.is_some() && (id == self.canvas || id == self.canvas_now)
    }

    pub(crate) fn pane_of(&self, ctx: &egui::Context, id: Option<Id>) -> Option<Pane> {
        let id = id?;
        if self.is_canvas(Some(id)) {
            return Some(Pane::Canvas);
        }
        if let Some(it) = self.last.iter().find(|i| i.id == id) {
            return Some(it.scope.pane());
        }
        // The panes are the window's panels; a window or menu floating over them is none.
        let r = ctx.read_response(id)?;
        if r.layer_id.order != egui::Order::Background {
            return None;
        }
        let at = r.rect.center();
        self.panes.iter().find(|(_, r)| r.contains(at)).map(|(p, _)| *p).or_else(|| {
            // The options panel scrolls up and down: a control scrolled out of it is still in it
            // (the tools, Recent and the ribbon note theirs, so they never get here).
            self.panes.iter().find(|(p, r)| *p == Pane::Options && r.x_range().contains(at.x)).map(|(p, _)| *p)
        })
    }
}

/// An input event's key, when it is a key press.
fn pressed(e: &Event) -> Option<(Key, egui::Modifiers)> {
    match e {
        Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
        _ => None,
    }
}

fn alt_only(m: egui::Modifiers) -> bool {
    m.alt && !m.ctrl && !m.shift && !m.mac_cmd && !m.command
}


/// Click a widget as a screen reader would (egui runs it as a click this frame).
fn click(ctx: &egui::Context, id: Id) {
    let req = egui::accesskit::ActionRequest { action: egui::accesskit::Action::Click, target_tree: egui::accesskit::TreeId::ROOT, target_node: id.accesskit_id(), data: None };
    ctx.input_mut(|i| i.events.push(Event::AccessKitActionRequest(req)));
}

impl SnagApp {
    /// Before the window is drawn: F6, the arrows, Esc and the key tips.
    pub(crate) fn keys_begin(&mut self, ctx: &egui::Context) {
        let k = &mut self.x.keys;
        k.last = take_items();
        k.panes = std::mem::take(&mut k.panes_now);
        // In F6's order, not the order they're laid out in.
        k.panes.sort_by_key(|p| p.0);
        k.canvas = k.canvas_now.take();
        // A box inside the window (Wayland) has the keys: F6, the arrows on the picture and
        // Alt+letter stay off the editor behind it.
        if crate::windows::box_has_keys(ctx) {
            k.tips = Tips::Off;
            k.before_tips = None;
            return;
        }
        let style = tip_style(crate::platform::system());
        let popup = egui::Popup::is_any_open(ctx);
        let typing = ctx.text_edit_focused() || self.text_edit.is_some();
        let focused = ctx.memory(|m| m.focused());
        let events = ctx.input(|i| i.events.clone());
        let mut tapped = false;
        for e in &events {
            if self.x.keys.alt.feed(e) {
                tapped = true;
            }
            if matches!(e, Event::PointerButton { pressed: true, .. } | Event::WindowFocused(false)) {
                self.x.keys.tips = Tips::Off;
            }
        }
        if style == TipStyle::Off || typing {
            self.x.keys.tips = Tips::Off;
        }
        // Windows: Alt (alone) or F10 shows or hides the key tips.
        let f10 = style == TipStyle::Office && !typing && !popup && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::F10));
        if style == TipStyle::Office && !typing && !popup && (tapped || f10) {
            self.x.keys.keys.clear();
            if self.x.keys.tips == Tips::Off {
                // As in Office the open tab takes the keyboard, so the arrows, Tab and Enter
                // work in the ribbon too; leaving gives it back.
                self.x.keys.tips = Tips::Tabs;
                self.x.keys.before_tips = Some(focused);
                self.enter_pane(ctx, Pane::Ribbon);
            } else {
                self.tips_off(ctx);
            }
        }
        // Alt+letter opens a tab at once (Windows and Linux), not with a menu open.
        if style != TipStyle::Off && !typing && !popup {
            let tips = self.tips_for(Scope::TabRow);
            let mut hit = None;
            ctx.input_mut(|i| {
                i.events.retain(|e| {
                    if hit.is_none()
                        && let Some((key, m)) = pressed(e)
                        && alt_only(m)
                        && let Some(c) = tip_char(key)
                        && let Typed::Is(n) = typed(&tips.1, &c.to_string())
                    {
                        hit = Some(n);
                        return false;
                    }
                    true
                })
            });
            if let Some(n) = hit {
                let it = tips.0[n].clone();
                self.tip_hit(ctx, &it);
            }
        }
        if self.x.keys.tips != Tips::Off {
            self.tips_keys(ctx);
        }
        if !popup {
            self.pane_keys(ctx, focused, typing);
        }
        if self.x.keys.tips == Tips::Off {
            self.x.keys.before_tips = None;
        }
    }

    /// The widgets of `scope` noted last frame, and their key tips.
    fn tips_for(&self, scope: Scope) -> (Vec<Item>, Vec<String>) {
        let items: Vec<Item> = self.x.keys.last.iter().filter(|i| i.scope == scope).cloned().collect();
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        let tips = assign(&labels);
        (items, tips)
    }

    /// Keys while key tips are shown: letters pick, Esc goes back a level, other keys leave.
    fn tips_keys(&mut self, ctx: &egui::Context) {
        let scope = if self.x.keys.tips == Tips::Band { Scope::Band } else { Scope::TabRow };
        let (items, tips) = self.tips_for(scope);
        let mut hit = None;
        let mut back = false;
        let mut leave = false;
        let keys = &mut self.x.keys.keys;
        ctx.input_mut(|i| {
            i.events.retain(|e| {
                if matches!(e, Event::Text(_)) {
                    return false;
                }
                let Some((key, m)) = pressed(e) else { return true };
                if key == Key::Escape && !m.any() {
                    back = true;
                    return false;
                }
                match tip_char(key) {
                    Some(c) if hit.is_none() && !m.ctrl && !m.command && !m.mac_cmd => {
                        keys.push(c);
                        match typed(&tips, keys) {
                            Typed::Is(n) => hit = Some(n),
                            Typed::Starts => {}
                            Typed::Nothing => keys.clear(),
                        }
                        false
                    }
                    _ => {
                        leave = true;
                        true
                    }
                }
            })
        });
        if back {
            self.x.keys.keys.clear();
            if self.x.keys.tips == Tips::Band {
                self.x.keys.tips = Tips::Tabs;
            } else {
                self.tips_off(ctx);
            }
        } else if let Some(n) = hit {
            self.x.keys.keys.clear();
            self.tip_hit(ctx, &items[n]);
        } else if leave {
            self.x.keys.keys.clear();
            self.x.keys.tips = Tips::Off;
        }
    }

    /// Alt, F10 or Esc hid the key tips: the keyboard goes back where it was before them.
    fn tips_off(&mut self, ctx: &egui::Context) {
        self.x.keys.tips = Tips::Off;
        if let Some(before) = self.x.keys.before_tips.take() {
            match before {
                Some(id) => ctx.memory_mut(|m| m.request_focus(id)),
                None => {
                    if let Some(f) = ctx.memory(|m| m.focused()) {
                        ctx.memory_mut(|m| m.surrender_focus(f));
                    }
                }
            }
        }
    }

    /// A key tip was typed: open its tab, or run its tool.
    fn tip_hit(&mut self, ctx: &egui::Context, it: &Item) {
        ctx.memory_mut(|m| m.request_focus(it.id));
        if let Some(tab) = it.tab {
            self.settings.ribbon_folded = false;
            if self.x.tab != tab {
                self.switch_tab(ctx, tab);
            }
            self.x.keys.tips = Tips::Band;
        } else {
            if it.enabled {
                click(ctx, it.id);
            }
            self.x.keys.tips = Tips::Off;
        }
    }

    /// F6 between panes, arrows within one, Esc back to the picture.
    fn pane_keys(&mut self, ctx: &egui::Context, focused: Option<Id>, typing: bool) {
        // An arrow let through to egui (a control we don't know, in the options panel) that
        // took the keyboard into another pane: back where it was.
        if let Some((id, pane)) = self.x.keys.arrowed.take()
            && focused != Some(id)
        {
            if self.x.keys.pane_of(ctx, focused) != Some(pane) {
                ctx.memory_mut(|m| m.request_focus(id));
                return;
            }
            // Still in the pane: keep it in the middle, so the next one down or up is in view too
            // (egui only moves to controls it can see).
            self.x.keys.scroll = focused;
        }
        let back = ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, Key::F6));
        let fwd = !back && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::F6));
        if fwd || back {
            self.x.keys.tips = Tips::Off;
            let at = self.x.keys.pane_of(ctx, focused.or(self.x.keys.prev_focus));
            let shown: Vec<Pane> = self.x.keys.panes.iter().map(|p| p.0).collect();
            if let Some(p) = next_pane(&shown, at, back) {
                self.enter_pane(ctx, p);
            }
            return;
        }
        if typing {
            return;
        }
        // Esc in a pane (egui has just let go of the focus) goes back to the picture.
        let k = &self.x.keys;
        if focused.is_none()
            && k.tips == Tips::Off
            && let Some(canvas) = k.canvas
            && !k.is_canvas(k.prev_focus)
            && k.pane_of(ctx, k.prev_focus).is_some()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape))
        {
            ctx.memory_mut(|m| m.request_focus(canvas));
            return;
        }
        let Some(f) = focused else { return };
        let dir = ctx.input(|i| i.events.iter().filter_map(pressed).find_map(|(key, m)| if m.any() && !m.shift_only() { None } else { arrow(key).map(|d| (key, m, d)) }));
        let Some((key, m, d)) = dir else { return };
        if k.is_canvas(Some(f)) {
            // On the picture the arrows move the picked mark (Shift: ten at a time).
            ctx.memory_mut(|mem| mem.move_focus(FocusDirection::None));
            ctx.input_mut(|i| i.consume_key(m, key));
            let now = ctx.input(|i| i.time);
            self.nudge_selected(d * if m.shift { 10.0 } else { 1.0 }, now);
            return;
        }
        if m.any() {
            return;
        }
        let Some(item) = k.last.iter().find(|i| i.id == f) else {
            // A control we don't know (the options panel's). A slider keeps its arrows. Down
            // and Up go to the next and the previous control, as the panel is a column: egui's
            // own search could pick something nearer in another pane (Recent, under a short
            // panel). Left and Right are egui's. Leaving the pane is undone above.
            let Some(pane) = k.pane_of(ctx, Some(f)) else { return };
            if ctx.read_response(f).is_some_and(|r| r.sense.senses_drag()) {
                return;
            }
            if d.y != 0.0 {
                ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key));
                ctx.memory_mut(|mem| mem.move_focus(if d.y > 0.0 { FocusDirection::Next } else { FocusDirection::Previous }));
            }
            self.x.keys.arrowed = Some((f, pane));
            return;
        };
        let pane = item.scope.pane();
        let mine: Vec<&Item> = k.last.iter().filter(|i| i.scope.pane() == pane && (i.enabled || i.id == f)).collect();
        let Some(from) = mine.iter().position(|i| i.id == f) else { return };
        let rects: Vec<Rect> = mine.iter().map(|i| i.rect).collect();
        // Not egui's own search, which looks in every pane.
        ctx.memory_mut(|mem| mem.move_focus(FocusDirection::None));
        ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key));
        if let Some(to) = step(&rects, from, d) {
            let id = mine[to].id;
            self.x.keys.focus(ctx, id);
        }
    }

    /// Give the keyboard to pane `p`: the open tab, the chosen tool, the picture, the picture
    /// shown in Recent, or else the pane's first control.
    fn enter_pane(&mut self, ctx: &egui::Context, p: Pane) {
        let k = &mut self.x.keys;
        let pick = |scope: Scope| {
            let mut of = k.last.iter().filter(|i| i.scope == scope);
            of.clone().find(|i| i.on).or_else(|| of.next()).map(|i| i.id)
        };
        let id = match p {
            Pane::Ribbon => pick(Scope::TabRow),
            Pane::Tools => pick(Scope::Tools),
            Pane::Recent => pick(Scope::Recent),
            Pane::Canvas => k.canvas,
            Pane::Options => None,
        };
        match id {
            Some(id) => k.focus(ctx, id),
            None => k.enter = Some(p),
        }
    }

    /// A pane is drawn here: remember where, and give it the keyboard when F6 asked for that.
    /// Call at the start of the pane's contents.
    pub(crate) fn pane_here(&mut self, ui: &mut egui::Ui, p: Pane, rect: Rect) {
        self.x.keys.panes_now.push((p, rect));
        if self.x.keys.enter == Some(p) {
            self.x.keys.enter = None;
            // A focus stop that passes the keyboard on to the next control drawn.
            let id = ui.scope_id().with("snagster-pane-start");
            ui.ctx().memory_mut(|m| {
                m.request_focus(id);
                m.move_focus(FocusDirection::Next);
            });
            ui.interact(Rect::from_min_size(ui.cursor().min, Vec2::ZERO), id, Sense::focusable_noninteractive());
        }
    }

    /// The canvas widget drawn this frame.
    pub(crate) fn canvas_here(&mut self, id: Id) {
        self.x.keys.canvas_now = Some(id);
    }

    /// Move the picked mark by `d` picture pixels.
    fn nudge_selected(&mut self, d: Vec2, now: f64) {
        let (Some(ci), Some(si)) = (self.current, self.selected) else { return };
        if si >= self.entries[ci].doc.shapes.len() {
            return;
        }
        // Arrow after arrow on the same mark (a held key) is one step to undo.
        let again = self.x.keys.nudged.is_some_and(|(c, s, at)| c == ci && s == si && now - at < 1.0);
        if !again {
            self.entries[ci].push_undo();
        }
        self.x.keys.nudged = Some((ci, si, now));
        self.entries[ci].doc.shapes[si].translate(d);
    }

    /// After the window is drawn: the key tips and the focus ring.
    pub(crate) fn keys_end(&mut self, ctx: &egui::Context) {
        let items = peek_items();
        let style = tip_style(crate::platform::system());
        let show = match self.x.keys.tips {
            Tips::Off if style == TipStyle::Mnemonics && self.x.keys.alt.held() && !ctx.text_edit_focused() => Some(Scope::TabRow),
            Tips::Off => None,
            Tips::Tabs => Some(Scope::TabRow),
            Tips::Band => Some(Scope::Band),
        };
        // Screen readers can read the tips too (Alt, then the tab's letter, then the tool's):
        // Narrator and NVDA do through UI Automation; AT-SPI on Linux doesn't carry them yet.
        if style != TipStyle::Off {
            let (tabs, tab_tips) = scoped(&items, Scope::TabRow);
            for (it, tip) in tabs.iter().zip(&tab_tips) {
                let key = crate::platform::keys(&format!("Alt+{tip}"));
                ctx.accesskit_node_builder(it.id, |n| n.set_access_key(key));
            }
            if let Some(open) = tabs.iter().position(|i| i.on && i.tab.is_some()) {
                let (band, band_tips) = scoped(&items, Scope::Band);
                for (it, tip) in band.iter().zip(&band_tips) {
                    let key = format!("{}, {tip}", crate::platform::keys(&format!("Alt+{}", tab_tips[open])));
                    ctx.accesskit_node_builder(it.id, |n| n.set_access_key(key));
                }
            }
        }
        if let Some(scope) = show.filter(|_| !egui::Popup::is_any_open(ctx)) {
            let (shown, tips) = scoped(&items, scope);
            let mut painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::unique("snagster-key-tips")));
            // Only over the ribbon: a tool scrolled out of the band shows none.
            if let Some((_, r)) = self.x.keys.panes_now.iter().find(|(p, _)| *p == Pane::Ribbon) {
                painter = painter.with_clip_rect(*r);
            }
            for (it, tip) in shown.iter().zip(&tips) {
                if tip.starts_with(self.x.keys.keys.as_str()) {
                    crate::look::key_tip(&painter, it.rect, tip, it.enabled);
                }
            }
        }
        // The focus ring (text boxes show their own).
        let focused = ctx.memory(|m| m.focused());
        if let Some(id) = focused
            && !ctx.text_edit_focused()
            && let Some(r) = ctx.read_response(id)
            && r.rect.is_positive()
        {
            // Kept inside its pane: a tool half scrolled out doesn't ring over the ribbon.
            let mut painter = ctx.layer_painter(r.layer_id);
            if let Some((_, pane)) = self.x.keys.panes_now.iter().find(|(_, p)| p.contains(r.rect.center())) {
                painter = painter.with_clip_rect(pane.expand(1.0));
            }
            crate::look::focus_ring(&painter, r.rect, self.x.keys.is_canvas(Some(id)));
        }
        self.x.keys.prev_focus = focused;
    }
}

fn scoped(items: &[Item], scope: Scope) -> (Vec<Item>, Vec<String>) {
    let v: Vec<Item> = items.iter().filter(|i| i.scope == scope).cloned().collect();
    let labels: Vec<&str> = v.iter().map(|i| i.label.as_str()).collect();
    let tips = assign(&labels);
    (v, tips)
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod drive;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn unique_and_prefix_free(tips: &[String]) {
        let set: HashSet<&String> = tips.iter().collect();
        assert_eq!(set.len(), tips.len(), "{tips:?}");
        for a in tips {
            assert!(!a.is_empty());
            for b in tips {
                assert!(a == b || !b.starts_with(a.as_str()), "{a} starts {b}");
            }
        }
    }

    #[test]
    fn tabs_get_their_own_first_letters_like_office() {
        let tips = assign(&["File", "Home", "Capture", "Record", "Studio", "Stream", "Edit", "Image", "Share", "View", "Help"]);
        assert_eq!(tips, ["F", "H", "C", "R", "S", "T", "E", "M", "A", "V", "P"]);
        unique_and_prefix_free(&tips);
    }

    #[test]
    fn tips_are_stable_and_follow_the_translated_names() {
        let en = assign(&["File", "Home", "Capture"]);
        assert_eq!(en, assign(&["File", "Home", "Capture"]), "the same names, the same tips");
        // German: Datei, Start, Aufnahme.
        assert_eq!(assign(&["Datei", "Start", "Aufnahme"]), ["D", "S", "A"]);
        // Words' first letters before the rest: "Save as" takes A after S is gone.
        assert_eq!(assign(&["Save", "Save as"]), ["S", "A"]);
    }

    #[test]
    fn names_in_other_scripts_get_free_letters_in_turn() {
        let tips = assign(&["Файл", "Главная", "Захват", "File"]);
        // "File" keeps its F; the Cyrillic names take what is free, in order.
        assert_eq!(tips, ["A", "B", "C", "F"]);
        unique_and_prefix_free(&tips);
        let cjk = assign(&["ファイル", "ホーム"]);
        assert_eq!(cjk, ["A", "B"]);
    }

    #[test]
    fn a_crowded_band_gets_two_key_tips_that_no_single_tip_starts() {
        let names: Vec<String> = (0..60).map(|i| format!("Tool {i}")).collect();
        let labels: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let tips = assign(&labels);
        assert_eq!(tips.len(), 60);
        unique_and_prefix_free(&tips);
        assert!(tips.iter().any(|t| t.len() == 2));
        assert!(tips.iter().all(|t| t.len() <= 2));
        // Up to 36, one key each.
        let few = assign(&labels[..36]);
        assert!(few.iter().all(|t| t.len() == 1), "{few:?}");
        unique_and_prefix_free(&few);
    }

    #[test]
    fn typing_a_tip_matches_it_whole_or_waits_for_more() {
        let tips: Vec<String> = ["A", "B", "Z1", "Z2"].iter().map(|s| s.to_string()).collect();
        assert_eq!(typed(&tips, "B"), Typed::Is(1));
        assert_eq!(typed(&tips, "Z"), Typed::Starts);
        assert_eq!(typed(&tips, "Z2"), Typed::Is(3));
        assert_eq!(typed(&tips, "Q"), Typed::Nothing);
        assert_eq!(typed(&tips, ""), Typed::Nothing);
    }

    #[test]
    fn keys_type_letters_and_digits_only() {
        assert_eq!(tip_char(Key::H), Some('H'));
        assert_eq!(tip_char(Key::Num3), Some('3'));
        assert_eq!(tip_char(Key::Tab), None);
        assert_eq!(tip_char(Key::F6), None);
        assert_eq!(tip_char(Key::Escape), None);
    }

    #[test]
    fn f6_goes_round_the_panes_shown_both_ways() {
        use Pane::*;
        let all = [Ribbon, Tools, Canvas, Options, Recent];
        assert_eq!(next_pane(&all, None, false), Some(Ribbon));
        assert_eq!(next_pane(&all, None, true), Some(Recent));
        let mut at = Some(Ribbon);
        let mut seen = vec![];
        for _ in 0..5 {
            at = next_pane(&all, at, false);
            seen.push(at.unwrap());
        }
        assert_eq!(seen, [Tools, Canvas, Options, Recent, Ribbon]);
        assert_eq!(next_pane(&all, Some(Ribbon), true), Some(Recent));
        assert_eq!(next_pane(&all, Some(Canvas), true), Some(Tools));
        // Options and Recent hidden: they're skipped.
        assert_eq!(next_pane(&[Ribbon, Tools, Canvas], Some(Canvas), false), Some(Ribbon));
        // A pane that isn't shown any more counts as nowhere.
        assert_eq!(next_pane(&[Ribbon, Canvas], Some(Options), false), Some(Ribbon));
        assert_eq!(next_pane(&[], None, false), None);
    }

    #[test]
    fn arrows_move_to_the_nearest_control_that_way_and_stop_at_the_edge() {
        // Two columns of three, like the tool column.
        let r = |x: f32, y: f32| Rect::from_min_size(pos2(x, y), vec2(50.0, 40.0));
        let rects = [r(0.0, 0.0), r(52.0, 0.0), r(0.0, 42.0), r(52.0, 42.0), r(0.0, 84.0), r(52.0, 84.0)];
        assert_eq!(step(&rects, 0, Vec2::RIGHT), Some(1));
        assert_eq!(step(&rects, 0, Vec2::DOWN), Some(2));
        assert_eq!(step(&rects, 3, Vec2::LEFT), Some(2));
        assert_eq!(step(&rects, 3, Vec2::UP), Some(1));
        assert_eq!(step(&rects, 5, Vec2::DOWN), None, "the bottom edge");
        assert_eq!(step(&rects, 0, Vec2::LEFT), None, "the left edge");
        assert_eq!(step(&rects, 0, Vec2::UP), None);
        // A ribbon: a big tool beside a column of small ones goes to the nearest small one.
        let big = Rect::from_min_size(pos2(0.0, 0.0), vec2(60.0, 62.0));
        let s = |y: f32| Rect::from_min_size(pos2(64.0, y), vec2(90.0, 20.0));
        assert_eq!(step(&[big, s(0.0), s(21.0), s(42.0)], 0, Vec2::RIGHT), Some(2));
        assert_eq!(step(&[big, s(0.0), s(21.0), s(42.0)], 3, Vec2::LEFT), Some(0));
    }

    fn mods(alt: bool, ctrl: bool) -> Event {
        Event::ModifiersChanged(egui::Modifiers { alt, ctrl, command: ctrl, ..Default::default() })
    }

    fn key(k: Key) -> Event {
        Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::ALT }
    }

    #[test]
    fn alt_tapped_on_its_own_counts_and_alt_with_anything_does_not() {
        let mut a = AltTap::default();
        assert!(!a.feed(&mods(true, false)));
        assert!(a.held());
        assert!(a.feed(&mods(false, false)), "pressed and let go");
        assert!(!a.held());
        // Alt+H: a tab's letter, not a tap.
        assert!(!a.feed(&mods(true, false)));
        assert!(!a.feed(&key(Key::H)));
        assert!(!a.held());
        assert!(!a.feed(&mods(false, false)));
        // AltGr (Ctrl+Alt) types a character.
        assert!(!a.feed(&mods(true, true)));
        assert!(!a.feed(&mods(false, false)));
        // Alt+Tab into Snagster: the window comes to the front while Alt is down.
        a.feed(&mods(true, false));
        a.feed(&Event::WindowFocused(true));
        assert!(!a.feed(&mods(false, false)));
        // Ctrl let go, Alt still down: still not alone.
        a.feed(&mods(true, true));
        a.feed(&mods(true, false));
        assert!(!a.feed(&mods(false, false)));
    }

    #[test]
    fn each_system_has_its_own_key_tips() {
        use crate::platform::System;
        assert_eq!(tip_style(System::Windows), TipStyle::Office);
        assert_eq!(tip_style(System::Gnome), TipStyle::Mnemonics);
        assert_eq!(tip_style(System::Kde), TipStyle::Mnemonics);
        assert_eq!(tip_style(System::OtherLinux), TipStyle::Mnemonics);
        assert_eq!(tip_style(System::Mac), TipStyle::Off);
    }
}
