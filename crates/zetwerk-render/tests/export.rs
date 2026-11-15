//! Pages and parts of pages as image files: what is written is the page,
//! and a part is that part and no more.

use std::sync::Arc;

use tiny_skia::Pixmap;
use typst::layout::{Abs, Point, Rect};
use typst_layout::PagedDocument;
use zetwerk_render::{export, render_page};
use zetwerk_typst::{Environment, FontSet, Project};

const TEXT: &str = "#set page(width: 200pt, height: 100pt, margin: 0pt, fill: yellow)\n\
                    #place(top + left, dx: 50pt, dy: 20pt, rect(width: 40pt, height: 30pt, fill: blue))\n";

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
        .unwrap_or_else(|| panic!("the document compiles"))
}

fn decode(png: &[u8]) -> Pixmap {
    Pixmap::decode_png(png).unwrap_or_else(|err| panic!("a PNG: {err}"))
}

fn pixel(pixmap: &Pixmap, x: u32, y: u32) -> (u8, u8, u8) {
    let color = pixmap
        .pixel(x, y)
        .unwrap_or_else(|| panic!("a pixel at {x}, {y}"));
    (color.red(), color.green(), color.blue())
}

fn area(x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect::new(
        Point::new(Abs::pt(x), Abs::pt(y)),
        Point::new(Abs::pt(x + width), Abs::pt(y + height)),
    )
}

#[test]
fn a_page_as_png_is_the_page_as_rendered() {
    let document = compile(TEXT);
    let page = &document.pages()[0];
    let png = export::png(page, None, 2.0).unwrap_or_else(|err| panic!("{err}"));
    let decoded = decode(&png);
    let rendered = render_page(page, 2.0);
    assert_eq!((decoded.width(), decoded.height()), (400, 200));
    assert_eq!(decoded.data(), rendered.data());
}

#[test]
fn a_part_of_a_page_is_that_part_on_the_pages_background() {
    let document = compile(TEXT);
    let page = &document.pages()[0];
    // The blue box and four points around it.
    let part = area(46.0, 16.0, 48.0, 38.0);
    let png = export::png(page, Some(part), 1.0).unwrap_or_else(|err| panic!("{err}"));
    let decoded = decode(&png);
    assert_eq!((decoded.width(), decoded.height()), (48, 38));
    assert_eq!(pixel(&decoded, 1, 1), (255, 220, 0), "the page's yellow");
    assert_eq!(pixel(&decoded, 24, 19), (0, 116, 217), "the box's blue");
    // And it is the same part of the whole page.
    let whole = render_page(page, 1.0);
    assert_eq!(pixel(&whole, 47, 17), pixel(&decoded, 1, 1));
    assert_eq!(pixel(&whole, 70, 35), pixel(&decoded, 24, 19));
}

#[test]
fn svg_has_the_size_of_what_it_shows() {
    let document = compile(TEXT);
    let page = &document.pages()[0];
    let whole = export::svg(page, None).unwrap_or_else(|err| panic!("{err}"));
    assert!(whole.starts_with("<svg"), "{}", &whole[..40]);
    assert!(whole.contains("viewBox=\"0 0 200 100\""), "{whole}");
    let part =
        export::svg(page, Some(area(46.0, 16.0, 48.0, 38.0))).unwrap_or_else(|err| panic!("{err}"));
    assert!(part.contains("viewBox=\"0 0 48 38\""), "{part}");
    assert!(part.len() < whole.len() + 200);
}

#[test]
fn too_many_pixels_are_refused_not_attempted() {
    let document = compile(TEXT);
    let page = &document.pages()[0];
    let refused = export::png(page, None, 2000.0);
    assert!(
        matches!(&refused, Err(why) if why.contains("more than one image may have")),
        "{refused:?}"
    );
}
