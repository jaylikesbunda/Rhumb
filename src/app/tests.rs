use super::*;

fn file(name: &str) -> Tab {
    Tab::file(Path::new(name))
}

/// The split geometry as the app assembles it, over a real `Ui`, so a drag can
/// be driven through it with real events.
///
/// A copy rather than a call, because `split_ui` needs a whole `Rhumb` to reach.
/// The parts that matter are copied exactly: the two child rects, the divider,
/// and the one line that writes the fraction back - because the bug this is
/// looking for is a disagreement between where the divider is *drawn* and where
/// the drag *thinks* it is, and that disagreement is invisible in the geometry
/// functions alone.
struct Split {
    split: f32,
    /// Whether a drag has been seen, so a test can tell "not recognised" from
    /// "recognised and produced no movement".
    dragged: bool,
    left: Rect,
    divider: Rect,
    right: Rect,
}

impl Split {
    fn new(rect: Rect, split: f32) -> Split {
        let divider_w = 7.0f32;
        let usable = (rect.width() - divider_w).max(1.0);
        let left_w = split_left(usable, split);
        let left = Rect::from_min_size(rect.min, Vec2::new(left_w, rect.height()));
        let divider = Rect::from_min_size(
            Pos2::new(rect.min.x + left_w, rect.min.y),
            Vec2::new(divider_w, rect.height()),
        );
        let right = Rect::from_min_max(Pos2::new(divider.max.x, rect.min.y), rect.max);
        Split {
            split,
            dragged: false,
            left,
            divider,
            right,
        }
    }

    /// The one line that moves the divider, copied verbatim.
    fn drag(&mut self, resp: &egui::Response, usable: f32) {
        self.dragged |= resp.dragged();
        if resp.dragged() {
            let left_w = self.left.width();
            self.split = split_fraction(usable, left_w + resp.drag_delta().x);
        }
    }

    /// The next frame's geometry, worked out from the stored fraction exactly as
    /// the top of `split_ui` does.
    fn next_frame(&mut self, panel: Rect) {
        let divider_w = 7.0f32;
        let usable = (panel.width() - divider_w).max(1.0);
        let left_w = split_left(usable, self.split);
        self.left = Rect::from_min_size(panel.min, Vec2::new(left_w, panel.height()));
        self.divider = Rect::from_min_size(
            Pos2::new(panel.min.x + left_w, panel.min.y),
            Vec2::new(divider_w, panel.height()),
        );
        self.right = Rect::from_min_max(Pos2::new(self.divider.max.x, panel.min.y), panel.max);
    }
}

/// One idle frame, so the divider has been drawn once and egui has it in its
/// hit list.
///
/// Not a detail: a widget becomes hoverable on the frame *after* it is drawn,
/// because egui resolves hovers against the previous pass's rectangles. A real
/// window draws the divider every frame, so by the time a reader's finger is on
/// it the divider has been there for many frames; a test that presses on the
/// very first frame is pressing a widget egui has never seen, and no drag will
/// ever be recognised however correct the drag handling is.
/// One frame that draws the widgets, so egui has them in its hit list.
///
/// Not a detail, and the reason this harness exists at all: a widget becomes
/// hoverable on the frame *after* it is drawn, because egui resolves hovers
/// against the previous pass's rectangles. A real window draws the divider on
/// every frame, so by the time a reader's finger is on it the divider has been
/// there for many frames. A test that presses on the very first frame is
/// pressing a widget egui has never seen, and no drag will be recognised
/// however correct the drag handling is.
fn warm_up(ctx: &Context, s: &mut Split) {
    let out = ctx.run_ui(egui::RawInput::default(), |ui| {
        let _ = ui.interact(s.left, Id::new("editor"), Sense::click_and_drag());
        let _ = ui.interact(
            s.divider,
            Id::new("split"),
            Sense::hover().union(Sense::drag()),
        );
    });
    out.drop_without_applying_deltas();
}

/// The events for one frame of a drag: a move always, and a button press or
/// release only on the frames that have one.
///
/// Sending a release on every frame - which is what "pressed on the first frame,
/// false on the rest" reads like - lets go of the button immediately, and a drag
/// that is not being held is not a drag. A platform sends one press and one
/// release and nothing in between, and so does this.
fn button_step(p: Pos2, pressed: bool) -> Vec<egui::Event> {
    if pressed {
        vec![
            egui::Event::PointerMoved(p),
            egui::Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
        ]
    } else {
        vec![egui::Event::PointerMoved(p)]
    }
}

/// The events for the last frame of a drag, which is the release.
fn release_step(p: Pos2) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(p),
        egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        },
    ]
}

/// Drives `Split` through a drag with real events and reports the divider's
/// left edge at the end.
fn drag_the_divider(panel: Rect, dx: f32) -> f32 {
    let divider_w = 7.0f32;
    let usable = (panel.width() - divider_w).max(1.0);
    let ctx = Context::default();
    ctx.set_fonts(theme::fonts());
    let mut s = Split::new(panel, 0.5);
    let start = Pos2::new(s.divider.center().x, s.divider.center().y);
    warm_up(&ctx, &mut s);
    // Press on the divider, move in steps, release.
    let steps = 8;
    for step in 0..=steps {
        let x = start.x + dx * (step as f32 / steps as f32);
        let p = Pos2::new(x, start.y);
        let pressed = step == 0;
        let out = ctx.run_ui(
            egui::RawInput {
                events: button_step(p, pressed),
                ..Default::default()
            },
            |ui| {
                // Exactly as `split_ui` assembles it: the two pane children are
                // created *first*, each claiming its own rect and its own
                // interaction, and only then is the divider interacted on the
                // parent. The order matters, because egui gives a press to one
                // widget and this is testing which one.
                let left_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(s.left)
                        .id_salt("editor-pane"),
                );
                let _editor = left_ui.interact(s.left, Id::new("editor"), Sense::click_and_drag());
                let right_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(s.right)
                        .id_salt("preview-pane"),
                );
                let _ = right_ui.max_rect();
                let resp = ui.interact(
                    s.divider,
                    Id::new("split"),
                    Sense::hover().union(Sense::drag()),
                );
                s.drag(&resp, usable);
                s.next_frame(panel);
            },
        );
        out.drop_without_applying_deltas();
    }
    let out = ctx.run_ui(
        egui::RawInput {
            events: release_step(Pos2::new(start.x + dx, start.y)),
            ..Default::default()
        },
        |ui| {
            let resp = ui.interact(
                s.divider,
                Id::new("split"),
                Sense::hover().union(Sense::drag()),
            );
            s.drag(&resp, usable);
            s.next_frame(panel);
        },
    );
    out.drop_without_applying_deltas();
    s.divider.left()
}

#[test]
fn the_editor_panel_width_follows_the_drag_and_respects_both_limits() {
    // Room for the panel and the list together is 1000.
    assert_eq!(doc_width(500.0, 1000.0), 500.0);
    assert_eq!(doc_width(100.0, 1000.0), DOC_MIN);
    assert_eq!(doc_width(990.0, 1000.0), 1000.0 - LIST_MIN);
    // A window too small for both keeps the panel at its minimum rather than
    // going below it or panicking on an inverted range.
    assert_eq!(doc_width(500.0, 300.0), DOC_MIN);
}

#[test]
fn dragging_the_divider_moves_it_and_it_stays_where_it_was_left() {
    // The complaint this pins down: grab the divider between the editor and the
    // preview, move it, and it springs back to where it was. Which is what a
    // drag whose *result* is written from a position that is recomputed from
    // something else looks like - the divider is drawn one place and the number
    // that decides where it is drawn comes from another.
    let panel = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(800.0, 600.0));
    let start = drag_the_divider(panel, 0.0);
    // Drag it 120 points to the right.
    let moved = drag_the_divider(panel, 120.0);
    assert!(
        moved > start + 60.0,
        "dragging the divider 120pt right moved it from {start:.1} to {moved:.1}, \
         which is much less than the 120pt the pointer moved"
    );
    // And dragging it left moves it left, which is the direction a spring-back
    // bug hides in, because a value written from a stale width goes back to the
    // old width whichever way it was dragged.
    let back = drag_the_divider(panel, -120.0);
    assert!(
        back < start - 60.0,
        "dragging the divider 120pt left moved it from {start:.1} to {back:.1}"
    );
}

#[test]
fn the_divider_keeps_its_position_while_the_drag_is_held() {
    // The same drag, frame by frame, and checked *during* the gesture. A divider
    // that follows the pointer while it is held and springs back on release is a
    // different fault from one that never moves, and it is only visible in the
    // frames in between.
    let panel = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(800.0, 600.0));
    let divider_w = 7.0f32;
    let usable = panel.width() - divider_w;
    let ctx = Context::default();
    ctx.set_fonts(theme::fonts());
    let mut s = Split::new(panel, 0.5);
    let start_x = s.divider.center().x;
    let y = s.divider.center().y;
    warm_up(&ctx, &mut s);
    let mut seen: Vec<f32> = Vec::new();
    for step in 0..=8 {
        let x = start_x + 120.0 * (step as f32 / 8.0);
        let p = Pos2::new(x, y);
        let out = ctx.run_ui(
            egui::RawInput {
                events: button_step(p, step == 0),
                ..Default::default()
            },
            |ui| {
                let resp = ui.interact(
                    s.divider,
                    Id::new("split"),
                    Sense::hover().union(Sense::drag()),
                );
                s.drag(&resp, usable);
                s.next_frame(panel);
            },
        );
        out.drop_without_applying_deltas();
        seen.push(s.divider.left());
    }
    let out = ctx.run_ui(
        egui::RawInput {
            events: release_step(Pos2::new(start_x + 120.0, y)),
            ..Default::default()
        },
        |ui| {
            let resp = ui.interact(
                s.divider,
                Id::new("split"),
                Sense::hover().union(Sense::drag()),
            );
            s.drag(&resp, usable);
        },
    );
    out.drop_without_applying_deltas();
    // Strictly increasing while the pointer moves right, which is the shape of
    // "it followed the drag". A value that goes back to the start fails here.
    for pair in seen.windows(2) {
        assert!(
            pair[1] > pair[0],
            "the divider went back while the drag was still going: {seen:?}"
        );
    }
}
#[test]
fn the_divider_goes_where_the_pointer_goes_in_the_narrow_panel() {
    // The complaint: with the file list showing, the doc panel sits at its
    // default width - around 380 points - rather than filling the centre, and
    // dragging the divider there "just bounces back to its original width".
    //
    // The cause is the minimum pane size. Two 160-point panes and a 7-point
    // divider is 327 of a 380-point panel, which leaves 53 points for the
    // divider to travel in the whole panel. A drag of 100 points is therefore
    // clipped to about 26, and a reader watching a divider move a quarter of
    // the way and stop reads that as the divider refusing to move rather than
    // as a limit.
    //
    // So the test drags in a panel the size the panel actually is, and asks
    // the only question that matters: did the divider go where the pointer
    // went?
    let panel = Rect::from_min_size(Pos2::ZERO, Vec2::new(380.0, 600.0));
    let start = drag_the_divider(panel, 0.0);
    let right = drag_the_divider(panel, 100.0);
    let moved = right - start;
    assert!(
        moved > 70.0,
        "dragging the divider 100pt right in a 380pt panel moved it {moved:.0}pt. The \
         two minimum panes leave almost nowhere for it to go, so the drag is \
         clipped away and the divider springs back"
    );
    let back = drag_the_divider(panel, -100.0);
    // 100pt left is past the limit in a 380pt panel, so it stops there - but it
    // must have *moved*, and it must have stayed there. The failure this test
    // exists for is it ending up back at 186.
    assert!(
        back < start - 50.0,
        "dragging 100pt left from {start:.0} left the divider at {back:.0}: it \
         sprang back rather than moving"
    );
    assert!(
        (back - split_min_pane(380.0 - 7.0)).abs() < 1.0,
        "and it should have stopped at the limit, {lo:.1}",
        lo = split_min_pane(380.0 - 7.0)
    );
}

#[test]
fn a_narrow_panel_still_leaves_the_divider_somewhere_to_go() {
    // The same thing stated as a property of the limits, so the number is
    // pinned rather than implied: whatever the panel width, the divider must be
    // able to travel at least a quarter of it.
    for w in [220.0f32, 300.0, 380.0, 500.0, 800.0, 1400.0] {
        let usable = w - 7.0;
        let lo = split_left(usable, 0.0);
        let hi = split_left(usable, 1.0);
        let travel = hi - lo;
        assert!(
            travel > usable * 0.25,
            "in a {w:.0}pt panel the divider can only travel {travel:.0}pt, which is \
             less than a quarter of it"
        );
    }
}

#[test]
fn grabbing_the_divider_without_moving_it_changes_nothing() {
    // Every fraction, at every panel width: the pixel width on screen and
    // the stored fraction have to agree, or the split jumps on first drag.
    for usable in [373.0, 500.0, 640.0, 900.0, 1400.0] {
        for step in 0..=20 {
            let split = step as f32 / 20.0;
            let left = split_left(usable, split);
            let back = split_fraction(usable, left);
            assert!(
                (back * usable - left).abs() < 0.01,
                "usable {usable}, split {split}: drew at {left}px but stored {back}"
            );
        }
    }
}

#[test]
fn neither_pane_is_squeezed_below_the_limit() {
    // Wide panel: the limit is the real one, because 160 fits twice over.
    assert_eq!(split_left(1000.0, 0.0), SPLIT_MIN_PANE);
    assert_eq!(split_left(1000.0, 1.0), 1000.0 - SPLIT_MIN_PANE);
    // Narrow panel: a quarter each, so both panes still have a usable share and
    // - the reason this changed - the divider still has somewhere to go. Half
    // each would be 150 of 300, which is correct as a floor and useless as a
    // drag: it pins the divider to one place in the whole panel.
    assert_eq!(split_left(300.0, 0.0), 75.0);
    assert_eq!(split_left(300.0, 1.0), 225.0);
    // Absurdly narrow: still no negative or overlapping rects.
    assert_eq!(split_left(10.0, 0.5), 5.0);
    // And never a floor above half the panel, which would invert the two rects.
    for w in [1.0f32, 10.0, 60.0, 200.0] {
        let lo = split_left(w, 0.0);
        let hi = split_left(w, 1.0);
        assert!(
            lo >= 0.0 && lo <= hi && hi <= w,
            "a {w}pt panel produced limits {lo}..{hi}, which are not inside it"
        );
    }
}

#[test]
fn the_divider_follows_the_drag_the_way_it_looks() {
    // Dragging right widens the editor by the same number of pixels.
    let usable = 800.0;
    let start = split_left(usable, 0.5);
    let after = split_fraction(usable, start + 40.0);
    assert!((split_left(usable, after) - (start + 40.0)).abs() < 0.01);
    // And the limit still holds at the extremes.
    assert!(split_left(usable, split_fraction(usable, 99_999.0)) <= 800.0);
}

#[test]
fn every_menu_opening_gets_a_popup_id_egui_has_not_drawn() {
    // Reusing the id is what broke this: egui falls back to an older frame
    // when deciding whether a click closes a popup, so the second right
    // click of a session opened the menu and closed it in one frame.
    let mut m = MenuState::default();
    let first = m.id();
    m.open(Some(Pos2::new(10.0, 20.0)), PathBuf::from("/a"));
    let second = m.id();
    m.close();
    m.open(Some(Pos2::new(30.0, 40.0)), PathBuf::from("/b"));
    let third = m.id();
    assert_ne!(first, second, "the first opening reused the idle id");
    assert_ne!(second, third, "the second opening reused the first id");
}

#[test]
fn opening_is_a_one_shot_so_the_menu_can_be_dismissed() {
    let mut m = MenuState::default();
    assert!(!m.take_open(), "nothing asked for a menu yet");
    m.open(Some(Pos2::new(1.0, 1.0)), PathBuf::from("/a"));
    assert!(m.take_open(), "the click should open the menu");
    assert!(!m.take_open(), "opening was re-asserted on a later frame");
}

#[test]
fn reopening_the_menu_for_its_own_folder_is_a_no_op() {
    // The press opens the menu and the release that follows would open it
    // again. Without this guard the second opening burns a new popup id
    // and drops any hover state inside the menu.
    let mut m = MenuState::default();
    assert!(m.open(Some(Pos2::new(5.0, 6.0)), PathBuf::from("/work")));
    let id = m.id();
    assert!(!m.open(Some(Pos2::new(7.0, 8.0)), PathBuf::from("/work")));
    assert_eq!(m.id(), id, "the id moved under an open menu");
    assert_eq!(m.anchor(), Some(Pos2::new(5.0, 6.0)), "the anchor moved");
    assert!(m.take_open(), "the first opening was lost");
}

#[test]
fn a_menu_keeps_its_path_and_anchor_until_it_closes() {
    let mut m = MenuState::default();
    m.open(Some(Pos2::new(5.0, 6.0)), PathBuf::from("/work"));
    // Frames pass with the menu up: the path has to survive, or the popup
    // is never drawn again and vanishes.
    for _ in 0..5 {
        assert_eq!(m.path(), Some(Path::new("/work")));
        assert_eq!(m.anchor(), Some(Pos2::new(5.0, 6.0)));
    }
    m.close();
    assert_eq!(m.path(), None);
    assert_eq!(m.anchor(), None);
    assert!(!m.take_open());
}

/// A frame of input for a mouse button at `at`.
#[test]
fn a_search_only_starts_when_one_is_queued_and_due() {
    // Nothing queued: this runs every frame, and starting anyway would
    // restart the walk and wipe the results as they arrive.
    assert!(!search_due(None, SEARCH_DEBOUNCE));
    // Queued but still inside the pause: wait, so typing does not thrash.
    assert!(!search_due(Some(Instant::now()), SEARCH_DEBOUNCE));
    // Queued and settled: go.
    assert!(search_due(
        Some(Instant::now() - SEARCH_DEBOUNCE - Duration::from_millis(20)),
        SEARCH_DEBOUNCE
    ));
}

#[test]
fn pins_keep_the_order_they_were_added_in() {
    let mut p = Pins::default();
    p.add(Path::new("/work/app"), 8).unwrap();
    p.add(Path::new("/work/notes"), 8).unwrap();
    p.add(Path::new("/home"), 8).unwrap();
    assert_eq!(
        *p,
        vec![
            PathBuf::from("/work/app"),
            PathBuf::from("/work/notes"),
            PathBuf::from("/home")
        ]
    );
}

#[test]
fn pinning_the_same_folder_twice_does_nothing() {
    let mut p = Pins::default();
    p.add(Path::new("/work"), 8).unwrap();
    // Same folder, spelled with a trailing separator and a detour.
    let again = p.add(Path::new("/work/./"), 8);
    assert!(matches!(again, Err(PinError::AlreadyThere)));
    assert_eq!(p.len(), 1);
    assert!(p.contains(Path::new("/work")));
}

#[test]
fn pins_stop_at_the_cap() {
    let mut p = Pins::default();
    p.add(Path::new("/a"), 2).unwrap();
    p.add(Path::new("/b"), 2).unwrap();
    assert!(matches!(p.add(Path::new("/c"), 2), Err(PinError::Full)));
    assert_eq!(p.len(), 2, "the rejected folder was not added");
    // Freeing a slot lets the next one in.
    assert!(p.remove(Path::new("/a")));
    p.add(Path::new("/c"), 2).unwrap();
    assert_eq!(*p, vec![PathBuf::from("/b"), PathBuf::from("/c")]);
}

#[test]
fn unpinning_says_whether_the_folder_was_there() {
    let mut p = Pins::default();
    p.add(Path::new("/a"), 8).unwrap();
    p.add(Path::new("/b"), 8).unwrap();
    assert!(p.remove(Path::new("/a")));
    assert!(!p.remove(Path::new("/a")), "removed twice");
    assert!(!p.remove(Path::new("/nowhere")));
    assert_eq!(*p, vec![PathBuf::from("/b")]);
}

fn three_files() -> Tabs {
    let mut t = Tabs::default();
    t.push(file("/a/one.txt"));
    t.push(file("/a/two.txt"));
    t.push(file("/a/three.txt"));
    t
}

#[test]
fn closing_a_middle_tab_keeps_the_same_file_on_screen() {
    let mut t = three_files();
    t.active = 2;
    // Closing the tab before the focus pulls the index back, so the file
    // that was on screen stays on screen.
    t.close(0);
    assert_eq!(t.active, 1);
    assert_eq!(t.active_tab().map(Tab::label).as_deref(), Some("three.txt"));
}

#[test]
fn closing_the_tab_on_screen_falls_back_to_the_new_last() {
    let mut t = three_files();
    t.active = 2;
    t.close(2);
    assert_eq!(t.active, 1);
    assert_eq!(t.active_tab().map(Tab::label).as_deref(), Some("two.txt"));
}

#[test]
fn closing_the_last_tab_empties_the_strip_safely() {
    let mut t = Tabs::default();
    t.push(file("/a/only.txt"));
    t.close(0);
    assert!(t.is_empty());
    assert_eq!(t.active, 0, "the index stays usable for the next open");
    // Closing again must not panic.
    t.close(0);
    t.close(5);
    assert!(t.is_empty());
}

#[test]
fn cycling_wraps_at_both_ends() {
    let mut t = three_files();
    t.active = 0;
    t.cycle(false);
    assert_eq!(t.active, 1);
    t.cycle(false);
    t.cycle(false);
    assert_eq!(t.active, 0, "forward wraps to the first");
    t.cycle(true);
    assert_eq!(t.active, 2, "backwards wraps to the last");
}

#[test]
fn a_single_tab_never_cycles() {
    let mut t = Tabs::default();
    t.push(file("/a/only.txt"));
    t.cycle(false);
    t.cycle(true);
    assert_eq!(t.active, 0);
}

#[test]
fn an_empty_strip_never_cycles() {
    let mut t = Tabs::default();
    t.cycle(false);
    t.cycle(true);
    assert!(t.active_tab().is_none());
}

#[test]
fn focusing_ignores_an_index_past_the_end() {
    let mut t = three_files();
    t.focus(1);
    assert_eq!(t.active, 1);
    t.focus(9);
    assert_eq!(t.active, 1, "an out-of-range focus is ignored");
}

#[test]
fn files_are_named_after_the_file_and_folder_tabs_after_the_folder() {
    assert_eq!(file("/a/notes.md").label(), "notes.md");
    assert_eq!(
        folders::FolderTab::new(Path::new("/a/photos")).label(),
        "photos"
    );
    // A drive root has no name of its own, so it is shown as what it is.
    assert_eq!(folders::FolderTab::new(Path::new("/")).label(), "/");
    if cfg!(windows) {
        assert_eq!(folders::FolderTab::new(Path::new("C:\\")).label(), "C:");
    }
}

#[test]
fn an_open_file_is_found_by_path() {
    let mut t = three_files();
    assert_eq!(t.index_of(Path::new("/a/two.txt")), Some(1));
    assert_eq!(t.index_of(Path::new("/a/missing.txt")), None);
    t.close(1);
    assert_eq!(t.index_of(Path::new("/a/two.txt")), None);
}

#[test]
fn only_dirty_tabs_are_reported() {
    let mut t = three_files();
    assert_eq!(t.first_dirty(), None);
    {
        let end = t[1].doc.text.len_chars();
        t[1].doc.text.insert(end, "edited");
    }
    t[1].doc.version += 1;
    assert_eq!(t.first_dirty(), Some(1));
}
