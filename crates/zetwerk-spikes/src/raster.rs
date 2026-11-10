//! Spike 2: what does it cost to put a page, or one changed line, on screen?

use zetwerk_render::PixelRegion;

use crate::support::{self, ms};

/// Pixels per point to measure. On the 1920×1200 development display an A4
/// page is ~1.4 px/pt fitted to the window height, ~1.6 in a half-width
/// tiled window, and ~3.2 fitted to the full width.
const SCALES: [f64; 3] = [1.4, 2.0, 3.2];

/// Height of the region standing in for "the line that changed", in points.
/// About two lines of 11 pt text.
const BAND_PT: f64 = 30.0;

pub fn run() {
    println!("\n== Spike 2: rasterization (typst-render, single thread) ==");
    println!("   cold = glyph cache empty, warm = same content rendered again\n");
    println!(
        "{:<26} {:>6} {:>11} {:>9} {:>9} {:>9} {:>9} {:>10}",
        "page", "px/pt", "size px", "full cold", "full warm", "band cold", "band warm", "band diff"
    );
    let env = support::environment();
    for doc in support::offline_docs().into_iter().take(3) {
        let project = support::project(&env, &doc.text);
        let Some(document) = support::compile(&project) else {
            continue;
        };
        // A page from the middle: body text rather than title or outline.
        let Some(page) = document.pages().get(document.pages().len() / 2) else {
            continue;
        };
        for scale in SCALES {
            comemo::evict(0);
            let (full, full_cold) = support::measure(|| zetwerk_render::render_page(page, scale));
            let (_, full_warm) = support::measure(|| zetwerk_render::render_page(page, scale));

            let region = PixelRegion {
                x: 0,
                y: full.height() / 5,
                width: full.width(),
                height: ((BAND_PT * scale).round() as u32).max(1),
            };
            comemo::evict(0);
            let (band, band_cold) =
                support::measure(|| zetwerk_render::render_region(page, region, scale));
            let (_, band_warm) =
                support::measure(|| zetwerk_render::render_region(page, region, scale));

            println!(
                "{:<26} {:>6.1} {:>11} {:>9.2} {:>9.2} {:>9.2} {:>9.2} {:>10}",
                doc.name,
                scale,
                format!("{}x{}", full.width(), full.height()),
                ms(full_cold.wall),
                ms(full_warm.wall),
                ms(band_cold.wall),
                ms(band_warm.wall),
                difference(&full, &band, region),
            );
        }
    }
    println!("\n   band diff = pixels of the band that differ from the full render");
    println!("               (count, and the largest per-channel difference)");
}

/// How the band render differs from the same rows of the full-page render.
fn difference(
    full: &tiny_skia_pixmap::Pixmap,
    band: &tiny_skia_pixmap::Pixmap,
    region: PixelRegion,
) -> String {
    let row_bytes = full.width() as usize * 4;
    let start = region.y as usize * row_bytes;
    let Some(expected) = full.data().get(start..start + band.data().len()) else {
        return "size mismatch".to_owned();
    };
    let mut differing = 0_usize;
    let mut largest = 0_u8;
    let (expected, _) = expected.as_chunks::<4>();
    let (actual, _) = band.data().as_chunks::<4>();
    for (a, b) in expected.iter().zip(actual) {
        let delta = a
            .iter()
            .zip(b)
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0);
        if delta > 0 {
            differing += 1;
            largest = largest.max(delta);
        }
    }
    format!("{differing} / {largest}")
}

/// `tiny-skia` under the name this module uses it by.
mod tiny_skia_pixmap {
    pub use tiny_skia::Pixmap;
}
