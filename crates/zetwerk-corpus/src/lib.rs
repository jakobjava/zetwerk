//! Documents that tests, benchmarks and spikes run against.
//!
//! The small documents are checked in. The thesis is generated, because a
//! realistic long document has its prose *in the source* (not produced by
//! `lorem()`), and a megabyte of filler does not belong in the repository.

use std::fmt::Write;

/// A two-page paper: headings, labels and references, display and inline
/// math, lists, a footnote. Ragged-right.
pub const PAPER: &str = include_str!("../docs/paper.typ");

/// A dense, justified, math-heavy article with first-line indents.
pub const ARTICLE: &str = include_str!("../docs/article.typ");

/// A slide deck using the `touying` package. Compiling it needs the package,
/// which is downloaded from Typst Universe on first use.
pub const SLIDES: &str = include_str!("../docs/slides.typ");

/// Generates a long, justified, book-style document.
///
/// With the embedded fonts, one chapter is roughly five pages, so
/// `thesis(25)` gives a document of well over a hundred pages. The output
/// depends only on `chapters`.
#[must_use]
pub fn thesis(chapters: usize) -> String {
    let mut rng = Rng(0x5EED_1234_ABCD_0001);
    let mut out = String::from(THESIS_PREAMBLE);
    for chapter in 1..=chapters {
        let title = title(&mut rng);
        let _ = writeln!(out, "= {title} <ch-{chapter}>\n");
        paragraph(&mut out, &mut rng, chapter, chapters);
        for section in 1..=4 {
            let title = title_case(&phrase(&mut rng, 2, 4));
            let _ = writeln!(out, "== {title} <sec-{chapter}-{section}>\n");
            for _ in 0..rng.range(3, 5) {
                paragraph(&mut out, &mut rng, chapter, chapters);
            }
            match section {
                1 => equation(&mut out, &mut rng, chapter),
                2 => list(&mut out, &mut rng),
                _ => {}
            }
            paragraph(&mut out, &mut rng, chapter, chapters);
        }
    }
    out
}

const THESIS_PREAMBLE: &str = r#"#set page(paper: "a4", margin: (x: 3cm, y: 3cm), numbering: "1")
#set text(size: 11pt, lang: "en")
#set par(justify: true)
#set heading(numbering: "1.1")
#set math.equation(numbering: "(1)")
#show heading.where(level: 1): it => { pagebreak(weak: true); it }

#outline()

"#;

fn paragraph(out: &mut String, rng: &mut Rng, chapter: usize, chapters: usize) {
    for index in 0..rng.range(4, 7) {
        if index > 0 {
            // One sentence per source line, as people who keep Typst in git
            // tend to write it.
            out.push('\n');
        }
        sentence(out, rng, chapter, chapters);
    }
    out.push_str("\n\n");
}

fn sentence(out: &mut String, rng: &mut Rng, chapter: usize, chapters: usize) {
    let words = rng.range(8, 22);
    for index in 0..words {
        let word = rng.pick(WORDS);
        if index == 0 {
            out.push_str(&capitalize(word));
        } else {
            out.push(' ');
            match rng.range(0, 60) {
                0 => {
                    let _ = write!(out, "_{word}_");
                }
                1 => {
                    let _ = write!(out, "*{word}*");
                }
                2 => {
                    let _ = write!(out, "$x_{}$", rng.range(1, 9));
                }
                3 => {
                    let _ = write!(out, "@ch-{}", rng.range(1, chapters));
                }
                4 if chapter > 1 => {
                    let _ = write!(out, "{word}#footnote[{}.]", phrase(rng, 4, 9));
                }
                _ => out.push_str(word),
            }
        }
        if index + 1 < words && rng.range(0, 9) == 0 {
            out.push(',');
        }
    }
    out.push('.');
}

fn equation(out: &mut String, rng: &mut Rng, chapter: usize) {
    let (a, b) = (rng.range(2, 9), rng.range(2, 9));
    let _ = writeln!(
        out,
        "$ sum_(k=1)^n k^{a} = integral_0^{b} f(x) dif x + O(n^{a}) $ <eq-{chapter}>\n"
    );
}

fn list(out: &mut String, rng: &mut Rng) {
    for _ in 0..rng.range(3, 5) {
        let _ = writeln!(out, "- {}.", capitalize(&phrase(rng, 5, 12)));
    }
    out.push('\n');
}

fn phrase(rng: &mut Rng, min: usize, max: usize) -> String {
    let words: Vec<&str> = (0..rng.range(min, max)).map(|_| rng.pick(WORDS)).collect();
    words.join(" ")
}

fn title(rng: &mut Rng) -> String {
    title_case(&phrase(rng, 2, 5))
}

fn title_case(text: &str) -> String {
    let words: Vec<String> = text.split(' ').map(capitalize).collect();
    words.join(" ")
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// A small xorshift generator: deterministic everywhere, no dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A value in `min..=max`.
    fn range(&mut self, min: usize, max: usize) -> usize {
        min + (self.next() % (max - min + 1) as u64) as usize
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[(self.next() % items.len() as u64) as usize]
    }
}

const WORDS: &[&str] = &[
    "the",
    "of",
    "and",
    "a",
    "to",
    "in",
    "is",
    "that",
    "for",
    "it",
    "as",
    "with",
    "on",
    "by",
    "this",
    "we",
    "are",
    "from",
    "or",
    "an",
    "be",
    "which",
    "can",
    "has",
    "not",
    "these",
    "also",
    "between",
    "such",
    "more",
    "each",
    "when",
    "where",
    "their",
    "one",
    "result",
    "method",
    "analysis",
    "system",
    "model",
    "function",
    "structure",
    "property",
    "estimate",
    "bound",
    "operator",
    "space",
    "sequence",
    "measure",
    "condition",
    "argument",
    "approach",
    "problem",
    "solution",
    "parameter",
    "value",
    "theory",
    "example",
    "definition",
    "theorem",
    "proof",
    "lemma",
    "section",
    "chapter",
    "figure",
    "table",
    "data",
    "process",
    "behaviour",
    "limit",
    "rate",
    "order",
    "term",
    "constant",
    "variable",
    "domain",
    "boundary",
    "energy",
    "distribution",
    "average",
    "general",
    "particular",
    "simple",
    "complex",
    "linear",
    "local",
    "global",
    "smooth",
    "continuous",
    "discrete",
    "finite",
    "infinite",
    "optimal",
    "uniform",
    "sharp",
    "classical",
    "standard",
    "natural",
    "essential",
    "sufficient",
    "necessary",
    "consider",
    "obtain",
    "show",
    "prove",
    "assume",
    "define",
    "denote",
    "follows",
    "implies",
    "satisfies",
    "depends",
    "converges",
    "holds",
    "yields",
    "requires",
    "provides",
    "describes",
    "remains",
    "becomes",
    "extends",
    "applies",
    "determines",
    "however",
    "therefore",
    "moreover",
    "furthermore",
    "consequently",
    "nevertheless",
    "independently",
    "approximately",
    "immediately",
    "precisely",
    "respectively",
    "characterisation",
    "representation",
    "generalisation",
    "approximation",
    "interpretation",
    "correspondence",
    "decomposition",
    "regularity",
    "compactness",
    "understanding",
    "observation",
    "construction",
    "assumption",
    "consequence",
];
