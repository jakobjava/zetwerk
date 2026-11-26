//! What changed, shown on the page as a printed proof shows it.
//!
//! The text is compared with a version the writer names (see [`Since`]):
//! the file as last saved (the usual), the text when the window opened,
//! a step of the history (`gh` lists them), or the file as a commit of
//! git has it (`HEAD`, any commit, a branch). It is compared by
//! paragraph and, inside the paragraphs that differ, by word (see
//! [`crate::diff`]); blanks and the breaks of the lines of the source do
//! not count, since the page does not show them.
//!
//! - **Bars.** Every line of the page that shows a word that is new or
//!   changed has a thin bar beside it, hard against the left edge of the
//!   text, between it and the numbers of the lines (see `aids`): the
//!   right margin is for tags and notes, and the tint of the page is for
//!   who wrote what. Where words were only taken out, the line where
//!   they stood has the bar; where whole paragraphs were taken out, a
//!   small mark stands between the lines where they were. `]v` and `[v`
//!   go to the next and the previous change (`]c` of Vim's diff mode is
//!   taken by tables), and the
//!   status line counts them. `:changes off` takes the bars away.
//! - **What it was.** `zc` on a change sets its paragraph over itself as
//!   it was and is at once, typeset in the way of the page: what went
//!   struck through, what came tinted (as a proposal of an agent is
//!   shown, see `notes`). Enter takes the old version back for that
//!   paragraph (one undo step; `:revert` does it too), `y` yanks the old
//!   text, Escape leaves it. A paragraph that cannot be set so (a change
//!   of its markup) is set as it was, without marks.
//! - **All of it.** `:changes list` (or `:changed`; `:changes` alone is
//!   Vim's list of where changes were made) lists the changes (the
//!   section, the first words, how many words), to go to. `:changes
//!   proof` shows the whole
//!   document marked as an editor marks a proof: new words tinted, words
//!   taken out struck through in place, all typeset by Typst; where a
//!   change is in markup that cannot carry marks, a bar stands beside it
//!   in the margin of the proof and the status line says how many.
//!   `:changes pdf` writes that proof as a PDF for someone who read the
//!   older version.
//! - **Git.** Where the document's folder is in a repository, the status
//!   line says the branch and whether the file differs from `HEAD`. It
//!   is asked when the window opens, when the file is saved and when git
//!   changes `HEAD` or its index; never for a key, and on a thread of its
//!   own (see [`crate::git`]). `:git log` lists the commits of the file to
//!   compare with; `:git commit [message]` commits the files of the
//!   document (the main file, what it includes, its bibliography, the
//!   images it shows: what its last compile read), never anything else,
//!   after saying which they are and asking; not with unsaved changes.
//!
//! What is compared is worked out again only when the text or the
//! version compared with changes, and only where it is asked (the status
//! line, the pages in view); the bars of a page are placed again only
//! when its pages, the text or the comparison change. Nothing runs while
//! nothing happens. Nothing of it is in the document, in an export (but
//! `:changes pdf`) or in what an agent is shown.

use super::notes::marker_length as block_mark;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use typst::syntax::{FileId, Source};
use zetwerk_core::StateId;
use zetwerk_typst::{PageLines, ProjectWorld};

use super::assist::Target;
use super::{Change, Edited, PAPER, PageRows, Quad, Scene, TextureId, Viewer, solid, tint};
use crate::chrome::{self, InPlace};
use crate::diff::{self, Passage};
use crate::editor;
use crate::git::{self, Commit, FileState, Repo};
use crate::keys::{Key, KeyPress};
use crate::typst_text::{self, Ground};

/// The bar beside a line that changed, and the mark where paragraphs
/// were taken out. Colours are RGBA; each has a name of its own, to be
/// given another at night.
pub(super) const CHANGE_BAR: [u8; 4] = [0x07, 0x66, 0x78, 0xd0];
pub(super) const CHANGE_GONE: [u8; 4] = [0xcc, 0x24, 0x1d, 0xe0];
/// The edge beside a paragraph set as it was.
pub(super) const CHANGE_SLIP_EDGE: [u8; 4] = [0x07, 0x66, 0x78, 0xff];
/// How far left of the text a bar stands, its right side, in points; how
/// wide it is, in pixels of the interface; how long the mark of what was
/// taken out is, in points.
const BAR_GAP: f64 = 2.0;
const BAR_WIDTH: f64 = 1.5;
const GONE_LENGTH: f64 = 8.0;
/// Lines whose bars are less than this many of their heights apart have
/// one bar.
const JOINED: f64 = 0.8;
/// How many words of a change its line of the list shows.
const LISTED_WORDS: usize = 8;
/// How many commits `:git log` lists.
const LOGGED: usize = 300;
/// How large the text of the page is where it does not say, in points.
const USUAL_SIZE: f64 = 11.0;
/// What a proof begins with: how its marks look.
const PROOF_RULES: &str = "#let zetwerk-new(body) = highlight(fill: rgb(\"#b8bb26\").transparentize(45%), body)\n\
     #let zetwerk-old(body) = text(fill: rgb(\"#cc241d\"), strike(stroke: 0.7pt + rgb(\"#cc241d\"), body))\n\
     #let zetwerk-gone(body) = block(text(fill: rgb(\"#cc241d\"), strike(stroke: 0.7pt + rgb(\"#cc241d\"), body)))\n\
     #let zetwerk-bar() = box(width: 0pt, place(dx: -1.5em, dy: -0.8em, rect(width: 1.5pt, height: 1.05em, fill: rgb(\"#076678\"))))\n";

/// The version the text is compared with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Since {
    /// The file as last saved (or read).
    Saved,
    /// The text when the window opened it.
    Opened,
    /// A state of the history (`gh`).
    Step(StateId),
    /// The file in a commit of git: a revision as git reads it.
    Git(String),
}

impl Since {
    /// `wanted` as a version: `saved`, `opened`, `step 12`, or else a
    /// revision of git.
    fn read(wanted: &str) -> Option<Self> {
        let wanted = wanted.trim();
        Some(match wanted {
            "" => return None,
            "saved" | "save" | "written" | "disk" => Self::Saved,
            "opened" | "open" | "start" => Self::Opened,
            _ => match wanted.strip_prefix("step ").map(|step| step.trim().parse()) {
                Some(Ok(state)) => Self::Step(state),
                Some(Err(_)) => return None,
                None => Self::Git(wanted.to_owned()),
            },
        })
    }

    /// What it is called in the status line.
    fn called(&self) -> String {
        match self {
            Self::Saved => "saved".to_owned(),
            Self::Opened => "opened".to_owned(),
            Self::Step(state) => format!("step {state}"),
            Self::Git(revision) => revision.clone(),
        }
    }
}

/// What the user's settings say of changes (`[changes]` in the config).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeOptions {
    /// Whether the bars are shown.
    pub bars: bool,
    /// What the text is compared with to begin with: `saved`, `opened`
    /// or a revision of git.
    pub since: String,
    /// Whether git is asked at all.
    pub git: bool,
}

impl Default for ChangeOptions {
    fn default() -> Self {
        Self {
            // Off unless asked for: beside equations and on slides the bar
            // stood against the text, and read as a second cursor.
            bars: false,
            since: "saved".to_owned(),
            git: true,
        }
    }
}

/// The text compared with.
struct Reference {
    file: FileId,
    /// What it is: the version asked for, and the state of the history
    /// it was for one that moves (the saved one).
    of: (Since, Option<StateId>),
    source: Source,
    paragraphs: Vec<Range<usize>>,
    generation: u64,
}

/// What differs, for a revision of the text and a reference.
struct Found {
    file: FileId,
    revision: u64,
    reference: u64,
    passages: Vec<Passage>,
    generation: u64,
}

/// Where the marks of a page stand, and what they were placed for.
struct Placed {
    key: PlaceKey,
    /// Bars: the left edge of the text they stand by, their top and
    /// bottom; marks of what was taken out: the edge and their height.
    /// In points of the page.
    bars: Vec<(f64, f64, f64)>,
    gone: Vec<(f64, f64)>,
}

/// What marks were placed for: the comparison, the pages, how the text
/// relates to theirs.
type PlaceKey = (u64, usize, Change);

/// A paragraph set as it was, over itself.
struct Slip {
    file: FileId,
    revision: u64,
    passage: Passage,
    /// The old text of it.
    old: String,
}

/// The document shown marked as a proof, in place of the text.
struct Proof {
    revision: u64,
    source: Source,
    /// Where the marks are in it, to go through them.
    marks: Vec<usize>,
    at: Option<usize>,
    /// A `]` or `[` waiting for its `c`, or a `g` for its `g`.
    bracket: Option<bool>,
    g: bool,
}

/// A question for git, on a thread of its own, and its answer.
enum Answer {
    Status(PathBuf, Result<git::Status, String>),
    Text(PathBuf, String, Result<(Source, Commit), String>),
    Log(Result<Vec<Commit>, String>),
    ToCommit(Result<Vec<PathBuf>, String>, String),
    Committed(Result<String, String>),
}

/// What is known of git.
#[derive(Default)]
struct Gits {
    on: bool,
    /// The last answer about the file, and the file and saved state it
    /// was asked for.
    status: Option<Result<git::Status, String>>,
    asked: Option<(PathBuf, Option<StateId>)>,
    /// The texts of files in commits, by file and revision as typed.
    texts: HashMap<(PathBuf, String), Result<(Source, Commit), String>>,
    /// The texts asked for and not back.
    fetching: Vec<(PathBuf, String)>,
    /// A commit asked about: its files and message, until y or n.
    commit: Option<(Vec<PathBuf>, String)>,
    /// How many questions are out.
    out: usize,
    answers: Option<(Sender<Answer>, Receiver<Answer>)>,
}

/// What the window keeps of changes.
pub(super) struct Changes {
    shown: bool,
    since: Since,
    /// The text when the window opened each file.
    opened: HashMap<FileId, Source>,
    reference: Option<Reference>,
    found: Option<Found>,
    placed: HashMap<usize, Placed>,
    slip: Option<Slip>,
    proof: Option<Proof>,
    /// `:changes pdf` waiting for the proof's pages: what to write.
    pdf: Option<String>,
    git: Gits,
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
    generations: u64,
}

impl Default for Changes {
    fn default() -> Self {
        Self {
            shown: true,
            since: Since::Saved,
            opened: HashMap::new(),
            reference: None,
            found: None,
            placed: HashMap::new(),
            slip: None,
            proof: None,
            pdf: None,
            git: Gits::default(),
            wake: None,
            generations: 0,
        }
    }
}

impl Changes {
    fn generation(&mut self) -> u64 {
        self.generations += 1;
        self.generations
    }
}

/// Where git is asked from for `file`: its directory. In tests, never
/// above it, and without the user's settings.
fn repo_for(file: &Path) -> Repo {
    let dir = file.parent().unwrap_or(Path::new("."));
    if cfg!(test) {
        Repo {
            dir: dir.to_owned(),
            ceiling: dir.parent().map(Path::to_owned),
            isolated: true,
        }
    } else {
        Repo::at(dir)
    }
}

/// Whether `text` holds only words and blanks: nothing markup reads as
/// more than letters, so that marks may be put around it.
fn plain(text: &str) -> bool {
    const MARKUP: &[char] = &[
        '#', '$', '*', '_', '`', '<', '>', '@', '[', ']', '\\', '/', '=', '~', '{', '}',
    ];
    !text.contains(MARKUP)
        && text.split_whitespace().all(|word| {
            let marker = matches!(word, "-" | "+" | "--" | "---")
                || (word.ends_with('.')
                    && word[..word.len() - 1].bytes().all(|b| b.is_ascii_digit()));
            !marker
        })
}

/// Whether byte `at` of `source` is in running text.
fn in_markup(source: &Source, at: usize) -> bool {
    at >= source.text().len() || typst_text::ground(source, at) == Ground::Markup
}

/// Where the words of `range` of `source` stand in running text, from
/// end to end.
fn markup_between(source: &Source, range: &Range<usize>) -> bool {
    in_markup(source, range.start)
        && in_markup(source, range.end.saturating_sub(1).max(range.start))
}

/// The edits that mark `passage` of `new` (whose old text is in `old`)
/// in place: old words struck through before the new ones, new words
/// tinted. `None` where its words are not plain running text.
fn marks_of(new: &Source, old: &str, passage: &Passage) -> Option<Vec<(Range<usize>, String)>> {
    let text = new.text();
    let mut edits = Vec::new();
    if passage.is_deletion() {
        let was = old.get(passage.old.clone())?;
        let at = passage.new.start;
        if !plain(was) || !in_markup(new, at) {
            return None;
        }
        let gone = format!("#zetwerk-gone[{was}]");
        let put = if at >= text.len() {
            format!("\n\n{gone}\n")
        } else {
            format!("{gone}\n\n")
        };
        edits.push((at..at, put));
        return Some(edits);
    }
    for (now, then) in &passage.words {
        let (is, was) = (text.get(now.clone())?, old.get(then.clone())?);
        if !plain(is) || !plain(was) || !markup_between(new, now) {
            return None;
        }
        let struck = if was.is_empty() {
            String::new()
        } else {
            format!("#zetwerk-old[{was}]")
        };
        let put = match (struck.is_empty(), is.is_empty()) {
            (_, false) if struck.is_empty() => format!("#zetwerk-new[{is}]"),
            (_, false) => format!("{struck} #zetwerk-new[{is}]"),
            // Only taken out: before the word that follows, or behind
            // the last.
            (_, true) => {
                let next = text[now.start..].chars().next();
                if next.is_some_and(|c| !c.is_whitespace()) {
                    format!("{struck} ")
                } else {
                    format!(" {struck}")
                }
            }
        };
        edits.push((now.clone(), put));
    }
    Some(edits)
}

/// Where a bar goes for `passage` of `new` that cannot carry marks: at
/// its first changed word, if that is in running text.
fn bar_of(new: &Source, passage: &Passage) -> Option<(Range<usize>, String)> {
    let at = passage
        .words
        .first()
        .map_or(passage.new.start, |(now, _)| now.start);
    in_markup(new, at).then(|| (at..at, "#zetwerk-bar()".to_owned()))
}

/// How a proof came out: the changes marked word by word, those only
/// with a bar, and those not marked at all.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Marked {
    words: usize,
    bars: usize,
    none: usize,
}

/// `new` with `edits` made (in the order of the text, apart), behind
/// the rules of a proof; and where each edit is in it.
fn applied(new: &str, edits: &[(Range<usize>, String)]) -> (String, Vec<usize>) {
    let mut proof = String::with_capacity(new.len() + PROOF_RULES.len() + edits.len() * 40);
    proof.push_str(PROOF_RULES);
    let mut marks = Vec::new();
    let mut at = 0;
    for (range, put) in edits {
        proof.push_str(&new[at..range.start]);
        marks.push(proof.len());
        proof.push_str(put);
        at = range.end;
    }
    proof.push_str(&new[at..]);
    (proof, marks)
}

/// The text of a proof of `new` against `old`: each change marked in
/// place where it can be, else by a bar beside it; and if marks would
/// keep the text from being read at all, by bars only. Also where the
/// marks are, and how it came out.
fn proof_text(new: &Source, old: &str, passages: &[Passage]) -> (String, Vec<usize>, Marked) {
    let broken = |text: &str| typst::syntax::parse(text).errors_and_warnings().0.len();
    let before = new.root().errors_and_warnings().0.len();
    for words_too in [true, false] {
        let mut edits = Vec::new();
        let mut marked = Marked::default();
        for passage in passages {
            if let Some(marks) = words_too.then(|| marks_of(new, old, passage)).flatten() {
                edits.extend(marks);
                marked.words += 1;
            } else if let Some(bar) = bar_of(new, passage) {
                edits.push(bar);
                marked.bars += 1;
            } else {
                marked.none += 1;
            }
        }
        edits.sort_by_key(|(range, _)| (range.start, range.end));
        let (proof, marks) = applied(new.text(), &edits);
        if broken(&proof) <= before {
            return (proof, marks, marked);
        }
    }
    let (proof, marks) = applied(new.text(), &[]);
    let marked = Marked {
        none: passages.len(),
        ..Marked::default()
    };
    (proof, marks, marked)
}

/// The first `most` words of `piece` of Typst markup as they are read.
fn first_words(piece: &str, most: usize) -> String {
    let read = crate::reading::reading(&Source::detached(piece));
    let words: Vec<&str> = read.text().split_whitespace().collect();
    let mut said = words
        .iter()
        .take(most)
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    if words.len() > most {
        said.push('\u{2026}');
    }
    said
}

/// What `passage` did, in a few words.
fn described(passage: &Passage, new: &str, old: &str) -> String {
    let words = |text: &str| text.split_whitespace().count();
    let (put, taken): (usize, usize) = passage
        .words
        .iter()
        .map(|(now, then)| (words(&new[now.clone()]), words(&old[then.clone()])))
        .fold((0, 0), |(a, b), (c, d)| (a + c, b + d));
    if passage.is_deletion() {
        let was = first_words(&old[passage.old.clone()], LISTED_WORDS);
        return format!("taken out: \u{201c}{was}\u{201d}");
    }
    let is = first_words(&new[passage.new.clone()], LISTED_WORDS);
    let how = match (put, taken) {
        (_, 0) if passage.old.is_empty() => format!("new, {put} words"),
        (put, 0) => format!("+{put} words"),
        (0, taken) => format!("\u{2212}{taken} words"),
        (put, taken) => format!("+{put} \u{2212}{taken} words"),
    };
    format!("\u{201c}{is}\u{201d}\t{how}")
}

/// The heading `at` is under in `text`, if any: its words.
fn section_of(text: &str, at: usize) -> Option<String> {
    let mut before = &text[..at.min(text.len())];
    loop {
        let start = before.rfind('\n').map_or(0, |found| found + 1);
        let line = &before[start..];
        if line.starts_with('=') {
            return Some(first_words(line, 5));
        }
        if start == 0 {
            return None;
        }
        before = &before[..start - 1];
    }
}

/// The value of `key` of the `#set text` rule of `source`, as a family
/// name or a size in points.
fn page_style(source: &Source) -> (Option<String>, f64) {
    let family = typst_text::rule_value(source, "text", "font").and_then(|font| {
        let font = font.trim();
        let name = font.strip_prefix('"')?.strip_suffix('"')?;
        (!name.contains('"')).then(|| name.to_owned())
    });
    let size = typst_text::rule_value(source, "text", "size")
        .and_then(|size| size.trim().strip_suffix("pt")?.trim().parse::<f64>().ok())
        .filter(|size| *size > 1.0 && *size < 100.0)
        .unwrap_or(USUAL_SIZE);
    (family, size)
}

impl Viewer {
    /// Shows the bars beside the lines that changed, or not
    /// (`:set changes.bars`).
    pub(super) fn changes_shown(&mut self, shown: bool) {
        self.changes.shown = shown;
    }

    /// Takes what the user's settings say of changes.
    pub(super) fn changes_configured(&mut self, options: &ChangeOptions) {
        self.changes.shown = options.bars;
        self.changes.git.on = options.git;
        if let Some(since) = Since::read(&options.since) {
            self.changes.since = since;
        }
    }

    /// Notes the text as the window opened it, to compare with later.
    pub(super) fn changes_opened(&mut self) {
        if let Some(editor) = &self.editor {
            self.changes
                .opened
                .entry(editor.file())
                .or_insert_with(|| editor.source().clone());
        }
    }

    /// Has `wake` called when git has answered: it should make the window
    /// draw.
    pub fn wake_for_changes(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        self.changes.wake = Some(Arc::new(wake));
    }

    /// Asks git on a thread of its own; the answer is taken up in a
    /// frame (see [`Viewer::changes_frame`]).
    fn ask_git(&mut self, ask: impl FnOnce() -> Answer + Send + 'static) {
        let git = &mut self.changes.git;
        let (send, _) = git.answers.get_or_insert_with(mpsc::channel);
        let send = send.clone();
        let wake = self.changes.wake.clone();
        let started = std::thread::Builder::new()
            .name("zetwerk-git".into())
            .spawn(move || {
                let _ = send.send(ask());
                if let Some(wake) = wake {
                    wake();
                }
            });
        if started.is_ok() {
            git.out += 1;
        }
    }

    /// The file the window edits, and its path.
    fn edited_path(&self) -> Option<(FileId, PathBuf)> {
        let editor = self.editor.as_ref()?;
        Some((editor.file(), editor.path().to_owned()))
    }

    /// The version compared with, of the file being edited: worked out
    /// when it is not known yet. `None` where there is none (a file never
    /// saved; a commit git has not given yet, or does not have).
    fn changes_reference(&mut self) -> Option<&Reference> {
        let editor = self.editor.as_ref()?;
        let file = editor.file();
        let since = self.changes.since.clone();
        let state = match since {
            Since::Saved => Some(editor.saved_state()?),
            _ => None,
        };
        let of = (since.clone(), state);
        let known = self
            .changes
            .reference
            .as_ref()
            .is_some_and(|reference| reference.file == file && reference.of == of);
        if !known {
            let source = match &since {
                Since::Saved | Since::Step(_) => {
                    let state = state.or(match since {
                        Since::Step(step) => Some(step),
                        _ => None,
                    })?;
                    if state == editor.history().current() {
                        editor.source().clone()
                    } else {
                        Source::new(file, editor.text_at(state)?)
                    }
                }
                Since::Opened => self.changes.opened.get(&file)?.clone(),
                Since::Git(revision) => {
                    let path = editor.path().to_owned();
                    let key = (path.clone(), revision.clone());
                    match self.changes.git.texts.get(&key) {
                        Some(Ok((source, _))) => source.clone(),
                        Some(Err(_)) => return None,
                        None => {
                            if !self.changes.git.fetching.contains(&key) {
                                self.changes.git.fetching.push(key);
                                let (revision, repo) = (revision.clone(), repo_for(&path));
                                self.ask_git(move || {
                                    let text = git::show(&repo, &revision, &path)
                                        .map(|(text, commit)| (Source::detached(text), commit));
                                    Answer::Text(path, revision, text)
                                });
                            }
                            return None;
                        }
                    }
                }
            };
            let paragraphs = diff::paragraphs(&source);
            let generation = self.changes.generation();
            self.changes.reference = Some(Reference {
                file,
                of,
                source,
                paragraphs,
                generation,
            });
        }
        self.changes.reference.as_ref()
    }

    /// What differs between the text and the version compared with,
    /// worked out again where either changed.
    fn changes_found(&mut self) -> Option<&Found> {
        let Some((file, reference)) = self
            .changes_reference()
            .map(|reference| (reference.file, reference.generation))
        else {
            self.changes.found = None;
            return None;
        };
        let revision = self.revision_of_text();
        let known = self.changes.found.as_ref().is_some_and(|found| {
            (found.file, found.revision, found.reference) == (file, revision, reference)
        });
        if !known {
            let (editor, compared) = (self.editor.as_ref()?, self.changes.reference.as_ref()?);
            let source = editor.source();
            let passages = if std::ptr::eq(source.text(), compared.source.text()) {
                Vec::new()
            } else {
                let paragraphs = diff::paragraphs(source);
                diff::passages(
                    compared.source.text(),
                    &compared.paragraphs,
                    source.text(),
                    &paragraphs,
                )
            };
            let generation = self.changes.generation();
            self.changes.found = Some(Found {
                file,
                revision,
                reference,
                passages,
                generation,
            });
        }
        self.changes.found.as_ref()
    }

    /// The changes as they are now, if they are known.
    fn passages(&mut self) -> Vec<Passage> {
        self.changes_found()
            .map(|found| found.passages.clone())
            .unwrap_or_default()
    }

    /// Runs `ask` on the lines of page `index`, as the cursor and the
    /// numbers of the lines know them.
    pub(super) fn changes_lines<T>(
        &self,
        index: usize,
        ask: impl FnOnce(&PageLines) -> T,
    ) -> Option<T> {
        let rows = PageRows {
            document: self.document.as_ref()?,
            world: self.compiled_world.as_ref()?,
            file: self.editor.as_ref()?.file(),
            lines: &self.page_lines,
            cursor_page: &self.cursor_page,
            change: Change::NONE,
            landed: &self.landed,
            elsewhere: &self.elsewhere,
        };
        rows.on(index, ask)
    }

    /// Where the marks of page `index` go: by the lines of its pages,
    /// where the text is to be found on them by `change`.
    fn place_changes(&self, index: usize, change: Change, found: &Found) -> Option<Placed> {
        let file = found.file;
        let back =
            |range: &Range<usize>| change.back_or_edge(range.start)..change.back_or_edge(range.end);
        let mut ranges = Vec::new();
        let mut points = Vec::new();
        let mut gone = Vec::new();
        for passage in &found.passages {
            if passage.is_deletion() {
                gone.push(change.back_or_edge(passage.new.start));
                continue;
            }
            for (now, _) in &passage.words {
                let then = back(now);
                if then.is_empty() {
                    points.push(then.start);
                } else {
                    ranges.push(then);
                }
            }
        }
        ranges.sort_by_key(|range| (range.start, range.end));
        self.changes_lines(index, |lines| {
            let shown = lines.showing(file, &ranges);
            let mut touched: Vec<usize> = shown.iter().map(|(line, _)| *line).collect();
            // A change that shows nothing (a label, a rule): the line of
            // what is before it.
            let unseen = (0..ranges.len()).filter(|at| !shown.iter().any(|(_, of)| of == at));
            let near = |at: usize| {
                lines
                    .covers(file, at)
                    .then(|| lines.find(file, at).map(|(line, _)| line))
                    .flatten()
            };
            touched.extend(unseen.filter_map(|at| near(ranges[at].start)));
            touched.extend(points.iter().filter_map(|at| near(*at)));
            touched.sort_unstable();
            touched.dedup();
            let edges = Edges::of(lines);
            let mut bars: Vec<(f64, f64, f64)> = Vec::new();
            let mut bands: Vec<(f64, f64, f64)> = touched
                .into_iter()
                .filter_map(|line| {
                    let (top, bottom) = lines.band(line)?;
                    Some((edges.of_line(lines, line), top, bottom))
                })
                .collect();
            bands.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
            for (edge, top, bottom) in bands {
                match bars.last_mut() {
                    Some(last)
                        if (last.0 - edge).abs() < 0.5
                            && top - last.2 < JOINED * (bottom - top)
                            && top >= last.1 =>
                    {
                        last.2 = last.2.max(bottom);
                    }
                    _ => bars.push((edge, top, bottom)),
                }
            }
            let gone: Vec<(f64, f64)> = gone
                .iter()
                .filter_map(|at| {
                    let before = at
                        .checked_sub(1)
                        .filter(|before| lines.covers(file, *before));
                    if let Some((line, _)) = before.and_then(|before| lines.find(file, before)) {
                        let (_, bottom) = lines.band(line)?;
                        return Some((edges.of_line(lines, line), bottom + 1.0));
                    }
                    let (line, _) = lines.covers(file, *at).then(|| lines.find(file, *at))??;
                    let (top, _) = lines.band(line)?;
                    Some((edges.of_line(lines, line), top - 1.0))
                })
                .collect();
            Placed {
                key: (found.generation, 0, change),
                bars,
                gone,
            }
        })
    }

    /// Draws beside the lines of page `index` (whose top-left corner is
    /// at `origin`) which of them changed, and a paragraph set as it was
    /// if it is on this page.
    pub(super) fn change_bars(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        index: usize,
        origin: (f64, f64),
    ) {
        if !self.changes.shown || self.editor.is_none() || self.changes.proof.is_some() {
            return;
        }
        let Some((generation, empty)) = self
            .changes_found()
            .map(|found| (found.generation, found.passages.is_empty()))
        else {
            self.changes.placed.clear();
            return;
        };
        if empty {
            self.changes.placed.clear();
            return;
        }
        let document = self
            .document
            .as_ref()
            .map_or(0, |document| document.pages().as_ptr() as usize);
        // Where the pages cannot say where the text is (they are being
        // made), the marks stay where they were: they would go and come
        // with every key.
        if let Some(change) = self.places_of_text() {
            let key = (generation, document, change);
            let stale = self
                .changes
                .placed
                .get(&index)
                .is_none_or(|placed| placed.key != key);
            if stale && let Some(found) = self.changes.found.take() {
                let placed = self.place_changes(index, change, &found);
                self.changes.found = Some(found);
                match placed {
                    Some(placed) => {
                        self.changes.placed.insert(index, Placed { key, ..placed });
                    }
                    None => {
                        self.changes.placed.remove(&index);
                    }
                }
            }
        }
        let zoom = self.view.zoom;
        let width = (BAR_WIDTH * self.scale).round().max(1.0);
        if let Some(placed) = self.changes.placed.get(&index) {
            for (edge, top, bottom) in &placed.bars {
                let x = (origin.0 + (edge - BAR_GAP) * zoom - width).round();
                let y = origin.1 + top * zoom;
                tint(
                    scene,
                    has,
                    CHANGE_BAR,
                    (x, y.round(), width, ((bottom - top) * zoom).round()),
                );
            }
            for (edge, y) in &placed.gone {
                let x = origin.0 + (edge - BAR_GAP - GONE_LENGTH / 2.0) * zoom;
                let y = origin.1 + y * zoom;
                let length = GONE_LENGTH * zoom;
                tint(
                    scene,
                    has,
                    CHANGE_GONE,
                    (x.round(), (y - width / 2.0).round(), length.round(), width),
                );
                // And a stroke down from where the bars stand: a caret.
                let stem = (origin.0 + (edge - BAR_GAP) * zoom - width).round();
                tint(
                    scene,
                    has,
                    CHANGE_GONE,
                    (stem, (y - 3.0 * zoom).round(), width, (3.0 * zoom).round()),
                );
            }
        }
        self.slip_page(scene, has, index, origin);
    }

    /// What the status line says of changes: how many there are, and
    /// git's branch.
    pub(super) fn changes_status(&mut self) -> String {
        let mut said = String::new();
        if let Some(proof) = &self.changes.proof {
            let _ = write!(said, "proof of {} changes  ", proof.marks.len());
        } else if self.changes.shown
            && let Some(count) = self.changes_found().map(|found| found.passages.len())
            && count > 0
        {
            let what = if count == 1 { "change" } else { "changes" };
            match &self.changes.since {
                Since::Saved => {
                    let _ = write!(said, "{count} {what} ]v  ");
                }
                since => {
                    let _ = write!(said, "{count} {what} since {} ]v  ", since.called());
                }
            }
        }
        if let Some(Ok(status)) = &self.changes.git.status {
            let branch = status.branch.as_deref().unwrap_or("no branch");
            let state = match (status.file, status.born) {
                (_, false) => ", no commits",
                (FileState::Changed, _) => "*",
                (FileState::Untracked, _) => ", new file",
                (FileState::Same | FileState::Ignored, _) => "",
            };
            let _ = write!(said, "git {branch}{state}  ");
        }
        said
    }

    /// Before a frame is drawn: what git said is taken up, and git is
    /// asked about the file where it was not since it was opened or
    /// saved.
    pub(super) fn changes_frame(&mut self) {
        self.git_answers();
        if self.changes.git.on
            && let Some((_, path)) = self.edited_path()
        {
            let saved = self.editor.as_ref().and_then(editor::Editor::saved_state);
            let asked = Some((path.clone(), saved));
            if self.changes.git.asked != asked {
                self.changes.git.asked = asked;
                let repo = repo_for(&path);
                self.ask_git(move || {
                    let status = git::status(&repo, &path);
                    Answer::Status(path, status)
                });
            }
        }
        // The proof's pages: if they would not typeset, back to the text.
        if let Some(of) = self.changes.proof.as_ref().map(|proof| proof.revision) {
            let refused = self.compiled_revision == Some(self.revision)
                && self.document_revision != Some(self.revision);
            if of != self.revision_of_text() {
                self.leave_proof("the text changed: the proof is put away");
            } else if refused {
                self.leave_proof(
                    "the proof could not be typeset: the bars beside the page show the changes",
                );
            } else if self.document_revision == Some(self.revision)
                && let Some(name) = self.changes.pdf.take()
            {
                self.export_command("pdf", &name);
            }
        }
    }

    /// The revision of the text being edited.
    fn revision_of_text(&self) -> u64 {
        self.editor.as_ref().map_or(0, editor::Editor::revision)
    }

    /// Takes up what git answered.
    fn git_answers(&mut self) {
        let mut answers = Vec::new();
        if let Some((_, receive)) = &self.changes.git.answers {
            while let Ok(answer) = receive.try_recv() {
                answers.push(answer);
            }
        }
        for answer in answers {
            self.changes.git.out = self.changes.git.out.saturating_sub(1);
            self.git_answered(answer);
        }
    }

    fn git_answered(&mut self, answer: Answer) {
        match answer {
            Answer::Status(path, status) => {
                if self.edited_path().is_some_and(|(_, now)| now == path) {
                    self.changes.git.status = Some(status);
                }
            }
            Answer::Text(path, revision, text) => {
                let key = (path, revision.clone());
                self.changes.git.fetching.retain(|asked| *asked != key);
                if let Err(why) = &text
                    && self.changes.since == Since::Git(revision.clone())
                {
                    self.alert(format!("cannot compare with {revision}: {why}"));
                }
                if let Ok((_, commit)) = &text
                    && self.changes.since == Since::Git(revision.clone())
                {
                    let told = format!(
                        "compared with {revision}: {} {} \u{201c}{}\u{201d}  (]v goes to the changes)",
                        commit.hash, commit.date, commit.subject
                    );
                    self.notice(told);
                }
                self.changes.git.texts.insert(key, text);
            }
            Answer::Log(Ok(commits)) => {
                let items = commits
                    .into_iter()
                    .map(|commit| {
                        let label = format!("{}  {}\t{}", commit.date, commit.subject, commit.hash);
                        (
                            label,
                            Target::Command(format!("changes since {}", commit.hash)),
                        )
                    })
                    .collect();
                self.open_picker("commits of this file", items, 0);
            }
            Answer::Log(Err(why)) | Answer::ToCommit(Err(why), _) => {
                self.alert(format!("git: {why}"));
            }
            Answer::ToCommit(Ok(files), message) => self.ask_commit(files, message),
            Answer::Committed(result) => {
                match result {
                    Ok(said) => self.notice(format!("committed: {said}")),
                    Err(why) => self.alert(format!("not committed: {why}")),
                }
                self.git_changed();
            }
        }
    }

    /// Git changed something: what it says is asked again.
    fn git_changed(&mut self) {
        self.changes.git.asked = None;
        self.changes.git.texts.clear();
        if matches!(self.changes.since, Since::Git(_)) {
            self.changes.reference = None;
        }
    }

    /// The files whose change means that git is to be asked again: its
    /// `HEAD` and its index, for the file watcher.
    #[must_use]
    pub fn git_watched(&self) -> Vec<PathBuf> {
        match &self.changes.git.status {
            Some(Ok(status)) => vec![status.git_dir.join("HEAD"), status.git_dir.join("index")],
            _ => Vec::new(),
        }
    }

    /// Files changed on disk: if git's `HEAD` or index are among them,
    /// git is asked again (in the next frame).
    pub fn git_files_changed(&mut self, paths: &[PathBuf]) {
        let watched = self.git_watched();
        if paths.iter().any(|path| watched.contains(path)) {
            self.git_changed();
        }
    }

    /// Waits until git has answered all it was asked, for a test.
    #[cfg(test)]
    pub(super) fn git_settled(&mut self) {
        while self.changes.git.out > 0 {
            let answer = self.changes.git.answers.as_ref().and_then(|(_, receive)| {
                receive
                    .recv_timeout(std::time::Duration::from_secs(30))
                    .ok()
            });
            let Some(answer) = answer else {
                break;
            };
            self.changes.git.out -= 1;
            self.git_answered(answer);
        }
    }

    /// Whether a paragraph is set as it was, for a test.
    #[cfg(test)]
    pub(super) fn slip_shown(&self) -> bool {
        self.changes.slip.is_some()
    }

    /// Turns git on, for a test.
    #[cfg(test)]
    pub(super) fn git_on(&mut self) {
        self.changes.git.on = true;
    }

    /// The bars drawn on page `index`, as last placed, for a test.
    #[cfg(test)]
    pub(super) fn changes_placed(&self, index: usize) -> (usize, usize) {
        self.changes
            .placed
            .get(&index)
            .map_or((0, 0), |placed| (placed.bars.len(), placed.gone.len()))
    }

    /// A key that may be for changes: for a paragraph set as it was, for
    /// the proof, or the answer to whether to commit. Returns whether it
    /// was.
    pub(super) fn changes_key(&mut self, press: KeyPress) -> bool {
        let commanding = self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.mode() == editor::Mode::Command);
        if commanding {
            return false;
        }
        if self.changes.git.commit.is_some() {
            self.commit_key(press);
            return true;
        }
        if self.changes.proof.is_some() {
            return self.proof_key(press);
        }
        if self.changes.slip.is_none() {
            return false;
        }
        let current = self.changes.slip.as_ref().is_some_and(|slip| {
            Some(slip.revision) == self.editor.as_ref().map(editor::Editor::revision)
        });
        if !current {
            self.changes.slip = None;
            return false;
        }
        match (press.key, press.ctrl) {
            (Key::Enter, _) => {
                self.revert();
                true
            }
            (Key::Char('y'), false) => {
                if let (Some(slip), Some(editor)) = (self.changes.slip.take(), &mut self.editor) {
                    editor.set_register('"', &slip.old);
                    editor.set_register('0', &slip.old);
                    self.notice("the old text is yanked: p puts it");
                }
                true
            }
            (Key::Escape, _) | (Key::Char('['), true) => {
                self.changes.slip = None;
                true
            }
            // `zc` again takes it away: the `z` waits for it.
            (Key::Char('z'), false) => false,
            _ => {
                self.changes.slip = None;
                false
            }
        }
    }

    /// The commands of changes and of git. Returns whether `command` was
    /// one of them.
    pub(super) fn changes_command(&mut self, command: &str) -> bool {
        let (name, rest) = command.split_once(' ').unwrap_or((command, ""));
        let rest = rest.trim();
        match (name, rest) {
            // `:changes` alone is Vim's list of the places of changes.
            ("changes", "list") | ("changed", "") => self.list_changes(),
            ("changes", "on" | "off") => {
                self.changes.shown = rest == "on";
                self.notice(if self.changes.shown {
                    "lines that changed have a bar beside them (]v goes to the next)"
                } else {
                    "no bars beside the lines that changed"
                });
            }
            ("changes", "next") => self.goto_change(true),
            ("changes", "previous") => self.goto_change(false),
            ("changes", "flip") => self.flip_change(),
            ("changes", "yank") => self.yank_change(),
            ("changes", "proof") => {
                if self.changes.proof.is_some() {
                    self.leave_proof("back to the text");
                } else {
                    self.show_proof();
                }
            }
            ("changes", "since") => self.choose_since(),
            ("changes", "since history" | "since steps") => self.choose_step(),
            ("changes", wanted) if wanted.starts_with("since ") => {
                self.compare_with(wanted.trim_start_matches("since ").trim());
            }
            ("changes", wanted) if wanted == "pdf" || wanted.starts_with("pdf ") => {
                let name = wanted.trim_start_matches("pdf").trim();
                self.proof_pdf(name);
            }
            ("revert", "") => self.revert(),
            ("git", "") => self.say_git(),
            ("git", "log") => self.git_log(),
            ("git", message) if message == "commit" || message.starts_with("commit ") => {
                self.git_commit(message.trim_start_matches("commit").trim());
            }
            ("changes" | "git", _) => {
                self.alert(format!(
                    "not a command: {command} (:changes list, :changes since …, :git log, :git commit …)"
                ));
            }
            _ => return false,
        }
        true
    }

    /// `:changes since <version>`.
    fn compare_with(&mut self, wanted: &str) {
        let Some(since) = Since::read(wanted) else {
            return self.alert(
                "since what? saved, opened, a step (:changes since history), or a commit of git",
            );
        };
        if let Since::Git(revision) = &since {
            if revision.starts_with('-') || revision.contains(char::is_whitespace) {
                return self.alert(format!("not a revision of git: {revision}"));
            }
            if !matches!(self.changes.git.status, Some(Ok(_))) {
                let why = match &self.changes.git.status {
                    Some(Err(why)) => why.clone(),
                    _ => "git has not said yet".to_owned(),
                };
                return self.alert(format!(
                    "no git here ({why}): since saved, opened or a step"
                ));
            }
            self.changes
                .git
                .texts
                .retain(|(_, asked), _| asked != revision);
        }
        self.changes.since = since.clone();
        self.changes.shown = true;
        self.changes.reference = None;
        self.changes.slip = None;
        self.changes.placed.clear();
        let count = self.changes_found().map(|found| found.passages.len());
        match (count, &since) {
            (None, Since::Git(revision)) => {
                self.notice(format!("asking git for the file in {revision} …"));
            }
            (None, _) => self.alert("that version is not known: nothing to compare with"),
            (Some(count), since) => self.notice(format!(
                "{count} changes since {}  (]v goes to them, :changes list lists them)",
                since.called()
            )),
        }
    }

    /// `:changes since` alone: the versions there are to compare with.
    fn choose_since(&mut self) {
        let mut items = vec![
            (
                "the file as last saved\tsaved".to_owned(),
                "changes since saved",
            ),
            (
                "the text when the window opened it\topened".to_owned(),
                "changes since opened",
            ),
            (
                "a step of the history\u{2026}\t:changes since history".to_owned(),
                "changes since history",
            ),
        ];
        if matches!(self.changes.git.status, Some(Ok(_))) {
            items.push(("the last commit\tHEAD".to_owned(), "changes since HEAD"));
            items.push((
                "a commit of this file\u{2026}\t:git log".to_owned(),
                "git log",
            ));
        }
        let items = items
            .into_iter()
            .map(|(label, command)| (label, Target::Command(command.to_owned())))
            .collect();
        self.open_picker("compare with", items, 0);
    }

    /// `:changes since history`: the states of the history, to compare
    /// with one.
    fn choose_step(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        let rows = super::history::rows(editor.history());
        let items: Vec<(String, Target)> = rows
            .into_iter()
            .filter_map(|(label, target)| {
                let Target::Command(command) = target else {
                    return None;
                };
                let state = command.strip_prefix("history go ")?;
                Some((
                    label,
                    Target::Command(format!("changes since step {state}")),
                ))
            })
            .collect();
        self.open_picker("compare with a step", items, 0);
    }

    /// `:changes`: the changes, to go to.
    fn list_changes(&mut self) {
        let passages = self.passages();
        let (Some(editor), Some(reference)) = (&self.editor, &self.changes.reference) else {
            return self.notice("nothing to compare with: the file was never saved");
        };
        let (new, old) = (editor.text(), reference.source.text());
        let cursor = editor.cursor();
        let items: Vec<(String, Target)> = passages
            .iter()
            .map(|passage| {
                let at = passage
                    .words
                    .first()
                    .map_or(passage.new.start, |(now, _)| now.start);
                let section =
                    section_of(new, passage.new.start).unwrap_or_else(|| "\u{2014}".to_owned());
                let what = described(passage, new, old);
                (format!("{section}  {what}"), Target::Offset(at))
            })
            .collect();
        let chosen = passages
            .iter()
            .position(|passage| passage.new.end >= cursor)
            .unwrap_or(0);
        let since = self.changes.since.called();
        if items.is_empty() {
            return self.notice(format!("no changes since {since}"));
        }
        self.open_picker("changes", items, chosen);
        self.notice(format!(
            "since {since}: Enter goes to it, zc shows what it was"
        ));
    }

    /// Where `passage` begins for the cursor: its first changed word.
    fn change_start(passage: &Passage) -> usize {
        passage
            .words
            .first()
            .map_or(passage.new.start, |(now, _)| now.start)
    }

    /// `]v` and `[v`: to the next change, or the one before.
    fn goto_change(&mut self, forward: bool) {
        let passages = self.passages();
        let Some(editor) = &mut self.editor else {
            return;
        };
        if passages.is_empty() {
            let since = self.changes.since.called();
            return self.notice(format!("no changes since {since}"));
        }
        let cursor = editor.cursor();
        let starts: Vec<usize> = passages.iter().map(Self::change_start).collect();
        let (at, wrapped) = if forward {
            match starts.iter().position(|start| *start > cursor) {
                Some(at) => (at, false),
                None => (0, true),
            }
        } else {
            match starts.iter().rposition(|start| *start < cursor) {
                Some(at) => (at, false),
                None => (starts.len() - 1, true),
            }
        };
        let last = editor.text().len().saturating_sub(1);
        editor.set_cursor(starts[at].min(last));
        self.changes.slip = None;
        let again = if wrapped { " (from the other end)" } else { "" };
        self.notice(format!(
            "change {} of {}{again}  (zc: what it was, :changes list)",
            at + 1,
            starts.len()
        ));
        self.follow_cursor();
    }

    /// The change the cursor is in, or beside where paragraphs were
    /// taken out.
    fn change_here(&mut self) -> Option<Passage> {
        let passages = self.passages();
        let editor = self.editor.as_ref()?;
        let (text, cursor) = (editor.text(), editor.cursor());
        if let Some(passage) = passages.iter().find(|passage| {
            !passage.is_deletion() && passage.new.start <= cursor && cursor <= passage.new.end
        }) {
            return Some(passage.clone());
        }
        // Paragraphs taken out: from the paragraph after them or before.
        let line_start = text[..cursor.min(text.len())]
            .rfind("\n\n")
            .map_or(0, |at| at + 2);
        let paragraph_end = text[cursor.min(text.len())..]
            .find("\n\n")
            .map_or(text.len(), |at| cursor + at);
        let after = text[paragraph_end..]
            .find(|c: char| !c.is_whitespace())
            .map_or(text.len(), |at| paragraph_end + at);
        passages.into_iter().find(|passage| {
            passage.is_deletion()
                && (passage.new.start == after
                    || (line_start..=paragraph_end.max(line_start)).contains(&passage.new.start))
        })
    }

    /// `zc`: the change here set as it was, over itself; again, taken
    /// away.
    fn flip_change(&mut self) {
        if self.changes.slip.take().is_some() {
            return;
        }
        let Some(passage) = self.change_here() else {
            return self.notice("no change here: ]v goes to the next");
        };
        let (Some(editor), Some(reference)) = (&self.editor, &self.changes.reference) else {
            return;
        };
        let old = reference.source.text()[passage.old.clone()].to_owned();
        self.changes.slip = Some(Slip {
            file: editor.file(),
            revision: editor.revision(),
            passage,
            old,
        });
        let since = self.changes.since.called();
        self.notice(format!(
            "as it was ({since}): struck went, tinted came · Enter takes the old back · y yanks it · Esc"
        ));
    }

    /// `:changes yank`: the old text of the change here, into the
    /// register.
    fn yank_change(&mut self) {
        let Some(passage) = self.change_here() else {
            return self.notice("no change here: ]v goes to the next");
        };
        let old = self
            .changes
            .reference
            .as_ref()
            .map(|reference| reference.source.text()[passage.old.clone()].to_owned());
        if let (Some(old), Some(editor)) = (old, &mut self.editor) {
            editor.set_register('"', &old);
            editor.set_register('0', &old);
            self.notice("the old text is yanked: p puts it");
        }
    }

    /// `:revert`, or Enter on a paragraph set as it was: the change here
    /// is taken back, as one undo step.
    fn revert(&mut self) {
        self.changes.slip = None;
        let Some(passage) = self.change_here() else {
            return self.notice("no change here: ]v goes to the next");
        };
        let Some(old) = self
            .changes
            .reference
            .as_ref()
            .map(|reference| reference.source.text()[passage.old.clone()].to_owned())
        else {
            return;
        };
        let Some(editor) = &mut self.editor else {
            return;
        };
        let text = editor.text();
        let (range, with) = if passage.is_deletion() {
            let at = passage.new.start;
            if at >= text.len() {
                let blanks = if text.ends_with('\n') { "\n" } else { "\n\n" };
                (at..at, format!("{blanks}{old}\n"))
            } else {
                (at..at, format!("{old}\n\n"))
            }
        } else if passage.old.is_empty() {
            // Only put in: it goes, with the blank lines after it.
            let after = text[passage.new.end..]
                .find(|c: char| !c.is_whitespace())
                .map_or(text.len(), |at| passage.new.end + at);
            if after >= text.len() {
                let before = text[..passage.new.start].trim_end().len();
                (before..text.len(), "\n".to_owned())
            } else {
                (passage.new.start..after, String::new())
            }
        } else {
            (passage.new.clone(), old.clone())
        };
        let told = first_words(
            if old.is_empty() {
                &text[passage.new.clone()]
            } else {
                &old
            },
            6,
        );
        match editor.replace(&[(range.clone(), with.as_str())]) {
            Ok(()) => {
                editor.set_cursor(range.start.min(editor.text().len().saturating_sub(1)));
                self.text_changed(Edited::Command);
                let since = self.changes.since.called();
                self.notice(format!(
                    "as it was ({since}): \u{201c}{told}\u{201d}  (u takes it back)"
                ));
            }
            Err(why) => self.alert(format!("not changed: {why}")),
        }
    }

    /// Draws on page `index` the paragraph set as it was, if it is there.
    #[allow(
        clippy::too_many_lines,
        reason = "where it goes, what it says, and drawing it"
    )]
    fn slip_page(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        index: usize,
        origin: (f64, f64),
    ) {
        let Some(slip) = &self.changes.slip else {
            return;
        };
        let current = self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.file() == slip.file && editor.revision() == slip.revision);
        let Some(change) = self.places_of_text().filter(|_| current) else {
            return;
        };
        let (passage, old) = (slip.passage.clone(), slip.old.clone());
        let file = slip.file;
        // Where it is on this page: its lines, from its first.
        let then = change.back_or_edge(passage.new.start)..change.back_or_edge(passage.new.end);
        let area = self.changes_lines(index, |lines| {
            let edges = Edges::of(lines);
            let (left, right) = (edges.page, edges.right);
            if passage.is_deletion() {
                let (line, _) = lines
                    .covers(file, then.start)
                    .then(|| lines.find(file, then.start))??;
                let (_, bottom) = lines.band(line)?;
                return Some((left, bottom + 1.0, right, bottom + 1.0, true));
            }
            let shown = lines.showing(file, std::slice::from_ref(&then));
            let first = shown.iter().map(|(line, _)| *line).min()?;
            // Not on the page it only goes on to.
            if !lines.covers(file, then.start) && first != 0 {
                return None;
            }
            let bands: Vec<(f64, f64)> = shown
                .iter()
                .filter_map(|(line, _)| lines.band(*line))
                .collect();
            let top = bands
                .iter()
                .map(|band| band.0)
                .fold(f64::INFINITY, f64::min);
            let bottom = bands
                .iter()
                .map(|band| band.1)
                .fold(f64::NEG_INFINITY, f64::max);
            Some((left, top, right, bottom, false))
        });
        let Some(Some((left, top, right, bottom, between))) = area else {
            return;
        };
        let Some(editor) = &self.editor else {
            return;
        };
        let source = editor.source();
        let (family, size) = page_style(source);
        let body = slip_body(source, &old, &passage);
        let style = InPlace {
            family: family.as_deref(),
            size,
            weight: 400,
            italic: false,
            width: (right - left).max(40.0),
        };
        let zoom = self.view.zoom;
        let rendered = body
            .as_deref()
            .and_then(|body| {
                self.chrome
                    .render(&chrome::in_place_markup(body, &style, false), zoom)
            })
            .or_else(|| {
                // Marks it cannot carry: set as it was, without them.
                let plainly = old.get(block_mark(&old)..).unwrap_or_default();
                self.chrome
                    .render(&chrome::in_place_markup(plainly, &style, false), zoom)
            });
        let Some(image) = rendered else {
            return;
        };
        let (x, y) = (origin.0 + left * zoom, origin.1 + top * zoom);
        let size = (
            f64::from(image.pixmap.width()),
            f64::from(image.pixmap.height()),
        );
        let covered = if between { 0.0 } else { (bottom - top) * zoom };
        solid(
            scene,
            has,
            PAPER,
            (
                x - 2.0,
                y - 2.0,
                size.0.max((right - left) * zoom) + 4.0,
                size.1.max(covered) + 4.0,
            ),
        );
        let edge = (2.0 * self.scale).round().max(1.0);
        tint(
            scene,
            has,
            CHANGE_SLIP_EDGE,
            ((x - 3.0 * edge).round(), y.round(), edge, size.1.round()),
        );
        scene.quads.push(Quad {
            texture: image.id,
            x: x.round() as f32,
            y: y.round() as f32,
            width: size.0 as f32,
            height: size.1 as f32,
        });
        if !has(image.id) {
            scene.images.push(image);
        }
    }

    /// `:changes proof`: the document marked as a proof, in place of
    /// the text.
    fn show_proof(&mut self) {
        let passages = self.passages();
        let (Some(editor), Some(reference)) = (&self.editor, &self.changes.reference) else {
            return self.notice("nothing to compare with: the file was never saved");
        };
        if passages.is_empty() {
            let since = self.changes.since.called();
            return self.notice(format!("no changes since {since}: nothing to mark"));
        }
        let typing = editor.mode() != editor::Mode::Normal;
        let (text, marks, marked) = proof_text(editor.source(), reference.source.text(), &passages);
        let source = Source::new(editor.file(), text);
        let revision = editor.revision();
        if typing {
            self.editor_key(KeyPress::plain(Key::Escape));
        }
        self.changes.slip = None;
        self.changes.proof = Some(Proof {
            revision,
            source,
            marks,
            at: None,
            bracket: None,
            g: false,
        });
        self.text_changed(Edited::Command);
        let mut told = format!(
            "proof since {}: {} changes marked in the words",
            self.changes.since.called(),
            marked.words
        );
        if marked.bars > 0 {
            let _ = write!(
                told,
                ", {} by a bar only (their markup changed)",
                marked.bars
            );
        }
        if marked.none > 0 {
            let _ = write!(told, ", {} not marked", marked.none);
        }
        told.push_str(" · n N go through them · :changes pdf writes it · Esc");
        self.notice(told);
    }

    /// The proof is put away: the pages are made of the text again.
    fn leave_proof(&mut self, told: &str) {
        if self.changes.proof.take().is_some() {
            self.changes.pdf = None;
            self.text_changed(Edited::Command);
            self.notice(told);
        }
    }

    /// Whether the pages are, or are about to be, those of a proof: where
    /// things are on them says nothing about the text then.
    pub(super) fn proof_shown(&self) -> bool {
        self.changes.proof.is_some()
    }

    /// What to typeset in place of the text while the proof is up.
    pub(super) fn proof_world(&mut self) -> Option<ProjectWorld> {
        let file = self.editor.as_ref().map(editor::Editor::file)?;
        let proof = self.changes.proof.as_ref()?;
        if proof.source.id() != file {
            return None;
        }
        let written = self.project.source(file)?.clone();
        self.project.set_source(proof.source.clone());
        let world = self.project.snapshot();
        self.project.set_source(written);
        Some(world)
    }

    /// `:changes pdf [file]`: the proof, written as a PDF.
    fn proof_pdf(&mut self, name: &str) {
        let name = if name.is_empty() {
            let stem = self
                .main_path()
                .as_deref()
                .and_then(Path::file_stem)
                .map_or_else(
                    || "document".to_owned(),
                    |stem| stem.to_string_lossy().into_owned(),
                );
            format!("{stem}-changes.pdf")
        } else {
            name.to_owned()
        };
        if self.changes.proof.is_none() {
            self.show_proof();
            if self.changes.proof.is_none() {
                return;
            }
        }
        if self.document_revision == Some(self.revision) && self.is_compiled() {
            self.export_command("pdf", &name);
        } else {
            self.changes.pdf = Some(name);
            self.notice("the proof is being typeset; it is written once it is …");
        }
    }

    /// A key while the proof is up: it is read, not written.
    fn proof_key(&mut self, press: KeyPress) -> bool {
        let line = super::LINE_SCROLL * self.scale;
        let height = f64::from(self.view.height);
        let Some(proof) = &mut self.changes.proof else {
            return false;
        };
        let bracket = proof.bracket.take();
        let g = std::mem::take(&mut proof.g);
        match (press.key, press.ctrl) {
            (Key::Char(':'), false) => return false,
            (Key::Escape, _) | (Key::Char('q'), false) => self.leave_proof("back to the text"),
            (Key::Char('v'), false) if bracket.is_some() => self.proof_mark(bracket == Some(true)),
            (Key::Char(']'), false) => proof.bracket = Some(true),
            (Key::Char('['), false) => proof.bracket = Some(false),
            (Key::Char('n'), false) => self.proof_mark(true),
            (Key::Char('N'), false) => self.proof_mark(false),
            (Key::Char('j') | Key::Down, false) => self.scroll(0.0, line),
            (Key::Char('k') | Key::Up, false) => self.scroll(0.0, -line),
            (Key::Char('d'), true) => self.scroll(0.0, height / 2.0),
            (Key::Char('u'), true) => self.scroll(0.0, -height / 2.0),
            (Key::Char('f'), true) | (Key::Char(' ') | Key::PageDown, false) => {
                self.scroll(0.0, height * 0.9);
            }
            (Key::Char('b'), true) | (Key::PageUp, false) => self.scroll(0.0, -height * 0.9),
            (Key::Char('G') | Key::End, _) => self.scroll(0.0, f64::INFINITY),
            (Key::Char('g'), false) if g => self.scroll(0.0, f64::NEG_INFINITY),
            (Key::Char('g'), false) => proof.g = true,
            _ => self.notice(
                "the proof is for reading: n N or ]v [v go through the changes, \
                 :changes pdf writes it, Esc leaves",
            ),
        }
        true
    }

    /// To the next mark of the proof, or the one before.
    fn proof_mark(&mut self, forward: bool) {
        let Some(proof) = &mut self.changes.proof else {
            return;
        };
        let count = proof.marks.len();
        if count == 0 {
            return;
        }
        let at = match (proof.at, forward) {
            (None, true) => 0,
            (None, false) => count - 1,
            (Some(at), true) => (at + 1) % count,
            (Some(at), false) => (at + count - 1) % count,
        };
        proof.at = Some(at);
        let offset = proof.marks[at];
        let source = proof.source.clone();
        self.notice(format!("change {} of {count}", at + 1));
        let (Some(document), true) = (
            &self.document,
            self.document_revision == Some(self.revision),
        ) else {
            return;
        };
        let Some(position) =
            zetwerk_typst::locate(document.pages(), &source, offset, self.cursor_page.get())
        else {
            return;
        };
        let Some(page) = self.layout.pages.get(position.page) else {
            return;
        };
        let zoom = self.view.zoom;
        let y = f64::from(page.y) + position.y * zoom;
        self.view.top = (y - f64::from(self.view.height) / 3.0) / zoom;
        self.view.clamp(&self.layout);
    }

    /// The panels of changes: what keys there are for a paragraph set as
    /// it was, and what a commit would take.
    pub(super) fn changes_panels(&mut self, scene: &mut Scene, has: &dyn Fn(TextureId) -> bool) {
        let lines = if let Some((files, message)) = &self.changes.git.commit {
            let top = match &self.changes.git.status {
                Some(Ok(status)) => status.top.clone(),
                _ => PathBuf::new(),
            };
            let branch = match &self.changes.git.status {
                Some(Ok(status)) => status
                    .branch
                    .clone()
                    .unwrap_or_else(|| "no branch".to_owned()),
                _ => String::new(),
            };
            let mut lines = vec![format!("commit to {branch}: \u{201c}{message}\u{201d}")];
            for file in files {
                let real = std::fs::canonicalize(file).unwrap_or_else(|_| file.clone());
                let shown = real.strip_prefix(&top).unwrap_or(&real);
                lines.push(format!("  {}", shown.display()));
            }
            lines.push("y commits these files, and nothing else · n or Esc leaves it".to_owned());
            lines
        } else if self.changes.slip.is_some() {
            vec![format!(
                "as it was ({}): struck went, tinted came   Enter: take the old back   y: yank it   Esc",
                self.changes.since.called()
            )]
        } else {
            return;
        };
        let markup = chrome::text_markup(&lines);
        let margin = super::PANEL_MARGIN * self.scale;
        let bottom = f64::from(self.window.1) - f64::from(self.status_height()) - margin;
        self.panel(scene, has, &markup, |(_, height)| (margin, bottom - height));
    }

    /// `:git`: what git says of the file.
    fn say_git(&mut self) {
        let told = match &self.changes.git.status {
            None if !self.changes.git.on => {
                "git is not asked here ([changes] git = false)".to_owned()
            }
            None => "git has not answered yet".to_owned(),
            Some(Err(why)) => format!("no git here: {why}"),
            Some(Ok(status)) => {
                let branch = status.branch.as_deref().unwrap_or("no branch");
                let state = match (status.file, status.born) {
                    (_, false) => "the branch has no commits yet".to_owned(),
                    (FileState::Same, _) => format!("{} is as in HEAD", self.file),
                    (FileState::Changed, _) => {
                        format!(
                            "{} differs from HEAD (:changes since HEAD shows how)",
                            self.file
                        )
                    }
                    (FileState::Untracked, _) => format!("{} is new to git", self.file),
                    (FileState::Ignored, _) => format!("{} is left out by .gitignore", self.file),
                };
                format!("on {branch}: {state}")
            }
        };
        self.notice(told);
    }

    /// `:git log`: the commits that changed the file, to compare with.
    fn git_log(&mut self) {
        let Some((_, path)) = self.edited_path() else {
            return;
        };
        if !matches!(self.changes.git.status, Some(Ok(_))) {
            return self.alert("no git here: nothing to list");
        }
        let repo = repo_for(&path);
        self.ask_git(move || Answer::Log(git::log(&repo, &path, LOGGED)));
        self.notice("asking git for the commits of this file …");
    }

    /// `:git commit [message]`: which files a commit would take, to ask
    /// about them first.
    fn git_commit(&mut self, message: &str) {
        let unwritten = self.unwritten();
        if !unwritten.is_empty() {
            return self.alert(format!(
                "unsaved changes in {}: :w first (a commit takes the files as they are on disk)",
                unwritten.join(", ")
            ));
        }
        let (Some((_, path)), Some(Ok(status))) = (self.edited_path(), &self.changes.git.status)
        else {
            return self.alert("no git here: nothing to commit to");
        };
        let top = status.top.clone();
        let message = if message.is_empty() {
            format!("Edit {}", self.file)
        } else {
            message.to_owned()
        };
        let files = self.dependencies();
        let repo = repo_for(&path);
        self.ask_git(move || Answer::ToCommit(git::to_commit(&repo, &top, &files), message));
        self.notice("asking git which of the document's files changed …");
    }

    /// Shows what a commit would take, and asks.
    fn ask_commit(&mut self, files: Vec<PathBuf>, message: String) {
        if files.is_empty() {
            return self.notice("nothing to commit: the document's files are as in HEAD");
        }
        let count = files.len();
        self.changes.git.commit = Some((files, message));
        self.notice(format!("commit {count} files? y commits, n leaves it"));
    }

    /// y or n, to whether to commit.
    fn commit_key(&mut self, press: KeyPress) {
        let yes = matches!(
            (press.key, press.ctrl),
            (Key::Char('y'), false) | (Key::Enter, _)
        );
        let no = matches!(
            (press.key, press.ctrl),
            (Key::Char('n' | 'q'), false) | (Key::Escape, _) | (Key::Char('c' | '['), true)
        );
        if no {
            self.changes.git.commit = None;
            return self.notice("not committed");
        }
        if !yes {
            return self.notice("y commits the files listed, n leaves it");
        }
        let Some((files, message)) = self.changes.git.commit.take() else {
            return;
        };
        let unwritten = self.unwritten();
        if !unwritten.is_empty() {
            return self.alert("not committed: there are unsaved changes since");
        }
        let Some((_, path)) = self.edited_path() else {
            return;
        };
        let repo = repo_for(&path);
        self.ask_git(move || Answer::Committed(git::commit(&repo, &files, &message)));
        self.notice("committing …");
    }
}

/// What a paragraph is set over itself as: its text now with what went
/// struck through and what came tinted, behind the rules for that. `None`
/// where its words cannot carry marks.
fn slip_body(new: &Source, old: &str, passage: &Passage) -> Option<String> {
    let edits = marks_of(new, old, passage)?;
    let text = new.text();
    if passage.is_deletion() {
        let was = old.get(passage.old.clone())?;
        let lead = block_mark(was);
        return Some(format!("{PROOF_RULES}#zetwerk-gone[{}]", &was[lead..]));
    }
    let range = passage.new.clone();
    let lead = block_mark(&text[range.clone()]);
    let mut body = String::from(PROOF_RULES);
    let mut at = range.start + lead;
    for (place, put) in &edits {
        if place.start < at {
            return None;
        }
        body.push_str(&text[at..place.start]);
        body.push_str(put);
        at = place.end;
    }
    body.push_str(&text[at..range.end]);
    Some(body)
}

/// Where the text of a page begins on the left: of the page, and of its
/// second column if it has one; and where it ends on the right.
struct Edges {
    page: f64,
    column: Option<f64>,
    right: f64,
}

impl Edges {
    /// As the numbers of the lines find them (see `aids`).
    fn of(lines: &PageLines) -> Self {
        let extents: Vec<(f64, f64)> = (0..lines.len())
            .filter_map(|line| lines.extent(line))
            .collect();
        let page = extents
            .iter()
            .map(|(left, _)| *left)
            .fold(f64::INFINITY, f64::min);
        let page = if page.is_finite() { page } else { 0.0 };
        let widest = extents.iter().map(|(left, _)| *left).fold(page, f64::max);
        let middle = f64::midpoint(page, widest);
        let column = extents
            .iter()
            .map(|(left, _)| *left)
            .filter(|edge| *edge > middle && widest - page > 150.0)
            .reduce(f64::min);
        let right = extents.iter().map(|(_, right)| *right).fold(page, f64::max);
        Self {
            page,
            column,
            right,
        }
    }

    /// The edge line `line` stands by.
    fn of_line(&self, lines: &PageLines, line: usize) -> f64 {
        match (self.column, lines.extent(line)) {
            (Some(column), Some((left, _))) if left >= column => column,
            _ => self.page,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::*;

    fn compared(old: &str, new: &str) -> (Source, Vec<Passage>) {
        let (a, b) = (Source::detached(old), Source::detached(new));
        let passages = diff::passages(old, &diff::paragraphs(&a), new, &diff::paragraphs(&b));
        (b, passages)
    }

    #[test]
    fn a_proof_marks_words_where_it_can_and_bars_where_it_cannot() {
        let old = "= Intro\nThe cat sat.\n\nGone away.\n\nA *bold* claim.\n\nEnd.\n";
        let new = "= Intro\nThe dog sat down.\n\nA *strong* claim.\n\nEnd.\n";
        let (source, passages) = compared(old, new);
        assert_eq!(passages.len(), 3, "{passages:#?}");
        let (proof, marks, marked) = proof_text(&source, old, &passages);
        assert_eq!(
            marked,
            Marked {
                words: 2,
                bars: 1,
                none: 0
            }
        );
        assert_eq!(marks.len(), 3);
        assert!(
            proof.contains("#zetwerk-old[cat sat.] #zetwerk-new[dog sat down.]"),
            "{proof}"
        );
        assert!(proof.contains("#zetwerk-gone[Gone away.]\n\nA "), "{proof}");
        assert!(proof.contains("A #zetwerk-bar()*strong*"), "{proof}");
        // It typesets.
        let world = typst::syntax::parse(&proof);
        assert_eq!(world.errors_and_warnings().0.len(), 0);
    }

    #[test]
    fn nothing_is_marked_in_an_equation_a_string_or_raw_text() {
        let old = "Let $x + y$ be `raw words` here.\n";
        let new = "Let $x + z$ be `raw text` here.\n";
        let (source, passages) = compared(old, new);
        let (proof, _, marked) = proof_text(&source, old, &passages);
        assert_eq!(marked.words, 0, "{proof}");
        assert!(!proof.contains("#zetwerk-new["), "{proof}");
    }

    #[test]
    fn markup_and_markers_are_not_plain() {
        assert!(plain("the quick brown fox, it said."));
        assert!(plain("well-known 3.5 words"));
        for markup in [
            "*bold*", "a $x$", "#emph", "<label>", "- ", "1.", "@ref", "a // b",
        ] {
            assert!(!plain(markup), "{markup}");
        }
        assert_eq!(block_mark("== Head"), 3);
        assert_eq!(block_mark("- item"), 2);
        assert_eq!(block_mark("Plain"), 0);
    }

    #[test]
    fn versions_are_read_from_what_is_typed() {
        assert_eq!(Since::read("saved"), Some(Since::Saved));
        assert_eq!(Since::read("opened"), Some(Since::Opened));
        assert_eq!(Since::read("step 4"), Some(Since::Step(4)));
        assert_eq!(Since::read("step x"), None);
        assert_eq!(Since::read("HEAD~3"), Some(Since::Git("HEAD~3".to_owned())));
        assert_eq!(Since::read(" "), None);
        assert_eq!(
            section_of("= One\ntext\n\n== Two\nmore", 20).as_deref(),
            Some("Two")
        );
        assert_eq!(section_of("no heading", 5), None);
    }
}
