//! Cutting pages into tiles that can be rasterized and cached one by one.
//!
//! A tile is identified by what it shows, not by where it is: its key is a
//! hash of the page content inside it. After an edit, every tile whose
//! content did not change keeps its key, so only the tiles the edit touched
//! are rasterized again. Tiles that look the same (blank margins, say) share
//! a key and are rasterized once.

use tiny_skia::Pixmap;
use typst::utils::hash128;
use typst_layout::Page;

use crate::{PixelRegion, crop_page, options, page_size_px};

/// Edge length of a tile in pixels. Tiles at the right and bottom edge of a
/// page are smaller.
pub const TILE_PX: u32 = 512;

/// Identifies the picture a tile shows. Equal keys mean equal pixels (to
/// within the tolerance documented on [`crate::render_region`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileKey(u128);

/// One tile of a page, ready to be rasterized.
#[derive(Clone)]
pub struct Tile {
    pub key: TileKey,
    /// Where on the page the tile is.
    pub region: PixelRegion,
    /// The page cropped to the tile.
    page: Page,
    pixel_per_pt: f64,
}

impl Tile {
    /// Rasterizes the tile. The result is `region.width` by `region.height`
    /// premultiplied RGBA pixels.
    #[must_use]
    pub fn render(&self) -> Pixmap {
        typst_render::render(&self.page, &options(self.pixel_per_pt))
    }
}

/// The top-left corners of the tiles that intersect `region`, row by row.
/// `region` is in page pixels and must lie within the page.
pub fn tile_origins(region: PixelRegion) -> impl Iterator<Item = (u32, u32)> + Clone {
    let right = region.x.saturating_add(region.width);
    let bottom = region.y.saturating_add(region.height);
    let columns = region.x / TILE_PX..right.div_ceil(TILE_PX);
    let rows = region.y / TILE_PX..bottom.div_ceil(TILE_PX);
    rows.flat_map(move |row| {
        columns
            .clone()
            .map(move |column| (column * TILE_PX, row * TILE_PX))
    })
}

/// The tiles of `page` at `pixel_per_pt` that intersect `visible`, row by
/// row. `visible` is in page pixels and may extend beyond the page.
#[must_use]
pub fn tiles(page: &Page, pixel_per_pt: f64, visible: PixelRegion) -> Vec<Tile> {
    let (page_width, page_height) = page_size_px(page, pixel_per_pt);
    let right = visible.x.saturating_add(visible.width).min(page_width);
    let bottom = visible.y.saturating_add(visible.height).min(page_height);
    if visible.x >= right || visible.y >= bottom {
        return Vec::new();
    }
    let on_page = PixelRegion {
        x: visible.x,
        y: visible.y,
        width: right - visible.x,
        height: bottom - visible.y,
    };
    tile_origins(on_page)
        .map(|(x, y)| {
            let region = PixelRegion {
                x,
                y,
                width: TILE_PX.min(page_width - x),
                height: TILE_PX.min(page_height - y),
            };
            let cropped = crop_page(page, region, pixel_per_pt);
            let key = TileKey(hash128(&(
                &cropped.frame,
                &cropped.fill,
                pixel_per_pt.to_bits(),
                region.width,
                region.height,
            )));
            Tile {
                key,
                region,
                page: cropped,
                pixel_per_pt,
            }
        })
        .collect()
}
