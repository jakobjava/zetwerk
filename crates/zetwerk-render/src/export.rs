//! Pages, and parts of them, as the bytes of image files.
//!
//! The pixels are those of `typst-render` and the vectors those of
//! `typst-svg`, so an exported page is the page Typst's own export would
//! give. A part of a page is the page cropped to it: everything the page
//! draws there, on the page's own background.

use std::panic::{AssertUnwindSafe, catch_unwind};

use typst::layout::Rect;
use typst_layout::Page;
use typst_svg::SvgOptions;

use crate::{cull, page_size_px, render_page};

/// More pixels than this are not made of one page or part of one: at
/// four bytes each they are a quarter of a gigabyte while the file is
/// written.
pub const MAX_PIXELS: u64 = 64_000_000;

/// `page`, or with `area` that part of it, as a PNG file with
/// `pixel_per_pt` pixels to the typographic point (72 of which are an
/// inch).
///
/// Never panics: what goes wrong in Typst's renderer is said.
pub fn png(page: &Page, area: Option<Rect>, pixel_per_pt: f64) -> Result<Vec<u8>, String> {
    let page = part(page, area);
    let (width, height) = page_size_px(&page, pixel_per_pt);
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(format!(
            "{width}×{height} pixels are more than one image may have; ask for fewer per inch"
        ));
    }
    let rendered = catch_unwind(AssertUnwindSafe(|| {
        render_page(&page, pixel_per_pt).encode_png()
    }));
    match rendered {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(err)) => Err(format!("the image could not be encoded: {err}")),
        Err(_) => Err("the renderer crashed on this page".to_owned()),
    }
}

/// `page`, or with `area` that part of it, as an SVG file. Its size is
/// that of the page or the part, in points.
///
/// Never panics, like [`png`].
pub fn svg(page: &Page, area: Option<Rect>) -> Result<String, String> {
    let page = part(page, area);
    catch_unwind(AssertUnwindSafe(|| {
        typst_svg::svg(&page, &SvgOptions::default())
    }))
    .map_err(|_| "the SVG exporter crashed on this page".to_owned())
}

/// The page that is `area` of `page`; `page` itself where no part of it
/// is asked for.
fn part(page: &Page, area: Option<Rect>) -> Page {
    match area {
        Some(area) => Page {
            frame: cull::crop(&page.frame, area),
            ..page.clone()
        },
        None => page.clone(),
    }
}
