//! The predictor against the real compiler.
//!
//! A prediction is allowed to be wrong when Typst decides to break the
//! paragraph differently; that is the price of not compiling. It must be
//! right about lines it understands, and it must leave a page alone when
//! nothing was typed. The thresholds below come from the M0 measurements
//! (`zetwerk-spikes predict`) with headroom; a drop means a regression.

use std::sync::Arc;

use typst::layout::Frame;
use typst::syntax::FileId;
use typst_layout::Page;
use zetwerk_predict::{Insertion, predict_insertion};
use zetwerk_typst::{Environment, FontSet, Origin, PageMap, Project};

const SCALE: f64 = 2.0;

fn open(text: &str) -> Project {
    let env = Environment::new(FontSet::Embedded);
    Project::in_memory(
        Arc::clone(&env),
        std::env::temp_dir(),
        "main.typ",
        text.to_owned(),
    )
    .unwrap_or_else(|| panic!("valid main file name"))
}

fn compile(project: &Project) -> typst_layout::PagedDocument {
    zetwerk_typst::compile(&project.snapshot())
        .document
        .unwrap_or_else(|| panic!("corpus document compiles"))
}

fn same_pixels(page: &Page, frame: Frame) -> bool {
    let other = Page {
        frame,
        ..page.clone()
    };
    zetwerk_render::render_page(page, SCALE).data()
        == zetwerk_render::render_page(&other, SCALE).data()
}

/// Offsets just after a letter of running prose, spread over the page.
fn prose_offsets(project: &Project, main: FileId, map: &PageMap, every: usize) -> Vec<usize> {
    let Some(source) = project.source(main) else {
        return Vec::new();
    };
    map.glyphs()
        .iter()
        .step_by(every)
        .filter_map(|glyph| match &glyph.origin {
            Origin::Text { file, range } if *file == main => Some(range.end),
            _ => None,
        })
        .filter(|&end| {
            source
                .text()
                .get(..end)
                .and_then(|before| before.chars().next_back())
                .is_some_and(|c| c.is_ascii_alphabetic())
        })
        .collect()
}

/// Typing nothing must give back the page exactly: this checks that the
/// predictor's model of a line (shaping, justification, positions)
/// reproduces what Typst produced.
fn assert_empty_insertion_is_identity(text: &str) {
    let project = open(text);
    let main = project.main();
    let world = project.snapshot();
    let document = compile(&project);
    let mut predicted = 0;
    for page in document.pages() {
        let map = PageMap::build(&world, &page.frame);
        for offset in prose_offsets(&project, main, &map, 23) {
            let insertion = Insertion {
                file: main,
                offset,
                text: "",
            };
            if let Ok(prediction) = predict_insertion(&page.frame, &map, insertion) {
                predicted += 1;
                assert!(
                    same_pixels(page, prediction.frame),
                    "an empty insertion at byte {offset} changed the page"
                );
            }
        }
    }
    assert!(predicted > 20, "only {predicted} positions were predicted");
}

#[test]
fn empty_insertion_is_identity_on_ragged_text() {
    assert_empty_insertion_is_identity(zetwerk_corpus::PAPER);
}

#[test]
fn empty_insertion_is_identity_on_justified_text() {
    assert_empty_insertion_is_identity(zetwerk_corpus::ARTICLE);
}

/// Types one letter at many positions and compares each prediction with the
/// real compile. Returns (predicted, pixel-identical, declined).
fn type_letters(text: &str) -> (usize, usize, usize) {
    let mut project = open(text);
    let main = project.main();
    let world = project.snapshot();
    let document = compile(&project);
    let (mut predicted, mut identical, mut declined) = (0, 0, 0);
    for (index, page) in document.pages().iter().enumerate() {
        let map = PageMap::build(&world, &page.frame);
        for (nth, offset) in prose_offsets(&project, main, &map, 31)
            .into_iter()
            .enumerate()
        {
            let typed = char::from(b'a' + (nth % 26) as u8).to_string();
            let insertion = Insertion {
                file: main,
                offset,
                text: &typed,
            };
            let prediction = predict_insertion(&page.frame, &map, insertion);

            project.edit(main, offset..offset, &typed);
            let truth = compile(&project);
            project.edit(main, offset..offset + typed.len(), "");

            match (prediction, truth.pages().get(index)) {
                (Ok(prediction), Some(real)) => {
                    predicted += 1;
                    identical += usize::from(same_pixels(real, prediction.frame));
                }
                _ => declined += 1,
            }
        }
    }
    (predicted, identical, declined)
}

#[test]
fn ragged_text_is_predicted_exactly() {
    let (predicted, identical, declined) = type_letters(zetwerk_corpus::PAPER);
    // The paper is full of headings, list items and centred lines, which
    // level A declines. Measured: 37 predicted, 15 declined.
    assert!(
        predicted >= 2 * declined,
        "predicted {predicted}, declined {declined}"
    );
    // Ragged lines only change when a word no longer fits, which the
    // predictor declines. Measured: 99 %.
    assert!(
        identical * 100 >= predicted * 95,
        "{identical} of {predicted} predictions matched the real compile"
    );
}

#[test]
fn justified_text_is_predicted_unless_typst_rebreaks() {
    let (predicted, identical, declined) = type_letters(zetwerk_corpus::ARTICLE);
    assert!(
        predicted >= 5 * declined,
        "predicted {predicted}, declined {declined}"
    );
    // Typst's optimal line breaking re-breaks a justified paragraph on
    // roughly one keystroke in six; those predictions are legitimately
    // wrong until the next compile. Measured: 82 % identical.
    assert!(
        identical * 100 >= predicted * 70,
        "{identical} of {predicted} predictions matched the real compile"
    );
}
