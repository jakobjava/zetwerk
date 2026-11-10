//! Region rendering must agree with full-page rendering: the tile cache
//! shows regions, exports and MCP screenshots show full pages, and the two
//! must not disagree about what the page looks like.

use std::sync::Arc;

use tiny_skia::Pixmap;
use typst_layout::PagedDocument;
use zetwerk_render::{PixelRegion, render_page, render_region};
use zetwerk_typst::{Environment, FontSet, Project};

const TILE: u32 = 256;

fn compile(text: &str) -> PagedDocument {
    let env = Environment::new(FontSet::Embedded);
    let project = Project::in_memory(
        Arc::clone(&env),
        std::env::temp_dir(),
        "main.typ",
        text.to_owned(),
    )
    .unwrap_or_else(|| panic!("valid main file name"));
    zetwerk_typst::compile(&project.snapshot())
        .document
        .unwrap_or_else(|| panic!("corpus document compiles"))
}

/// Renders `page` tile by tile and stitches the tiles together.
fn stitched(page: &typst_layout::Page, scale: f64, like: &Pixmap) -> Pixmap {
    let mut out =
        Pixmap::new(like.width(), like.height()).unwrap_or_else(|| panic!("non-empty page"));
    let stride = like.width() as usize * 4;
    for y in (0..like.height()).step_by(TILE as usize) {
        for x in (0..like.width()).step_by(TILE as usize) {
            let region = PixelRegion {
                x,
                y,
                width: TILE.min(like.width() - x),
                height: TILE.min(like.height() - y),
            };
            let tile = render_region(page, region, scale);
            assert_eq!((tile.width(), tile.height()), (region.width, region.height));
            for row in 0..region.height as usize {
                let src = row * region.width as usize * 4;
                let dst = (y as usize + row) * stride + x as usize * 4;
                let len = region.width as usize * 4;
                out.data_mut()[dst..dst + len].copy_from_slice(&tile.data()[src..src + len]);
            }
        }
    }
    out
}

/// Number of pixels that differ and the largest per-channel difference.
fn difference(a: &Pixmap, b: &Pixmap) -> (usize, u8) {
    let mut count = 0;
    let mut largest = 0;
    let (a, _) = a.data().as_chunks::<4>();
    let (b, _) = b.data().as_chunks::<4>();
    for (p, q) in a.iter().zip(b) {
        let delta = p
            .iter()
            .zip(q)
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0);
        if delta > 0 {
            count += 1;
            largest = largest.max(delta);
        }
    }
    (count, largest)
}

fn assert_tiles_match(text: &str, scale: f64) {
    let document = compile(text);
    for (index, page) in document.pages().iter().enumerate() {
        let full = render_page(page, scale);
        let tiles = stitched(page, scale, &full);
        let (count, largest) = difference(&full, &tiles);
        // Typst rasterizes each glyph at its sub-pixel position, computed in
        // `f32` from the glyph's distance to the canvas origin. A tile has a
        // different origin, so that position can round differently in its
        // last bits and shift an anti-aliased edge by one level out of 255.
        // Anything larger means content is missing or misplaced.
        assert!(
            largest <= 1,
            "page {} at {scale} px/pt: {count} pixels differ, by up to {largest}",
            index + 1
        );
    }
}

#[test]
fn paper_tiles_match_full_render() {
    assert_tiles_match(zetwerk_corpus::PAPER, 2.0);
}

#[test]
fn article_tiles_match_full_render() {
    assert_tiles_match(zetwerk_corpus::ARTICLE, 2.0);
}

#[test]
fn tiles_match_at_fractional_scale() {
    assert_tiles_match(zetwerk_corpus::ARTICLE, 1.4);
}
