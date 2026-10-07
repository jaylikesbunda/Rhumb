use super::*;
use std::fs;
use std::time::{Duration, Instant, SystemTime};

pub struct App {
    ctx: Context,
    app: Rhumb,
    time: f64,
    /// How long the last frame asked to wait before the next one.
    repaint_in: Duration,
}

impl App {
    /// A fresh app looking at `dir`, with the window the size of the real one.
    pub fn new(dir: &Path) -> App {
        // A preferences file that does not exist, so the app starts as it does for
        // someone who has never run it, whatever is saved on this machine.
        let nowhere = std::env::temp_dir()
            .join(format!("rhumb-test-prefs-{}", std::process::id()))
            .join("prefs.txt");
        PREFS_OVERRIDE.with(|p| *p.borrow_mut() = Some(nowhere));
        let ctx = Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = Rhumb::new(&cc);
        app.navigate(dir);
        let mut a = App {
            ctx,
            app,
            time: 0.0,
            repaint_in: Duration::MAX,
        };
        // Until the listing has arrived from its worker.
        a.settle(|a| !a.app.entries.is_empty());
        a
    }

    /// One frame with `events`, and how long it took.
    pub fn frame_with(&mut self, events: Vec<egui::Event>) -> Duration {
        self.time += 1.0 / 60.0;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1160.0, 720.0))),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let start = Instant::now();
        let out = self.ctx.run_ui(input, |ui| {
            let ctx = ui.ctx().clone();
            self.app.draw(ui, &ctx);
        });
        // What a window does next with what was drawn, which is part of the frame a
        // person waits for and is not part of drawing it.
        let _ = self
            .ctx
            .tessellate(out.shapes.clone(), out.pixels_per_point);
        let took = start.elapsed();
        self.repaint_in = out
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map_or(Duration::MAX, |v| v.repaint_delay);
        out.drop_without_applying_deltas();
        took
    }

    pub fn frame(&mut self) -> Duration {
        self.frame_with(Vec::new())
    }

    /// Runs frames until `done` holds, letting the worker threads catch up.
    pub fn settle(&mut self, done: impl Fn(&App) -> bool) {
        for _ in 0..1000 {
            if done(self) {
                return;
            }
            self.frame();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Opens `path` in the editor and gives it the keyboard.
    pub fn open_and_focus(&mut self, path: &Path) {
        self.app.open_path(path);
        self.settle(|a| a.app.loading.is_none() && a.app.doc().is_some());
        self.frame();
        self.ctx
            .memory_mut(|m| m.request_focus(Id::new(codeedit::ID)));
        self.frame();
        self.frame();
    }

    /// Types one character, as a keypress would deliver it.
    pub fn type_char(&mut self, ch: &str) -> Duration {
        self.frame_with(vec![egui::Event::Text(ch.to_owned())])
    }

    /// How far the editor is scrolled, in points.
    pub fn scrolled(&self) -> f32 {
        self.app.ed.scroll_offset()
    }

    /// Puts the pointer over the editor, where the wheel will reach it.
    pub fn hover_editor(&mut self) {
        self.frame_with(vec![egui::Event::PointerMoved(Pos2::new(900.0, 400.0))]);
    }

    /// One wheel notch, `notches` lines' worth, as a mouse wheel delivers it.
    pub fn wheel(&mut self, notches: f32) -> Duration {
        let phase = |p| egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: Vec2::new(0.0, notches),
            phase: p,
            modifiers: egui::Modifiers::default(),
        };
        // Only `Move`, which is all a mouse wheel on Windows ever sends. `Start`
        // marks a touchpad gesture, which egui applies at once without smoothing.
        self.frame_with(vec![phase(egui::TouchPhase::Move)])
    }

    #[allow(dead_code)]
    pub fn text(&self) -> String {
        self.app.doc().map(|d| d.text.to_text()).unwrap_or_default()
    }

    /// Where the caret is, as a character index into the text.
    pub fn caret(&self) -> usize {
        self.app.ed.caret_index()
    }

    /// A click: the pointer arrives, presses and lets go, on separate frames, as a
    /// hand does it.
    pub fn click_at(&mut self, pos: Pos2) {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        self.frame_with(vec![egui::Event::PointerMoved(pos)]);
        self.frame_with(vec![button(true)]);
        self.frame_with(vec![button(false)]);
        self.frame();
    }
}

/// A folder with `files` small files and one long source file to edit.
pub fn workspace(name: &str, files: usize, lines: usize) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for i in 0..files {
        fs::write(dir.join(format!("file_{i:05}.txt")), "x").unwrap();
    }
    let src = dir.join("main.rs");
    let line = "    let value = compute(alpha, beta) + gamma; // a plausible line of code\n";
    fs::write(&src, line.repeat(lines)).unwrap();
    (dir, src)
}

fn percentiles(mut ms: Vec<f64>) -> (f64, f64, f64) {
    ms.sort_by(|a, b| a.total_cmp(b));
    (ms[ms.len() / 2], ms[ms.len() * 99 / 100], ms[ms.len() - 1])
}

#[test]
fn typing_into_an_opened_file_reaches_the_buffer_and_marks_it_modified() {
    let (dir, src) = workspace("rhumb-app-typing", 5, 50);
    let mut a = App::new(&dir);
    a.open_and_focus(&src);
    assert!(
        a.app.doc().is_some_and(|d| !d.dirty()),
        "opening it modified nothing"
    );
    let before = a.text().chars().count();
    for ch in ["h", "i", "!"] {
        a.type_char(ch);
    }
    assert!(
        a.text().contains("hi!"),
        "the keystrokes went into the file"
    );
    assert_eq!(a.text().chars().count(), before + 3);
    assert!(
        a.app.doc().is_some_and(|d| d.dirty()),
        "and it now counts as modified"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The target: 240 frames a second, which is 4.2 ms for a whole frame - the
/// window's own chrome, the sidebar, the file list and the editor together, and
/// the tessellation after them - with the graphics card still needing its share.
const FRAME_240_MS: f64 = 4.2;

/// Times `n` calls of `f`, in milliseconds, as `(median, worst)`, and holds them to
/// the 240 fps budget: the median must be under `median_ms`, always, and the worst
/// under a frame at 240 fps when `RHUMB_STRICT_SPEED` is set, for a run on a quiet
/// machine. Otherwise the worst only has to be short of a freeze, since the whole
/// test suite shares the CPU and another test can take a frame from this one.
#[track_caller]
fn at_240fps(what: &str, n: usize, median_ms: f64, mut f: impl FnMut() -> Duration) {
    let mut best: Option<(f64, f64)> = None;
    for _ in 0..3 {
        let mut ms: Vec<f64> = (0..n).map(|_| f().as_secs_f64() * 1000.0).collect();
        ms.sort_by(|a, b| a.total_cmp(b));
        let pair = (ms[ms.len() / 2], ms[ms.len() - 1]);
        best = Some(match best {
            None => pair,
            Some(b) => (b.0.min(pair.0), b.1.min(pair.1)),
        });
    }
    let (median, worst) = best.unwrap_or((0.0, 0.0));
    eprintln!(
        "  {what:44} median {median:6.3} ms  worst {worst:6.3} ms   (frame at 240 fps: {FRAME_240_MS} ms)"
    );
    let strict = std::env::var_os("RHUMB_STRICT_SPEED").is_some();
    // A shared CI runner is slow and noisy; the absolute budgets were written
    // for a quiet desktop. Unless a strict run is asked for, a CI build is
    // treated like a debug one: loose enough not to fail on VM noise, tight
    // enough that an O(document) regression still blows the budget.
    let ci = std::env::var_os("CI").is_some();
    let slow = if cfg!(debug_assertions) || (ci && !strict) {
        30.0
    } else {
        1.0
    };
    let worst_budget = if strict {
        FRAME_240_MS * slow
    } else {
        60.0 * slow
    };
    assert!(
        median <= median_ms * slow && worst <= worst_budget,
        "{what}: median {median:.2} ms (budget {:.1}), worst {worst:.2} ms (budget {worst_budget:.1})",
        median_ms * slow
    );
}

#[test]
fn typing_in_the_whole_app_fits_in_a_frame_at_240_fps() {
    let (dir, src) = workspace("rhumb-app-240-typing", 3_000, 20_000);
    let mut a = App::new(&dir);
    a.open_and_focus(&src);
    for _ in 0..10 {
        a.frame();
    }
    at_240fps(
        "a typed character, whole app, 20,000 lines",
        60,
        2.0,
        || a.type_char("x"),
    );
    at_240fps("a line break, whole app", 30, 2.0, || a.type_char("\n"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn scrolling_the_whole_app_fits_in_a_frame_at_240_fps() {
    let (dir, src) = workspace("rhumb-app-240-scroll", 3_000, 20_000);
    let mut a = App::new(&dir);
    a.open_and_focus(&src);
    a.hover_editor();
    for _ in 0..10 {
        a.frame();
    }
    at_240fps("a frame of scrolling the editor", 90, 2.0, || a.wheel(-0.3));
    at_240fps("an idle frame", 60, 1.5, || a.frame());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_frame_of_the_whole_app_with_a_big_folder_and_a_file_open_stays_cheap() {
    let (dir, src) = workspace("rhumb-app-frames", 10_000, 2_000);
    let mut a = App::new(&dir);
    a.open_and_focus(&src);
    for _ in 0..10 {
        a.frame();
    }
    at_240fps(
        "a typed character with 10,000 files listed",
        60,
        2.0,
        || a.type_char("x"),
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A markdown document of `sections` sections, each with a heading, paragraphs,
/// a list, a table, a fenced code block and a quote, so every kind of block the
/// preview draws is in it many times over.
pub fn markdown_workspace(name: &str, sections: usize) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let mut doc = String::from("# A large document\n\n");
    for i in 0..sections {
        doc.push_str(&format!(
            "## Section {i}\n\nSome *emphasised* and **strong** text with `inline code` and a \
             [link](https://example.com/{i}), going on long enough to wrap onto a second and \
             third line of the preview so that wrapping is exercised as well as paragraphs.\n\n\
             - first item {i}\n- second item with **bold**\n  - a nested item\n- third item\n\n\
             | name | value |\n|------|-------|\n| a{i} | {i} |\n| b | two |\n\n\
             ```rust\n// a comment\nfn section_{i}() -> u32 {{\n    let s = \"text\";\n    {i}\n}}\n```\n\n\
             > A quotation in section {i}.\n\n"
        ));
    }
    let md = dir.join("big.md");
    fs::write(&md, doc).unwrap();
    (dir, md)
}

fn press(a: &mut App, key: egui::Key, modifiers: egui::Modifiers) {
    for pressed in [true, false] {
        a.frame_with(vec![egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers,
        }]);
    }
}

#[test]
fn ctrl_comma_opens_the_settings_and_again_closes_them() {
    let (dir, _) = workspace("rhumb-settings-key", 5, 10);
    let mut a = App::new(&dir);
    a.frame();
    press(&mut a, egui::Key::Comma, egui::Modifiers::CTRL);
    assert!(matches!(a.app.dialog, Dialog::Settings { .. }));
    for _ in 0..3 {
        a.frame();
    }
    assert!(
        matches!(a.app.dialog, Dialog::Settings { .. }),
        "and it stays up"
    );
    press(&mut a, egui::Key::Comma, egui::Modifiers::CTRL);
    assert!(matches!(a.app.dialog, Dialog::None));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_settings_window_draws_every_section_without_trouble() {
    let (dir, _) = workspace("rhumb-settings-draw", 5, 10);
    let mut a = App::new(&dir);
    for section in 0..3 {
        a.app.dialog = Dialog::Settings { section };
        for _ in 0..3 {
            a.frame();
        }
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_sidebar_starts_wide_enough_to_read_a_drive_and_what_is_free_on_it() {
    let (dir, _) = workspace("rhumb-sidebar-width", 5, 10);
    let _ = App::new(&dir);
    const { assert!(SIDEBAR_DEFAULT >= 240.0) };
    // A width saved from before the default moved is not a choice, and follows it.
    let mut x = App::new(&dir);
    x.app.apply_prefs_text(
        "sidebar_w=224
",
        None,
    );
    assert_eq!(x.app.sidebar_w, SIDEBAR_DEFAULT);
    x.app.apply_prefs_text(
        "sidebar_w=300
",
        None,
    );
    assert_eq!(x.app.sidebar_w, 300.0, "one that was dragged is kept");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_pinned_folder_can_be_unpinned_from_the_menu_wherever_the_menu_was_opened() {
    let (dir, _) = workspace("rhumb-unpin", 5, 10);
    let mut a = App::new(&dir);
    a.frame();
    assert!(a.app.pin(&dir));
    a.frame();
    // The menu is asked for from the sidebar, with the list not on screen at all.
    let anchor = Some(Pos2::new(100.0, 100.0));
    a.app.menu.open(anchor, dir.clone());
    for _ in 0..4 {
        a.frame();
    }
    assert_eq!(
        a.app.menu.path(),
        Some(dir.as_path()),
        "the menu is still up, and being drawn"
    );
    a.app.unpin(&dir);
    assert!(!a.app.is_pinned(&dir));
    let _ = fs::remove_dir_all(&dir);
}

/// The editor and the preview are two panes over the same file, and a lock between
/// them makes one follow the other's scrolling.
fn scroll_lock_setup(name: &str) -> (App, PathBuf, Pos2, Pos2) {
    let (dir, md) = markdown_workspace(name, 200);
    let mut a = App::new(&dir);
    a.open_and_focus(&md);
    // Until the preview has been parsed on its worker and has a length to scroll,
    // however long a busy machine takes to get there.
    for _ in 0..1000 {
        a.frame();
        std::thread::sleep(Duration::from_millis(5));
        if a.app.preview_range > 0.0 {
            break;
        }
    }
    for _ in 0..10 {
        a.frame();
    }
    let preview = a.app.preview_rect;
    assert!(preview.width() > 50.0, "the preview is on screen");
    let over_preview = preview.center();
    let over_editor = Pos2::new(preview.left() - 150.0, preview.center().y);
    (a, dir, over_editor, over_preview)
}

fn wheel_at(a: &mut App, at: Pos2, notches: f32, frames: usize) {
    a.frame_with(vec![egui::Event::PointerMoved(at)]);
    for _ in 0..frames {
        a.wheel(notches);
    }
    for _ in 0..30 {
        a.frame();
    }
}

/// How many lines apart what the editor shows and what the preview shows are.
fn lines_apart(a: &App) -> f32 {
    (a.app.ed.scroll_line() - a.app.preview.line_at_y(a.app.preview_off)).abs()
}

#[test]
fn scrolling_the_editor_scrolls_the_preview_with_it_while_they_are_locked() {
    let (mut a, dir, editor, _) = scroll_lock_setup("rhumb-lock-editor");
    assert!(a.app.sync_scroll, "locked by default");
    wheel_at(&mut a, editor, -3.0, 40);
    assert!(a.app.ed.scroll_line() > 20.0, "the editor moved");
    assert!(a.app.preview_off > 100.0, "and the preview with it");
    assert!(lines_apart(&a) < 2.0, "{} lines apart", lines_apart(&a));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn scrolling_the_preview_scrolls_the_editor_with_it_while_they_are_locked() {
    let (mut a, dir, _, preview) = scroll_lock_setup("rhumb-lock-preview");
    wheel_at(&mut a, preview, -3.0, 120);
    assert!(a.app.preview_off > 100.0, "the preview moved");
    assert!(a.app.ed.scroll_line() > 5.0, "and the editor with it");
    assert!(lines_apart(&a) < 2.0, "{} lines apart", lines_apart(&a));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn unlocked_the_two_panes_scroll_on_their_own() {
    let (mut a, dir, editor, preview) = scroll_lock_setup("rhumb-lock-off");
    a.app.sync_scroll = false;
    wheel_at(&mut a, editor, -3.0, 40);
    assert!(a.app.ed.scroll_fraction() > 0.05);
    assert!(a.app.preview_off < 1.0, "the preview stayed at the top");
    wheel_at(&mut a, preview, -3.0, 20);
    assert!(a.app.preview_off > 1.0);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn locking_again_brings_the_preview_to_where_the_editor_is() {
    let (mut a, dir, editor, _) = scroll_lock_setup("rhumb-lock-again");
    a.app.sync_scroll = false;
    wheel_at(&mut a, editor, -3.0, 40);
    assert!(a.app.preview_off < 1.0);
    a.app.sync_scroll = true;
    a.app.align_preview_to_editor();
    for _ in 0..10 {
        a.frame();
    }
    assert!(a.app.preview_off > 100.0);
    assert!(lines_apart(&a) < 2.0, "{} lines apart", lines_apart(&a));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_end_of_the_editor_is_the_end_of_the_preview_and_the_other_way_round() {
    let (mut a, dir, _, preview) = scroll_lock_setup("rhumb-lock-ends");
    a.app.ed.set_scroll_fraction(1.0);
    for _ in 0..20 {
        a.frame();
    }
    assert!(
        a.app.preview_off >= a.app.preview_range - 2.0,
        "{} of {}",
        a.app.preview_off,
        a.app.preview_range
    );
    wheel_at(&mut a, preview, 6.0, 1500);
    assert!(a.app.preview_off < 5.0, "back at the top");
    assert!(a.app.ed.scroll_line() < 3.0);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn nothing_drifts_when_nothing_is_touched() {
    let (mut a, dir, editor, _) = scroll_lock_setup("rhumb-lock-still");
    wheel_at(&mut a, editor, -3.0, 30);
    // Until the wheel has finished easing to a stop.
    for _ in 0..400 {
        a.frame();
    }
    let (ed, pv) = (a.app.ed.scroll_y_for_test(), a.app.preview_off);
    for _ in 0..200 {
        a.frame();
    }
    assert!(
        (a.app.ed.scroll_y_for_test() - ed).abs() < 0.01,
        "the editor drifted"
    );
    assert!((a.app.preview_off - pv).abs() < 0.01, "the preview drifted");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_editor_scrolls_exactly_as_it_does_alone_whatever_the_preview_is_doing() {
    // The preview is being measured as the editor drives it, and the editor must not be
    // disturbed by that: its position per frame is the same as with the lock off.
    let mut runs: Vec<Vec<f32>> = Vec::new();
    for locked in [true, false] {
        let (mut a, dir, editor, _) = scroll_lock_setup(&format!("rhumb-lock-alone-{locked}"));
        a.app.sync_scroll = locked;
        a.frame_with(vec![egui::Event::PointerMoved(editor)]);
        let mut ys = Vec::new();
        for _ in 0..150 {
            a.wheel(-1.0);
            ys.push(a.app.ed.scroll_y_for_test());
        }
        runs.push(ys);
        let _ = fs::remove_dir_all(&dir);
    }
    for (i, (l, u)) in runs[0].iter().zip(&runs[1]).enumerate() {
        assert!((l - u).abs() < 0.01, "frame {i}: locked {l}, alone {u}");
    }
}

#[test]
fn the_preview_follows_the_editor_smoothly_frame_by_frame() {
    let (mut a, dir, editor, _) = scroll_lock_setup("rhumb-lock-smooth");
    a.frame_with(vec![egui::Event::PointerMoved(editor)]);
    let mut offs = Vec::new();
    for _ in 0..150 {
        a.wheel(-1.0);
        offs.push(a.app.preview_off);
    }
    let steps: Vec<f32> = offs.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(
        steps.iter().all(|s| *s >= -0.5),
        "the preview went back: {steps:?}"
    );
    let mut sorted = steps[10..].to_vec();
    sorted.sort_by(|x, y| x.total_cmp(y));
    let median = sorted[sorted.len() / 2];
    let worst = *sorted.last().unwrap();
    assert!(
        worst < median * 2.5 + 2.0,
        "a step of {worst} among steps of {median}: {steps:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_editor_follows_the_preview_smoothly_frame_by_frame() {
    let (mut a, dir, _, preview) = scroll_lock_setup("rhumb-lock-smooth-back");
    a.frame_with(vec![egui::Event::PointerMoved(preview)]);
    let mut ys = Vec::new();
    for _ in 0..150 {
        a.wheel(-1.0);
        ys.push(a.app.ed.scroll_y_for_test());
    }
    let steps: Vec<f32> = ys.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(
        steps.iter().all(|s| *s >= -0.5),
        "the editor went back: {steps:?}"
    );
    let mut sorted = steps[10..].to_vec();
    sorted.sort_by(|x, y| x.total_cmp(y));
    let median = sorted[sorted.len() / 2];
    let worst = *sorted.last().unwrap();
    assert!(
        worst < median * 3.0 + 3.0,
        "a step of {worst} among steps of {median}: {steps:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_heading_in_the_editor_is_at_the_top_of_the_preview_when_the_editor_is_there() {
    let (mut a, dir, _, _) = scroll_lock_setup("rhumb-lock-heading");
    let text = a.text();
    let target = text
        .lines()
        .position(|l| l == "## Section 100")
        .expect("the heading is in the document");
    a.app.ed.set_scroll_line(target as f32);
    for _ in 0..20 {
        a.frame();
    }
    let line = a.app.preview.line_at_y(a.app.preview_off);
    assert!(
        (line - target as f32).abs() < 1.5,
        "the preview shows line {line}, not {target}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_lock_follows_a_new_file_and_not_the_old_ones_position() {
    let (dir, first, second) = two_notes("rhumb-lock-newfile");
    let mut a = App::new(&dir);
    show_file(&mut a, &first);
    a.app.ed.set_scroll_line(0.0);
    show_file(&mut a, &second);
    for _ in 0..10 {
        a.frame();
    }
    assert!(a.app.preview_off < 1.0);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_preview_catches_up_after_typing_pauses_and_not_on_every_keystroke() {
    let (dir, md) = markdown_workspace("rhumb-preview-debounce", 20);
    let mut a = App::new(&dir);
    a.open_and_focus(&md);
    for _ in 0..5 {
        a.frame();
    }
    let typed = "QQQ";
    assert!(!a.app.preview_buffer.contains(typed));
    // A burst of typing with no pause: what the preview is drawn from is left alone.
    for ch in typed.chars() {
        a.type_char(&ch.to_string());
    }
    assert!(
        a.text().contains(typed),
        "the keystrokes reached the document"
    );
    assert!(
        !a.app.preview_buffer.contains(typed),
        "the preview did not re-read the document while typing was still going on"
    );
    // Then a pause longer than the debounce, and it catches up.
    std::thread::sleep(Duration::from_millis(250));
    for _ in 0..5 {
        a.frame();
    }
    assert!(
        a.app.preview_buffer.contains(typed),
        "after the pause the preview shows what was typed"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// `cargo test --release probe_preview -- --ignored --nocapture`
#[test]
#[ignore]
fn probe_preview() {
    for sections in [50, 500, 3_000] {
        let (dir, md) = markdown_workspace(&format!("rhumb-preview-{sections}"), sections);
        let bytes = fs::metadata(&md).unwrap().len();
        let mut a = App::new(&dir);
        let opened = Instant::now();
        a.open_and_focus(&md);
        let first = opened.elapsed();
        for _ in 0..5 {
            a.frame();
        }
        let mut idle = Vec::new();
        for _ in 0..40 {
            idle.push(a.frame().as_secs_f64() * 1000.0);
        }
        let mut typing = Vec::new();
        for _ in 0..40 {
            typing.push(a.type_char("x").as_secs_f64() * 1000.0);
        }
        // After a pause, so the debounce fires and the preview re-parses.
        std::thread::sleep(Duration::from_millis(200));
        let mut after = Vec::new();
        for _ in 0..6 {
            after.push(a.frame().as_secs_f64() * 1000.0);
        }
        idle.sort_by(|a, b| a.total_cmp(b));
        typing.sort_by(|a, b| a.total_cmp(b));
        let st = a.app.preview.stats();
        eprintln!(
            "      preview stats: {} blocks, parse {} us, shape {:.2} ms, document is markdown: {}, preview on: {}",
            st.blocks,
            st.parse_us,
            st.shape_ms,
            a.app.doc().is_some_and(|d| d.kind == DocKind::Markdown),
            a.app.preview_visible
        );
        eprintln!(
            "{sections:5} sections ({:5} KB): open+first frames {:6.0} ms | idle median {:.2} max {:.2} | typing median {:.2} max {:.2} | frames after the debounce {:?}",
            bytes / 1024,
            first.as_secs_f64() * 1000.0,
            idle[idle.len() / 2],
            idle[idle.len() - 1],
            typing[typing.len() / 2],
            typing[typing.len() - 1],
            after
                .iter()
                .map(|m| (m * 10.0).round() / 10.0)
                .collect::<Vec<_>>()
        );
        let _ = fs::remove_dir_all(&dir);
    }
}

/// `cargo test --release probe_whole_app_scrolling -- --ignored --nocapture`
///
/// The scroll position after every frame of one wheel notch, then of a steady
/// spin, as the deltas between frames. An even scroll is a run of similar small
/// numbers; a stutter is a big one followed by nothing.
#[test]
#[ignore]
fn probe_whole_app_scrolling() {
    let (dir, src) = workspace("rhumb-probe-scroll", 50, 5_000);
    let mut a = App::new(&dir);
    a.open_and_focus(&src);
    a.hover_editor();
    for _ in 0..10 {
        a.frame();
    }
    let start = a.scrolled();
    let mut last = start;
    let mut deltas = Vec::new();
    a.wheel(-1.0);
    deltas.push(a.scrolled() - last);
    last = a.scrolled();
    for _ in 0..40 {
        a.frame();
        deltas.push(a.scrolled() - last);
        last = a.scrolled();
    }
    eprintln!(
        "one notch, per-frame movement: {:?}",
        deltas
            .iter()
            .map(|d| (d * 10.0).round() / 10.0)
            .collect::<Vec<_>>()
    );
    // A steady spin: a notch every third frame for a second.
    let mut deltas = Vec::new();
    for i in 0..60 {
        if i % 3 == 0 {
            a.wheel(-1.0);
        } else {
            a.frame();
        }
        deltas.push(a.scrolled() - last);
        last = a.scrolled();
    }
    eprintln!(
        "steady spin, per-frame movement: {:?}",
        deltas
            .iter()
            .map(|d| (d * 10.0).round() / 10.0)
            .collect::<Vec<_>>()
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A window that nobody is touching should not be redrawn faster than its caret
/// blinks: every frame it asks for is CPU and battery spent on a picture that has
/// not changed.
#[test]
fn an_idle_window_with_a_focused_editor_asks_for_no_more_than_the_caret_needs() {
    let (dir, src) = workspace("rhumb-idle-repaint", 20, 500);
    let mut a = App::new(&dir);
    a.open_and_focus(&src);
    // Ten seconds in which the window is redrawn exactly when it asks to be.
    let mut frames = 0;
    let mut waited = Duration::ZERO;
    while waited < Duration::from_secs(10) {
        // Never advance by nothing: a zero repaint delay would spin here forever
        // instead of failing the count below, which is worse than a failure.
        let wait = a
            .repaint_in
            .min(Duration::from_secs(10))
            .max(Duration::from_millis(1));
        a.time += wait.as_secs_f64();
        waited += wait;
        a.frame();
        frames += 1;
    }
    // The caret turns on and off twice a second - about twenty frames - plus a
    // few for a worker answering once and the first frame. A window that asked
    // for a frame every time would draw six hundred or more.
    assert!(
        frames <= 40,
        "an idle window drew {frames} frames in ten seconds"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Clicking in the empty space to the right of a line puts the caret after the
/// line's last letter, in the whole window and not just in the editor on its own:
/// whatever else is around it must not take the click.
#[test]
fn a_click_in_the_blank_space_after_a_line_puts_the_caret_at_its_end() {
    let dir = std::env::temp_dir().join(format!("rhumb-eol-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let docs: [(&str, String); 5] = [
        (
            "plain",
            (1..=60)
                .map(|i| {
                    format!(
                        "let value_{i} = {i};
"
                    )
                })
                .collect(),
        ),
        (
            "tabs",
            (1..=60)
                .map(|i| {
                    format!(
                        "	if x == {i} {{	}}
"
                    )
                })
                .collect(),
        ),
        (
            "trailing spaces",
            (1..=60)
                .map(|i| {
                    format!(
                        "row {i}   
"
                    )
                })
                .collect(),
        ),
        (
            "crlf",
            (1..=60)
                .map(|i| {
                    format!(
                        "crlf line {i}
"
                    )
                })
                .collect(),
        ),
        (
            "blank lines mixed in",
            (1..=60)
                .map(|i| {
                    if i % 3 == 0 {
                        "
"
                        .into()
                    } else {
                        format!(
                            "text {i}
"
                        )
                    }
                })
                .collect(),
        ),
    ];
    for (label, body) in docs {
        let path = dir.join("doc.txt");
        fs::write(&path, &body).unwrap();
        let mut a = App::new(&dir);
        a.open_and_focus(&path);
        let text = a.text();
        let ends: Vec<usize> = {
            let mut at = 0;
            text.split('\n')
                .map(|l| {
                    let end = at + l.chars().count();
                    at = end + 1;
                    end
                })
                .collect()
        };
        for x in [700.0, 1000.0, 1100.0] {
            for y in (170..700).step_by(17) {
                a.click_at(Pos2::new(x, y as f32));
                let caret = a.caret();
                assert!(
                    ends.contains(&caret),
                    "{label}: clicking at ({x}, {y}) put the caret at {caret}, which is not the end of a line"
                );
            }
        }
    }
    let _ = fs::remove_dir_all(&dir);
}

/// The same on a first click, before the editor has the keyboard, and on a screen
/// that scales the interface, which is where a fraction of a point decides a hit.
#[test]
fn a_first_click_after_a_line_reaches_its_end_at_any_scale() {
    let dir = std::env::temp_dir().join(format!("rhumb-eols-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("doc.txt");
    let body: String = (1..=60)
        .map(|i| {
            format!(
                "line number {i} ends here
"
            )
        })
        .collect();
    fs::write(&path, &body).unwrap();
    let text = body.clone();
    let ends: Vec<usize> = {
        let mut at = 0;
        text.split('\n')
            .map(|l| {
                let end = at + l.chars().count();
                at = end + 1;
                end
            })
            .collect()
    };
    for scale in [1.0, 1.25, 1.5, 2.0] {
        for focused_first in [false, true] {
            let mut a = App::new(&dir);
            a.ctx.set_pixels_per_point(scale);
            a.app.open_path(&path);
            a.settle(|a| a.app.loading.is_none() && a.app.doc().is_some());
            a.frame();
            if focused_first {
                a.ctx.memory_mut(|m| m.request_focus(Id::new(codeedit::ID)));
                a.frame();
            }
            for y in (170..700).step_by(13) {
                a.click_at(Pos2::new(1000.0, y as f32));
                let caret = a.caret();
                assert!(
                    ends.contains(&caret),
                    "scale {scale}, focused first {focused_first}: a click at y={y} put the caret at {caret}"
                );
                // A fresh, unfocused editor is unfocused only for the first click.
            }
        }
    }
    let _ = fs::remove_dir_all(&dir);
}

/// The same with soft wrap on: a click in the blank space after a *row* belongs to
/// the end of that row, which is not always the end of its line.
#[test]
fn a_click_in_the_blank_space_after_a_wrapped_row_puts_the_caret_at_its_end() {
    let dir = std::env::temp_dir().join(format!("rhumb-eolw-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("doc.txt");
    let body: String = (1..=40)
        .map(|i| {
            format!(
                "{i} {}
",
                "the quick brown fox jumps over the lazy dog ".repeat(1 + i % 7)
            )
        })
        .collect();
    fs::write(&path, &body).unwrap();
    let mut a = App::new(&dir);
    a.app.wrap = true;
    a.open_and_focus(&path);
    a.frame();
    // The last row of a line is mostly short, so far to the right of it is blank,
    // and a click there has to land at the end of the line.
    let mut checked = 0;
    for y in (150..720).step_by(9) {
        a.click_at(Pos2::new(1100.0, y as f32));
        let caret = a.caret();
        let rows = a.app.ed.last_rows().to_vec();
        let Some(i) = rows
            .iter()
            .position(|r| r.chars.0 <= caret && caret <= r.chars.1)
        else {
            continue;
        };
        let last_of_line = rows.get(i + 1).is_none_or(|n| n.line != rows[i].line);
        if last_of_line && rows[i].chars.1 - rows[i].chars.0 < 60 {
            assert_eq!(
                caret, rows[i].chars.1,
                "clicking at y={y} after a line's last row put the caret at {caret}: {rows:?}"
            );
            checked += 1;
        }
    }
    assert!(checked > 5, "only {checked} line ends were clicked");
    let _ = fs::remove_dir_all(&dir);
}

/// `cargo test --release probe_whole_app_typing -- --ignored --nocapture`
#[test]
#[ignore]
fn probe_whole_app_typing() {
    for (label, files, lines, focus) in [
        ("small folder", 20, 3_000, false),
        ("2,000 files", 2_000, 3_000, false),
        ("10,000 files", 10_000, 3_000, false),
        ("10,000 files, focus mode", 10_000, 3_000, true),
    ] {
        let (dir, src) = workspace(&format!("rhumb-probe-{files}"), files, lines);
        let mut a = App::new(&dir);
        a.open_and_focus(&src);
        if focus {
            a.app.focus = true;
            a.frame();
        }
        let mut idle = Vec::new();
        for _ in 0..60 {
            idle.push(a.frame().as_secs_f64() * 1000.0);
        }
        let mut typing = Vec::new();
        for i in 0..200 {
            typing.push(
                a.type_char(if i % 25 == 24 { "\n" } else { "x" })
                    .as_secs_f64()
                    * 1000.0,
            );
        }
        let (im, ip, ix) = percentiles(idle);
        let (tm, tp, tx) = percentiles(typing);
        eprintln!(
            "{label:26} idle median {im:.2} p99 {ip:.2} max {ix:.2} | typing median {tm:.2} p99 {tp:.2} max {tx:.2} ms"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}

// ---- search through the index --------------------------------------------------------

fn search_tree(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("a/b")).unwrap();
    fs::write(root.join("report.txt"), b"x").unwrap();
    fs::write(root.join("a/annual-report.txt"), b"x").unwrap();
    fs::write(root.join("a/b/report"), b"x").unwrap();
    fs::write(root.join("other.md"), b"x").unwrap();
    root
}

/// Types into the search box, past the debounce, and runs frames until the answer is in.
fn search_for(a: &mut App, query: &str) {
    a.app.filter = query.to_owned();
    a.app.on_filter_changed();
    a.app.search_typed = Some(Instant::now() - Duration::from_secs(5));
    for _ in 0..600 {
        a.frame();
        if !a.app.search.running && a.app.search_shown {
            return;
        }
        std::thread::sleep(Duration::from_millis(3));
    }
    panic!("the search never finished");
}

fn wait_for_index(a: &mut App) {
    for _ in 0..1000 {
        a.frame();
        if a.app.indexes.ready_for(&a.app.cwd).is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(3));
    }
    panic!("the index never finished");
}

#[test]
fn showing_a_folder_starts_an_index_of_it() {
    let root = search_tree("rhumb-app-idx-start");
    let mut a = App::new(&root);
    a.frame();
    assert!(a.app.indexes.any_for(&root).is_some());
    wait_for_index(&mut a);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn navigating_into_a_folder_the_index_covers_does_not_start_another() {
    let root = search_tree("rhumb-app-idx-reuse");
    let mut a = App::new(&root);
    a.frame();
    wait_for_index(&mut a);
    a.app.navigate(&root.join("a"));
    a.frame();
    assert_eq!(a.app.indexes.len(), 1);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_search_with_a_ready_index_comes_from_it_and_is_ranked() {
    let root = search_tree("rhumb-app-idx-ranked");
    let mut a = App::new(&root);
    wait_for_index(&mut a);
    search_for(&mut a, "report");
    assert!(a.app.search.indexed, "answered from the index");
    let names: Vec<&str> = a
        .app
        .search
        .results
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    // `report.txt` is a whole name and `report` another: those first, the shallower first.
    assert_eq!(
        names,
        vec!["report", "report.txt", "annual-report.txt"],
        "{names:?}"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_search_before_the_index_is_ready_still_finds_everything_by_walking() {
    let root = search_tree("rhumb-app-idx-walk");
    let mut a = App::new(&root);
    // No frame has run, so no index has been asked for yet.
    search_for(&mut a, "report");
    assert_eq!(a.app.search.results.len(), 3);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn the_status_bar_says_when_results_came_from_an_index() {
    let root = search_tree("rhumb-app-idx-status");
    let mut a = App::new(&root);
    wait_for_index(&mut a);
    search_for(&mut a, "md");
    assert!(a.app.search.indexed);
    assert_eq!(a.app.search.results.len(), 1);
    assert!(a.app.search.scanned >= 4, "{}", a.app.search.scanned);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_new_file_appears_in_search_results_after_the_folder_changes() {
    let root = search_tree("rhumb-app-idx-fresh");
    let mut a = App::new(&root);
    wait_for_index(&mut a);
    search_for(&mut a, "brandnew");
    assert!(a.app.search.results.is_empty());
    fs::write(root.join("a/brandnew.txt"), b"x").unwrap();
    // The watcher's message for it.
    a.app.indexes.changed(&root.join("a"));
    for _ in 0..300 {
        std::thread::sleep(Duration::from_millis(5));
        if a.app
            .indexes
            .ready_for(&root)
            .is_some_and(|ix| !ix.search(&root, "brandnew", 3).0.is_empty())
        {
            break;
        }
    }
    search_for(&mut a, "brandnew");
    assert_eq!(a.app.search.results.len(), 1);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn an_answer_to_a_search_that_was_replaced_is_ignored() {
    let root = search_tree("rhumb-app-idx-stale");
    let mut a = App::new(&root);
    wait_for_index(&mut a);
    search_for(&mut a, "report");
    let before = a.app.search.results.len();
    // A late chunk from a search that is no longer the current one.
    let stale = workers::SearchChunk {
        token: a.app.search.token - 1,
        found: vec![fs_model::Entry {
            name: "ghost".into(),
            path: root.join("ghost"),
            is_dir: false,
            is_symlink: false,
            size: 0,
            modified: None,
            hidden: false,
        }],
        scanned: 1,
        done: true,
        truncated: false,
    };
    a.app.tx.send(Msg::Search(stale)).unwrap();
    a.frame();
    assert_eq!(a.app.search.results.len(), before);
    assert!(a.app.search.results.iter().all(|e| e.name != "ghost"));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn clearing_the_search_box_clears_the_results() {
    let root = search_tree("rhumb-app-idx-clear");
    let mut a = App::new(&root);
    wait_for_index(&mut a);
    search_for(&mut a, "report");
    assert!(!a.app.search.results.is_empty());
    a.app.filter.clear();
    a.app.on_filter_changed();
    a.frame();
    assert!(a.app.search.results.is_empty());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn searching_only_this_folder_does_not_use_the_index() {
    let root = search_tree("rhumb-app-idx-here");
    let mut a = App::new(&root);
    wait_for_index(&mut a);
    a.app.scope = SearchScope::Here;
    a.app.filter = "report".into();
    a.app.on_filter_changed();
    a.frame();
    assert!(!a.app.search.indexed);
    assert_eq!(
        a.app.search.results.len(),
        0,
        "the list is filtered in place instead"
    );
    assert_eq!(a.app.row_count(), 1, "report.txt is the one file here");
    let _ = fs::remove_dir_all(&root);
}

// ---- archives as folders -------------------------------------------------------------

fn zip_in(dir: &Path, name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    use std::io::Write;
    let path = dir.join(name);
    let f = fs::File::create(&path).unwrap();
    let mut z = zip::ZipWriter::new(f);
    let opt = zip::write::SimpleFileOptions::default();
    for (n, body) in files {
        if n.ends_with('/') {
            z.add_directory(*n, opt).unwrap();
        } else {
            z.start_file(*n, opt).unwrap();
            z.write_all(body).unwrap();
        }
    }
    z.finish().unwrap();
    path
}

fn archive_workspace(name: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let z = zip_in(
        &dir,
        "pack.zip",
        &[
            ("readme.txt", b"hello from the archive"),
            ("src/main.rs", b"fn main() {}\n"),
            ("src/lib.rs", b"// lib\n"),
            ("src/deep/er/note.md", b"# note\n"),
            (".hidden", b"h"),
            ("empty/", b""),
        ],
    );
    fs::write(dir.join("plain.txt"), b"plain").unwrap();
    (dir, z)
}

fn listed(a: &mut App) {
    for _ in 0..600 {
        a.frame();
        if !matches!(a.app.listing, Listing::Loading) {
            return;
        }
        std::thread::sleep(Duration::from_millis(3));
    }
    panic!("the listing never arrived");
}

fn shown_names(a: &App) -> Vec<String> {
    let mut n: Vec<String> = a
        .app
        .visible
        .iter()
        .filter_map(|i| a.app.entries.get(*i))
        .map(|e| e.name.clone())
        .collect();
    n.sort();
    n
}

fn finish_jobs(a: &mut App) {
    for _ in 0..1000 {
        a.frame();
        if a.app.jobs.is_empty() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("a job never finished");
}

#[test]
fn an_archive_opens_as_a_folder_and_lists_what_is_in_it() {
    let (dir, z) = archive_workspace("rhumb-arch-open");
    let mut a = App::new(&dir);
    a.frame();
    a.app.open_path(&z);
    assert_eq!(a.app.cwd, z, "the archive is now the folder shown");
    listed(&mut a);
    assert!(matches!(a.app.listing, Listing::Ready));
    assert_eq!(shown_names(&a), vec!["empty", "readme.txt", "src"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_folder_inside_an_archive_opens_and_lists() {
    let (dir, z) = archive_workspace("rhumb-arch-inner");
    let mut a = App::new(&dir);
    a.app.open_path(&z);
    listed(&mut a);
    a.app.open_path(&z.join("src"));
    listed(&mut a);
    assert_eq!(a.app.cwd, z.join("src"));
    assert_eq!(shown_names(&a), vec!["deep", "lib.rs", "main.rs"]);
    a.app.open_path(&z.join("src/deep/er"));
    listed(&mut a);
    assert_eq!(shown_names(&a), vec!["note.md"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn hidden_names_inside_an_archive_follow_the_hidden_files_setting() {
    let (dir, z) = archive_workspace("rhumb-arch-hidden");
    let mut a = App::new(&dir);
    a.app.show_hidden = false;
    a.app.open_path(&z);
    listed(&mut a);
    assert!(!shown_names(&a).contains(&".hidden".to_owned()));
    a.app.show_hidden = true;
    a.app.request_listing();
    listed(&mut a);
    assert!(shown_names(&a).contains(&".hidden".to_owned()));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_address_bar_names_the_archive_and_each_folder_inside_it() {
    let (dir, z) = archive_workspace("rhumb-arch-crumbs");
    let crumbs = fs_model::breadcrumbs(&z.join("src/deep"));
    let labels: Vec<&str> = crumbs.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(
        &labels[labels.len() - 3..],
        ["pack.zip", "src", "deep"],
        "{labels:?}"
    );
    // Each segment leads to its own place.
    let archive_seg = &crumbs[crumbs.len() - 3];
    assert_eq!(archive_seg.1, z);
    assert_eq!(crumbs[crumbs.len() - 2].1, z.join("src"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn up_from_inside_an_archive_goes_one_level_and_from_its_top_to_the_folder_holding_it() {
    let (dir, z) = archive_workspace("rhumb-arch-up");
    let mut a = App::new(&dir);
    a.app.open_path(&z.join("src/deep"));
    // `open_path` on a folder inside walks there directly.
    listed(&mut a);
    assert_eq!(a.app.cwd, z.join("src/deep"));
    a.app.go_up();
    assert_eq!(a.app.cwd, z.join("src"));
    a.app.go_up();
    assert_eq!(a.app.cwd, z);
    a.app.go_up();
    assert_eq!(
        a.app.cwd, dir,
        "out of the archive and into the folder it is in"
    );
    listed(&mut a);
    assert!(shown_names(&a).contains(&"pack.zip".to_owned()));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn back_and_forward_walk_in_and_out_of_an_archive() {
    let (dir, z) = archive_workspace("rhumb-arch-history");
    let mut a = App::new(&dir);
    a.app.open_path(&z);
    a.app.open_path(&z.join("src"));
    a.app.go_back();
    assert_eq!(a.app.cwd, z);
    a.app.go_back();
    assert_eq!(a.app.cwd, dir);
    a.app.go_forward();
    assert_eq!(a.app.cwd, z);
    listed(&mut a);
    assert_eq!(shown_names(&a), vec!["empty", "readme.txt", "src"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_file_inside_an_archive_opens_in_the_editor_and_cannot_be_changed() {
    let (dir, z) = archive_workspace("rhumb-arch-readfile");
    let mut a = App::new(&dir);
    a.app.open_path(&z);
    listed(&mut a);
    let file = z.join("readme.txt");
    a.app.open_path(&file);
    a.settle(|a| a.app.loading.is_none() && a.app.doc().is_some());
    assert_eq!(a.text(), "hello from the archive");
    let doc = a.app.doc().unwrap();
    assert!(doc.read_only, "an archive is never written back to");
    assert_eq!(doc.path, file, "and it is named for where it is");
    // Typing does nothing to it.
    a.ctx.memory_mut(|m| m.request_focus(Id::new(codeedit::ID)));
    a.frame();
    a.type_char("X");
    assert_eq!(a.text(), "hello from the archive");
    assert!(!a.app.doc().unwrap().dirty());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn saving_a_file_from_an_archive_does_nothing() {
    let (dir, z) = archive_workspace("rhumb-arch-nosave");
    let mut a = App::new(&dir);
    a.app.open_path(&z.join("readme.txt"));
    a.settle(|a| a.app.loading.is_none() && a.app.doc().is_some());
    a.app.save_doc();
    // The archive is as it was.
    let again = archive::list(&z, "").unwrap();
    assert_eq!(
        again.iter().find(|e| e.name == "readme.txt").unwrap().size,
        22
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_file_that_is_not_in_the_archive_says_so_instead_of_opening() {
    let (dir, z) = archive_workspace("rhumb-arch-nofile");
    let mut a = App::new(&dir);
    a.app.open_path(&z.join("no-such-file.txt"));
    for _ in 0..200 {
        a.frame();
        std::thread::sleep(Duration::from_millis(3));
        if a.app.loading.is_none() {
            break;
        }
    }
    assert!(a.app.loading.is_none());
    assert!(a.app.doc().is_none() || a.app.doc().unwrap().text.is_empty());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_file_can_be_pasted_into_a_zip() {
    let (dir, z) = archive_workspace("rhumb-arch-paste");
    let mut a = App::new(&dir);
    // A zip is editable, so this starts a job rather than refusing.
    a.app
        .start_transfer(vec![dir.join("plain.txt")], z.clone(), false);
    assert!(!a.app.jobs.is_empty(), "adding to a zip starts a job");
    finish_jobs(&mut a);
    a.app
        .start_transfer(vec![dir.join("plain.txt")], z.join("src"), false);
    assert!(!a.app.jobs.is_empty());
    finish_jobs(&mut a);
    let names: Vec<String> = archive::list(&z, "")
        .unwrap()
        .iter()
        .map(|e| e.name.clone())
        .collect();
    assert!(names.iter().any(|n| n == "plain.txt"), "{names:?}");
    // The second add went under src/, so it is listed there, not at the root.
    let inner: Vec<String> = archive::list(&z, "src")
        .unwrap()
        .iter()
        .map(|e| e.name.clone())
        .collect();
    assert!(
        inner.iter().any(|n| n == "plain.txt"),
        "the file landed under src/: {inner:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_zip_entry_can_be_deleted_but_not_renamed_or_created() {
    let (dir, z) = archive_workspace("rhumb-arch-edit");
    let mut a = App::new(&dir);
    a.app.open_path(&z);
    listed(&mut a);
    a.app.sel.clear();
    a.app.sel.insert(z.join("readme.txt"));
    a.app.delete_selection(false);
    assert!(!a.app.jobs.is_empty(), "removing a zip entry starts a job");
    assert!(
        matches!(a.app.dialog, Dialog::None),
        "an archive has no recycle bin"
    );
    finish_jobs(&mut a);
    let names: Vec<String> = archive::list(&z, "")
        .unwrap()
        .iter()
        .map(|e| e.name.clone())
        .collect();
    assert!(
        !names.iter().any(|n| n == "readme.txt"),
        "the entry is gone: {names:?}"
    );
    // Renaming and creating inside an archive are still refused.
    a.app.start_rename(&z.join("src/lib.rs"));
    assert!(matches!(a.app.dialog, Dialog::None));
    a.app.apply_create(&z, "new.txt", false);
    assert!(
        archive::list(&z, "")
            .unwrap()
            .iter()
            .all(|e| e.name != "new.txt")
    );
    assert!(z.is_file(), "the archive itself is untouched");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn files_can_be_copied_out_of_an_archive_into_a_folder() {
    let (dir, z) = archive_workspace("rhumb-arch-copyout");
    let dest = dir.join("out");
    fs::create_dir_all(&dest).unwrap();
    let mut a = App::new(&dir);
    a.app.start_transfer(
        vec![z.join("readme.txt"), z.join("src")],
        dest.clone(),
        false,
    );
    finish_jobs(&mut a);
    assert_eq!(
        fs::read(dest.join("readme.txt")).unwrap(),
        b"hello from the archive"
    );
    assert_eq!(fs::read(dest.join("src/lib.rs")).unwrap(), b"// lib\n");
    assert!(dest.join("src/deep/er/note.md").is_file());
    assert!(z.is_file());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn files_cannot_be_moved_out_of_an_archive() {
    let (dir, z) = archive_workspace("rhumb-arch-moveout");
    let dest = dir.join("out");
    fs::create_dir_all(&dest).unwrap();
    let mut a = App::new(&dir);
    a.app
        .start_transfer(vec![z.join("readme.txt")], dest.clone(), true);
    assert!(a.app.jobs.is_empty());
    assert!(!dest.join("readme.txt").exists());
    assert!(
        archive::list(&z, "")
            .unwrap()
            .iter()
            .any(|e| e.name == "readme.txt")
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_paste_that_would_overwrite_asks_before_starting() {
    let (dir, _) = workspace("rhumb-collision", 0, 0);
    let other = dir.join("other");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("a.txt"), b"new").unwrap();
    fs::write(dir.join("a.txt"), b"old").unwrap();
    let mut a = App::new(&dir);
    a.app
        .start_transfer(vec![other.join("a.txt")], dir.clone(), false);
    assert!(
        a.app.jobs.is_empty(),
        "nothing runs until the conflict is answered"
    );
    assert!(matches!(a.app.dialog, Dialog::Collision { .. }));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_paste_with_no_conflict_starts_at_once() {
    let (dir, _) = workspace("rhumb-nocollision", 0, 0);
    let other = dir.join("other");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("b.txt"), b"x").unwrap();
    let mut a = App::new(&dir);
    a.app
        .start_transfer(vec![other.join("b.txt")], dir.clone(), false);
    assert!(!a.app.jobs.is_empty(), "no conflict, so it starts");
    assert!(matches!(a.app.dialog, Dialog::None));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn copying_a_folder_into_itself_is_refused() {
    let (dir, _) = workspace("rhumb-selfcopy", 0, 0);
    let mut a = App::new(&dir);
    a.app.start_transfer(vec![dir.clone()], dir.clone(), false);
    assert!(a.app.jobs.is_empty(), "no runaway copy");
    assert!(a.app.toasts.iter().any(|t| t.text.contains("into itself")));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn extract_here_makes_a_folder_beside_the_archive_with_everything_in_it() {
    let (dir, z) = archive_workspace("rhumb-arch-extract");
    let mut a = App::new(&dir);
    a.app.start_extract(&z);
    finish_jobs(&mut a);
    let out = dir.join("pack");
    assert_eq!(
        fs::read(out.join("readme.txt")).unwrap(),
        b"hello from the archive"
    );
    assert!(out.join("src/deep/er/note.md").is_file());
    assert!(out.join("empty").is_dir());
    // A second time does not overwrite: it makes another folder.
    a.app.start_extract(&z);
    finish_jobs(&mut a);
    assert!(dir.join("pack (2)").join("readme.txt").is_file());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn extracting_a_folder_from_inside_an_archive_takes_only_that_folder() {
    let (dir, z) = archive_workspace("rhumb-arch-extract-part");
    let mut a = App::new(&dir);
    a.app.start_extract(&z.join("src/deep"));
    finish_jobs(&mut a);
    let out = dir.join("pack");
    assert!(out.join("deep/er/note.md").is_file());
    assert!(!out.join("readme.txt").exists());
    assert!(!out.join("src").exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_cancelled_extraction_leaves_nothing_behind() {
    let (dir, _) = archive_workspace("rhumb-arch-cancel");
    let big: Vec<(String, Vec<u8>)> = (0..400)
        .map(|i| (format!("f{i}.bin"), vec![1u8; 20_000]))
        .collect();
    let refs: Vec<(&str, &[u8])> = big
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    let z = zip_in(&dir, "many.zip", &refs);
    let mut a = App::new(&dir);
    a.app.start_extract(&z);
    // Stopped at once.
    for j in &a.app.jobs {
        j.job
            .cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    finish_jobs(&mut a);
    let out = dir.join("many");
    assert!(!out.exists() || fs::read_dir(&out).unwrap().count() < 400);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn extracting_a_tar_gz_names_the_folder_without_either_extension() {
    use std::io::Write;
    let (dir, _) = archive_workspace("rhumb-arch-targz");
    let p = dir.join("bundle.tar.gz");
    let f = fs::File::create(&p).unwrap();
    let mut b = tar::Builder::new(flate2::write::GzEncoder::new(
        f,
        flate2::Compression::fast(),
    ));
    let mut h = tar::Header::new_gnu();
    h.set_size(3);
    h.set_mode(0o644);
    h.set_cksum();
    b.append_data(&mut h, "inside.txt", &b"abc"[..]).unwrap();
    b.into_inner().unwrap().finish().unwrap().flush().unwrap();
    let mut a = App::new(&dir);
    a.app.open_path(&p);
    listed(&mut a);
    assert_eq!(shown_names(&a), vec!["inside.txt"]);
    a.app.start_extract(&p);
    finish_jobs(&mut a);
    assert_eq!(fs::read(dir.join("bundle/inside.txt")).unwrap(), b"abc");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_broken_archive_shows_as_a_folder_that_could_not_be_read() {
    let (dir, _) = archive_workspace("rhumb-arch-broken");
    fs::write(dir.join("broken.zip"), b"definitely not a zip").unwrap();
    let mut a = App::new(&dir);
    a.app.open_path(&dir.join("broken.zip"));
    // Not an archive that can be read, so the file is treated as an ordinary file and
    // opened in the editor instead of being walked into.
    for _ in 0..300 {
        a.frame();
        std::thread::sleep(Duration::from_millis(3));
    }
    assert!(
        a.app.cwd == dir || matches!(a.app.listing, Listing::Failed),
        "{:?} {}",
        a.app.cwd,
        a.app.entries.len()
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn filtering_by_name_works_inside_an_archive() {
    let (dir, z) = archive_workspace("rhumb-arch-filter");
    let mut a = App::new(&dir);
    a.app.open_path(&z.join("src"));
    listed(&mut a);
    a.app.scope = SearchScope::Here;
    a.app.filter = "main".into();
    a.app.on_filter_changed();
    a.frame();
    assert_eq!(shown_names(&a), vec!["main.rs"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_deep_search_inside_an_archive_does_not_walk_or_index_anything() {
    let (dir, z) = archive_workspace("rhumb-arch-nosearch");
    let mut a = App::new(&dir);
    a.app.open_path(&z);
    listed(&mut a);
    let before = a.app.indexes.len();
    a.app.scope = SearchScope::Below;
    a.app.filter = "main".into();
    a.app.on_filter_changed();
    a.app.search_typed = Some(Instant::now() - Duration::from_secs(5));
    for _ in 0..5 {
        a.frame();
    }
    assert!(!a.app.search.running);
    assert_eq!(a.app.indexes.len(), before, "no index of an archive");
    assert!(a.app.indexes.roots().iter().all(|r| !r.starts_with(&z)));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn sorting_applies_inside_an_archive() {
    let (dir, z) = archive_workspace("rhumb-arch-sort");
    let mut a = App::new(&dir);
    a.app.open_path(&z.join("src"));
    listed(&mut a);
    a.app.sort = SortKey::Size;
    a.app.ascending = false;
    a.app.apply_sort();
    let order: Vec<&str> = a.app.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(order[0], "deep", "folders first");
    assert_eq!(
        &order[1..],
        ["main.rs", "lib.rs"],
        "then the larger file first"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_context_menu_for_an_archive_offers_to_extract_it() {
    let (dir, z) = archive_workspace("rhumb-arch-menu");
    assert!(archive::is_archive_file(&z));
    assert!(archive::is_virtual(&z.join("src")));
    assert!(!archive::is_virtual(&dir.join("plain.txt")));
    let mut a = App::new(&dir);
    a.frame();
    a.app.menu.open(Some(Pos2::new(300.0, 200.0)), z.clone());
    for _ in 0..4 {
        a.frame();
    }
    assert_eq!(a.app.menu.path(), Some(z.as_path()));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn archives_are_not_watched_as_if_they_were_folders() {
    let (dir, z) = archive_workspace("rhumb-arch-watch");
    let mut a = App::new(&dir);
    a.app.open_path(&z);
    listed(&mut a);
    a.frame();
    assert_ne!(a.app.watch_target.as_deref(), Some(z.as_path()));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_big_archive_opens_without_stalling_the_frame() {
    let (dir, _) = archive_workspace("rhumb-arch-bigopen");
    let names: Vec<String> = (0..3000).map(|i| format!("d{}/f{i}.txt", i % 30)).collect();
    let refs: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b"x"[..])).collect();
    let z = zip_in(&dir, "big.zip", &refs);
    let mut a = App::new(&dir);
    a.app.open_path(&z);
    // The listing is read on a worker, so no frame waits for it.
    let mut worst = Duration::ZERO;
    for _ in 0..300 {
        let t = a.frame();
        worst = worst.max(t);
        if !matches!(a.app.listing, Listing::Loading) {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(worst < Duration::from_millis(250), "a frame took {worst:?}");
    assert_eq!(a.app.entries.len(), 30);
    let _ = fs::remove_dir_all(&dir);
}

// ---- the preview follows the file ---------------------------------------------------

fn two_notes(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let a = dir.join("first.md");
    let b = dir.join("second.md");
    fs::write(&a, "# First\n\nthe first file's words\n").unwrap();
    fs::write(&b, "# Second\n\nthe second file's words\n").unwrap();
    (dir, a, b)
}

fn show_file(a: &mut App, path: &Path) {
    a.app.open_path(path);
    a.settle(|a| a.app.loading.is_none() && a.app.doc().is_some_and(|d| d.path == path));
    for _ in 0..8 {
        a.frame();
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn the_preview_shows_the_file_that_was_opened_last_and_not_the_first() {
    let (dir, first, second) = two_notes("rhumb-preview-follow");
    let mut a = App::new(&dir);
    show_file(&mut a, &first);
    assert!(a.app.preview_buffer.contains("first file"));
    show_file(&mut a, &second);
    assert!(
        a.app.preview_buffer.contains("second file"),
        "the preview still has {:?}",
        a.app.preview_buffer
    );
    assert!(!a.app.preview_buffer.contains("first file"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn switching_between_tabs_changes_the_preview_each_time() {
    let (dir, first, second) = two_notes("rhumb-preview-tabs");
    let mut a = App::new(&dir);
    show_file(&mut a, &first);
    show_file(&mut a, &second);
    for round in 0..3 {
        let i = a.app.tab_index(&first).unwrap();
        a.app.focus_tab(i);
        for _ in 0..6 {
            a.frame();
        }
        assert!(a.app.preview_buffer.contains("first file"), "round {round}");
        let i = a.app.tab_index(&second).unwrap();
        a.app.focus_tab(i);
        for _ in 0..6 {
            a.frame();
        }
        assert!(
            a.app.preview_buffer.contains("second file"),
            "round {round}"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_preview_of_a_file_that_is_edited_in_a_second_tab_is_that_tabs_text() {
    let (dir, first, second) = two_notes("rhumb-preview-edit");
    let mut a = App::new(&dir);
    show_file(&mut a, &first);
    show_file(&mut a, &second);
    a.ctx.memory_mut(|m| m.request_focus(Id::new(codeedit::ID)));
    a.frame();
    a.type_char("Z");
    std::thread::sleep(Duration::from_millis(200));
    for _ in 0..6 {
        a.frame();
    }
    assert!(
        a.app.preview_buffer.starts_with('Z'),
        "{:?}",
        a.app.preview_buffer
    );
    assert!(a.app.preview_buffer.contains("second file"));
    let _ = fs::remove_dir_all(&dir);
}

// ---- folder tabs and the files open in them ----------------------------------------------

fn tabs_workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    for sub in ["one", "two", "three", "one/inner"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    for (p, body) in [
        ("one/a.txt", "a"),
        ("one/b.txt", "b"),
        ("one/c.md", "# c\n\ntext\n"),
        ("two/x.txt", "x"),
        ("two/y.txt", "y"),
        ("three/z.txt", "z"),
        ("one/inner/deep.txt", "d"),
        ("top.txt", "t"),
    ] {
        fs::write(dir.join(p), body).unwrap();
    }
    dir
}

fn folder_labels(a: &App) -> Vec<String> {
    a.app
        .folders
        .iter()
        .map(folders::FolderTab::label)
        .collect()
}

fn file_labels(a: &App) -> Vec<String> {
    a.app.tabs.iter().map(Tab::label).collect()
}

fn open_file(a: &mut App, path: &Path) {
    a.app.open_path(path);
    a.settle(|a| a.app.loading.is_none() && a.app.doc().is_some_and(|d| d.path == path));
    a.frame();
}

#[test]
fn a_window_starts_with_one_folder_tab_named_for_where_it_opened() {
    let dir = tabs_workspace("rhumb-ftab-start");
    let mut a = App::new(&dir.join("one"));
    a.frame();
    assert_eq!(folder_labels(&a), vec!["one"]);
    assert_eq!(a.app.active_folder, 0);
    assert!(!a.app.shows_file_tab(), "and no file is open");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_new_tab_is_another_place_at_the_same_folder_and_comes_to_the_front() {
    let dir = tabs_workspace("rhumb-ftab-new");
    let mut a = App::new(&dir.join("one"));
    a.frame();
    a.app.new_folder_tab();
    assert_eq!(folder_labels(&a), vec!["one", "one"]);
    assert_eq!(a.app.active_folder, 1);
    assert_eq!(a.app.cwd, dir.join("one"));
    assert!(a.app.sel.is_empty() && a.app.filter.is_empty());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn each_tab_keeps_its_own_folder_and_comes_back_to_it() {
    let dir = tabs_workspace("rhumb-ftab-own");
    let mut a = App::new(&dir.join("one"));
    a.frame();
    a.app.new_folder_tab();
    a.app.navigate(&dir.join("two"));
    listed(&mut a);
    assert_eq!(a.app.cwd, dir.join("two"));
    a.app.switch_folder(0);
    assert_eq!(a.app.cwd, dir.join("one"));
    listed(&mut a);
    assert_eq!(shown_names(&a), vec!["a.txt", "b.txt", "c.md", "inner"]);
    a.app.switch_folder(1);
    assert_eq!(a.app.cwd, dir.join("two"));
    listed(&mut a);
    assert_eq!(shown_names(&a), vec!["x.txt", "y.txt"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_name_of_a_tab_follows_where_it_goes() {
    let dir = tabs_workspace("rhumb-ftab-label");
    let mut a = App::new(&dir.join("one"));
    a.app.new_folder_tab();
    assert_eq!(folder_labels(&a), vec!["one", "one"]);
    a.app.navigate(&dir.join("three"));
    assert_eq!(folder_labels(&a), vec!["one", "three"]);
    a.app.go_up();
    assert_eq!(
        folder_labels(&a)[1],
        dir.file_name().unwrap().to_string_lossy()
    );
    assert_eq!(folder_labels(&a)[0], "one", "the other is as it was");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn each_tab_has_its_own_way_back_and_forward() {
    let dir = tabs_workspace("rhumb-ftab-history");
    let mut a = App::new(&dir.join("one"));
    a.app.new_folder_tab();
    a.app.navigate(&dir.join("two"));
    a.app.navigate(&dir.join("three"));
    a.app.switch_folder(0);
    // The first tab never visited `two` or `three`: going back does not take it there.
    a.app.go_back();
    assert_ne!(a.app.cwd, dir.join("two"));
    assert_ne!(a.app.cwd, dir.join("three"));
    a.app.go_forward();
    assert_eq!(a.app.cwd, dir.join("one"));
    a.app.switch_folder(1);
    a.app.go_back();
    assert_eq!(a.app.cwd, dir.join("two"));
    a.app.go_back();
    assert_eq!(a.app.cwd, dir.join("one"));
    a.app.go_forward();
    assert_eq!(a.app.cwd, dir.join("two"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_selection_comes_back_with_the_tab() {
    let dir = tabs_workspace("rhumb-ftab-selection");
    let mut a = App::new(&dir.join("one"));
    listed(&mut a);
    a.app.new_folder_tab();
    listed(&mut a);
    assert!(
        a.app.sel.is_empty(),
        "a new tab starts with nothing selected"
    );
    a.app.switch_folder(0);
    listed(&mut a);
    a.app.sel.insert(dir.join("one/a.txt"));
    a.app.sel.insert(dir.join("one/b.txt"));
    a.app.cursor = 1;
    a.app.anchor = 0;
    a.app.switch_folder(1);
    listed(&mut a);
    assert!(a.app.sel.is_empty());
    a.app.switch_folder(0);
    assert_eq!(a.app.sel.len(), 2);
    assert!(a.app.sel.contains(&dir.join("one/b.txt")));
    assert_eq!((a.app.cursor, a.app.anchor), (1, 0));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_filter_comes_back_with_the_tab_and_a_new_tab_starts_without_one() {
    let dir = tabs_workspace("rhumb-ftab-filter");
    let mut a = App::new(&dir.join("one"));
    listed(&mut a);
    a.app.scope = SearchScope::Here;
    a.app.filter = "txt".into();
    a.app.on_filter_changed();
    a.frame();
    assert_eq!(shown_names(&a), vec!["a.txt", "b.txt"]);
    a.app.new_folder_tab();
    listed(&mut a);
    assert!(a.app.filter.is_empty());
    assert_eq!(shown_names(&a).len(), 4);
    a.app.switch_folder(0);
    listed(&mut a);
    assert_eq!(a.app.filter, "txt");
    assert_eq!(shown_names(&a), vec!["a.txt", "b.txt"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_scroll_position_is_kept_and_put_back() {
    let dir = tabs_workspace("rhumb-ftab-scroll");
    let mut a = App::new(&dir.join("one"));
    listed(&mut a);
    a.app.new_folder_tab();
    a.app.switch_folder(0);
    a.app.list_scroll = 123.0;
    a.app.switch_folder(1);
    a.app.switch_folder(0);
    assert_eq!(a.app.scroll_restore, Some(123.0));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn each_tab_has_its_own_open_files() {
    let dir = tabs_workspace("rhumb-ftab-files");
    let mut a = App::new(&dir.join("one"));
    listed(&mut a);
    open_file(&mut a, &dir.join("one/a.txt"));
    open_file(&mut a, &dir.join("one/b.txt"));
    assert_eq!(file_labels(&a), vec!["a.txt", "b.txt"]);
    assert!(a.app.shows_file_tab());
    a.app.new_folder_tab();
    assert!(!a.app.shows_file_tab(), "the new tab has no files open");
    assert!(file_labels(&a).is_empty());
    a.app.navigate(&dir.join("two"));
    open_file(&mut a, &dir.join("two/x.txt"));
    assert_eq!(file_labels(&a), vec!["x.txt"]);
    // Back: the first tab's files are there, with the one that was in front still in front.
    a.app.switch_folder(0);
    assert_eq!(file_labels(&a), vec!["a.txt", "b.txt"]);
    assert_eq!(a.app.doc().unwrap().file_name(), "b.txt");
    assert_eq!(a.text(), "b");
    a.app.switch_folder(1);
    assert_eq!(file_labels(&a), vec!["x.txt"]);
    assert_eq!(a.text(), "x");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn opening_a_file_adds_no_folder_tab_and_closing_the_last_file_closes_the_editor() {
    let dir = tabs_workspace("rhumb-ftab-nofolder");
    let mut a = App::new(&dir.join("one"));
    listed(&mut a);
    open_file(&mut a, &dir.join("one/a.txt"));
    assert_eq!(a.app.folder_tab_count(), 1);
    assert!(a.app.shows_file_tab());
    a.app.close_tab(0);
    assert!(!a.app.shows_file_tab());
    assert_eq!(a.app.folder_tab_count(), 1);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn opening_the_same_file_again_brings_its_tab_forward_instead_of_opening_another() {
    let dir = tabs_workspace("rhumb-ftab-same-file");
    let mut a = App::new(&dir.join("one"));
    listed(&mut a);
    open_file(&mut a, &dir.join("one/a.txt"));
    open_file(&mut a, &dir.join("one/b.txt"));
    a.app.open_path(&dir.join("one/a.txt"));
    assert_eq!(file_labels(&a), vec!["a.txt", "b.txt"]);
    assert_eq!(a.app.tabs.active, 0);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_same_file_can_be_open_in_two_folder_tabs() {
    let dir = tabs_workspace("rhumb-ftab-same-file-two");
    let mut a = App::new(&dir.join("one"));
    listed(&mut a);
    open_file(&mut a, &dir.join("one/a.txt"));
    a.app.new_folder_tab();
    open_file(&mut a, &dir.join("one/a.txt"));
    assert_eq!(file_labels(&a), vec!["a.txt"]);
    a.app.switch_folder(0);
    assert_eq!(file_labels(&a), vec!["a.txt"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn closing_the_tab_in_front_shows_its_neighbour_as_it_was() {
    let dir = tabs_workspace("rhumb-ftab-close");
    let mut a = App::new(&dir.join("one"));
    listed(&mut a);
    a.app.new_folder_tab();
    a.app.navigate(&dir.join("two"));
    a.app.new_folder_tab();
    a.app.navigate(&dir.join("three"));
    assert_eq!(a.app.folder_tab_count(), 3);
    a.app.close_folder_tab(2);
    assert_eq!(folder_labels(&a), vec!["one", "two"]);
    assert_eq!(a.app.cwd, dir.join("two"));
    listed(&mut a);
    assert_eq!(shown_names(&a), vec!["x.txt", "y.txt"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn closing_the_first_tab_in_front_shows_the_one_after_it() {
    let dir = tabs_workspace("rhumb-ftab-close-first");
    let mut a = App::new(&dir.join("one"));
    a.app.new_folder_tab();
    a.app.navigate(&dir.join("two"));
    a.app.switch_folder(0);
    a.app.close_folder_tab(0);
    assert_eq!(folder_labels(&a), vec!["two"]);
    assert_eq!(a.app.active_folder, 0);
    assert_eq!(a.app.cwd, dir.join("two"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn closing_a_tab_that_is_not_in_front_changes_nothing_on_screen() {
    let dir = tabs_workspace("rhumb-ftab-close-other");
    let mut a = App::new(&dir.join("one"));
    listed(&mut a);
    a.app.new_folder_tab();
    a.app.navigate(&dir.join("two"));
    a.app.new_folder_tab();
    a.app.navigate(&dir.join("three"));
    a.app.close_folder_tab(0);
    assert_eq!(folder_labels(&a), vec!["two", "three"]);
    assert_eq!(a.app.cwd, dir.join("three"));
    assert_eq!(a.app.active_folder, 1);
    a.app.switch_folder(0);
    assert_eq!(a.app.cwd, dir.join("two"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_last_folder_tab_cannot_be_closed() {
    let dir = tabs_workspace("rhumb-ftab-last");
    let mut a = App::new(&dir.join("one"));
    a.frame();
    a.app.close_folder_tab(0);
    assert_eq!(a.app.folder_tab_count(), 1);
    a.app.new_folder_tab();
    a.app.close_folder_tab(1);
    a.app.close_folder_tab(0);
    assert_eq!(a.app.folder_tab_count(), 1);
    assert_eq!(a.app.cwd, dir.join("one"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn closing_a_tab_closes_the_files_that_were_open_in_it() {
    let dir = tabs_workspace("rhumb-ftab-close-files");
    let mut a = App::new(&dir.join("one"));
    listed(&mut a);
    a.app.new_folder_tab();
    open_file(&mut a, &dir.join("one/a.txt"));
    a.app.close_folder_tab(1);
    assert!(!a.app.shows_file_tab());
    assert_eq!(a.app.folder_tab_count(), 1);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_tab_with_unsaved_changes_is_not_closed_but_brought_forward() {
    let dir = tabs_workspace("rhumb-ftab-dirty");
    let mut a = App::new(&dir.join("one"));
    listed(&mut a);
    a.app.new_folder_tab();
    open_file(&mut a, &dir.join("one/a.txt"));
    a.ctx.memory_mut(|m| m.request_focus(Id::new(codeedit::ID)));
    a.frame();
    a.type_char("Q");
    assert!(a.app.doc().unwrap().dirty());
    a.app.switch_folder(0);
    a.app.close_folder_tab(1);
    assert_eq!(a.app.folder_tab_count(), 2, "it is still there");
    assert_eq!(a.app.active_folder, 1, "and in front");
    assert!(
        a.app
            .toasts
            .iter()
            .any(|t| t.text.contains("Save or close"))
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_tab_with_unsaved_changes_shows_it_on_its_strip_entry() {
    let dir = tabs_workspace("rhumb-ftab-dirty-dot");
    let mut a = App::new(&dir.join("one"));
    listed(&mut a);
    a.app.new_folder_tab();
    open_file(&mut a, &dir.join("one/a.txt"));
    a.ctx.memory_mut(|m| m.request_focus(Id::new(codeedit::ID)));
    a.frame();
    a.type_char("Q");
    assert!(a.app.folder_has_unsaved(1));
    assert!(!a.app.folder_has_unsaved(0));
    a.app.switch_folder(0);
    assert!(
        a.app.folder_has_unsaved(1),
        "also while it is not on screen"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn quitting_with_unsaved_changes_in_another_tab_brings_that_tab_forward() {
    let dir = tabs_workspace("rhumb-ftab-quit");
    let mut a = App::new(&dir.join("one"));
    listed(&mut a);
    a.app.new_folder_tab();
    open_file(&mut a, &dir.join("one/a.txt"));
    a.ctx.memory_mut(|m| m.request_focus(Id::new(codeedit::ID)));
    a.frame();
    a.type_char("Q");
    a.app.switch_folder(0);
    assert_eq!(a.app.unsaved_tab(), Some(0));
    assert_eq!(
        a.app.active_folder, 1,
        "the tab with the unsaved file is shown"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_file_that_finishes_loading_after_the_tab_was_left_lands_in_its_own_tab() {
    let dir = tabs_workspace("rhumb-ftab-loading");
    let mut a = App::new(&dir.join("one"));
    listed(&mut a);
    a.app.open_path(&dir.join("one/a.txt"));
    // Gone to another tab before the file has been read.
    a.app.new_folder_tab();
    for _ in 0..400 {
        a.frame();
        std::thread::sleep(Duration::from_millis(2));
        if a.app.loading.is_none() {
            break;
        }
    }
    assert!(!a.app.shows_file_tab(), "it is not in this tab");
    a.app.switch_folder(0);
    assert_eq!(file_labels(&a), vec!["a.txt"]);
    assert_eq!(a.text(), "a");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ctrl_w_closes_the_file_and_then_the_folder_tab() {
    let dir = tabs_workspace("rhumb-ftab-ctrlw");
    let mut a = App::new(&dir.join("one"));
    listed(&mut a);
    a.app.new_folder_tab();
    open_file(&mut a, &dir.join("one/a.txt"));
    press(&mut a, egui::Key::W, egui::Modifiers::CTRL);
    assert!(!a.app.shows_file_tab());
    assert_eq!(a.app.folder_tab_count(), 2, "the file went, the tab stayed");
    press(&mut a, egui::Key::W, egui::Modifiers::CTRL);
    assert_eq!(a.app.folder_tab_count(), 1, "and now the tab");
    press(&mut a, egui::Key::W, egui::Modifiers::CTRL);
    assert_eq!(a.app.folder_tab_count(), 1, "the last one stays");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ctrl_t_opens_a_folder_tab() {
    let dir = tabs_workspace("rhumb-ftab-ctrlt");
    let mut a = App::new(&dir.join("one"));
    a.frame();
    press(&mut a, egui::Key::T, egui::Modifiers::CTRL);
    assert_eq!(a.app.folder_tab_count(), 2);
    press(&mut a, egui::Key::T, egui::Modifiers::CTRL);
    assert_eq!(a.app.folder_tab_count(), 3);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ctrl_tab_goes_round_the_folder_tabs_when_the_editor_does_not_have_the_keyboard() {
    let dir = tabs_workspace("rhumb-ftab-cycle");
    let mut a = App::new(&dir.join("one"));
    a.frame();
    a.app.new_folder_tab();
    a.app.navigate(&dir.join("two"));
    a.app.new_folder_tab();
    a.app.navigate(&dir.join("three"));
    press(&mut a, egui::Key::Tab, egui::Modifiers::CTRL);
    assert_eq!(a.app.active_folder, 0);
    assert_eq!(a.app.cwd, dir.join("one"));
    press(
        &mut a,
        egui::Key::Tab,
        egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
    );
    assert_eq!(a.app.active_folder, 2);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ctrl_tab_goes_round_the_files_when_the_editor_has_the_keyboard() {
    let dir = tabs_workspace("rhumb-ftab-cycle-files");
    let mut a = App::new(&dir.join("one"));
    listed(&mut a);
    a.app.new_folder_tab();
    open_file(&mut a, &dir.join("one/a.txt"));
    open_file(&mut a, &dir.join("one/b.txt"));
    a.ctx.memory_mut(|m| m.request_focus(Id::new(codeedit::ID)));
    a.frame();
    a.frame();
    assert!(a.app.ed_focused);
    press(&mut a, egui::Key::Tab, egui::Modifiers::CTRL);
    assert_eq!(a.app.doc().unwrap().file_name(), "a.txt");
    assert_eq!(a.app.active_folder, 1, "the folder tab did not change");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn there_is_a_limit_to_how_many_folder_tabs_can_be_opened() {
    let dir = tabs_workspace("rhumb-ftab-limit");
    let mut a = App::new(&dir.join("one"));
    for _ in 0..MAX_TABS + 5 {
        a.app.new_folder_tab();
    }
    assert_eq!(a.app.folder_tab_count(), MAX_TABS);
    assert!(a.app.toasts.iter().any(|t| t.text.contains("At most")));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_tab_whose_folder_has_gone_shows_as_unavailable_and_the_others_still_work() {
    let dir = tabs_workspace("rhumb-ftab-gone");
    let mut a = App::new(&dir.join("one"));
    listed(&mut a);
    a.app.new_folder_tab();
    a.app.navigate(&dir.join("three"));
    listed(&mut a);
    a.app.switch_folder(0);
    fs::remove_dir_all(dir.join("three")).unwrap();
    a.app.switch_folder(1);
    listed(&mut a);
    assert!(matches!(a.app.listing, Listing::Failed));
    a.app.switch_folder(0);
    listed(&mut a);
    assert!(matches!(a.app.listing, Listing::Ready));
    assert_eq!(shown_names(&a).len(), 4);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn folder_tabs_can_be_rearranged_and_the_one_in_front_stays_in_front() {
    let dir = tabs_workspace("rhumb-ftab-move");
    let mut a = App::new(&dir.join("one"));
    a.app.new_folder_tab();
    a.app.navigate(&dir.join("two"));
    a.app.new_folder_tab();
    a.app.navigate(&dir.join("three"));
    assert_eq!(folder_labels(&a), vec!["one", "two", "three"]);
    a.app.move_folder_tab(2, 0);
    assert_eq!(folder_labels(&a), vec!["three", "one", "two"]);
    assert_eq!(a.app.active_folder, 0, "the tab in front went with it");
    a.app.move_folder_tab(0, 1);
    assert_eq!(folder_labels(&a), vec!["one", "three", "two"]);
    assert_eq!(a.app.active_folder, 1);
    a.app.move_folder_tab(1, 1);
    a.app.move_folder_tab(9, 0);
    a.app.move_folder_tab(0, 9);
    assert_eq!(folder_labels(&a), vec!["one", "three", "two"]);
    // The state still belongs to the right tab after the move.
    a.app.switch_folder(2);
    assert_eq!(a.app.cwd, dir.join("two"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn switching_to_a_tab_that_does_not_exist_or_to_the_one_in_front_does_nothing() {
    let dir = tabs_workspace("rhumb-ftab-badswitch");
    let mut a = App::new(&dir.join("one"));
    a.app.new_folder_tab();
    a.app.switch_folder(17);
    a.app.switch_folder(1);
    assert_eq!(a.app.active_folder, 1);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_search_in_one_tab_does_not_leak_into_another() {
    let dir = tabs_workspace("rhumb-ftab-search");
    let mut a = App::new(&dir);
    listed(&mut a);
    a.app.new_folder_tab();
    a.app.navigate(&dir.join("two"));
    a.app.switch_folder(0);
    a.app.scope = SearchScope::Below;
    a.app.filter = "deep".into();
    a.app.on_filter_changed();
    a.app.search_typed = Some(Instant::now() - Duration::from_secs(5));
    for _ in 0..200 {
        a.frame();
        std::thread::sleep(Duration::from_millis(3));
        if !a.app.search.running && a.app.search_shown {
            break;
        }
    }
    assert_eq!(a.app.search.results.len(), 1);
    a.app.switch_folder(1);
    assert!(a.app.filter.is_empty(), "the other tab has no search");
    assert!(a.app.search.results.is_empty());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_preview_of_a_markdown_file_is_that_files_in_whichever_tab_it_is_in() {
    let dir = tabs_workspace("rhumb-ftab-preview");
    let mut a = App::new(&dir.join("one"));
    listed(&mut a);
    open_file(&mut a, &dir.join("one/c.md"));
    for _ in 0..6 {
        a.frame();
    }
    assert!(a.app.preview_buffer.contains("# c"));
    a.app.new_folder_tab();
    open_file(&mut a, &dir.join("one/a.txt"));
    a.app.switch_folder(0);
    for _ in 0..6 {
        a.frame();
    }
    assert!(
        a.app.preview_buffer.contains("# c"),
        "{:?}",
        a.app.preview_buffer
    );
    let _ = fs::remove_dir_all(&dir);
}

// ---- the two rows of tabs, driven by the pointer -----------------------------------------

fn button(pos: Pos2, pressed: bool, which: egui::PointerButton) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: which,
        pressed,
        modifiers: Default::default(),
    }
}

/// A press and release of a button at `pos`, on separate frames, as a hand does it.
fn press_at(a: &mut App, pos: Pos2, which: egui::PointerButton) {
    a.frame_with(vec![egui::Event::PointerMoved(pos)]);
    a.frame_with(vec![button(pos, true, which)]);
    a.frame_with(vec![button(pos, false, which)]);
    a.frame();
}

/// A press that moves on before it lets go.
fn drag_between(a: &mut App, from: Pos2, to: Pos2) {
    a.frame_with(vec![egui::Event::PointerMoved(from)]);
    a.frame_with(vec![button(from, true, egui::PointerButton::Primary)]);
    for k in 1..=6 {
        let p = from + (to - from) * (k as f32 / 6.0);
        a.frame_with(vec![egui::Event::PointerMoved(p)]);
    }
    a.frame_with(vec![button(to, false, egui::PointerButton::Primary)]);
    a.frame();
}

/// An app with `n` folder tabs, each in a folder of its own: `t0`, `t1`...
fn with_folder_tabs(name: &str, n: usize) -> (App, PathBuf) {
    let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    for i in 0..n {
        fs::create_dir_all(dir.join(format!("t{i}"))).unwrap();
    }
    let mut a = App::new(&dir.join("t0"));
    a.frame();
    for i in 1..n {
        a.app.new_folder_tab();
        a.app.navigate(&dir.join(format!("t{i}")));
    }
    for _ in 0..3 {
        a.frame();
    }
    (a, dir)
}

fn folder_tab_at(a: &App, i: usize) -> Pos2 {
    let r = a.app.folder_strip.rects[i];
    Pos2::new(r.center().x - 10.0, r.center().y)
}

fn file_tab_at(a: &App, i: usize) -> Pos2 {
    let r = a.app.doc_strip.rects[i];
    Pos2::new(r.center().x - 10.0, r.center().y)
}

#[test]
fn pressing_a_folder_tab_brings_it_forward() {
    let (mut a, dir) = with_folder_tabs("rhumb-strip-click", 4);
    assert_eq!(a.app.active_folder, 3);
    let at = folder_tab_at(&a, 1);
    press_at(&mut a, at, egui::PointerButton::Primary);
    assert_eq!(a.app.active_folder, 1);
    assert_eq!(a.app.cwd, dir.join("t1"));
    let at = folder_tab_at(&a, 2);
    press_at(&mut a, at, egui::PointerButton::Primary);
    assert_eq!(a.app.cwd, dir.join("t2"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_press_that_wobbles_a_few_pixels_still_selects_the_tab() {
    let (mut a, dir) = with_folder_tabs("rhumb-strip-wobble", 4);
    let from = folder_tab_at(&a, 0);
    drag_between(&mut a, from, from + Vec2::new(9.0, 2.0));
    assert_eq!(a.app.active_folder, 0);
    assert_eq!(
        folder_labels(&a),
        vec!["t0", "t1", "t2", "t3"],
        "and nothing was moved"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn dragging_a_folder_tab_to_another_place_moves_it_there() {
    let (mut a, dir) = with_folder_tabs("rhumb-strip-drag", 4);
    let from = folder_tab_at(&a, 0);
    let to = folder_tab_at(&a, 2);
    drag_between(&mut a, from, to);
    assert_eq!(folder_labels(&a), vec!["t1", "t2", "t0", "t3"]);
    assert_eq!(a.app.cwd, dir.join("t0"), "the one dragged is in front");
    a.app.switch_folder(0);
    assert_eq!(a.app.cwd, dir.join("t1"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn dragging_past_either_end_puts_the_tab_at_that_end() {
    let (mut a, dir) = with_folder_tabs("rhumb-strip-drag-ends", 4);
    let from = folder_tab_at(&a, 1);
    drag_between(&mut a, from, Pos2::new(5.0, from.y));
    assert_eq!(folder_labels(&a)[0], "t1");
    let from = folder_tab_at(&a, 0);
    let last = a.app.folder_strip.rects[3];
    drag_between(&mut a, from, Pos2::new(last.right() + 200.0, from.y));
    assert_eq!(folder_labels(&a)[3], "t1");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_middle_button_closes_the_folder_tab_it_is_over_and_no_other() {
    let (mut a, dir) = with_folder_tabs("rhumb-strip-middle", 4);
    let at = folder_tab_at(&a, 1);
    press_at(&mut a, at, egui::PointerButton::Middle);
    assert_eq!(folder_labels(&a), vec!["t0", "t2", "t3"]);
    let at = folder_tab_at(&a, 0);
    press_at(&mut a, at, egui::PointerButton::Middle);
    assert_eq!(folder_labels(&a), vec!["t2", "t3"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_cross_closes_its_own_folder_tab() {
    let (mut a, dir) = with_folder_tabs("rhumb-strip-cross", 4);
    a.app.switch_folder(2);
    a.frame();
    let r = a.app.folder_strip.rects[2];
    let cross = Pos2::new(r.right() - 11.0, r.center().y);
    press_at(&mut a, cross, egui::PointerButton::Primary);
    assert_eq!(folder_labels(&a), vec!["t0", "t1", "t3"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_plus_button_opens_a_new_folder_tab_after_the_last() {
    let (mut a, dir) = with_folder_tabs("rhumb-strip-plus", 3);
    let last = *a.app.folder_strip.rects.last().unwrap();
    let plus = Pos2::new(last.right() + 14.0, last.center().y);
    press_at(&mut a, plus, egui::PointerButton::Primary);
    assert_eq!(a.app.folder_tab_count(), 4);
    assert_eq!(a.app.active_folder, 3);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn folder_tabs_that_do_not_fit_slide_instead_of_vanishing() {
    let (mut a, dir) = with_folder_tabs("rhumb-strip-overflow", 22);
    let r = a.app.folder_strip.rects[a.app.active_folder];
    assert!(r.left() >= 0.0 && r.right() <= 1160.0, "{r:?}");
    a.app.switch_folder(0);
    a.frame();
    assert_eq!(a.app.folder_strip.scroll, 0.0);
    a.app.switch_folder(21);
    a.frame();
    assert!(a.app.folder_strip.scroll > 0.0);
    let r = a.app.folder_strip.rects[21];
    assert!(r.right() <= 1160.0 && r.left() >= 0.0, "{r:?}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_wheel_over_the_folder_tabs_slides_them_sideways() {
    let (mut a, dir) = with_folder_tabs("rhumb-strip-wheel", 22);
    a.app.switch_folder(0);
    a.frame();
    let at = Pos2::new(300.0, a.app.folder_strip.rects[0].center().y);
    a.frame_with(vec![egui::Event::PointerMoved(at)]);
    for _ in 0..8 {
        a.frame_with(vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: Vec2::new(0.0, -3.0),
            phase: egui::TouchPhase::Move,
            modifiers: Default::default(),
        }]);
    }
    for _ in 0..30 {
        a.frame();
    }
    assert!(
        a.app.folder_strip.scroll > 100.0,
        "{}",
        a.app.folder_strip.scroll
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_folder_tab_that_is_half_out_of_the_row_can_still_be_pressed() {
    let (mut a, dir) = with_folder_tabs("rhumb-strip-edge", 22);
    a.app.switch_folder(0);
    a.frame();
    let i = (0..22)
        .find(|i| {
            let r = a.app.folder_strip.rects[*i];
            r.right() > 1100.0 && r.left() < 1100.0
        })
        .expect("a tab is cut off");
    let r = a.app.folder_strip.rects[i];
    let at = Pos2::new(r.left() + 20.0, r.center().y);
    press_at(&mut a, at, egui::PointerButton::Primary);
    assert_eq!(a.app.active_folder, i);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cycling_folder_tabs_always_brings_the_tab_into_view() {
    let (mut a, dir) = with_folder_tabs("rhumb-strip-cycle", 22);
    for _ in 0..30 {
        press(&mut a, egui::Key::Tab, egui::Modifiers::CTRL);
        a.frame();
        let r = a.app.folder_strip.rects[a.app.active_folder];
        assert!(
            r.left() >= -0.5 && r.right() <= 1160.0,
            "tab {} at {r:?}",
            a.app.active_folder
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn closing_folder_tabs_while_slid_keeps_the_row_in_range() {
    let (mut a, dir) = with_folder_tabs("rhumb-strip-close-slid", 22);
    a.app.switch_folder(21);
    a.frame();
    assert!(a.app.folder_strip.scroll > 0.0);
    for _ in 0..14 {
        let last = a.app.folder_tab_count() - 1;
        a.app.close_folder_tab(last);
        a.frame();
    }
    assert_eq!(a.app.folder_tab_count(), 8);
    assert_eq!(a.app.folder_strip.scroll, 0.0, "everything fits again");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn an_idle_row_does_not_drift() {
    let (mut a, dir) = with_folder_tabs("rhumb-strip-idle", 22);
    a.app.switch_folder(11);
    for _ in 0..5 {
        a.frame();
    }
    let s = a.app.folder_strip.scroll;
    for _ in 0..100 {
        a.frame();
    }
    assert_eq!(a.app.folder_strip.scroll, s);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn every_folder_tab_has_room_to_show_its_name() {
    let (a, dir) = with_folder_tabs("rhumb-strip-widths", 6);
    for r in &a.app.folder_strip.rects {
        assert!(r.width() >= 100.0, "{r:?}");
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn pressing_the_folder_tab_that_is_already_in_front_does_nothing_harmful() {
    let (mut a, dir) = with_folder_tabs("rhumb-strip-same", 3);
    let at = folder_tab_at(&a, 2);
    for _ in 0..4 {
        press_at(&mut a, at, egui::PointerButton::Primary);
    }
    assert_eq!(a.app.active_folder, 2);
    assert_eq!(a.app.cwd, dir.join("t2"));
    assert_eq!(folder_labels(&a), vec!["t0", "t1", "t2"]);
    let _ = fs::remove_dir_all(&dir);
}

// the files above the editor

fn with_files(name: &str, n: usize) -> (App, PathBuf) {
    let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for i in 0..n {
        fs::write(dir.join(format!("f{i:02}.txt")), format!("file {i}")).unwrap();
    }
    let mut a = App::new(&dir);
    listed(&mut a);
    for i in 0..n {
        open_file(&mut a, &dir.join(format!("f{i:02}.txt")));
    }
    for _ in 0..4 {
        a.frame();
    }
    (a, dir)
}

#[test]
fn pressing_a_file_tab_brings_that_file_forward() {
    let (mut a, dir) = with_files("rhumb-files-click", 4);
    assert_eq!(a.app.tabs.active, 3);
    let at = file_tab_at(&a, 1);
    press_at(&mut a, at, egui::PointerButton::Primary);
    assert_eq!(a.app.tabs.active, 1);
    a.frame();
    assert_eq!(a.text(), "file 1");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_row_of_files_sits_above_the_editor_inside_its_pane() {
    let (a, dir) = with_files("rhumb-files-place", 2);
    let strip = a.app.doc_strip.rects[0];
    let folders = a.app.folder_strip.rects[0];
    assert!(strip.top() > folders.bottom(), "below the folder tabs");
    assert!(
        strip.left() > 100.0,
        "inside the editor pane, not along the whole window"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_cross_on_a_file_tab_closes_that_file() {
    let (mut a, dir) = with_files("rhumb-files-cross", 3);
    let r = a.app.doc_strip.rects[1];
    let cross = Pos2::new(r.right() - 11.0, r.center().y);
    press_at(&mut a, cross, egui::PointerButton::Primary);
    assert_eq!(file_labels(&a), vec!["f00.txt", "f02.txt"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_middle_button_closes_a_file_tab() {
    let (mut a, dir) = with_files("rhumb-files-middle", 3);
    let at = file_tab_at(&a, 0);
    press_at(&mut a, at, egui::PointerButton::Middle);
    assert_eq!(file_labels(&a), vec!["f01.txt", "f02.txt"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn file_tabs_can_be_dragged_into_another_order() {
    let (mut a, dir) = with_files("rhumb-files-drag", 3);
    let from = file_tab_at(&a, 0);
    let to = file_tab_at(&a, 2);
    drag_between(&mut a, from, to);
    assert_eq!(file_labels(&a), vec!["f01.txt", "f02.txt", "f00.txt"]);
    assert_eq!(a.app.doc().unwrap().file_name(), "f00.txt");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_file_with_unsaved_changes_asks_before_its_cross_closes_it() {
    let (mut a, dir) = with_files("rhumb-files-dirty-cross", 2);
    a.ctx.memory_mut(|m| m.request_focus(Id::new(codeedit::ID)));
    a.frame();
    a.type_char("Q");
    a.frame();
    let i = a.app.tabs.active;
    let r = a.app.doc_strip.rects[i];
    let cross = Pos2::new(r.right() - 11.0, r.center().y);
    press_at(&mut a, cross, egui::PointerButton::Primary);
    assert!(matches!(a.app.dialog, Dialog::Unsaved { .. }));
    assert_eq!(a.app.tabs.len(), 2, "and it is still there");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn many_files_slide_in_their_row_and_the_one_in_front_is_in_view() {
    let (mut a, dir) = with_files("rhumb-files-overflow", 16);
    let pane_left = a.app.doc_strip.rects[0].left().min(0.0);
    let _ = pane_left;
    let r = a.app.doc_strip.rects[a.app.tabs.active];
    let area_right = a.app.preview_rect.right().max(1160.0);
    assert!(r.right() <= area_right + 1.0, "{r:?}");
    a.app.focus_tab(0);
    a.frame();
    assert_eq!(a.app.doc_strip.scroll, 0.0);
    a.app.focus_tab(15);
    a.frame();
    assert!(a.app.doc_strip.scroll > 0.0);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_files_row_and_the_folder_row_do_not_share_widgets() {
    // Both rows hold a tab at the same place in their order, and pressing one must not be
    // taken for pressing the other.
    let (mut a, dir) = with_files("rhumb-files-separate", 3);
    a.app.new_folder_tab();
    a.app.switch_folder(0);
    let at = file_tab_at(&a, 0);
    press_at(&mut a, at, egui::PointerButton::Primary);
    assert_eq!(a.app.active_folder, 0, "the folder tab did not change");
    assert_eq!(a.app.tabs.active, 0);
    let _ = fs::remove_dir_all(&dir);
}

// ---- grouping and filters ---------------------------------------------------

/// A folder with one of each kind of file, plus a folder, so the filters and
/// groupings have something to sort.
fn filter_workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("folder")).unwrap();
    fs::write(dir.join("notes.txt"), b"x").unwrap();
    fs::write(dir.join("photo.png"), b"x").unwrap();
    fs::write(dir.join("song.mp3"), b"x").unwrap();
    fs::write(dir.join("clip.mp4"), b"x").unwrap();
    fs::write(dir.join("pack.zip"), b"x").unwrap();
    dir
}

/// The names in the order the list shows them, not sorted.
fn listed_names(a: &App) -> Vec<String> {
    a.app
        .visible
        .iter()
        .filter_map(|i| a.app.entries.get(*i))
        .map(|e| e.name.clone())
        .collect()
}

#[test]
fn a_kind_filter_narrows_the_list_and_combines_with_the_name() {
    let dir = filter_workspace("rhumb-filter-kind");
    let mut a = App::new(&dir);
    listed(&mut a);
    assert_eq!(a.app.row_count(), 6, "a folder and five files");

    a.app.kind_filter = KindFilter::Images;
    a.app.recompute_visible();
    assert_eq!(a.app.row_count(), 1);
    assert_eq!(shown_names(&a), vec!["photo.png"]);

    // The name box still applies on top of the kind filter.
    a.app.filter = "photo".into();
    a.app.recompute_visible();
    assert_eq!(a.app.row_count(), 1);
    a.app.filter = "song".into();
    a.app.recompute_visible();
    assert_eq!(a.app.row_count(), 0, "song is not an image");

    // Folders are their own choice.
    a.app.filter.clear();
    a.app.kind_filter = KindFilter::Folders;
    a.app.recompute_visible();
    assert_eq!(shown_names(&a), vec!["folder"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_size_filter_bands_files_and_never_hides_a_folder() {
    let dir = filter_workspace("rhumb-filter-size");
    let mut a = App::new(&dir);
    listed(&mut a);
    a.app.size_filter = SizeFilter::Large;
    a.app.recompute_visible();
    // No file here reaches 100 MB, but the folder is exempt and stays.
    assert_eq!(shown_names(&a), vec!["folder"]);
    a.app.size_filter = SizeFilter::Small;
    a.app.recompute_visible();
    assert_eq!(a.app.row_count(), 6, "every small file and the folder");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_date_filter_drops_files_older_than_the_choice() {
    let dir = filter_workspace("rhumb-filter-date");
    let old = dir.join("old.txt");
    fs::write(&old, b"x").unwrap();
    let f = fs::OpenOptions::new().write(true).open(&old).unwrap();
    f.set_modified(SystemTime::now() - Duration::from_secs(60 * 86_400))
        .unwrap();
    let mut a = App::new(&dir);
    listed(&mut a);
    a.app.date_filter = DateFilter::Last30;
    a.app.recompute_visible();
    assert!(!shown_names(&a).contains(&"old.txt".to_owned()));
    assert!(shown_names(&a).contains(&"notes.txt".to_owned()));
    a.app.date_filter = DateFilter::Any;
    a.app.recompute_visible();
    assert!(shown_names(&a).contains(&"old.txt".to_owned()));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn grouping_by_name_orders_the_list_into_letter_buckets_with_headers() {
    let dir = std::env::temp_dir().join(format!("rhumb-group-name-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for name in ["apple.txt", "banana.txt", "cherry.txt", "1file.txt"] {
        fs::write(dir.join(name), b"x").unwrap();
    }
    let mut a = App::new(&dir);
    listed(&mut a);
    a.app.group_by = GroupBy::Name;
    a.app.recompute_visible();
    let labels: Vec<&str> = a.app.groups.iter().map(|g| g.label.as_str()).collect();
    assert_eq!(labels, vec!["#", "A", "B", "C"], "{labels:?}");
    // Headers are extra rows, not extra items: the items are unchanged.
    assert_eq!(a.app.row_count(), 4);
    assert_eq!(
        a.app.groups.iter().map(|g| g.len).sum::<usize>(),
        a.app.row_count()
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn grouping_gathers_interleaved_kinds_together_and_keeps_the_sort_inside_them() {
    // Sorted by name the kinds alternate: a.txt, b.png, c.txt. Grouping by
    // type has to pull the two documents together, keeping their name order.
    let dir = std::env::temp_dir().join(format!("rhumb-group-type-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for name in ["a.txt", "b.png", "c.txt"] {
        fs::write(dir.join(name), b"x").unwrap();
    }
    let mut a = App::new(&dir);
    listed(&mut a);
    a.app.group_by = GroupBy::Type;
    a.app.recompute_visible();
    let labels: Vec<&str> = a.app.groups.iter().map(|g| g.label.as_str()).collect();
    assert_eq!(labels, vec!["Documents", "Images"], "{labels:?}");
    assert_eq!(listed_names(&a), vec!["a.txt", "c.txt", "b.png"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn grouping_draws_headers_in_every_view() {
    let dir = filter_workspace("rhumb-group-views");
    let mut a = App::new(&dir);
    listed(&mut a);
    a.app.group_by = GroupBy::Type;
    a.app.recompute_visible();
    assert!(a.app.groups.len() >= 2, "more than one kind is listed");
    for view in ViewMode::ALL {
        a.app.set_view(view);
        for _ in 0..3 {
            a.frame();
        }
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_grouping_and_filters_round_trip_through_prefs() {
    let dir = filter_workspace("rhumb-prefs-group");
    let mut a = App::new(&dir);
    a.app.group_by = GroupBy::Type;
    a.app.kind_filter = KindFilter::Images;
    a.app.date_filter = DateFilter::Last7;
    a.app.size_filter = SizeFilter::Large;
    // A prefs file of this test's own, so it never races another.
    let prefs = std::env::temp_dir()
        .join(format!("rhumb-prefs-roundtrip-{}", std::process::id()))
        .join("prefs.txt");
    let _ = fs::remove_dir_all(prefs.parent().unwrap());
    PREFS_OVERRIDE.with(|p| *p.borrow_mut() = Some(prefs.clone()));
    a.app.write_prefs();
    let text = fs::read_to_string(&prefs).unwrap();
    assert!(text.contains("group=type"), "{text}");
    assert!(text.contains("kind=images"), "{text}");
    assert!(text.contains("date=last7"), "{text}");
    assert!(text.contains("size=large"), "{text}");
    // A fresh app reading that text gets the choices back.
    let mut b = App::new(&dir);
    b.app.apply_prefs_text(&text, None);
    assert_eq!(b.app.group_by, GroupBy::Type);
    assert_eq!(b.app.kind_filter, KindFilter::Images);
    assert_eq!(b.app.date_filter, DateFilter::Last7);
    assert_eq!(b.app.size_filter, SizeFilter::Large);
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(prefs.parent().unwrap());
}

/// A folder `dir` with a few files, and a subfolder `sub` with one file of its
/// own, so the two panes of a dual view start at folders that both list.
fn nested_folders(name: &str) -> (PathBuf, PathBuf) {
    let (dir, _) = workspace(name, 4, 2);
    let sub = dir.join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("inner.txt"), "inner").unwrap();
    (dir, sub)
}

/// Opens `dir`, steps into `sub`, and turns the dual view on with both lists
/// settled, which every dual-pane test starts from.
fn dual_view(name: &str) -> (App, PathBuf, PathBuf) {
    let (dir, sub) = nested_folders(name);
    let mut a = App::new(&dir);
    a.app.navigate(&sub);
    a.settle(|a| matches!(a.app.listing, Listing::Ready));
    a.app.toggle_dual();
    a.settle(|a| {
        a.app
            .second
            .as_ref()
            .is_some_and(|s| matches!(s.listing, Listing::Ready))
    });
    (a, dir, sub)
}

#[test]
fn toggling_dual_pane_shows_two_lists_and_the_second_lists_another_folder() {
    let (a, dir, sub) = dual_view("rhumb-dual-on");
    assert!(a.app.dual, "the view is on");
    let second = a.app.second.as_ref().expect("a second list was made");
    assert_ne!(
        second.cwd, a.app.cwd,
        "the second list is a different folder"
    );
    assert_eq!(second.cwd, dir, "it starts in the folder above the first");
    assert!(!second.entries.is_empty(), "and it listed what is there");
    assert!(!a.app.entries.is_empty(), "the first list is still there");
    assert_eq!(a.app.cwd, sub);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn navigating_the_second_pane_leaves_the_first_alone() {
    let (mut a, dir, sub) = dual_view("rhumb-dual-navigate");
    // A click in the second pane makes it the live one, on the side it was
    // drawn on; the first is parked.
    a.app.activate_second();
    assert!(
        !a.app.live_on_left,
        "the clicked pane is live on its own side"
    );
    assert_eq!(a.app.cwd, dir, "the second list is now the live one");

    let first = a.app.second.as_ref().unwrap();
    assert_eq!(first.cwd, sub, "the first list kept its folder");
    let first_sel = first.sel.clone();

    // Move the live (second) list somewhere else.
    a.app.navigate(&sub);
    a.settle(|a| matches!(a.app.listing, Listing::Ready));

    let first = a.app.second.as_ref().unwrap();
    assert_eq!(first.cwd, sub, "the parked first list did not move");
    assert_eq!(first.sel, first_sel, "nor did its selection");
    assert_eq!(a.app.cwd, sub, "the live list did move");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_paste_while_the_second_pane_is_live_lands_in_its_folder() {
    let (mut a, dir, sub) = dual_view("rhumb-dual-paste");
    // A file in the first list's folder, put on the clipboard as if copied.
    let source = sub.join("move_me.txt");
    fs::write(&source, "payload").unwrap();
    a.app.clip = Some(ops::Clipboard {
        paths: vec![source.clone()],
        cut: false,
    });

    // The second pane is now live, and its folder is `dir`.
    a.app.activate_second();
    assert_eq!(a.app.cwd, dir);
    a.app.paste();
    a.settle(|a| a.app.jobs.is_empty());

    assert!(
        dir.join("move_me.txt").exists(),
        "the copy landed in the second pane's folder"
    );
    assert!(source.exists(), "the original is still there");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn toggling_dual_off_returns_to_the_single_list() {
    let (mut a, dir, sub) = dual_view("rhumb-dual-off");
    a.app.toggle_dual();
    assert!(!a.app.dual, "off again");
    for _ in 0..3 {
        a.frame();
    }
    assert_eq!(a.app.cwd, sub, "the live list is where it was");
    assert!(
        a.app.entries.iter().any(|e| e.name == "inner.txt"),
        "and it is still listed"
    );
    // The parked list is kept, so turning the view back on finds it as it was.
    assert!(a.app.second.is_some());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn dual_pane_state_survives_a_prefs_round_trip() {
    let (dir, _) = workspace("rhumb-dual-prefs", 3, 2);
    let mut a = App::new(&dir);
    a.app.dual = true;
    a.app.dual_split = 0.37;
    // A prefs file of this test's own, so it never races another.
    let prefs = std::env::temp_dir()
        .join(format!("rhumb-prefs-dual-{}", std::process::id()))
        .join("prefs.txt");
    let _ = fs::remove_dir_all(prefs.parent().unwrap());
    PREFS_OVERRIDE.with(|p| *p.borrow_mut() = Some(prefs.clone()));
    a.app.write_prefs();
    let text = fs::read_to_string(&prefs).unwrap();
    assert!(text.contains("dual=true"), "{text}");

    a.app.dual = false;
    a.app.dual_split = 0.5;
    a.app.apply_prefs_text(&text, None);
    assert!(a.app.dual, "the dual view came back");
    assert!(
        (a.app.dual_split - 0.37).abs() < 0.001,
        "and where the divider was"
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(prefs.parent().unwrap());
}

// ---- batch rename -------------------------------------------------------------

#[test]
fn a_batch_rename_applies_a_pattern_to_every_selected_file() {
    let dir = std::env::temp_dir().join(format!("rhumb-batch-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for n in ["alpha.txt", "beta.txt", "gamma.txt"] {
        fs::write(dir.join(n), n).unwrap();
    }
    let mut a = App::new(&dir);
    a.frame();
    // Three selected files, the way a reader gets there with Ctrl+click.
    a.app.sel = ["alpha.txt", "beta.txt", "gamma.txt"]
        .iter()
        .map(|n| dir.join(n))
        .collect();
    let selected: Vec<PathBuf> = a.app.sel.iter().cloned().collect();
    a.app.start_batch_rename(selected);
    assert!(
        matches!(a.app.dialog, Dialog::BatchRename { .. }),
        "more than one item opens the batch dialog"
    );
    a.app.dialog = Dialog::None;

    let selected: Vec<PathBuf> = a.app.sel.iter().cloned().collect();
    a.app
        .apply_batch_rename(&selected, "{name}_v{n}.{ext}", "1");
    listed(&mut a);

    for n in ["alpha_v1.txt", "beta_v2.txt", "gamma_v3.txt"] {
        assert!(
            dir.join(n).is_file(),
            "{n} should exist: {:?}",
            shown_names(&a)
        );
    }
    assert!(!dir.join("alpha.txt").exists(), "the old names are gone");
    // The renamed items are the selection now.
    assert_eq!(a.app.sel.len(), 3);
    assert!(a.app.sel.contains(&dir.join("beta_v2.txt")));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_batch_rename_shifts_names_that_collide_with_each_other() {
    let dir = std::env::temp_dir().join(format!("rhumb-batch-shift-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for (n, body) in [("1.txt", "one"), ("2.txt", "two"), ("3.txt", "three")] {
        fs::write(dir.join(n), body).unwrap();
    }
    let mut a = App::new(&dir);
    a.frame();
    let paths: Vec<PathBuf> = ["1.txt", "2.txt", "3.txt"]
        .iter()
        .map(|n| dir.join(n))
        .collect();
    // 1 -> 2, 2 -> 3, 3 -> 4: every target but the last is a name that was
    // still on disk when the batch began, which only works because each source
    // is moved to a temporary name before any final name is written.
    a.app.apply_batch_rename(&paths, "{n}.txt", "2");
    listed(&mut a);
    assert_eq!(fs::read_to_string(dir.join("2.txt")).unwrap(), "one");
    assert_eq!(fs::read_to_string(dir.join("3.txt")).unwrap(), "two");
    assert_eq!(fs::read_to_string(dir.join("4.txt")).unwrap(), "three");
    assert!(!dir.join("1.txt").exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_batch_rename_leaves_an_existing_name_it_would_have_overwritten() {
    let dir = std::env::temp_dir().join(format!("rhumb-batch-taken-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("a.txt"), "a").unwrap();
    fs::write(dir.join("b.txt"), "b").unwrap();
    // Not part of the batch, and not to be touched.
    fs::write(dir.join("a_1.txt"), "keep").unwrap();
    let mut a = App::new(&dir);
    a.frame();
    let paths = vec![dir.join("a.txt"), dir.join("b.txt")];
    a.app.apply_batch_rename(&paths, "{name}_{n}.{ext}", "1");
    listed(&mut a);
    assert_eq!(
        fs::read_to_string(dir.join("a_1.txt")).unwrap(),
        "keep",
        "the file that was already there is untouched"
    );
    assert!(
        dir.join("a.txt").exists(),
        "the rename that would have clobbered it was skipped"
    );
    assert!(
        dir.join("b_2.txt").is_file(),
        "the free one went through, numbered by its place"
    );
    assert!(!dir.join("b.txt").exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_batch_rename_dialog_draws_and_previews_without_trouble() {
    let (dir, _) = workspace("rhumb-batch-dialog", 5, 10);
    let mut a = App::new(&dir);
    a.frame();
    let paths: Vec<PathBuf> = (0..3)
        .map(|i| dir.join(format!("file_{i:05}.txt")))
        .collect();
    a.app.dialog = Dialog::BatchRename {
        paths,
        pattern: "{name}_{n:2}".into(),
        start: "1".into(),
    };
    for _ in 0..3 {
        a.frame();
    }
    assert!(
        matches!(a.app.dialog, Dialog::BatchRename { .. }),
        "it stays up while it is being filled in"
    );
    let _ = fs::remove_dir_all(&dir);
}

// ---- links --------------------------------------------------------------------

#[test]
#[cfg(windows)]
fn a_junction_is_created_beside_a_folder() {
    let dir = std::env::temp_dir().join(format!("rhumb-junction-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let target = dir.join("target");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("inside.txt"), b"x").unwrap();
    let mut a = App::new(&dir);
    a.app.create_junction(&target);
    let link = dir.join("target - link");
    if !link.exists() {
        // `mklink /J` normally needs no privilege, but a locked-down machine can
        // still refuse it. Say so and move on rather than fail the run.
        eprintln!("junction creation was refused on this machine; skipping the checks");
        let _ = fs::remove_dir_all(&dir);
        return;
    }
    assert!(link.is_dir(), "a junction reads as a directory");
    assert!(
        link.join("inside.txt").is_file(),
        "and reaches what it points at"
    );
    // A junction is a reparse point: its own metadata is a directory, and
    // removing the link must leave the target alone.
    let md = fs::symlink_metadata(&link).unwrap();
    assert!(md.file_type().is_symlink() || md.is_dir());
    let _ = fs::remove_dir(&link);
    assert!(target.join("inside.txt").is_file(), "the target survives");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_symbolic_link_is_created_beside_a_file_when_the_system_allows_it() {
    let dir = std::env::temp_dir().join(format!("rhumb-symlink-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("note.txt");
    fs::write(&file, b"hello").unwrap();
    let mut a = App::new(&dir);
    a.app.create_symlink(&file);
    let link = dir.join("note.txt - link");
    if !link.exists() {
        // A symbolic link needs Developer Mode or elevation on Windows; this is
        // the tolerant branch for a machine that has neither.
        eprintln!("symbolic links are not permitted here; skipping the checks");
        let _ = fs::remove_dir_all(&dir);
        return;
    }
    assert_eq!(fs::read_to_string(&link).unwrap(), "hello");
    let _ = fs::remove_file(&link);
    let _ = fs::remove_dir_all(&dir);
}

// ---- searching a virtual location ------------------------------------------

/// A made-up entry, so a test of the Recycle Bin or This PC does not depend on
/// what happens to be in the real one on the machine running it.
fn virtual_entry(root: &str, name: &str) -> fs_model::Entry {
    fs_model::Entry {
        name: name.to_owned(),
        path: PathBuf::from(root).join(name),
        is_dir: false,
        is_symlink: false,
        size: 0,
        modified: None,
        hidden: false,
    }
}

/// An app showing a virtual root with two made-up entries.
///
/// The root is read for real once - the same call the listing worker makes - so
/// that "without error" is checked, but the rows themselves are made up so what
/// gets filtered does not depend on the machine's own recycle bin or drives.
fn virtual_listing(name: &str, root: &str) -> (App, PathBuf) {
    let (dir, _) = workspace(name, 3, 5);
    let mut a = App::new(&dir);
    assert!(
        fs_model::read_dir(Path::new(root), false).is_ok(),
        "the virtual root reads without error"
    );
    a.app.cwd = PathBuf::from(root);
    a.app.entries = vec![
        virtual_entry(root, "report.txt"),
        virtual_entry(root, "notes.md"),
    ];
    a.app.listing = Listing::Ready;
    a.app.recompute_visible();
    (a, dir)
}

#[test]
fn filtering_the_recycle_bin_narrows_in_place_without_starting_a_search() {
    let (mut a, dir) = virtual_listing("rhumb-recycle-filter", crate::recycle::ROOT);
    let indexed_before = a.app.indexes.len();
    a.app.scope = SearchScope::Below;
    a.app.filter = "report".into();
    a.app.on_filter_changed();
    // A search that is due, so the only thing that can stop it is the virtual
    // root itself.
    a.app.search_typed = Some(Instant::now() - Duration::from_secs(5));
    for _ in 0..5 {
        a.frame();
    }
    assert_eq!(
        shown_names(&a),
        vec!["report.txt"],
        "the filter narrowed it"
    );
    assert!(!a.app.search.running, "no walk of the bin was started");
    assert!(!a.app.searching(), "the list is filtered in place");
    assert_eq!(
        a.app.indexes.len(),
        indexed_before,
        "and nothing was indexed"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn filtering_this_pc_narrows_in_place_without_starting_a_search() {
    let (mut a, dir) = virtual_listing("rhumb-thispc-filter", crate::this_pc::ROOT);
    let indexed_before = a.app.indexes.len();
    a.app.scope = SearchScope::Below;
    a.app.filter = "notes".into();
    a.app.on_filter_changed();
    a.app.search_typed = Some(Instant::now() - Duration::from_secs(5));
    for _ in 0..5 {
        a.frame();
    }
    assert_eq!(shown_names(&a), vec!["notes.md"]);
    assert!(!a.app.search.running, "no walk of This PC was started");
    assert!(!a.app.searching(), "the list is filtered in place");
    assert_eq!(
        a.app.indexes.len(),
        indexed_before,
        "and nothing was indexed"
    );
    let _ = fs::remove_dir_all(&dir);
}
