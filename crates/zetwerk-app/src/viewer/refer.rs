//! The reference helper: what there is to refer to, in one list.
//!
//! `@` while typing asks "what do you want to refer to?": the sections,
//! figures, tables and equations of the document and the sources of its
//! bibliography come up, each with what kind of thing it is, its number
//! and what it is called. Typing narrows the list by label and by what
//! the thing is called; Enter puts the reference in. What has no label
//! yet is listed all the same: choosing it gives it one, made of its
//! kind and its title, and refers to it, as one step to undo.
//!
//! A reference shows a number. A section where sections have none is
//! referred to by its title instead, and an equation without a number
//! gets one: a reference Typst would refuse is never put in.
//!
//! `:references` lists what is wrong with labels and references; `gd`
//! on a reference goes to what it refers to, `K` says what that is.
//!
//! What the things are is read off the text (see [`crate::ref_text`]),
//! so that it is right for what was typed a moment ago; their numbers
//! are the compiled document's, looked up once for each compile. The
//! list itself is made once for each `@`: a key that narrows it only
//! looks through what is listed.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};

use typst::foundations::{Content, NativeElement, Repr, Smart, StyleChain};
use typst::introspection::{Count, CounterState, CounterUpdate, CounterUpdateElem};
use typst::math::EquationElem;
use typst::model::{FigureElem, FigureKind, HeadingElem, Numbering, Refable, TableElem};
use typst::syntax::{FileId, Source, Span};
use typst::text::RawElem;
use typst::{World, WorldExt};
use typst_layout::PagedDocument;
use zetwerk_typst::{Candidate, ProjectWorld};

use super::agent::line_column;
use super::assist::{Completion, Entry, Target as Goes, cut, score};
use super::{Edited, Viewer};
use crate::editor;
use crate::keys::{Key, KeyPress};
use crate::ref_text::{self, Fault, Kind, Marks};
use crate::typst_text::{self, Ground};

/// How much of a label, and of what is said about it, a row shows.
const ROW_LABEL: usize = 40;
const ROW_ABOUT: usize = 64;

/// What the helper keeps from one key to the next.
#[derive(Default)]
pub(super) struct Refer {
    /// What the compiled document says of its things.
    known: Option<Known>,
    /// The sources of the bibliography, as last read.
    sources: Option<Bibliography>,
    /// The list that is up, while one reference is being typed.
    listed: Option<Listed>,
}

/// What the compiled document says of the things that can be referred
/// to: looked up once, when first asked for.
struct Known {
    /// The document it is said of.
    document: Weak<PagedDocument>,
    /// By the node each thing was written as: whether a reference to it
    /// shows a number, and the number.
    numbers: HashMap<Span, (bool, Option<String>)>,
    /// The things that have a label, in the order of the document.
    labelled: Vec<Told>,
    /// Every label that is on something, with the file that thing is
    /// written in.
    labels: Vec<(String, Option<FileId>)>,
    /// The keys of the sources its bibliographies hold, each with its
    /// title.
    keys: Vec<(String, Option<String>)>,
    /// Whether any heading has a number, and any equation.
    sections_numbered: bool,
    equations_numbered: bool,
    /// Whether the numbers and the things labelled are told: counting
    /// them takes long on a long document, and what labels there are
    /// (looked at after every compile) does not need it.
    counted: bool,
    /// The whole document the numbers were counted for: pages a part
    /// typed was put into have those of the whole they were put into.
    whole: Weak<PagedDocument>,
}

/// The sources of the document's bibliography file, as they were when
/// the pages were made: read again only if the file has changed since.
/// (Which file it is takes a walk through the text to tell; the pages
/// are made anew whenever that changes.)
struct Bibliography {
    path: std::path::PathBuf,
    /// When the file was last written, if there is one.
    written: Option<std::time::SystemTime>,
    entries: Vec<crate::bib::Entry>,
}

/// When the file at `path` was last written; `None` if there is none.
fn written_at(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path)
        .and_then(|file| file.modified())
        .ok()
}

/// A thing with a label, as the compiled document has it.
struct Told {
    kind: Kind,
    label: String,
    numbered: bool,
    number: Option<String>,
    called: String,
    /// The node it was written as: its file, and where in it, is asked
    /// of that only when somebody wants to go there.
    span: Span,
}

/// Something to refer to, as the list offers it.
#[derive(Debug, Clone)]
struct Offer {
    kind: Kind,
    /// Its label; for one that has none, the label it is to get.
    label: String,
    /// Whether it has that label already.
    labelled: bool,
    /// Whether a reference to it shows a number, and the number, as far
    /// as the compiled document knows it.
    numbered: bool,
    number: Option<String>,
    called: String,
    /// Which of the things of the file being edited it is, counting
    /// from 0, and the byte it begins at; `None` for what is elsewhere.
    here: Option<(usize, usize)>,
    /// Whether it is an equation written between dollar signs in that
    /// file: one that can be given a number.
    bare: bool,
    /// The node it was written as, where that is in another file.
    elsewhere: Option<Span>,
}

/// What choosing an [`Offer`] takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Way {
    /// Its label is put in: `@label`.
    Plain,
    /// A link that reads as its title is put in: it has no number.
    Named,
    /// It is changed first: it gets a label, or a number, or both.
    Make,
}

/// What was chosen of the list, and what is to be done for it.
enum Chosen {
    /// This label is put in behind the `@`.
    Label(String),
    /// This takes the place of the `@` and what is typed behind it.
    Named(String),
    /// The thing is changed first.
    Make(Offer),
}

impl Offer {
    /// What choosing it takes: a section without a number cannot be
    /// referred to by one, an equation without a number can get one
    /// where it is written between dollar signs in the file being
    /// edited, and what has no label needs one.
    fn way(&self) -> Way {
        match (self.labelled, self.numbered, self.kind) {
            (false, _, _) => Way::Make,
            (true, false, Kind::Equation) if self.bare => Way::Make,
            (true, false, Kind::Section) if !self.called.is_empty() => Way::Named,
            _ => Way::Plain,
        }
    }

    /// What is written where the cursor is to refer to it.
    fn reference(&self) -> String {
        let shows_number = self.numbered || (self.kind == Kind::Equation && self.bare);
        ref_text::reference(self.kind, &self.label, &self.called, shows_number)
    }

    /// What the list knows it by: no two are known by the same.
    fn key(&self) -> String {
        match self.way() {
            Way::Plain => self.label.clone(),
            Way::Named | Way::Make => self.reference(),
        }
    }

    /// Its kind and its number: `section 2.1`.
    fn numbered_kind(&self) -> String {
        match &self.number {
            Some(number) => format!("{} {number}", self.kind.word()),
            None => self.kind.word().to_owned(),
        }
    }

    /// What it is, in a line: `section 2.1 · Main result`.
    fn told(&self) -> String {
        match self.called.as_str() {
            "" => self.numbered_kind(),
            called => format!("{} · {called}", self.numbered_kind()),
        }
    }

    /// Its row in the list: what it is listed as, and a word about it.
    fn row(&self) -> (String, String) {
        if self.labelled {
            return (cut(&self.label, ROW_LABEL), cut(&self.told(), ROW_ABOUT));
        }
        let called = match self.called.as_str() {
            "" => "(nothing written yet)",
            called => called,
        };
        let about = format!("{} · no label yet", self.numbered_kind());
        (cut(called, ROW_LABEL), about)
    }

    /// What choosing it does, where that is more than putting in its
    /// label.
    fn detail(&self) -> Option<String> {
        let label = &self.label;
        let told = match (self.way(), self.kind) {
            (Way::Plain, _) => return None,
            (Way::Named, _) => {
                "sections have no numbers: it is referred to by its title".to_owned()
            }
            (Way::Make, _) if self.labelled => {
                "Enter gives it a number and refers to it".to_owned()
            }
            (Way::Make, Kind::Section) if !self.numbered => {
                format!("Enter gives it the label {label} and refers to it by its title")
            }
            (Way::Make, Kind::Equation) if !self.numbered && self.bare => {
                format!("Enter gives it the label {label} and a number, and refers to it")
            }
            (Way::Make, _) => format!("Enter gives it the label {label} and refers to it"),
        };
        Some(told)
    }

    /// How well it goes with what was `typed` behind the `@`: by its
    /// label first, then by what it is called. `None` if not at all.
    fn found(&self, typed: &str) -> Option<i32> {
        if typed.is_empty() {
            return Some(1001);
        }
        let lower = typed.to_lowercase();
        let label = self.label.to_lowercase();
        let begins = |text: &str| {
            text.split(|c: char| !c.is_alphanumeric())
                .any(|word| word.starts_with(&lower))
        };
        // The label a thing is to get is no more than a word for its
        // kind: nobody knows it yet.
        let of_kind = [self.kind.word(), self.kind.prefix()]
            .iter()
            .any(|word| word.starts_with(&lower));
        if self.labelled && label.starts_with(&lower) {
            Some(1000 + i32::from(self.label.starts_with(typed)))
        } else if self.labelled && begins(&label) {
            Some(900)
        } else if begins(&self.called.to_lowercase()) {
            Some(800)
        } else if of_kind || label.starts_with(&lower) {
            Some(700)
        } else {
            let by_label = score(&self.label, typed).map(|score| 400 + score.clamp(0, 250));
            let by_name = score(&self.called, typed).map(|score| 100 + score.clamp(0, 250));
            by_label.max(by_name)
        }
    }

    /// It as one of the things that could be typed behind an `@`.
    fn candidate(&self) -> Candidate {
        let (label, _) = self.row();
        let text = self.key();
        Candidate {
            label,
            kind: self.kind.word(),
            detail: self.detail(),
            cursor: text.len(),
            text,
        }
    }
}

/// The list for one reference being typed: made when its `@` is typed,
/// and only looked through while more is typed behind it.
struct Listed {
    file: FileId,
    /// The byte of the `@`, and how long the text is without what is
    /// typed behind it: as long as both stay, the list is good.
    at: usize,
    rest: usize,
    /// The compiled document the numbers are of.
    document: Option<Weak<PagedDocument>>,
    offers: Vec<Offer>,
    /// Each offer as one of the things that could be typed.
    candidates: Vec<Candidate>,
    /// The offers by what each is known by.
    keys: HashMap<String, usize>,
    /// The labels Typst itself knows that are none of the offers: of
    /// things that are no section, figure or equation.
    others: Vec<Candidate>,
}

/// Something of the document that can be referred to, for an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Referable {
    /// `section`, `figure`, `table`, `listing`, `equation` or `source`.
    pub kind: &'static str,
    /// Its label: what `@` is to be followed by. `None` for a thing
    /// that has none yet.
    pub label: Option<String>,
    /// Its number as a reference shows it, if the compiled document
    /// knows it.
    pub number: Option<String>,
    /// Whether it has a number at all: a section or an equation that
    /// has none cannot be referred to as `@label`.
    pub numbered: bool,
    /// Its title, its caption, the equation as written; of a source,
    /// who wrote it, when, and what it is called.
    pub called: String,
    /// The file it is in, where that is not the file being edited.
    pub file: Option<String>,
    /// The line it begins on, counting from 1.
    pub line: Option<usize>,
}

/// Advances the state of a counter by `update`. A state that only
/// running the document's code could tell is not known from then on.
fn advance(state: &mut Option<CounterState>, update: CounterUpdate) {
    match update {
        CounterUpdate::Set(to) => *state = Some(to),
        CounterUpdate::Step(level) => {
            if let Some(state) = state {
                state.step(level, 1);
            }
        }
        CounterUpdate::Func(_) => *state = None,
    }
}

/// `numbers` as `numbering` shows them; plainly, with dots between
/// them, where only running the document's code could tell.
fn shown(numbering: &Numbering, numbers: &[u64]) -> String {
    let plain = || {
        let numbers: Vec<String> = numbers.iter().map(u64::to_string).collect();
        numbers.join(".")
    };
    match numbering {
        Numbering::Pattern(pattern) => pattern
            .apply(None, numbers)
            .map_or_else(|_| plain(), |shown| shown.to_string()),
        Numbering::Func(_) => plain(),
    }
}

/// Where the node `span` is written: the file, and the line (counting
/// from 1) and the byte of it as it is written. Finding a node takes a
/// walk through its file: this is for the one thing somebody asks
/// about, not for all there are.
fn place_of(span: Span, world: Option<&ProjectWorld>) -> Option<(FileId, usize, usize)> {
    let (file, world) = (span.id()?, world?);
    let at = world.range(span)?.start;
    let line = line_column(world.source(file).ok()?.text(), at).0;
    Some((file, line, world.written_at(file, at)))
}

/// What kind of thing to refer to `element` is and what it is called,
/// if it is one.
fn referable(element: &Content) -> Option<(Kind, String)> {
    let styles = StyleChain::default();
    if let Some(heading) = element.to_packed::<HeadingElem>() {
        return Some((Kind::Section, heading.body.plain_text().to_string()));
    }
    if let Some(figure) = element.to_packed::<FigureElem>() {
        let kind = match figure.kind.get_cloned(styles) {
            Smart::Custom(FigureKind::Elem(held)) if held == TableElem::ELEM => Kind::Table,
            Smart::Custom(FigureKind::Elem(held)) if held == RawElem::ELEM => Kind::Listing,
            _ => Kind::Figure,
        };
        let caption = figure.caption.get_ref(styles).as_ref();
        let called = caption.map(|caption| caption.body.plain_text().to_string());
        return Some((kind, called.unwrap_or_default()));
    }
    let equation = element.to_packed::<EquationElem>()?;
    if !equation.block.get(styles) {
        return None;
    }
    // Its letters and signs in a row: not as it is written (that is
    // read off the text, for the file being edited), but enough to
    // know it by.
    let read = equation.body.plain_text();
    Some((
        Kind::Equation,
        read.split_whitespace().collect::<Vec<_>>().join(" "),
    ))
}

/// Looks up what `document` says of the things that can be referred to.
/// One walk over what it can find again (its headings, figures,
/// equations and the like, not its pages): the numbers are counted as
/// Typst counts them; only with `count` (else only the labels and the
/// keys of the sources are told).
fn know(document: &Arc<PagedDocument>, count: bool) -> Known {
    let mut known = Known {
        document: Arc::downgrade(document),
        numbers: HashMap::new(),
        labelled: Vec::new(),
        labels: Vec::new(),
        keys: zetwerk_typst::bibliography_keys(document),
        sections_numbered: false,
        equations_numbered: false,
        counted: count,
        whole: Arc::downgrade(document),
    };
    let mut counters: HashMap<String, Option<CounterState>> = HashMap::new();
    let begun = || Some(CounterState::init(false));
    for element in document.introspector().elements().all() {
        let label = element
            .label()
            .map(|label| label.resolve().as_str().to_owned());
        if let Some(label) = &label {
            known.labels.push((label.clone(), element.span().id()));
        }
        if !count {
            continue;
        }
        let update = element.with::<dyn Count>().and_then(Count::update);
        if element.elem() == CounterUpdateElem::ELEM {
            // Which counter it is for is told by what it is called.
            let key = element.get_by_name("key").map(|key| key.repr());
            if let (Ok(key), Some(update)) = (key, update) {
                let state = counters
                    .entry(format!("counter({key})"))
                    .or_insert_with(begun);
                advance(state, update);
            }
            continue;
        }
        let (Some((kind, called)), Some(refable)) =
            (referable(element), element.with::<dyn Refable>())
        else {
            continue;
        };
        let state = counters
            .entry(refable.counter().repr().to_string())
            .or_insert_with(begun);
        if let Some(update) = update {
            advance(state, update);
        }
        let numbering = refable.numbering();
        let number = match (numbering, &state) {
            (Some(numbering), Some(state)) => Some(shown(numbering, &state.0)),
            _ => None,
        };
        let numbered = numbering.is_some();
        known.sections_numbered |= numbered && kind == Kind::Section;
        known.equations_numbered |= numbered && kind == Kind::Equation;
        known
            .numbers
            .entry(element.span())
            .or_insert_with(|| (numbered, number.clone()));
        if let Some(label) = label {
            known.labelled.push(Told {
                kind,
                label,
                numbered,
                number,
                called,
                span: element.span(),
            });
        }
    }
    known
}

/// The byte of the `@` of the reference that is being typed, if the
/// cursor at byte `cursor` of `source` is at the end of one.
fn being_typed(source: &Source, cursor: usize) -> Option<usize> {
    let before = source.text().get(..cursor)?;
    let name = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | ':' | '.');
    let at = before.trim_end_matches(name).len().checked_sub(1)?;
    let begins = before.get(at..)?.starts_with('@')
        && (!before.get(..at)?.ends_with('\\') || asked(source, at));
    (begins && typst_text::ground(source, at) == Ground::Markup).then_some(at)
}

/// Whether the `@` at byte `at` of `source` is written `\@` to be shown
/// as it is while a reference is asked for: as `@` is typed on a page
/// (see [`typst_text::typed`]), not behind a letter (an address).
fn asked(source: &Source, at: usize) -> bool {
    let text = source.text();
    let Some(sign) = at.checked_sub(1) else {
        return false;
    };
    let escape = typst::syntax::LinkedNode::new(source.root())
        .leaf_at(sign, typst::syntax::Side::After)
        .is_some_and(|leaf| {
            leaf.kind() == typst::syntax::SyntaxKind::Escape && leaf.range() == (sign..at + 1)
        });
    escape && !text[..sign].ends_with(char::is_alphanumeric)
}

/// Where the reference whose `@` is at byte `at` of `source` begins: at
/// its backslash, where it is asked for.
fn reference_start(source: &Source, at: usize) -> usize {
    if source.text()[..at].ends_with('\\') {
        at - 1
    } else {
        at
    }
}

/// `reference`, put in where the cursor is at byte `cursor` of `text`:
/// with a backslash behind it where `text` goes on there with what would
/// be more of its name.
fn sealed(text: &str, cursor: usize, reference: String) -> String {
    let goes_on = text[cursor..]
        .chars()
        .next()
        .is_some_and(|c| typst::syntax::is_id_continue(c) || matches!(c, '.' | ':'));
    if goes_on && reference.starts_with('@') {
        format!("{reference}\\")
    } else {
        reference
    }
}

/// The offers for the things `marks` found in the file being edited,
/// and for what else `known` and `sources` have. `rules` is what the
/// document's main file says of numbers, where that is another file.
fn offers_of(
    marks: &Marks,
    file: FileId,
    known: Option<&Known>,
    rules: (Option<bool>, Option<bool>),
    sources: &[crate::bib::Entry],
) -> Vec<Offer> {
    let mut taken: HashSet<String> = marks.defined.iter().map(|(name, _)| name.clone()).collect();
    taken.extend(sources.iter().map(|entry| entry.key.clone()));
    if let Some(known) = known {
        taken.extend(known.labels.iter().map(|(label, _)| label.clone()));
        taken.extend(known.keys.iter().map(|(key, _)| key.clone()));
    }
    let sections = marks.sections_numbered.or(rules.0);
    let equations = marks.equations_numbered.or(rules.1);
    let mut offers = Vec::new();
    for (index, target) in marks.targets.iter().enumerate() {
        let said = known.and_then(|known| known.numbers.get(&target.span));
        // What was written since the pages were made is numbered as
        // the rest of its kind is.
        let numbered = said.map_or_else(
            || match target.kind {
                Kind::Section => {
                    sections.unwrap_or(known.is_some_and(|known| known.sections_numbered))
                }
                Kind::Equation => target
                    .numbers
                    .or(equations)
                    .unwrap_or_else(|| known.is_some_and(|known| known.equations_numbered)),
                _ => true,
            },
            |(numbered, _)| *numbered,
        );
        let label = target.label.clone().unwrap_or_else(|| {
            let made =
                ref_text::label_for(target.kind, &target.called, &|label| taken.contains(label));
            taken.insert(made.clone());
            made
        });
        offers.push(Offer {
            kind: target.kind,
            label,
            labelled: target.label.is_some(),
            numbered,
            number: said.and_then(|(_, number)| number.clone()),
            called: target.called.clone(),
            here: Some((index, target.range.start)),
            bare: target.kind == Kind::Equation && target.numbers.is_none(),
            elsewhere: None,
        });
    }
    let here: HashSet<&str> = marks
        .defined
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    let told = known.map_or(&[][..], |known| &known.labelled);
    for told in told {
        if told.span.id() != Some(file) && !here.contains(told.label.as_str()) {
            offers.push(Offer {
                kind: told.kind,
                label: told.label.clone(),
                labelled: true,
                numbered: told.numbered,
                number: told.number.clone(),
                called: told.called.clone(),
                here: None,
                bare: false,
                elsewhere: Some(told.span),
            });
        }
    }
    offers.extend(sources.iter().map(|entry| Offer {
        kind: Kind::Source,
        label: entry.key.clone(),
        labelled: true,
        numbered: true,
        number: None,
        called: entry.line(),
        here: None,
        bare: false,
        elsewhere: None,
    }));
    offers
}

impl Viewer {
    /// Brings what is known of the compiled document up to date with
    /// the document there is; with `count`, its numbers too.
    fn refresh_known(&mut self, count: bool) {
        let Some(document) = self.document.as_ref().map(super::local::Pages::whole) else {
            self.refer.known = None;
            return;
        };
        let same = |known: &Known| {
            known.document.as_ptr() == Arc::as_ptr(document) && (known.counted || !count)
        };
        if self.refer.known.as_ref().is_some_and(same) {
            return;
        }
        // While a part is typed into the pages, what is counted is
        // that of the whole they were made of: counting it again with
        // every key that puts a part in takes long on a long document.
        let whole = self.whole.as_ref().map_or(std::ptr::null(), Arc::as_ptr);
        let counted = self.refer.known.take().filter(|known| {
            count && known.counted && known.whole.as_ptr() == whole && !whole.is_null()
        });
        let mut known = know(document, count && counted.is_none());
        if let Some(counted) = counted {
            known.numbers = counted.numbers;
            known.labelled = counted.labelled;
            known.sections_numbered = counted.sections_numbered;
            known.equations_numbered = counted.equations_numbered;
            known.counted = true;
            known.whole = counted.whole;
        } else if let Some(whole) = &self.whole {
            known.whole = Arc::downgrade(whole);
        }
        self.refer.known = Some(known);
        self.refer.sources = None;
    }

    /// The sources of the document's bibliography file; none if it has
    /// no such file (sources kept otherwise are Typst's to list).
    fn bibliography(&mut self) -> Vec<crate::bib::Entry> {
        if let Some(kept) = &self.refer.sources
            && kept.written == written_at(&kept.path)
        {
            return kept.entries.clone();
        }
        let Ok(sources) = self.sources() else {
            return Vec::new();
        };
        // Kept while there are pages to say when to ask again.
        if self.refer.known.is_some() {
            self.refer.sources = Some(Bibliography {
                written: written_at(&sources.path),
                path: sources.path,
                entries: sources.entries.clone(),
            });
        }
        sources.entries
    }

    /// All there is to refer to, as the text and the compiled document
    /// are now.
    fn offers(&mut self) -> Vec<Offer> {
        self.refresh_known(true);
        let sources = self.bibliography();
        let Some(editor) = &self.editor else {
            return Vec::new();
        };
        let (source, file) = (editor.source(), editor.file());
        let marks = ref_text::read(source);
        // A chapter is numbered as the file that includes it says.
        let main = self.project.source(self.project.main());
        let rules = main
            .filter(|main| main.id() != file)
            .map(ref_text::read)
            .map_or((None, None), |main| {
                (main.sections_numbered, main.equations_numbered)
            });
        offers_of(&marks, file, self.refer.known.as_ref(), rules, &sources)
    }

    /// Makes the list for the reference whose `@` is at byte `at`.
    fn list(&mut self, at: usize, rest: usize) -> Option<Listed> {
        let offers = self.offers();
        let editor = self.editor.as_ref()?;
        let (source, cursor, file) = (editor.source().clone(), editor.cursor(), editor.file());
        let mut keys = HashMap::new();
        let mut labels = HashSet::new();
        let candidates: Vec<Candidate> = offers.iter().map(Offer::candidate).collect();
        for (index, (offer, candidate)) in offers.iter().zip(&candidates).enumerate() {
            keys.entry(candidate.text.clone()).or_insert(index);
            if offer.labelled {
                labels.insert(offer.label.as_str());
            }
        }
        // What else Typst knows a label of: asked as if the `@` were not
        // written to show as itself.
        let (asked_of, cursor) = if asked(&source, at) {
            let mut code = source.clone();
            code.edit(at - 1..at, "");
            (code, cursor - 1)
        } else {
            (source.clone(), cursor)
        };
        self.project.set_source(asked_of.clone());
        let world = self.project.snapshot();
        self.project.set_source(source.clone());
        let document = self.document.as_ref().map(|pages| &**pages.whole());
        let own = zetwerk_typst::complete(&world, document, &asked_of, cursor, false);
        let mut seen = HashSet::new();
        let others = own
            .map(|(_, candidates)| candidates)
            .unwrap_or_default()
            .into_iter()
            .filter(|candidate| candidate.kind == "label")
            .filter(|candidate| !labels.contains(candidate.label.as_str()))
            .filter(|candidate| seen.insert(candidate.label.clone()))
            .collect();
        Some(Listed {
            file,
            at,
            rest,
            document: self
                .document
                .as_ref()
                .map(|pages| Arc::downgrade(pages.whole())),
            offers,
            candidates,
            keys,
            others,
        })
    }

    /// Brings up the list of what can be referred to, if a reference is
    /// being typed where the cursor is. Returns whether one is: the
    /// list of what could be typed is this one's to make then.
    pub(super) fn refer_complete(&mut self) -> bool {
        let found = self.editor.as_ref().and_then(|editor| {
            let (source, cursor) = (editor.source(), editor.cursor());
            let at = being_typed(source, cursor)?;
            let typed = source.text().get(at + 1..cursor)?.to_owned();
            let start = reference_start(source, at);
            Some((at, typed, source.text().len(), editor.file(), start))
        });
        let Some((at, typed, length, file, start)) = found else {
            self.refer.listed = None;
            return false;
        };
        let rest = length - typed.len();
        let document = self
            .document
            .as_ref()
            .map(|pages| Arc::as_ptr(pages.whole()));
        let good = self.refer.listed.as_ref().is_some_and(|listed| {
            let pages = listed.document.as_ref().map(Weak::as_ptr);
            (listed.file, listed.at, listed.rest, pages) == (file, at, rest, document)
        });
        if !good {
            self.refer.listed = self.list(at, rest);
        }
        let Some(listed) = &self.refer.listed else {
            return true;
        };
        let mut entries: Vec<(i32, Entry)> = Vec::new();
        for (offer, candidate) in listed.offers.iter().zip(&listed.candidates) {
            if let Some(score) = offer.found(&typed) {
                // Its label takes the place of what was typed; anything
                // else, that of the `@` too.
                let from = match offer.way() {
                    Way::Plain => at + 1,
                    Way::Named | Way::Make => start,
                };
                let entry = Entry {
                    candidate: candidate.clone(),
                    from,
                    insert: None,
                };
                entries.push((score, entry));
            }
        }
        for candidate in &listed.others {
            let begins = candidate
                .label
                .to_lowercase()
                .starts_with(&typed.to_lowercase())
                .then_some(1000 + i32::from(candidate.label.starts_with(&typed)));
            if let Some(score) = begins.or_else(|| score(&candidate.label, &typed)) {
                let entry = Entry {
                    candidate: candidate.clone(),
                    from: at + 1,
                    insert: None,
                };
                entries.push((score, entry));
            }
        }
        // The best first; among equals, in the order of the document.
        entries.sort_by_key(|(score, _)| Reverse(*score));
        self.completion = (!entries.is_empty()).then(|| Completion {
            candidates: entries.into_iter().map(|(_, entry)| entry).collect(),
            chosen: 0,
        });
        true
    }

    /// The offer that `candidate` of the list on screen stands for.
    fn offer_of(&self, candidate: &Candidate) -> Option<&Offer> {
        let listed = self.refer.listed.as_ref()?;
        let offer = listed.offers.get(*listed.keys.get(&candidate.text)?)?;
        (offer.kind.word() == candidate.kind).then_some(offer)
    }

    /// The row of `candidate` in the list on screen, if it is something
    /// to refer to: what it is listed as, and its kind, its number and
    /// what it is called.
    pub(super) fn refer_row(&self, candidate: &Candidate) -> Option<(String, String)> {
        self.offer_of(candidate).map(Offer::row)
    }

    /// A key that may choose something to refer to: Enter, Tab or
    /// Ctrl-Y on one of the list. Returns whether it did.
    pub(super) fn refer_key(&mut self, press: KeyPress) -> bool {
        let chooses = matches!(
            (press.key, press.ctrl),
            (Key::Enter | Key::Tab, false) | (Key::Char('y'), true)
        );
        let typing = self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.mode() == editor::Mode::Insert);
        if !(chooses && typing) {
            return false;
        }
        let chosen = self.completion.as_ref().and_then(|completion| {
            let candidate = &completion.candidates.get(completion.chosen)?.candidate;
            let at = self.refer.listed.as_ref()?.at;
            let chosen = match self.offer_of(candidate) {
                Some(offer) => match offer.way() {
                    Way::Plain => Chosen::Label(offer.label.clone()),
                    Way::Named => Chosen::Named(offer.reference()),
                    Way::Make => Chosen::Make(offer.clone()),
                },
                // A label Typst knows, of a thing that is none of
                // these: put in like any other.
                None if candidate.kind == "label" => Chosen::Label(candidate.text.clone()),
                None => return None,
            };
            Some((chosen, at))
        });
        let (Some((chosen, at)), Some(editor)) = (chosen, &mut self.editor) else {
            return false;
        };
        let cursor = editor.cursor();
        let Some(typed) = editor.text().get(at + 1..cursor).map(str::to_owned) else {
            return false;
        };
        // Asked for on a page, the `@` is written to show as itself
        // until something is chosen: it goes with what was typed.
        let start = reference_start(editor.source(), at);
        let signs = at + 1 - start;
        match chosen {
            // All of it is typed already: the key is for the text.
            Chosen::Label(label) if typed == label && signs == 1 => return false,
            Chosen::Label(label) => {
                let reference = sealed(editor.text(), cursor, format!("@{label}"));
                let within = label.len() + 1;
                let replaced = typed.chars().count() + signs;
                let press = editor.completed(replaced, reference, within);
                self.completion = None;
                self.editor_key(press);
            }
            Chosen::Named(reference) => {
                let within = reference.len();
                let reference = sealed(editor.text(), cursor, reference);
                let press = editor.completed(typed.chars().count() + signs, reference, within);
                self.completion = None;
                self.editor_key(press);
                self.notice(
                    "sections have no numbers: referred to by its title  \
                     (:numbering on numbers them)",
                );
            }
            Chosen::Make(offer) => {
                self.completion = None;
                self.refer_make(&offer, at);
            }
        }
        true
    }

    /// A key that ends a reference asked for on a page and typed out
    /// without choosing from the list: a blank or a mark, Escape, Enter
    /// or Tab where nothing is listed. Where something is labelled with
    /// the name typed, it is referred to; else what was typed stays as it
    /// shows, an `@` and a word.
    pub(super) fn refer_end(&mut self, press: KeyPress) {
        let ends = match press.key {
            Key::Escape => true,
            Key::Enter | Key::Tab => self.completion.is_none(),
            Key::Char(c) => {
                !(press.ctrl || typst::syntax::is_id_continue(c) || matches!(c, '.' | ':'))
            }
            _ => false,
        };
        let Some(editor) = self
            .editor
            .as_mut()
            .filter(|editor| ends && editor.mode() == editor::Mode::Insert)
        else {
            return;
        };
        let (source, cursor) = (editor.source(), editor.cursor());
        let Some(at) = being_typed(source, cursor).filter(|at| asked(source, *at)) else {
            return;
        };
        let text = source.text();
        let typed = &text[at + 1..cursor];
        let name = typed.trim_end_matches(['.', ':']);
        let Some(listed) = self.refer.listed.as_ref().filter(|listed| listed.at == at) else {
            return;
        };
        let offer = listed
            .offers
            .iter()
            .find(|offer| offer.labelled && offer.label == name);
        let reference = match offer.map(Offer::way) {
            Some(Way::Plain) => format!("@{typed}"),
            Some(Way::Named) => {
                offer.map(Offer::reference).unwrap_or_default() + &typed[name.len()..]
            }
            None if listed.others.iter().any(|other| other.label == name) => format!("@{typed}"),
            Some(Way::Make) | None => return,
        };
        if name.is_empty() {
            return;
        }
        let within = reference.len();
        let reference = sealed(text, cursor, reference);
        let replaced = typed.chars().count() + 2;
        let press = editor.completed(replaced, reference, within);
        self.editor_key(press);
    }

    /// Makes `offer`, a thing of the file being edited, something a
    /// reference can be to (gives it a label, an equation a number) and
    /// refers to it where the reference begun at byte `at` is being
    /// typed: both as one step to undo.
    fn refer_make(&mut self, offer: &Offer, at: usize) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let (text, cursor) = (editor.text(), editor.cursor());
        // The text as it is now says where the thing is.
        let marks = ref_text::read(editor.source());
        let target = offer
            .here
            .and_then(|(index, _)| marks.targets.get(index))
            .filter(|target| {
                (target.kind, &target.called) == (offer.kind, &offer.called)
                    && target.label.is_some() == offer.labelled
            });
        let Some(target) = target else {
            self.alert("the text has changed since the list came up: @ asks again");
            return;
        };
        let label = (!offer.labelled).then_some(offer.label.as_str());
        let number = offer.kind == Kind::Equation && !offer.numbered;
        let reference = offer.reference();
        let length = reference.len();
        let start = reference_start(editor.source(), at);
        let mut edits = vec![(start..cursor, sealed(text, cursor, reference))];
        edits.extend(ref_text::prepared(text, target, label, number));
        // Where the reference will begin: behind what is put in before
        // it.
        let shift: isize = edits
            .iter()
            .filter(|(range, _)| range.end <= start && range.start < start)
            .map(|(range, with)| with.len().cast_signed() - range.len().cast_signed())
            .sum();
        let sealed = edits[0].1.len() > length;
        // From the last to the first, so that the ranges stay good.
        edits.sort_by_key(|(range, _)| Reverse(range.start));
        let edits: Vec<(std::ops::Range<usize>, &str)> = edits
            .iter()
            .map(|(range, with)| (range.clone(), with.as_str()))
            .collect();
        if let Err(why) = editor.replace(&edits) {
            self.alert(format!("not referred to: {why}"));
            return;
        }
        // Before the backslash that keeps the text behind out of its name.
        if sealed {
            editor.set_cursor(start.saturating_add_signed(shift) + length);
        }
        self.text_changed(Edited::Command);
        let thing = match offer.called.as_str() {
            "" => format!("the {}", offer.kind.word()),
            called => format!("the {} “{}”", offer.kind.word(), cut(called, ROW_LABEL)),
        };
        let numbered = if number {
            ", which has a number now"
        } else {
            ""
        };
        self.notice(if offer.labelled {
            format!("referred to {thing}{numbered}  (u takes it back)")
        } else {
            format!(
                "{} is the label of {thing} now{numbered}  (u takes it back)",
                offer.label
            )
        });
    }

    /// The labels there are outside the file being edited, whose labels
    /// and references are `marks`, and the labels referred to outside
    /// it. With `files` the other files of the document are read for
    /// them; without, only what is known anyway is given, and nothing
    /// of what is referred to.
    fn besides(&mut self, marks: &Marks, files: bool) -> (Vec<String>, HashSet<String>) {
        self.refresh_known(false);
        let sources = self.bibliography();
        let mut labels: Vec<String> = sources.into_iter().map(|entry| entry.key).collect();
        let Some(editor) = &self.editor else {
            return (Vec::new(), HashSet::new());
        };
        let file = editor.file();
        let mut used = HashSet::new();
        if let Some(known) = &self.refer.known {
            labels.extend(known.keys.iter().map(|(key, _)| key.clone()));
            // A label the pages have on something of this file that is
            // not written here was made by code, or has been taken out
            // since: if the pages are those of this text, or the text
            // makes labels by code, the former.
            let trusted = self.document_revision == Some(self.revision) || marks.makes_labels;
            let here: HashSet<&str> = marks
                .defined
                .iter()
                .map(|(name, _)| name.as_str())
                .collect();
            let outside = known.labels.iter().filter(|(label, of)| {
                *of != Some(file) || (trusted && !here.contains(label.as_str()))
            });
            labels.extend(outside.map(|(label, _)| label.clone()));
        }
        if files {
            let world = self.project.snapshot();
            for path in self.project.dependencies() {
                let is_text = path.extension().is_some_and(|ending| ending == "typ");
                let Some(id) = self
                    .project
                    .file_id(&path)
                    .filter(|id| *id != file && is_text)
                else {
                    continue;
                };
                // As it is open here, if it is; else as it is on disk.
                let open = self.others.iter().find(|other| other.file() == id);
                let source = open.map(|other| other.source().clone());
                let Some(source) = source.or_else(|| world.source(id).ok()) else {
                    continue;
                };
                let other = ref_text::read(&source);
                labels.extend(other.defined.into_iter().map(|(name, _)| name));
                used.extend(other.used.into_iter().map(|(name, _)| name));
            }
        }
        labels.sort();
        labels.dedup();
        (labels, used)
    }

    /// `:references`: what is wrong with the labels and references of
    /// the file being edited, as a list; Enter goes to the place.
    pub(super) fn references(&mut self) {
        let Some(editor) = &self.editor else {
            self.alert("this window only shows the document");
            return;
        };
        let marks = ref_text::read(editor.source());
        let (labels, used) = self.besides(&marks, true);
        let problems = ref_text::check(&marks, &labels, &|label| used.contains(label));
        let Some(text) = self.text() else {
            return;
        };
        if problems.is_empty() {
            self.notice(format!(
                "nothing wrong: {} references, {} labels, each reference to a label there is, \
                 each label referred to",
                marks.used.len(),
                marks.defined.len()
            ));
            return;
        }
        let items = problems
            .iter()
            .map(|problem| {
                let line = line_column(text, problem.range.start).0;
                let listed = format!("{}\tline {line}", problem.told());
                (listed, Goes::Offset(problem.range.start))
            })
            .collect();
        self.open_picker("references", items, 0);
    }

    /// Puts what is wrong with the labels and references of the file
    /// being edited among what the compiler complained about (`]d`),
    /// unless the compiler said so itself: references to nothing, where
    /// it gave up before it came to them, and labels on two things.
    /// Labels nothing refers to are left to `:references`: they are not
    /// wrong.
    pub(super) fn refer_problems(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        let file = editor.file();
        let marks = ref_text::read(editor.source());
        if marks.used.is_empty() && marks.defined.is_empty() {
            return;
        }
        let (labels, _) = self.besides(&marks, false);
        // Pages that were made have every reference lead somewhere;
        // and without any pages nothing is known of the other files.
        let failed = self.document_revision != self.compiled_revision;
        let judged = failed && self.refer.known.is_some();
        let mut found = Vec::new();
        for problem in ref_text::check(&marks, &labels, &|_| true) {
            let is_error = match problem.fault {
                Fault::Missing(_) if judged => true,
                Fault::Twice => false,
                Fault::Missing(_) | Fault::Unused => continue,
            };
            let named = format!("<{}>", problem.label);
            let said = self.diagnostics.iter().any(|(of, range, _, message)| {
                let there = range.start < problem.range.end && problem.range.start < range.end;
                message.contains(&named) || (*of == file && there)
            });
            if !said {
                found.push((file, problem.range.clone(), is_error, problem.told()));
            }
        }
        self.diagnostics.extend(found);
        self.diagnostics.sort_by_key(|(_, range, _, _)| range.start);
    }

    /// The label the cursor is on, if it is on one: its name, whether
    /// it is put on something there (and not referred to), and the
    /// offer for the thing it is the label of, if that is something
    /// known here.
    fn label_under_cursor(&mut self) -> Option<(String, bool, Option<Offer>)> {
        let editor = self.editor.as_ref()?;
        let (name, _, defines) = ref_text::label_at(editor.source(), editor.cursor())?;
        let offers = self.offers();
        let offer = offers
            .into_iter()
            .find(|offer| offer.labelled && offer.label == name);
        Some((name, defines, offer))
    }

    /// That the label `name` is nowhere, and which one may be meant.
    fn nowhere(&mut self, name: &str) -> String {
        let marks = self
            .editor
            .as_ref()
            .map(|editor| ref_text::read(editor.source()))
            .unwrap_or_default();
        let (labels, _) = self.besides(&marks, true);
        let here = marks.defined.iter().map(|(label, _)| label.as_str());
        let all = here.chain(labels.iter().map(String::as_str));
        match ref_text::nearest(name, all) {
            Some(nearest) => {
                format!("refers to nothing: there is no label {name} (but there is {nearest})")
            }
            None => format!("refers to nothing: there is no label {name}"),
        }
    }

    /// Whether Typst knows a label `name` on something of the document
    /// that is none of the things listed here.
    fn known_otherwise(&self, name: &str) -> bool {
        let known = self.refer.known.as_ref();
        known.is_some_and(|known| known.labels.iter().any(|(label, _)| label == name))
    }

    /// `K` on a reference or a label: what kind of thing it is of, its
    /// number and what it is called. Returns whether the cursor is on
    /// one that this can say something of.
    pub(super) fn refer_describe(&mut self) -> bool {
        let Some((name, defines, offer)) = self.label_under_cursor() else {
            return false;
        };
        let told = match offer {
            Some(offer) => {
                let place = match (offer.here, offer.elsewhere) {
                    (Some((_, at)), _) => {
                        let line = self.text().map_or(0, |text| line_column(text, at).0);
                        format!("line {line}")
                    }
                    (None, Some(span)) => match span.id() {
                        Some(file) => format!("in {}", file.vpath().get_without_slash()),
                        None => "in no file of the document".to_owned(),
                    },
                    (None, None) => "in the bibliography".to_owned(),
                };
                format!("{}\n{place}", offer.told())
            }
            // Something else that has a label: Typst says what.
            None if defines || self.known_otherwise(&name) => return false,
            None => self.nowhere(&name),
        };
        self.tooltip = Some(told);
        true
    }

    /// `gd` on a reference: to what it refers to. Returns whether the
    /// cursor is on one that this knows the way of.
    pub(super) fn refer_goto(&mut self) -> bool {
        let Some((name, defines, offer)) = self.label_under_cursor() else {
            return false;
        };
        if defines {
            return false;
        }
        // A label on something that is no section, figure or equation.
        let labelled = || {
            let editor = self.editor.as_ref()?;
            let marks = ref_text::read(editor.source());
            let found = marks.defined.iter().find(|(label, _)| *label == name);
            found.map(|(_, range)| range.start)
        };
        let world = self.compiled_world.as_deref();
        let elsewhere = |span: Span| place_of(span, world);
        match offer.map(|offer| (offer.kind, offer.here, offer.elsewhere)) {
            Some((_, Some((_, at)), _)) => self.refer_jump(at),
            Some((_, None, Some(span))) => match elsewhere(span) {
                Some((file, _, at)) => self.open_file(file, Some(at)),
                None => return false,
            },
            Some((Kind::Source, None, None)) => self.bib_command(&format!("open {name}")),
            Some(_) => return false,
            None => match labelled() {
                Some(at) => self.refer_jump(at),
                None if self.known_otherwise(&name) => return false,
                None => {
                    let nowhere = self.nowhere(&name);
                    self.alert(format!("@{name} {nowhere}"));
                }
            },
        }
        true
    }

    /// Puts the cursor at byte `at` as a jump, and shows it.
    fn refer_jump(&mut self, at: usize) {
        if let Some(editor) = &mut self.editor {
            editor.jump(at);
        }
        self.follow_cursor();
    }

    /// All of the document that can be referred to, for an agent: the
    /// sections, figures, tables and equations of the file being edited
    /// (with or without a label), those with a label of its other
    /// files, and the sources of its bibliography.
    ///
    /// # Errors
    ///
    /// If no file is being edited.
    pub fn referables(&mut self) -> Result<Vec<Referable>, String> {
        let offers = self.offers();
        let text = self.text().ok_or(super::agent::NOT_EDITING)?;
        let world = self.compiled_world.as_deref();
        let listed = |offer: Offer| {
            let elsewhere = offer.elsewhere.and_then(|span| place_of(span, world));
            let here = offer.here.map(|(_, at)| line_column(text, at).0);
            Referable {
                kind: offer.kind.word(),
                line: here.or(elsewhere.map(|(_, line, _)| line)),
                file: elsewhere.map(|(file, _, _)| file.vpath().get_without_slash().to_owned()),
                label: offer.labelled.then_some(offer.label),
                number: offer.number,
                numbered: offer.numbered,
                called: offer.called,
            }
        };
        Ok(offers.into_iter().map(listed).collect())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use zetwerk_typst::{CompileOutput, Environment, FontSet, Project};

    use super::*;
    use crate::editor::Editor;
    use crate::keys;

    /// A viewer editing a `main.typ` that holds `text`, in a directory
    /// of its own that also holds `files` (names and what is in them).
    fn editing(text: &str, files: &[(&str, &str)]) -> (Viewer, PathBuf) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "zetwerk-refer-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("main.typ"), text).unwrap();
        for (name, content) in files {
            std::fs::write(directory.join(name), content).unwrap();
        }
        (
            open(&directory.join("main.typ"), &directory.join("state")),
            directory,
        )
    }

    /// A viewer editing the file at `path`, compiled.
    fn open(path: &Path, state: &Path) -> Viewer {
        let env = Environment::new(FontSet::Embedded);
        let project = Project::open(Arc::clone(&env), path).unwrap();
        let (editor, opened) = Editor::open(path, state, project.main()).unwrap();
        let mut viewer = Viewer::new(env, project, (1000, 620), 1.0)
            .unwrap()
            .editing(editor, &opened);
        compile(&mut viewer);
        viewer
    }

    /// Compiles the viewer's text as it is, on this thread.
    fn compile(viewer: &mut Viewer) {
        let (revision, world) = viewer.snapshot();
        let compiled = zetwerk_typst::compile(&world);
        viewer.compiled(CompileOutput {
            revision,
            world: Arc::new(world),
            compiled,
            duration: Duration::from_millis(3),
        });
    }

    fn press(viewer: &mut Viewer, notation: &str) {
        for key in keys::parse(notation).unwrap() {
            viewer.key(key);
        }
    }

    /// The rows of the list that is up.
    fn rows(viewer: &Viewer) -> Vec<(String, String)> {
        let completion = viewer.completion.as_ref();
        completion.map_or_else(Vec::new, |completion| {
            let row = |entry: &Entry| {
                let candidate = &entry.candidate;
                let plain = || (candidate.label.clone(), candidate.kind.to_owned());
                viewer.refer_row(candidate).unwrap_or_else(plain)
            };
            completion.candidates.iter().map(row).collect()
        })
    }

    /// What the rows are listed as.
    fn listed(viewer: &Viewer) -> Vec<String> {
        rows(viewer).into_iter().map(|(label, _)| label).collect()
    }

    fn message(viewer: &Viewer) -> String {
        let message = viewer.message.as_ref();
        message.map_or_else(String::new, |message| message.text.clone())
    }

    const PAPER: &str = "#set heading(numbering: \"1.1\")\n\
        #set math.equation(numbering: \"(1)\")\n\
        = Introduction <intro>\n\
        Some _words_ <words> to begin with.\n\n\
        == Main result\n\
        #figure(rect(), caption: [A plot of energy]) <fig:plot>\n\n\
        #figure(table(columns: 2, [a], [b]), caption: [Rates])\n\n\
        $ E = m c^2 $\n\n\
        Closing words.\n\n\
        #bibliography(\"refs.bib\")\n";

    const SOURCES: &str = "@book{knuth1984,\n  author = {Donald E. Knuth},\n  \
        title = {The TeXbook},\n  year = {1984},\n}\n";

    fn pair(label: &str, about: &str) -> (String, String) {
        (label.to_owned(), about.to_owned())
    }

    #[test]
    fn at_lists_what_there_is_to_refer_to() {
        let (mut viewer, directory) = editing(PAPER, &[("refs.bib", SOURCES)]);
        assert_eq!(viewer.state().problems, Vec::<String>::new());
        press(&mut viewer, "/Closing<CR>A See @");
        assert_eq!(
            rows(&viewer),
            [
                pair("intro", "section 1 · Introduction"),
                pair("Main result", "section 1.1 · no label yet"),
                pair("fig:plot", "figure 1 · A plot of energy"),
                pair("Rates", "table 1 · no label yet"),
                pair("E = m c^2", "equation (1) · no label yet"),
                pair("knuth1984", "source · Knuth (1984) The TeXbook"),
                // What Typst knows a label of and this does not.
                pair("words", "label"),
            ]
        );
        // Typing narrows: by the label, by what the thing is called
        // (a title, a caption, an author), by its kind.
        press(&mut viewer, "fig:");
        assert_eq!(listed(&viewer), ["fig:plot"]);
        press(&mut viewer, "<BS><BS><BS><BS>ener");
        assert_eq!(listed(&viewer), ["fig:plot"]);
        press(&mut viewer, "<BS><BS><BS><BS>texb");
        assert_eq!(listed(&viewer), ["knuth1984"]);
        press(&mut viewer, "<BS><BS><BS><BS>knuth");
        assert_eq!(listed(&viewer), ["knuth1984"]);
        press(&mut viewer, "<BS><BS><BS><BS><BS>sec");
        assert_eq!(listed(&viewer), ["intro", "Main result"]);
        press(&mut viewer, "<BS><BS><BS>resu");
        assert_eq!(listed(&viewer), ["Main result"]);
        // Enter puts the reference in; the keys of every such list
        // work here too.
        press(
            &mut viewer,
            "<BS><BS><BS><BS>int<CR> and @<C-n><C-n><Tab> and @kn<C-y>.<Esc>",
        );
        let text = viewer.text().unwrap().to_owned();
        assert!(
            text.contains("Closing words. See @intro and @fig:plot and @knuth1984.\n"),
            "{text}"
        );
        assert!(viewer.completion.is_none());
        // All of it typed: Enter is for the text.
        press(&mut viewer, "o@intro");
        assert_eq!(listed(&viewer), ["intro"]);
        press(&mut viewer, "<CR>x<Esc>");
        assert!(viewer.text().unwrap().contains("@intro\nx\n"));
        // What is in an equation is no reference.
        press(&mut viewer, "o$x@");
        assert!(!listed(&viewer).contains(&"intro".to_owned()));
        press(&mut viewer, "<Esc>dd");
        compile(&mut viewer);
        assert_eq!(viewer.state().problems, Vec::<String>::new());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn what_has_no_label_gets_one_when_it_is_chosen() {
        let (mut viewer, directory) = editing(PAPER, &[("refs.bib", SOURCES)]);
        press(&mut viewer, "/Closing<CR>A See @rat");
        assert_eq!(rows(&viewer), [pair("Rates", "table 1 · no label yet")]);
        let before = viewer.text().unwrap().to_owned();
        press(&mut viewer, "<CR>");
        let text = viewer.text().unwrap().to_owned();
        assert!(text.contains("caption: [Rates]) <tab:rates>\n"), "{text}");
        assert!(text.contains("Closing words. See @tab:rates\n"), "{text}");
        assert!(message(&viewer).contains("tab:rates is the label of the table “Rates” now"));
        // Typing goes on behind the reference.
        press(&mut viewer, " and @eq:");
        assert_eq!(
            rows(&viewer),
            [pair("E = m c^2", "equation (1) · no label yet")]
        );
        press(&mut viewer, "<Tab>.<Esc>");
        let text = viewer.text().unwrap().to_owned();
        assert!(text.contains("$ E = m c^2 $ <eq:e-m-c>\n"), "{text}");
        assert!(text.contains("See @tab:rates and @eq:e-m-c.\n"), "{text}");
        // The pages are made of it: the labels are where Typst wants
        // them.
        compile(&mut viewer);
        assert_eq!(viewer.state().problems, Vec::<String>::new());
        // What got a label is listed with it from then on.
        press(&mut viewer, "A @tab");
        assert_eq!(rows(&viewer), [pair("tab:rates", "table 1 · Rates")]);
        press(&mut viewer, "<BS><BS><BS><BS><Esc>");
        // The label and the reference are one step to undo: after the
        // blank and the full stop, that of the equation.
        press(&mut viewer, "uuu");
        let undone = viewer.text().unwrap().to_owned();
        assert!(
            undone.contains("<tab:rates>") && !undone.contains("<eq:e-m-c>"),
            "{undone}"
        );
        press(&mut viewer, "uu");
        assert_eq!(viewer.text().unwrap(), before);
        // Two things called the same get labels of their own.
        press(&mut viewer, "u");
        press(&mut viewer, "Go== Main result<CR><CR>See @main");
        assert_eq!(listed(&viewer), ["Main result", "Main result"]);
        press(&mut viewer, "<C-n><CR><Esc>");
        let text = viewer.text().unwrap().to_owned();
        assert!(
            text.ends_with("== Main result <sec:main-result-2>\n\nSee @sec:main-result-2\n"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_reference_typst_would_refuse_is_not_put_in() {
        // No rule gives sections or equations numbers here.
        let text = "= Start\n\n$ a + b $\n\n= End <end>\n\nText.\n";
        let (mut viewer, directory) = editing(text, &[]);
        press(&mut viewer, "GA See @sta");
        assert_eq!(rows(&viewer), [pair("Start", "section · no label yet")]);
        press(&mut viewer, "<CR>, @en<CR>");
        assert!(message(&viewer).contains("referred to by its title"));
        press(&mut viewer, " and @eq<CR>");
        assert!(message(&viewer).contains("which has a number now"));
        press(&mut viewer, ".<Esc>");
        assert_eq!(
            viewer.text().unwrap(),
            "= Start <sec:start>\n\n\
             #math.equation(block: true, numbering: \"(1)\", $ a + b $) <eq:a-b>\n\n\
             = End <end>\n\n\
             Text. See #link(<sec:start>)[Start], #link(<end>)[End] and @eq:a-b.\n"
        );
        compile(&mut viewer);
        assert_eq!(viewer.state().problems, Vec::<String>::new());
        // With numbers, a section is referred to as every other thing.
        press(&mut viewer, ":numbering on<CR>");
        compile(&mut viewer);
        press(&mut viewer, "Go@en<CR><Esc>");
        assert!(viewer.text().unwrap().ends_with("@end\n"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn on_the_page_the_list_is_the_same() {
        let (mut viewer, directory) = editing(PAPER, &[("refs.bib", SOURCES)]);
        press(&mut viewer, ":view page<CR>/Closing<CR>A See @resu");
        assert_eq!(
            rows(&viewer),
            [pair("Main result", "section 1.1 · no label yet")]
        );
        press(&mut viewer, "<CR>");
        assert!(message(&viewer).contains("sec:main-result is the label of the section"));
        // Enter takes what Typst alone knows a label of, too.
        press(&mut viewer, " and @wor");
        assert_eq!(rows(&viewer), [pair("words", "label")]);
        press(&mut viewer, "<CR>");
        // An address is no reference: nothing comes up for it.
        press(&mut viewer, ", me@");
        assert_eq!(rows(&viewer), []);
        press(&mut viewer, "home<Esc>");
        let text = viewer.text().unwrap().to_owned();
        assert!(
            text.contains("== Main result <sec:main-result>\n"),
            "{text}"
        );
        assert!(
            text.contains("Closing words. See @sec:main-result and @words, me\\@home\n"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn numbers_are_counted_only_for_what_asks_for_them() {
        let text = "#set heading(numbering: \"1.\")\n= One <one>\n= Two\nSee @one.\n";
        let (mut viewer, directory) = editing(text, &[]);
        // The check after a compile asks only for the labels.
        viewer.refer_problems();
        let known = viewer.refer.known.as_ref().unwrap();
        assert!(!known.counted && known.labels.iter().any(|(label, _)| label == "one"));
        let numbers = |viewer: &mut Viewer| -> Vec<Option<String>> {
            let things = viewer.referables().unwrap().into_iter();
            things.map(|thing| thing.number).collect()
        };
        let counted = numbers(&mut viewer);
        assert_eq!(counted, [Some("1.".to_owned()), Some("2.".to_owned())]);
        assert!(viewer.refer.known.as_ref().unwrap().counted);
        // Pages a part was put into have the numbers of the whole they
        // were made of, not counted again.
        let pages = viewer.document.as_ref().map(|pages| {
            let world = viewer.compiled_world.clone().unwrap();
            super::super::local::Pages::new(Arc::new((**pages.whole()).clone()), world)
        });
        viewer.document = pages;
        assert_eq!(numbers(&mut viewer), counted);
        let known = viewer.refer.known.as_ref().unwrap();
        assert!(known.counted && known.document.as_ptr() != known.whole.as_ptr());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn numbers_are_counted_as_typst_counts_them() {
        let text = "#set heading(numbering: \"1.a\")\n\
                    = One\n== Sub\n= Two\n\
                    #heading(numbering: none)[Aside]\n\
                    #counter(heading).update(6)\n\
                    = Seven <seven>\n\
                    #figure(rect(), caption: [First])\n\
                    #figure(table([a]), caption: [A table])\n\
                    #figure(rect(), caption: [Second])\n\
                    $ x $\n";
        let (mut viewer, directory) = editing(text, &[]);
        assert_eq!(viewer.state().problems, Vec::<String>::new());
        let told: Vec<(String, Option<String>, Option<usize>)> = viewer
            .referables()
            .unwrap()
            .into_iter()
            .map(|thing| {
                (
                    format!("{} {}", thing.kind, thing.called),
                    thing.number,
                    thing.line,
                )
            })
            .collect();
        let some = |number: &str| Some(number.to_owned());
        assert_eq!(
            told,
            [
                ("section One".to_owned(), some("1"), Some(2)),
                ("section Sub".to_owned(), some("1.a"), Some(3)),
                ("section Two".to_owned(), some("2"), Some(4)),
                ("section Seven".to_owned(), some("7"), Some(7)),
                ("figure First".to_owned(), some("1"), Some(8)),
                ("table A table".to_owned(), some("1"), Some(9)),
                ("figure Second".to_owned(), some("2"), Some(10)),
                ("equation x".to_owned(), None, Some(11)),
            ]
        );
        let labels: Vec<Option<String>> = viewer
            .referables()
            .unwrap()
            .into_iter()
            .map(|thing| thing.label)
            .collect();
        assert_eq!(labels[3], some("seven"));
        assert_eq!(labels[0], None);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn references_are_checked_described_and_followed() {
        let text = "#set heading(numbering: \"1.\")\n\
                    = One <one>\n= Two <two>\n= Again <two>\n$ x $ <eq:x>\n\n\
                    See @one and @tow, and @knuth1984.\n\n\
                    #bibliography(\"refs.bib\")\n";
        let (mut viewer, directory) = editing(text, &[("refs.bib", SOURCES)]);
        let position = |viewer: &Viewer| viewer.editor.as_ref().unwrap().position();
        // The compiler says that @tow leads nowhere; that is not said
        // a second time. That a label is there twice it does not say.
        let said: Vec<(bool, &str)> = viewer
            .diagnostics
            .iter()
            .map(|(_, _, is_error, message)| (*is_error, message.as_str()))
            .collect();
        assert_eq!(said.len(), 3, "{said:?}");
        assert_eq!(
            said.iter()
                .filter(|(_, message)| message.contains("<tow>"))
                .count(),
            1
        );
        let twice = (false, "<two> is the label of more than one thing");
        assert_eq!(said[..2], [twice, twice]);
        // The list of all that is wrong: Enter goes there.
        press(&mut viewer, ":references<CR>");
        assert!(viewer.is_picking());
        press(&mut viewer, "tow<CR>");
        assert_eq!(position(&viewer), (7, 14));
        press(&mut viewer, ":references<CR>eqnowhere<CR>");
        assert_eq!(position(&viewer), (5, 7), "the label nothing refers to");
        // K says what a reference is to; gd goes there.
        press(&mut viewer, "/@one<CR>K");
        assert_eq!(viewer.tooltip.as_deref(), Some("section · One\nline 2"));
        press(&mut viewer, "gd");
        assert_eq!(position(&viewer), (2, 1));
        press(&mut viewer, "/@tow<CR>K");
        let told = viewer.tooltip.clone().unwrap();
        assert_eq!(
            told,
            "refers to nothing: there is no label tow (but there is two)"
        );
        press(&mut viewer, "gd");
        assert!(message(&viewer).contains("(but there is two)"));
        assert_eq!(position(&viewer), (7, 14));
        press(&mut viewer, "/@knuth<CR>K");
        assert_eq!(
            viewer.tooltip.as_deref(),
            Some("source · Knuth (1984) The TeXbook\nin the bibliography")
        );
        // Of a source, all that is known of it is shown.
        press(&mut viewer, "gd");
        assert!(viewer.is_picking());
        press(&mut viewer, "<Esc><Esc>");
        // A text with nothing wrong says so.
        press(&mut viewer, "/tow<CR>cwtwo<Esc>:3<CR>dd");
        press(&mut viewer, "/eq:x<CR>hD");
        press(&mut viewer, ":references<CR>");
        assert!(!viewer.is_picking(), "{}", viewer.text().unwrap());
        assert!(message(&viewer).starts_with("nothing wrong: 3 references, 2 labels"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    #[ignore = "a measurement, not a test: the file named by MEASURE is opened and referred to"]
    fn measure() {
        let path = PathBuf::from(std::env::var("MEASURE").unwrap());
        let state =
            std::env::temp_dir().join(format!("zetwerk-refer-measure-{}", std::process::id()));
        let begun = Instant::now();
        let mut viewer = open(&path, &state);
        eprintln!(
            "{} pages compiled in {:?}",
            viewer.state().pages,
            begun.elapsed()
        );
        let mut timed = |keys: &str, what: &str| {
            let begun = Instant::now();
            press(&mut viewer, keys);
            let rows = viewer
                .completion
                .as_ref()
                .map_or(0, |list| list.candidates.len());
            eprintln!("{what}: {:?}, {rows} rows", begun.elapsed());
        };
        timed("Goplai", "a new line and five letters");
        timed("n", "a letter of a word (nothing listed)");
        timed(" ", "a blank");
        timed("@", "@ (the list is made)");
        timed("s", "a letter");
        timed("e", "a letter");
        timed("c", "a letter");
        timed("<BS>", "a letter less");
        timed("<Esc>o@", "@ again (the document is known)");
        timed("c", "a letter");
        timed("<Esc>K", "K on the reference");
        timed(":references<CR>", ":references");
        // The parts by themselves, the best of several goes each: the
        // machine has other work.
        press(&mut viewer, "<Esc>A");
        let mut best = |what: &str, part: &mut dyn FnMut(&mut Viewer)| {
            let took = (0..15).map(|_| {
                let begun = Instant::now();
                part(&mut viewer);
                begun.elapsed()
            });
            eprintln!("{what}: {:?}", took.min().unwrap());
        };
        best("a key that narrows the list", &mut |viewer| {
            viewer.refer_complete();
        });
        best("the list made anew (what @ takes)", &mut |viewer| {
            viewer.refer.listed = None;
            viewer.refer_complete();
        });
        best("  of that, reading the text", &mut |viewer| {
            let _ = ref_text::read(viewer.editor.as_ref().unwrap().source());
        });
        best("  of that, asking Typst for its labels", &mut |viewer| {
            let editor = viewer.editor.as_ref().unwrap();
            let (source, cursor) = (editor.source().clone(), editor.cursor());
            let world = viewer.project.snapshot();
            let document = viewer.document.as_ref().map(|pages| &**pages.whole());
            let _ = zetwerk_typst::complete(&world, document, &source, cursor, false);
        });
        best(
            "the document asked for its numbers (once a compile, with @)",
            &mut |viewer| {
                let _ = know(viewer.document.as_ref().unwrap().whole(), true);
            },
        );
        best(
            "the document asked for its labels (once a compile)",
            &mut |viewer| {
                let _ = know(viewer.document.as_ref().unwrap().whole(), false);
            },
        );
        best(
            "the bibliography found and read (once a compile)",
            &mut |viewer| {
                let _ = viewer.sources();
            },
        );
        best("the check after a compile", &mut |viewer| {
            // Of pages it has not seen.
            let pages = viewer.document.as_ref().map(|pages| {
                let world = viewer.compiled_world.clone().unwrap();
                super::super::local::Pages::new(Arc::new((**pages.whole()).clone()), world)
            });
            viewer.document = pages;
            viewer.refer_problems();
        });
        best("K on a reference", &mut |viewer| {
            viewer.refer_describe();
        });
        let _ = std::fs::remove_dir_all(state);
    }
}
