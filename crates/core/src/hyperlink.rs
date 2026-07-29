//! Purpose: OSC 8 hyperlinks -- which link, if any, newly printed cells belong to.
//! Public surface (crate): `State::osc_hyperlink`, `State::stamp_link`.
//! Why this file: the rules are small but live in three places at once -- the OSC that
//!   sets the cursor's link, the print path that stamps cells, and the grid relocation
//!   that keeps a stamp with its cell through scrolls. `terminal.rs` holds the print
//!   path; the parsing and interning live here.
//! Reference: the oracle implements OSC 8 (`../ruuah/src/terminal/osc.zig`
//!   `hyperlink_start`/`hyperlink_end`, storage in the pages) but exposes NONE of it
//!   through the libghostty-vt ABI -- there is no readable hyperlink surface at all, so
//!   the differential harness cannot see this feature and the reference is the oracle's
//!   source plus these unit tests (the scrollback-policy precedent).
//! V1 boundaries, documented not hidden: links live on the active grid only (rows
//!   scrolled into history drop them); reflow does not carry them; DECSC does not save
//!   the cursor link; the table caps at 255 distinct links and further NEW links are
//!   dropped (cells keep printing, unlinked).

use crate::terminal::State;

/// Table indices must fit the 8 bits the packed cell spends on them, minus the 0 = none
/// sentinel the frame layer uses.
const MAX_LINKS: usize = 255;

impl State {
    /// OSC 8 ; params ; URI -- start (non-empty URI) or end (empty URI) a hyperlink.
    ///
    /// vte has already split on every `;`, but the URI itself may contain them, so
    /// everything from the third field on is rejoined verbatim. The params field is
    /// `:`-separated key=value pairs of which only `id=` matters (VTE extension); two
    /// links are the same entry only when explicit id AND uri both match.
    pub(crate) fn osc_hyperlink(&mut self, params: &[&[u8]]) {
        let explicit_id = params
            .get(1)
            .and_then(|field| {
                field
                    .split(|&byte| byte == b':')
                    .find_map(|pair| pair.strip_prefix(b"id="))
            })
            .map(|id| String::from_utf8_lossy(id).into_owned())
            .unwrap_or_default();

        let uri = match params.get(2..) {
            Some(rest) if !rest.is_empty() => {
                let mut uri = Vec::new();
                for (position, field) in rest.iter().enumerate() {
                    if position > 0 {
                        uri.push(b';');
                    }
                    uri.extend_from_slice(field);
                }
                String::from_utf8_lossy(&uri).into_owned()
            }
            _ => String::new(),
        };

        if uri.is_empty() {
            self.cursor_link = None;
            return;
        }

        let existing = self
            .link_table
            .iter()
            .position(|(id, table_uri)| *id == explicit_id && *table_uri == uri);
        self.cursor_link = match existing {
            Some(index) => Some(index as u16),
            None if self.link_table.len() < MAX_LINKS => {
                self.link_table.push((explicit_id, uri));
                Some((self.link_table.len() - 1) as u16)
            }
            // Table full: the print keeps printing, the new link is dropped. Loud in the
            // module card, silent at runtime -- exactly like upstream's allocator caps.
            None => None,
        };
    }

    /// Stamps the cell just written with the cursor's link, if one is open.
    pub(crate) fn stamp_link(&mut self, index: usize) {
        if let Some(link) = self.cursor_link {
            self.screen_mut().grid.set_link(index, link);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::terminal::Terminal;

    /// The uri of the link stamped on the cell at (x, y), if any.
    fn link_at(terminal: &Terminal, x: u16, y: u16) -> Option<String> {
        let grid = &terminal.screen().grid;
        let id = grid.link_id(grid.index(x, y))?;
        terminal.link_uri(id).map(str::to_owned)
    }

    #[test]
    fn printed_cells_wear_the_open_link_until_it_closes() {
        let mut terminal = Terminal::new(20, 2);
        terminal.write(b"a\x1b]8;;https://x.il\x07bc\x1b]8;;\x07d");
        assert_eq!(link_at(&terminal, 0, 0), None, "printed before the link");
        assert_eq!(link_at(&terminal, 1, 0), Some("https://x.il".into()));
        assert_eq!(link_at(&terminal, 2, 0), Some("https://x.il".into()));
        assert_eq!(link_at(&terminal, 3, 0), None, "printed after the close");
    }

    #[test]
    fn a_uri_containing_semicolons_is_rejoined() {
        let mut terminal = Terminal::new(40, 1);
        terminal.write(b"\x1b]8;;https://x.il/a;b=1;c=2\x07z");
        assert_eq!(link_at(&terminal, 0, 0), Some("https://x.il/a;b=1;c=2".into()));
    }

    #[test]
    fn the_same_id_and_uri_intern_to_one_entry() {
        let mut terminal = Terminal::new(20, 1);
        terminal.write(b"\x1b]8;id=k;https://x.il\x07a\x1b]8;;\x07");
        terminal.write(b"\x1b]8;id=k;https://x.il\x07b");
        let grid = &terminal.screen().grid;
        let first = grid.link_id(grid.index(0, 0)).expect("a linked");
        let second = grid.link_id(grid.index(1, 0)).expect("b linked");
        assert_eq!(first, second, "same (id, uri) is one identity");
    }

    #[test]
    fn overwriting_a_linked_cell_clears_the_stamp() {
        let mut terminal = Terminal::new(20, 1);
        terminal.write(b"\x1b]8;;https://x.il\x07abc\x1b]8;;\x07\x1b[1;2Hx");
        assert_eq!(link_at(&terminal, 0, 0), Some("https://x.il".into()));
        assert_eq!(link_at(&terminal, 1, 0), None, "overwritten without a link");
        assert_eq!(link_at(&terminal, 2, 0), Some("https://x.il".into()));
    }

    /// A row that moved WITHIN the screen carries its links (relocation moves the
    /// stamps with the cells). Rows that scroll off the top drop theirs -- the
    /// documented v1 boundary, not asserted here because it is a non-guarantee.
    #[test]
    fn a_row_scrolled_up_inside_the_screen_keeps_its_links() {
        let mut terminal = Terminal::new(20, 3);
        terminal.write(b"\r\n\x1b]8;;https://y.il\x07q\x1b]8;;\x07"); // row 1
        terminal.write(b"\r\n\r\n"); // cursor to the last row, then one scroll
        assert_eq!(
            link_at(&terminal, 0, 0),
            Some("https://y.il".into()),
            "the linked row moved from row 1 to row 0 with its stamp"
        );
    }

    #[test]
    fn a_wide_glyph_links_head_and_tail() {
        let mut terminal = Terminal::new(20, 1);
        terminal.write("\x1b]8;;https://x.il\x07🧿".as_bytes());
        assert_eq!(link_at(&terminal, 0, 0), Some("https://x.il".into()));
        assert_eq!(link_at(&terminal, 1, 0), Some("https://x.il".into()), "spacer tail too");
    }
}

#[cfg(test)]
mod resize_tests {
    use crate::terminal::Terminal;

    fn link_at(terminal: &Terminal, x: u16, y: u16) -> Option<String> {
        let grid = &terminal.screen().grid;
        let id = grid.link_id(grid.index(x, y))?;
        terminal.link_uri(id).map(str::to_owned)
    }

    /// Found live 2026-07-30: a resized window silently lost every link stamp, so
    /// cmd+click died exactly when a human drove the window (SCAR-014's shape). The
    /// stamps now ride HistoryCell through the drain-reflow-write round trip the same
    /// way grapheme continuations do.
    #[test]
    fn links_survive_a_resize_in_both_directions() {
        let mut terminal = Terminal::new(20, 3);
        terminal.write(b"pre \x1b]8;;https://x.il\x07LINK\x1b]8;;\x07 post");
        assert_eq!(link_at(&terminal, 4, 0), Some("https://x.il".into()));

        terminal.resize(30, 3);
        assert_eq!(
            link_at(&terminal, 4, 0),
            Some("https://x.il".into()),
            "wider: the row did not move and the stamp must still be there"
        );
        assert_eq!(link_at(&terminal, 0, 0), None, "unlinked cells stay unlinked");

        // Narrow enough to force a reflow split through the linked span.
        terminal.resize(6, 4);
        let grid = &terminal.screen().grid;
        let mut found = 0;
        for y in 0..4u16 {
            for x in 0..6u16 {
                if grid.link_id(grid.index(x, y)).is_some() {
                    found += 1;
                }
            }
        }
        assert_eq!(
            found, 4,
            "narrower: all four LINK cells wear the stamp wherever reflow put them"
        );
    }
}
