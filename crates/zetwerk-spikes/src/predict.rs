//! Spike 4: how good is predicting a keystroke instead of compiling it?
//!
//! For random positions in running prose, type one character, let the
//! predictor patch the page, then compile for real and compare the two.

use std::collections::BTreeMap;

use typst::layout::Frame;
use typst::syntax::FileId;
use typst_layout::Page;
use zetwerk_predict::{Decline, Insertion, Replacement, predict, predict_insertion};
use zetwerk_typst::{Origin, PageMap, Project};

use crate::support::{self, Doc, ms, percentile};

/// Keystrokes tried per document.
const SAMPLES: usize = 400;

/// Scale at which predicted and real pages are compared pixel by pixel.
const SCALE: f64 = 2.0;

pub fn run() {
    println!("\n== Spike 4: predicting a keystroke (level A: line patch, no re-break) ==");
    println!("   {SAMPLES} random single-character insertions into prose per document\n");
    let env = support::environment();
    let docs = [
        Doc {
            name: "paper (ragged)",
            text: zetwerk_corpus::PAPER.to_owned(),
        },
        Doc {
            name: "article (justified)",
            text: zetwerk_corpus::ARTICLE.to_owned(),
        },
        Doc {
            name: "thesis-5 (justified)",
            text: zetwerk_corpus::thesis(5),
        },
    ];
    for doc in &docs {
        let mut project = support::project(&env, &doc.text);
        match measure(&mut project) {
            Some(stats) => stats.print(doc.name),
            None => println!("{}: could not be measured", doc.name),
        }
    }
    println!(
        "\n   Typing on: {BURSTS} places per document, {} characters typed at each",
        PHRASE.len()
    );
    println!("   (levels A and B: the paragraph is re-broken when a line is full)\n");
    for doc in &docs {
        let mut project = support::project(&env, &doc.text);
        if bursts(&mut project, doc.name).is_none() {
            println!("{}: could not be measured", doc.name);
        }
    }
    println!(
        "\n   Deleting: {BURSTS} places per document, {DELETED} characters deleted backwards at each\n"
    );
    for doc in &docs {
        let mut project = support::project(&env, &doc.text);
        if deletions(&mut project, doc.name).is_none() {
            println!("{}: could not be measured", doc.name);
        }
    }
}

/// Deletes [`DELETED`] characters backwards, one at a time, at random
/// places in prose and reports how often the predictor shows that, and how
/// close the page then is to the compile's.
fn deletions(project: &mut Project, name: &str) -> Option<()> {
    let main = project.main();
    let base = support::compile(project)?;
    let world = project.snapshot();
    let pages = base.pages();
    let maps: Vec<PageMap> = pages
        .iter()
        .map(|page| PageMap::build(&world, &page.frame))
        .collect();
    let mut rng = Rng(0x632B_E59B_D9B4_E019);
    let (mut samples, mut shown, mut complete, mut exact) = (0_usize, 0_usize, 0_usize, 0_usize);
    let mut wrong = Vec::new();
    let mut attempts = 0;
    while samples < BURSTS && attempts < BURSTS * 30 {
        attempts += 1;
        let page_index = rng.below(pages.len());
        let (page, map) = (&pages[page_index], &maps[page_index]);
        let Some((end, _)) = pick_keystroke(project, main, map, &mut rng) else {
            continue;
        };
        let text = project.source(main)?.text().to_owned();
        let Some(start) = end
            .checked_sub(DELETED)
            .filter(|start| text.is_char_boundary(*start))
        else {
            continue;
        };
        // Only what is plain prose on one line of the source.
        if !text[start..end]
            .chars()
            .all(|c| c.is_ascii_alphabetic() || c == ' ')
            || text[start..end].contains("  ")
        {
            continue;
        }
        samples += 1;
        let mut last = None;
        for removed in 1..=DELETED {
            let edit = Replacement {
                file: main,
                offset: end - removed,
                removed,
                text: "",
            };
            if let Ok(prediction) = predict(&page.frame, map, edit) {
                shown += 1;
                last = (removed == DELETED).then_some(prediction);
            }
        }
        let Some(prediction) = last else {
            continue;
        };
        complete += 1;
        project.edit(main, start..end, "")?;
        let truth = support::compile(project);
        project.edit(main, start..start, &text[start..end])?;
        let truth = truth?;
        let Some(true_page) = truth.pages().get(page_index) else {
            continue;
        };
        let off = wrong_pixels(true_page, prediction.frame);
        if off == 0.0 {
            exact += 1;
        } else {
            wrong.push(off);
        }
    }
    let percent = |part: usize, whole: usize| 100.0 * part as f64 / whole.max(1) as f64;
    println!("{name}");
    println!(
        "  deletions shown without a compile    {:5.1} %",
        percent(shown, samples * DELETED)
    );
    println!(
        "  …then the same pixels as the compile {:5.1} %  (else {:.2} % of the page differs, median)",
        percent(exact, complete),
        percentile(&mut wrong, 50.0)
    );
    Some(())
}

/// How many characters are deleted at each place.
const DELETED: usize = 8;

/// For a look at what differs: with `ZETWERK_DUMP=dir`, saves a predicted
/// page next to the compiled one.
fn dump(name: &str, number: usize, predicted: &Frame, real: &Page) {
    let Some(dir) = std::env::var_os("ZETWERK_DUMP").map(std::path::PathBuf::from) else {
        return;
    };
    let ours = Page {
        frame: predicted.clone(),
        ..real.clone()
    };
    let tag = name.split_whitespace().next().unwrap_or("doc");
    for (kind, page) in [("predicted", &ours), ("compiled", real)] {
        if let Ok(png) = zetwerk_render::render_page(page, 1.5).encode_png() {
            let _ = std::fs::write(dir.join(format!("{tag}-{number}-{kind}.png")), png);
        }
    }
}

/// Places tried per document for typing on.
const BURSTS: usize = 120;

/// What is typed at each place, one character at a time.
const PHRASE: &str = "ed and so on it goes for a while";

/// Types [`PHRASE`] at random places in prose and reports how much of it
/// the predictor shows, and how close the page is to the compile's once
/// all of it is typed.
fn bursts(project: &mut Project, name: &str) -> Option<()> {
    let main = project.main();
    let base = support::compile(project)?;
    let world = project.snapshot();
    let pages = base.pages();
    let maps: Vec<PageMap> = pages
        .iter()
        .map(|page| PageMap::build(&world, &page.frame))
        .collect();
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let (mut samples, mut shown, mut complete, mut exact) = (0_usize, 0_usize, 0_usize, 0_usize);
    let mut stopped: BTreeMap<Decline, usize> = BTreeMap::new();
    let mut wrong = Vec::new();
    let mut costs = Vec::new();
    let mut attempts = 0;
    while samples < BURSTS && attempts < BURSTS * 30 {
        attempts += 1;
        let page_index = rng.below(pages.len());
        let (page, map) = (&pages[page_index], &maps[page_index]);
        let Some((offset, _)) = pick_keystroke(project, main, map, &mut rng) else {
            continue;
        };
        samples += 1;
        let mut last = None;
        let mut reason = None;
        for end in 1..=PHRASE.len() {
            let insertion = Insertion {
                file: main,
                offset,
                text: &PHRASE[..end],
            };
            let (prediction, cost) =
                support::measure(|| predict_insertion(&page.frame, map, insertion));
            costs.push(ms(cost.wall));
            match prediction {
                Ok(prediction) => {
                    shown += 1;
                    last = (end == PHRASE.len()).then_some(prediction);
                }
                Err(why) => {
                    reason.get_or_insert(why);
                }
            }
        }
        if let Some(why) = reason {
            *stopped.entry(why).or_default() += 1;
        }
        let Some(prediction) = last else {
            continue;
        };
        complete += 1;
        project.edit(main, offset..offset, PHRASE)?;
        let truth = support::compile(project);
        project.edit(main, offset..offset + PHRASE.len(), "")?;
        let truth = truth?;
        let Some(true_page) = truth.pages().get(page_index) else {
            continue;
        };
        if complete <= 3 {
            dump(name, complete, &prediction.frame, true_page);
        }
        let off = wrong_pixels(true_page, prediction.frame);
        if off == 0.0 {
            exact += 1;
        } else {
            wrong.push(off);
        }
    }
    let percent = |part: usize, whole: usize| 100.0 * part as f64 / whole.max(1) as f64;
    println!("{name}");
    println!(
        "  characters shown without a compile  {:5.1} %",
        percent(shown, samples * PHRASE.len())
    );
    println!(
        "  all {} characters shown              {:5.1} %  of places",
        PHRASE.len(),
        percent(complete, samples)
    );
    println!(
        "  …then the same pixels as the compile {:5.1} %  (else {:.2} % of the page differs, median)",
        percent(exact, complete),
        percentile(&mut wrong, 50.0)
    );
    println!(
        "  a prediction costs                    {:.2} ms (median)",
        percentile(&mut costs, 50.0)
    );
    for (why, count) in stopped {
        println!(
            "  first not shown because: {why:?}  {:.1} % of places",
            percent(count, samples)
        );
    }
    Some(())
}

#[derive(Default)]
struct Stats {
    samples: usize,
    declined: BTreeMap<Decline, usize>,
    predicted: usize,
    /// Predictions whose page is pixel-identical to the real compile.
    exact: usize,
    /// Predictions where the real compile broke the edited line at the same
    /// places as before the edit, which is what level A assumes.
    same_breaks: usize,
    /// Of those, the ones that are pixel-identical to the real compile.
    same_breaks_exact: usize,
    /// Horizontal caret error in points, for predictions with same breaks.
    caret_error: Vec<f64>,
    /// Share of page pixels that differ, for predictions that are not exact.
    wrong_pixels: Vec<f64>,
    predict_ms: Vec<f64>,
    compile_ms: Vec<f64>,
}

impl Stats {
    fn print(mut self, name: &str) {
        let share = |n: usize, of: usize| 100.0 * n as f64 / of.max(1) as f64;
        let declined: usize = self.declined.values().sum();
        println!("{name}");
        println!(
            "  predicted {:>5.1} %   declined {:>5.1} %",
            share(self.predicted, self.samples),
            share(declined, self.samples)
        );
        for (reason, count) in &self.declined {
            println!(
                "    declined {:>5.1} %  {reason:?}",
                share(*count, self.samples)
            );
        }
        println!("  of the predictions:");
        println!(
            "    pixel-identical to the real compile   {:>5.1} %",
            share(self.exact, self.predicted)
        );
        println!(
            "    Typst kept the line's breaks          {:>5.1} %",
            share(self.same_breaks, self.predicted)
        );
        println!(
            "      of those, pixel-identical           {:>5.1} %",
            share(self.same_breaks_exact, self.same_breaks)
        );
        println!(
            "      of those, caret error (pt)          p50 {:.3}  p95 {:.3}  max {:.3}",
            percentile(&mut self.caret_error, 50.0),
            percentile(&mut self.caret_error, 95.0),
            percentile(&mut self.caret_error, 100.0),
        );
        if !self.wrong_pixels.is_empty() {
            println!(
                "    page pixels wrong when not identical  p50 {:.2} %  max {:.2} %",
                percentile(&mut self.wrong_pixels, 50.0),
                percentile(&mut self.wrong_pixels, 100.0),
            );
        }
        println!(
            "  cost per keystroke: predict {:.3} ms   compile {:.1} ms\n",
            percentile(&mut self.predict_ms, 50.0),
            percentile(&mut self.compile_ms, 50.0),
        );
    }
}

fn measure(project: &mut Project) -> Option<Stats> {
    let main = project.main();
    let base = support::compile(project)?;
    let world = project.snapshot();
    let pages = base.pages();
    let maps: Vec<PageMap> = pages
        .iter()
        .map(|page| PageMap::build(&world, &page.frame))
        .collect();

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut stats = Stats::default();
    let mut attempts = 0;
    while stats.samples < SAMPLES && attempts < SAMPLES * 20 {
        attempts += 1;
        let page_index = rng.below(pages.len());
        let (page, map) = (&pages[page_index], &maps[page_index]);
        let Some((offset, typed)) = pick_keystroke(project, main, map, &mut rng) else {
            continue;
        };
        stats.samples += 1;

        let insertion = Insertion {
            file: main,
            offset,
            text: &typed,
        };
        let (prediction, cost) =
            support::measure(|| predict_insertion(&page.frame, map, insertion));
        stats.predict_ms.push(ms(cost.wall));

        // The real thing, then put the source back.
        project.edit(main, offset..offset, &typed)?;
        let (truth, cost) = support::measure(|| support::compile(project));
        let after = project.snapshot();
        project.edit(main, offset..offset + typed.len(), "")?;
        stats.compile_ms.push(ms(cost.wall));
        let truth = truth?;
        let Some(true_page) = truth.pages().get(page_index) else {
            continue;
        };

        match prediction {
            Err(reason) => *stats.declined.entry(reason).or_default() += 1,
            Ok(predicted) => {
                stats.predicted += 1;
                let true_map = PageMap::build(&after, &true_page.frame);
                // The line as it was, and as it really is now. Unchanged
                // breaks mean it grew by exactly the typed text.
                let before = line_extent(map, main, offset - 1);
                let now = line_extent(&true_map, main, offset);
                let same_breaks = match (&before, &now) {
                    (Some((old, _)), Some((new, _))) => {
                        new.start == old.start && new.end == old.end + typed.len()
                    }
                    _ => false,
                };
                let wrong = wrong_pixels(true_page, predicted.frame);
                if wrong == 0.0 {
                    stats.exact += 1;
                } else {
                    stats.wrong_pixels.push(wrong);
                }
                if same_breaks {
                    stats.same_breaks += 1;
                    stats.same_breaks_exact += usize::from(wrong == 0.0);
                    if let Some((_, real)) = now {
                        let error = (predicted.typed.max.x - real.max.x).to_pt().abs();
                        stats.caret_error.push(error);
                    }
                }
            }
        }
    }
    Some(stats)
}

/// The source range covered by the prose on the visual line that shows the
/// byte at `offset`, and the cell of the glyph showing that byte.
fn line_extent(
    map: &PageMap,
    main: FileId,
    offset: usize,
) -> Option<(std::ops::Range<usize>, typst::layout::Rect)> {
    // Everything that has a place in the source counts, so that a break
    // moving next to an inline equation is noticed too.
    let text = |glyph: &zetwerk_typst::MappedGlyph| match &glyph.origin {
        Origin::Text { file, range }
        | Origin::Space { file, range }
        | Origin::Math { file, range }
            if *file == main =>
        {
            Some(range.clone())
        }
        _ => None,
    };
    let glyphs = map.glyphs();
    let here = glyphs
        .iter()
        .find(|glyph| text(glyph).is_some_and(|range| range.contains(&offset)))?;
    let bottom = here.rect.max.y;
    // Glyphs on one line sit within a few points of each other vertically
    // (sub- and superscripts included); lines are further apart than that.
    let on_line =
        |glyph: &&zetwerk_typst::MappedGlyph| (glyph.rect.max.y - bottom).to_pt().abs() < 4.5;
    let ranges: Vec<_> = glyphs.iter().filter(on_line).filter_map(text).collect();
    let start = ranges.iter().map(|range| range.start).min()?;
    let end = ranges.iter().map(|range| range.end).max()?;
    Some((start..end, here.rect))
}

/// Chooses a place in running prose and a character to type there: a letter
/// after any letter, or a space between two letters.
fn pick_keystroke(
    project: &Project,
    main: FileId,
    map: &PageMap,
    rng: &mut Rng,
) -> Option<(usize, String)> {
    let text = project.source(main)?.text();
    let glyphs = map.glyphs();
    let glyph = glyphs.get(rng.below(glyphs.len().max(1)))?;
    let Origin::Text { file, range } = &glyph.origin else {
        return None;
    };
    if *file != main {
        return None;
    }
    let before = text.get(..range.end)?.chars().next_back()?;
    let after = text.get(range.end..)?.chars().next()?;
    if !before.is_ascii_alphabetic() {
        return None;
    }
    let typed = if after.is_ascii_alphabetic() && rng.below(5) == 0 {
        ' '
    } else {
        char::from(b'a' + u8::try_from(rng.below(26)).ok()?)
    };
    Some((range.end, typed.to_string()))
}

/// Percentage of the page's pixels on which the prediction differs visibly
/// from the real page.
fn wrong_pixels(real: &Page, predicted: Frame) -> f64 {
    let ours = Page {
        frame: predicted,
        ..real.clone()
    };
    let real = zetwerk_render::render_page(real, SCALE);
    let ours = zetwerk_render::render_page(&ours, SCALE);
    if (real.width(), real.height()) != (ours.width(), ours.height()) {
        return 100.0;
    }
    let differing = real
        .pixels()
        .iter()
        .zip(ours.pixels())
        .filter(|(a, b)| a != b)
        .count();
    100.0 * differing as f64 / f64::from(real.width() * real.height())
}

/// A small xorshift generator, so runs are repeatable.
struct Rng(u64);

impl Rng {
    fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % bound.max(1) as u64) as usize
    }
}
