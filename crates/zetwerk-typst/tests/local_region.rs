//! What `local` says of random edits of random documents: the region it
//! finds holds all that changed, and what is cut down to it keeps the
//! numbers its nodes have in the whole.
//!
//! The second does not hold of itself: a mark that sets text off typed
//! and not closed, or a list marker typed on the line of a rule, is read
//! otherwise in what is cut down than in the whole (27 of 2000 edits
//! once). `cut_down` finds that and refuses; what it gives keeps them.

#![allow(clippy::unwrap_used, reason = "a failed step should fail the test")]

use std::collections::HashSet;
use std::ops::Range;

use typst::syntax::{LinkedNode, Source};
use zetwerk_typst::local::{changed_region, cut_down};

/// A small generator of numbers, the same for the same seed.
struct Dice(u64);

impl Dice {
    fn next(&mut self) -> u64 {
        // SplitMix64.
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn pick<'a>(&mut self, of: &[&'a str]) -> &'a str {
        of[self.below(of.len())]
    }
}

const RULES: &[&str] = &[
    "#set text(size: 11pt)\n",
    "#set heading(numbering: \"1.\")\n",
    "#let x = 1\n",
    "// a comment\n",
    "\n",
];

const BLOCKS: &[&str] = &[
    "= Heading\n",
    "A paragraph of words.\n",
    "Words with $x^2$ in them,\nand a second line.\n",
    "$ a / b $\n",
    "- item\n- more\n",
    "+ one\n+ two\n",
    "#figure(table(columns: 1, [x]), caption: [C]) <fig>\n",
    "See @fig and *strong* _emph_ é 漢字.\n",
    "#footnote[a note]\n",
    "```\ncode\n```\n",
];

const TYPED: &[&str] = &[
    "x", " ", "\n", "\n\n", "$", "^", "_", "(", ")", "*", "= ", "- ", "#", "[", "]", "é", "word",
];

/// The number of a node within its file.
fn number(node: &LinkedNode) -> u64 {
    node.span().into_raw().get() & ((1 << 48) - 1)
}

/// The numbers of the nodes of `source` that lie within `bytes`.
fn numbers_within(source: &Source, bytes: &Range<usize>) -> Vec<u64> {
    fn gather(node: &LinkedNode, bytes: &Range<usize>, out: &mut Vec<u64>) {
        let range = node.range();
        if bytes.start <= range.start && range.end <= bytes.end && !node.span().is_detached() {
            out.push(number(node));
        }
        for child in node.children() {
            gather(&child, bytes, out);
        }
    }
    let mut out = Vec::new();
    gather(&LinkedNode::new(source.root()), bytes, &mut out);
    out
}

/// A document of rules and paragraphs.
fn document(dice: &mut Dice) -> String {
    let mut text: String = (0..dice.below(3)).map(|_| dice.pick(RULES)).collect();
    for _ in 0..2 + dice.below(6) {
        text.push('\n');
        text.push_str(dice.pick(BLOCKS));
    }
    text
}

/// A place between characters of `text`.
fn place(dice: &mut Dice, text: &str) -> usize {
    let mut at = dice.below(text.len() + 1);
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// Random edits of random documents, each asked about; the cases in
/// which what was cut down lost numbers of the region.
fn regions() -> Vec<String> {
    let (mut found, mut refused, mut read_otherwise) = (0, 0, 0);
    let mut losses = Vec::new();
    for seed in 0..2000 {
        let mut dice = Dice(seed);
        let text = document(&mut dice);
        let old = Source::detached(text.clone());
        let mut new = old.clone();
        // Something typed, or something deleted.
        let from = place(&mut dice, &text);
        let to = if dice.below(3) == 0 {
            let to = place(&mut dice, &text);
            to.max(from).min(from + 12)
        } else {
            from
        };
        let mut to = to;
        while !text.is_char_boundary(to) {
            to -= 1;
        }
        let typed = if to > from { "" } else { dice.pick(TYPED) };
        new.edit(from..to, typed);
        let changed = from..from + typed.len();
        let case = || format!("seed {seed}: {from}..{to} → {typed:?} in {text:?}");
        let Ok(region) = changed_region(&old, &new, &changed) else {
            refused += 1;
            continue;
        };
        found += 1;
        let now = new.text();
        for range in [&region.bytes, &region.kept] {
            assert!(
                range.start <= range.end
                    && range.end <= now.len()
                    && now.is_char_boundary(range.start)
                    && now.is_char_boundary(range.end),
                "{}: {range:?}",
                case()
            );
        }
        assert!(
            region.bytes.start <= changed.start && changed.end <= region.bytes.end,
            "{}: the region {:?} does not hold the change {changed:?}",
            case(),
            region.bytes
        );
        assert!(
            region.kept.start <= region.bytes.start && region.bytes.end <= region.kept.end,
            "{}: kept {:?}, region {:?}",
            case(),
            region.kept,
            region.bytes
        );
        // What is cut down to the region keeps the numbers of all that is
        // in the region.
        for region in [region.clone(), region.clone().alone(&new)] {
            let lead = "#set page(height: auto)\n";
            // What would be read otherwise by itself is not typeset so.
            let Ok(cut) = cut_down(&new, &region, lead) else {
                read_otherwise += 1;
                continue;
            };
            assert!(
                cut.text().ends_with(&now[region.kept.clone()]),
                "{}: cut down to {:?}",
                case(),
                cut.text()
            );
            let kept: HashSet<u64> = numbers_within(&cut, &(0..cut.text().len()))
                .into_iter()
                .collect();
            let lost: Vec<u64> = numbers_within(&new, &region.bytes)
                .into_iter()
                .filter(|number| !kept.contains(number))
                .collect();
            if !lost.is_empty() {
                losses.push(format!(
                    "{}: the cut {:?} lost {} numbers",
                    case(),
                    cut.text(),
                    lost.len()
                ));
            }
        }
    }
    println!(
        "{found} regions found, {refused} refused, {read_otherwise} cuts read otherwise, {} \
         cuts lost numbers",
        losses.len()
    );
    assert!(found > 500, "{found} regions found, {refused} refused");
    assert!(
        read_otherwise <= 60,
        "{read_otherwise} cuts read otherwise: more than is to be expected"
    );
    losses
}

#[test]
fn the_region_holds_the_change() {
    let losses = regions();
    assert!(losses.len() <= 40, "{losses:#?}");
}

#[test]
fn the_cut_keeps_the_numbers_of_the_region() {
    // Once lost: seed 21, `*` typed behind `*strong*`; seed 174, `- `
    // typed at the end of the line of a `#set` rule; seed 1084, `]` in a
    // caption.
    let losses = regions();
    assert!(losses.is_empty(), "{} cuts: {losses:#?}", losses.len());
}
