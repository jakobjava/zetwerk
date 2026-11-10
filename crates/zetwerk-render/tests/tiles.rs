//! Tiles: they must cover the page exactly, and their keys must change
//! exactly where the page changed.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tiny_skia::Pixmap;
use typst_layout::PagedDocument;
use zetwerk_render::{PixelRegion, Rasterizer, TileKey, page_size_px, render_page, tiles};
use zetwerk_typst::{Environment, FontSet, Project};

const SCALE: f64 = 2.0;
const EVERYTHING: PixelRegion = PixelRegion {
    x: 0,
    y: 0,
    width: u32::MAX,
    height: u32::MAX,
};

fn project(text: &str) -> Project {
    let env = Environment::new(FontSet::Embedded);
    Project::in_memory(
        Arc::clone(&env),
        std::env::temp_dir(),
        "main.typ",
        text.to_owned(),
    )
    .unwrap_or_else(|| panic!("valid main file name"))
}

fn compile(project: &Project) -> PagedDocument {
    zetwerk_typst::compile(&project.snapshot())
        .document
        .unwrap_or_else(|| panic!("document compiles"))
}

#[test]
fn tiles_cover_the_page_without_gaps_or_overlap() {
    let document = compile(&project(zetwerk_corpus::ARTICLE));
    let page = &document.pages()[0];
    let (width, height) = page_size_px(page, SCALE);
    let full = render_page(page, SCALE);
    assert_eq!((full.width(), full.height()), (width, height));

    let mut covered = vec![0_u8; (width * height) as usize];
    for tile in tiles(page, SCALE, EVERYTHING) {
        let region = tile.region;
        let pixmap = tile.render();
        assert_eq!(
            (pixmap.width(), pixmap.height()),
            (region.width, region.height)
        );
        for y in region.y..region.y + region.height {
            for x in region.x..region.x + region.width {
                covered[(y * width + x) as usize] += 1;
            }
        }
    }
    assert!(covered.iter().all(|&count| count == 1));
}

#[test]
fn only_tiles_intersecting_the_visible_region_are_returned() {
    let document = compile(&project(zetwerk_corpus::ARTICLE));
    let page = &document.pages()[0];
    let visible = PixelRegion {
        x: 600,
        y: 700,
        width: 100,
        height: 400,
    };
    let regions: Vec<_> = tiles(page, SCALE, visible)
        .iter()
        .map(|tile| tile.region)
        .collect();
    // Columns 1..=1 (x 512..1024), rows 1..=2 (y 512..1536).
    assert_eq!(regions.len(), 2);
    assert!(regions.iter().all(|region| region.x == 512));
    assert_eq!(
        regions.iter().map(|region| region.y).collect::<Vec<_>>(),
        [512, 1024]
    );

    let off_page = PixelRegion {
        x: 0,
        y: 1_000_000,
        width: 100,
        height: 100,
    };
    assert!(tiles(page, SCALE, off_page).is_empty());
}

#[test]
fn an_edit_changes_only_the_tiles_it_touches() {
    let mut project = project(zetwerk_corpus::PAPER);
    let main = project.main();
    let before = compile(&project);

    // Type into a word of the last paragraph: only the end of the last page
    // can change.
    let text = project
        .source(main)
        .map(|source| source.text().to_owned())
        .unwrap_or_default();
    let word = "contracts";
    let end = text
        .rfind(word)
        .unwrap_or_else(|| panic!("word is in the paper"))
        + word.len();
    project.edit(main, end..end, "x");
    let after = compile(&project);
    assert_eq!(before.pages().len(), after.pages().len());

    let keys = |document: &PagedDocument, page: usize| -> Vec<TileKey> {
        tiles(&document.pages()[page], SCALE, EVERYTHING)
            .iter()
            .map(|tile| tile.key)
            .collect()
    };
    assert_eq!(
        keys(&before, 0),
        keys(&after, 0),
        "the first page is untouched"
    );

    let last = before.pages().len() - 1;
    let (old, new) = (keys(&before, last), keys(&after, last));
    let changed = old.iter().zip(&new).filter(|(a, b)| a != b).count();
    assert!(changed >= 1, "the edited line must get a new tile");
    assert!(
        changed <= 2,
        "{changed} of {} tiles changed for a one-letter edit",
        old.len()
    );
}

#[test]
fn identical_tiles_share_a_key() {
    let document = compile(&project("#set page(width: 2000pt, height: 2000pt)\nx"));
    let all = tiles(&document.pages()[0], 1.0, EVERYTHING);
    let distinct: HashSet<_> = all.iter().map(|tile| tile.key).collect();
    // A 4×4 grid of which only the first tile has content; the right and
    // bottom edge tiles are narrower than the rest.
    assert_eq!(all.len(), 16);
    assert!(
        distinct.len() <= 5,
        "{} distinct keys for 15 blank tiles",
        distinct.len()
    );
}

#[test]
fn the_rasterizer_delivers_every_requested_tile_once() {
    let document = compile(&project(zetwerk_corpus::ARTICLE));
    let wanted = tiles(&document.pages()[0], SCALE, EVERYTHING);
    let distinct: HashSet<_> = wanted.iter().map(|tile| tile.key).collect();

    let rasterizer =
        Rasterizer::spawn(3, || {}).unwrap_or_else(|err| panic!("threads spawn: {err}"));
    rasterizer.request(wanted.clone());
    // Asking again while the work is in flight must not duplicate it.
    rasterizer.request(wanted.clone());

    let mut done: Vec<(TileKey, Pixmap)> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !(rasterizer.is_idle() && done.len() >= distinct.len()) {
        assert!(Instant::now() < deadline, "rasterizer did not finish");
        done.extend(
            rasterizer
                .poll()
                .into_iter()
                .map(|tile| (tile.key, tile.pixmap)),
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    done.extend(
        rasterizer
            .poll()
            .into_iter()
            .map(|tile| (tile.key, tile.pixmap)),
    );

    assert_eq!(done.len(), distinct.len());
    for (key, pixmap) in &done {
        let tile = wanted.iter().find(|tile| tile.key == *key);
        let region = tile.map(|tile| tile.region);
        assert_eq!(
            region.map(|r| (r.width, r.height)),
            Some((pixmap.width(), pixmap.height()))
        );
    }
}
