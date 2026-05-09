use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use rmux_core::input::mode;
use rmux_proto::{
    ErrorResponse, PaneSnapshotCell, PaneSnapshotCursor, PaneSnapshotRequest, PaneSnapshotResponse,
    Response, RmuxError,
};

use super::super::RequestHandler;
use crate::pane_terminal_lookup::pane_id_for_target;

/// Saturating cast for cursor coordinates emitted by `Screen::cursor_position`.
///
/// `Screen` stores the cursor as `u32` while the wire protocol uses `u16`. A
/// well-formed pane keeps the cursor inside `u16` bounds, but a defensive
/// saturating cast guarantees that pathological screen state cannot produce a
/// silently-truncated cursor coordinate on the wire.
fn cursor_coord_to_u16(value: u32) -> u16 {
    if value > u16::MAX as u32 {
        u16::MAX
    } else {
        value as u16
    }
}

impl RequestHandler {
    pub(in crate::handler) async fn handle_pane_snapshot(
        &self,
        request: PaneSnapshotRequest,
    ) -> Response {
        let state = self.state.lock().await;
        let target = &request.target;
        let pane_id = match pane_id_for_target(
            &state.sessions,
            target.session_name(),
            target.window_index(),
            target.pane_index(),
        ) {
            Ok(pane_id) => pane_id,
            Err(error) => return Response::Error(ErrorResponse { error }),
        };
        let transcript = match state.transcript_handle(target) {
            Ok(transcript) => transcript,
            Err(error) => return Response::Error(ErrorResponse { error }),
        };

        let (cols, rows, cells, cursor, output_sequence, history_size, history_bytes) = {
            let transcript = transcript
                .lock()
                .expect("pane transcript mutex must not be poisoned");
            let screen = transcript.clone_screen();
            let size = screen.size();
            let cols = size.cols;
            let rows = size.rows;
            let history_size = screen.history_size();
            let history_bytes = screen.history_bytes();
            let (cursor_x, cursor_y) = screen.cursor_position();
            let cursor_visible = (screen.mode() & mode::MODE_CURSOR) != 0;
            let cursor = PaneSnapshotCursor {
                row: cursor_coord_to_u16(cursor_y),
                col: cursor_coord_to_u16(cursor_x),
                visible: cursor_visible,
                style: screen.cursor_style(),
            };
            let output_sequence = transcript.output_sequence();

            let cells = match collect_cells(&screen, cols, rows, history_size) {
                Ok(cells) => cells,
                Err(error) => return Response::Error(ErrorResponse { error }),
            };
            (
                cols,
                rows,
                cells,
                cursor,
                output_sequence,
                history_size,
                history_bytes,
            )
        };

        let revision = compute_revision(
            cols,
            rows,
            &cells,
            &cursor,
            output_sequence,
            history_size,
            history_bytes,
            pane_id.as_u32(),
        );

        Response::PaneSnapshot(PaneSnapshotResponse {
            cols,
            rows,
            cells,
            cursor,
            revision,
        })
    }
}

fn collect_cells(
    screen: &rmux_core::Screen,
    cols: u16,
    rows: u16,
    history_size: usize,
) -> Result<Vec<PaneSnapshotCell>, RmuxError> {
    let cols_usize = usize::from(cols);
    let rows_usize = usize::from(rows);
    let total = cols_usize.saturating_mul(rows_usize);
    let mut cells = Vec::with_capacity(total);
    if cols_usize == 0 || rows_usize == 0 {
        return Ok(cells);
    }

    for row in 0..rows_usize {
        let line = screen.absolute_line_view(history_size + row);
        let mut row_cells = match line {
            Some(line) => line
                .cells()
                .iter()
                .take(cols_usize)
                .map(|cell| PaneSnapshotCell {
                    text: cell.text().to_owned(),
                    width: cell.width(),
                    padding: cell.is_padding(),
                    attributes: cell.attr(),
                    fg: cell.fg(),
                    bg: cell.bg(),
                    us: cell.us(),
                    link: cell.link(),
                })
                .collect::<Vec<_>>(),
            None => Vec::new(),
        };
        // The screen library normally clips at `cols`, but a misconfigured or
        // future grid backend could hand us a row that does not. Truncate so
        // the on-the-wire row length is invariant: exactly `cols` cells.
        if row_cells.len() > cols_usize {
            row_cells.truncate(cols_usize);
        }
        while row_cells.len() < cols_usize {
            row_cells.push(blank_cell());
        }
        cells.extend(row_cells);
    }

    Ok(cells)
}

fn blank_cell() -> PaneSnapshotCell {
    PaneSnapshotCell {
        text: " ".to_owned(),
        width: 1,
        padding: false,
        attributes: 0,
        fg: 8,
        bg: 8,
        us: 8,
        link: 0,
    }
}

#[allow(clippy::too_many_arguments)]
fn compute_revision(
    cols: u16,
    rows: u16,
    cells: &[PaneSnapshotCell],
    cursor: &PaneSnapshotCursor,
    output_sequence: u64,
    history_size: usize,
    history_bytes: usize,
    pane_id_value: u32,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    cols.hash(&mut hasher);
    rows.hash(&mut hasher);
    cursor.row.hash(&mut hasher);
    cursor.col.hash(&mut hasher);
    cursor.visible.hash(&mut hasher);
    cursor.style.hash(&mut hasher);
    for cell in cells {
        cell.text.hash(&mut hasher);
        cell.width.hash(&mut hasher);
        cell.padding.hash(&mut hasher);
        cell.attributes.hash(&mut hasher);
        cell.fg.hash(&mut hasher);
        cell.bg.hash(&mut hasher);
        cell.us.hash(&mut hasher);
        cell.link.hash(&mut hasher);
    }
    output_sequence.hash(&mut hasher);
    history_size.hash(&mut hasher);
    history_bytes.hash(&mut hasher);
    pane_id_value.hash(&mut hasher);
    let raw = hasher.finish();
    if raw == 0 {
        0xFFFF_FFFF_FFFF_FFFF
    } else {
        raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use rmux_core::{Screen, TerminalScreen};
    use rmux_proto::TerminalSize;

    fn screen_with_size(cols: u16, rows: u16) -> Screen {
        Screen::new(TerminalSize { cols, rows }, 0)
    }

    fn snapshot_cursor(row: u16, col: u16) -> PaneSnapshotCursor {
        PaneSnapshotCursor {
            row,
            col,
            visible: true,
            style: 0,
        }
    }

    fn baseline_cell() -> PaneSnapshotCell {
        PaneSnapshotCell {
            text: "x".to_owned(),
            width: 1,
            padding: false,
            attributes: 0,
            fg: 8,
            bg: 8,
            us: 8,
            link: 0,
        }
    }

    #[test]
    fn cursor_coord_to_u16_clamps_extreme_values() {
        assert_eq!(cursor_coord_to_u16(0), 0);
        assert_eq!(cursor_coord_to_u16(80), 80);
        assert_eq!(cursor_coord_to_u16(u16::MAX as u32), u16::MAX);
        // Pathological cursor coordinates from a misbehaving backend must
        // saturate rather than silently truncate via `as u16` wrap-around.
        assert_eq!(cursor_coord_to_u16(u16::MAX as u32 + 1), u16::MAX);
        assert_eq!(cursor_coord_to_u16(u32::MAX), u16::MAX);
    }

    #[test]
    fn collect_cells_returns_empty_vec_when_either_dim_is_zero() {
        let screen = screen_with_size(0, 4);
        let cells = collect_cells(&screen, 0, 4, 0).expect("zero cols ok");
        assert!(cells.is_empty());

        let screen = screen_with_size(4, 0);
        let cells = collect_cells(&screen, 4, 0, 0).expect("zero rows ok");
        assert!(cells.is_empty());
    }

    #[test]
    fn collect_cells_pads_short_rows_to_exactly_cols_blank_cells() {
        // `screen_with_size` produces a clean grid where every row has exactly
        // `cols` cells, so the fallback we are validating is purely defensive
        // for any future grid backend that could hand us short rows. The
        // captured row count must always equal `rows * cols`.
        let screen = screen_with_size(4, 2);
        let cells = collect_cells(&screen, 4, 2, 0).expect("collect ok");
        assert_eq!(cells.len(), 8);
        for cell in &cells {
            // Default cells are blank single-width spaces with default colors.
            assert!(!cell.padding);
            assert_eq!(cell.width, 1);
        }
    }

    #[test]
    fn collect_cells_preserves_padding_metadata_for_wide_cells() {
        // Feed a wide glyph through the core terminal boundary into a Screen.
        let mut terminal = TerminalScreen::new(TerminalSize { cols: 4, rows: 1 }, 0);
        terminal.feed("界x".as_bytes());
        let screen = terminal.screen().clone();
        let cells = collect_cells(&screen, 4, 1, 0).expect("collect ok");
        assert_eq!(cells.len(), 4);
        assert!(!cells[0].padding);
        assert_eq!(cells[0].text, "界");
        assert_eq!(cells[0].width, 2);
        // The trailing padding column carries width 0 and the padding flag,
        // matching the rmux-core grid contract.
        assert!(cells[1].padding);
        assert_eq!(cells[1].width, 0);
        assert!(!cells[2].padding);
        assert_eq!(cells[2].text, "x");
    }

    #[test]
    fn collect_cells_skips_history_offset_and_returns_visible_rows() {
        // Pre-fill the screen with two rows of content via the parser, then
        // verify that the visible row offset stays correct as `history_size`
        // advances. With a zero history limit `history_size` stays at zero
        // here, but the function must not panic for non-zero offsets either.
        let mut terminal = TerminalScreen::new(TerminalSize { cols: 4, rows: 2 }, 0);
        terminal.feed(b"abcd\r\nefgh");
        let screen = terminal.screen().clone();
        let cells = collect_cells(&screen, 4, 2, 0).expect("collect ok");
        assert_eq!(cells.len(), 8);
        let row0_text: String = cells[0..4].iter().map(|c| c.text.as_str()).collect();
        let row1_text: String = cells[4..8].iter().map(|c| c.text.as_str()).collect();
        assert_eq!(row0_text, "abcd");
        assert_eq!(row1_text, "efgh");
    }

    #[test]
    fn compute_revision_is_never_zero_for_default_inputs() {
        let cursor = snapshot_cursor(0, 0);
        let revision = compute_revision(0, 0, &[], &cursor, 0, 0, 0, 0);
        assert_ne!(revision, 0);
    }

    #[test]
    fn compute_revision_changes_with_each_observable_field() {
        let cursor = snapshot_cursor(0, 0);
        let baseline = compute_revision(80, 24, &[], &cursor, 0, 0, 0, 1);

        // Each observable input must influence the revision. We do not assert
        // exact deltas (which would couple to the hash internals); only that
        // the revision value moves when one input changes.
        assert_ne!(baseline, compute_revision(81, 24, &[], &cursor, 0, 0, 0, 1));
        assert_ne!(baseline, compute_revision(80, 25, &[], &cursor, 0, 0, 0, 1));
        assert_ne!(baseline, compute_revision(80, 24, &[], &cursor, 1, 0, 0, 1));
        assert_ne!(baseline, compute_revision(80, 24, &[], &cursor, 0, 1, 0, 1));
        assert_ne!(baseline, compute_revision(80, 24, &[], &cursor, 0, 0, 1, 1));
        assert_ne!(baseline, compute_revision(80, 24, &[], &cursor, 0, 0, 0, 2));
        assert_ne!(
            baseline,
            compute_revision(80, 24, &[], &snapshot_cursor(1, 0), 0, 0, 0, 1)
        );
        assert_ne!(
            baseline,
            compute_revision(80, 24, &[baseline_cell()], &cursor, 0, 0, 0, 1)
        );
    }

    #[test]
    fn compute_revision_is_stable_for_identical_inputs() {
        // The revision is hashed; for two captures of the exact same observable
        // state, the revision must compare equal so consumers can use it as a
        // "did anything change?" signal without spurious mismatches.
        let cursor = snapshot_cursor(2, 5);
        let cells = vec![baseline_cell(); 4];
        let a = compute_revision(80, 24, &cells, &cursor, 7, 1, 100, 9);
        let b = compute_revision(80, 24, &cells, &cursor, 7, 1, 100, 9);
        assert_eq!(a, b);
    }
}
