//! Spelling and grammar, on the page.
//!
//! What is wrong with the text is found on a thread of its own (see
//! `zetwerk_language`): never by a key. This is what the window does
//! with it. A word no dictionary knows gets a thin red line under it on
//! the page, words that do not go together a blue one; `]s` and `[s` go
//! from one to the next, the status line says what is wrong with the
//! one the cursor is on, `z=` lists what could be written instead, `zg`
//! makes the word under the cursor one of the user's own words and `zw`
//! takes that back.
//!
//! The text is checked when it has been compiled, which is when typing
//! pauses, and only as much of it as the window shows; `]s` has checked
//! as far as the next thing found, `:issues` all of it. What was found
//! stays where it is while the text changes under it, until the text is
//! checked again.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use typst::layout::{Abs, Rect};
use typst::syntax::{FileId, Source};
use typst::utils::hash128;
use zetwerk_language::{Checked, Finding, Job, Kind, Personal, Scope, Service, Settings};
use zetwerk_typst::{Origin, PageMap};

use super::assist::Target;
use super::{Change, Difference, Edited, Scene, TextureId, Viewer, difference, tint};
use crate::editor;

/// The line under a word no dictionary knows, and under words that do
/// not go together. Colours are RGBA.
pub(super) const PAGE_SPELLING: [u8; 4] = [0xcc, 0x24, 0x1d, 0xd8];
pub(super) const PAGE_GRAMMAR: [u8; 4] = [0x45, 0x85, 0x88, 0xd8];

/// How much more of the text than the window shows is checked with it,
/// in bytes.
const AROUND: usize = 2000;
/// How many words `z=` offers.
const SUGGESTIONS: usize = 9;
/// How many findings `:issues` lists; more are no list to read.
const LISTED: usize = 2000;
/// How much of what a finding is about is shown in a list.
const SHOWN: usize = 40;

/// Something to do once the text has been checked far enough.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Waiting {
    /// Go to the next finding, or the one before.
    Jump { forward: bool },
    /// List all there is.
    List,
}

/// What was found in a text.
struct Found {
    file: FileId,
    /// The revision that was checked, and its text.
    revision: u64,
    source: Source,
    /// In the order of the text.
    findings: Vec<Finding>,
    /// Whether all of the text was checked.
    complete: bool,
}

/// What `z=` offered: the bytes to replace, the revision they are bytes
/// of, and what with.
struct Choices {
    range: Range<usize>,
    revision: u64,
    with: Vec<String>,
}

/// What the window knows about the spelling and grammar of the text.
#[derive(Default)]
pub(super) struct Language {
    /// What the user has decided; nothing is checked by default.
    pub(super) settings: Settings,
    /// The thread that checks, once something was to be checked.
    service: Option<Service>,
    /// How that thread says it has an answer.
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
    /// The user's own words.
    personal: Personal,
    /// The number of the job asked for last, and of the last one that
    /// was answered to the end.
    serial: u64,
    answered: u64,
    /// The text jobs are asked for, by its mark (see [`mark_of`]).
    asked_text: Option<(u64, FileId, u64, Source)>,
    /// What the window showed when it last asked.
    asked_window: Option<(u64, Range<usize>)>,
    found: Option<Found>,
    /// Counts answers: what is drawn is drawn for one of them.
    generation: u64,
    /// The findings moved to where they are in a later revision.
    moved: Option<(u64, u64, Vec<Finding>)>,
    /// The lines under the words of the pages drawn so far, in points
    /// from each page's corner, and the answer and the document they
    /// are of.
    lines: HashMap<usize, Vec<(Rect, Kind)>>,
    lines_of: (u64, Option<u64>),
    /// The findings as places in the text the pages were made of.
    on_pages: Vec<(Range<usize>, Kind)>,
    /// The bytes of the text each page shows, and the revision the
    /// pages were made of.
    spans: HashMap<usize, Option<Range<usize>>>,
    spans_of: Option<u64>,
    waiting: Option<(u64, Waiting)>,
    choices: Option<Choices>,
    /// The dictionary and what checks grammar, as the last answer has
    /// them.
    dictionary: Option<String>,
    grammar: Option<String>,
    /// The document's language, and the revision it was read from.
    written_in: Option<(u64, String)>,
}

/// A number for the text of `file` at `revision`.
fn mark_of(file: FileId, revision: u64) -> u64 {
    hash128(&(file, revision)) as u64
}

/// `text` cut to `most` characters.
fn cut(text: &str, most: usize) -> String {
    let mut shown: String = text.chars().take(most).collect();
    if shown.len() < text.len() {
        shown.push('…');
    }
    shown.replace('\n', " ")
}

/// `findings`, which are places in a text, as places in a text that
/// differs from it by `change`. What the change touches is left out: it
/// is being written.
fn moved(findings: &[Finding], change: Change) -> Vec<Finding> {
    if change == Change::NONE {
        return findings.to_vec();
    }
    let end = change.offset + change.removed;
    let kept = findings.iter().filter_map(|finding| {
        let range = &finding.range;
        if range.end < change.offset {
            return Some(finding.clone());
        }
        (range.start > end).then(|| Finding {
            range: change.forth(range.start)..change.forth(range.end),
            ..finding.clone()
        })
    });
    kept.collect()
}

/// How `new` differs from `old`, as one replacement.
fn change_between(old: &str, new: &str) -> Change {
    match difference(old, new) {
        Difference::Nothing => Change::NONE,
        Difference::Replacement {
            offset,
            removed,
            text,
        } => Change {
            offset,
            removed,
            inserted: text.len(),
        },
    }
}

/// The finding among `findings` that byte `at` is in.
fn finding_at(findings: &[Finding], at: usize) -> Option<&Finding> {
    let behind = findings.partition_point(|finding| finding.range.start <= at);
    // Findings may lie in one another; few do.
    let near = findings.get(behind.saturating_sub(4)..behind)?;
    near.iter().rev().find(|finding| finding.range.end > at)
}

/// Adds `cell` to `rects`, which are of one kind: to the last of them,
/// if it goes on where that ends.
fn join(rects: &mut Vec<(Rect, Kind)>, cell: Rect, kind: Kind) {
    let slack = Abs::pt(0.5);
    match rects.last_mut() {
        Some((last, of))
            if *of == kind
                && (last.min.y - cell.min.y).abs() < slack
                && (last.max.y - cell.max.y).abs() < slack
                && cell.min.x >= last.min.x
                && cell.min.x <= last.max.x + slack =>
        {
            last.max.x = last.max.x.max(cell.max.x);
        }
        _ => rects.push((cell, kind)),
    }
}

/// The bytes of `file` a glyph shows, if it shows text or a blank.
fn shown_by(origin: &Origin, file: FileId) -> Option<&Range<usize>> {
    match origin {
        Origin::Text { file: of, range } | Origin::Space { file: of, range } if *of == file => {
            Some(range)
        }
        _ => None,
    }
}

impl Language {
    /// Whether anything is checked.
    fn is_on(&self) -> bool {
        self.settings.spelling || self.settings.grammar
    }

    /// The findings as places in the text of `revision`, which is
    /// `text`, of file `file`.
    fn current(&mut self, file: FileId, revision: u64, text: &str) -> &[Finding] {
        let Some(found) = self.found.as_ref().filter(|found| found.file == file) else {
            return &[];
        };
        if found.revision == revision {
            return &found.findings;
        }
        let fresh =
            matches!(&self.moved, Some((of, to, _)) if (*of, *to) == (self.generation, revision));
        if !fresh {
            let change = change_between(found.source.text(), text);
            let findings = moved(&found.findings, change);
            self.moved = Some((self.generation, revision, findings));
        }
        self.moved.as_ref().map_or(&[], |(_, _, findings)| findings)
    }

    /// Forgets what was found and what was asked: everything is asked
    /// again.
    fn forget(&mut self) {
        self.found = None;
        self.asked_text = None;
        self.asked_window = None;
        self.waiting = None;
        self.choices = None;
        self.generation += 1;
    }
}

/// What an agent is told about the language of the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The revision of the text that was checked.
    pub revision: u64,
    /// The dictionary spelling was checked with, and what checked the
    /// grammar: `None` where nothing did.
    pub dictionary: Option<String>,
    pub grammar: Option<String>,
    pub issues: Vec<Issue>,
    /// What there is to say besides: that a dictionary is missing.
    pub notes: Vec<String>,
}

/// One thing found, for an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// Where it begins, counting lines and characters from 1.
    pub line: usize,
    pub column: usize,
    /// The text it is about.
    pub text: String,
    /// Whether it is a matter of grammar, not of spelling.
    pub grammar: bool,
    pub message: String,
    pub suggestions: Vec<String>,
}

/// Something that checks all of a text and reports, to be run where
/// waiting does no harm.
pub type Check = Box<dyn FnOnce() -> Result<Report, String> + Send>;

/// The languages of two letters (ISO 639-1), for `:lang`: Typst takes
/// any two or three letters, and a slip is not a language.
const LANGUAGE_CODES: &str = "aa ab ae af ak am an ar as av ay az ba be bg bh bi bm bn bo br \
    bs ca ce ch co cr cs cu cv cy da de dv dz ee el en eo es et eu fa ff fi fj fo fr fy ga gd \
    gl gn gu gv ha he hi ho hr ht hu hy hz ia id ie ig ii ik io is it iu ja jv ka kg ki kj kk \
    kl km kn ko kr ks ku kv kw ky la lb lg li ln lo lt lu lv mg mh mi mk ml mn mr ms mt my na \
    nb nd ne ng nl nn no nr nv ny oc oj om or os pa pi pl ps pt qu rm rn ro ru rw sa sc sd se \
    sg si sk sl sm sn so sq sr ss st su sv sw ta te tg th ti tk tl tn to tr ts tt tw ty ug uk \
    ur uz ve vi vo wa wo xh yi yo za zh zu";

impl Viewer {
    /// Takes what the user has to say about spelling and grammar.
    pub(super) fn set_language(&mut self, settings: &Settings) {
        self.language.settings = settings.clone();
        if let Some(path) = &settings.words {
            let (personal, complaint) = Personal::open(path.clone());
            self.language.personal = personal;
            if let Some(complaint) = complaint {
                self.alert(complaint);
            }
        }
    }

    /// The language the settings say to check in, whatever the document
    /// says, if they say one: the user's own (see `tutor`).
    pub(super) fn settings_language(&self) -> Option<&str> {
        self.language.settings.language.as_deref()
    }

    /// Has `wake` called when what was found is to be collected: it
    /// should make the window draw.
    pub fn wake_for_language(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        self.language.wake = Some(Arc::new(wake));
    }

    /// The thread that checks: started when it is first needed.
    fn language_service(&mut self) -> Option<&Service> {
        if self.language.service.is_none() {
            let wake = self.language.wake.clone();
            let service = Service::spawn(self.language.settings.clone(), move || {
                if let Some(wake) = &wake {
                    wake();
                }
            });
            match service {
                Ok(service) => {
                    service.set_words(self.language.personal.words());
                    self.language.service = Some(service);
                }
                Err(err) => self.alert(format!("spelling is not checked: {err}")),
            }
        }
        self.language.service.as_ref()
    }

    /// The language the text is checked in: the one the user chose
    /// (`:spell de`), or the document's, which is English where it
    /// names none.
    fn language_of_document(&mut self) -> String {
        if let Some(chosen) = &self.language.settings.language {
            return chosen.clone();
        }
        if let Some((revision, language)) = &self.language.written_in
            && *revision == self.revision
        {
            return language.clone();
        }
        // The rule is in the main file, whichever file is edited.
        let main = self.project.main();
        let edited = self.editor.as_ref().filter(|editor| editor.file() == main);
        let source = edited
            .map(editor::Editor::source)
            .or(self.project.source(main));
        let named = |key: &str| {
            let value =
                source.and_then(|source| crate::typst_text::rule_value(source, "text", key))?;
            let name = value.trim_matches('"').trim().to_owned();
            (!name.is_empty() && value.starts_with('"')).then_some(name)
        };
        let language = match (named("lang"), named("region")) {
            (Some(language), Some(region)) => format!("{language}-{region}"),
            (Some(language), None) => language,
            (None, _) => "en".to_owned(),
        };
        self.language.written_in = Some((self.revision, language.clone()));
        language
    }

    /// Asks for the text as it is to be checked as far as `scope` says.
    /// Returns the number of the job.
    fn language_ask(&mut self, scope: Scope, suggest: bool) -> Option<u64> {
        let job = self.language_job(scope, suggest)?;
        let serial = job.serial;
        self.language_service()?.check(job);
        Some(serial)
    }

    /// A job to check the text as it is.
    fn language_job(&mut self, scope: Scope, suggest: bool) -> Option<Job> {
        let language = self.language_of_document();
        let editor = self.editor.as_ref()?;
        let (file, revision) = (editor.file(), self.revision);
        let text = mark_of(file, revision);
        if self
            .language
            .asked_text
            .as_ref()
            .is_none_or(|(of, ..)| *of != text)
        {
            self.language.asked_text = Some((text, file, revision, editor.source().clone()));
        }
        self.language.serial += 1;
        Some(Job {
            serial: self.language.serial,
            text,
            source: editor.source().clone(),
            language,
            scope,
            suggest,
        })
    }

    /// The bytes of the text the pages in the window show; where the
    /// cursor is, where no page shows text.
    fn language_window(&mut self) -> Range<usize> {
        let file = self.file_id();
        let cursor = self.editor.as_ref().map_or(0, editor::Editor::cursor);
        let mut window: Option<Range<usize>> = None;
        if self.language.spans_of != self.document_revision {
            self.language.spans_of = self.document_revision;
            self.language.spans.clear();
        }
        if let (Some(document), Some(world)) = (&self.document, &self.compiled_world) {
            for visible in self.view.visible(&self.layout) {
                let Some(page) = document.pages().get(visible.index) else {
                    continue;
                };
                let maps = &mut self.maps;
                let span = self.language.spans.entry(visible.index).or_insert_with(|| {
                    let map = maps
                        .entry(visible.index)
                        .or_insert_with(|| PageMap::build(world, &page.frame));
                    let shown = map
                        .glyphs()
                        .iter()
                        .filter_map(|glyph| shown_by(&glyph.origin, file));
                    shown.fold(None, |span: Option<Range<usize>>, shown| match span {
                        Some(span) => Some(span.start.min(shown.start)..span.end.max(shown.end)),
                        None => Some(shown.clone()),
                    })
                });
                window = match (window, span.clone()) {
                    (Some(window), Some(span)) => {
                        Some(window.start.min(span.start)..window.end.max(span.end))
                    }
                    (window, span) => window.or(span),
                };
            }
        }
        window.unwrap_or(cursor..cursor)
    }

    /// Takes what the thread has found since it was last asked.
    fn language_collect(&mut self) {
        let Some(service) = &self.language.service else {
            return;
        };
        for checked in service.poll() {
            self.language_checked(checked);
        }
    }

    /// Takes one answer.
    fn language_checked(&mut self, checked: Checked) {
        if let Some(note) = checked.notes.first() {
            self.alert(
                checked
                    .notes
                    .iter()
                    .skip(1)
                    .fold(note.clone(), |all, note| all + "; " + note),
            );
        }
        let language = &mut self.language;
        language.dictionary.clone_from(&checked.dictionary);
        language.grammar.clone_from(&checked.grammar);
        let Some((_, file, revision, source)) = language
            .asked_text
            .as_ref()
            .filter(|(text, ..)| *text == checked.text)
        else {
            // An answer about a text nobody asks about any more.
            return;
        };
        language.found = Some(Found {
            file: *file,
            revision: *revision,
            source: source.clone(),
            findings: checked.findings,
            complete: checked.complete,
        });
        language.generation += 1;
        if checked.done {
            language.answered = language.answered.max(checked.serial);
        }
        let waited = language
            .waiting
            .filter(|(serial, _)| *serial == checked.serial);
        if let (true, Some((_, waiting))) = (checked.done, waited) {
            self.language.waiting = None;
            match waiting {
                Waiting::Jump { forward } => self.jump_to_issue(forward),
                Waiting::List => self.list_issues(),
            }
        }
    }

    /// What is done about spelling and grammar when a frame is drawn:
    /// what was found is taken, what the window shows is asked for if
    /// it was not, and the status line says what the cursor is on.
    /// None of it is work to speak of.
    pub(super) fn language_frame(&mut self) {
        if !self.language.is_on() {
            return;
        }
        self.language_collect();
        let Some(editor) = &self.editor else {
            return;
        };
        let (file, cursor) = (editor.file(), editor.cursor());
        let resting = editor.mode() == editor::Mode::Normal;
        // Text that is being typed is checked when the typing pauses,
        // which is when it is compiled.
        if self.compiled_revision == Some(self.revision) {
            let text = mark_of(file, self.revision);
            let shown = self.language_window();
            let asked = &self.language.asked_window;
            let covers = |(of, asked): &(u64, Range<usize>)| {
                *of == text && asked.start <= shown.start && asked.end >= shown.end
            };
            if !asked.as_ref().is_some_and(covers) {
                // Some more than is shown: what comes into view next
                // is known by then.
                let window = shown.start.saturating_sub(AROUND)..shown.end + AROUND;
                self.language.asked_window = Some((text, window.clone()));
                self.language_ask(Scope::Window(window), false);
            }
        }
        if resting && self.message.is_none() && self.picker.is_none() {
            let (revision, dictionary) = (self.revision, self.language.dictionary.clone());
            let Some(editor) = &self.editor else {
                return;
            };
            let text = editor.text();
            let findings = self.language.current(file, revision, text);
            let told = finding_at(findings, cursor)
                .map(|finding| says(finding, text, dictionary.as_deref()));
            if let Some(told) = told {
                self.notice(told);
            }
        }
    }

    /// Waits until the thread has answered everything asked of it. For
    /// tests, where nobody is woken.
    #[cfg(test)]
    pub(super) fn language_settle(&mut self) {
        self.language_frame();
        while self.language.answered < self.language.serial {
            let Some(service) = &self.language.service else {
                return;
            };
            let Some(checked) = service.wait() else {
                return;
            };
            self.language_checked(checked);
        }
    }

    /// The lines under what was found on page `index`, whose top-left
    /// corner is drawn at `origin`.
    ///
    /// They are where the page has the words, whatever was typed since
    /// the page was made; only the line that typing is worked into is
    /// left bare, for its words move.
    pub(super) fn language_page(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        index: usize,
        origin: (f64, f64),
    ) {
        let file = self.file_id();
        let (Some(document), Some(world)) = (&self.document, &self.compiled_world) else {
            return;
        };
        let language = &mut self.language;
        let Some(found) = language.found.as_ref().filter(|found| found.file == file) else {
            return;
        };
        let of = (language.generation, self.document_revision);
        if language.lines_of != of {
            language.lines_of = of;
            language.lines.clear();
            // Where the findings are in the text the pages were made of.
            let change = match world.written(file) {
                Ok(compiled) if Some(found.revision) != self.document_revision => {
                    change_between(found.source.text(), compiled.text())
                }
                _ => Change::NONE,
            };
            let there = moved(&found.findings, change);
            language.on_pages = there
                .into_iter()
                .map(|finding| (finding.range, finding.kind))
                .collect();
        }
        if language.on_pages.is_empty() {
            return;
        }
        let Some(page) = document.pages().get(index) else {
            return;
        };
        let maps = &mut self.maps;
        let on_pages = &language.on_pages;
        let lines = language.lines.entry(index).or_insert_with(|| {
            let map = maps
                .entry(index)
                .or_insert_with(|| PageMap::build(world, &page.frame));
            let mut lines = Vec::new();
            for glyph in map.glyphs() {
                let Some(shown) = shown_by(&glyph.origin, file) else {
                    continue;
                };
                let behind = on_pages.partition_point(|(range, _)| range.start < shown.end);
                let near = on_pages
                    .get(behind.saturating_sub(4)..behind)
                    .unwrap_or(&[]);
                for (_, kind) in near.iter().filter(|(range, _)| range.end > shown.start) {
                    join(&mut lines, glyph.rect, *kind);
                }
            }
            lines
        });
        let zoom = self.view.zoom;
        let thick = (1.2 * self.scale).round().max(1.0);
        // The line typing is worked into: its words are not where the
        // page as compiled has them.
        let typed = self
            .predicted
            .as_ref()
            .filter(|predicted| predicted.index == index);
        let typed = typed.map(|predicted| predicted.typed);
        // Only the marks the user wants on the page (see `set`).
        let marked = (self.settings.spelling_marks, self.settings.grammar_marks);
        for (rect, kind) in lines.iter() {
            if typed.is_some_and(|typed| rect.min.y < typed.max.y && rect.max.y > typed.min.y) {
                continue;
            }
            if !matches!(
                (kind, marked),
                (Kind::Spelling, (true, _)) | (Kind::Grammar, (_, true))
            ) {
                continue;
            }
            let height = (rect.max.y - rect.min.y).to_pt() * zoom;
            let (colour, down) = match kind {
                Kind::Spelling => (PAGE_SPELLING, 0.12),
                Kind::Grammar => (PAGE_GRAMMAR, 0.02),
            };
            let x = origin.0 + rect.min.x.to_pt() * zoom;
            let y = origin.1 + rect.max.y.to_pt() * zoom - down * height - thick;
            let width = (rect.max.x - rect.min.x).to_pt() * zoom;
            tint(scene, has, colour, (x, y.round(), width, thick));
        }
    }

    /// The commands of spelling and grammar. Returns whether `command`
    /// was one.
    pub(super) fn language_command(&mut self, command: &str) -> bool {
        let (name, wanted) = match command.split_once(' ') {
            Some((name, wanted)) => (name, wanted.trim()),
            None => (command, ""),
        };
        if self.editor.is_none() {
            return false;
        }
        match (name, wanted) {
            ("spell", "" | "on" | "off") => {
                let on = match wanted {
                    "" => !self.language.is_on(),
                    wanted => wanted == "on",
                };
                self.switch_checking(on);
            }
            ("spell", "?") => self.tell_checking(),
            ("spell", wanted) if wanted.starts_with("take ") => {
                let chosen = wanted.trim_start_matches("take ").parse().ok();
                self.take_suggestion(chosen);
            }
            ("spell", language)
                if !matches!(language, "auto" | "document")
                    && zetwerk_language::find(language, &self.language.settings.directories)
                        .is_none() =>
            {
                self.offer_dictionaries(language, "spell");
            }
            ("spell", language) => {
                let document = matches!(language, "auto" | "document");
                self.language.settings.language = (!document).then(|| language.to_owned());
                self.language.settings.spelling = true;
                self.reconfigure_language();
                let language = self.language_of_document();
                self.notice(format!("spelling is checked as {language}"));
            }
            ("grammar", "" | "on" | "off") => {
                let on = match wanted {
                    "" => !self.language.settings.grammar,
                    wanted => wanted == "on",
                };
                self.language.settings.grammar = on;
                self.reconfigure_language();
                let told = match (on, self.unchecked_grammar()) {
                    (false, _) => "grammar is not checked any more".to_owned(),
                    (true, None) => "grammar is checked from now on".to_owned(),
                    (true, Some(why)) => why,
                };
                self.notice(told);
            }
            ("issues", "") => self.issues(),
            ("issue", "next") => self.goto_issue(true),
            ("issue", "previous") => self.goto_issue(false),
            ("suggest", "") => self.suggest(),
            ("word", "good") => self.own_word(true),
            ("word", "wrong") => self.own_word(false),
            _ => return false,
        }
        true
    }

    /// Where no dictionary is installed for `language`: says so, and
    /// puts up those that are, each to be had by `command` (`spell` by
    /// its name, `lang` by its language).
    pub(super) fn offer_dictionaries(&mut self, language: &str, command: &str) {
        let installed = zetwerk_language::installed(&self.language.settings.directories);
        let mut items: Vec<(String, super::assist::Target)> = Vec::new();
        for name in installed {
            let code = name
                .split(['_', '-'])
                .next()
                .unwrap_or(&name)
                .to_lowercase();
            let (label, given) = if command == "lang" {
                (format!("{code}\t{name}"), code)
            } else {
                (format!("{name}\t{code}"), name.clone())
            };
            if !items
                .iter()
                .any(|(listed, _)| listed.split('\t').next() == label.split('\t').next())
            {
                items.push((
                    label,
                    super::assist::Target::Command(format!("{command} {given}")),
                ));
            }
        }
        let wanted = zetwerk_language::package(language);
        if items.is_empty() {
            return self.alert(format!(
                "no dictionary for {language} is installed, nor any other: {wanted} or the \
                 like brings one"
            ));
        }
        self.open_picker("dictionaries installed", items, 0);
        self.alert(format!(
            "no dictionary for {language} is installed ({wanted} would bring one): those \
             that are are listed"
        ));
    }

    /// Whether `code` names a language: one of the codes of two letters
    /// (ISO 639-1), or one a dictionary is installed for.
    pub(super) fn knows_language(&self, code: &str) -> bool {
        let code = code.to_ascii_lowercase();
        LANGUAGE_CODES.split_whitespace().any(|known| known == code)
            || zetwerk_language::find(&code, &self.language.settings.directories).is_some()
    }

    /// Has the thread go by the settings as they are now, and all that
    /// was found forgotten.
    pub(super) fn reconfigure_language(&mut self) {
        self.language.forget();
        if let Some(service) = &self.language.service {
            service.configure(self.language.settings.clone());
        }
    }

    /// `:spell`: turns checking on or off.
    fn switch_checking(&mut self, on: bool) {
        let settings = &mut self.language.settings;
        if on && !(settings.spelling || settings.grammar) {
            settings.spelling = true;
        } else if !on {
            settings.spelling = false;
            settings.grammar = false;
        }
        self.reconfigure_language();
        if on {
            let language = self.language_of_document();
            self.notice(format!(
                "spelling is checked as {language}: ]s goes to what is found, \
                 :issues lists it, :grammar checks grammar too"
            ));
        } else {
            self.notice("spelling and grammar are not checked any more");
        }
    }

    /// Why the grammar of the document's language is not checked, if
    /// nothing here can: there is no LanguageTool server to ask, and
    /// the rules built in know English alone.
    fn unchecked_grammar(&mut self) -> Option<String> {
        if self.language.settings.languagetool.is_some() {
            return None;
        }
        let language = self.language_of_document();
        let english = language.to_lowercase().starts_with("en");
        (!english || !cfg!(feature = "harper")).then(|| {
            format!(
                "nothing here checks the grammar of {language}: that takes a LanguageTool \
                 server (languagetool = \"http://localhost:8081\" under [language] in the settings)"
            )
        })
    }

    /// `:spell ?`: says what is checked, and what with.
    fn tell_checking(&mut self) {
        if !self.language.is_on() {
            self.notice("nothing is checked (:spell turns spelling on)");
            return;
        }
        let language = self.language_of_document();
        let none = "nothing".to_owned();
        let spelling = self
            .language
            .dictionary
            .clone()
            .unwrap_or_else(|| none.clone());
        let grammar = self.language.grammar.clone().unwrap_or(none);
        let found = self
            .language
            .found
            .as_ref()
            .map_or(0, |found| found.findings.len());
        let wanted = self.language.settings.grammar;
        let told = match self.unchecked_grammar().filter(|_| wanted) {
            Some(why) => format!("language {language}; spelling by {spelling}; {why}"),
            None => format!(
                "language {language}; spelling by {spelling}; grammar by {grammar}; \
                 {found} found so far"
            ),
        };
        self.notice(told);
    }

    /// Whether all of the text as it is has been checked.
    fn all_checked(&self) -> bool {
        let file = self.file_id();
        let found = self.language.found.as_ref();
        found.is_some_and(|found| {
            found.complete && found.file == file && found.revision == self.revision
        })
    }

    /// `]s` and `[s`: to the next thing found, or the one before. The
    /// text is checked as far as that first, if it was not.
    fn goto_issue(&mut self, forward: bool) {
        if !self.language.is_on() {
            self.notice("nothing is checked (:spell turns spelling on)");
            return;
        }
        self.language_collect();
        if self.all_checked() {
            return self.jump_to_issue(forward);
        }
        let Some(from) = self.editor.as_ref().map(editor::Editor::cursor) else {
            return;
        };
        let serial = self.language_ask(Scope::Next { from, forward }, false);
        self.language.waiting = serial.map(|serial| (serial, Waiting::Jump { forward }));
        self.notice("checking as far as the next thing to find…");
    }

    /// Puts the cursor on the finding after it, or the one before, and
    /// says what it is.
    fn jump_to_issue(&mut self, forward: bool) {
        let (revision, dictionary) = (self.revision, self.language.dictionary.clone());
        let Some(editor) = &mut self.editor else {
            return;
        };
        let (file, cursor) = (editor.file(), editor.cursor());
        let findings = self.language.current(file, revision, editor.text());
        let target = if forward {
            let next = findings.iter().find(|finding| finding.range.start > cursor);
            next.or(findings.first())
        } else {
            let before = findings
                .iter()
                .rev()
                .find(|finding| finding.range.start < cursor);
            before.or(findings.last())
        };
        let Some(target) = target else {
            self.notice("nothing wrong was found");
            return;
        };
        let told = says(target, editor.text(), dictionary.as_deref());
        editor.set_cursor(target.range.start);
        self.notice(told);
        self.follow_cursor();
    }

    /// `:issues`: all that was found, to go to. All of the text is
    /// checked first, if it was not.
    fn issues(&mut self) {
        if !self.language.is_on() {
            self.notice("nothing is checked (:spell turns spelling on)");
            return;
        }
        self.language_collect();
        if self.all_checked() {
            return self.list_issues();
        }
        let serial = self.language_ask(Scope::All, false);
        self.language.waiting = serial.map(|serial| (serial, Waiting::List));
        self.notice("checking all of the document…");
    }

    /// Puts up the list of all that was found.
    fn list_issues(&mut self) {
        let revision = self.revision;
        let Some(editor) = &self.editor else {
            return;
        };
        let text = editor.text();
        let findings = self.language.current(editor.file(), revision, text);
        if findings.is_empty() {
            self.notice("nothing wrong was found");
            return;
        }
        let mut items: Vec<(String, Target)> = Vec::new();
        // Lines are counted along the way: the findings are in order.
        let (mut line, mut counted) = (1, 0);
        for finding in findings.iter().take(LISTED) {
            let start = finding.range.start.min(text.len());
            line += text
                .get(counted..start)
                .map_or(0, |between| between.matches('\n').count());
            counted = counted.max(start);
            let about = cut(text.get(finding.range.clone()).unwrap_or(""), SHOWN);
            let what = match finding.kind {
                Kind::Spelling => "not in the dictionary".to_owned(),
                Kind::Grammar => finding.message.clone(),
            };
            items.push((format!("{line}: {about} — {what}"), Target::Offset(start)));
        }
        if findings.len() > LISTED {
            let more = findings.len() - LISTED;
            items.push((format!("… and {more} more"), Target::Nothing));
        }
        self.open_picker("issues", items, 0);
    }

    /// `z=`: what could be written in place of what the cursor is on.
    fn suggest(&mut self) {
        self.language_collect();
        let revision = self.revision;
        let Some(editor) = &self.editor else {
            return;
        };
        let (file, cursor, text) = (editor.file(), editor.cursor(), editor.text());
        let findings = self.language.current(file, revision, text);
        let Some(finding) = finding_at(findings, cursor).cloned() else {
            self.notice("nothing is marked here (]s goes to the next thing that is)");
            return;
        };
        let about = text.get(finding.range.clone()).unwrap_or("").to_owned();
        let with = match finding.kind {
            Kind::Grammar => finding.replacements.clone(),
            Kind::Spelling => {
                let speller = self.language.service.as_ref().and_then(Service::speller);
                speller.map_or_else(Vec::new, |speller| speller.suggest(&about, SUGGESTIONS))
            }
        };
        if with.is_empty() {
            self.notice(format!("nothing to suggest for {}", cut(&about, SHOWN)));
            return;
        }
        let shown = |with: &String| match with.as_str() {
            "" => "(take it out)".to_owned(),
            with => with.to_owned(),
        };
        let items = with.iter().enumerate();
        let items =
            items.map(|(at, with)| (shown(with), Target::Command(format!("spell take {at}"))));
        let items = items.collect();
        self.language.choices = Some(Choices {
            range: finding.range,
            revision,
            with,
        });
        self.open_picker("suggestions", items, 0);
    }

    /// Writes what was chosen of what `z=` offered, as one change.
    fn take_suggestion(&mut self, chosen: Option<usize>) {
        let choices = self.language.choices.take();
        let chosen = choices.as_ref().and_then(|choices| {
            let with = choices.with.get(chosen?)?;
            (choices.revision == self.revision).then_some((choices.range.clone(), with))
        });
        let (Some((range, with)), Some(editor)) = (chosen, &mut self.editor) else {
            self.notice("the text has changed since: z= again, please");
            return;
        };
        let start = range.start;
        match editor.replace(&[(range, with.as_str())]) {
            Ok(()) => {
                editor.set_cursor(start);
                self.text_changed(Edited::Command);
                self.notice(format!("{}  (u takes it back)", cut(with, SHOWN)));
            }
            Err(why) => self.alert(format!("not changed: {why}")),
        }
    }

    /// The word the cursor is on, as it would be one of the user's own
    /// words: without what marks it as somebody's.
    fn word_at_cursor(&self) -> Option<String> {
        let editor = self.editor.as_ref()?;
        let (text, cursor) = (editor.text(), editor.cursor());
        let line = text[..cursor].rfind('\n').map_or(0, |at| at + 1);
        let end = text[cursor..]
            .find('\n')
            .map_or(text.len(), |at| cursor + at);
        let written = &text[line..end];
        let mut words = zetwerk_language::words(written);
        let word = words.find(|word| line + word.start <= cursor && cursor < line + word.end)?;
        let word = &written[word];
        let word = word
            .strip_suffix("'s")
            .or(word.strip_suffix("’s"))
            .unwrap_or(word);
        Some(word.to_owned())
    }

    /// `zg` makes the word under the cursor one of the user's own, `zw`
    /// takes it from them.
    fn own_word(&mut self, good: bool) {
        let Some(word) = self.word_at_cursor() else {
            self.notice("the cursor is on no word");
            return;
        };
        let personal = &mut self.language.personal;
        let changed = if good {
            personal.add(&word)
        } else {
            personal.remove(&word)
        };
        let told = match (changed, good) {
            (Err(err), _) => {
                self.alert(format!("your words were not written: {err}"));
                return;
            }
            (Ok(true), true) => format!("{word} is a word from now on  (zw takes that back)"),
            (Ok(false), true) => format!("{word} is one of your words already"),
            (Ok(true), false) => format!("{word} is none of your words any more"),
            (Ok(false), false) => format!("{word} is none of your words"),
        };
        let words = self.language.personal.words();
        if let Some(service) = &self.language.service {
            service.set_words(words);
        }
        // What the window shows is asked for again, with the new words.
        self.language.asked_window = None;
        self.language.waiting = None;
        self.notice(told);
    }

    /// Something that checks all of the text as it is, spelling with
    /// suggestions, and reports what it found: for an agent, whose
    /// thread may wait for it. The error says why nothing is checked.
    pub fn language_check(&mut self) -> Result<Check, String> {
        if !self.language.is_on() {
            return Err("spelling and grammar are not checked: \
                        the user has them switched off (:spell in the window)"
                .to_owned());
        }
        let job = self.language_job(Scope::All, true);
        let job = job.ok_or(super::agent::NOT_EDITING)?;
        let handle = self.language_service().and_then(Service::handle);
        let handle = handle.ok_or("the checker could not be started")?;
        let revision = self.revision;
        Ok(Box::new(move || {
            let source = job.source.clone();
            let checked = handle.check(job).ok_or("the checker has stopped")?;
            let text = source.text();
            let issues = checked.findings.into_iter().map(|finding| {
                let (line, column) = super::agent::line_column(text, finding.range.start);
                let dictionary = checked.dictionary.as_deref().unwrap_or("the");
                Issue {
                    line,
                    column,
                    text: text.get(finding.range.clone()).unwrap_or("").to_owned(),
                    grammar: finding.kind == Kind::Grammar,
                    message: match finding.kind {
                        Kind::Spelling => format!("not in the {dictionary} dictionary"),
                        Kind::Grammar => finding.message,
                    },
                    suggestions: finding.replacements,
                }
            });
            Ok(Report {
                revision,
                issues: issues.collect(),
                dictionary: checked.dictionary,
                grammar: checked.grammar,
                notes: checked.notes,
            })
        }))
    }
}

/// What the status line says about `finding`, which is in `text`.
fn says(finding: &Finding, text: &str, dictionary: Option<&str>) -> String {
    let about = cut(text.get(finding.range.clone()).unwrap_or(""), SHOWN);
    match finding.kind {
        Kind::Spelling => {
            let dictionary = dictionary.map_or(String::new(), |name| format!(" ({name})"));
            format!(
                "“{about}” is not in the dictionary{dictionary}: \
                 z= for what may be meant, zg if it is a word"
            )
        }
        Kind::Grammar if finding.replacements.is_empty() => finding.message.clone(),
        Kind::Grammar => format!("{}  (z= for what to write)", finding.message),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use zetwerk_typst::{CompileOutput, Environment, FontSet, Project};

    use super::*;
    use crate::editor::Editor;
    use crate::gpu::Quad;
    use crate::keys;

    /// The directory of the small dictionaries `zetwerk-language` keeps
    /// for tests.
    fn dictionaries() -> PathBuf {
        PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../zetwerk-language/fixtures"
        ))
    }

    /// A fresh directory holding `main.typ` with `text`.
    fn directory(text: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "zetwerk-language-view-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("main.typ"), text).unwrap();
        directory
    }

    /// Compiles the viewer's current revision on this thread.
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

    /// A viewer in a 1000×620 window editing `text`, which checks its
    /// spelling (and, with `grammar`, its grammar) with the dictionaries
    /// kept for tests; and the directory its file is in.
    fn checking(text: &str, grammar: bool) -> (Viewer, PathBuf) {
        let directory = directory(text);
        let path = directory.join("main.typ");
        let env = Environment::new(FontSet::Embedded);
        let project = Project::open(Arc::clone(&env), &path).unwrap();
        let (editor, opened) =
            Editor::open(&path, &directory.join("state"), project.main()).unwrap();
        let mut viewer = Viewer::new(env, project, (1000, 620), 1.0)
            .unwrap()
            .editing(editor, &opened);
        viewer.state_dir = Some(directory.join("state"));
        viewer.set_language(&Settings {
            spelling: true,
            grammar,
            directories: vec![dictionaries()],
            words: Some(directory.join("words.txt")),
            ..Settings::default()
        });
        compile(&mut viewer);
        viewer.language_settle();
        (viewer, directory)
    }

    fn press(viewer: &mut Viewer, notation: &str) {
        for key in keys::parse(notation).unwrap() {
            viewer.key(key);
        }
    }

    /// Presses keys and waits for what they asked of the checker.
    fn press_and_settle(viewer: &mut Viewer, notation: &str) {
        press(viewer, notation);
        viewer.language_settle();
    }

    /// The lines of one colour under words of the pages.
    fn lines(viewer: &mut Viewer, rgba: [u8; 4]) -> Vec<Quad> {
        let id = TextureId(hash128(&("tint", rgba)));
        let scene = viewer.scene(&|_| false);
        let of_colour = |quad: &&Quad| quad.texture == id;
        scene.quads.iter().filter(of_colour).copied().collect()
    }

    fn message(viewer: &Viewer) -> &str {
        viewer
            .message
            .as_ref()
            .map_or("", |message| message.text.as_str())
    }

    /// The word the cursor is at the beginning of.
    fn word_under_cursor(viewer: &Viewer) -> String {
        let editor = viewer.editor.as_ref().unwrap();
        let rest = &editor.text()[editor.cursor()..];
        rest.split(|c: char| !c.is_alphanumeric())
            .next()
            .unwrap()
            .to_owned()
    }

    fn remove(directory: &Path) {
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn findings_move_with_the_text_and_go_where_it_is_changed() {
        let finding = |range: Range<usize>| Finding {
            range,
            kind: Kind::Spelling,
            message: String::new(),
            replacements: Vec::new(),
        };
        let found = [finding(0..5), finding(10..15), finding(20..25)];
        let places = |change: Change| -> Vec<Range<usize>> {
            moved(&found, change)
                .into_iter()
                .map(|finding| finding.range)
                .collect()
        };
        assert_eq!(places(Change::NONE), [0..5, 10..15, 20..25]);
        // Three bytes typed between the first and the second.
        let typed = change_between("01234 6789abcde", "01234 xyz6789abcde");
        assert_eq!(places(typed), [0..5, 13..18, 23..28]);
        // A change to the second, or right behind it: it is being written.
        let within = change_between("0123456789abcdefghij", "0123456789aXcdefghij");
        assert_eq!(places(within), [0..5, 20..25]);
        let behind = Change {
            offset: 15,
            removed: 0,
            inserted: 1,
        };
        assert_eq!(places(behind), [0..5, 21..26]);
        assert_eq!(
            finding_at(&found, 12).map(|finding| finding.range.clone()),
            Some(10..15)
        );
        assert_eq!(
            finding_at(&found, 15).map(|finding| finding.range.clone()),
            None
        );
        assert_eq!(
            finding_at(&found, 0).map(|finding| finding.range.clone()),
            Some(0..5)
        );
    }

    #[test]
    fn a_misspelt_word_has_a_line_under_it_on_the_page() {
        let (mut viewer, directory) = checking("This is a wrnog word and a good one.\n", false);
        let under = lines(&mut viewer, PAGE_SPELLING);
        assert_eq!(under.len(), 1, "{under:?}");
        // A thin line as wide as a word, not a block over it.
        assert!(under[0].height <= 2.0 && under[0].width > 20.0, "{under:?}");
        assert_eq!(lines(&mut viewer, PAGE_GRAMMAR), []);
        // Nothing wrong, nothing drawn.
        let (mut clean, other) = checking("This is a good word.\n", false);
        assert_eq!(lines(&mut clean, PAGE_SPELLING), []);
        remove(&directory);
        remove(&other);
    }

    #[test]
    fn nothing_is_checked_unless_the_user_wants_it() {
        let directory = directory("A wrnog word.\n");
        let path = directory.join("main.typ");
        let env = Environment::new(FontSet::Embedded);
        let project = Project::open(Arc::clone(&env), &path).unwrap();
        let (editor, opened) =
            Editor::open(&path, &directory.join("state"), project.main()).unwrap();
        let mut viewer = Viewer::new(env, project, (1000, 620), 1.0)
            .unwrap()
            .editing(editor, &opened);
        compile(&mut viewer);
        assert_eq!(lines(&mut viewer, PAGE_SPELLING), []);
        assert!(
            viewer.language.service.is_none(),
            "no thread is started for nothing"
        );
        press(&mut viewer, "]s");
        assert!(message(&viewer).contains(":spell"), "{}", message(&viewer));
        assert!(viewer.language_check().is_err());
        remove(&directory);
    }

    #[test]
    fn keys_go_from_one_finding_to_the_next_and_say_what_it_is() {
        let text = "A wrnog word.\n\nA good paragraph.\n\nA secnod wrod.\n";
        let (mut viewer, directory) = checking(text, false);
        press_and_settle(&mut viewer, "gg]s");
        assert_eq!(word_under_cursor(&viewer), "wrnog");
        assert!(message(&viewer).contains("wrnog") && message(&viewer).contains("en_GB"));
        assert!(message(&viewer).contains("z="), "{}", message(&viewer));
        press_and_settle(&mut viewer, "]s");
        assert_eq!(word_under_cursor(&viewer), "secnod");
        press_and_settle(&mut viewer, "]s");
        assert_eq!(word_under_cursor(&viewer), "wrod");
        // Around the end, and back again.
        press_and_settle(&mut viewer, "]s");
        assert_eq!(word_under_cursor(&viewer), "wrnog");
        press_and_settle(&mut viewer, "[s");
        assert_eq!(word_under_cursor(&viewer), "wrod");
        press_and_settle(&mut viewer, "[s");
        assert_eq!(word_under_cursor(&viewer), "secnod");
        // The cursor coming to rest on a finding says what it is, too.
        press(&mut viewer, "gg0");
        viewer.scene(&|_| false);
        assert_eq!(message(&viewer), "");
        press(&mut viewer, "w");
        viewer.scene(&|_| false);
        assert!(message(&viewer).contains("wrnog"), "{}", message(&viewer));
        remove(&directory);
    }

    #[test]
    fn a_suggestion_replaces_the_word_as_one_change() {
        let (mut viewer, directory) = checking("He said helo to the world.\n", false);
        press_and_settle(&mut viewer, "gg]s");
        press(&mut viewer, "z=");
        assert!(viewer.picker.is_some());
        let offered = viewer.language.choices.as_ref().unwrap().with.clone();
        let hello = offered.iter().position(|with| with == "hello");
        let hello = hello.unwrap_or_else(|| panic!("{offered:?}"));
        for _ in 0..hello {
            press(&mut viewer, "<C-n>");
        }
        press(&mut viewer, "<CR>");
        assert_eq!(viewer.text(), Some("He said hello to the world.\n"));
        assert_eq!(viewer.take_edited(), Some(Edited::Command));
        press(&mut viewer, "u");
        assert_eq!(viewer.text(), Some("He said helo to the world.\n"));
        // Where nothing is marked there is nothing to suggest.
        press(&mut viewer, "0z=");
        assert!(viewer.picker.is_none());
        assert!(
            message(&viewer).contains("nothing is marked"),
            "{}",
            message(&viewer)
        );
        remove(&directory);
    }

    #[test]
    fn the_users_own_words_are_kept_and_taken_back() {
        let (mut viewer, directory) = checking("Typst is good. A Typst word is wrnog.\n", false);
        assert_eq!(lines(&mut viewer, PAGE_SPELLING).len(), 3);
        press_and_settle(&mut viewer, "gg]szg");
        assert!(
            message(&viewer).contains("Typst is a word"),
            "{}",
            message(&viewer)
        );
        assert_eq!(
            std::fs::read_to_string(directory.join("words.txt")).unwrap(),
            "Typst\n"
        );
        assert_eq!(lines(&mut viewer, PAGE_SPELLING).len(), 1);
        press_and_settle(&mut viewer, "]s");
        assert_eq!(word_under_cursor(&viewer), "wrnog");
        // The text was not changed by any of this.
        assert_eq!(viewer.state().revision, 0);
        press_and_settle(&mut viewer, "0zw");
        assert_eq!(
            std::fs::read_to_string(directory.join("words.txt")).unwrap(),
            ""
        );
        assert_eq!(lines(&mut viewer, PAGE_SPELLING).len(), 3);
        remove(&directory);
    }

    #[test]
    fn the_language_is_the_documents_unless_the_user_names_one() {
        let text = "#set text(lang: \"de\")\nDas Haus ist good.\n";
        let (mut viewer, directory) = checking(text, false);
        press_and_settle(&mut viewer, "gg]s");
        assert_eq!(word_under_cursor(&viewer), "good");
        assert!(message(&viewer).contains("de_DE"), "{}", message(&viewer));
        assert_eq!(lines(&mut viewer, PAGE_SPELLING).len(), 1);
        // Dutch, because the user says so.
        press_and_settle(&mut viewer, ":spell nl<CR>");
        assert_eq!(lines(&mut viewer, PAGE_SPELLING).len(), 4);
        press_and_settle(&mut viewer, ":spell ?<CR>");
        assert!(message(&viewer).contains("nl_NL"), "{}", message(&viewer));
        // The document's again; then nothing at all.
        press_and_settle(&mut viewer, ":spell document<CR>");
        assert_eq!(lines(&mut viewer, PAGE_SPELLING).len(), 1);
        press_and_settle(&mut viewer, ":spell<CR>");
        assert_eq!(lines(&mut viewer, PAGE_SPELLING), []);
        press_and_settle(&mut viewer, ":spell<CR>");
        assert_eq!(lines(&mut viewer, PAGE_SPELLING).len(), 1);
        remove(&directory);
    }

    #[test]
    fn a_language_without_a_dictionary_is_said_once_and_left_alone() {
        let text = "#set text(lang: \"fr\")\nUn mot.\n";
        let (mut viewer, directory) = checking(text, false);
        assert!(
            message(&viewer).contains("hunspell-fr"),
            "{}",
            message(&viewer)
        );
        assert_eq!(lines(&mut viewer, PAGE_SPELLING), []);
        press_and_settle(&mut viewer, "A encore<Esc>");
        compile(&mut viewer);
        viewer.language_settle();
        assert_eq!(message(&viewer), "");
        remove(&directory);
    }

    #[test]
    fn what_changed_is_checked_when_it_is_compiled_and_not_before() {
        let (mut viewer, directory) = checking("A good word.\n\nAnother good one.\n", false);
        assert_eq!(lines(&mut viewer, PAGE_SPELLING), []);
        let asked = viewer.language.serial;
        press(&mut viewer, "A And a wrnog one.");
        // Typing asks nothing of the checker, however often it is drawn.
        viewer.scene(&|_| false);
        assert_eq!(viewer.language.serial, asked);
        press(&mut viewer, "<Esc>");
        compile(&mut viewer);
        viewer.language_settle();
        assert_eq!(viewer.language.serial, asked + 1);
        assert_eq!(lines(&mut viewer, PAGE_SPELLING).len(), 1);
        // Drawing again asks nothing again.
        viewer.scene(&|_| false);
        viewer.scene(&|_| false);
        assert_eq!(viewer.language.serial, asked + 1);
        // What was found stays under its word while text is typed
        // before it, and is found again from where the text is now.
        press(&mut viewer, "ggOA new line.<Esc>");
        press_and_settle(&mut viewer, "gg]s");
        assert_eq!(word_under_cursor(&viewer), "wrnog");
        remove(&directory);
    }

    #[test]
    fn all_that_was_found_is_listed_to_go_to() {
        let text = "A wrnog word.\n\nA good paragraph.\n\nA secnod wrod.\n";
        let (mut viewer, directory) = checking(text, false);
        press_and_settle(&mut viewer, ":issues<CR>");
        assert!(viewer.picker.is_some());
        assert_eq!(viewer.language.found.as_ref().unwrap().findings.len(), 3);
        // The list is narrowed by typing, as every list is.
        press(&mut viewer, "5: wrod<CR>");
        assert_eq!(word_under_cursor(&viewer), "wrod");
        press_and_settle(&mut viewer, ":issues<CR>");
        press(&mut viewer, "<C-n><CR>");
        assert_eq!(word_under_cursor(&viewer), "secnod");
        remove(&directory);
    }

    #[test]
    fn an_agent_is_told_what_was_found_with_what_may_be_meant() {
        let (mut viewer, directory) = checking("Good.\n\nHe said helo.\n", false);
        let check = viewer.language_check().unwrap();
        let report = check().unwrap();
        assert_eq!(report.dictionary.as_deref(), Some("en_GB"));
        assert_eq!(report.issues.len(), 1);
        let issue = &report.issues[0];
        assert_eq!(
            (issue.line, issue.column, issue.text.as_str()),
            (3, 9, "helo")
        );
        assert!(!issue.grammar && issue.suggestions.contains(&"hello".to_owned()));
        assert!(issue.message.contains("en_GB"), "{}", issue.message);
        remove(&directory);
    }

    #[cfg(feature = "harper")]
    #[test]
    fn grammar_has_a_line_of_its_own_and_offers_what_to_write() {
        let (mut viewer, directory) = checking("This is an test of the checker.\n", true);
        assert_eq!(lines(&mut viewer, PAGE_GRAMMAR).len(), 1);
        assert_eq!(lines(&mut viewer, PAGE_SPELLING), []);
        press_and_settle(&mut viewer, "gg]s");
        assert_eq!(word_under_cursor(&viewer), "an");
        assert!(message(&viewer).contains("z="), "{}", message(&viewer));
        press(&mut viewer, "z=<CR>");
        assert_eq!(viewer.text(), Some("This is a test of the checker.\n"));
        // Grammar off, spelling stays.
        press(&mut viewer, "u");
        compile(&mut viewer);
        viewer.language_settle();
        assert_eq!(lines(&mut viewer, PAGE_GRAMMAR).len(), 1);
        press_and_settle(&mut viewer, ":grammar off<CR>");
        assert_eq!(lines(&mut viewer, PAGE_GRAMMAR), []);
        remove(&directory);
    }

    #[test]
    fn a_language_without_a_dictionary_is_not_taken_and_those_there_are_offered() {
        let (mut viewer, directory) = checking("Words.\n", false);
        let listed = |viewer: &Viewer| -> Vec<String> {
            let picker = viewer.picker.as_ref();
            let listing = picker.map(|picker| picker.listing().1).unwrap_or_default();
            listing.into_iter().map(str::to_owned).collect()
        };
        press(&mut viewer, ":spell xyz<CR>");
        assert_eq!(viewer.language.settings.language, None);
        assert_eq!(
            listed(&viewer),
            ["de_DE\tde", "en_GB\ten", "nl_NL\tnl"],
            "{:?}",
            viewer.message.as_ref().map(|message| &message.text)
        );
        press(&mut viewer, "nl<CR>");
        assert_eq!(viewer.language.settings.language.as_deref(), Some("nl_NL"));
        // The language of the document likewise: a slip is no language.
        press(&mut viewer, ":lang xq<CR>");
        assert_eq!(viewer.text(), Some("Words.\n"));
        assert_eq!(listed(&viewer), ["de\tde_DE", "en\ten_GB", "nl\tnl_NL"]);
        press(&mut viewer, "<Esc>:lang fr<CR>");
        assert!(
            viewer
                .text()
                .is_some_and(|text| text.contains("lang: \"fr\""))
        );
        let _ = std::fs::remove_dir_all(directory);
    }
}
