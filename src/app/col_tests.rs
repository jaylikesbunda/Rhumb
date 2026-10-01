use super::*;

const WIDE: f32 = 900.0;
const NARROW: f32 = 330.0;

#[test]
fn a_wide_pane_keeps_both_columns_at_their_dragged_width() {
    assert_eq!(fit_columns(WIDE, 92.0, 132.0), (92.0, 132.0));
}

#[test]
fn a_narrow_pane_keeps_the_name_readable() {
    let (size, date) = fit_columns(NARROW, 92.0, 132.0);
    // Whatever happens, the name keeps its minimum, so the columns must
    // have given way rather than the name being squeezed to nothing.
    let chrome = sp::SM + sp::ICON + sp::SM + sp::MD + sp::MD + sp::SM;
    let name_room = NARROW - chrome - size - date;
    assert!(
        name_room >= MIN_NAME_COL,
        "name left only {name_room}px with columns {size}/{date}"
    );
}

#[test]
fn the_date_column_is_dropped_before_the_size_one() {
    // Ask for far more than any pane holds, then narrow the pane until only
    // one column can stay: the size survives, because a size is more useful
    // at a glance than a timestamp.
    let (size, date) = fit_columns(300.0, 300.0, 300.0);
    assert!(date == 0.0, "the date should go first, got {date}");
    assert!(size >= MIN_SIZE_COL, "the size should stay, got {size}");
}

#[test]
fn a_very_narrow_pane_drops_the_columns_entirely() {
    let (size, date) = fit_columns(200.0, 92.0, 132.0);
    assert_eq!((size, date), (0.0, 0.0), "nothing fits, so nothing shows");
}

#[test]
fn widening_the_pane_brings_a_dropped_column_back() {
    // A column that was dropped is not forgotten: it returns with the room.
    let (_, narrow_date) = fit_columns(320.0, 92.0, 132.0);
    let (_, wide_date) = fit_columns(900.0, 92.0, 132.0);
    assert!(narrow_date <= wide_date);
    assert_eq!(wide_date, 132.0, "the preference is intact");
}

#[test]
fn the_drag_limit_respects_the_readable_minimum() {
    let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(WIDE, 26.0));
    let l = RowLayout::new(rect, 92.0, 132.0);
    // Widening the date as far as the limit allows still leaves the name.
    let limit = col_limit(&l, 92.0);
    let at_limit = RowLayout::new(rect, 92.0, limit);
    assert!(at_limit.name_limit() - at_limit.name.x >= MIN_NAME_COL - 0.01);
}
