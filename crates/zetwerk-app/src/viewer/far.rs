//! What is kept of pages far from the view and the cursor.
//!
//! The map of a page (where its glyphs are, for the cursor, the search,
//! the frames), its lines and the tiles planned of it (each with the
//! page cut down to it) are made when the page is first drawn, and were
//! kept for as long as the pages stay: scrolling through 312 pages kept
//! all of them, some 350 MB. Of pages that are neither in view nor near
//! the cursor they are forgotten, once there are more than a few; they
//! are made again if those pages come back into view.

use super::Viewer;

/// How many pages keep their map and lines before those far away are
/// forgotten.
const KEPT: usize = 24;

/// How many tiles are planned before those of pages far away are
/// forgotten: those of a few screens.
const TILES_KEPT: usize = 8 * KEPT;

/// How many pages before and behind those in view, and around the
/// cursor's, keep theirs.
const NEAR: usize = 4;

impl Viewer {
    /// Forgets the maps, lines and planned tiles of pages far from those
    /// in view and from the cursor's, where more than [`KEPT`] pages
    /// have them (or [`TILES_KEPT`] tiles are planned).
    pub(super) fn forget_far_pages(&mut self) {
        if self.maps.len() <= KEPT
            && self.page_lines.get_mut().len() <= KEPT
            && self.planned.len() <= TILES_KEPT
        {
            return;
        }
        let visible = self.view.visible(&self.layout);
        let cursor = self.cursor_page.get();
        let near = |index: &usize| {
            let close = |page: usize| index.abs_diff(page) <= NEAR;
            close(cursor) || visible.iter().any(|visible| close(visible.index))
        };
        self.maps.retain(|index, _| near(index));
        self.page_lines.get_mut().retain(|index, _| near(index));
        self.planned.retain(|(index, _, _), _| near(index));
    }
}
