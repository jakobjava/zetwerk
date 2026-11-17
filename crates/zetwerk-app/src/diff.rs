//! What differs between two texts: by paragraph, and inside paragraphs by
//! word. The page shows a writer what changed since a version they name
//! (see `viewer/changes.rs`); this works out where.
//!
//! Two lists of things are compared with Myers' algorithm (the shortest
//! way of taking things out of one and putting things in to make the
//! other), after what both begin and end with alike is set aside. It
//! finds the fewest changes up to a bound that keeps the work in
//! proportion to the texts; beyond it, all that is left between what is
//! alike counts as one change.
//!
//! A text is first compared by paragraph (the stretches between blank
//! lines, as Typst takes them apart): paragraphs that are the same are
//! passed over at the cost of comparing their bytes. Where paragraphs
//! differ, their words are compared, and only words count: a paragraph
//! whose lines were only broken anew in the source has not changed on
//! the page.

use std::ops::Range;

use typst::syntax::{Source, SyntaxKind};

/// A stretch of one list that stands where a stretch of the other is:
/// what was taken out (`old`) and what was put in (`new`), as indices.
/// One of them may be empty: only taken out, or only put in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old: Range<usize>,
    pub new: Range<usize>,
}

/// How much work the comparison may do, in steps along the lists: beyond
/// it the rest is one change.
const WORK: usize = 4_000_000;
/// Within that, how many changes are looked for at most, and at least.
const MOST_CHANGES: usize = 1024;
const LEAST_CHANGES: usize = 64;

/// Where `new` differs from `old`: the stretches that differ, in order,
/// none touching the next. What lies between them is alike in both.
#[must_use]
pub fn hunks<T: PartialEq>(old: &[T], new: &[T]) -> Vec<Hunk> {
    let before = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let behind = old[before..]
        .iter()
        .rev()
        .zip(new[before..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let (a, b) = (
        &old[before..old.len() - behind],
        &new[before..new.len() - behind],
    );
    if a.is_empty() && b.is_empty() {
        return Vec::new();
    }
    let most = (WORK / (a.len() + b.len())).clamp(LEAST_CHANGES, MOST_CHANGES);
    let matches = matched(a, b, most).unwrap_or_default();
    // The stretches between what is alike.
    let mut found = Vec::new();
    let (mut x, mut y) = (0, 0);
    for (i, j) in matches.into_iter().chain([(a.len(), b.len())]) {
        if i > x || j > y {
            found.push(Hunk {
                old: before + x..before + i,
                new: before + y..before + j,
            });
        }
        (x, y) = (i + 1, j + 1);
    }
    found
}

/// The pairs of things alike in `a` and `b` that the fewest changes
/// leave, in order. `None` if more than `most` changes are needed.
fn matched<T: PartialEq>(a: &[T], b: &[T], most: usize) -> Option<Vec<(usize, usize)>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let limit = (a.len() + b.len()).min(most) as isize;
    // How far along `a` the furthest path on each diagonal `k = x - y`
    // reached, for `k` from `-limit` to `limit`.
    let offset = limit + 1;
    let mut v = vec![0_isize; 2 * offset as usize + 1];
    // What `v` was before each step, of the diagonals reachable then.
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let at = |k: isize| (k + offset) as usize;
    let mut reached = None;
    'steps: for d in 0..=limit {
        trace.push(v[at(-d)..=at(d)].to_vec());
        for k in (-d..=d).step_by(2) {
            let mut x = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) {
                v[at(k + 1)]
            } else {
                v[at(k - 1)] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[at(k)] = x;
            if x >= n && y >= m {
                reached = Some(d);
                break 'steps;
            }
        }
    }
    let steps = reached?;
    // Back along the path, from its end.
    let mut pairs = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (1..=steps).rev() {
        let before = &trace[d as usize];
        let get = |k: isize| before[(k + d) as usize];
        let k = x - y;
        let from = if k == -d || (k != d && get(k - 1) < get(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let from_x = get(from);
        let from_y = from_x - from;
        while x > from_x && y > from_y {
            x -= 1;
            y -= 1;
            pairs.push((x as usize, y as usize));
        }
        (x, y) = (from_x, from_y);
    }
    while x > 0 && y > 0 {
        x -= 1;
        y -= 1;
        pairs.push((x as usize, y as usize));
    }
    pairs.reverse();
    Some(pairs)
}

/// The words of `text`: what stands between blanks.
#[must_use]
pub fn words(text: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut start = None;
    for (at, c) in text.char_indices() {
        match (c.is_whitespace(), start) {
            (true, Some(begun)) => {
                found.push(begun..at);
                start = None;
            }
            (false, None) => start = Some(at),
            _ => {}
        }
    }
    if let Some(begun) = start {
        found.push(begun..text.len());
    }
    found
}

/// Where the words of `new` differ from those of `old`: the bytes of
/// each in its own text. Where words were only taken out, the bytes of
/// `new` are an empty stretch where they stood: before the word that
/// follows them, or at the end of the last.
#[must_use]
pub fn word_changes(old: &str, new: &str) -> Vec<(Range<usize>, Range<usize>)> {
    let (old_words, new_words) = (words(old), words(new));
    let texts = |text: &str, words: &[Range<usize>]| -> Vec<String> {
        words
            .iter()
            .map(|word| text[word.clone()].to_owned())
            .collect()
    };
    let found = hunks(&texts(old, &old_words), &texts(new, &new_words));
    found
        .into_iter()
        .map(|hunk| {
            (
                bytes_of(&hunk.old, &old_words, old.len()),
                bytes_of(&hunk.new, &new_words, new.len()),
            )
        })
        .collect()
}

/// The bytes of the words `items` of a text `len` bytes long whose words
/// are `words`; where `items` is empty, the place where they would be.
fn bytes_of(items: &Range<usize>, words: &[Range<usize>], len: usize) -> Range<usize> {
    if items.is_empty() {
        let after_last = || {
            let last = items.start.checked_sub(1).and_then(|last| words.get(last));
            last.map_or(0, |word| word.end).min(len)
        };
        let at = words
            .get(items.start)
            .map_or_else(after_last, |word| word.start);
        return at..at;
    }
    words[items.start].start..words[items.end - 1].end
}

/// The paragraphs of `source`: the stretches between blank lines that
/// have something in them, as Typst takes them apart (a blank line in
/// a call or a block of code parts nothing), without the blanks around
/// them.
#[must_use]
pub fn paragraphs(source: &Source) -> Vec<Range<usize>> {
    let text = source.text();
    let mut found = Vec::new();
    let mut add = |range: Range<usize>| {
        let piece = &text[range.clone()];
        let lead = piece.len() - piece.trim_start().len();
        let kept = piece.trim().len();
        if kept > 0 {
            found.push(range.start + lead..range.start + lead + kept);
        }
    };
    let (mut at, mut begun) = (0, 0);
    for child in source.root().children() {
        let end = at + child.len();
        if child.kind() == SyntaxKind::Parbreak {
            add(begun..at);
            begun = end;
        }
        at = end;
    }
    add(begun..text.len());
    found
}

/// What changed in one place: a run of paragraphs that differ, and the
/// words of them that do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Passage {
    /// The bytes of the paragraphs in the text as it is now, and in the
    /// text compared with. Where paragraphs were only taken out, the
    /// first is the empty stretch where they stood (where the paragraph
    /// after them begins); where only put in, the second is.
    pub new: Range<usize>,
    pub old: Range<usize>,
    /// The words that differ, as bytes of each text (see
    /// [`word_changes`]); none where only paragraphs were taken out.
    pub words: Vec<(Range<usize>, Range<usize>)>,
}

impl Passage {
    /// Whether all of it was taken out: nothing of it is in the text now.
    #[must_use]
    pub fn is_deletion(&self) -> bool {
        self.new.is_empty()
    }
}

/// The bytes of the paragraphs `items` of a text `len` bytes long whose
/// paragraphs are `paragraphs`; where `items` is empty, the place where
/// they would begin.
fn paragraph_bytes(items: &Range<usize>, paragraphs: &[Range<usize>], len: usize) -> Range<usize> {
    if items.is_empty() {
        let at = paragraphs
            .get(items.start)
            .map_or(len, |paragraph| paragraph.start);
        return at..at;
    }
    paragraphs[items.start].start..paragraphs[items.end - 1].end
}

/// What changed from `old` to `new`, whose paragraphs are `old_paragraphs`
/// and `new_paragraphs` (see [`paragraphs`]): the passages, in the order
/// of the text. A passage whose words are the same (its lines were only
/// broken otherwise, or blanks changed) is none.
#[must_use]
pub fn passages(
    old: &str,
    old_paragraphs: &[Range<usize>],
    new: &str,
    new_paragraphs: &[Range<usize>],
) -> Vec<Passage> {
    fn pieces<'a>(text: &'a str, paragraphs: &[Range<usize>]) -> Vec<Words<'a>> {
        paragraphs
            .iter()
            .map(|range| Words(&text[range.clone()]))
            .collect()
    }
    let (was, now) = (pieces(old, old_paragraphs), pieces(new, new_paragraphs));
    let one = |old_items: Range<usize>, new_items: Range<usize>| {
        passage(
            old,
            old_paragraphs,
            old_items,
            new,
            new_paragraphs,
            new_items,
        )
    };
    let mut found = Vec::new();
    for hunk in hunks(&was, &now) {
        let (m, n) = (hunk.old.len(), hunk.new.len());
        if m == n {
            // As many as there were: each changed by itself.
            found.extend(
                hunk.old
                    .clone()
                    .zip(hunk.new.clone())
                    .filter_map(|(a, b)| one(a..a + 1, b..b + 1)),
            );
            continue;
        }
        if m == 0 || n == 0 || m * n > PAIRED {
            found.extend(one(hunk.old.clone(), hunk.new.clone()));
            continue;
        }
        // Some taken out or put in among others changed: those alike
        // in most of their words are one paragraph changed.
        let like = |items: &Range<usize>, texts: &[Words<'_>]| -> Vec<Like> {
            texts[items.clone()]
                .iter()
                .map(|words| Like::of(words.0))
                .collect()
        };
        let (a, b) = (like(&hunk.old, &was), like(&hunk.new, &now));
        let (mut x, mut y) = (0, 0);
        let tail = Hunk {
            old: m..m,
            new: n..n,
        };
        for inner in hunks(&a, &b).into_iter().chain([tail]) {
            for (i, j) in (x..inner.old.start).zip(y..inner.new.start) {
                let (i, j) = (hunk.old.start + i, hunk.new.start + j);
                found.extend(one(i..i + 1, j..j + 1));
            }
            if !(inner.old.is_empty() && inner.new.is_empty()) {
                let shift = |range: &Range<usize>, by: usize| by + range.start..by + range.end;
                found.extend(one(
                    shift(&inner.old, hunk.old.start),
                    shift(&inner.new, hunk.new.start),
                ));
            }
            (x, y) = (inner.old.end, inner.new.end);
        }
    }
    found
}

/// How many pairs of paragraphs a stretch that changed may have before
/// it is not looked through for paragraphs that are alike.
const PAIRED: usize = 400;

/// A paragraph as it is compared: the same as another where its words
/// are, however its lines are broken in the source.
#[derive(Debug, Clone, Copy)]
struct Words<'a>(&'a str);

impl PartialEq for Words<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0 || self.0.split_whitespace().eq(other.0.split_whitespace())
    }
}

/// A paragraph as it is matched with another among changes: alike where
/// most of their words are the same.
#[derive(Debug)]
struct Like(Vec<String>);

impl Like {
    fn of(text: &str) -> Self {
        let mut words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
        words.sort_unstable();
        Self(words)
    }
}

impl PartialEq for Like {
    fn eq(&self, other: &Self) -> bool {
        let (a, b) = (&self.0, &other.0);
        let (mut i, mut j, mut common) = (0, 0, 0);
        while i < a.len() && j < b.len() {
            match a[i].cmp(&b[j]) {
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Greater => j += 1,
                std::cmp::Ordering::Equal => {
                    common += 1;
                    i += 1;
                    j += 1;
                }
            }
        }
        // At least half, as Dice counts it.
        common * 4 >= a.len() + b.len() && common > 0
    }
}

/// The passage of paragraphs `old_items` of `old` that became `new_items`
/// of `new`; `None` where their words are the same.
fn passage(
    old: &str,
    old_paragraphs: &[Range<usize>],
    old_items: Range<usize>,
    new: &str,
    new_paragraphs: &[Range<usize>],
    new_items: Range<usize>,
) -> Option<Passage> {
    let old_bytes = paragraph_bytes(&old_items, old_paragraphs, old.len());
    let new_bytes = paragraph_bytes(&new_items, new_paragraphs, new.len());
    let mut words: Vec<(Range<usize>, Range<usize>)> =
        word_changes(&old[old_bytes.clone()], &new[new_bytes.clone()])
            .into_iter()
            .map(|(was, now)| {
                (
                    new_bytes.start + now.start..new_bytes.start + now.end,
                    old_bytes.start + was.start..old_bytes.start + was.end,
                )
            })
            .collect();
    if words.is_empty() && old_items.len() == new_items.len() {
        // The same words in as many paragraphs: nothing on the page is
        // different.
        return None;
    }
    if words.is_empty() && !new_items.is_empty() {
        // The same words, parted otherwise into paragraphs.
        words.push((new_bytes.clone(), old_bytes.clone()));
    }
    if new_items.is_empty() {
        words.clear();
    }
    Some(Passage {
        new: new_bytes,
        old: old_bytes,
        words,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::*;

    /// A seeded stream of numbers, for tests that try many cases.
    struct Seeded(u64);

    impl Seeded {
        fn next(&mut self, below: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            usize::try_from(self.0 >> 33).unwrap_or(0) % below.max(1)
        }
    }

    /// The longest run of things both lists have in the same order, the
    /// slow and sure way.
    fn common(a: &[u8], b: &[u8]) -> usize {
        let mut table = vec![vec![0; b.len() + 1]; a.len() + 1];
        for i in (0..a.len()).rev() {
            for j in (0..b.len()).rev() {
                table[i][j] = if a[i] == b[j] {
                    table[i + 1][j + 1] + 1
                } else {
                    table[i + 1][j].max(table[i][j + 1])
                };
            }
        }
        table[0][0]
    }

    /// Checks what `hunks` says of `a` and `b`: in order, apart, and
    /// what lies between them alike; with `fewest`, no change more than
    /// needed.
    fn check(a: &[u8], b: &[u8], fewest: bool) {
        let found = hunks(a, b);
        let (mut x, mut y) = (0, 0);
        let mut changed = 0;
        for hunk in &found {
            assert!(!(hunk.old.is_empty() && hunk.new.is_empty()), "{found:?}");
            assert!(hunk.old.start >= x && hunk.new.start >= y, "{found:?}");
            // Between this and the last, alike.
            assert_eq!(
                hunk.old.start - x,
                hunk.new.start - y,
                "{a:?} {b:?} {found:?}"
            );
            assert_eq!(a[x..hunk.old.start], b[y..hunk.new.start], "{found:?}");
            // Not touching the last.
            assert!(
                x == 0 && y == 0 || hunk.old.start > x || hunk.new.start > y,
                "{found:?}"
            );
            changed += hunk.old.len() + hunk.new.len();
            (x, y) = (hunk.old.end, hunk.new.end);
        }
        assert_eq!(a[x..], b[y..], "{a:?} {b:?} {found:?}");
        if fewest {
            assert_eq!(changed, a.len() + b.len() - 2 * common(a, b), "{a:?} {b:?}");
        }
    }

    #[test]
    fn what_differs_is_found_with_the_fewest_changes() {
        let mut seeded = Seeded(11);
        for _ in 0..3000 {
            let alphabet = 1 + seeded.next(4);
            let a: Vec<u8> = (0..seeded.next(12))
                .map(|_| seeded.next(alphabet) as u8)
                .collect();
            let mut b = a.clone();
            for _ in 0..seeded.next(5) {
                match seeded.next(3) {
                    0 if !b.is_empty() => {
                        b.remove(seeded.next(b.len()));
                    }
                    1 => b.insert(seeded.next(b.len() + 1), seeded.next(alphabet) as u8),
                    _ if !b.is_empty() => {
                        let at = seeded.next(b.len());
                        b[at] = seeded.next(alphabet) as u8;
                    }
                    _ => {}
                }
            }
            check(&a, &b, true);
            check(&b, &a, true);
        }
    }

    #[test]
    fn lists_that_differ_everywhere_are_one_change_when_too_long() {
        let a: Vec<u32> = (0..100_000).collect();
        let b: Vec<u32> = (0..100_000).map(|n| n * 7 + 3).collect();
        let started = std::time::Instant::now();
        let found = hunks(&a, &b);
        assert!(started.elapsed().as_secs() < 5);
        assert!(!found.is_empty());
        let alike: Vec<u8> = vec![1; 5000];
        assert_eq!(hunks(&alike, &alike), Vec::new());
        // Long and much alike, bigger than the bound: still right.
        let mut seeded = Seeded(3);
        let long: Vec<u8> = (0..20_000).map(|_| seeded.next(200) as u8).collect();
        let mut other = long.clone();
        for _ in 0..3000 {
            let at = seeded.next(other.len());
            other[at] = 255;
        }
        check(&long, &other, false);
    }

    #[test]
    fn words_differ_and_blanks_do_not() {
        let changes = word_changes("The cat sat on\nthe mat.", "The  dog sat on the\n mat.");
        assert_eq!(changes.len(), 1);
        let (old, new) = &changes[0];
        assert_eq!((&"The cat sat"[old.clone()], new), ("cat", &(5..8)));
        // Only taken out: where it was, before what follows it.
        let changes = word_changes("one two three", "one three");
        assert_eq!(changes, vec![(4..7, 4..4)]);
        let changes = word_changes("one two", "one");
        assert_eq!(changes, vec![(4..7, 3..3)]);
        let changes = word_changes("one", "zero one");
        assert_eq!(changes, vec![(0..0, 0..4)]);
    }

    #[test]
    fn paragraphs_are_what_stands_between_blank_lines() {
        let text = "= Head\nFirst words\nand more.\n\n  \n#figure(\n  rect(),\n\n  caption: [A],\n)\n\nLast.\n";
        let source = Source::detached(text);
        let found: Vec<&str> = paragraphs(&source)
            .into_iter()
            .map(|range| &text[range])
            .collect();
        assert_eq!(
            found,
            vec![
                "= Head\nFirst words\nand more.",
                "#figure(\n  rect(),\n\n  caption: [A],\n)",
                "Last."
            ]
        );
        assert!(paragraphs(&Source::detached("\n\n")).is_empty());
    }

    fn compared(old: &str, new: &str) -> Vec<Passage> {
        let (a, b) = (Source::detached(old), Source::detached(new));
        passages(old, &paragraphs(&a), new, &paragraphs(&b))
    }

    #[test]
    fn passages_are_the_paragraphs_that_changed_and_their_words() {
        let old = "One a.\n\nTwo b.\n\nThree c.\n\nFour d.\n";
        // A word changed in the second, the third taken out, a fifth
        // put in, and the first broken otherwise in the source.
        let new = "One\na.\n\nTwo B.\n\nFour d.\n\nFive e.\n";
        let found = compared(old, new);
        assert_eq!(found.len(), 3, "{found:#?}");
        assert_eq!(&new[found[0].new.clone()], "Two B.");
        assert_eq!(found[0].words.len(), 1);
        assert_eq!(&new[found[0].words[0].0.clone()], "B.");
        assert_eq!(&old[found[0].words[0].1.clone()], "b.");
        assert!(found[1].is_deletion());
        assert_eq!(&old[found[1].old.clone()], "Three c.");
        assert_eq!(&new[found[1].new.start..], "Four d.\n\nFive e.\n");
        assert_eq!(&new[found[2].new.clone()], "Five e.");
        assert!(found[2].old.is_empty());
        // The same text: nothing.
        assert_eq!(compared(old, old), Vec::new());
        // A paragraph parted in two is a change, though no word is.
        let parted = compared("A b c d.\n", "A b\n\nc d.\n");
        assert!(!parted.is_empty());
    }

    #[test]
    fn passages_rebuild_the_text_they_were_found_in() {
        // Any edit: the passages, put into the old text, give the new.
        let mut seeded = Seeded(5);
        let base = "Alpha beta.\n\nGamma delta epsilon.\n\n= Zeta\nEta theta.\n\nIota.\n";
        let pieces = ["word ", "\n\n", " ", "x", "\n", "= ", "kappa lambda"];
        for _ in 0..500 {
            let mut new = base.to_owned();
            for _ in 0..1 + seeded.next(4) {
                let at = seeded.next(new.len() + 1);
                let end = (at + seeded.next(12)).min(new.len());
                if new.is_char_boundary(at) && new.is_char_boundary(end) {
                    new.replace_range(at..end, pieces[seeded.next(pieces.len())]);
                }
            }
            let found = compared(base, &new);
            let mut rebuilt = String::new();
            let mut at = 0;
            for passage in &found {
                rebuilt.push_str(&base[at..passage.old.start]);
                rebuilt.push(' ');
                rebuilt.push_str(&new[passage.new.clone()]);
                rebuilt.push(' ');
                at = passage.old.end;
            }
            rebuilt.push_str(&base[at..]);
            // The same words, if not the same blanks.
            let words = |text: &str| -> Vec<String> {
                text.split_whitespace().map(str::to_owned).collect()
            };
            assert_eq!(words(&rebuilt), words(&new), "{new:?}");
        }
    }
}
