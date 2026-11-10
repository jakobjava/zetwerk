//! Rasterization of compiled Typst pages.
//!
//! All drawing of document content is delegated to `typst-render`, so what
//! zetwerk shows is pixel-identical to Typst's own PNG export. This crate
//! only decides *which part* of a page to draw: a whole page, or a region of
//! it built from a culled copy of the page frame, so that redrawing one
//! changed line does not cost a full page.

mod cull;
mod rasterizer;
mod tiles;

use tiny_skia::Pixmap;
use typst::layout::{Abs, Point, Rect, Size};
use typst::utils::Scalar;
use typst_layout::Page;
use typst_render::RenderOptions;

pub use cull::content_bounds;
pub use rasterizer::{Rasterized, Rasterizer};
pub use tiles::{TILE_PX, Tile, TileKey, tile_origins, tiles};

/// Renders a whole page at `pixel_per_pt` device pixels per typographic point.
#[must_use]
pub fn render_page(page: &Page, pixel_per_pt: f64) -> Pixmap {
    typst_render::render(page, &options(pixel_per_pt))
}

/// A rectangular part of a page, in device pixels.
///
/// Regions are specified in pixels rather than points so that adjacent
/// regions tile exactly, with no seams from rounding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PixelRegion {
    /// Left edge, in pixels from the page's left edge.
    pub x: u32,
    /// Top edge, in pixels from the page's top edge.
    pub y: u32,
    /// Width in pixels. Must be non-zero.
    pub width: u32,
    /// Height in pixels. Must be non-zero.
    pub height: u32,
}

/// Renders only `region` of a page.
///
/// The result matches the same region of a full-page render to within one
/// level (of 255) per channel. The remaining difference comes from Typst
/// positioning each glyph at a sub-pixel offset computed in `f32` relative to
/// the canvas origin, which rounds differently for a different origin.
#[must_use]
pub fn render_region(page: &Page, region: PixelRegion, pixel_per_pt: f64) -> Pixmap {
    typst_render::render(
        &crop_page(page, region, pixel_per_pt),
        &options(pixel_per_pt),
    )
}

/// The size in pixels of a whole-page render at `pixel_per_pt`.
#[must_use]
pub fn page_size_px(page: &Page, pixel_per_pt: f64) -> (u32, u32) {
    // The same rounding `typst-render` applies to its canvas.
    let px = |length: Abs| (pixel_per_pt * length.to_pt()).round().max(1.0) as u32;
    (px(page.frame.width()), px(page.frame.height()))
}

/// A page consisting of just `region` of `page`.
fn crop_page(page: &Page, region: PixelRegion, pixel_per_pt: f64) -> Page {
    let to_pt = |px: u32| Abs::pt(f64::from(px) / pixel_per_pt);
    let origin = Point::new(to_pt(region.x), to_pt(region.y));
    let size = Size::new(to_pt(region.width), to_pt(region.height));
    let frame = cull::crop(&page.frame, Rect::from_pos_size(origin, size));
    Page {
        frame,
        ..page.clone()
    }
}

fn options(pixel_per_pt: f64) -> RenderOptions {
    RenderOptions {
        pixel_per_pt: Scalar::new(pixel_per_pt),
        render_bleed: false,
    }
}
