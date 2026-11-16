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
    // part. The typist found it: seed 3000, `yyo` in the paper. Where
    // the page breaks is for the whole to say; nothing is to be shown
    // below the text (the paper has margins of 2.5 cm).
    let (mut viewer, directory) = editing(PAPER);
    press(&mut viewer, "gsgs");
    if let Some(editor) = viewer.editor.as_mut() {
        let at = "of the original points.";
        editor.set_cursor(PAPER.find(at).unwrap_or(0) + at.len());
    }
    press(&mut viewer, "o");
    assert!(viewer.stats.local > 0, "{:?}", viewer.declined);
    assert_eq!(below_foot(&viewer, 841.89 - 70.87), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(directory);
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

#[test]
fn lines_a_part_left_out_come_back_when_it_shrinks_again() {
    // A paragraph begun at the foot of a full page pushes its last line
    // off, which is left out until the whole puts it on the next page.
    // Taken back before the pause, the line was still left out: each part
    // was put into the page as the one before had left it. The typist
    // found it: seed 3005 in the paper.
    let text = format!(
        "#set page(width: 300pt, height: 200pt, margin: (top: 20pt, bottom: 60pt))\n\n{}",
        paragraphs(6)
    );
    let (mut viewer, directory) = editing(&text);
    press(&mut viewer, "gsgs");
    if let Some(editor) = viewer.editor.as_mut() {
        editor.set_cursor(text.find("in it.").unwrap_or(0) + 6);
    }
    press(&mut viewer, "A<CR>x");
    let parts = viewer.stats.local;
    assert!(parts > 0, "{:?}", viewer.declined);
    press(&mut viewer, "<BS>");
    assert_eq!(
        viewer.document_revision,
        Some(viewer.revision),
        "{:?}",
        viewer.declined
    );
    assert_eq!(differs_from_whole(&mut viewer), None);
    let _ = std::fs::remove_dir_all(directory);
}

/// The tags beside the text of the page on screen, as drawn: where each
/// stands, to a pixel.
fn tags_drawn(viewer: &mut Viewer) -> Vec<[i32; 2]> {
    let scene = viewer.scene(&|_| false);
    let height = f64::from(viewer.view.height) as f32;
    let tints =
        [PAGE_CURSOR, PAGE_BAR, PAGE_SELECTION].map(|rgba| TextureId(hash128(&("tint", rgba))));
    let mut tags: Vec<[i32; 2]> = scene
        .quads
        .iter()
        .filter(|quad| quad.y < height && quad.x > 400.0 && !tints.contains(&quad.texture))
        .map(|quad| [quad.x.round() as i32, quad.y.round() as i32])
        .collect();
    tags.sort_unstable();
    tags
}

#[test]
fn the_tags_in_the_margin_follow_the_lines_a_part_moves() {
    // A paragraph begun above a labelled heading: the heading and the
    // words with a comment move a line down at once, and their tags stood
    // where they were until the pause.
    let text = "#set page(width: 300pt, height: 400pt)\n\
                Before it.\n\n= Head <lab>\n\nSome words. // said\n";
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "gsgs");
    let settled = tags_drawn(&mut viewer);
    assert_eq!(settled.len(), 2, "{settled:?}");
    press(&mut viewer, "/\\=Before it<CR>A<CR>a");
    assert!(viewer.stats.local > 0, "{:?}", viewer.declined);
    assert_eq!(
        viewer.document_revision,
        Some(viewer.revision),
        "{:?}",
        viewer.declined
    );
    let typed = tags_drawn(&mut viewer);
    // And as the line is worked out, or typeset by itself, letter by
    // letter.
    press(&mut viewer, "b");
    let more = tags_drawn(&mut viewer);
    compile(&mut viewer);
    let typeset = tags_drawn(&mut viewer);
    assert_ne!(typeset, settled);
    assert_eq!(typed, typeset);
    assert_eq!(more, typeset);
    let _ = std::fs::remove_dir_all(directory);
}

/// The bytes each glyph of a page shows, page by page.
type Shown = HashMap<usize, Vec<Option<Range<usize>>>>;

/// The maps of all pages of `viewer`, with the text they were made of.
fn maps_now(viewer: &mut Viewer) -> (String, Shown) {
    let world = viewer
        .compiled_world
        .clone()
        .unwrap_or_else(|| panic!("pages"));
    let pages = viewer.document.clone().unwrap_or_else(|| panic!("pages"));
    let file = viewer.file_id();
    let text = world
        .written(file)
        .map(|source| source.text().to_owned())
        .unwrap_or_default();
    let ranges = |map: &PageMap| -> Vec<Option<Range<usize>>> {
        map.glyphs()
            .iter()
            .map(|glyph| match &glyph.origin {
                zetwerk_typst::Origin::Text { range, .. }
                | zetwerk_typst::Origin::Space { range, .. }
                | zetwerk_typst::Origin::Math { range, .. }
                | zetwerk_typst::Origin::Object { range, .. } => Some(range.clone()),
                zetwerk_typst::Origin::Atomic(_) => None,
            })
            .collect()
    };
    let mut all = HashMap::new();
    for (index, page) in pages.pages().iter().enumerate() {
        let map = viewer
            .maps
            .entry(index)
            .or_insert_with(|| PageMap::build(&world, &page.frame));
        all.insert(index, ranges(map));
    }
    (text, all)
}

#[test]
fn the_maps_of_pages_a_part_is_not_put_into_are_kept_and_moved() {
    // The maps of all pages were made anew after every part: three pages
    // a frame on 312 pages, 3.5 to 4.7 ms. Those of the other pages stay,
    // the text they show moved by what was typed; one that shows some of
    // the part (a heading in the table of contents) is made anew.
    let text = format!(
        "#set page(width: 300pt, height: 200pt)\n#outline()\n\n= First\n\n{}= Second part\n\n{}",
        paragraphs(8),
        paragraphs(4)
    );
    for (at, keys, shown_elsewhere) in [("x_1", "a^2", false), ("Second", "i$x", true)] {
        let (mut viewer, directory) = editing(&text);
        press(&mut viewer, "gsgs");
        if let Some(editor) = viewer.editor.as_mut() {
            editor.set_cursor(text.find(at).unwrap_or(0) + at.len());
        }
        viewer.follow_cursor();
        let (before, then) = maps_now(&mut viewer);
        let parts = viewer.stats.local;
        press(&mut viewer, keys);
        assert!(viewer.stats.local > parts, "{at}: {:?}", viewer.declined);
        let spliced = viewer.document.as_ref().and_then(local::Pages::spliced_at);
        // The page with the cursor is mapped anew when it is followed.
        let kept: Vec<usize> = (viewer.maps.keys().copied())
            .filter(|index| Some(*index) != spliced)
            .collect();
        if shown_elsewhere {
            assert!(!kept.contains(&0), "{at}: the contents kept their map");
        } else {
            assert_eq!(kept.len() + 1, then.len(), "{at}: only {kept:?} kept");
        }
        let (after, now) = maps_now(&mut viewer);
        for index in kept {
            for (old, new) in then[&index].iter().zip(&now[&index]) {
                let (Some(old), Some(new)) = (old, new) else {
                    assert_eq!(old.is_some(), new.is_some(), "{at}: page {index}");
                    continue;
                };
                assert_eq!(
                    before.get(old.clone()),
                    after.get(new.clone()),
                    "{at}: page {index}, {old:?} now {new:?}"
                );
            }
        }
        let _ = std::fs::remove_dir_all(directory);
    }
}

/// What is wrong with the page after `keys`, typed at what `at` finds in
/// `text` (with `files` beside it), if it says it shows the text.
fn after_keys_beside(files: &[(&str, &str)], text: &str, at: &str, keys: &str) -> Option<String> {
    let directory = directory(text);
    for (name, content) in files {
        std::fs::write(directory.join(name), content).unwrap_or_else(|err| panic!("{err}"));
    }
    let (mut viewer, _) = open(&directory);
    press(&mut viewer, "gsgs");
    let found = text
        .find(at)
        .unwrap_or_else(|| panic!("{at:?} is not in the text"));
    if let Some(editor) = viewer.editor.as_mut() {
        editor.set_cursor(found);
    }
    viewer.follow_cursor();
    let mut wrong = None;
    for key in keys::parse(keys).unwrap_or_default() {
        viewer.key(key);
        if viewer.document_revision == Some(viewer.revision) && wrong.is_none() {
            wrong = differs_from_whole(&mut viewer)
                .map(|wrong| format!("{key:?}: {wrong}; parts {}", viewer.stats.local));
        }
    }
    let _ = std::fs::remove_dir_all(directory);
    wrong
}

#[test]
fn parts_of_documents_of_other_shapes_show_what_the_whole_shows() {
    // The starters, slides made with a package, columns made by a rule,
    // a footnote on every page, a page wider than high with margins in
    // percent, and all of the text wrapped in a block by a rule (where
    // the mark of the foot of the text stood at the foot of the block):
    // a part shows what the whole shows, or the page waits for it.
    let starter = |name: &str| match name {
        "article" => include_str!("../../../templates/article.typ"),
        "report" => include_str!("../../../templates/report.typ"),
        "letter" => include_str!("../../../templates/letter.typ"),
        "notes" => include_str!("../../../templates/notes.typ"),
        _ => include_str!("../../../templates/slides.typ"),
    };
    let bib = include_str!("../../../templates/refs.bib");
    let noted = (0..8).fold(String::new(), |mut text, index| {
        use std::fmt::Write as _;
        let _ = write!(
            text,
            "Paragraph {index} of the text, with words#footnote[Note {index}.] enough in it \
             to be broken into lines, and $x_{index}$ in it.\n\n"
        );
        text
    });
    let documents: Vec<(String, &str)> = vec![
        (starter("article").to_owned(), "‹Why the question"),
        (starter("report").to_owned(), "‹What the work is about"),
        (starter("letter").to_owned(), "‹Name of the addressee"),
        (starter("notes").to_owned(), "‹More notes."),
        (starter("slides").to_owned(), "‹What this slide says."),
        (
            include_str!("../../../../zetwerk-corpus/docs/slides.typ").to_owned(),
            "Repeat until",
        ),
        (
            format!(
                "#set page(width: 300pt, height: 300pt)\n#show: columns.with(2)\n\n{}",
                paragraphs(9)
            ),
            "Paragraph 3",
        ),
        (
            format!("#set page(width: 300pt, height: 200pt)\n\n{noted}"),
            "Paragraph 5",
        ),
        (
            format!(
                "#set page(paper: \"a6\", flipped: true, margin: (x: 8%, y: 12%))\n\n{}",
                paragraphs(6)
            ),
            "Paragraph 3",
        ),
        (
            format!(
                "#set page(width: 300pt, height: 200pt)\n#show: body => block(inset: 8pt, body)\n\n{}",
                paragraphs(8)
            ),
            "Paragraph 4",
        ),
        (
            format!(
                "#set page(width: 300pt, height: 200pt)\n#show: body => block(inset: 8pt, body)\n\n{}",
                paragraphs(2)
            ),
            "Paragraph 1",
        ),
    ];
    let mut wrong = Vec::new();
    for (text, at) in &documents {
        for keys in ["i$y", "A<CR>x", "Ax <BS><BS>"] {
            if let Some(why) = after_keys_beside(&[("refs.bib", bib)], text, at, keys) {
                wrong.push(format!("{at:?} {keys:?}: {why}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Where the cursor is drawn on the pages, to a pixel, and where the view
/// begins.
fn cursor_drawn(viewer: &mut Viewer) -> (Vec<[i32; 4]>, f64) {
    let scene = viewer.scene(&|_| false);
    let cursors = [PAGE_CURSOR, PAGE_BAR].map(|rgba| TextureId(hash128(&("tint", rgba))));
    let drawn = scene
        .quads
        .iter()
        .filter(|quad| cursors.contains(&quad.texture))
        .map(|quad| [quad.x, quad.y, quad.width, quad.height].map(|v| v.round() as i32))
        .collect();
    (drawn, viewer.view.top)
}

#[test]
fn the_whole_comes_without_a_jump_where_the_page_breaks_otherwise() {
    // A paragraph begun at the foot of a full page of the paper: what it
    // pushes below the text is left out, and the whole puts more on the
    // next page (a heading keeps to what follows it). When the whole
    // comes, the cursor and the view stay where they are.
    let (mut viewer, directory) = editing(PAPER);
    press(&mut viewer, "gsgs");
    if let Some(editor) = viewer.editor.as_mut() {
        let at = "of the original points.";
        editor.set_cursor(PAPER.find(at).unwrap_or(0) + at.len());
    }
    viewer.follow_cursor();
    press(&mut viewer, "o");
    assert!(viewer.stats.local > 0, "{:?}", viewer.declined);
    press(&mut viewer, "Words");
    let (typed, top) = cursor_drawn(&mut viewer);
    assert_eq!(typed.len(), 1, "{typed:?}");
    compile(&mut viewer);
    let (typeset, now) = cursor_drawn(&mut viewer);
    assert!(
        (top - now).abs() < 0.5,
        "the view moved from {top} to {now}"
    );
    assert_eq!(typeset.len(), 1, "{typeset:?}");
    assert!(
        (typed[0][0] - typeset[0][0]).abs() <= 2 && (typed[0][1] - typeset[0][1]).abs() <= 2,
        "typed {typed:?}, typeset {typeset:?}"
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn what_is_drawn_over_the_page_follows_the_lines_a_part_moves() {
    // Below a paragraph begun by itself: the note on words, the marks of
    // a search, the numbers of the lines, the tags beside them. A frame
    // after the part draws them where the frame after the whole does.
    let text = "#set page(width: 300pt, height: 400pt)\n\
                Before it.\n\n= Head <lab>\n\nSome words of the text. // said\n\n\
                More words here.\n";
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "gsgs/\\=Some<CR>:note look here<CR>");
    press(&mut viewer, "/words<CR>/\\=Before it<CR>A<CR>a");
    assert!(viewer.stats.local > 0, "{:?}", viewer.declined);
    assert_eq!(
        viewer.document_revision,
        Some(viewer.revision),
        "{:?}",
        viewer.declined
    );
    let drawn = |viewer: &mut Viewer| {
        let scene = viewer.scene(&|_| false);
        let height = f64::from(viewer.view.height) as f32;
        let mut quads: Vec<(u128, [i32; 4])> = scene
            .quads
            .iter()
            .filter(|quad| quad.y < height)
            .map(|quad| {
                let place = [quad.x, quad.y, quad.width, quad.height];
                (quad.texture.0, place.map(|v| v.round() as i32))
            })
            .collect();
        quads.sort_unstable();
        quads
    };
    let typed = drawn(&mut viewer);
    compile(&mut viewer);
    let typeset = drawn(&mut viewer);
    let gone: Vec<_> = typed.iter().filter(|q| !typeset.contains(q)).collect();
    let came: Vec<_> = typeset.iter().filter(|q| !typed.contains(q)).collect();
    assert!(
        gone.is_empty() && came.is_empty(),
        "only after the part: {gone:?}; only after the whole: {came:?}"
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn references_of_every_kind_show_in_a_part_what_they_show_in_the_whole() {
    // Stand-ins were tried for one equation, one heading and one figure:
    // here headings of two levels, two equations, a table, a figure of
    // something else (which has none, and is typeset whole), several in
    // one paragraph, and one in an item of a list.
    let text = "#set page(width: 300pt, height: 500pt)\n#set heading(numbering: \"1.1\")\n\
                #set math.equation(numbering: \"(1)\")\n\n= Start <s1>\n\n== Sub <s2>\n\nWords.\n\n\
                $ a = b $ <e1>\n\n$ c = d $ <e2>\n\n\
                #figure(table(columns: 1, [x]), caption: [T]) <t1>\n\n\
                #figure(rect(width: 10pt), caption: [R]) <r1>\n\n= Next <s3>\n\n\
                See @s1, @s2, @e1, @e2 and @t1, with $x$ in it.\n\nThen @r1 and $y$ here.\n\n\
                - An item with @e2 and $z$.\n\nLast words.\n";
    for at in ["$x", "$y", "$z"] {
        assert_eq!(after_keys(text, at, true, "a^2"), None, "{at}");
    }
}
