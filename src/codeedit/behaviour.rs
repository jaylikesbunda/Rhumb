//! What the editor is supposed to do, expressed as tests.
//!
//! Each one drives real input events through a real frame and then looks at
//! the buffer, so it exercises the whole path a user's keystroke takes: focus,
//! key claiming, the caret, the index, and the buffer. The drawn geometry is
//! checked too, because a buffer can be right while the caret is drawn on the
//! wrong line, and that is invisible until someone looks at the screen.
//!
//! Run with `cargo test codeedit::behaviour`.

use super::Options;
use super::harness::{Harness, Rng, random_doc};
use egui::{Key, Modifiers, Pos2, Vec2};

const CTRL: Modifiers = Modifiers::CTRL;
const SHIFT: Modifiers = Modifiers::SHIFT;
const CTRL_SHIFT: Modifiers = Modifiers {
    ctrl: true,
    shift: true,
    ..Modifiers::NONE
};

/// Clicks in the middle of the given character of the given row.
fn click_char(h: &mut Harness, row: usize, col: usize) {
    let pos = h.pos_of(row, col as f32);
    h.click(pos);
}

// ---- typing and the caret --------------------------------------------------

#[test]
fn typing_goes_in_where_the_caret_is() {
    let mut h = Harness::new("hello world");
    click_char(&mut h, 0, 5);
    h.type_text(" there");
    assert_eq!(h.text(), "hello there world");
    assert_eq!(h.caret(), 11);
}

#[test]
fn backspace_removes_the_character_before_the_caret() {
    let mut h = Harness::new("abc");
    click_char(&mut h, 0, 3);
    h.key(Key::Backspace);
    assert_eq!(h.text(), "ab");
    h.key(Key::Backspace);
    h.key(Key::Backspace);
    assert_eq!(h.text(), "");
    // Nothing left to delete, and it must not wrap or panic.
    h.key(Key::Backspace);
    assert_eq!(h.text(), "");
    assert_eq!(h.caret(), 0);
}

#[test]
fn backspace_joins_two_lines() {
    let mut h = Harness::new("ab\ncd");
    click_char(&mut h, 1, 0);
    h.key(Key::Backspace);
    assert_eq!(h.text(), "abcd", "the newline went with it");
    assert_eq!(h.caret(), 2);
}

#[test]
fn delete_removes_the_character_after_the_caret() {
    let mut h = Harness::new("abcdef");
    click_char(&mut h, 0, 2);
    h.key(Key::Delete);
    assert_eq!(h.text(), "abdef");
    assert_eq!(
        h.caret(),
        2,
        "the caret stays put and takes the character with it"
    );
}

#[test]
fn enter_splits_the_line_and_keeps_the_indent() {
    // Split just before the `1`. The new line takes the indent of the line being
    // split, and the caret lands after it.
    let mut h = Harness::new("    let x = 1;");
    click_char(&mut h, 0, 12);
    h.key(Key::Enter);
    assert_eq!(h.text(), "    let x = \n    1;");
    // 12 to the split, then the newline, then the four spaces of indent.
    assert_eq!(
        h.caret(),
        17,
        "the caret is on the new line, after the indent"
    );
}

#[test]
fn enter_after_a_bracket_steps_in_one_level() {
    let mut h = Harness::new("fn f() {");
    click_char(&mut h, 0, 8);
    h.key(Key::Enter);
    assert_eq!(h.text(), "fn f() {\n    ");
}

#[test]
fn tab_indents_and_shift_tab_outdents() {
    // Click to set the anchor, then shift-click the far end: a selection over
    // both lines.
    let mut h = Harness::new("a\nb");
    click_char(&mut h, 0, 0);
    h.click_mod(h.pos_of(1, 1.0), SHIFT);
    assert_eq!(h.selected(), "a\nb", "both lines are selected");
    h.key(Key::Tab);
    assert_eq!(h.text(), "    a\n    b", "both are indented");
    h.key_mod(Key::Tab, SHIFT);
    assert_eq!(h.text(), "a\nb", "and both come back out");
}

#[test]
fn the_arrows_walk_the_document_one_character_at_a_time() {
    let mut h = Harness::new("ab\ncd");
    click_char(&mut h, 0, 0);
    for _ in 0..5 {
        h.key(Key::ArrowRight);
    }
    assert_eq!(h.caret(), 5, "the end of the document");
    h.key(Key::ArrowRight);
    assert_eq!(h.caret(), 5, "and it stops there");
    for _ in 0..9 {
        h.key(Key::ArrowLeft);
    }
    assert_eq!(h.caret(), 0, "and at the start");
}

#[test]
fn home_and_end_go_to_the_edges_of_the_line() {
    let mut h = Harness::new("hello\nworld");
    click_char(&mut h, 1, 3);
    h.key(Key::Home);
    assert_eq!(h.caret(), 6, "the start of the second line");
    h.key(Key::End);
    assert_eq!(h.caret(), 11, "the end of it");
}

#[test]
fn ctrl_home_and_ctrl_end_reach_the_ends_of_the_document() {
    let mut h = Harness::new("one\ntwo\nthree");
    click_char(&mut h, 0, 1);
    h.key_mod(Key::End, CTRL);
    assert_eq!(h.caret(), 13, "the end of the document");
    h.key_mod(Key::Home, CTRL);
    assert_eq!(h.caret(), 0, "the start of it");
}

#[test]
fn ctrl_with_a_vertical_arrow_moves_the_whole_line() {
    let mut h = Harness::new("one\ntwo\nthree");
    click_char(&mut h, 0, 1);
    h.key_mod(Key::ArrowDown, CTRL);
    assert_eq!(h.text(), "two\none\nthree");
    h.key_mod(Key::ArrowUp, CTRL);
    assert_eq!(h.text(), "one\ntwo\nthree", "and back again");
    assert_eq!(h.caret(), 1, "the caret followed the line it moved");
}

#[test]
fn ctrl_with_a_horizontal_arrow_moves_by_word() {
    let mut h = Harness::new("alpha beta gamma");
    click_char(&mut h, 0, 0);
    h.key_mod(Key::ArrowRight, CTRL);
    assert_eq!(h.caret(), 6, "past `alpha`");
    h.key_mod(Key::ArrowRight, CTRL);
    assert_eq!(h.caret(), 11, "past `beta`");
}

#[test]
fn shift_with_an_arrow_extends_the_selection() {
    let mut h = Harness::new("hello world");
    click_char(&mut h, 0, 0);
    h.key_mod(Key::ArrowRight, SHIFT);
    h.key_mod(Key::ArrowRight, SHIFT);
    assert_eq!(h.selected(), "he");
}

// ---- input methods ---------------------------------------------------------

/// Whether `s` was drawn on the last frame, anywhere.
fn drawn_text(h: &Harness, s: &str) -> bool {
    h.shapes().iter().any(|(_, _, t)| t == s)
}

#[test]
fn a_preedit_is_stored_and_drawn_without_touching_the_document() {
    let mut h = Harness::new("ab");
    click_char(&mut h, 0, 1);
    h.ime_preedit("か");
    assert_eq!(h.text(), "ab", "a candidate is not in the document");
    assert_eq!(h.caret(), 1, "and the caret has not moved");
    assert_eq!(h.preedit(), Some("か"), "the composition is held aside");
    assert!(
        drawn_text(&h, "か"),
        "the composition is drawn on the frame"
    );
    // A later preedit replaces the one before it, as the IME refines a candidate.
    h.ime_preedit("かん");
    assert_eq!(h.preedit(), Some("かん"));
    assert_eq!(h.text(), "ab", "still only a candidate");
    // And an empty preedit clears it, as the IME does when it is dismissed.
    h.ime_preedit("");
    assert_eq!(h.preedit(), None);
    assert!(!drawn_text(&h, "かん"), "the overlay went with it");
}

#[test]
fn committing_a_composition_types_it_at_the_caret_and_clears_the_preedit() {
    let mut h = Harness::new("ab");
    click_char(&mut h, 0, 1);
    h.ime_preedit("にほん");
    assert_eq!(h.text(), "ab", "still only a candidate");
    h.ime_commit("日本");
    assert_eq!(h.text(), "a日本b", "the committed text landed at the caret");
    assert_eq!(h.caret(), 3, "and the caret follows it");
    assert_eq!(h.preedit(), None, "the composition is over");
    assert!(!drawn_text(&h, "日本"), "and is no longer an overlay");
}

#[test]
fn dismissing_the_input_method_drops_the_preedit() {
    let mut h = Harness::new("ab");
    click_char(&mut h, 0, 1);
    h.ime_preedit("か");
    assert_eq!(h.preedit(), Some("か"));
    h.ime_disabled();
    assert_eq!(h.preedit(), None, "the composition is gone");
    assert_eq!(h.text(), "ab", "and nothing was typed");
}

#[test]
fn committing_replaces_a_selection_like_typing_does() {
    let mut h = Harness::new("one two three");
    click_char(&mut h, 0, 4);
    for _ in 0..3 {
        h.key_mod(Key::ArrowRight, SHIFT);
    }
    assert_eq!(h.selected(), "two", "the selection the commit will replace");
    h.ime_preedit("に");
    h.ime_commit("2");
    assert_eq!(h.text(), "one 2 three", "the selection was replaced");
    assert_eq!(h.caret(), 5);
    assert_eq!(h.preedit(), None);
}

#[test]
fn a_read_only_document_takes_no_committed_text() {
    let mut h = Harness::new("ab");
    h.set_editable(false);
    click_char(&mut h, 0, 1);
    h.ime_preedit("か");
    h.ime_commit("日本");
    assert_eq!(h.text(), "ab", "nothing was inserted");
    assert_eq!(h.preedit(), None, "but the composition is still over");
}

// ---- undo ------------------------------------------------------------------

#[test]
fn a_burst_of_typing_undoes_in_one_step() {
    let mut h = Harness::new("");
    h.type_text("hello");
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), "", "the whole burst went at once");
}

#[test]
fn undo_walks_back_through_separate_edits() {
    // Each edit is separated from the last by a pause, which is what a person
    // typing does and what makes the steps separate. Typing straight through is
    // the other test.
    let mut h = Harness::new("");
    h.type_text("a");
    h.pause();
    h.type_text("b");
    h.pause();
    h.type_text("c");
    assert_eq!(h.text(), "abc");
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), "ab", "only the last keystroke went");
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), "a", "and then the one before it");
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), "");
}

#[test]
fn redo_brings_the_edit_back() {
    let mut h = Harness::new("");
    h.type_text("x");
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), "");
    h.key_mod(Key::Y, CTRL);
    assert_eq!(h.text(), "x", "redo works from Ctrl+Y");
    h.key_mod(Key::Z, CTRL);
    h.key_mod(Key::Z, SHIFT | CTRL);
    assert_eq!(h.text(), "x", "and from Ctrl+Shift+Z");
}

// ---- clipboard -------------------------------------------------------------

#[test]
fn the_system_clipboard_carries_the_selection() {
    // Copy is not observable from the buffer, so what is checked here is that
    // the shortcut is claimed by the editor rather than falling through to the
    // file list, which is the bug this path had: the window layer turns Ctrl+C
    // into an event and drops the key, so a handler waiting for the key never
    // sees it.
    let mut h = Harness::new("hello world");
    click_char(&mut h, 0, 0);
    h.key_mod(Key::A, CTRL);
    h.send_copy();
    assert!(h.took_clipboard(), "the editor must claim the copy");
}

// ---- pairing ---------------------------------------------------------------

#[test]
fn typing_an_opener_puts_its_closer_in_and_leaves_the_caret_between() {
    let mut h = Harness::new("");
    h.type_text("f(");
    assert_eq!(h.text(), "f()");
    assert_eq!(h.caret(), 2, "the caret is between the two");
}

#[test]
fn typing_the_closer_steps_over_it_rather_than_doubling_it() {
    let mut h = Harness::new("");
    h.type_text("f(");
    h.type_text(")");
    assert_eq!(h.text(), "f()", "and not f())");
    assert_eq!(h.caret(), 3);
}

#[test]
fn ctrl_slash_toggles_a_comment() {
    let mut h = Harness::new("let x = 1;");
    click_char(&mut h, 0, 0);
    h.key_mod(Key::A, CTRL);
    h.key_mod(Key::Slash, CTRL);
    assert_eq!(h.text(), "// let x = 1;");
    h.key_mod(Key::Slash, CTRL);
    assert_eq!(h.text(), "let x = 1;", "and back again");
}

// ---- line commands ---------------------------------------------------------

#[test]
fn home_goes_to_the_code_and_then_to_the_very_start() {
    // The "smart" Home: the first press is for reaching the code, the second for
    // the real column zero. On an unindented line the two are the same place, so
    // nothing happens twice.
    let mut h = Harness::new("    let x = 1;");
    click_char(&mut h, 0, 9);
    h.key(Key::Home);
    assert_eq!(h.caret(), 4, "to the first character of the code");
    h.key(Key::Home);
    assert_eq!(h.caret(), 0, "and pressing it again reaches column zero");
    h.key(Key::Home);
    assert_eq!(
        h.caret(),
        4,
        "and from column zero it goes back to the code, so the key keeps working"
    );
}

#[test]
fn home_on_a_blank_line_just_goes_to_the_start() {
    let mut h = Harness::new("one\n\ntwo");
    click_char(&mut h, 1, 0);
    h.key(Key::Home);
    assert_eq!(h.caret(), 4, "the start of the empty line");
    h.key(Key::Home);
    assert_eq!(h.caret(), 4, "and it stays there");
}

#[test]
fn home_and_end_still_extend_the_selection() {
    let mut h = Harness::new("    let x = 1;");
    click_char(&mut h, 0, 0);
    h.key(Key::End);
    h.key_mod(Key::Home, SHIFT);
    assert_eq!(
        h.selected(),
        "let x = 1;",
        "back to the first character of the code"
    );
    h.key_mod(Key::Home, SHIFT);
    assert_eq!(h.selected(), "    let x = 1;", "and then on to column zero");
}

#[test]
fn ctrl_backspace_deletes_the_word_before_the_caret() {
    let mut h = Harness::new("alpha beta gamma");
    click_char(&mut h, 0, 16);
    h.key_mod(Key::Backspace, CTRL);
    assert_eq!(h.text(), "alpha beta ");
    assert_eq!(h.caret(), 11);
    h.key_mod(Key::Backspace, CTRL);
    assert_eq!(h.text(), "alpha ");
    assert_eq!(h.caret(), 6);
    h.key_mod(Key::Backspace, CTRL);
    assert_eq!(h.text(), "", "and the gap before it goes with the word");
    assert_eq!(h.caret(), 0);
}

#[test]
fn ctrl_delete_deletes_the_word_after_the_caret_and_the_gap_behind_it() {
    // The same word boundary the caret movement uses, so the word *and* the run
    // of spaces up to the next one go together. Half a gap is the one thing a
    // reader would notice as a mistake: the text is left looking ragged where it
    // was tidy a moment ago.
    let mut h = Harness::new("alpha beta gamma");
    click_char(&mut h, 0, 0);
    h.key_mod(Key::Delete, CTRL);
    assert_eq!(h.text(), "beta gamma");
    assert_eq!(h.caret(), 0, "the caret does not move");
    h.key_mod(Key::Delete, CTRL);
    assert_eq!(h.text(), "gamma");
}

#[test]
fn ctrl_backspace_and_delete_delete_exactly_what_the_word_movement_crosses() {
    // The two have to agree, or the caret walks one way and the deletion goes
    // another, which is the kind of thing nobody notices until it eats a
    // character that mattered.
    let doc = "one two\tthree\nfour_five  six";
    let mut moved = Harness::new(doc);
    let mut deleted = Harness::new(doc);
    click_char(&mut moved, 0, 0);
    click_char(&mut deleted, 0, 0);
    for _ in 0..4 {
        moved.key_mod(Key::ArrowRight, CTRL);
        deleted.key_mod(Key::Delete, CTRL);
    }
    // Every character the caret walked over is gone, and the walk crosses word
    // boundaries, tabs and the newline alike rather than stopping at each.
    assert_eq!(deleted.text(), "six");
    assert_eq!(moved.caret(), doc.chars().count() - 3);
}

#[test]
fn a_word_delete_at_the_start_and_end_of_the_document_stays_put() {
    let mut h = Harness::new("one two");
    click_char(&mut h, 0, 0);
    h.key_mod(Key::Backspace, CTRL);
    assert_eq!(h.text(), "one two", "nothing before the first character");
    h.key_mod(Key::ArrowLeft, CTRL);
    click_char(&mut h, 0, 7);
    h.key_mod(Key::Delete, CTRL);
    assert_eq!(h.text(), "one two", "nothing after the last");
}

#[test]
fn ctrl_shift_k_deletes_the_whole_line_and_its_newline() {
    let mut h = Harness::new("one\ntwo\nthree");
    click_char(&mut h, 1, 1);
    h.key_mod(Key::K, CTRL | SHIFT);
    assert_eq!(h.text(), "one\nthree");
    assert_eq!(h.caret(), 4, "where the deleted line was");
}

#[test]
fn ctrl_shift_k_deletes_every_line_a_selection_touches() {
    let mut h = Harness::new("one\ntwo\nthree\nfour");
    h.click(h.pos_of(1, 1.0));
    h.click_mod(h.pos_of(2, 1.0), SHIFT);
    h.key_mod(Key::K, CTRL | SHIFT);
    assert_eq!(h.text(), "one\nfour");
}

#[test]
fn ctrl_shift_k_on_the_last_line_without_a_newline_leaves_the_ones_above() {
    let mut h = Harness::new("one\ntwo");
    click_char(&mut h, 1, 2);
    h.key_mod(Key::K, CTRL | SHIFT);
    assert_eq!(h.text(), "one\n", "the line and its newline before it");
}

#[test]
fn ctrl_shift_k_on_an_empty_document_does_nothing_at_all() {
    // Not even an undo step: a keystroke that appears to do nothing must not
    // quietly swallow the next Ctrl+Z.
    let mut h = Harness::new("");
    h.key_mod(Key::K, CTRL | SHIFT);
    assert_eq!(h.text(), "");
    h.type_text("x");
    h.key_mod(Key::Z, CTRL);
    assert_eq!(
        h.text(),
        "",
        "and the undo after it is the typing, not this"
    );
}

#[test]
fn ctrl_shift_d_duplicates_the_line_below_itself() {
    let mut h = Harness::new("one\ntwo\nthree");
    click_char(&mut h, 1, 1);
    h.key_mod(Key::D, CTRL_SHIFT);
    assert_eq!(h.text(), "one\ntwo\ntwo\nthree");
    assert_eq!(h.caret(), 4, "the caret stays on its own line");
}

#[test]
fn ctrl_shift_d_duplicates_a_selection_as_a_block() {
    let mut h = Harness::new("one\ntwo\nthree");
    h.click(h.pos_of(0, 1.0));
    h.click_mod(h.pos_of(1, 1.0), SHIFT);
    h.key_mod(Key::D, CTRL_SHIFT);
    assert_eq!(h.text(), "one\ntwo\none\ntwo\nthree");
}

#[test]
fn ctrl_shift_d_on_the_last_line_without_a_newline_puts_the_copy_underneath() {
    let mut h = Harness::new("one\ntwo");
    click_char(&mut h, 1, 1);
    h.key_mod(Key::D, CTRL_SHIFT);
    assert_eq!(h.text(), "one\ntwo\ntwo");
}

#[test]
fn ctrl_shift_d_undoes_in_one_step() {
    let mut h = Harness::new("one\ntwo");
    click_char(&mut h, 1, 1);
    h.key_mod(Key::D, CTRL_SHIFT);
    assert_eq!(h.text(), "one\ntwo\ntwo");
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), "one\ntwo", "the copy was not a separate edit");
}

#[test]
fn a_line_command_does_nothing_in_a_read_only_document() {
    let opts = Options {
        editable: false,
        ..Options::default()
    };
    let mut h = Harness::with_options("one\ntwo", opts, egui::vec2(600.0, 600.0));
    h.frame();
    click_char(&mut h, 0, 1);
    h.key_mod(Key::K, CTRL | SHIFT);
    h.key_mod(Key::D, CTRL_SHIFT);
    h.key_mod(Key::Backspace, CTRL);
    assert_eq!(h.text(), "one\ntwo");
}

// ---- scrolling -------------------------------------------------------------

#[test]
fn the_wheel_turns_the_window_the_way_the_wheel_is_turned() {
    // egui names its own sign: a positive `smooth_scroll_delta.y` is scrolling
    // *up*, and the platform reports a wheel rotated upwards as positive. So a
    // positive delta moves the window towards the top of the document. The editor
    // had it the other way round, which turned the wheel the wrong way while the
    // rest of the scrolling tests - written in the reader's direction, through the
    // harness - stayed green.
    let doc = (0..400)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut h = Harness::new(&doc);
    h.frame();
    // Halfway down first, so there is room to move in both directions.
    h.scroll(200.0);
    h.frame();
    let middle = h.top_line().expect("rows were drawn");
    assert!(middle > 50, "scrolling down should have moved down, not up");

    // A wheel rotated up, which is a positive delta in egui's sign.
    h.raw_wheel(6.0);
    h.frame();
    let after_up = h.top_line().expect("rows were drawn");
    assert!(
        after_up < middle,
        "a wheel turned upwards must move the window towards the top, but it went \
         from {middle} to {after_up}"
    );

    // And a wheel rotated down, the same distance the other way, must come back.
    h.raw_wheel(-6.0);
    h.frame();
    assert_eq!(
        h.top_line(),
        Some(middle),
        "and the other way must undo it exactly"
    );
}

#[test]
fn the_wheel_scrolls_the_window() {
    let doc = (0..200)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut h = Harness::new(&doc);
    h.frame();
    let first = h.top_line().expect("rows were drawn");
    assert_eq!(first, 0, "the window starts at the top of the document");
    // Positive is the direction a wheel turns to move the content on.
    h.scroll(10.0);
    let after = h.top_line().expect("rows were drawn");
    assert!(
        after > first,
        "scrolling should raise the first line on screen, but it is still {after}"
    );
    h.scroll(-10.0);
    assert_eq!(
        h.top_line(),
        Some(first),
        "and scrolling back returns to where it was"
    );
}

#[test]
fn scrolling_stops_at_both_ends_of_the_document() {
    let doc = (0..40)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut h = Harness::new(&doc);
    h.frame();
    // A long way past the end.
    h.scroll(9000.0);
    let shown = h.drawn_line_numbers();
    // Forty lines of text joined by newlines, so the document has a fortieth line
    // that is empty - that is where a final Enter leaves the cursor, and the
    // window is allowed to show it. What it must not do is scroll past the end
    // into nothing, so the last row is either that empty line or the one before.
    let last = shown.last().copied().unwrap_or(0);
    assert!(
        last == 39 || last == 40,
        "scrolling to the end should stop at the last line, but the last row is \
         line {last}: {shown:?}"
    );
    // And a long way before the start.
    h.scroll(-9000.0);
    assert_eq!(
        h.top_line(),
        Some(0),
        "scrolling back up reaches the first line"
    );
}

#[test]
fn the_caret_is_brought_back_into_view_when_it_moves_off_screen() {
    // A document far taller than the pane, so the caret walks off the bottom
    // and has to bring the window with it.
    let doc = (0..300)
        .map(|i| format!("line {i:03}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut h = Harness::new(&doc);
    h.frame();
    click_char(&mut h, 0, 0);
    h.frame();
    let top = h.top_line().expect("rows were drawn");
    // Down well past the bottom of the pane.
    for _ in 0..60 {
        h.key(Key::ArrowDown);
    }
    h.frame();
    let after = h.top_line().expect("rows were drawn");
    assert!(
        after > top,
        "the window should have followed the caret down, but it is still at {after}"
    );
    assert!(
        h.caret_row().is_some(),
        "the caret should still be on screen after arrowing down"
    );
    // And back up off the top.
    for _ in 0..120 {
        h.key(Key::ArrowUp);
    }
    h.frame();
    assert_eq!(
        h.top_line(),
        Some(0),
        "and arrowing back up should reach the top of the document"
    );
    assert_eq!(h.caret(), 0, "with the caret at the first character");
}

#[test]
fn a_page_moves_by_a_screenful_and_stays_on_the_document() {
    let doc = (0..300)
        .map(|i| format!("line {i:03}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut h = Harness::new(&doc);
    h.frame();
    click_char(&mut h, 0, 0);
    h.frame();
    h.key(Key::PageDown);
    h.frame();
    let after = h.top_line().expect("rows were drawn");
    assert!(
        after > 0,
        "page down should have moved, but the top is {after}"
    );
    assert!(
        h.caret_row().is_some(),
        "the caret should still be on screen after a page"
    );
    h.key(Key::PageUp);
    h.frame();
    assert_eq!(h.caret(), 0, "page up returns to where it started");
}

#[test]
fn a_double_click_selects_a_word_and_a_triple_click_selects_the_line() {
    // The second click of a triple is also a double, so the order the two are
    // checked in decides which one wins. Line is the wider of the two answers and
    // is what a third click is asking for.
    let mut h = Harness::new("alpha beta\ngamma delta");
    h.click(h.pos_of(0, 7.0));
    h.click(h.pos_of(0, 7.0));
    assert_eq!(h.selected(), "beta", "two clicks take the word");
    h.click(h.pos_of(0, 7.0));
    assert_eq!(h.selected(), "alpha beta\n", "three take the whole line");
    // And the newline is in the selection, so deleting it takes the line rather
    // than leaving an empty one behind.
    h.key(Key::Delete);
    assert_eq!(h.text(), "gamma delta");
}

#[test]
fn a_triple_click_on_the_last_line_stops_at_the_end_of_the_document() {
    let mut h = Harness::new("one\ntwo");
    h.click(h.pos_of(1, 1.0));
    h.click(h.pos_of(1, 1.0));
    h.click(h.pos_of(1, 1.0));
    assert_eq!(h.selected(), "two", "there is no newline to include");
    h.key(Key::Delete);
    assert_eq!(h.text(), "one\n");
}

#[test]
fn the_caret_reports_the_line_and_column_a_reader_expects() {
    // One-based, as every status bar shows it, and counting characters rather
    // than bytes so a line of multi-byte text is not a lie.
    let mut h = Harness::new("alpha\n    béta\nlast");
    click_char(&mut h, 0, 0);
    assert_eq!(h.line_column(), (1, 1), "the first character");
    h.key(Key::ArrowRight);
    h.key(Key::ArrowRight);
    assert_eq!(h.line_column(), (1, 3));
    h.key(Key::End);
    assert_eq!(h.line_column(), (1, 6), "one past the last character");
    h.key(Key::ArrowDown);
    assert_eq!(
        h.line_column(),
        (2, 6),
        "down keeps the column where the line is long enough to allow it"
    );
    h.key(Key::End);
    assert_eq!(
        h.line_column(),
        (2, 9),
        "the é is one character, not two bytes"
    );
    h.key(Key::ArrowDown);
    h.key(Key::End);
    assert_eq!(h.line_column(), (3, 5));
    h.key(Key::ArrowDown);
    assert_eq!(
        h.line_column(),
        (3, 5),
        "and there is no fourth line to move to"
    );
}

#[test]
fn a_line_longer_than_the_pane_can_be_reached_by_scrolling_sideways() {
    // Without this the file is readable only up to the width of the window, and
    // `End` puts the caret somewhere the reader cannot see. The caret is what
    // brings the view with it, so walking right is what scrolls.
    let long: String = (0..400).map(|i| format!("{i} ")).collect();
    let mut h = Harness::new(&long);
    h.frame();
    assert_eq!(h.scroll_x(), 0.0, "it starts at the left");
    for _ in 0..200 {
        h.key(Key::ArrowRight);
    }
    h.frame();
    assert!(
        h.scroll_x() > 100.0,
        "walking right along a long line should scroll the view, but it is at {}",
        h.scroll_x()
    );
    // And the caret is inside the visible band, not merely the offset having
    // moved: an offset that drifts ahead of the caret scrolls past what is being
    // read, which is worse than not scrolling at all.
    let caret = h.caret_rect().expect("the caret is on screen");
    let r = h.rect();
    assert!(
        caret.left() >= r.left() - 1.0 && caret.right() <= r.right() + 1.0,
        "the caret is at {caret:?}, outside the pane {r:?}"
    );
}

#[test]
fn the_sideways_scroll_reaches_both_ends_and_no_further() {
    let long: String = (0..400).map(|i| format!("{i} ")).collect();
    let mut h = Harness::new(&long);
    h.frame();
    h.key(Key::End);
    h.frame();
    let at_end = h.scroll_x();
    assert!(
        at_end > 0.0,
        "End should have scrolled to the end of the line"
    );
    for _ in 0..50 {
        h.key(Key::ArrowRight);
    }
    h.frame();
    assert_eq!(
        h.scroll_x(),
        at_end,
        "and there is nothing further right to go"
    );
    h.key(Key::Home);
    h.key(Key::Home);
    h.frame();
    assert_eq!(h.scroll_x(), 0.0, "Home brings the view back to the left");
}

#[test]
fn a_wrapped_line_is_never_scrolled_sideways() {
    // A wrapped line is never wider than the pane, so a sideways offset would be
    // showing the reader a blank strip for no reason.
    let long: String = (0..400).map(|i| format!("{i} ")).collect();
    let opts = Options {
        wrap: true,
        line_numbers: false,
        ..Options::default()
    };
    let mut h = Harness::with_options(&long, opts, egui::vec2(300.0, 600.0));
    h.frame();
    h.key(Key::End);
    h.frame();
    assert_eq!(h.scroll_x(), 0.0);
}

#[test]
fn the_gutter_stays_put_while_the_text_scrolls_sideways() {
    let long: String = (0..400).map(|i| format!("{i} ")).collect();
    let mut h = Harness::new(&long);
    h.frame();
    let before = h.gutter_left().expect("there is a gutter");
    for _ in 0..200 {
        h.key(Key::ArrowRight);
    }
    h.frame();
    let after = h.gutter_left().expect("there is a gutter");
    assert!(
        (after - before).abs() < 0.5,
        "the line numbers moved with the text: {before} then {after}"
    );
}

// ---- find in the document --------------------------------------------------

#[test]
fn ctrl_f_opens_the_find_bar_and_the_text_goes_into_it_not_the_file() {
    let mut h = Harness::new("alpha\nbeta\nalpha");
    click_char(&mut h, 0, 0);
    h.key_mod(Key::F, CTRL);
    assert!(h.find_open(), "Ctrl+F opens the bar");
    h.type_text("beta");
    assert_eq!(
        h.text(),
        "alpha\nbeta\nalpha",
        "and nothing was typed into the file"
    );
    assert_eq!(h.find_needle(), "beta", "it went into the box instead");
    assert_eq!(h.find_hits(), 1, "one match");
}

#[test]
fn ctrl_f_with_a_selection_searches_for_what_is_selected() {
    // The fastest way to ask "where else is this?", and the reason the bar
    // pre-fills at all. A double click, because what is wanted here is a
    // selection rather than a position and a double click is how a reader makes
    // one without counting columns.
    let mut h = Harness::new("foo bar\nfoo baz");
    h.click(h.pos_of(0, 1.0));
    h.click(h.pos_of(0, 1.0));
    assert_eq!(h.selected(), "foo", "a double click takes the word");
    h.key_mod(Key::F, CTRL);
    assert_eq!(h.find_needle(), "foo", "and Ctrl+F searches for it");
    assert_eq!(h.find_hits(), 2, "the other line has it too");
}
#[test]
fn the_matches_are_counted_and_the_current_one_is_selected() {
    let mut h = Harness::new("needle one\nneedle two\nneedle three");
    click_char(&mut h, 0, 0);
    h.key_mod(Key::F, CTRL);
    h.type_text("needle");
    assert_eq!(h.find_hits(), 3);
    assert_eq!(h.selected(), "needle", "the first match is selected");
    assert_eq!(h.caret(), 6, "and the caret is at its end");
}

#[test]
fn enter_and_shift_enter_step_through_the_matches_and_wrap() {
    let mut h = Harness::new("a a a a");
    click_char(&mut h, 0, 0);
    h.key_mod(Key::F, CTRL);
    h.type_text("a");
    // Typing selects the first match, so Enter is already "the next one" and does
    // not spend a press re-finding what the reader can see.
    assert_eq!(h.caret(), 1, "the first match is selected as it is typed");
    h.key(Key::Enter);
    assert_eq!(h.caret(), 3, "Enter moves to the second");
    h.key(Key::Enter);
    assert_eq!(h.caret(), 5, "then the third");
    h.key(Key::Enter);
    assert_eq!(h.caret(), 7, "and round to the first again");
    h.key_mod(Key::Enter, SHIFT);
    assert_eq!(h.caret(), 5, "and back the other way");
}

#[test]
fn stepping_to_a_match_scrolls_it_into_view() {
    let doc = (0..400)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut h = Harness::new(&doc);
    h.frame();
    click_char(&mut h, 0, 0);
    h.key_mod(Key::F, CTRL);
    h.type_text("line 300");
    for _ in 0..20 {
        h.key(Key::Enter);
    }
    h.frame();
    // The caret is not drawn while the find field has the keyboard, so what is
    // checked is that the window followed the match rather than that a caret is
    // visible.
    let top = h.top_line().expect("rows were drawn");
    assert!(
        top > 200,
        "and the window should have followed it, not sat at {top}"
    );
}

#[test]
fn escape_closes_the_bar_and_gives_the_keyboard_back_to_the_text() {
    let mut h = Harness::new("alpha");
    click_char(&mut h, 0, 5);
    h.key_mod(Key::F, CTRL);
    assert!(h.find_open());
    h.key(Key::Escape);
    assert!(!h.find_open(), "Escape closes it");
    h.type_text("!");
    assert_eq!(h.text(), "alpha!", "and typing goes to the file again");
}

#[test]
fn a_match_that_is_not_there_leaves_the_caret_where_it_was() {
    let mut h = Harness::new("alpha\nbeta");
    click_char(&mut h, 0, 3);
    h.key_mod(Key::F, CTRL);
    h.type_text("zebra");
    assert_eq!(h.find_hits(), 0);
    h.key(Key::Enter);
    assert_eq!(
        h.caret(),
        3,
        "and Enter with nothing to go to does not move it"
    );
}

#[test]
fn the_find_options_narrow_the_search() {
    let mut h = Harness::new("Cat cat CAT");
    click_char(&mut h, 0, 0);
    h.key_mod(Key::F, CTRL);
    h.type_text("cat");
    assert_eq!(h.find_hits(), 3, "case is ignored to begin with");
    h.click(h.find_button("Match case"));
    h.frame();
    assert_eq!(h.find_hits(), 1, "and matching it exactly leaves one");

    // A second search rather than editing the first: the field still holds "cat",
    // and typing into it would make "catat" rather than "at".
    let mut w = Harness::new("Cat cat CAT");
    click_char(&mut w, 0, 0);
    w.key_mod(Key::F, CTRL);
    w.type_text("at");
    assert_eq!(w.find_hits(), 3, "a substring matches to begin with");
    w.click(w.find_button("Match whole word only"));
    w.frame();
    assert_eq!(w.find_hits(), 0, "`at` is not a whole word inside `cat`");
}

#[test]
fn the_hits_are_highlighted_in_the_text() {
    let mut h = Harness::new("needle one\nneedle two");
    click_char(&mut h, 0, 0);
    h.key_mod(Key::F, CTRL);
    h.type_text("needle");
    h.frame();
    // Two hits, one selected and one only washed in, so there are three marked
    // stretches of text on screen in total: the wash, the selection, and the
    // selection again if it is on the second line.
    let marked = h.marked_spans();
    assert!(
        marked >= 2,
        "the other match should be marked even though it is not selected, found {marked}"
    );
}

#[test]
fn an_edit_makes_the_hits_be_found_again() {
    // The character index of every hit after an edit moves, so hits found before
    // it point at the wrong text afterwards.
    let mut h = Harness::new("needle one\nneedle two");
    click_char(&mut h, 0, 0);
    h.key_mod(Key::F, CTRL);
    h.type_text("needle");
    assert_eq!(h.find_hits(), 2);
    h.key(Key::Escape);
    h.frame();
    assert!(h.focused(), "and the keyboard goes back to the text");
    // At the end of the *first* line, where the search left the selection, so the
    // edit lands after a match rather than replacing it.
    h.key(Key::Home);
    h.key(Key::End);
    h.type_text("!");
    assert_eq!(h.text(), "needle one!\nneedle two");
    h.key_mod(Key::F, CTRL);
    assert_eq!(h.find_needle(), "needle", "the needle survived the edit");
    assert_eq!(
        h.find_hits(),
        2,
        "and the hits were found again in the text as it is now"
    );
}

#[test]
fn a_search_over_a_large_document_is_fast_enough_to_type_into() {
    // A search that takes longer than a keystroke makes the feature unusable,
    // and the file this runs against is the size where that stops being obvious.
    // Kept under the cap on held hits, so this measures the scan and not the cap.
    let doc = "let value = 1;\n".repeat(15_000);
    let started = std::time::Instant::now();
    let hits = super::find::find_all(&doc, "value", super::find::Query::loose());
    let took = started.elapsed();
    assert_eq!(hits.len(), 15_000, "every one of them");
    assert!(
        took.as_millis() < 60,
        "a search over {} KB took {took:?}, which is longer than a frame",
        doc.len() / 1024
    );
}

// ---- the scrollbar ---------------------------------------------------------

#[test]
fn a_document_that_fits_draws_no_scrollbar() {
    let mut h = Harness::new("one\ntwo\nthree");
    h.frame();
    assert!(
        h.scrollbar().is_none(),
        "there is nothing to scroll, so there is nothing to show"
    );
}

#[test]
fn a_long_document_draws_a_thumb_that_says_how_much_is_on_screen() {
    let doc = (0..400)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut h = Harness::new(&doc);
    h.frame();
    let (track, thumb) = h.scrollbar().expect("a scrollbar should be drawn");
    // The thumb is the fraction of the document that fits, and it sits inside the
    // track, which is the whole of what a reader needs to know from it.
    let shown = h.drawn_line_numbers().len() as f32;
    let fraction = thumb.height() / track.height();
    assert!(
        (fraction - shown / 400.0).abs() < 0.06,
        "the thumb is {:.0}% of the track, but {:.0}% of the document is on screen",
        fraction * 100.0,
        shown / 400.0 * 100.0
    );
    assert!(
        track.contains_rect(thumb),
        "the thumb {thumb:?} is outside the track {track:?}"
    );
}

#[test]
fn the_thumb_moves_down_as_the_window_scrolls_and_stops_at_both_ends() {
    let doc = (0..400)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut h = Harness::new(&doc);
    h.frame();
    let (track, top_thumb) = h.scrollbar().expect("a scrollbar");
    h.scroll(2000.0);
    h.frame();
    let (_, moved) = h.scrollbar().expect("a scrollbar");
    assert!(
        moved.top() > top_thumb.top(),
        "scrolling should have pushed the thumb down"
    );
    // All the way to the end: the thumb reaches the bottom of the track rather
    // than overshooting it by its own height.
    h.scroll(100_000.0);
    h.frame();
    let (_, bottom) = h.scrollbar().expect("a scrollbar");
    // Two points, not one: the thumb is drawn inset by a point at each end so it
    // reads as a separate object from the track, so it stops that far short.
    assert!(
        (bottom.bottom() - track.bottom()).abs() < 2.0,
        "at the end of the document the thumb should be at the end of the track, \
         but it is at {bottom:?} and the track is {track:?}"
    );
    // And all the way back.
    h.scroll(-100_000.0);
    h.frame();
    let (_, again) = h.scrollbar().expect("a scrollbar");
    assert!(
        (again.top() - top_thumb.top()).abs() < 2.0,
        "scrolling back to the top should put the thumb back where it was"
    );
}

#[test]
fn dragging_the_thumb_moves_the_window_and_the_window_moves_the_thumb() {
    let doc = (0..400)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut h = Harness::new(&doc);
    h.frame();
    let (_, thumb) = h.scrollbar().expect("a scrollbar");
    // Grab the middle of the thumb and drag it to the middle of the track: the
    // document is 400 lines and about 40 are on screen, so the middle of the
    // track is a little past the middle of the file.
    let r = h.rect();
    h.drag(thumb.center(), egui::pos2(thumb.center().x, r.center().y));
    h.frame();
    let after = h.top_line().expect("rows were drawn");
    assert!(
        after > 100 && after < 300,
        "dragging the thumb to the middle should land near the middle, not at {after}"
    );
    let (_, moved) = h.scrollbar().expect("a scrollbar");
    assert!(
        moved.top() > thumb.top(),
        "and the thumb should have followed"
    );
}

#[test]
fn clicking_the_track_pages_the_window() {
    let doc = (0..400)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut h = Harness::new(&doc);
    h.frame();
    let (track, thumb) = h.scrollbar().expect("a scrollbar");
    // Below the thumb: a page down. A jump rather than a page would be useless
    // on a file this size, because there is no way back but the bar.
    let below = egui::pos2(track.center().x, track.bottom() - 4.0);
    assert!(
        below.y > thumb.bottom(),
        "the test point is below the thumb"
    );
    h.click_scrollbar(below);
    h.frame();
    let down = h.top_line().expect("rows were drawn");
    assert!(down > 0, "a page down should have moved the window");
    // And above the thumb: a page back.
    h.click_scrollbar(egui::pos2(track.center().x, track.top() + 4.0));
    h.frame();
    assert_eq!(h.top_line(), Some(0), "a page up returns to the top");
}

#[test]
fn clicking_the_text_does_not_page_the_window() {
    // The scrollbar is a separate widget, and the text is under it. A click that
    // lands on the text must not also be read as a click on the track, or every
    // click in the right margin would scroll the file.
    let doc = (0..400)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut h = Harness::new(&doc);
    h.frame();
    let r = h.rect();
    h.click(egui::pos2(r.right() - 40.0, r.top() + 4.0));
    h.frame();
    assert_eq!(h.top_line(), Some(0), "the window did not move");
}

#[test]
fn the_text_stops_at_the_scrollbar_rather_than_running_under_it() {
    // A long unwrapped line used to be a way to find out the file was longer than
    // the window. It must now stop short of the bar instead of running under it
    // and out the other side. Needs more than one line, or there is no bar.
    let doc = (0..60)
        .map(|i| format!("{i}{}", "x".repeat(4000)))
        .collect::<Vec<_>>()
        .join("\n");
    let mut h = Harness::new(&doc);
    h.frame();
    let (track, _) = h.scrollbar().expect("a scrollbar");
    let texts = h.text_shapes();
    assert!(!texts.is_empty(), "some text was drawn");
    for (drawn, clip) in &texts {
        assert!(
            clip.right() <= track.left() + 0.5,
            "a line ending at {} was only clipped to {clip:?}, so it runs under \
             the scrollbar which starts at {}",
            drawn.right(),
            track.left()
        );
    }
}

// ---- read-only -------------------------------------------------------------

#[test]
fn a_read_only_document_can_be_read_and_selected_but_not_changed() {
    let opts = Options {
        editable: false,
        ..Options::default()
    };
    let mut h = Harness::with_options("hello world", opts, egui::vec2(600.0, 600.0));
    h.frame();
    click_char(&mut h, 0, 0);
    h.key_mod(Key::A, CTRL);
    assert_eq!(h.selected(), "hello world", "selection still works");
    h.type_text("nope");
    h.key(Key::Backspace);
    h.key(Key::Delete);
    h.key(Key::Enter);
    assert_eq!(h.text(), "hello world", "but nothing changed it");
}

#[test]
fn wrapped_lines_still_have_line_numbers() {
    // The number belongs to the line, not to the row, so a line that takes three
    // rows is still numbered once - against its first row, which is the only one
    // a reader would call "the top of the line".
    let long: String = "z".repeat(200);
    let doc = format!("one\n{long}\nthree");
    let opts = Options {
        wrap: true,
        ..Options::default()
    };
    let mut h = Harness::with_options(&doc, opts, egui::vec2(300.0, 600.0));
    h.frame();
    let rows = h.drawn_line_numbers();
    assert!(rows.len() > 3, "the long line should span several rows");
    let numbers = h.gutter_numbers();
    assert_eq!(
        numbers,
        vec!["1".to_owned(), "2".to_owned(), "3".to_owned()],
        "three lines, three numbers, whatever the row count"
    );
    // And the gutter still takes its width, so the text does not run under it.
    assert!(
        h.gutter_rect().is_some(),
        "a wrapped document still has a gutter"
    );
}

#[test]
fn the_gutter_never_numbers_a_line_the_document_does_not_have() {
    // The layout emits one row more than there are lines, for the position after
    // the final newline. It is a real row and the caret can sit on it, but
    // numbering it put a number in the gutter for a line that is not there - the
    // last thing a reader checking a line count would want to be wrong about.
    for doc in ["one\ntwo\nthree", "one\ntwo\nthree\n", "one"] {
        let mut h = Harness::new(doc);
        h.frame();
        // A document ending in a newline has an empty line after it, and that is
        // the line the gutter must not number: there is nothing there to scroll to
        // and nothing to put a caret in.
        let expected = doc.split('\n').count() - usize::from(doc.ends_with('\n'));
        let numbers: Vec<usize> = h
            .gutter_numbers()
            .iter()
            .map(|n| {
                n.parse().unwrap_or_else(|_| {
                    panic!(
                        "the gutter drew {n:?}, which is not a number; the whole gutter was {:?}",
                        h.gutter_numbers()
                    )
                })
            })
            .collect();
        assert_eq!(
            numbers,
            (1..=expected).collect::<Vec<_>>(),
            "{doc:?} has {expected} lines"
        );
    }
}

#[test]
fn a_click_in_the_middle_of_a_line_puts_the_caret_where_the_pointer_is() {
    // A click has to land between the two characters either side of the pointer,
    // not at the end of the line. The end is where a click *past* the text goes,
    // and confusing the two is invisible until someone clicks on a line and the
    // caret jumps to the right margin.
    let doc = "a short line here\nand another one\nand a third";
    for row in 0..3 {
        let mut h = Harness::new(doc);
        h.frame();
        let width = h.advance();
        for col in 0..4 {
            h.click(h.pos_of(row, col as f32 + 0.5));
            h.frame();
            let caret = h.caret();
            let line_start = doc
                .split('\n')
                .take(row)
                .map(|l| l.chars().count() + 1)
                .sum::<usize>();
            let into = caret - line_start;
            assert!(
                into == col || into == col + 1,
                "clicking halfway into character {col} of row {row} put the caret \
                 {into} characters into the line, at {caret}"
            );
        }
        let _ = width;
    }
}

#[test]
fn a_click_past_the_end_of_a_lines_text_goes_to_the_end_of_that_line() {
    // The other half of the rule above: a click in the empty space to the right of
    // a line, still on that line's row, belongs to that line's end. Not to the
    // end of the document, and not to the start of the next line.
    let doc = "short\n\nalso short\nlast";
    let mut h = Harness::new(doc);
    h.frame();
    h.click(h.pos_of(0, 40.0));
    h.frame();
    assert_eq!(h.caret(), 5, "the end of the first line");
    let mut h = Harness::new(doc);
    h.frame();
    h.click(h.pos_of(2, 40.0));
    h.frame();
    assert_eq!(
        h.caret(),
        17,
        "the end of the third line, not the document's"
    );
}

#[test]
fn the_caret_is_drawn_where_the_click_puts_it_on_the_same_frame() {
    // The complaint this pins down is that clicking felt loose: the caret landed a
    // frame late, so it appeared at the old place and then jumped. Resolving the
    // click after painting is exactly that - the frame draws the caret where it
    // was, and only the next frame draws it where the pointer was. So the drawn
    // rectangle is checked on the frame of the click itself, with no extra frame in
    // between to hide the lag.
    let mut h = Harness::new("alpha beta gamma");
    h.frame();
    click_char(&mut h, 0, 0);
    for col in 1..=9 {
        h.click(h.pos_of(0, col as f32 + 0.5));
        let caret = h.caret();
        assert!(
            caret == col || caret == col + 1,
            "clicking character {col} put the caret at {caret}"
        );
        // Where the caret *should* be drawn: one character width per column from
        // the left of the text. An absolute check rather than a difference from
        // the last one, so a caret stuck in one place fails as loudly as one in
        // the wrong place.
        let want = h.text_left() + h.advance() * caret as f32;
        let drawn = h
            .caret_rect()
            .unwrap_or_else(|| panic!("the caret vanished on the click frame at {col}"));
        assert!(
            (drawn.left() - want).abs() < 1.0,
            "the caret is at character {caret}, so it should be drawn at x={want:.1}, \
             but it is at x={:.1} - the frame is drawing where the caret used to be",
            drawn.left()
        );
    }
}

#[test]
fn a_drag_paints_the_selection_it_has_reached() {
    // The same lag, for the selection rather than the caret. A drag is recognised
    // on the frame the pointer moves, and the highlight has to be there on that
    // frame: a reader dragging across a line watches the highlight, and a
    // one-frame-old highlight reads as the drag not being tracked.
    let mut h = Harness::new("alpha beta gamma delta");
    h.frame();
    // Press at the start of the line and drag halfway along it, still held down.
    h.pointer(h.pos_of(0, 0.0), true);
    h.press_and_move_to(h.pos_of(0, 6.5));
    let caret = h.caret();
    assert!(
        (6..=7).contains(&caret),
        "dragging to the middle of the line put the caret at {caret}"
    );
    let want = h.text_left() + h.advance() * caret as f32;
    let drawn = h.caret_rect().expect("the caret is drawn during a drag");
    assert!(
        (drawn.left() - want).abs() < 1.0,
        "the caret is at character {caret}, so it should be drawn at x={want:.1}, \
         but it is at x={:.1}",
        drawn.left()
    );
    assert_eq!(
        h.selected(),
        "alpha b",
        "and the highlight covers the drag on the same frame, not the next"
    );
    h.pointer(h.pos_of(0, 6.5), false);
}

#[test]
fn dragging_over_a_wrapped_line_highlights_every_row_it_crosses() {
    // With soft wrap on, one line is several rows. A selection is painted one row
    // at a time, and the wash on a row is the width between two cursor positions
    // in the *shaped* text. At a row break the cursor that ends a row and the
    // cursor that starts the next one are the same index with two different
    // positions, and taking the wrong one of the two is how the wrapped tail of a
    // selection ends up with no wash on it at all.
    //
    // So this drags over three rows of one wrapped line and checks each of them,
    // by the row it belongs to, rather than checking that some highlight exists.
    let mut h = Harness::new(&"x".repeat(400));
    h.set_wrap(true);
    h.resize(Vec2::new(200.0, 400.0));
    h.frame();
    assert!(
        h.row_count() >= 3,
        "400 characters did not wrap into three rows in a 200pt pane, so this test \
         proves nothing (got {})",
        h.row_count()
    );

    let a = h.pos_of(0, 0.0);
    let b = h.pos_of(2, 6.0);
    h.pointer(a, true);
    h.press_and_move_to(b);
    h.pointer(b, false);

    // The selection has to be real, or the rest of this says nothing. Its far end
    // has to be on row 2, which is the row the pointer was released on.
    let third = h.rows()[2].chars.0 + h.window_base();
    assert!(
        h.caret() >= third,
        "the drag reached character {} but row 2 starts at {third}",
        h.caret()
    );
    let rects = h.selection_rects();
    for row in 0..3 {
        let top = h.row_tops()[row];
        let on_this_row = rects
            .iter()
            .any(|r| (r.top() - top).abs() < 1.0 && r.width() > 0.5);
        assert!(
            on_this_row,
            "row {row} (top {top:.1}) has no selection wash on it, though the drag \
             crossed it. The wash is on {rects:?} and the rows start at {:?}",
            h.row_tops()
        );
    }
}

#[test]
fn a_selection_ending_mid_line_is_washed_to_the_nearest_glyph_edge() {
    // The other half of the same thing, and the reason the wash is drawn from
    // glyph boundaries rather than from character counts: half a character is
    // still half a wash, and the wash must not run past the character it is over.
    let mut h = Harness::new("alpha beta gamma");
    h.frame();
    let a = h.pos_of(0, 0.0);
    let b = h.pos_of(0, 6.5);
    h.pointer(a, true);
    h.press_and_move_to(b);
    h.pointer(b, false);
    let rects = h.selection_rects();
    let r = rects.first().expect("the selection was washed");
    let advance = h.advance();
    assert!(
        (r.width() - advance * 6.5).abs() < advance,
        "the wash is {:.1}pt wide for a selection of 6.5 characters of {advance:.1}pt",
        r.width()
    );
    assert!(
        r.right() <= h.text_left() + advance * 7.0 + 0.5,
        "the wash runs to {:.1}, past the end of the selection",
        r.right()
    );
}

// ---- the drawing -----------------------------------------------------------

#[test]
fn the_gutter_numbers_the_lines_that_are_on_screen() {
    let doc = (0..30)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut h = Harness::new(&doc);
    h.frame();
    let numbers = h.gutter_numbers();
    // The pane is 600pt tall, so not all thirty lines fit. The ones drawn have
    // to start at 1 and run consecutively, with no gaps and no repeats.
    assert!(!numbers.is_empty(), "no line numbers were drawn");
    for (i, n) in numbers.iter().enumerate() {
        assert_eq!(
            n,
            &(i + 1).to_string(),
            "the gutter drew {numbers:?}, which skips or repeats"
        );
    }
}

#[test]
fn a_wrapped_line_spans_several_rows_and_still_tiles() {
    // Wrapping gives up the gutter, so what matters here is that the rows tile
    // the window: several rows for one line, no gap between them, and the lines
    // they belong to going forwards. The caret and the click test both depend on
    // exactly that.
    let long = "z".repeat(400);
    let opts = Options {
        line_numbers: false,
        wrap: true,
        ..Options::default()
    };
    let mut h = Harness::with_options(&format!("{long}\n{long}"), opts, egui::vec2(300.0, 400.0));
    h.frame();
    assert!(
        h.row_count() > 3,
        "a 400-character line should wrap over several rows, got {}",
        h.row_count()
    );
    assert_rows_tile(&h);
}

#[test]
fn a_wrapped_line_is_one_number_however_many_rows_it_takes() {
    // With wrapping on there is no gutter, so the line numbers come from the
    // editor's own record of which line each row belongs to. Several rows
    // sharing a line is correct; the same line being numbered repeatedly is not.
    let long = "w".repeat(300);
    let opts = Options {
        line_numbers: false,
        wrap: true,
        ..Options::default()
    };
    let mut h = Harness::with_options(&format!("{long}\nshort"), opts, egui::vec2(260.0, 400.0));
    h.frame();
    assert!(
        h.row_count() > 4,
        "300 characters in a narrow pane should wrap over several rows, got {}",
        h.row_count()
    );
    let lines = h.drawn_line_numbers();
    assert!(!lines.is_empty(), "no line numbers were recorded at all");
    for pair in lines.windows(2) {
        assert!(
            pair[1] >= pair[0],
            "line numbers went backwards: {lines:?}, so a wrapped line is being \
             numbered more than once"
        );
    }
}

/// The invariant that makes a hit test and a caret position meaningful: the
/// rows cover the shaped text exactly once, in order, with no gap and no
/// overlap.
///
/// A gap means a character that belongs to no row, so a click there cannot be
/// resolved to a position and the caret cannot be drawn. An overlap means one
/// character is claimed by two rows, so it lands on the wrong one. Both are
/// invisible until someone clicks in the wrong place, which is why they are
/// checked over generated documents rather than a handful of examples.
fn assert_rows_tile(h: &Harness) {
    let rows = h.rows();
    assert_eq!(
        rows.len(),
        h.row_count(),
        "the editor recorded {} rows but laid out {}",
        rows.len(),
        h.row_count()
    );
    assert_eq!(
        rows.first().map_or(0, |r| r.chars.0),
        0,
        "the first row does not start at the beginning of the window"
    );
    for (i, pair) in rows.windows(2).enumerate() {
        let (a, b) = (pair[0], pair[1]);
        if a.line != b.line {
            // Between two rows of different lines sits the newline that ended the
            // first line, which no row covers because no row draws it. That gap
            // is one character wide and it is correct: a caret may sit on it, and
            // a click on it resolves to the end of the earlier line.
            assert_eq!(
                b.chars.0,
                a.chars.1 + 1,
                "rows {i} and {} are on lines {} and {}, so exactly one \
                 character - the newline - should be between them, but the gap \
                 is {} to {}: {rows:?}",
                i + 1,
                a.line,
                b.line,
                a.chars.1,
                b.chars.0
            );
        } else {
            assert_eq!(
                a.chars.1,
                b.chars.0,
                "rows {i} and {} are both on line {}, so they must meet \
                 exactly: {rows:?}",
                i + 1,
                a.line
            );
        }
        assert!(
            a.line <= b.line,
            "row {i} belongs to line {} and row {} to line {}, so the lines go \
             backwards: {rows:?}",
            a.line,
            i + 1,
            b.line
        );
    }
    let covered = rows.iter().filter(|r| r.chars.1 > r.chars.0).count();
    assert!(
        covered > 0,
        "no row covers any characters, so nothing can be hit-tested: {rows:?}"
    );
    // The last row has to end at the end of the shaped text, or the characters
    // after it belong to no row.
    let last = rows.last().expect("at least one row");
    assert_eq!(
        last.chars.1,
        h.shaped_len(),
        "the rows cover {} characters but {} were laid out: {rows:?}",
        last.chars.1,
        h.shaped_len()
    );
}

// ---- properties ------------------------------------------------------------

/// The invariant that ties the whole editor together: whatever else happens,
/// every drawn row's character range must tile the window exactly once, with no
/// gap and no overlap, and the caret must be on the row its character is in.
///
/// Run over random documents, random scroll positions, both wrap settings and
/// random carets, because the bugs this catches were all "off by one per line",
/// which a hand-written case will usually agree with.
#[test]
fn the_rows_always_tile_the_window_and_the_caret_is_on_its_own_row() {
    for seed in 1..40u64 {
        for wrap in [false, true] {
            let mut rng = Rng::new(seed * 7919);
            let how_many = 12 + rng.below(40);
            let doc = random_doc(&mut rng, how_many);
            let opts = Options {
                line_numbers: !wrap,
                wrap,
                ..Options::default()
            };
            let mut h = Harness::with_options(&doc, opts, egui::vec2(340.0, 260.0));
            h.frame();
            // Scroll somewhere, and put the caret somewhere else.
            h.scroll(-40.0 * (rng.below(20) as f32));
            h.frame();
            // Click somewhere on screen, then scroll the caret into view, so the
            // property is checked with the caret both under the pointer and
            // arrived at by key - the two ways a caret gets placed.
            let row = rng.below(h.row_count());
            let col = rng.below(20);
            click_char(&mut h, row, col);
            h.frame();
            if rng.chance(50) {
                h.key(Key::End);
                h.key(Key::Home);
            }
            h.frame();

            assert_rows_tile(&h);
            // A click can leave the caret below the last line, where there is no
            // row to draw it on. That is correct - the caret is then at the end
            // of the document, which is where a click past the text belongs - so
            // it is only checked when a row was drawn at all.
            if let Some(drawn) = h.caret_row() {
                let local = h.caret().saturating_sub(h.window_base());
                let want = h.drawn_row_of(local);
                assert_eq!(
                    drawn,
                    want,
                    "seed {seed} wrap {wrap}: the caret is at character {} (window \
                     character {local}) drawn on row {drawn}, but that character is \
                     on row {want}",
                    h.caret()
                );
            } else if h.focused() {
                // No caret was drawn even though the editor has the keyboard, so
                // the only remaining reason is that it is outside the window.
                let base = h.window_base();
                assert!(
                    h.caret() < base || h.caret() >= h.shaped_len() + base,
                    "seed {seed} wrap {wrap}: the caret at {} is inside the window \
                     {}..{} but was not drawn",
                    h.caret(),
                    base,
                    base + h.shaped_len()
                );
            }
        }
    }
}

#[test]
fn no_sequence_of_keystrokes_can_panic_or_lose_the_buffer() {
    // A fuzzer over the real key path. It is not looking for a particular
    // behaviour, only for the two things that must never happen: a panic, and a
    // caret outside the document. Both were possible while the row arithmetic
    // was wrong.
    let keys = [
        Key::ArrowLeft,
        Key::ArrowRight,
        Key::ArrowUp,
        Key::ArrowDown,
        Key::Home,
        Key::End,
        Key::PageUp,
        Key::PageDown,
        Key::Backspace,
        Key::Delete,
        Key::Enter,
        Key::Tab,
        Key::A,
        Key::Z,
        Key::Y,
        Key::Slash,
        Key::F1,
    ];
    let mods = [
        Modifiers::default(),
        SHIFT,
        CTRL,
        SHIFT | CTRL,
        Modifiers::ALT,
    ];
    for seed in 1..30u64 {
        let mut rng = Rng::new(seed * 104_729);
        let doc = random_doc(&mut rng, 30);
        let mut h = Harness::new(&doc);
        h.frame();
        for step in 0..120 {
            // The exact step is reported, so a failure can be replayed on its own
            // rather than by re-running the whole sequence and hoping.
            let key = keys[rng.below(keys.len())];
            let m = mods[rng.below(mods.len())];
            h.key_mod(key, m);
            if rng.chance(30) {
                let text: String = WORDS[rng.below(6)].to_owned();
                h.type_text(&text);
            }
            if rng.chance(10) {
                h.paste("pasted");
            }
            if rng.chance(15) {
                let row = rng.below(h.row_count());
                h.click(h.pos_of(row, rng.below(30) as f32));
            }
            let len = h.text().chars().count();
            assert!(
                h.caret() <= len,
                "seed {seed} step {step} ({key:?} with {m:?}): the caret is at {} \
                 in a document of {len} characters: {:?}",
                h.caret(),
                h.text()
            );
            let (lo, hi) = h.selection();
            assert!(
                lo <= hi && hi <= len,
                "seed {seed} step {step} ({key:?} with {m:?}): the selection is \
                 {lo}..{hi} in a document of {len} characters"
            );
            // Nothing may split a multi-byte character. Every read of the buffer
            // goes through a character-to-byte conversion, and one of those
            // landing mid-character would panic rather than corrupt anything, so
            // this is the check that the conversions are all still in step.
            assert!(
                h.text().is_char_boundary(0),
                "the buffer is not valid text: {:?}",
                h.text()
            );
        }
    }
}

const WORDS: &[&str] = &["a", "bb", "let x", "\"s\"", "// c", "'", "é", "\t", "()"];

#[test]
fn an_edit_is_in_the_text_of_the_frame_that_made_it() {
    // The complaint this pins down is that typing feels loose, and the cause is
    // not the caret and not the layout: the editor used to lay the page out
    // *before* reading the keyboard, so the frame a character was typed on drew
    // the document as it was before the character. At sixty frames a second that
    // is a visible lag between the key and the letter, on every key.
    //
    // Checked on the frame of the keystroke itself, with no extra frame in
    // between to hide it, and checked against the *shaped text* rather than
    // against the buffer - the buffer is right by definition, since the buffer is
    // what the keystroke changed.
    let mut h = Harness::new("first line\n\nlast line\n");
    h.click(h.pos_of(2, 3.0));
    h.key(Key::End);
    h.type_text("!");
    assert_eq!(
        h.text(),
        "first line\n\nlast line!\n",
        "the keystroke did not reach the buffer"
    );
    assert_eq!(
        h.job_text(),
        h.text(),
        "the frame that typed the character did not draw it"
    );
    // And the same for the keystrokes that add and remove whole lines, which move
    // every line number in the document and so invalidate the index hardest.
    h.key(Key::Enter);
    assert_eq!(
        h.job_text(),
        h.text(),
        "a newline was not drawn when pressed"
    );
    h.type_text("x");
    assert_eq!(
        h.job_text(),
        h.text(),
        "a character was not drawn when typed"
    );
    h.key(Key::Backspace);
    assert_eq!(
        h.job_text(),
        h.text(),
        "a backspace was not drawn when pressed"
    );
    h.key_mod(Key::A, Modifiers::CTRL);
    h.type_text("replaced");
    assert_eq!(
        h.job_text(),
        h.text(),
        "a replacement was not drawn when typed"
    );
}

#[test]
fn a_deleted_line_is_gone_from_the_text_of_the_frame_that_deleted_it() {
    // The other direction, and the one that used to be worse: a deletion makes
    // the buffer shorter, so the index built at the top of the frame no longer
    // matches it, and a stale index slices lines at offsets that have moved.
    // The line the caret was on comes back with a character of the *next* line
    // glued to its end.
    let mut h = Harness::new("alpha\nbravo\ncharlie\ndelta\n");
    h.click(h.pos_of(1, 2.0));
    h.key_mod(Key::K, Modifiers::CTRL | Modifiers::SHIFT);
    assert_eq!(
        h.text(),
        "alpha\ncharlie\ndelta\n",
        "the line was not deleted"
    );
    assert_eq!(
        h.job_text(),
        h.text(),
        "the frame that deleted the line drew the document without it"
    );
    assert_eq!(
        h.gutter_numbers(),
        vec!["1", "2", "3"],
        "and the line numbers went with it: three lines left, three numbers"
    );
}

#[test]
fn probe_scroll_y() {
    let doc = (0..400)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut h = Harness::new(&doc);
    h.frame();
    eprintln!(
        "start: scroll_y={} top={:?} room={}",
        h.scroll_y(),
        h.top_line(),
        h.scroll_room()
    );
    h.scroll(2000.0);
    eprintln!(
        "after 2000: scroll_y={} top={:?}",
        h.scroll_y(),
        h.top_line()
    );
    h.scroll(100_000.0);
    eprintln!(
        "after 100k: scroll_y={} top={:?}",
        h.scroll_y(),
        h.top_line()
    );
}

#[test]
fn the_wheel_moves_the_text_by_fractions_of_a_row() {
    // The whole of "it does not scroll smoothly". A window positioned in whole rows
    // cannot scroll smoothly: a trackpad reports a fraction of a row every frame,
    // and a whole-row window either discards that fraction or jumps a row at a time.
    // Both of those feel like the same thing to the hand on the wheel.
    //
    // So the assertion is about the points, not the lines: a small wheel movement
    // has to move the text by a small number of points, and the row the window
    // starts at must not change until a whole row has gone by. If this ever fails
    // because a small movement moved a whole row, the window has gone back to being
    // a row counter.
    let doc = "line\n".repeat(400);
    let mut h = Harness::new(&doc);
    h.frame();
    let row = h.row_height();
    let before = h.row_tops()[0];

    let after = h.wheel_once(0.05);
    let moved = (before - h.row_tops()[0]).abs();
    assert!(
        moved > 0.5,
        "a small wheel movement did not move the text at all ({moved:.1}pt), which is \
         what a window that rounds every frame's share to whole rows does"
    );
    assert!(
        moved < row * 0.6,
        "a small wheel movement moved the text {moved:.1}pt, and a row is {row:.1}pt. A \
         whole-row window cannot do this: it moves a whole row or it does not move"
    );
    assert_eq!(
        h.top_line(),
        Some(0),
        "and the window is still on the first line, because no whole row has passed"
    );
    // The window's own position is the same movement, not a rounded version of it.
    assert!(
        (after - moved).abs() < 1.0,
        "the window scrolled to {after:.1} but the text moved {moved:.1}pt, so the two \
         disagree about how far the document has been scrolled"
    );
}

#[test]
fn a_wheel_scroll_accumulates_so_a_slow_gesture_still_moves() {
    // The other half of not being smooth: several small movements have to add up, or
    // a high-resolution wheel appears to do nothing however long it is turned. Sent
    // one frame at a time with no settling between, which is exactly what a slow
    // drag looks like.
    let doc = "line\n".repeat(400);
    let mut h = Harness::new(&doc);
    h.frame();
    let row = h.row_height();
    // Turned until the window is more than a row down, in movements small enough that
    // no single one of them is a row.
    let mut n = 0;
    while h.scroll_y() < row * 2.0 && n < 200 {
        h.wheel_once(0.01);
        n += 1;
    }
    assert!(
        n < 200,
        "40 small wheel movements never added up to two rows, so a slow drag does \
         nothing: {n} movements reached {moved:.1}pt",
        moved = h.scroll_y()
    );
    assert!(
        h.top_line().unwrap_or(0) >= 2,
        "and having moved two whole rows down, the window should be at least two lines \
         down, not {}",
        h.top_line().unwrap_or(0)
    );
}

#[test]
fn the_window_cannot_scroll_past_the_last_screenful() {
    // The end of a document is a real place and a reader has to be able to get
    // there. A window that scrolled until its last line was off the bottom would be
    // a file whose end cannot be seen.
    let doc = "line\n".repeat(400);
    let mut h = Harness::new(&doc);
    h.frame();
    h.scroll(1_000_000.0);
    let room = h.scroll_room();
    assert_eq!(
        h.top_line(),
        Some(room),
        "the furthest the window goes is the last screenful, and no further"
    );
    let shown = h.drawn_line_numbers();
    assert!(
        shown.contains(&(400 - 1)),
        "and the last line of the document is one of the lines shown; the window is \
         showing lines {shown:?}"
    );
}

#[test]
fn a_drag_starts_the_selection_where_the_pointer_went_down() {
    // The complaint this pins down: press somewhere, drag, and the highlight begins
    // somewhere the pointer has never been. A drag is never registered as a click -
    // egui drops the claim as soon as the pointer moves past the threshold - so the
    // click branch, which is what sets the anchor, never runs, and the anchor is
    // left wherever the caret last was. Everything then selects from *there*, which
    // is a selection the reader did not ask for and cannot account for.
    //
    // The caret is put far away first, so an anchor left alone is unmistakable.
    let mut h = Harness::new("alpha bravo charlie delta echo foxtrot");
    h.frame();
    h.click(h.pos_of(0, 30.0));
    h.frame();
    let parked = h.caret();
    assert!(parked > 20, "the caret is parked near the end of the line");

    // Press at the start of the line and drag to the middle, without letting go.
    h.pointer(h.pos_of(0, 0.0), true);
    h.press_and_move_to(h.pos_of(0, 11.0));

    let (lo, hi) = h.selection();
    assert_eq!(
        lo, 0,
        "the selection should begin at the start of the line, where the pointer went \
         down, and it begins at {lo} (the caret was parked at {parked})"
    );
    assert!(
        (10..=11).contains(&hi),
        "and end where the pointer is, near character 11, not {hi}"
    );
    assert_eq!(
        h.selected(),
        "alpha bravo",
        "so the text between the press and the pointer is what is selected"
    );
    h.pointer(h.pos_of(0, 11.0), false);
}

#[test]
fn a_drag_after_a_word_selects_from_the_word_not_from_the_old_caret() {
    // The same thing with a double click first, which is the case a reader hits
    // constantly: select a word, then drag somewhere else. The drag must replace the
    // selection, not grow it from the word that is still there.
    let mut h = Harness::new("alpha bravo charlie delta echo foxtrot");
    h.frame();
    h.double_click(h.pos_of(0, 8.0));
    h.frame();
    assert_eq!(
        h.selected(),
        "bravo",
        "a double click selects the word under the pointer"
    );

    h.pointer(h.pos_of(0, 20.0), true);
    h.press_and_move_to(h.pos_of(0, 24.0));
    let (lo, _) = h.selection();
    assert_eq!(
        lo, 20,
        "the drag starts at the pointer, not at the word that was selected before it"
    );
    h.pointer(h.pos_of(0, 24.0), false);
}

#[test]
fn a_shift_drag_extends_the_selection_that_is_already_there() {
    // The other half of the press: shift held when the drag begins extends from the
    // existing anchor rather than starting a new selection, which is what makes
    // shift-drag the way to add to a selection without retyping it.
    let mut h = Harness::new("alpha bravo charlie delta echo foxtrot");
    h.frame();
    h.click(h.pos_of(0, 6.0));
    h.frame();
    assert_eq!(
        h.anchor(),
        6,
        "and the click left the anchor where it clicked"
    );

    // Shift held for the whole press-and-drag.
    let a = h.pos_of(0, 12.0);
    let b = h.pos_of(0, 18.0);
    h.pointer_mod(a, Modifiers::SHIFT, true);
    h.press_and_move_to(b);
    assert_eq!(
        h.anchor(),
        6,
        "the anchor stays where the click put it, so the selection grows from there"
    );
    let (_, hi) = h.selection();
    assert!(
        (17..=18).contains(&hi),
        "and the caret follows the pointer to {hi}"
    );
    h.pointer_mod(b, Modifiers::SHIFT, false);
}

#[test]
fn ctrl_h_opens_a_replace_field_and_typing_goes_into_it() {
    let mut h = Harness::new("cat dog cat");
    click_char(&mut h, 0, 0);
    h.find("cat");
    h.replace_with("bird");
    assert_eq!(h.text(), "cat dog cat", "nothing was typed into the file");
    assert_eq!(h.replacement(), "bird");
    assert_eq!(h.find_needle(), "cat", "and the search was left alone");
}

#[test]
fn replace_swaps_the_current_match_and_moves_on_to_the_next() {
    let mut h = Harness::new("cat dog cat");
    click_char(&mut h, 0, 0);
    h.find("cat");
    h.replace_with("bird");
    h.press_replace(false);
    assert_eq!(h.text(), "bird dog cat");
    assert_eq!(
        h.selected(),
        "cat",
        "the next match is now the selected one"
    );
    h.press_replace(false);
    assert_eq!(h.text(), "bird dog bird");
    assert_eq!(h.find_hits(), 0, "and there is nothing left to find");
}

#[test]
fn replace_all_swaps_every_match_including_adjacent_ones_and_undoes_in_one_step() {
    let mut h = Harness::new("aa aé aa");
    click_char(&mut h, 0, 0);
    h.find("a");
    h.replace_with("xyz");
    h.press_replace(true);
    assert_eq!(h.text(), "xyzxyz xyzé xyzxyz");
    // Back into the text, which is where undo lives.
    click_char(&mut h, 0, 0);
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), "aa aé aa", "one Ctrl+Z puts it all back");
}

#[test]
fn a_replacement_that_contains_the_needle_does_not_loop() {
    let mut h = Harness::new("a b a");
    click_char(&mut h, 0, 0);
    h.find("a");
    h.replace_with("aa");
    h.press_replace(false);
    h.press_replace(false);
    assert_eq!(
        h.text(),
        "aa b aa",
        "each match was replaced once and only once"
    );
}

#[test]
fn nothing_is_replaced_in_a_read_only_document() {
    let mut h = Harness::new("cat");
    h.set_editable(false);
    h.find("cat");
    h.replace_with("dog");
    h.press_replace(true);
    assert_eq!(h.text(), "cat");
}

#[test]
fn the_hits_follow_the_text_when_it_is_edited_with_the_bar_open() {
    let mut h = Harness::new("cat cat");
    click_char(&mut h, 0, 0);
    h.find("cat");
    assert_eq!(h.find_hits(), 2);
    // Back into the text and delete a whole match.
    click_char(&mut h, 0, 7);
    for _ in 0..4 {
        h.key(Key::Backspace);
    }
    assert_eq!(h.text(), "cat");
    assert_eq!(h.find_hits(), 1, "the count is of the text as it is now");
}

#[test]
#[ignore]
fn probe_glyphs_for_awkward_text() {
    for line in [
        "combining e\u{301} marks a\u{308}",
        "\ttab",
        "emoji \u{1F600} x",
        "\u{65E5}\u{672C}",
    ] {
        let h = Harness::new(line);
        for shape in &h.drawn_shapes() {
            let egui::Shape::Text(t) = &shape.shape else {
                continue;
            };
            if t.galley.job.text != line {
                continue;
            }
            for row in &t.galley.rows {
                let chrs: Vec<String> = row
                    .row
                    .glyphs
                    .iter()
                    .map(|g| format!("{:?}@{:.1}+{:.1}", g.chr, g.pos.x, g.advance_width))
                    .collect();
                eprintln!(
                    "{line:?} chars={} glyphs={}: {chrs:?}",
                    line.chars().count(),
                    row.row.glyphs.len()
                );
            }
        }
    }
}

#[test]
#[ignore]
fn probe_typing_brackets() {
    for s in ["f() {", "\"a\"", "(a)", "ab"] {
        let mut h = Harness::new("");
        let mut steps = Vec::new();
        for ch in s.chars() {
            h.type_like_a_person(&ch.to_string());
            steps.push(format!("{ch}->{:?}", h.text()));
        }
        eprintln!("typing {s:?}: {steps:?}");
    }
}

#[test]
#[ignore]
fn probe_emoji_long_line() {
    let text = "\u{1F600} y ".repeat(9_000);
    let n = text.chars().count();
    let mut h = Harness::new(&text);
    h.key(Key::End);
    h.frame();
    h.frame();
    let caret = h.caret_rect().unwrap();
    eprintln!(
        "n {n} scroll_x {} caret {:?} pane {:?}",
        h.scroll_x(),
        caret,
        h.rect()
    );
    eprintln!(
        "adv {} shaped_len {} rows {:?}",
        h.advance(),
        h.shaped_len(),
        h.rows()
    );
    eprintln!("origin {:?}", h.debug_origin());
    h.click(Pos2::new(caret.left() - 40.0, caret.center().y));
    eprintln!("after click caret {} scroll_x {}", h.caret(), h.scroll_x());
}

#[test]
#[ignore]
fn probe_units_in_a_row() {
    for unit in ["a\tb ", "\u{e9}\u{4e2d}x ", "\u{1F600} y "] {
        let text = unit.repeat(9_000);
        let n = text.chars().count();
        let mut h = Harness::new(&text);
        h.key(Key::End);
        h.frame();
        h.frame();
        let caret = h.caret_rect().unwrap();
        eprintln!("{unit:?}: n {n} scroll_x {} caret {caret:?}", h.scroll_x());
        eprintln!(
            "  adv {} shaped {} origin {:?}",
            h.advance(),
            h.shaped_len(),
            h.debug_origin()
        );
        h.click(Pos2::new(caret.left() - 40.0, caret.center().y));
        eprintln!(
            "  after click caret {} scroll_x {} rect {:?}",
            h.caret(),
            h.scroll_x(),
            h.caret_rect()
        );
    }
}

#[test]
#[ignore]
fn probe_sideways_wheel() {
    let long: String = (0..400).map(|i| format!("{i} ")).collect();
    let mut h = Harness::new(&long);
    h.frame();
    eprintln!("before: scroll_x {}", h.scroll_x());
    h.wheel_sideways(5.0);
    eprintln!("after shift+wheel: scroll_x {}", h.scroll_x());
}

#[test]
#[ignore]
fn probe_long_line_geometry() {
    let text = long_line();
    let n = text.chars().count();
    let mut h = Harness::new(&text);
    h.key(Key::End);
    h.frame();
    h.frame();
    eprintln!(
        "n {n} adv {} scroll_x {} caret_rect {:?} pane {:?}",
        h.advance(),
        h.scroll_x(),
        h.caret_rect(),
        h.rect()
    );
    eprintln!(
        "glyph_extent {:?} shaped_len {} rows {:?}",
        h.glyph_extent(),
        h.shaped_len(),
        h.rows()
    );
    eprintln!(
        "row text tail: {:?}",
        h.row_text(0)
            .chars()
            .rev()
            .take(30)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>()
    );
    eprintln!("origin {:?} text_left {}", h.debug_origin(), h.text_left());
}

#[test]
#[ignore]
fn probe_huge_line_costs() {
    let doc = "abcdefghij ".repeat(100_000);
    for (label, highlight) in [("highlight on", true), ("highlight off", false)] {
        let mut h = Harness::new(&doc);
        h.set_highlight(highlight);
        h.set_lang(BLOCKY);
        for _ in 0..3 {
            h.frame();
        }
        let t = std::time::Instant::now();
        for _ in 0..20 {
            h.frame();
        }
        let idle = t.elapsed().as_secs_f64() * 1000.0 / 20.0;
        let t = std::time::Instant::now();
        h.type_like_a_person("hi");
        let typing = t.elapsed().as_secs_f64() * 1000.0 / 4.0;
        eprintln!("{label}: idle frame {idle:.1} ms, keystroke {typing:.1} ms");
    }
}

#[test]
#[ignore]
fn probe_wrapped_click_and_type() {
    let mut h = wrapped();
    h.scroll(4.5);
    h.frame();
    eprintln!("after scroll: {}", h.scroll_debug());
    eprintln!("rows[0..8]: {:?}", &h.rows()[..8]);
    eprintln!("row_tops[0..8]: {:?}", &h.row_tops()[..8]);
    let p = h.pos_of(3, 1.0);
    eprintln!("click at {p:?}, pane {:?}", h.rect());
    h.click(p);
    h.frame();
    eprintln!("caret {} line_col {:?}", h.caret(), h.line_column());
    let mut h = wrapped();
    h.scroll(1.0);
    h.frame();
    eprintln!("A scrolled 1: {}", h.scroll_debug());
    h.click(h.pos_of(3, 1.0));
    h.frame();
    eprintln!("B clicked: {} caret {}", h.scroll_debug(), h.caret());
    h.scroll(4.5);
    h.frame();
    eprintln!("C scrolled 4.5 more: {}", h.scroll_debug());
    let mut h = wrapped();
    h.key_mod(Key::End, CTRL);
    eprintln!(
        "at end: {} caret_rect {:?}",
        h.scroll_debug(),
        h.caret_rect()
    );
    h.type_like_a_person("more words more words more words more words more words more words");
    eprintln!(
        "typed: {} caret_rect {:?} pane {:?}",
        h.scroll_debug(),
        h.caret_rect(),
        h.rect()
    );
}

#[test]
#[ignore]
fn probe_wrapped_scroll_state() {
    let mut h = wrapped();
    for i in 0..8 {
        h.raw_wheel(30.0);
        eprintln!("raw {i}: {}", h.scroll_debug());
    }
    for i in 0..30 {
        h.wheel_move(-3.0);
        eprintln!("{i:2} {}", h.scroll_debug());
    }
    for i in 0..60 {
        h.frame();
        if i % 6 == 0 {
            eprintln!("settle {i:2} {}", h.scroll_debug());
        }
    }
}

/// How far one particular line moves on screen each frame while a wheel turns
/// steadily, unwrapped and wrapped. Smooth scrolling is a run of similar small
/// numbers; a jump is a number many times its neighbours.
/// `cargo test --release probe_on_screen_scroll_steps -- --ignored --nocapture`
#[test]
#[ignore]
fn probe_on_screen_scroll_steps() {
    let doc = (0..200)
        .map(|i| format!("L{i}: {}", "word ".repeat(if i % 3 == 0 { 70 } else { 8 })))
        .collect::<Vec<_>>()
        .join("\n");
    for wrap in [false, true] {
        let mut h = Harness::new(&doc);
        h.set_wrap(wrap);
        h.frame();
        // Where line `n` is on screen, if any of it is.
        let y_of = |h: &Harness, n: usize| -> Option<f32> {
            let tops = h.row_tops();
            h.rows()
                .iter()
                .position(|r| r.line == n)
                .and_then(|i| tops.get(i).copied())
        };
        let mut ys: Vec<(f32, f32)> = Vec::new();
        for _ in 0..90 {
            // 0.05 of a notch: two points, a fine steady turn.
            h.wheel_move(-0.05);
            let scroll = h.scroll_y();
            if let Some(y) = y_of(&h, 6) {
                ys.push((scroll, y));
            }
        }
        let steps: Vec<f32> = ys
            .windows(2)
            .map(|w| ((w[1].1 - w[0].1) * 10.0).round() / 10.0)
            .collect();
        eprintln!("wrap {wrap}: line 6 moved per frame {steps:?}");
    }
}

/// What a fresh harness costs, which every swept document pays.
/// `cargo test --release probe_harness_setup_cost -- --ignored --nocapture`
#[test]
#[ignore]
fn probe_harness_setup_cost() {
    use std::time::Instant;
    let t = Instant::now();
    for _ in 0..200 {
        let _ = Harness::new("abc\ndef");
    }
    eprintln!(
        "Harness::new: {:.2} ms each",
        t.elapsed().as_secs_f64() * 1000.0 / 200.0
    );
    let mut h = Harness::new("abc\ndef");
    let t = Instant::now();
    for _ in 0..200 {
        h.frame();
    }
    eprintln!(
        "one frame:    {:.3} ms each",
        t.elapsed().as_secs_f64() * 1000.0 / 200.0
    );
}

/// Frame times for typing into a realistic file, printed rather than asserted:
/// `cargo test --release probe_typing_frame_times -- --ignored --nocapture`.
#[test]
#[ignore]
fn probe_typing_frame_times() {
    use std::time::Instant;
    let line = "    let value = compute(alpha, beta) + gamma; // a plausible line of code\n";
    for (label, lines, wrap, find) in [
        ("3k lines", 3_000, false, false),
        ("3k lines, wrap", 3_000, true, false),
        ("3k lines, find open", 3_000, false, true),
        ("100k lines", 100_000, false, false),
    ] {
        let doc = line.repeat(lines);
        let mut h = Harness::new(&doc);
        h.set_wrap(wrap);
        h.set_lang(BLOCKY);
        if find {
            h.find("value");
        }
        click_char(&mut h, lines / 2 % 40, 10);
        let mut times = Vec::new();
        for i in 0..300 {
            let t = Instant::now();
            h.type_text(if i % 20 == 19 { "\n" } else { "x" });
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(|a, b| a.total_cmp(b));
        eprintln!(
            "{label:22} median {:.2} ms  p99 {:.2} ms  max {:.2} ms",
            times[times.len() / 2],
            times[times.len() * 99 / 100],
            times[times.len() - 1]
        );
    }
}

// ---- what every editor does -----------------------------------------------------
//
// One rule per test, each a thing that behaves the same in every editor a person has
// used, so that a difference here is a bug and not a preference.

#[test]
fn left_with_a_selection_collapses_it_to_its_start_and_right_to_its_end() {
    let mut h = Harness::new("one two three");
    click_char(&mut h, 0, 4);
    for _ in 0..3 {
        h.key_mod(Key::ArrowRight, SHIFT);
    }
    assert_eq!(h.selected(), "two");
    h.key(Key::ArrowLeft);
    assert_eq!(
        h.caret(),
        4,
        "Left drops the selection and goes to where it began"
    );
    assert_eq!(h.selected(), "");
    for _ in 0..3 {
        h.key_mod(Key::ArrowRight, SHIFT);
    }
    h.key(Key::ArrowRight);
    assert_eq!(h.caret(), 7, "Right drops it and goes to where it ended");
}

#[test]
fn an_apostrophe_inside_a_word_is_just_an_apostrophe() {
    let mut h = Harness::new("");
    h.type_like_a_person("don't stop");
    assert_eq!(h.text(), "don't stop", "no second quote appeared");
}

#[test]
fn a_quote_after_a_space_opens_a_pair_and_typing_over_it_closes_it() {
    let mut h = Harness::new("");
    h.type_like_a_person("say \"hi\" now");
    assert_eq!(h.text(), "say \"hi\" now");
}

#[test]
fn a_bracket_typed_in_front_of_text_does_not_grab_a_closer() {
    let mut h = Harness::new("word");
    click_char(&mut h, 0, 0);
    h.type_like_a_person("(");
    assert_eq!(
        h.text(),
        "(word",
        "there is text right after, so no closer is added"
    );
}

#[test]
fn a_bracket_typed_at_the_end_of_a_line_or_before_a_space_still_closes() {
    let mut h = Harness::new("a b");
    click_char(&mut h, 0, 1);
    h.type_like_a_person("(");
    assert_eq!(h.text(), "a() b");
}

#[test]
fn backspace_inside_an_empty_pair_removes_both_halves() {
    let mut h = Harness::new("");
    h.type_like_a_person("f(");
    assert_eq!(h.text(), "f()");
    h.key(Key::Backspace);
    assert_eq!(
        h.text(),
        "f",
        "the opener and the closer it brought with it"
    );
}

#[test]
fn backspace_after_text_inside_a_pair_leaves_the_closer() {
    let mut h = Harness::new("");
    h.type_like_a_person("(ab");
    h.key(Key::Backspace);
    assert_eq!(h.text(), "(a)");
}

#[test]
fn control_backspace_and_delete_take_a_word() {
    let mut h = Harness::new("alpha beta gamma");
    click_char(&mut h, 0, 10);
    h.key_mod(Key::Backspace, CTRL);
    assert_eq!(h.text(), "alpha  gamma", "back over `beta`");
    h.key_mod(Key::Delete, CTRL);
    assert_eq!(
        h.text(),
        "alpha gamma",
        "forward to the start of the next word, the Windows way: it takes the gap"
    );
}

#[test]
fn control_shift_arrows_select_by_word_and_home_end_by_document() {
    let mut h = Harness::new("alpha beta gamma\nsecond line");
    click_char(&mut h, 0, 6);
    h.key_mod(Key::ArrowRight, Modifiers::CTRL | Modifiers::SHIFT);
    assert_eq!(h.selected(), "beta ");
    h.key_mod(Key::End, Modifiers::CTRL | Modifiers::SHIFT);
    assert_eq!(h.selected(), "beta gamma\nsecond line");
    h.key_mod(Key::Home, Modifiers::CTRL | Modifiers::SHIFT);
    assert_eq!(
        h.selected(),
        "alpha ",
        "the anchor stayed put and the caret went to the top"
    );
}

#[test]
fn backspace_and_delete_with_a_selection_remove_just_the_selection() {
    let mut h = Harness::new("hello brave world");
    click_char(&mut h, 0, 6);
    for _ in 0..6 {
        h.key_mod(Key::ArrowRight, SHIFT);
    }
    h.key(Key::Backspace);
    assert_eq!(h.text(), "hello world");
    assert_eq!(h.caret(), 6);
}

#[test]
fn tab_with_a_selection_indents_every_line_it_touches() {
    let mut h = Harness::new("a\nb\nc");
    click_char(&mut h, 0, 0);
    h.key_mod(Key::ArrowDown, SHIFT);
    h.key(Key::Tab);
    assert_eq!(h.text(), "    a\n    b\nc");
    h.key_mod(Key::Tab, SHIFT);
    assert_eq!(h.text(), "a\nb\nc", "and Shift+Tab takes it back off both");
}

#[test]
fn enter_with_a_selection_replaces_it_with_a_line_break() {
    let mut h = Harness::new("before MIDDLE after");
    click_char(&mut h, 0, 7);
    for _ in 0..6 {
        h.key_mod(Key::ArrowRight, SHIFT);
    }
    h.key(Key::Enter);
    assert_eq!(h.text(), "before \n after");
}

#[test]
fn dragging_from_one_line_to_another_selects_between_them() {
    let mut h = Harness::new("first line\nsecond line\nthird line");
    h.drag(h.pos_of(0, 3.0), h.pos_of(2, 4.0));
    assert_eq!(h.selected(), "st line\nsecond line\nthir");
}

#[test]
fn shift_click_extends_the_selection_from_the_caret() {
    let mut h = Harness::new("0123456789");
    click_char(&mut h, 0, 2);
    h.click_mod(h.pos_of(0, 7.0), SHIFT);
    assert_eq!(h.selected(), "23456");
}

#[test]
fn a_combining_accent_is_one_step_for_the_arrows_and_for_backspace() {
    // "e" followed by a combining acute is one letter on screen. Stepping over it
    // must not stop in the middle, where the caret has no visible place to be.
    let mut h = Harness::new("ae\u{301}b");
    click_char(&mut h, 0, 1);
    h.key(Key::ArrowRight);
    assert_eq!(h.caret(), 3, "over the letter and its accent together");
    h.key(Key::ArrowLeft);
    assert_eq!(h.caret(), 1);
    h.key(Key::ArrowRight);
    h.key(Key::Backspace);
    assert_eq!(h.text(), "ab", "and backspace takes the whole letter");
}

#[test]
fn an_emoji_with_a_variation_selector_or_joiner_is_one_step() {
    let mut h = Harness::new("a\u{2764}\u{FE0F}b\u{1F468}\u{200D}\u{1F469}c");
    click_char(&mut h, 0, 1);
    h.key(Key::ArrowRight);
    assert_eq!(h.caret(), 3, "a heart with its selector");
    h.key(Key::ArrowRight);
    h.key(Key::ArrowRight);
    assert_eq!(h.caret(), 7, "a joined pair is one glyph and one step");
}

#[test]
fn copy_puts_the_selection_on_the_clipboard_and_cut_removes_it() {
    let mut h = Harness::new("keep CUT keep");
    click_char(&mut h, 0, 5);
    for _ in 0..3 {
        h.key_mod(Key::ArrowRight, SHIFT);
    }
    h.send_copy();
    assert_eq!(h.copied().as_deref(), Some("CUT"));
    assert_eq!(h.text(), "keep CUT keep", "copy changes nothing");
    h.send_cut();
    assert_eq!(h.copied().as_deref(), Some("CUT"));
    assert_eq!(h.text(), "keep  keep");
}

#[test]
fn paste_replaces_the_selection_and_lands_with_the_caret_after_it() {
    let mut h = Harness::new("one two three");
    click_char(&mut h, 0, 4);
    for _ in 0..3 {
        h.key_mod(Key::ArrowRight, SHIFT);
    }
    h.paste("2");
    assert_eq!(h.text(), "one 2 three");
    assert_eq!(h.caret(), 5);
    h.paste("a\nb");
    assert_eq!(
        h.text(),
        "one 2a\nb three",
        "a multi-line paste keeps its lines"
    );
}

#[test]
fn the_line_move_and_duplicate_commands_keep_the_caret_on_the_same_text() {
    let mut h = Harness::new("one\ntwo\nthree");
    click_char(&mut h, 1, 1);
    h.key_mod(Key::ArrowUp, CTRL);
    assert_eq!(h.text(), "two\none\nthree");
    assert_eq!(h.line_column(), (1, 2), "the caret went up with its line");
    h.key_mod(Key::ArrowDown, CTRL);
    assert_eq!(h.text(), "one\ntwo\nthree");
    h.key_mod(Key::D, CTRL_SHIFT);
    assert_eq!(h.text(), "one\ntwo\ntwo\nthree");
}

#[test]
fn alt_and_the_arrows_move_a_line_too() {
    // Alt+Up and Alt+Down are how most editors spell it.
    let mut h = Harness::new("one\ntwo\nthree");
    click_char(&mut h, 1, 1);
    h.key_mod(Key::ArrowUp, Modifiers::ALT);
    assert_eq!(h.text(), "two\none\nthree");
    h.key_mod(Key::ArrowDown, Modifiers::ALT);
    assert_eq!(h.text(), "one\ntwo\nthree");
}

#[test]
fn commenting_several_lines_comments_all_and_uncommenting_takes_them_all_back() {
    let mut h = Harness::new("a\n\nb\n    c");
    h.key_mod(Key::A, CTRL);
    h.key_mod(Key::Slash, CTRL);
    assert_eq!(
        h.text(),
        "// a\n\n// b\n    // c",
        "blank lines are left alone, indent is kept"
    );
    h.key_mod(Key::Slash, CTRL);
    assert_eq!(h.text(), "a\n\nb\n    c");
}

#[test]
fn a_selection_dragged_past_the_bottom_of_the_pane_scrolls_the_text() {
    let doc = (0..300)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut h = Harness::new(&doc);
    let start = h.pos_of(2, 1.0);
    let below = Pos2::new(start.x + 40.0, h.rect().bottom() + 30.0);
    h.pointer(start, true);
    for _ in 0..30 {
        h.pointer_moved(below);
        h.frame();
    }
    h.pointer(below, false);
    assert!(
        h.top_line().is_some_and(|t| t > 5),
        "dragging past the edge should scroll; the window is still at line {:?}",
        h.top_line()
    );
    assert!(
        h.selected().lines().count() > 10,
        "and the selection grew with it"
    );
}

#[test]
fn clicking_the_gutter_puts_the_caret_at_the_start_of_that_line() {
    let mut h = Harness::new("first\nsecond\nthird");
    let mut at = h.pos_of(1, 0.0);
    at.x = h.rect().left() + 4.0;
    h.click(at);
    assert_eq!(h.line_column(), (2, 1));
}

#[test]
fn one_enormous_line_does_not_make_every_frame_slow() {
    // A minified file is one line of a megabyte. The editor must not shape or hash
    // all of it every frame just to draw the part that is on screen.
    let doc = "abcdefghij ".repeat(100_000);
    let mut h = Harness::new(&doc);
    for _ in 0..3 {
        h.frame();
    }
    let got = timed(&mut h, 20, 3, |h, _| h.frame());
    // Known limit: a line is shaped whole, so a megabyte on one line costs a frame of
    // tens of milliseconds to tessellate. Ordinary long lines are far below this; see
    // the hundred-kilobyte test.
    within("a frame over a one-line megabyte", got, 120.0, 200.0);
}

#[test]
fn a_frame_over_a_hundred_kilobyte_line_is_still_instant() {
    // Minified scripts and data files are this size, which is where a long line
    // actually turns up. It must not cost a frame of its own.
    let doc = "abcdefghij ".repeat(9_000);
    let mut h = Harness::new(&doc);
    for _ in 0..3 {
        h.frame();
    }
    let got = timed(&mut h, 30, 3, |h, _| h.frame());
    within("a frame over a 100 KB line", got, 10.0, 30.0);
    let typing = timed(&mut h, 20, 3, |h, _| h.type_text("x"));
    within("typing into a 100 KB line", typing, 25.0, 60.0);
}

#[test]
fn typing_into_a_one_line_megabyte_stays_responsive() {
    let doc = "abcdefghij ".repeat(100_000);
    let mut h = Harness::new(&doc);
    h.frame();
    let start = std::time::Instant::now();
    h.type_like_a_person("hello");
    let per_key = start.elapsed().as_secs_f64() * 1000.0 / 10.0;
    // Shaping a million-character line is the cost, and it is the same in any editor
    // that lays a line out whole. This only guards against it getting far worse.
    assert!(
        per_key < 400.0,
        "a keystroke in a one-line megabyte took {per_key:.1} ms"
    );
}

// ---- wrapped text ---------------------------------------------------------------
//
// With wrapping on a line is several rows tall and lines are different heights, which
// is where scrolling, the caret and Home/End all stop being simple. These use a
// document of tall and short lines so every one of them has to deal with both.

/// Sixty lines, every third of them long enough to wrap onto several rows.
fn wrapped_doc() -> String {
    (0..60)
        .map(|i| format!("L{i}: {}", "word ".repeat(if i % 3 == 0 { 70 } else { 6 })))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A harness over `wrapped_doc` with wrapping on.
fn wrapped() -> Harness {
    let mut h = Harness::new(&wrapped_doc());
    h.set_wrap(true);
    h.frame();
    h
}

#[test]
fn a_steady_wheel_moves_wrapped_text_at_a_steady_speed() {
    // The complaint this pins down: scrolling was smooth until a line wrapped, and
    // then the text lurched by the whole height of the wrapped line at once. Follow
    // one line's position on screen frame by frame; no step may be more than a few
    // times the typical one.
    let mut h = wrapped();
    let y_of = |h: &Harness, n: usize| {
        let tops = h.row_tops();
        h.rows()
            .iter()
            .position(|r| r.line == n)
            .and_then(|i| tops.get(i).copied())
    };
    let mut last = y_of(&h, 6).expect("line 6 is on screen");
    let mut steps = Vec::new();
    for _ in 0..160 {
        h.wheel_move(-0.05);
        match y_of(&h, 6) {
            Some(y) => {
                steps.push(last - y);
                last = y;
            }
            None => break,
        }
    }
    let biggest = steps.iter().cloned().fold(0.0f32, f32::max);
    assert!(
        steps.len() > 40 && biggest <= 6.0,
        "the text moved {biggest:.1} points in one frame (steps: {steps:?})"
    );
}

#[test]
fn scrolling_all_the_way_down_a_wrapped_document_ends_with_its_last_line_at_the_bottom() {
    let mut h = wrapped();
    for _ in 0..60 {
        h.scroll(30.0);
    }
    h.frame();
    let shown = h.drawn_line_numbers();
    assert_eq!(*shown.last().unwrap(), 59, "the last line is on screen");
    let tops = h.row_tops();
    let bottom = tops.last().unwrap() + h.row_height();
    assert!(
        bottom <= h.rect().bottom() + 1.0 && bottom >= h.rect().bottom() - 2.0 * h.row_height(),
        "the end of the document should sit at the bottom of the pane, not {:.0} from it",
        h.rect().bottom() - bottom
    );
}

#[test]
fn scrolling_back_up_a_wrapped_document_returns_to_the_very_top() {
    let mut h = wrapped();
    for _ in 0..20 {
        h.scroll(15.0);
    }
    for _ in 0..40 {
        h.scroll(-15.0);
    }
    h.frame();
    assert_eq!(h.top_line(), Some(0));
    assert_eq!(h.scroll_y(), 0.0, "and not a point past it");
}

#[test]
fn the_caret_stays_on_screen_walking_down_through_tall_wrapped_lines() {
    let mut h = wrapped();
    click_char(&mut h, 0, 2);
    for step in 0..400 {
        h.key(Key::ArrowDown);
        let caret = h.caret_rect();
        assert!(
            caret.is_some_and(
                |r| r.top() >= h.rect().top() - 1.0 && r.bottom() <= h.rect().bottom() + 1.0
            ),
            "after {step} presses the caret at line {} is not inside the pane: {caret:?}",
            h.line_column().0
        );
    }
}

#[test]
fn typing_at_the_end_of_a_tall_wrapped_line_keeps_the_caret_in_view() {
    let mut h = wrapped();
    h.key_mod(Key::End, CTRL);
    h.type_like_a_person(&"more words ".repeat(40));
    assert!(
        h.caret_rect()
            .is_some_and(|r| r.bottom() <= h.rect().bottom() + 1.0),
        "the caret ran off the bottom while typing"
    );
}

#[test]
fn after_scrolling_wrapped_text_a_click_lands_on_the_line_under_the_pointer() {
    let mut h = wrapped();
    for notches in [1.0, 4.5, 9.0, 20.0] {
        h.scroll(notches);
        h.frame();
        let shown = h.drawn_line_numbers();
        // Only rows that are wholly inside the pane: with the window scrolled into the
        // middle of a tall line, the first rows are above it and cannot be clicked.
        let tops = h.row_tops();
        let room = h.rect().bottom() - h.row_height();
        let visible: Vec<usize> = (0..h.rows().len())
            .filter(|&r| tops[r] >= 0.0 && tops[r] <= room)
            .collect();
        for row in [visible[1], visible[4], visible[8]] {
            let want = h.rows()[row].line;
            h.click(h.pos_of(row, 1.0));
            h.frame();
            assert_eq!(
                h.line_column().0 - 1,
                want,
                "after scrolling {notches}, a click on row {row} of {shown:?} went to another line"
            );
        }
    }
}

#[test]
fn home_and_end_in_wrapped_text_go_to_the_edges_of_the_row_they_are_on() {
    let mut h = wrapped();
    // Line 0 is long and wraps. Put the caret in the middle of its second row.
    let second_row = h.rows()[1];
    let inside = second_row.chars.0 + 6;
    let mut at = h.pos_of(1, 6.0);
    at.x += 1.0;
    h.click(at);
    assert_eq!(h.caret(), inside);
    h.key(Key::Home);
    assert_eq!(
        h.caret(),
        second_row.chars.0,
        "Home: the start of this row, not of the line"
    );
    h.key(Key::End);
    let end = h.caret();
    assert!(
        end + 1 >= second_row.chars.1 && end <= second_row.chars.1,
        "End: the end of this row ({}), not of the line; got {end}",
        second_row.chars.1
    );
}

#[test]
fn a_word_longer_than_the_pane_wraps_instead_of_running_off_the_edge() {
    let mut h = Harness::new(&"x".repeat(600));
    h.set_wrap(true);
    h.frame();
    assert!(
        h.rows().len() > 1,
        "a 600 character word should take several rows"
    );
    let (_, right) = h.glyph_extent().expect("text drawn");
    let edge = h.bar().map_or(h.rect().right(), |b| b.left());
    assert!(
        right <= edge + 1.0,
        "the text runs to {right:.0}, past the edge at {edge:.0}"
    );
}

// ---- starting a selection with nothing under the caret ----------------------------
//
// A document opened from the file list has never had the keyboard, and the first
// thing a person often does with it is drag across some text. Nothing has been
// clicked, so there is no caret placed by the reader and no focus to lean on: the
// drag itself has to do the whole job.

#[test]
fn dragging_across_text_in_a_document_that_was_never_clicked_selects_it() {
    let mut h = Harness::never_focused("alpha beta gamma delta");
    assert!(!h.has_focus(), "the test starts without the keyboard");
    h.drag_like_a_person(h.pos_of(0, 6.0), h.pos_of(0, 10.0));
    assert_eq!(h.selected(), "beta", "exactly what was dragged over");
    assert!(h.has_focus(), "and the drag took the keyboard");
}

#[test]
fn a_drag_does_not_anchor_at_the_old_caret_when_the_document_was_not_focused() {
    // The caret was left at the very end by something earlier. A drag in the middle
    // must select from where the button went down, not from that stale caret.
    let mut h = Harness::new("one two three four five");
    h.key_mod(Key::End, CTRL);
    h.blur();
    h.drag_like_a_person(h.pos_of(0, 4.0), h.pos_of(0, 7.0));
    assert_eq!(h.selected(), "two");
}

#[test]
fn dragging_backwards_in_an_unfocused_document_selects_the_same_text() {
    let mut h = Harness::never_focused("alpha beta gamma");
    h.drag_like_a_person(h.pos_of(0, 10.0), h.pos_of(0, 6.0));
    assert_eq!(h.selected(), "beta");
}

#[test]
fn typing_right_after_an_unfocused_drag_replaces_what_was_selected() {
    let mut h = Harness::never_focused("alpha beta gamma");
    h.drag_like_a_person(h.pos_of(0, 6.0), h.pos_of(0, 10.0));
    h.type_like_a_person("X");
    assert_eq!(h.text(), "alpha X gamma");
}

#[test]
fn dragging_down_across_lines_in_an_unfocused_document_selects_between_them() {
    let mut h = Harness::never_focused("first line\nsecond line\nthird line");
    h.drag_like_a_person(h.pos_of(0, 6.0), h.pos_of(2, 5.0));
    assert_eq!(h.selected(), "line\nsecond line\nthird");
}

#[test]
fn a_plain_click_in_an_unfocused_document_takes_focus_and_places_the_caret() {
    let mut h = Harness::never_focused("alpha beta");
    h.click_like_a_person(h.pos_of(0, 6.0));
    assert!(h.has_focus());
    assert_eq!(h.caret(), 6);
    h.type_like_a_person("X");
    assert_eq!(h.text(), "alpha Xbeta");
}

#[test]
fn a_drag_that_starts_in_the_gutter_selects_from_the_start_of_that_line() {
    let mut h = Harness::never_focused("first\nsecond\nthird");
    let mut from = h.pos_of(1, 0.0);
    from.x = h.rect().left() + 3.0;
    h.drag_like_a_person(from, h.pos_of(1, 3.0));
    assert_eq!(h.selected(), "sec");
}

#[test]
fn dragging_in_an_empty_document_does_nothing_and_does_not_panic() {
    let mut h = Harness::never_focused("");
    h.drag_like_a_person(Pos2::new(60.0, 10.0), Pos2::new(200.0, 80.0));
    assert_eq!(h.selected(), "");
    assert_eq!(h.caret(), 0);
}

#[test]
fn a_drag_that_leaves_the_pane_sideways_keeps_selecting_to_the_edge() {
    let mut h = Harness::never_focused("alpha beta gamma delta");
    let outside = Pos2::new(h.rect().right() + 200.0, h.pos_of(0, 0.0).y);
    h.drag_like_a_person(h.pos_of(0, 6.0), outside);
    assert_eq!(
        h.selected(),
        "beta gamma delta",
        "out past the end of the line means the end"
    );
}

#[test]
fn shift_arrows_extend_a_selection_that_began_as_a_drag() {
    let mut h = Harness::never_focused("alpha beta gamma");
    h.drag_like_a_person(h.pos_of(0, 6.0), h.pos_of(0, 10.0));
    h.key_mod(Key::ArrowRight, SHIFT);
    assert_eq!(h.selected(), "beta ");
}

// ---- selecting in wrapped text ----------------------------------------------------

#[test]
fn shift_end_in_a_wrapped_row_selects_to_the_end_of_that_row_only() {
    let mut h = wrapped();
    let row = h.rows()[0];
    click_char(&mut h, 0, 4);
    h.key_mod(Key::End, SHIFT);
    let (lo, hi) = h.selection();
    assert_eq!(lo, 4);
    assert!(
        hi + 1 >= row.chars.1 && hi <= row.chars.1,
        "selected to {hi}, the row ends at {}",
        row.chars.1
    );
    assert!(!h.selected().contains('\n'));
}

#[test]
fn shift_home_in_a_continuation_row_selects_back_to_the_start_of_that_row() {
    let mut h = wrapped();
    let second = h.rows()[1];
    let mut at = h.pos_of(1, 8.0);
    at.x += 1.0;
    h.click(at);
    h.key_mod(Key::Home, SHIFT);
    assert_eq!(
        h.selection().0,
        second.chars.0,
        "back to the start of row two, not of the line"
    );
}

#[test]
fn shift_down_in_wrapped_text_selects_one_row_worth_and_lands_under_the_same_column() {
    let mut h = wrapped();
    let (first, second) = (h.rows()[0], h.rows()[1]);
    click_char(&mut h, 0, 5);
    h.key_mod(Key::ArrowDown, SHIFT);
    let (lo, hi) = h.selection();
    assert_eq!(lo, 5);
    assert_eq!(hi, second.chars.0 + 5, "the same column one row down");
    assert_eq!(h.selected().chars().count(), second.chars.0 + 5 - 5);
    let _ = first;
}

#[test]
fn dragging_down_across_wrapped_rows_selects_exactly_the_text_between() {
    let mut h = wrapped();
    let (r1, r3) = (h.rows()[1], h.rows()[3]);
    h.drag_like_a_person(h.pos_of(1, 3.0), h.pos_of(3, 9.0));
    let (lo, hi) = h.selection();
    assert_eq!(lo, r1.chars.0 + 3);
    assert_eq!(hi, r3.chars.0 + 9);
    assert!(
        !h.selected().contains('\n'),
        "all of it is inside one wrapped line"
    );
}

#[test]
fn the_wash_over_wrapped_rows_is_one_band_per_row_and_none_beyond() {
    let mut h = wrapped();
    h.drag_like_a_person(h.pos_of(1, 3.0), h.pos_of(4, 9.0));
    let bands = h.wash_rows();
    assert_eq!(bands.len(), 4, "rows two to five: {bands:?}");
    // The first band starts partway along and the last ends partway along; the ones
    // between reach from the left edge of the text to the right edge of the row.
    assert!(bands[0].left() > h.text_left() + 1.0);
    assert!((bands[1].left() - h.text_left()).abs() < 1.5);
    assert!((bands[2].left() - h.text_left()).abs() < 1.5);
    assert!(bands[3].right() < bands[2].right());
    for pair in bands.windows(2) {
        assert!(
            (pair[1].top() - pair[0].bottom()).abs() < 1.0,
            "bands touch, with no gap or overlap between rows"
        );
    }
}

#[test]
fn triple_click_in_a_wrapped_line_selects_all_its_rows() {
    let mut h = wrapped();
    h.triple_click(h.pos_of(2, 4.0));
    let text = h.selected();
    assert!(
        text.starts_with("L0: "),
        "the whole line, from its start: {:?}",
        &text[..20]
    );
    assert!(text.chars().count() > 300, "all of its rows, not one");
    assert!(text.ends_with('\n'));
}

#[test]
fn double_click_in_a_wrapped_row_selects_the_word_under_the_pointer() {
    let mut h = wrapped();
    h.double_click(h.pos_of(2, 7.5));
    assert_eq!(h.selected(), "word");
}

#[test]
fn end_then_typing_puts_the_text_at_the_end_of_the_row_not_the_start_of_the_next() {
    let mut h = wrapped();
    click_char(&mut h, 0, 4);
    h.key(Key::End);
    let caret = h.caret();
    h.type_like_a_person("X");
    assert_eq!(h.text().chars().nth(caret), Some('X'));
    assert_eq!(h.caret(), caret + 1);
}

#[test]
fn right_from_the_end_of_a_wrapped_row_goes_to_the_start_of_the_next() {
    let mut h = wrapped();
    let second = h.rows()[1];
    click_char(&mut h, 0, 4);
    h.key(Key::End);
    h.key(Key::ArrowRight);
    assert_eq!(h.caret(), second.chars.0);
}

#[test]
fn control_shift_end_in_wrapped_text_selects_everything_below() {
    let mut h = wrapped();
    click_char(&mut h, 0, 4);
    h.key_mod(Key::End, Modifiers::CTRL | Modifiers::SHIFT);
    let all = h.text();
    let expected: String = all.chars().skip(4).collect();
    assert_eq!(h.selected(), expected);
}

// ---- the Ctrl key ---------------------------------------------------------------

#[test]
fn control_left_at_the_start_of_a_line_goes_to_the_end_of_the_line_above() {
    let mut h = Harness::new("first line\nsecond");
    click_char(&mut h, 1, 0);
    h.key_mod(Key::ArrowLeft, CTRL);
    assert_eq!(h.caret(), 6, "the start of the last word above");
    let mut h = Harness::new("first line\n\nsecond");
    click_char(&mut h, 2, 0);
    h.key_mod(Key::ArrowLeft, CTRL);
    assert!(h.caret() < 11, "over the blank line and into the one above");
}

#[test]
fn control_right_at_the_end_of_the_document_does_nothing() {
    let mut h = Harness::new("one two");
    h.key_mod(Key::End, CTRL);
    h.key_mod(Key::ArrowRight, CTRL);
    assert_eq!(h.caret(), 7);
    h.key_mod(Key::Delete, CTRL);
    assert_eq!(h.text(), "one two");
}

#[test]
fn control_backspace_at_the_start_of_a_line_removes_the_line_break() {
    let mut h = Harness::new("one\ntwo");
    click_char(&mut h, 1, 0);
    h.key_mod(Key::Backspace, CTRL);
    assert!(
        h.text() == "onetwo" || h.text() == "two",
        "got {:?}",
        h.text()
    );
    assert!(!h.text().contains('\n'));
}

#[test]
fn control_shift_left_walks_back_over_words_and_punctuation() {
    let mut h = Harness::new("let x = foo.bar(baz);");
    h.key_mod(Key::End, CTRL);
    let mut selections = Vec::new();
    for _ in 0..4 {
        h.key_mod(Key::ArrowLeft, Modifiers::CTRL | Modifiers::SHIFT);
        selections.push(h.selected());
    }
    assert!(
        selections.windows(2).all(|w| w[1].len() > w[0].len()),
        "each step grows the selection: {selections:?}"
    );
}

#[test]
fn copy_and_cut_with_nothing_selected_take_the_whole_line() {
    let mut h = Harness::new("one\ntwo\nthree");
    click_char(&mut h, 1, 1);
    h.send_copy();
    assert_eq!(h.copied().as_deref(), Some("two\n"));
    assert_eq!(h.text(), "one\ntwo\nthree");
    h.send_cut();
    assert_eq!(h.copied().as_deref(), Some("two\n"));
    assert_eq!(h.text(), "one\nthree");
}

#[test]
fn control_a_then_copy_then_paste_over_everything_round_trips() {
    let mut h = Harness::new("keep me\nand me");
    h.key_mod(Key::A, CTRL);
    h.send_copy();
    let copied = h.copied().unwrap();
    h.type_like_a_person("x");
    assert_eq!(h.text(), "x");
    h.key_mod(Key::A, CTRL);
    h.paste(&copied);
    assert_eq!(h.text(), "keep me\nand me");
}

#[test]
fn undo_and_redo_walk_through_paste_delete_and_typing_in_order() {
    let mut h = Harness::new("base");
    h.key_mod(Key::End, CTRL);
    h.paste(" pasted");
    h.pause();
    h.key(Key::Backspace);
    h.pause();
    h.type_like_a_person("!");
    assert_eq!(h.text(), "base paste!");
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), "base paste");
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), "base pasted");
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), "base");
    h.key_mod(Key::Y, CTRL);
    assert_eq!(h.text(), "base pasted");
    h.key_mod(Key::Z, Modifiers::CTRL | Modifiers::SHIFT);
    assert_eq!(h.text(), "base paste", "Ctrl+Shift+Z is redo as well");
}

#[test]
fn control_enter_opens_a_line_below_indented_like_this_one() {
    let mut h = Harness::new("one\n    two\nthree");
    click_char(&mut h, 1, 5);
    h.key_mod(Key::Enter, CTRL);
    assert_eq!(h.text(), "one\n    two\n    \nthree");
    assert_eq!(h.line_column(), (3, 5), "with the caret on it");
}

// ---- speed --------------------------------------------------------------------
//
// Editing has to feel instantaneous: a keystroke that takes more than a frame is a
// keystroke a person can feel. These fix budgets for the things done all day, on
// documents bigger than people usually have, so that a change which makes any of them
// slower fails here instead of being noticed by someone typing.
//
// Each is measured over many repetitions and reported as the median, which is what
// typing feels like, and the worst case of the best of several trials, which is what
// a stutter feels like - taking the best trial so that another test using the CPU at
// the same moment cannot fail this one. In a debug build everything is far slower and
// the budgets are scaled to match; the real numbers are the release ones.

/// Runs a speed test up to three times and passes if any run does.
///
/// The whole suite runs at once, and another test using the CPU at the moment a
/// millisecond is being measured makes that millisecond twenty. A real slowdown is
/// slow on every attempt; a busy machine is not.
fn retrying(mut body: impl FnMut()) {
    for attempt in 0..3 {
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut body));
        match run {
            Ok(()) => return,
            Err(e) if attempt == 2 => std::panic::resume_unwind(e),
            Err(_) => {}
        }
    }
}

/// The time `f` takes per call in milliseconds: `(median, worst)`, over `reps` calls,
/// the pair from the best of `trials` runs.
fn timed(
    h: &mut Harness,
    reps: usize,
    trials: usize,
    mut f: impl FnMut(&mut Harness, usize),
) -> (f64, f64) {
    // With the rest of a real frame included, not only the editor's own drawing.
    h.set_tessellate(true);
    let mut best: Option<(f64, f64)> = None;
    for _ in 0..trials {
        let mut ms = Vec::with_capacity(reps);
        for i in 0..reps {
            let t = std::time::Instant::now();
            f(h, i);
            ms.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        ms.sort_by(|a, b| a.total_cmp(b));
        let pair = (ms[ms.len() / 2], ms[ms.len() - 1]);
        best = Some(match best {
            None => pair,
            Some(b) => (b.0.min(pair.0), b.1.min(pair.1)),
        });
    }
    best.unwrap()
}

/// A budget in milliseconds, relaxed for a debug build.
fn ms(release: f64) -> f64 {
    if cfg!(debug_assertions) {
        release * 40.0
    } else {
        release
    }
}

/// Five thousand lines of plausible code, with highlighting on.
fn big_code(lines: usize) -> Harness {
    let line = "    let value = compute(alpha, beta) + gamma; // a plausible line of code\n";
    let mut h = Harness::new(&line.repeat(lines));
    h.set_lang(BLOCKY);
    h.frame();
    h
}

#[track_caller]
fn within(what: &str, got: (f64, f64), median: f64, worst: f64) {
    // Printed as well as checked, so `--nocapture` shows how much room each budget has.
    eprintln!(
        "  {what:52} median {:6.3} ms  worst {:6.3} ms   (budget {median} / {worst})",
        got.0, got.1
    );
    // The median is what typing feels like, and it holds steady however busy the
    // machine is, so it is always held to its budget. The worst case is a stall, and
    // on a machine running the whole test suite at once a stall is as likely to be
    // another test as this one: so unless `RHUMB_STRICT_SPEED` is set, for a run on a
    // quiet machine, the worst case only has to be short of something a person would
    // call a freeze.
    let strict = std::env::var_os("RHUMB_STRICT_SPEED").is_some();
    // At 240 frames a second a whole frame is 4.2 ms, editor and everything else in it,
    // so in the strict run no single step of editing gets more than 4 ms. Operations
    // that work over a whole large document - pasting a hundred kilobytes, deleting
    // millions of characters - are one-off jobs and keep the budgets they were given.
    let worst_budget = if strict {
        ms(if worst <= 16.0 { worst.min(4.0) } else { worst })
    } else {
        ms(worst).max(ms(worst * 8.0)).max(60.0)
    };
    assert!(
        got.0 <= ms(median) && got.1 <= worst_budget,
        "{what}: median {:.2} ms (budget {:.1}), worst {:.2} ms (budget {:.1})",
        got.0,
        ms(median),
        got.1,
        worst_budget
    );
}

#[test]
fn typing_a_character_is_instant_with_and_without_wrapping() {
    retrying(|| {
        for wrap in [false, true] {
            let mut h = big_code(5_000);
            h.set_wrap(wrap);
            click_char(&mut h, 12, 20);
            let got = timed(&mut h, 200, 3, |h, _| h.type_text("x"));
            within(&format!("typing a character (wrap {wrap})"), got, 1.0, 5.0);
        }
    });
}

#[test]
fn enter_backspace_and_delete_are_instant_in_the_middle_of_a_big_file() {
    retrying(|| {
        let mut h = big_code(20_000);
        click_char(&mut h, 15, 20);
        let enter = timed(&mut h, 100, 3, |h, _| h.key(Key::Enter));
        within("Enter", enter, 2.0, 8.0);
        let back = timed(&mut h, 100, 3, |h, _| h.key(Key::Backspace));
        within("Backspace", back, 2.0, 8.0);
        let del = timed(&mut h, 100, 3, |h, _| h.key(Key::Delete));
        within("Delete", del, 2.0, 8.0);
        let word = timed(&mut h, 60, 3, |h, _| h.key_mod(Key::Backspace, CTRL));
        within("Ctrl+Backspace", word, 2.0, 8.0);
    });
}

#[test]
fn moving_the_caret_is_instant_whatever_the_size_of_the_file() {
    retrying(|| {
        let mut h = big_code(100_000);
        click_char(&mut h, 10, 10);
        for key in [
            Key::ArrowLeft,
            Key::ArrowRight,
            Key::ArrowDown,
            Key::ArrowUp,
            Key::Home,
            Key::End,
        ] {
            let got = timed(&mut h, 100, 3, |h, _| h.key(key));
            within(&format!("{key:?} in 100,000 lines"), got, 1.5, 6.0);
        }
        let page = timed(&mut h, 60, 3, |h, i| {
            h.key(if i % 2 == 0 {
                Key::PageDown
            } else {
                Key::PageUp
            })
        });
        within("Page Down / Up", page, 2.0, 8.0);
        let ends = timed(&mut h, 40, 3, |h, i| {
            h.key_mod(if i % 2 == 0 { Key::End } else { Key::Home }, CTRL)
        });
        within("Ctrl+End / Ctrl+Home", ends, 2.0, 8.0);
    });
}

#[test]
fn a_held_arrow_key_makes_every_repeat_instant() {
    retrying(|| {
        let mut h = big_code(50_000);
        click_char(&mut h, 3, 10);
        let got = timed(&mut h, 300, 3, |h, _| h.hold(Key::ArrowDown, 0));
        within("a held Down arrow", got, 2.0, 16.0);
    });
}

#[test]
fn undo_and_redo_are_instant_even_after_a_long_editing_session() {
    retrying(|| {
        let mut h = big_code(10_000);
        click_char(&mut h, 5, 5);
        for i in 0..300 {
            h.pause();
            h.type_text(if i % 3 == 0 { "\n" } else { "ab" });
        }
        let undo = timed(&mut h, 250, 1, |h, _| h.key_mod(Key::Z, CTRL));
        within("Ctrl+Z", undo, 2.0, 16.0);
        let redo = timed(&mut h, 200, 1, |h, _| h.key_mod(Key::Y, CTRL));
        within("Ctrl+Y", redo, 2.0, 16.0);
    });
}

#[test]
fn pasting_and_undoing_a_paste_of_a_hundred_kilobytes_is_instant() {
    retrying(|| {
        let mut h = big_code(5_000);
        click_char(&mut h, 8, 0);
        let chunk = "some pasted text that goes on a bit\n".repeat(3_000);
        let paste = timed(&mut h, 1, 1, |h, _| h.paste(&chunk));
        within("pasting 100 KB", paste, 25.0, 25.0);
        let undo = timed(&mut h, 1, 1, |h, _| h.key_mod(Key::Z, CTRL));
        within("undoing it", undo, 25.0, 25.0);
    });
}

#[test]
fn select_all_and_delete_on_a_big_document_and_undoing_it_are_quick() {
    retrying(|| {
        let mut h = big_code(50_000);
        let sel = timed(&mut h, 1, 1, |h, _| h.key_mod(Key::A, CTRL));
        within("select all", sel, 8.0, 8.0);
        let del = timed(&mut h, 1, 1, |h, _| h.key(Key::Delete));
        within("deleting three million characters", del, 40.0, 40.0);
        let undo = timed(&mut h, 1, 1, |h, _| h.key_mod(Key::Z, CTRL));
        within("undoing it", undo, 40.0, 40.0);
    });
}

#[test]
fn line_commands_over_a_thousand_lines_are_quick() {
    retrying(|| {
        let mut h = big_code(5_000);
        click_char(&mut h, 0, 0);
        for _ in 0..1000 {
            h.key_mod(Key::ArrowDown, SHIFT);
        }
        let comment = timed(&mut h, 1, 1, |h, _| h.key_mod(Key::Slash, CTRL));
        within("commenting 1000 lines", comment, 40.0, 40.0);
        let indent = timed(&mut h, 1, 1, |h, _| h.key(Key::Tab));
        within("indenting 1000 lines", indent, 40.0, 40.0);
        let outdent = timed(&mut h, 1, 1, |h, _| h.key_mod(Key::Tab, SHIFT));
        within("outdenting 1000 lines", outdent, 40.0, 40.0);
    });
}

#[test]
fn moving_and_duplicating_a_line_deep_in_a_big_file_is_instant() {
    retrying(|| {
        let mut h = big_code(100_000);
        h.key_mod(Key::End, CTRL);
        click_char(&mut h, 5, 3);
        let moved = timed(&mut h, 60, 3, |h, i| {
            h.key_mod(
                if i % 2 == 0 {
                    Key::ArrowUp
                } else {
                    Key::ArrowDown
                },
                CTRL,
            )
        });
        within("moving a line", moved, 2.0, 8.0);
        let dup = timed(&mut h, 60, 3, |h, _| h.key_mod(Key::D, CTRL_SHIFT));
        within("duplicating a line", dup, 2.0, 8.0);
    });
}

#[test]
fn scrolling_frames_are_instant_with_and_without_wrapping() {
    retrying(|| {
        for wrap in [false, true] {
            let mut h = big_code(20_000);
            h.set_wrap(wrap);
            let got = timed(&mut h, 120, 3, |h, _| h.wheel_move(-0.6));
            within(
                &format!("a frame of scrolling (wrap {wrap})"),
                got,
                1.5,
                14.0,
            );
        }
    });
}

#[test]
fn the_first_frame_of_a_five_megabyte_document_is_quick() {
    retrying(|| {
        let line = "    let value = compute(alpha, beta) + gamma; // a plausible line of code\n";
        let doc = line.repeat(5_000_000 / line.len());
        let t = std::time::Instant::now();
        let mut h = Harness::new(&doc);
        h.set_lang(BLOCKY);
        h.frame();
        let took = t.elapsed().as_secs_f64() * 1000.0;
        assert!(
            took < ms(120.0),
            "opening a 5 MB document and drawing it took {took:.0} ms"
        );
    });
}

#[test]
fn searching_a_two_megabyte_document_as_you_type_stays_under_a_frame_or_two() {
    retrying(|| {
        let line = "    let value = compute(alpha, beta) + gamma; // a plausible line of code\n";
        let mut h = Harness::new(&line.repeat(2_000_000 / line.len()));
        h.key_mod(Key::F, CTRL);
        let got = timed(&mut h, 8, 1, |h, i| h.type_text(&"compute("[i..i + 1]));
        within("a keystroke in the find box over 2 MB", got, 40.0, 60.0);
    });
}

#[test]
fn replace_all_over_thousands_of_matches_is_quick() {
    retrying(|| {
        let mut h = Harness::new(&"foo bar foo baz\n".repeat(5_000));
        h.find("foo");
        h.replace_with("quux");
        let got = timed(&mut h, 1, 1, |h, _| h.press_replace(true));
        within("replacing 10,000 matches", got, 150.0, 150.0);
        assert!(h.text().starts_with("quux bar quux baz"));
    });
}

#[test]
fn dragging_a_selection_across_a_thousand_lines_is_instant_per_frame() {
    retrying(|| {
        let mut h = big_code(10_000);
        let start = h.pos_of(2, 3.0);
        h.pointer(start, true);
        let got = timed(&mut h, 40, 3, |h, i| {
            h.pointer_moved(Pos2::new(
                start.x + 10.0,
                h.rect().bottom() + 20.0 + i as f32,
            ));
            h.frame();
        });
        within("a frame of dragging past the bottom", got, 2.0, 8.0);
        h.pointer(start, false);
    });
}

// ---- the scrollbar that grows to meet the pointer -------------------------------

/// A document tall enough to have a scrollbar.
fn scrollable() -> Harness {
    let doc = (0..300)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut h = Harness::new(&doc);
    for _ in 0..3 {
        h.frame();
    }
    h
}

/// Frames enough for the growth to finish, a little over the animation.
fn settle_bar(h: &mut Harness) {
    for _ in 0..24 {
        h.frame();
    }
}

#[test]
fn the_scrollbar_is_thin_at_rest_and_thick_when_the_pointer_comes_near() {
    let mut h = scrollable();
    let bar = h.bar().expect("a document this tall has a scrollbar");
    h.pointer_moved(Pos2::new(h.rect().center().x - 100.0, h.rect().center().y));
    settle_bar(&mut h);
    let resting = h.drawn_thumb().expect("the thumb is drawn").width();
    assert!(resting <= 5.0, "at rest the thumb is thin: {resting}");
    // Within reach of the bar, still over the text, nowhere near the thumb vertically.
    h.pointer_moved(Pos2::new(bar.left() - 30.0, h.rect().bottom() - 20.0));
    settle_bar(&mut h);
    let near = h.drawn_thumb().unwrap().width();
    assert!(near >= 12.0, "near the pointer it has grown: {near}");
    h.pointer_moved(Pos2::new(h.rect().center().x - 100.0, h.rect().center().y));
    settle_bar(&mut h);
    assert!(
        h.drawn_thumb().unwrap().width() <= 5.0,
        "and it goes back to thin when the pointer leaves"
    );
}

#[test]
fn the_scrollbar_does_not_grow_for_a_pointer_that_is_nowhere_near_it() {
    let mut h = scrollable();
    let bar = h.bar().unwrap();
    h.pointer_moved(Pos2::new(bar.left() - 200.0, h.rect().center().y));
    settle_bar(&mut h);
    assert!(h.drawn_thumb().unwrap().width() <= 5.0);
}

#[test]
fn the_scrollbar_grows_smoothly_and_not_in_a_single_jump() {
    let mut h = scrollable();
    let bar = h.bar().unwrap();
    h.pointer_moved(Pos2::new(h.rect().center().x - 100.0, h.rect().center().y));
    settle_bar(&mut h);
    h.pointer_moved(Pos2::new(bar.left() - 20.0, h.rect().center().y));
    let mut widths = Vec::new();
    for _ in 0..14 {
        h.frame();
        widths.push(h.drawn_thumb().unwrap().width());
    }
    assert!(
        widths.windows(2).all(|w| w[1] >= w[0] - 0.01),
        "growing, never shrinking on the way: {widths:?}"
    );
    assert!(
        widths.iter().any(|w| *w > 5.0 && *w < 12.0),
        "with steps in between: {widths:?}"
    );
}

#[test]
fn the_grown_scrollbar_can_be_grabbed_from_over_the_edge_of_the_text() {
    let mut h = scrollable();
    let bar = h.bar().unwrap();
    // Approach as a hand does, then press a little to the left of the strip itself.
    h.pointer_moved(Pos2::new(bar.left() - 30.0, h.rect().center().y));
    settle_bar(&mut h);
    let thumb = h.drawn_thumb().unwrap();
    let grab = Pos2::new(bar.left() - 6.0, thumb.center().y);
    h.pointer_moved(grab);
    h.frame();
    let before = h.scroll_y();
    h.drag_like_a_person(grab, Pos2::new(grab.x, grab.y + 200.0));
    assert!(
        h.scroll_y() > before + 100.0,
        "dragging it down moved the document from {before} to {}",
        h.scroll_y()
    );
}

#[test]
fn a_press_next_to_a_thin_scrollbar_is_still_a_click_in_the_text() {
    // Nothing has approached the bar, so it is thin and its reach is short: a click
    // there is a click on the text, which is where a reader who has not looked at the
    // bar means it to go.
    let mut h = Harness::new(
        &(0..300)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    h.frame();
    let bar = h.bar().unwrap();
    let before = h.scroll_y();
    let at = Pos2::new(bar.left() - 6.0, h.pos_of(3, 0.0).y);
    h.click(at);
    assert_eq!(h.scroll_y(), before, "the document did not page");
    assert_eq!(
        h.line_column().0,
        4,
        "the caret went to the line that was clicked"
    );
}

#[test]
fn a_thumb_being_dragged_stays_thick_however_far_the_pointer_wanders() {
    let mut h = scrollable();
    let bar = h.bar().unwrap();
    h.pointer_moved(Pos2::new(bar.left() - 5.0, h.rect().center().y));
    settle_bar(&mut h);
    let thumb = h.drawn_thumb().unwrap();
    let grab = Pos2::new(bar.right() - 3.0, thumb.center().y);
    h.pointer_moved(grab);
    h.frame();
    h.pointer(grab, true);
    for step in 1..=20 {
        // Out into the middle of the text, far from the bar, while the button is down.
        h.pointer_moved(Pos2::new(
            grab.x - 10.0 * step as f32,
            grab.y + 5.0 * step as f32,
        ));
        h.frame();
    }
    settle_bar(&mut h);
    assert!(
        h.drawn_thumb().unwrap().width() >= 12.0,
        "the thumb shrank while it was being dragged"
    );
    h.pointer(Pos2::new(grab.x - 200.0, grab.y + 100.0), false);
}

#[test]
fn a_document_that_fits_has_no_scrollbar_to_grow() {
    let mut h = Harness::new("short\ndocument");
    h.pointer_moved(Pos2::new(h.rect().right() - 5.0, 100.0));
    settle_bar(&mut h);
    assert!(h.bar().is_none());
    assert!(h.drawn_thumb().is_none());
}

// ---- very long lines ------------------------------------------------------------
//
// A line far longer than the pane is shaped only where it is on screen, so that a
// minified file costs what a screenful costs. These check that this is invisible:
// everything a person does on such a line lands on the character they meant.

/// About 130,000 characters on one line, every word different so a wrong position is
/// a wrong word.
fn long_line() -> String {
    (0..20_000).map(|i| format!("w{i} ")).collect()
}

#[test]
fn a_long_line_is_drawn_but_only_the_part_that_is_on_screen_is_shaped() {
    let h = Harness::new(&long_line());
    assert!(h.glyph_extent().is_some(), "text was drawn");
    assert!(
        h.shaped_len() < 6_000,
        "shaped {} characters of a 130,000 character line",
        h.shaped_len()
    );
    assert!(
        h.row_text(0).starts_with("w0 w1 w2"),
        "the start of the line is what shows"
    );
}

#[test]
fn clicking_a_long_line_puts_the_caret_on_the_character_under_the_pointer() {
    let mut h = Harness::new(&long_line());
    click_char(&mut h, 0, 10);
    assert_eq!(h.caret(), 10);
    click_char(&mut h, 0, 30);
    assert_eq!(h.caret(), 30);
}

#[test]
fn end_on_a_long_line_scrolls_to_the_end_and_the_caret_is_there() {
    let text = long_line();
    let n = text.chars().count();
    let mut h = Harness::new(&text);
    h.key(Key::End);
    h.frame();
    assert_eq!(h.caret(), n);
    assert!(
        h.scroll_x() > 100_000.0,
        "scrolled a long way: {}",
        h.scroll_x()
    );
    let caret = h
        .caret_rect()
        .expect("the caret is drawn at the end of the line");
    assert!(caret.right() <= h.rect().right(), "and inside the pane");
    h.type_like_a_person("END");
    assert!(h.text().ends_with(" END"), "typing lands at the end");
    h.key(Key::Home);
    h.key(Key::Home);
    h.frame();
    assert_eq!(h.scroll_x(), 0.0, "Home brings the start back into view");
    assert!(h.row_text(0).starts_with("w0 "));
}

#[test]
fn clicking_far_along_a_long_line_lands_on_the_right_character() {
    let text = long_line();
    let n = text.chars().count();
    let mut h = Harness::new(&text);
    h.key(Key::End);
    h.frame();
    // The caret is at the end; click ten characters to its left.
    let caret = h.caret_rect().unwrap();
    let mut at = Pos2::new(caret.left() - 10.0 * h.advance() + 1.0, caret.center().y);
    at.x = at.x.max(h.rect().left() + 60.0);
    h.click(at);
    let want = n - (((caret.left() - at.x) / h.advance()).round() as usize);
    assert!(
        h.caret().abs_diff(want) <= 1,
        "clicked for {want} and the caret went to {}",
        h.caret()
    );
}

#[test]
fn editing_in_the_middle_of_a_long_line_far_from_its_start_changes_that_word() {
    let text = long_line();
    let n = text.chars().count();
    let mut h = Harness::new(&text);
    h.key(Key::End);
    for _ in 0..7 {
        h.key(Key::ArrowLeft);
    }
    h.type_like_a_person("X");
    let edited = h.text();
    assert_eq!(edited.chars().count(), n + 1);
    assert_eq!(
        edited.chars().nth(n - 7),
        Some('X'),
        "seven characters from the end"
    );
}

#[test]
fn scrolling_sideways_along_a_long_line_stays_where_it_was_put() {
    let mut h = Harness::new(&long_line());
    h.wheel_sideways(30.0);
    // Let the wheel's glide finish, so that what is checked next is where it stopped.
    let mut there = h.scroll_x();
    for _ in 0..200 {
        h.frame();
        if h.scroll_x() == there {
            break;
        }
        there = h.scroll_x();
    }
    assert!(there > 500.0, "the wheel moved the view: {there}");
    for _ in 0..5 {
        h.frame();
    }
    assert_eq!(
        h.scroll_x(),
        there,
        "and nothing dragged it back to the caret"
    );
    assert!(h.glyph_extent().is_some(), "with text drawn there");
    assert!(
        h.text_left() < h.rect().left() - 100.0,
        "the text has moved left by the scroll: it starts at {}",
        h.text_left()
    );
}

#[test]
fn selecting_all_of_a_long_line_selects_and_copies_every_character() {
    let text = long_line();
    let mut h = Harness::new(&text);
    h.triple_click(h.pos_of(0, 3.0));
    assert_eq!(h.selected(), text);
    h.send_copy();
    assert_eq!(h.copied().as_deref(), Some(text.as_str()));
    assert!(
        !h.wash_rows().is_empty(),
        "and the wash is drawn over what is visible"
    );
    assert!(
        h.wash_rows()[0].right() >= h.rect().right() - 40.0,
        "out to the edge of the pane"
    );
}

#[test]
fn long_lines_with_tabs_and_wide_characters_still_land_clicks_on_the_right_character() {
    for unit in ["a\tb ", "\u{e9}\u{4e2d}x ", "\u{1F600} y "] {
        let text = unit.repeat(9_000);
        let n = text.chars().count();
        let mut h = Harness::new(&text);
        h.key(Key::End);
        h.frame();
        h.frame();
        assert_eq!(h.caret(), n);
        let caret = h.caret_rect().expect("the caret is on screen at the end");
        assert!(
            caret.right() <= h.rect().right() + 1.0,
            "{unit:?}: inside the pane"
        );
        // A click a little to the left of the caret lands a few characters before the end.
        h.click(Pos2::new(caret.left() - 40.0, caret.center().y));
        assert!(
            h.caret() < n && h.caret() + 12 > n,
            "{unit:?}: clicked forty points left of the end and the caret is {} of {n}",
            h.caret()
        );
    }
}

#[test]
fn a_long_line_between_short_ones_does_not_disturb_them() {
    let doc = format!("first\n{}\nlast line", "x".repeat(50_000));
    let mut h = Harness::new(&doc);
    assert_eq!(h.drawn_line_numbers(), vec![0, 1, 2]);
    h.click(h.pos_of(2, 3.0));
    assert_eq!(h.line_column(), (3, 4));
    h.click(h.pos_of(0, 2.0));
    assert_eq!(h.line_column(), (1, 3));
    h.key(Key::ArrowDown);
    h.key(Key::ArrowDown);
    assert_eq!(
        h.line_column().0,
        3,
        "down twice passes through the long line"
    );
}

#[test]
fn a_long_line_that_is_still_coloured_keeps_its_colours_where_it_is_scrolled_to() {
    // Under the limit above which lines are drawn plain, but long enough to be sliced.
    let text = format!("// {}", "comment word ".repeat(600));
    let mut h = Harness::new(&text);
    h.set_lang(BLOCKY);
    h.frame();
    let start = h.colour_of("comment").expect("drawn");
    h.key(Key::End);
    h.frame();
    let end = h.colour_of("word").expect("drawn at the end");
    assert_eq!(
        start, end,
        "the whole line is a comment, wherever it is scrolled to"
    );
}

// ---- windows line endings -----------------------------------------------------
//
// The editor's buffer holds `\n` only: a file with Windows endings has them turned
// into newlines when it is opened and back into CRLF when it is saved (see
// `Doc::open`). These tests drive the editor over a document loaded that way, and
// the loader itself is tested in `buffer` and `editor`.

#[test]
fn a_document_opened_from_a_windows_file_behaves_like_any_other_at_line_ends() {
    let (buf, crlf) =
        crate::buffer::Buffer::from_reader_lf(std::io::Cursor::new(b"abc\r\ndef\r\nghi")).unwrap();
    assert!(crlf);
    let mut h = Harness::new(&buf.to_text());
    let mut at = h.pos_of(0, 0.0);
    at.x = h.rect().right() - 40.0;
    h.click(at);
    h.frame();
    assert_eq!(
        h.caret(),
        3,
        "a click past the end goes after the last letter"
    );
    h.type_text("X");
    assert_eq!(h.text(), "abcX\ndef\nghi");
    h.key(Key::Home);
    h.key(Key::End);
    h.key(Key::ArrowRight);
    assert_eq!(
        h.line_column(),
        (2, 1),
        "one Right at a line's end goes to the start of the next, no hidden step"
    );
}

// ---- a person at the keyboard -------------------------------------------------
//
// The tests above drive the editor with the smallest input that proves a rule. These
// use the inputs a person produces - keys with their text, held keys with repeats,
// clicks with time between the press and the release - and check what is on the
// screen and in the buffer at the end, which is the only thing they can see.

#[test]
fn typing_a_function_the_way_a_person_does_indents_and_closes_it() {
    let mut h = Harness::new("");
    // Typed up to the end of the statement inside the braces. The brace closed itself
    // when it was typed, and the line break put that closer on a line of its own with
    // the caret indented between the two.
    h.type_like_a_person("fn main() {\nlet x = 1;");
    assert_eq!(
        h.text(),
        "fn main() {\n    let x = 1;\n}",
        "auto-indent after the brace, the paren closed and typed over, the brace closed for me"
    );
}

#[test]
fn typing_a_key_does_not_insert_it_twice() {
    // A keyboard sends the key and the text it makes. An editor that acted on both
    // would double every character, and one that acted on the key for Enter and the
    // text for a newline would double every line break.
    let mut h = Harness::new("");
    h.type_like_a_person("ab\ncd");
    assert_eq!(h.text(), "ab\ncd");
    assert_eq!(h.line_column(), (2, 3));
}

#[test]
fn a_held_down_arrow_walks_every_line_and_stops_at_the_end() {
    let doc = (0..60)
        .map(|i| format!("line {i} has some text"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut h = Harness::new(&doc);
    click_char(&mut h, 0, 3);
    let mut seen = vec![h.line_column().0];
    for _ in 0..70 {
        h.hold(Key::ArrowDown, 0);
        seen.push(h.line_column().0);
    }
    assert!(
        seen.windows(2)
            .all(|w| w[1] == w[0] + 1 || (w[0] == 60 && w[1] == 60)),
        "each press moved exactly one line and the last line held: {seen:?}"
    );
    assert_eq!(*seen.last().unwrap(), 60);
    // And a single press-and-hold with repeats does the same thing, faster. Back to
    // the top first: the window is scrolled, so row 0 is not line 1.
    h.key_mod(Key::Home, CTRL);
    h.hold(Key::ArrowDown, 20);
    assert_eq!(
        h.line_column().0,
        22,
        "one press and twenty repeats is twenty-one lines"
    );
}

#[test]
fn the_caret_stays_on_screen_while_a_key_is_held_down() {
    let doc = (0..300)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut h = Harness::new(&doc);
    click_char(&mut h, 0, 2);
    for _ in 0..250 {
        h.hold(Key::ArrowDown, 0);
        assert!(
            h.caret_rect().is_some(),
            "the caret was scrolled out of the pane at line {}",
            h.line_column().0
        );
    }
}

#[test]
fn shift_and_the_arrows_select_and_typing_replaces_the_selection() {
    let mut h = Harness::new("hello world\nsecond line");
    click_char(&mut h, 0, 6);
    for _ in 0..5 {
        h.key_mod(Key::ArrowRight, SHIFT);
    }
    assert_eq!(h.selected(), "world");
    h.type_like_a_person("there");
    assert_eq!(h.text(), "hello there\nsecond line");
    assert_eq!(
        h.selected(),
        "",
        "typing over a selection leaves none behind"
    );
}

#[test]
fn shift_down_selects_to_the_same_column_on_the_next_line() {
    let mut h = Harness::new("abcdef\nabcdef");
    click_char(&mut h, 0, 2);
    h.key_mod(Key::ArrowDown, SHIFT);
    assert_eq!(h.selected(), "cdef\nab");
}

#[test]
fn control_and_the_arrows_move_a_word_at_a_time() {
    let mut h = Harness::new("alpha beta gamma");
    click_char(&mut h, 0, 0);
    h.key_mod(Key::ArrowRight, CTRL);
    assert_eq!(h.caret(), 6, "to the start of the next word");
    h.key_mod(Key::ArrowRight, CTRL);
    assert_eq!(h.caret(), 11);
    h.key_mod(Key::ArrowLeft, CTRL);
    assert_eq!(h.caret(), 6);
}

#[test]
fn home_goes_to_the_code_first_and_the_line_start_second() {
    let mut h = Harness::new("        indented");
    click_char(&mut h, 0, 12);
    h.key(Key::Home);
    assert_eq!(
        h.caret(),
        8,
        "first press: the first character that is not a space"
    );
    h.key(Key::Home);
    assert_eq!(h.caret(), 0, "second press: the very start");
    h.key(Key::Home);
    assert_eq!(h.caret(), 8, "and it alternates");
}

#[test]
fn double_click_takes_a_word_and_triple_click_a_line() {
    let mut h = Harness::new("one two three\nfour five");
    h.double_click(h.pos_of(0, 5.0));
    assert_eq!(h.selected(), "two");
    h.triple_click(h.pos_of(1, 2.0));
    assert_eq!(h.selected(), "four five");
}

#[test]
fn backspace_at_the_start_of_a_line_joins_it_to_the_one_above() {
    let mut h = Harness::new("first\nsecond");
    click_char(&mut h, 1, 0);
    h.key(Key::Backspace);
    assert_eq!(h.text(), "firstsecond");
    assert_eq!(h.caret(), 5, "the caret is at the join");
}

#[test]
fn delete_at_the_end_of_a_line_joins_the_next_one_up() {
    let mut h = Harness::new("first\nsecond");
    click_char(&mut h, 0, 5);
    h.key(Key::Delete);
    assert_eq!(h.text(), "firstsecond");
}

#[test]
fn enter_in_an_indented_line_keeps_the_indent_and_shift_tab_takes_it_back() {
    let mut h = Harness::new("    let x = 1;");
    click_char(&mut h, 0, 14);
    h.key(Key::Enter);
    h.type_like_a_person("y");
    assert_eq!(h.text(), "    let x = 1;\n    y");
    h.key_mod(Key::Tab, SHIFT);
    assert_eq!(
        h.text(),
        "    let x = 1;\ny",
        "one level out, and only that line"
    );
}

#[test]
fn select_all_delete_and_undo_brings_the_document_back() {
    let mut h = Harness::new("keep\nthis\ntext");
    h.key_mod(Key::A, CTRL);
    h.key(Key::Delete);
    assert_eq!(h.text(), "");
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), "keep\nthis\ntext");
}

#[test]
fn after_scrolling_a_click_lands_on_the_line_under_the_pointer() {
    // The window is positioned in points and drawn from partway into its first
    // line, so a click has to be mapped through that offset. Getting it wrong puts
    // the caret a line away from where the pointer is, and only after scrolling.
    let doc = (0..200)
        .map(|i| format!("line number {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut h = Harness::new(&doc);
    for notches in [1.0, 2.5, 7.0, 13.0] {
        h.scroll(notches);
        h.frame();
        let shown = h.drawn_line_numbers();
        // Rows below the first: the first is partly scrolled off the top, and a
        // click above the pane is not a click on the editor.
        for row in [2usize, 3, 8] {
            h.click(h.pos_of(row, 2.0));
            h.frame();
            assert_eq!(
                h.line_column().0 - 1,
                shown[row],
                "after scrolling {notches}, a click on row {row} went to a different line"
            );
        }
    }
}

#[test]
fn a_click_held_for_a_few_frames_still_puts_the_caret_under_the_pointer() {
    let mut h = Harness::new("alpha beta\ngamma delta");
    h.click_like_a_person(h.pos_of(1, 3.0));
    assert_eq!(h.line_column(), (2, 4));
}

#[test]
fn page_down_and_up_move_by_about_a_screenful_and_keep_the_caret_visible() {
    let doc = (0..300)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut h = Harness::new(&doc);
    click_char(&mut h, 0, 0);
    h.key(Key::PageDown);
    let after = h.line_column().0;
    assert!(
        after > 20 && after < 60,
        "a page is about a screenful of lines, not {after}"
    );
    assert!(
        h.caret_rect().is_some(),
        "the caret is on screen after a page"
    );
    h.key(Key::PageUp);
    assert_eq!(h.line_column().0, 1, "and back up to where it started");
}

// ---- moving the caret up and down ---------------------------------------------

#[test]
fn moving_down_through_a_short_line_remembers_the_column() {
    // The behaviour of every editor: the column you were in is where you come back
    // to on the next long line, whatever short line you passed through.
    let mut h = Harness::new("a long line of text\nx\nanother long line here");
    click_char(&mut h, 0, 12);
    h.key(Key::ArrowDown);
    assert_eq!(
        h.line_column(),
        (2, 2),
        "the short line has only its end to offer"
    );
    h.key(Key::ArrowDown);
    assert_eq!(
        h.line_column(),
        (3, 13),
        "back on a long line, at the column the caret started in"
    );
    h.key(Key::ArrowUp);
    h.key(Key::ArrowUp);
    assert_eq!(h.line_column(), (1, 13), "and the same going back up");
}

#[test]
fn a_horizontal_move_or_a_click_forgets_the_remembered_column() {
    let mut h = Harness::new("a long line of text\nx\nanother long line here");
    click_char(&mut h, 0, 12);
    h.key(Key::ArrowDown);
    h.key(Key::ArrowLeft);
    h.key(Key::ArrowDown);
    assert_eq!(h.line_column().0, 3);
    assert_eq!(
        h.line_column().1,
        1,
        "the column was reset by the Left, so it is now the short line's"
    );
}

#[test]
fn up_on_the_first_line_goes_to_its_start_and_down_on_the_last_to_its_end() {
    let mut h = Harness::new("first line\nsecond line");
    click_char(&mut h, 0, 5);
    h.key(Key::ArrowUp);
    assert_eq!(h.caret(), 0, "Up from the top line goes to the very start");
    click_char(&mut h, 1, 4);
    h.key(Key::ArrowDown);
    assert_eq!(
        h.caret(),
        22,
        "Down from the bottom line goes to the very end"
    );
}

#[test]
fn moving_between_lines_keeps_the_caret_under_the_same_screen_position() {
    // A tab is wide and a letter is narrow, so the same *character* column is not
    // the same place on screen. Moving between an indented line and a plain one
    // has to follow where the caret is, not how many characters it is in.
    let mut h = Harness::new("\t\tindented text here\nplain text without indent");
    click_char(&mut h, 0, 8);
    let before = h.caret_rect().expect("caret drawn").center().x;
    h.key(Key::ArrowDown);
    h.frame();
    let after = h.caret_rect().expect("caret drawn").center().x;
    assert!(
        (before - after).abs() < h.advance() * 0.6,
        "the caret jumped sideways from x={before:.1} to x={after:.1}"
    );
}

#[test]
fn down_in_wrapped_text_moves_a_row_not_a_whole_line() {
    let long = "word ".repeat(60);
    let doc = format!("{long}\nnext line");
    let mut h = Harness::new(&doc);
    h.set_wrap(true);
    h.frame();
    click_char(&mut h, 0, 3);
    h.key(Key::ArrowDown);
    h.frame();
    let row = h.caret_row().expect("caret on a row");
    assert_eq!(
        row, 1,
        "one Down moves to the next visual row of the same line"
    );
    assert!(
        h.caret() < long.chars().count(),
        "still inside the wrapped line"
    );
}

#[test]
fn a_run_of_downs_visits_every_line_once_and_never_skips_or_sticks() {
    let doc = (0..30)
        .map(|i| format!("line number {i} with some text"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut h = Harness::new(&doc);
    click_char(&mut h, 0, 5);
    for expect in 1..30 {
        h.key(Key::ArrowDown);
        assert_eq!(h.line_column().0, expect + 1, "after {expect} Downs");
    }
    for expect in (0..29).rev() {
        h.key(Key::ArrowUp);
        assert_eq!(h.line_column().0, expect + 1, "coming back up");
    }
}

// ---- clicking past the end of a row -------------------------------------------

/// Lines that are awkward for a layout that assumes one glyph per character.
const AWKWARD: &[&str] = &[
    "plain ascii line",
    "\tindented with one tab",
    "\t\ttwo tabs then text",
    "tab\tin the\tmiddle",
    "caf\u{e9} na\u{ef}ve r\u{e9}sum\u{e9}",
    "emoji \u{1F600} and \u{65E5}\u{672C}\u{8A9E} text",
    "combining e\u{301} marks a\u{308}",
    "trailing spaces   ",
    "",
    "last line",
];

#[test]
fn a_click_right_of_any_row_puts_the_caret_at_the_end_of_that_line() {
    let doc = AWKWARD.join("\n");
    for scale in [1.0f32, 1.25, 1.5, 2.0] {
        let mut h = Harness::new(&doc);
        h.set_scale(scale);
        let mut start = 0usize;
        for (i, line) in AWKWARD.iter().enumerate() {
            let end = start + line.chars().count();
            // Far to the right of the text on this row, inside the pane.
            let mut at = h.pos_of(i, 0.0);
            at.x = h.rect().right() - 40.0;
            h.click(at);
            h.frame();
            assert_eq!(
                h.caret(),
                end,
                "scale {scale}: clicking right of line {i} ({line:?}) put the caret at {} \
                 instead of its end at {end}",
                h.caret()
            );
            start = end + 1;
        }
    }
}

#[test]
fn a_click_right_of_a_wrapped_row_lands_at_the_end_of_that_row() {
    // Wrapped, a line is several rows, and "the end of the row" is where the row
    // breaks, which is not the end of the line.
    let long = "word ".repeat(60);
    let mut h = Harness::new(&long);
    h.set_wrap(true);
    h.frame();
    let rows = h.rows().to_vec();
    assert!(rows.len() > 2, "the line should wrap onto several rows");
    let base = h.window_base();
    // The right edge of the text area itself, just left of the scrollbar.
    let edge = h.bar().map_or(h.rect().right(), |b| b.left()) - 1.0;
    for (i, span) in rows.iter().enumerate().take(rows.len() - 1) {
        let mut at = h.pos_of(i, 0.0);
        at.x = edge;
        h.click(at);
        h.frame();
        let want = base + span.chars.1;
        let got = h.caret();
        assert!(
            got == want || got + 1 == want,
            "row {i} covers {:?} but a click at its right edge put the caret at {got}",
            span.chars
        );
    }
}

// ---- colouring across lines ---------------------------------------------------

/// A language with `/* */` block comments, the shape of C, Rust and most others.
const BLOCKY: super::Lang = super::Lang {
    line: "//",
    block: Some(("/*", "*/")),
    triple: false,
    template: false,
};

#[test]
fn a_block_comment_colours_every_line_it_spans_and_stops_where_it_ends() {
    let mut h = Harness::new("/* opens\nstill inside\nends */ outside");
    h.set_lang(BLOCKY);
    h.frame();
    let opens = h.colour_of("opens").expect("on screen");
    let inside = h.colour_of("inside").expect("on screen");
    let outside = h.colour_of("outside").expect("on screen");
    assert_eq!(opens, inside, "the middle line is still the comment");
    assert_ne!(inside, outside, "and the text after the close is not");
}

#[test]
fn opening_a_comment_above_recolours_the_lines_below_it() {
    let mut h = Harness::new("one\nlet value = 1");
    h.set_lang(BLOCKY);
    h.frame();
    let before = h.colour_of("value").expect("on screen");
    click_char(&mut h, 0, 0);
    h.type_text("/*");
    h.frame();
    let comment = h.colour_of("/*").expect("on screen");
    assert_eq!(
        h.colour_of("value"),
        Some(comment),
        "the line below is inside the comment that was just opened"
    );
    assert_ne!(before, comment);
}

#[test]
fn colouring_a_line_far_below_is_worked_out_off_the_frame_and_arrives() {
    // Far more lines than are scanned in a frame, with a comment opened at the top
    // and closed just before the last line, so the answer depends on all of them.
    let mut text = String::from("/* opens\n");
    text.push_str(&"filler\n".repeat(60_000));
    text.push_str("*/ closes\nlet x = 1");
    let mut h = Harness::new(&text);
    h.set_lang(BLOCKY);
    h.frame();
    h.key_mod(Key::End, CTRL);
    h.frame();
    let plain = h.colour_of("x").expect("the end of the file is on screen");
    // Uncoloured while the worker runs, and the keyword colour once it answers.
    let coloured = h.frames_until(|h| h.colour_of("let") != h.colour_of("x"));
    assert!(coloured, "the keyword was never coloured");
    assert_eq!(h.colour_of("x"), Some(plain), "plain text is unchanged");
}

#[test]
fn an_idle_editor_does_not_ask_to_be_redrawn() {
    let mut h = Harness::new("nothing is happening here");
    for _ in 0..5 {
        h.frame();
    }
    assert!(
        h.repaint_delay() >= std::time::Duration::from_millis(250),
        "an idle frame asked for another one after {:?}",
        h.repaint_delay()
    );
}

// ---- a search too big for one frame ---------------------------------------------

#[test]
fn a_large_document_is_searched_on_a_worker_and_the_hits_arrive() {
    // Over the size searched inside a frame.
    let line = "some ordinary line of text here\n";
    let big = line.repeat(5 * 1024 * 1024 / line.len() + 1);
    let mut h = Harness::new(&big);
    h.find("ordinary");
    assert!(
        h.find_searching() || h.find_hits() > 0,
        "the search was either running or already done"
    );
    assert!(
        h.frames_until(|h| !h.find_searching() && h.find_hits() > 0),
        "the hits never arrived"
    );
    assert_eq!(h.find_hits(), super::find::MAX_HITS, "held up to the cap");
    assert!(h.find_hits_capped());
    assert_eq!(h.selected(), "ordinary", "and the first one is selected");
}

// ---- the caret is drawn where it is, while typing --------------------------------

/// After every step, the caret's drawn rectangle is on a row of the line the caret is
/// in. A caret on the line underneath the one being typed on is the fault.
fn assert_caret_on_its_line(h: &mut Harness, what: &str, log: &[String]) {
    h.frame();
    let text = h.text();
    let caret = h.caret();
    let line = text.chars().take(caret).filter(|c| *c == '\n').count();
    let Some(rect) = h.caret_rect() else {
        panic!(
            "{what}: no caret drawn at {caret}; line/col {:?}, scroll_x {}, top line {}, rows {}, focused {}, text {text:?}\n{}",
            h.line_column(),
            h.scroll_x(),
            h.top_line_raw(),
            h.row_tops().len(),
            h.focused(),
            log.join(" ")
        );
    };
    let tops = h.row_tops();
    let Some(row) = tops.iter().position(|t| (rect.top() - t).abs() < 1.0) else {
        panic!(
            "{what}: the caret at y={} is on no row {tops:?}\n{}",
            rect.top(),
            log.join(" ")
        );
    };
    let rows = h.rows().to_vec();
    let drawn_line = rows[row].line;
    let top = h.top_line_raw();
    assert_eq!(
        drawn_line,
        line,
        "{what}: the caret is at char {caret} on line {line}, but is drawn on row {row} \
         which belongs to line {drawn_line} (window from line {top})\n{}\ntext: {text:?}",
        log.join(" ")
    );
}

#[test]
fn typing_at_random_keeps_the_caret_on_the_line_being_typed() {
    let alphabet = [
        "a", "b", "e", "x", " ", " ", "\n", "(", ")", "{", "}", "\"", "'", "\t", "é", "日",
    ];
    for wrap in [false, true] {
        for seed in 0..30u64 {
            let mut rng = Rng::new(0xC0FFEE + seed * 31);
            let opts = Options {
                wrap,
                ..Options::default()
            };
            let mut h = Harness::with_options(
                "fn main() {\n    let x = 1;\n}\n\nsome prose that is long enough to wrap around the pane a couple of times over\n",
                opts,
                Vec2::new(260.0, 300.0),
            );
            h.key_mod(egui::Key::End, Modifiers::CTRL);
            let mut log: Vec<String> = Vec::new();
            for step in 0..120 {
                let before = format!("{:?} caret {} sel {:?}", h.text(), h.caret(), h.selection());
                let what = format!("wrap {wrap}, seed {seed}, step {step}, before: {before}");
                match rng.below(12) {
                    0..=6 => {
                        let s = alphabet[rng.below(alphabet.len())];
                        log.push(format!("type{s:?}"));
                        if s == "\n" {
                            h.key(egui::Key::Enter);
                        } else if s == "\t" {
                            h.key(egui::Key::Tab);
                        } else {
                            h.type_text(s);
                        }
                    }
                    7 => {
                        log.push("Backspace".into());
                        h.key(egui::Key::Backspace);
                    }
                    8 => {
                        let k = [
                            egui::Key::ArrowLeft,
                            egui::Key::ArrowRight,
                            egui::Key::ArrowUp,
                            egui::Key::ArrowDown,
                        ][rng.below(4)];
                        log.push(format!("{k:?}"));
                        h.key(k);
                    }
                    9 => {
                        let k = [egui::Key::Home, egui::Key::End][rng.below(2)];
                        log.push(format!("{k:?}"));
                        h.key(k);
                    }
                    10 => {
                        log.push("Delete".into());
                        h.key(egui::Key::Delete);
                    }
                    _ => {
                        // A click somewhere on a row, far to the right of its text or not.
                        let rows = h.row_tops().len().max(1);
                        let row = rng.below(rows);
                        let col = [0.5, 3.0, 8.0, 25.0][rng.below(4)];
                        log.push(format!("click(row {row}, col {col})"));
                        // Where the text is, when the view has scrolled sideways, can be
                        // off the pane; a click is always somewhere in it.
                        let at = h.pos_of(row, col);
                        let pane = h.rect();
                        h.click(Pos2::new(
                            at.x.clamp(pane.left() + 35.0, pane.right() - 30.0),
                            at.y,
                        ));
                    }
                }
                assert_caret_on_its_line(&mut h, &what, &log);
            }
        }
    }
}

// ---- where the caret is drawn: line ends, line starts, and the moves between -----

/// The line and the column of the caret, from the text and the caret index alone.
fn line_col_of(h: &Harness) -> (usize, usize) {
    let text = h.text();
    let caret = h.caret();
    let before: String = text.chars().take(caret).collect();
    let line = before.matches('\n').count();
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count());
    (line, col)
}

/// The width of one character, measured over a long row: a short one has too few
/// glyphs, each snapped to a whole pixel, to give it exactly.
fn cell_width() -> f32 {
    Harness::new(&"x".repeat(60)).advance()
}

/// The caret is drawn on the row that shows its line, at the column it is in: `col`
/// characters of a monospaced face along from where the text starts. Not on a
/// neighbouring line, and not at a neighbouring column.
fn assert_caret_drawn_at(h: &mut Harness, line: usize, col: usize, what: &str) {
    h.frame();
    assert_eq!(
        line_col_of(h),
        (line, col),
        "{what}: the caret index is wrong"
    );
    let rect = h
        .caret_rect()
        .unwrap_or_else(|| panic!("{what}: no caret is drawn"));
    let row = h
        .row_tops()
        .iter()
        .position(|t| (rect.top() - t).abs() < 1.0)
        .unwrap_or_else(|| panic!("{what}: the caret is on no row"));
    let drawn = h.rows()[row].line;
    let top = h.top_line_raw();
    assert_eq!(
        drawn, line,
        "{what}: the caret belongs on line {line} but is drawn on a row of line {drawn} (window from line {top})"
    );
    let want = h.text_left() + col as f32 * cell_width() - h.scroll_x();
    assert!(
        (rect.left() - want).abs() < 1.5,
        "{what}: the caret is drawn at x={:.1}, and column {col} is at x={want:.1}",
        rect.left()
    );
}

#[test]
fn the_end_key_draws_the_caret_after_the_last_letter_of_the_line_not_on_the_next_line() {
    let doc = "alpha\nbe\n\nlonger line here\nz";
    for (line, len) in [(0, 5), (1, 2), (2, 0), (3, 16), (4, 1)] {
        let mut h = Harness::new(doc);
        h.click(h.pos_of(line, 0.5));
        h.key(egui::Key::End);
        assert_caret_drawn_at(&mut h, line, len, &format!("End on line {line}"));
        h.key(egui::Key::Home);
        assert_caret_drawn_at(&mut h, line, 0, &format!("Home on line {line}"));
    }
}

#[test]
fn a_click_past_the_end_of_a_line_draws_the_caret_at_the_end_of_that_line() {
    let doc = "alpha\nbe\n\nlonger line here\nz\n";
    for (line, len) in [(0, 5), (1, 2), (2, 0), (3, 16), (4, 1), (5, 0)] {
        let mut h = Harness::new(doc);
        h.click(h.pos_of(line, 30.0));
        assert_caret_drawn_at(
            &mut h,
            line,
            len,
            &format!("a click far right of line {line}"),
        );
    }
}

#[test]
fn arrows_across_a_line_break_move_the_caret_to_the_other_line_and_draw_it_there() {
    let mut h = Harness::new("abc\nde\nf");
    h.click(h.pos_of(0, 0.5));
    h.key(egui::Key::End);
    assert_caret_drawn_at(&mut h, 0, 3, "end of the first line");
    h.key(egui::Key::ArrowRight);
    assert_caret_drawn_at(&mut h, 1, 0, "Right over the line break");
    h.key(egui::Key::ArrowLeft);
    assert_caret_drawn_at(&mut h, 0, 3, "Left back over it");
    h.key(egui::Key::ArrowDown);
    assert_caret_drawn_at(&mut h, 1, 2, "Down from the end of a longer line");
    h.key(egui::Key::ArrowDown);
    assert_caret_drawn_at(&mut h, 2, 1, "Down onto the last line");
    h.key(egui::Key::ArrowUp);
    assert_caret_drawn_at(&mut h, 1, 2, "Up returns to the end of the shorter line");
}

#[test]
fn enter_at_the_end_of_a_line_draws_the_caret_at_the_start_of_the_new_line() {
    let mut h = Harness::new("first\nsecond");
    h.click(h.pos_of(0, 30.0));
    h.key(egui::Key::Enter);
    assert_caret_drawn_at(&mut h, 1, 0, "after Enter");
    h.type_text("x");
    assert_caret_drawn_at(&mut h, 1, 1, "typing on the new line");
    h.key(egui::Key::Backspace);
    h.key(egui::Key::Backspace);
    assert_caret_drawn_at(&mut h, 0, 5, "Backspace over the break rejoins the lines");
}

#[test]
fn typing_along_a_line_moves_the_caret_one_column_a_letter_on_the_same_row() {
    let mut h = Harness::new("start\nnext");
    h.click(h.pos_of(0, 30.0));
    let y = h.caret_rect().unwrap().top();
    for (i, ch) in "hello world".chars().enumerate() {
        h.type_text(&ch.to_string());
        assert_caret_drawn_at(&mut h, 0, 5 + i + 1, &format!("after typing {ch:?}"));
        assert_eq!(
            h.caret_rect().unwrap().top(),
            y,
            "the caret stays on its row"
        );
    }
}

#[test]
fn a_blank_line_has_the_caret_at_its_start_and_only_there() {
    let mut h = Harness::new("a\n\n\nb");
    h.click(h.pos_of(1, 20.0));
    assert_caret_drawn_at(&mut h, 1, 0, "clicking a blank line");
    h.key(egui::Key::ArrowDown);
    assert_caret_drawn_at(&mut h, 2, 0, "Down to the next blank line");
    h.key(egui::Key::End);
    assert_caret_drawn_at(&mut h, 2, 0, "End on a blank line");
    h.key(egui::Key::ArrowLeft);
    assert_caret_drawn_at(&mut h, 1, 0, "Left from a blank line");
}

#[test]
fn the_last_line_with_and_without_a_final_newline_holds_the_caret() {
    let mut h = Harness::new("one\ntwo");
    h.key_mod(egui::Key::End, Modifiers::CTRL);
    assert_caret_drawn_at(&mut h, 1, 3, "end of a document with no final newline");
    let mut h = Harness::new("one\ntwo\n");
    h.key_mod(egui::Key::End, Modifiers::CTRL);
    assert_caret_drawn_at(&mut h, 2, 0, "end of a document that ends in a newline");
    h.type_text("x");
    assert_caret_drawn_at(&mut h, 2, 1, "typing there");
}

#[test]
fn lines_of_wide_and_accented_characters_put_the_caret_after_them() {
    let mut h = Harness::new("é日x\nnext");
    h.click(h.pos_of(0, 30.0));
    let (line, col) = line_col_of(&h);
    assert_eq!((line, col), (0, 3));
    let rect = h.caret_rect().unwrap();
    let rows = h.row_tops();
    assert!(
        (rect.top() - rows[0]).abs() < 1.0,
        "on the first row, not the second"
    );
    assert!(rect.left() > h.text_left(), "and after the letters");
}

#[test]
fn deep_in_a_big_file_the_caret_is_still_on_its_line_at_line_ends() {
    let doc: String = (0..5000).map(|i| format!("line {i}\n")).collect();
    let mut h = Harness::new(&doc);
    for _ in 0..40 {
        h.key(egui::Key::PageDown);
    }
    let line = line_col_of(&h).0;
    h.key(egui::Key::End);
    let len = format!("line {line}").len();
    assert_caret_drawn_at(&mut h, line, len, "End after paging down");
    h.key(egui::Key::ArrowRight);
    assert_caret_drawn_at(
        &mut h,
        line + 1,
        0,
        "Right over the break, deep in the file",
    );
    for _ in 0..30 {
        h.key(egui::Key::ArrowDown);
    }
    h.key(egui::Key::End);
    let line = line_col_of(&h).0;
    assert_caret_drawn_at(
        &mut h,
        line,
        format!("line {line}").len(),
        "End after walking down",
    );
}

#[test]
fn selecting_to_the_end_of_a_line_leaves_the_caret_at_that_end() {
    let mut h = Harness::new("hello world\nnext line");
    h.key_mod(egui::Key::Home, Modifiers::CTRL);
    h.key_mod(egui::Key::End, Modifiers::SHIFT);
    assert_eq!(h.selected(), "hello world");
    assert_caret_drawn_at(&mut h, 0, 11, "Shift+End");
    h.key_mod(egui::Key::ArrowDown, Modifiers::SHIFT);
    assert_caret_drawn_at(&mut h, 1, 9, "Shift+Down from the end of a line");
}

#[test]
fn pasting_several_lines_leaves_the_caret_at_the_end_of_the_last_one() {
    let mut h = Harness::new("ab\ncd");
    h.click(h.pos_of(0, 30.0));
    h.paste("X\nYY\nZZZ");
    assert_caret_drawn_at(
        &mut h,
        2,
        3,
        "after a three line paste at the end of a line",
    );
    h.key_mod(egui::Key::Z, Modifiers::CTRL);
    assert_caret_drawn_at(&mut h, 0, 2, "after undoing it");
}

#[test]
fn every_position_in_a_document_is_drawn_on_its_own_line_and_column() {
    // The caret walked through the whole of a small document with Right, then back
    // with Left: at every one of the positions it is where its line and column say.
    let doc = "ab\n\ncde f\n g\nend";
    let mut h = Harness::new(doc);
    h.key_mod(egui::Key::Home, Modifiers::CTRL);
    let n = doc.chars().count();
    for _ in 0..=n {
        let (line, col) = line_col_of(&h);
        assert_caret_drawn_at(&mut h, line, col, "walking right");
        h.key(egui::Key::ArrowRight);
    }
    for _ in 0..=n {
        let (line, col) = line_col_of(&h);
        assert_caret_drawn_at(&mut h, line, col, "walking left");
        h.key(egui::Key::ArrowLeft);
    }
}

#[test]
fn wrapped_rows_draw_the_caret_on_the_row_it_is_typed_on() {
    let opts = Options {
        wrap: true,
        ..Options::default()
    };
    let mut h = Harness::with_options("word ".repeat(40).trim_end(), opts, Vec2::new(200.0, 400.0));
    h.key_mod(egui::Key::End, Modifiers::CTRL);
    let rows_before = h.row_count();
    assert!(rows_before > 3, "the line wrapped onto several rows");
    assert_eq!(h.caret_row(), Some(rows_before - 1));
    // Typing on: the caret is on the last row, wherever the text wraps to.
    let mut last = h.caret_row().unwrap();
    for _ in 0..60 {
        h.type_text("y");
        let row = h.caret_row().unwrap();
        assert_eq!(
            row,
            h.row_count() - 1,
            "typing at the end is on the last row"
        );
        assert!(row >= last, "the caret never moves up while typing");
        last = row;
    }
    // Home and End of a middle row go to that row's ends, and are drawn on it.
    h.click(h.pos_of(1, 3.0));
    let row = h.caret_row().unwrap();
    h.key(egui::Key::End);
    assert_eq!(h.caret_row(), Some(row), "End stays on the row");
    h.key(egui::Key::Home);
    assert_eq!(h.caret_row(), Some(row), "Home stays on the row");
}

#[test]
fn an_undo_that_removes_a_line_break_puts_the_caret_at_the_end_of_the_joined_line() {
    let mut h = Harness::new("head\ntail");
    h.click(h.pos_of(0, 30.0));
    h.key(egui::Key::Enter);
    h.type_text("new");
    h.key_mod(egui::Key::Z, Modifiers::CTRL);
    h.key_mod(egui::Key::Z, Modifiers::CTRL);
    let (line, col) = line_col_of(&h);
    assert_caret_drawn_at(&mut h, line, col, "after undoing the typing and the Enter");
    assert_eq!(h.text(), "head\ntail");
}

#[test]
fn an_editor_given_the_keyboard_without_a_click_keeps_it_through_every_arrow_key() {
    // The keyboard is handed to the editor when a file opens, with no click. egui moves
    // the focus on an arrow key unless the widget has claimed the arrows, and the
    // claim used to be made only when the editor was clicked.
    let doc: String = (0..300).map(|i| format!("line {i}\n")).collect();
    let mut h = Harness::new(&doc);
    assert!(h.focused());
    for key in [
        egui::Key::ArrowRight,
        egui::Key::ArrowDown,
        egui::Key::ArrowLeft,
        egui::Key::ArrowUp,
        egui::Key::End,
        egui::Key::ArrowRight,
        egui::Key::Home,
        egui::Key::PageDown,
        egui::Key::ArrowRight,
        egui::Key::Tab,
    ] {
        h.key(key);
        assert!(
            h.focused(),
            "{key:?} took the keyboard away from the editor"
        );
    }
    for _ in 0..200 {
        h.key(egui::Key::ArrowRight);
    }
    assert!(h.focused(), "and it still has it after two hundred more");
}

#[test]
fn walking_right_through_a_document_scrolls_to_keep_the_caret_on_screen() {
    // Right and Left cross line breaks, so they can carry the caret off the bottom or
    // the top of the pane like Up and Down can.
    let doc: String = (0..200).map(|i| format!("row {i}\n")).collect();
    let mut h = Harness::with_options(&doc, Options::default(), Vec2::new(400.0, 150.0));
    for step in 0..900 {
        h.key(egui::Key::ArrowRight);
        assert!(
            h.caret_rect().is_some(),
            "step {step}: the caret at {} is off the pane",
            h.caret()
        );
    }
    for step in 0..900 {
        h.key(egui::Key::ArrowLeft);
        assert!(
            h.caret_rect().is_some(),
            "step {step} back: the caret at {} is off the pane",
            h.caret()
        );
    }
}

#[test]
fn the_caret_is_never_below_the_bottom_edge_after_moving_down_a_line_at_a_time() {
    let doc: String = (0..500).map(|i| format!("row {i}\n")).collect();
    for height in [100.0, 137.0, 200.0, 333.0, 600.0] {
        let mut h = Harness::with_options(&doc, Options::default(), Vec2::new(400.0, height));
        for step in 0..200 {
            h.key(egui::Key::ArrowDown);
            let rect = h
                .caret_rect()
                .unwrap_or_else(|| panic!("height {height}, step {step}: no caret is drawn"));
            assert!(
                rect.bottom() <= height + 0.5,
                "height {height}, step {step}: the caret's bottom is at {}, below the pane",
                rect.bottom()
            );
        }
    }
}

// ---- folding ---------------------------------------------------------------

/// The drawn row that holds document line `line`.
///
/// A fold moves the rows, so a test cannot click "row 3" and mean line 3: it asks
/// which row the line ended up on.
fn row_of_line(h: &Harness, line: usize) -> usize {
    let drawn = h.drawn_line_numbers();
    drawn
        .iter()
        .position(|&l| l == line)
        .unwrap_or_else(|| panic!("line {line} is not on screen: {drawn:?}"))
}

#[test]
fn a_brace_block_and_an_indentation_block_are_foldable() {
    // A brace block: the line ending in `{`, through its matching closer.
    let h = Harness::new("fn f() {\n    let a = 1;\n    let b = 2;\n}\n");
    assert!(
        h.fold_starts().contains(&0),
        "the brace block should fold, got {:?}",
        h.fold_starts()
    );

    // No braces anywhere, so only the indentation can find this one.
    let h = Harness::new("root\n  child one\n  child two\nsibling\n");
    assert!(
        h.fold_starts().contains(&0),
        "the indentation block should fold, got {:?}",
        h.fold_starts()
    );
    assert!(
        !h.fold_starts().contains(&3),
        "the sibling is not part of the block"
    );
}

#[test]
fn folding_hides_the_lines_inside_and_unfolding_brings_them_back() {
    let mut h = Harness::new("fn f() {\n    let a = 1;\n    let b = 2;\n}\ntail\n");
    h.toggle_fold(0);
    assert!(h.fold_closed(0), "the click closed the fold");
    let job = h.job_text();
    assert!(
        !job.contains("let a") && !job.contains("let b"),
        "the hidden lines are not shaped: {job:?}"
    );
    assert!(job.contains("fn f() {"), "the opening line stays: {job:?}");
    let drawn = h.drawn_line_numbers();
    assert!(
        !drawn.contains(&1) && !drawn.contains(&2) && !drawn.contains(&3),
        "no hidden line is drawn: {drawn:?}"
    );

    h.toggle_fold(0);
    assert!(!h.fold_closed(0), "the second click opened it");
    let job = h.job_text();
    assert!(
        job.contains("let a") && job.contains("let b"),
        "unfolding brings them back: {job:?}"
    );
}

#[test]
fn the_caret_cannot_land_inside_a_folded_region() {
    let mut h = Harness::new("fn f() {\n    let a = 1;\n    let b = 2;\n}\ntail");
    h.toggle_fold(0);
    h.key_mod(Key::End, CTRL);
    assert_eq!(h.line_column().0, 5, "the caret starts below the fold");
    h.key(Key::ArrowUp);
    assert_eq!(
        h.line_column().0,
        1,
        "up out of the fold lands on its first line"
    );
    h.key(Key::ArrowDown);
    assert_eq!(
        h.line_column().0,
        5,
        "down crosses the whole fold in one step"
    );
}

#[test]
fn editing_near_a_fold_keeps_the_buffer_and_the_fold_consistent() {
    let mut h = Harness::new("fn f() {\n    let a = 1;\n    let b = 2;\n}\ntail");
    h.toggle_fold(0);
    assert!(h.fold_closed(0));
    // Type on the folded line, before its brace. The brace is still last, so the
    // region is still there.
    let row = row_of_line(&h, 0);
    click_char(&mut h, row, 7);
    h.type_text("x");
    assert_eq!(
        h.text(),
        "fn f() x{\n    let a = 1;\n    let b = 2;\n}\ntail"
    );
    assert!(h.fold_closed(0), "typing on the fold line keeps it");
    assert!(
        !h.job_text().contains("let a"),
        "and the inside stays hidden"
    );
    // Delete the fold's own line: there is nothing left to fold.
    let row = row_of_line(&h, 0);
    click_char(&mut h, row, 0);
    h.key_mod(Key::K, CTRL_SHIFT);
    assert_eq!(h.text(), "    let a = 1;\n    let b = 2;\n}\ntail");
    assert!(!h.fold_closed(0), "deleting the fold's line unfolds it");
}

#[test]
fn typing_still_works_with_a_fold_present() {
    let mut h = Harness::new("fn f() {\n    let a = 1;\n}\ntail");
    h.toggle_fold(0);
    assert!(h.fold_closed(0));
    let row = row_of_line(&h, 3);
    click_char(&mut h, row, 0);
    h.type_text("x");
    assert_eq!(h.text(), "fn f() {\n    let a = 1;\n}\nxtail");
    assert!(h.fold_closed(0), "the fold survives the edit below it");
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), "fn f() {\n    let a = 1;\n}\ntail");
    assert!(h.fold_closed(0), "and survives the undo");
}

#[test]
fn two_nested_folds_can_both_be_closed() {
    let mut h = Harness::new("outer {\n  inner {\n    x\n  }\n}\ntail");
    assert!(
        h.fold_starts().contains(&0) && h.fold_starts().contains(&1),
        "both blocks open a fold, got {:?}",
        h.fold_starts()
    );
    // The inner first, then the block around it.
    h.toggle_fold(1);
    h.toggle_fold(0);
    assert!(h.fold_closed(0) && h.fold_closed(1));
    // The outer fold is the one that decides where the caret can stand, even
    // though the inner fold starts later and ends sooner.
    h.key_mod(Key::End, CTRL);
    h.key(Key::ArrowUp);
    assert_eq!(
        h.line_column().0,
        1,
        "the caret comes to rest on the outer fold's first line"
    );
    assert!(
        !h.job_text().contains("inner"),
        "the inner block is hidden: {:?}",
        h.job_text()
    );
}

#[test]
fn the_fold_all_and_unfold_all_shortcuts_work() {
    let mut h = Harness::new("fn f() {\n    a\n}\nfn g() {\n    b\n}\n");
    h.key_mod(Key::OpenBracket, CTRL_SHIFT);
    let starts = h.fold_starts();
    assert!(starts.len() >= 2, "two blocks should fold, got {starts:?}");
    assert!(
        starts.iter().all(|&s| h.fold_closed(s)),
        "Ctrl+Shift+[ closes every fold"
    );
    h.key_mod(Key::CloseBracket, CTRL_SHIFT);
    assert!(
        starts.iter().all(|&s| !h.fold_closed(s)),
        "Ctrl+Shift+] opens every fold"
    );
}
