//! What the page shows between a key and the compile of all of the text:
//! a part typeset by itself and put in, or the line worked out. It is to
//! be what all of the text typeset at once shows, or the page waits.

use super::*;

/// Where every glyph of `frame` stands, to a tenth of a point, and which
/// byte it shows, in order of the page.
fn placed(world: &ProjectWorld, frame: &typst::layout::Frame) -> Vec<(i64, i64, Option<usize>)> {
    let map = PageMap::build(world, frame);
    let tenth = |length: typst::layout::Abs| (length.to_pt() * 10.0).round() as i64;
    let mut all: Vec<_> = map
        .glyphs()
        .iter()
        .map(|glyph| {
            let byte = match &glyph.origin {
                zetwerk_typst::Origin::Text { range, .. }
                | zetwerk_typst::Origin::Space { range, .. }
                | zetwerk_typst::Origin::Math { range, .. }
                | zetwerk_typst::Origin::Object { range, .. } => Some(range.start),
                zetwerk_typst::Origin::Atomic(_) => None,
            };
            (tenth(glyph.rect.min.x), tenth(glyph.rect.min.y), byte)
        })
        .collect();
    all.sort_unstable();
    all
}

/// What is wrong with the pages on screen, if anything: a page that
/// shows otherwise than all of the text as it is, typeset at once.
fn differs_from_whole(viewer: &mut Viewer) -> Option<String> {
    let shown = viewer.document.clone()?;
    let shown_world = viewer.compiled_world.clone()?;
    let (_, world) = viewer.snapshot();
    let whole = zetwerk_typst::compile(&world).document?;
    if whole.pages().len() != shown.pages().len() {
        return Some(format!(
            "{} pages, {} in the whole",
            shown.pages().len(),
            whole.pages().len()
        ));
    }
    for (index, (page, full)) in shown.pages().iter().zip(whole.pages()).enumerate() {
        let (here, there) = (
            placed(&shown_world, &page.frame),
            placed(&world, &full.frame),
        );
        if here != there {
            let first = here.iter().zip(&there).position(|(a, b)| a != b);
            return Some(format!(
                "page {index}: {} glyphs, {} in the whole; first apart: {:?}",
                here.len(),
                there.len(),
                first.map(|at| (here[at], there[at]))
            ));
        }
    }
    None
}

/// The article of the corpus: justified, indented, with references.
const ARTICLE: &str = include_str!("../../../../zetwerk-corpus/docs/article.typ");
/// The paper of the corpus: headings, references, a footnote, lists.
const PAPER: &str = include_str!("../../../../zetwerk-corpus/docs/paper.typ");

/// `keys` pressed on the page of `text`, with the cursor put first at
/// what `at` finds (at its end, with `behind`); what is wrong with the
/// page then, if it says it shows the text.
fn after_keys(text: &str, at: &str, behind: bool, keys: &str) -> Option<String> {
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "gsgs");
    let found = text
        .find(at)
        .unwrap_or_else(|| panic!("{at:?} is not in the text"));
    if let Some(editor) = viewer.editor.as_mut() {
        editor.set_cursor(found + if behind { at.len() } else { 0 });
    }
    press(&mut viewer, keys);
    let wrong = (viewer.document_revision == Some(viewer.revision))
        .then(|| differs_from_whole(&mut viewer))
        .flatten()
        .map(|wrong| format!("{wrong}; part: {}", viewer.stats.local));
    let _ = std::fs::remove_dir_all(directory);
    wrong
}

#[test]
fn a_star_typed_behind_strong_text_is_not_shown_as_it_is_not() {
    // `*` behind `*strong*` begins strong text that is not closed; cut
    // down to the part, Typst read the paragraph otherwise.
    let text = "#set page(width: 300pt, height: 300pt)\n\n\
                See *strong* _emph_ words.\n\n- item\n- more\n\nLast.\n";
    assert_eq!(after_keys(text, "*strong*", true, "i*"), None);
    assert_eq!(after_keys(text, "_emph_", true, "i_"), None);
}

#[test]
fn a_list_marker_typed_on_the_line_of_a_rule_is_not_shown_as_a_list() {
    // In the whole, `- ` behind a rule on its line is text; in the part,
    // after the lead, it began a line and was a list.
    let text = "#set page(width: 300pt, height: 300pt)\n\nWords of a paragraph.\n\nMore.\n";
    assert_eq!(after_keys(text, "300pt)", true, "i- "), None);
}

#[test]
fn what_follows_the_rules_keeps_its_numbers_in_the_part() {
    // A figure right behind the rules: the lead put in before it began
    // with `#` as the figure does, and Typst took one for the other.
    let text = "#set page(width: 300pt, height: 300pt)\n\n\
                #figure(table(columns: 1, [x]), caption: [Cap]) <fig>\n\nMore words.\n";
    assert_eq!(after_keys(text, "Cap", true, "itions"), None);
}

#[test]
fn a_paragraph_with_a_reference_shows_what_the_whole_shows() {
    // The part showed `@eq-weyl` by its name, which is narrower than
    // `Equation (1)`: the lines broke elsewhere. The typist found it:
    // seed 2001, `ywyyi<Down><C-b>` in the article.
    assert_eq!(after_keys(ARTICLE, "the bill", true, "i<C-b>"), None);
}

#[test]
fn a_line_more_on_a_full_page_is_for_the_whole() {
    // A paragraph begun at the foot of a full page: the heading below
    // goes to the next page in the whole, and stood in the margin in the
    // part. The typist found it: seed 3000, `yyo` in the paper.
    assert_eq!(
        after_keys(PAPER, "of the original points.", true, "o"),
        None
    );
}

#[test]
fn a_footnote_put_in_is_typeset_with_all_of_the_page() {
    // Its text stands at the foot of the page, which the part is not
    // put into. The typist found it in an empty document.
    let text = "#set page(width: 300pt, height: 300pt)\n\nthe\n\nWords here.\n";
    assert_eq!(after_keys(text, "the", false, "i#foot<Tab>"), None);
}

#[test]
fn a_rule_among_the_text_holds_for_the_part() {
    // A rule set between paragraphs, not among the rules before them
    // all: the part was typeset without it.
    let text = "#set page(width: 300pt, height: 300pt)\n\nFirst words.\n\n\
                #set text(size: 16pt)\n\nSecond words.\n\nThird $x$ words.\n";
    assert_eq!(after_keys(text, "$x", true, "a^2"), None);
}

#[test]
fn text_in_columns_is_typeset_whole() {
    // Columns made by a rule that is not `#set page(columns: ..)`: the
    // part is typeset in two columns of a page of its own, beginning one
    // where it stood in the second.
    // The part with the paragraph before it began a column of its own.
    let text = format!(
        "#set page(width: 300pt, height: 300pt)\n#show: columns.with(2)\n\n{}",
        paragraphs(9)
    );
    for at in ["x_3", "x_6"] {
        assert_eq!(after_keys(&text, at, true, "a^2"), None, "{at}");
    }
}

/// The elements that begin on `frame` and do not end there, by where
/// they are: what a glyph with no place of its own is taken to be part
/// of, the page number among them.
fn left_open(frame: &typst::layout::Frame) -> Vec<String> {
    use typst::introspection::Tag;
    use typst::layout::FrameItem;
    fn walk(frame: &typst::layout::Frame, open: &mut Vec<String>) {
        for (_, item) in frame.items() {
            match item {
                FrameItem::Group(group) => walk(&group.frame, open),
                FrameItem::Tag(Tag::Start(element, _)) => {
                    open.push(format!(
                        "{:?} {}",
                        element.location(),
                        element.elem().name()
                    ));
                }
                FrameItem::Tag(Tag::End(location, ..)) => {
                    let name = format!("{:?} ", Some(*location));
                    if let Some(at) = open.iter().rposition(|open| open.starts_with(&name)) {
                        open.remove(at);
                    }
                }
                _ => {}
            }
        }
    }
    let mut open = Vec::new();
    walk(frame, &mut open);
    open
}

#[test]
fn nothing_is_left_open_on_a_page_a_part_is_put_into() {
    // An equation of the part and one of the page were given the same
    // place by Typst, each in its own document: one of them was left
    // open, and the page number was taken to be part of it. The typist
    // found it: seed 2000, `A<C-i>` in the article.
    let (mut viewer, directory) = editing(ARTICLE);
    press(&mut viewer, "gsgs");
    if let Some(editor) = viewer.editor.as_mut() {
        editor.set_cursor(647);
    }
    press(&mut viewer, "A<C-i>");
    assert_eq!(viewer.stats.local, 1, "{:?}", viewer.declined);
    let document = viewer.document.clone().unwrap_or_else(|| panic!("pages"));
    assert_eq!(left_open(&document.pages()[0].frame), Vec::<String>::new());
    assert_eq!(differs_from_whole(&mut viewer), None);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_part_not_put_in_leaves_the_mark_of_typing_to_be_taken_away() {
    // Typing into empty strong text, typeset whole while typing with the
    // cursor's mark holding it open; a letter that cannot be typeset by
    // itself (the paragraph places something elsewhere), taken back, and
    // Escape. The letter's attempt had taken the mark from what the pages
    // were said to be made with, so the page kept it after typing.
    let text = "#set page(width: 300pt, height: 300pt)\n\n\
                Words #place(top)[P] here #strong[] end.\n\nMore.\n";
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "gsgs");
    if let Some(editor) = viewer.editor.as_mut() {
        editor.set_cursor(text.find("#strong[").unwrap_or(0) + 8);
    }
    press(&mut viewer, "i");
    let _ = viewer.take_edited();
    compile(&mut viewer);
    assert!(viewer.is_compiled() && viewer.shows_typing());
    press(&mut viewer, "x<BS><Esc>");
    assert_eq!(viewer.take_edited(), Some(Edited::LeftInsert));
    let _ = std::fs::remove_dir_all(directory);
}

/// Paragraphs enough to fill pages of 300 by 200 points.
fn paragraphs(count: usize) -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    for index in 0..count {
        let _ = write!(
            text,
            "Paragraph {index} of the text, with words enough in it to be \
             broken into lines, and $x_{index}$ in it.\n\n"
        );
    }
    text
}

#[test]
fn a_page_wider_than_high_keeps_its_margins_in_a_part() {
    // Typst's margins are a share of the shorter side of the page; on a
    // page as long as it takes, that was the width.
    let text = format!(
        "#set page(width: 400pt, height: 250pt)\n\n{}",
        paragraphs(2)
    );
    assert_eq!(after_keys(&text, "x_1", true, "a^2"), None);
}

#[test]
fn margins_as_shares_of_the_page_are_kept_in_a_part() {
    let text = format!(
        "#set page(width: 300pt, height: 300pt, margin: (x: 10%, y: 12%))\n\n{}",
        paragraphs(2)
    );
    assert_eq!(after_keys(&text, "x_1", true, "a^2"), None);
}

#[test]
fn margins_of_two_sides_are_kept_on_a_left_page() {
    // A part is typeset on a first page, which is a right one.
    let text = format!(
        "#set page(width: 300pt, height: 200pt, margin: (inside: 70pt, outside: 20pt, y: 20pt))\n\n{}",
        paragraphs(8)
    );
    assert_eq!(after_keys(&text, "x_6", true, "a^2"), None);
}

/// The text of the document (not the page number, which comes of no
/// place in it) that the page on screen with the cursor shows on lines
/// lower than `foot` points: their baselines (a subscript a few points
/// lower is on the line).
fn below_foot(viewer: &Viewer, foot: f64) -> Vec<String> {
    use typst::layout::{Frame, FrameItem, Point};
    fn walk(frame: &Frame, at: Point, foot: f64, out: &mut Vec<String>) {
        for (position, item) in frame.items() {
            let here = at + *position;
            match item {
                FrameItem::Group(group) => walk(&group.frame, here, foot, out),
                FrameItem::Text(text)
                    if here.y.to_pt() > foot + 5.0
                        && text.glyphs.iter().any(|glyph| !glyph.span.0.is_detached()) =>
                {
                    out.push(text.text.to_string());
                }
                _ => {}
            }
        }
    }
    let page = match (&viewer.predicted, &viewer.document) {
        (Some(predicted), _) => predicted.page.clone(),
        (None, Some(document)) => document.pages()[0].clone(),
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    walk(&page.frame, Point::zero(), foot, &mut out);
    out
}

#[test]
fn a_line_more_worked_out_on_a_full_page_is_not_shown_in_its_margin() {
    // A paragraph grown on a full page: what it pushed down was drawn in
    // the margin below the text, which was taken to be as wide as the one
    // above it.
    for page in [
        "#set page(width: 300pt, height: 200pt, margin: (top: 20pt, bottom: 60pt))",
        "#set page(width: 300pt, height: 200pt, margin: 30pt, header: [Head])",
    ] {
        let text = format!("{page}\n\n{}", paragraphs(6));
        let (mut viewer, directory) = editing(&text);
        press(&mut viewer, "gsgs");
        let foot = if page.contains("bottom: 60pt") {
            140.5
        } else {
            170.5
        };
        if let Some(editor) = viewer.editor.as_mut() {
            editor.set_cursor(text.find("in it.").unwrap_or(0) + 6);
        }
        for key in ["o", "A", " new", " paragraph", " of", " words."] {
            press(&mut viewer, key);
            assert_eq!(
                below_foot(&viewer, foot),
                Vec::<String>::new(),
                "{page} {key}"
            );
        }
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn a_list_item_begun_below_a_paragraph_stands_where_the_whole_puts_it() {
    // The typist found it: seed 3000, `no+ ` in the paper, at the top of
    // its second page.
    for keys in ["o+ ", "o+ the", "o<Esc>o+ "] {
        assert_eq!(after_keys(PAPER, "$square$", true, keys), None, "{keys}");
    }
}

#[test]
fn a_list_item_with_no_text_yet_is_taken_out_where_it_stands() {
    // An item of a list that holds nothing but its number: the part was
    // put in where the heading behind it stood, a line lower than the
    // whole puts it. The typist found it: seed 3000, `no+ ` in the paper.
    for (text, at) in [
        (PAPER, "$square$"),
        (
            "#set page(width: 300pt, height: 300pt)\n\nWords before.\n\n= Heading\n\nWords after.\n",
            "before.",
        ),
    ] {
        for keys in [" ", " x"] {
            let (mut viewer, directory) = editing(text);
            press(&mut viewer, "gsgs");
            if let Some(editor) = viewer.editor.as_mut() {
                editor.set_cursor(text.find(at).unwrap_or(0) + at.len());
            }
            press(&mut viewer, "o+");
            compile(&mut viewer);
            press(&mut viewer, keys);
            if viewer.document_revision == Some(viewer.revision) {
                assert_eq!(differs_from_whole(&mut viewer), None, "{keys:?} in {at}");
            }
            let _ = std::fs::remove_dir_all(directory);
        }
    }
}

#[test]
fn what_is_worked_out_from_where_it_stands_is_typeset_whole() {
    // A part is typeset on a first page of its own: the number of the
    // page it is on, or of the headings before it, came out otherwise.
    let text = format!(
        "#set page(width: 300pt, height: 200pt)\n\n{}On page #context counter(page).display() of \
         the text, $y$ here.\n\n{}",
        paragraphs(5),
        paragraphs(1)
    );
    assert_eq!(after_keys(&text, "$y", true, "a^2"), None);
}

#[test]
fn a_paragraph_behind_a_list_item_turned_text_is_typeset_whole() {
    // `-` alone is an item of a list; `-**` is a paragraph, and the
    // article indents the paragraph behind it as it does not behind a
    // list: it stood where it was. The typist found it: seed 2014,
    // `O<Esc>o<Esc>o-<C-b>` in the article.
    let text = ARTICLE.replacen("<eq-weyl>\n\n", "<eq-weyl>\n\n-\n\n", 1);
    assert_eq!(after_keys(&text, "<eq-weyl>\n\n-", true, "a<C-b>"), None);
}

#[test]
fn a_list_begun_and_taken_back_at_the_top_leaves_nothing_behind() {
    // `- ` typed into a paragraph at the top of the text, and taken back:
    // the bullet stayed on the page, and all below it a line lower. Its
    // paragraph was numbered below what is kept now, and was taken to be
    // of the rules; and the marks of its elements went in without it.
    // The typist found it: seed 13 in an empty document.
    let text = "of - nüber =\nüber \n\né\n";
    for keys in ["O-<lt><C-w>of ", "O- <BS><BS>of ", "O- x<C-w><BS><BS>of "] {
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "gsgs");
        if let Some(editor) = viewer.editor.as_mut() {
            editor.set_cursor(0);
        }
        for key in keys::parse(keys).unwrap_or_default() {
            viewer.key(key);
            if viewer.document_revision == Some(viewer.revision) {
                assert_eq!(differs_from_whole(&mut viewer), None, "{keys}: {key:?}");
            }
        }
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn a_paragraph_with_references_is_typeset_by_itself_as_in_the_whole() {
    // What is referred to stands hidden in the lead of the part: the
    // references show what they show in the whole, and the part need not
    // wait for all of it to be typeset (it did, for every key in such a
    // paragraph, after references were found to show otherwise).
    let text = "#set page(width: 300pt, height: 400pt)\n#set heading(numbering: \"1.1\")\n\
                #set math.equation(numbering: \"(1)\")\n\n= Start <sec>\n\nWords.\n\n\
                $ a = b $ <eq>\n\n#figure(table(columns: 1, [x]), caption: [C]) <fig>\n\n\
                == More\n\nSee @sec, @eq and @fig, with $x$ in it.\n\nLast words.\n";
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "gsgs");
    if let Some(editor) = viewer.editor.as_mut() {
        editor.set_cursor(text.find("$x").unwrap_or(0) + 2);
    }
    press(&mut viewer, "a^2");
    assert!(viewer.stats.local > 0, "{:?}", viewer.declined);
    assert_eq!(differs_from_whole(&mut viewer), None);
    let _ = std::fs::remove_dir_all(directory);
}
