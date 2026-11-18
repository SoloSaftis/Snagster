//! The markup tools driven the way a person uses them: mouse presses, drags, releases and keys
//! go through egui into the real editor canvas, frame by frame. No screen is needed (egui runs
//! headless), so this runs with `cargo test` on every system and in CI.
//!
//! Each test says what a person does in the editor and checks the marks that come out. These
//! catch what calling `finish_draw` directly can't: where a drag really starts, which presses
//! take a handle, and what the keys do in a text box.

use super::*;

/// The picture the tests mark up, and the window it is shown in.
const PIC: (u32, u32) = (400, 300);
const WINDOW: Vec2 = vec2(1000.0, 700.0);

/// A person at the editor: a window with one picture shown at 100 %, a mouse and a keyboard.
struct Desk {
    app: SnagApp,
    ctx: egui::Context,
    time: f64,
    mods: Modifiers,
    /// Where the pointer is, in window points.
    at: Pos2,
    /// The canvas area as the last frame laid it out.
    area: Rect,
    /// The pointer shape the last frame asked for.
    cursor: egui::CursorIcon,
}

impl Desk {
    fn new(tool: Tool) -> Desk {
        let mut app = SnagApp::blank(Settings::default(), false, None);
        app.add_entry("Test".into(), Doc::new(RgbaImage::from_pixel(PIC.0, PIC.1, image::Rgba([255, 255, 255, 255]))));
        app.zoom = Some(1.0);
        app.tool = tool;
        let mut d = Desk { app, ctx: egui::Context::default(), time: 0.0, mods: Modifiers::NONE, at: pos2(5.0, 5.0), area: Rect::NOTHING, cursor: egui::CursorIcon::Default };
        d.frame(Vec::new());
        d
    }

    /// One frame with these events.
    fn frame(&mut self, mut events: Vec<egui::Event>) {
        self.time += 1.0 / 60.0;
        events.insert(0, egui::Event::ModifiersChanged(self.mods));
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, WINDOW)),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let (app, area) = (&mut self.app, &mut self.area);
        let mut out = self.ctx.run_ui(raw, |ui| {
            *area = ui.max_rect();
            app.canvas(ui);
        });
        // No GPU here: the pictures the frame would upload are dropped.
        out.textures_delta.clear();
        self.cursor = out.platform_output.cursor_icon;
    }

    /// A picture position as a window position (the canvas centres the picture at 100 %).
    fn on_screen(&self, p: Pos2) -> Pos2 {
        let size = vec2(PIC.0 as f32, PIC.1 as f32);
        image_to_screen(p, canvas_origin(self.area.center(), size, 1.0, self.app.pan), 1.0)
    }

    fn move_to(&mut self, p: Pos2) {
        self.at = self.on_screen(p);
        self.frame(vec![egui::Event::PointerMoved(self.at)]);
    }

    fn button(&mut self, pressed: bool) {
        let e = egui::Event::PointerButton { pos: self.at, button: PointerButton::Primary, pressed, modifiers: self.mods };
        self.frame(vec![e]);
    }

    /// Press at `from`, move to `to` in a few steps as a hand does, and let go.
    fn drag(&mut self, from: Pos2, to: Pos2) {
        self.drag_via(from, &[to]);
    }

    /// Press at `from`, pass through `via` (the last is where the button is let go).
    fn drag_via(&mut self, from: Pos2, via: &[Pos2]) {
        self.move_to(from);
        self.button(true);
        let mut last = from;
        for &p in via {
            for i in 1..=6 {
                self.move_to(last + (p - last) * (i as f32 / 6.0));
            }
            last = p;
        }
        self.button(false);
        self.frame(Vec::new());
    }

    fn click(&mut self, p: Pos2) {
        self.move_to(p);
        self.button(true);
        self.button(false);
        self.frame(Vec::new());
    }

    fn key(&mut self, key: Key, modifiers: Modifiers) {
        let old = self.mods;
        self.mods = modifiers;
        self.frame(vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }]);
        self.frame(vec![egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers }]);
        self.mods = old;
    }

    fn type_text(&mut self, s: &str) {
        self.frame(vec![egui::Event::Text(s.into())]);
    }

    /// Hold a modifier key for what `f` does.
    fn holding(&mut self, m: Modifiers, f: impl FnOnce(&mut Desk)) {
        self.mods = m;
        f(self);
        self.mods = Modifiers::NONE;
    }

    fn marks(&self) -> &[Shape] {
        &self.app.entries[0].doc.shapes
    }

    fn undo_steps(&self) -> usize {
        self.app.entries[0].undo.len()
    }
}

/// Positions in these tests are whole pixels; the canvas maps them exactly at 100 %.
fn near(a: Pos2, b: Pos2) -> bool {
    a.distance(b) < 0.01
}

fn r(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    Rect::from_min_max(pos2(x0, y0), pos2(x1, y1))
}

#[test]
fn every_drawing_tool_starts_its_mark_where_the_button_went_down() {
    // egui notices a drag only after a few pixels; the mark must still start at the press.
    let (from, to) = (pos2(100.0, 80.0), pos2(220.0, 170.0));
    let mut d = Desk::new(Tool::Arrow);
    d.drag(from, to);
    let [Shape::Arrow { a, b, .. }] = d.marks() else { panic!("{:?}", d.marks()) };
    assert!(near(*a, from) && near(*b, to), "arrow {a:?} -> {b:?}");

    let mut d = Desk::new(Tool::Line);
    d.drag(from, to);
    let [Shape::Line { a, b, .. }] = d.marks() else { panic!("{:?}", d.marks()) };
    assert!(near(*a, from) && near(*b, to), "line {a:?} -> {b:?}");

    for tool in [Tool::Rect, Tool::Ellipse, Tool::Highlight, Tool::Spotlight] {
        let mut d = Desk::new(tool);
        d.drag(from, to);
        assert_eq!(d.marks().len(), 1, "{tool:?}");
        let b = d.marks()[0].boxed().unwrap_or_else(|| panic!("{tool:?} made {:?}", d.marks()[0]));
        assert!(near(b.min, from) && near(b.max, to), "{tool:?} box {b:?}");
    }

    let mut d = Desk::new(Tool::Magnify);
    d.drag(from, pos2(130.0, 80.0));
    let [Shape::Magnify { center, radius, .. }] = d.marks() else { panic!("{:?}", d.marks()) };
    assert!(near(*center, from) && (radius - 30.0).abs() < 0.01, "magnifier at {center:?}, {radius}");

    let mut d = Desk::new(Tool::Pen);
    d.drag(from, to);
    let [Shape::Pen { points, .. }] = d.marks() else { panic!("{:?}", d.marks()) };
    assert!(near(points[0], from), "the stroke starts at the press, not at {:?}", points[0]);
}

#[test]
fn a_drag_from_right_to_left_or_upwards_draws_the_same_box() {
    let mut d = Desk::new(Tool::Rect);
    d.drag(pos2(220.0, 170.0), pos2(100.0, 80.0));
    assert_eq!(d.marks()[0].boxed(), Some(r(100.0, 80.0, 220.0, 170.0)));
}

#[test]
fn a_click_with_a_drawing_tool_adds_nothing() {
    for tool in [Tool::Arrow, Tool::Line, Tool::Rect, Tool::Ellipse, Tool::Highlight, Tool::Spotlight] {
        let mut d = Desk::new(tool);
        d.click(pos2(100.0, 100.0));
        assert!(d.marks().is_empty(), "{tool:?} made {:?}", d.marks());
        assert_eq!(d.undo_steps(), 0, "{tool:?}");
    }
}

#[test]
fn an_arrow_is_reshaped_by_its_ends_without_leaving_the_arrow_tool() {
    let mut d = Desk::new(Tool::Arrow);
    d.drag(pos2(50.0, 50.0), pos2(200.0, 150.0));
    assert_eq!(d.app.selected, Some(0), "the new arrow is picked");
    // Drag its point somewhere else: the same arrow changes, no second arrow appears.
    d.drag(pos2(201.0, 149.0), pos2(300.0, 60.0));
    assert_eq!(d.marks().len(), 1, "{:?}", d.marks());
    let [Shape::Arrow { a, b, .. }] = d.marks() else { panic!() };
    assert!(near(*a, pos2(50.0, 50.0)) && near(*b, pos2(300.0, 60.0)), "{a:?} -> {b:?}");
    // And its tail.
    d.drag(pos2(50.0, 50.0), pos2(20.0, 250.0));
    let [Shape::Arrow { a, b, .. }] = d.marks() else { panic!() };
    assert!(near(*a, pos2(20.0, 250.0)) && near(*b, pos2(300.0, 60.0)), "{a:?} -> {b:?}");
    // Each reshape is one undo step.
    assert_eq!(d.undo_steps(), 3);
    d.app.undo();
    let [Shape::Arrow { a, .. }] = d.marks() else { panic!() };
    assert!(near(*a, pos2(50.0, 50.0)));
}

#[test]
fn shift_snaps_a_dragged_arrow_end_to_45_degrees() {
    let mut d = Desk::new(Tool::Arrow);
    d.drag(pos2(50.0, 50.0), pos2(200.0, 60.0));
    d.holding(Modifiers::SHIFT, |d| d.drag(pos2(200.0, 60.0), pos2(250.0, 58.0)));
    let [Shape::Arrow { b, .. }] = d.marks() else { panic!() };
    assert!((b.y - 50.0).abs() < 0.01, "level with the tail: {b:?}");
}

#[test]
fn an_older_arrow_is_picked_again_with_a_click_and_then_adjusted() {
    let mut d = Desk::new(Tool::Arrow);
    d.drag(pos2(50.0, 50.0), pos2(200.0, 50.0));
    d.drag(pos2(50.0, 200.0), pos2(200.0, 200.0));
    assert_eq!(d.app.selected, Some(1));
    // A press on the first arrow's point picks it up (it's not the picked one, so its handle
    // isn't live yet): a click picks it, then its point can be dragged.
    d.click(pos2(120.0, 50.0));
    assert_eq!(d.app.selected, Some(0));
    d.drag(pos2(200.0, 50.0), pos2(260.0, 120.0));
    let Shape::Arrow { b, .. } = &d.marks()[0] else { panic!() };
    assert!(near(*b, pos2(260.0, 120.0)), "{b:?}");
    assert_eq!(d.marks().len(), 2);
}

#[test]
fn pressing_on_an_arrow_body_with_the_arrow_tool_moves_it() {
    let mut d = Desk::new(Tool::Arrow);
    d.drag(pos2(50.0, 50.0), pos2(200.0, 50.0));
    d.drag(pos2(120.0, 50.0), pos2(140.0, 90.0));
    assert_eq!(d.marks().len(), 1, "moved, not a new arrow");
    let [Shape::Arrow { a, b, .. }] = d.marks() else { panic!() };
    assert!(near(*a, pos2(70.0, 90.0)) && near(*b, pos2(220.0, 90.0)), "{a:?} -> {b:?}");
}

#[test]
fn a_box_is_resized_by_its_corners_and_sides_and_moved_by_its_edge() {
    let mut d = Desk::new(Tool::Rect);
    d.drag(pos2(100.0, 100.0), pos2(200.0, 160.0));
    // Bottom-right corner.
    d.drag(pos2(200.0, 160.0), pos2(260.0, 200.0));
    assert_eq!(d.marks()[0].boxed(), Some(r(100.0, 100.0, 260.0, 200.0)));
    // Left side: only that edge moves.
    d.drag(pos2(100.0, 150.0), pos2(60.0, 10.0));
    assert_eq!(d.marks()[0].boxed(), Some(r(60.0, 100.0, 260.0, 200.0)));
    // Shift on a corner keeps it square.
    d.holding(Modifiers::SHIFT, |d| d.drag(pos2(260.0, 200.0), pos2(300.0, 210.0)));
    let b = d.marks()[0].boxed().unwrap();
    assert!((b.width() - b.height()).abs() < 0.01, "{b:?}");
    // The outline (not a handle) moves the whole box.
    let before = d.marks()[0].boxed().unwrap();
    d.drag(pos2(before.min.x, before.center().y + 20.0), pos2(before.min.x + 10.0, before.center().y + 25.0));
    assert_eq!(d.marks()[0].boxed(), Some(before.translate(vec2(10.0, 5.0))));
    assert_eq!(d.marks().len(), 1);
}

#[test]
fn a_new_box_can_be_drawn_inside_another_one() {
    // Inside an outlined box is not on it: a new box starts there.
    let mut d = Desk::new(Tool::Rect);
    d.drag(pos2(50.0, 50.0), pos2(300.0, 250.0));
    d.drag(pos2(120.0, 120.0), pos2(180.0, 170.0));
    assert_eq!(d.marks().len(), 2);
    assert_eq!(d.marks()[1].boxed(), Some(r(120.0, 120.0, 180.0, 170.0)));
}

#[test]
fn alt_draws_a_box_around_where_the_button_went_down() {
    let mut d = Desk::new(Tool::Ellipse);
    d.holding(Modifiers::ALT, |d| d.drag(pos2(150.0, 150.0), pos2(190.0, 170.0)));
    assert_eq!(d.marks()[0].boxed(), Some(r(110.0, 130.0, 190.0, 170.0)));
}

#[test]
fn shift_draws_squares_and_level_arrows() {
    let mut d = Desk::new(Tool::Rect);
    d.holding(Modifiers::SHIFT, |d| d.drag(pos2(50.0, 50.0), pos2(150.0, 90.0)));
    let b = d.marks()[0].boxed().unwrap();
    assert_eq!(b.width(), b.height());
    let mut d = Desk::new(Tool::Arrow);
    d.holding(Modifiers::SHIFT, |d| d.drag(pos2(50.0, 50.0), pos2(150.0, 57.0)));
    let [Shape::Arrow { b, .. }] = d.marks() else { panic!() };
    assert_eq!(b.y, 50.0);
}

#[test]
fn the_highlighter_marks_text_with_a_see_through_box_and_can_draw_freehand() {
    let mut d = Desk::new(Tool::Highlight);
    d.app.settings.color = [255, 220, 0];
    d.drag(pos2(40.0, 100.0), pos2(240.0, 118.0));
    let [Shape::Marker { rect, .. }] = d.marks() else { panic!("{:?}", d.marks()) };
    assert_eq!(*rect, r(40.0, 100.0, 240.0, 118.0));
    // On the picture it is yellow and see-through: white under it stays light, nothing outside.
    let out = render::compose(&d.app.entries[0].doc);
    let inside = out.get_pixel(140, 109);
    assert!(inside[0] > 200 && inside[1] > 180 && inside[2] < 200, "{inside:?}");
    assert_eq!(out.get_pixel(140, 130).0, [255, 255, 255, 255]);
    // Its corner reshapes it with the highlighter still chosen.
    d.drag(pos2(240.0, 118.0), pos2(300.0, 122.0));
    assert_eq!(d.marks()[0].boxed(), Some(r(40.0, 100.0, 300.0, 122.0)));
    // Freehand: a stroke along the path.
    d.app.settings.highlight_box = false;
    d.drag_via(pos2(40.0, 200.0), &[pos2(120.0, 210.0), pos2(200.0, 200.0)]);
    let Shape::Highlight { points, .. } = &d.marks()[1] else { panic!("{:?}", d.marks()[1]) };
    assert!(near(points[0], pos2(40.0, 200.0)) && points.len() > 3);
}

#[test]
fn the_selection_tool_moves_and_reshapes_any_mark() {
    let mut d = Desk::new(Tool::Arrow);
    d.drag(pos2(50.0, 50.0), pos2(150.0, 50.0));
    d.app.tool = Tool::Rect;
    d.drag(pos2(50.0, 150.0), pos2(150.0, 250.0));
    d.app.tool = Tool::Select;
    d.app.selected = None;
    d.frame(Vec::new());
    d.drag(pos2(100.0, 50.0), pos2(100.0, 70.0));
    let Shape::Arrow { a, .. } = &d.marks()[0] else { panic!() };
    assert!(near(*a, pos2(50.0, 70.0)), "moved: {a:?}");
    d.click(pos2(50.0, 200.0));
    assert_eq!(d.app.selected, Some(1));
    d.drag(pos2(150.0, 250.0), pos2(180.0, 280.0));
    assert_eq!(d.marks()[1].boxed(), Some(r(50.0, 150.0, 180.0, 280.0)));
}

#[test]
fn shift_enter_starts_a_new_line_and_enter_finishes_the_text() {
    let mut d = Desk::new(Tool::Text);
    d.click(pos2(60.0, 60.0));
    assert!(d.app.text_edit.is_some(), "a click opens a text box");
    d.frame(Vec::new());
    d.type_text("First");
    d.key(Key::Enter, Modifiers::SHIFT);
    assert!(d.app.text_edit.is_some(), "Shift+Enter keeps the box open");
    d.type_text("Second");
    d.key(Key::Enter, Modifiers::NONE);
    assert!(d.app.text_edit.is_none(), "Enter finishes");
    let [Shape::Text { text, pos, .. }] = d.marks() else { panic!("{:?}", d.marks()) };
    assert_eq!(text, "First\nSecond");
    assert!(near(*pos, pos2(60.0, 60.0)), "{pos:?}");
}

#[test]
fn clicking_outside_the_text_box_keeps_the_text() {
    let mut d = Desk::new(Tool::Text);
    d.click(pos2(60.0, 60.0));
    d.frame(Vec::new());
    d.type_text("Kept");
    d.click(pos2(300.0, 250.0));
    let Some(Shape::Text { text, .. }) = d.marks().first() else { panic!("{:?}", d.marks()) };
    assert_eq!(text, "Kept");
}

#[test]
fn delete_removes_the_mark_just_drawn_and_undo_brings_it_back() {
    let mut d = Desk::new(Tool::Arrow);
    d.drag(pos2(50.0, 50.0), pos2(150.0, 50.0));
    d.app.delete_selected();
    assert!(d.marks().is_empty());
    d.app.undo();
    assert_eq!(d.marks().len(), 1);
}

#[test]
fn the_pointer_shows_what_a_press_will_do() {
    let mut d = Desk::new(Tool::Rect);
    d.drag(pos2(100.0, 100.0), pos2(200.0, 160.0));
    let cursor = |d: &mut Desk, p: Pos2| {
        d.move_to(p);
        d.cursor
    };
    assert_eq!(cursor(&mut d, pos2(200.0, 160.0)), egui::CursorIcon::ResizeNwSe, "a corner handle");
    assert_eq!(cursor(&mut d, pos2(150.0, 100.0)), egui::CursorIcon::ResizeVertical, "the top side");
    assert_eq!(cursor(&mut d, pos2(100.0, 130.0)), egui::CursorIcon::ResizeHorizontal, "the left side");
    assert_eq!(cursor(&mut d, pos2(130.0, 100.0)), egui::CursorIcon::Move, "the outline moves it");
    assert_eq!(cursor(&mut d, pos2(300.0, 250.0)), egui::CursorIcon::Crosshair, "elsewhere draws");
}
