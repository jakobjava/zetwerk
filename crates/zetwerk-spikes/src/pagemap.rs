//! Spike 3: how much of a real page can be edited in place?

use std::collections::BTreeMap;

use zetwerk_typst::{Atomic, Origin, PageMap};

use crate::support::{self, Doc, ms};

pub fn run() {
    println!("\n== Spike 3: page map — what the glyphs on a page map to ==\n");
    println!(
        "{:<26} {:>7} {:>7} {:>7} {:>7} {:>7} {:>10}",
        "document", "glyphs", "text %", "space %", "math %", "atomic %", "ms / page"
    );
    let env = support::environment();
    let mut docs = support::offline_docs();
    docs.truncate(3);
    docs.push(Doc {
        name: "slides (touying)",
        text: zetwerk_corpus::SLIDES.to_owned(),
    });

    for doc in docs {
        let project = support::project(&env, &doc.text);
        let Some(document) = support::compile(&project) else {
            println!("{:<26} failed to compile", doc.name);
            continue;
        };
        let world = project.snapshot();
        let (maps, cost) = support::measure(|| {
            document
                .pages()
                .iter()
                .map(|page| PageMap::build(&world, &page.frame))
                .collect::<Vec<_>>()
        });

        let mut counts = Counts::default();
        for glyph in maps.iter().flat_map(PageMap::glyphs) {
            counts.add(&glyph.origin);
        }
        let share = |n: usize| 100.0 * n as f64 / counts.total.max(1) as f64;
        println!(
            "{:<26} {:>7} {:>7.1} {:>7.1} {:>7.1} {:>7.1} {:>10.3}",
            doc.name,
            counts.total,
            share(counts.text),
            share(counts.space),
            share(counts.math),
            share(counts.atomic),
            ms(cost.wall) / maps.len().max(1) as f64,
        );
        for (reason, count) in &counts.reasons {
            println!("{:<26}   atomic: {:>5.1} %  {reason}", "", share(*count));
        }
    }
}

#[derive(Default)]
struct Counts {
    total: usize,
    text: usize,
    space: usize,
    math: usize,
    atomic: usize,
    reasons: BTreeMap<String, usize>,
}

impl Counts {
    fn add(&mut self, origin: &Origin) {
        self.total += 1;
        match origin {
            Origin::Text { .. } => self.text += 1,
            Origin::Space { .. } => self.space += 1,
            Origin::Math { .. } => self.math += 1,
            Origin::Object { kind, .. } => {
                self.atomic += 1;
                *self.reasons.entry(format!("object: {kind:?}")).or_default() += 1;
            }
            Origin::Atomic(reason) => {
                self.atomic += 1;
                let reason = match reason {
                    Atomic::Detached => "no source location".to_owned(),
                    Atomic::ForeignFile => "package or unowned file".to_owned(),
                    Atomic::Transformed => "text node, but transformed".to_owned(),
                    Atomic::Markup(kind) => format!("markup: {kind:?}"),
                };
                *self.reasons.entry(reason).or_default() += 1;
            }
        }
    }
}
