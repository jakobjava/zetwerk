//! A text Typst refuses, said plainly, and the ways out of it.
//!
//! The page is the only surface, so a text Typst refuses to typeset is
//! the worst state there is: the pages stay as they were (or, where the
//! text has not been typeset since it was opened, there are none), and
//! what is wrong may be something the page shows as nothing — a `#set`
//! line, a name behind `#`, a reference to nothing.
//!
//! While the newest text is refused, the status line says that the page
//! is behind it, where (the line, and the words around the place) and
//! what is wrong, said
//! for a writer, and that `:why` tells more. `:why` puts up a panel with
//! the same and the ways out, whose keys are the panel's while it is up:
//! Enter goes there (into the file it is in), `x` takes out of the text
//! what Typst refuses (the code behind `#` it is in, the `@` of a
//! reference, the mark that is not closed), `u` goes back by undoing to
//! the text the pages show, `e` loads the file as it was saved (`:e!`), and Esc puts
//! the panel away; any other key puts it away too and does what it
//! always does. Undo brings back what any of them changed.
//!
//! Where there are no pages to be behind, the panel comes up by itself,
//! over the source the window shows then; so too after `x`, `u` or `e`
//! where the text they lead to is refused as well. Nothing of this is in
//! the text.

use std::fmt::Write;
use std::ops::Range;
use std::sync::Arc;

use typst::syntax::{FileId, LinkedNode, Source, SyntaxKind};
use zetwerk_core::StateId;
use zetwerk_typst::ProjectWorld;

use super::{ERROR_MARK, PANEL_MARGIN, Scene, TextureId, Viewer, solid};
use crate::editor;
use crate::keys::{Key, KeyPress};

/// How many characters of what is taken out are shown, and of the
/// words before and from the place Typst refuses.
const BEGINS: usize = 48;
const BEFORE: usize = 16;
const FROM: usize = 28;
/// How many steps back undo is searched for the text the pages show.
const UNDO_STEPS: usize = 200;

/// What is kept about refused texts between keys and compiles.
#[derive(Default)]
pub(super) struct Refused {
    /// Whether the panel is up.
    up: bool,
    /// Whether it is to come up by itself if the next compile is
    /// refused: a way out was taken.
    again: bool,
    /// Whether the user put it away while there were no pages: it does
    /// not come up by itself then until there are.
    away: bool,
    /// What the last compile that was not refused typeset: what `u`
    /// goes back to.
    good: Option<Arc<ProjectWorld>>,
    /// Of a revision of the text, where undoing leads to that text.
    back: Option<(u64, Option<(StateId, usize)>)>,
    /// What the last compile was made of: the text of the other files
    /// it complains about.
    last: Option<Arc<ProjectWorld>>,
}

/// The first thing Typst refuses in the newest text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Refusal {
    file: FileId,
    /// Its bytes in the text of that file; `None` where Typst says of no
    /// place (it crashed, say).
    range: Option<Range<usize>>,
    /// The name of the file, where it is not the one being edited.
    elsewhere: Option<String>,
    /// The number of its line, from 1, and the words around it there.
    line: usize,
    begins: String,
    /// What is wrong, in the words of a writer.
    said: String,
    /// How many other errors there are.
    more: usize,
}

impl Refusal {
    /// Where it is: the file if it is another, the line and the words
    /// around it.
    fn place(&self) -> String {
        let file = self
            .elsewhere
            .as_ref()
            .map_or_else(String::new, |name| format!("{name}, "));
        format!("{file}line {} “{}”", self.line, self.begins)
    }

    /// Where it is and what is wrong, in one.
    fn told(&self) -> String {
        if self.range.is_some() {
            format!("{}: {}", self.place(), self.said)
        } else {
            self.said.clone()
        }
    }
}

/// `text` cut to `most` characters, with an ellipsis where it was cut.
fn shortened(text: &str, most: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= most {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(most.saturating_sub(1)).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    cut
}

/// Markup for the panel: `title` in the colour of alerts, then `lines`;
/// in the interface's font and colours, a little larger than other
/// panels, as nothing is more in the way than this.
fn panel_markup(title: &str, lines: &[String]) -> String {
    let literal = |text: &str| {
        let mut written = String::from("\"");
        for c in text.chars() {
            match c {
                '\\' | '"' => {
                    written.push('\\');
                    written.push(c);
                }
                '\n' | '\r' | '\t' => written.push(' '),
                c => written.push(c),
            }
        }
        written.push('"');
        written
    };
    let mut stacked = format!(
        "  text(weight: \"bold\", fill: rgb(\"#fb4934\"), {}),\n",
        literal(title)
    );
    for line in lines {
        let _ = writeln!(stacked, "  {} + h(0pt),", literal(line));
    }
    format!(
        r##"#set page(width: auto, height: auto, margin: 10pt, fill: rgb("#282828"))
#set text(font: "DejaVu Sans Mono", size: 11pt, fill: rgb("#ebdbb2"), ligatures: false, hyphenate: false)
#stack(
  spacing: 7pt,
{stacked})"##
    )
}

/// The words of `line` around byte `at` of it: a few before, more
/// from there on, cut at blanks where they are cut.
fn excerpt(line: &str, at: usize) -> String {
    let (before, from) = line.split_at(at.min(line.len()));
    let before = before.trim_start();
    let mut shown = String::new();
    if before.chars().count() > BEFORE {
        let skip = before.chars().count() - BEFORE;
        let tail: String = before.chars().skip(skip).collect();
        let tail = tail.split_once(' ').map_or(tail.as_str(), |(_, rest)| rest);
        shown.push('…');
        shown.push_str(tail);
    } else {
        shown.push_str(before);
    }
    if from.chars().count() > FROM {
        let head: String = from.chars().take(FROM).collect();
        let head = head
            .rsplit_once(' ')
            .map_or(head.as_str(), |(head, _)| head);
        shown.push_str(head);
        shown.push('…');
    } else {
        shown.push_str(from.trim_end());
    }
    shown
}

/// What Typst says, said for someone who writes and does not program:
/// the complaints a document most often meets, in other words; the
/// rest as Typst says them.
pub(super) fn for_writer(message: &str) -> String {
    let between = |before: &str, after: &str| {
        message
            .strip_prefix(before)
            .and_then(|rest| rest.strip_suffix(after))
            .map(str::to_owned)
    };
    if let Some(name) = message.strip_prefix("unknown variable: ") {
        return format!("Typst knows nothing called “{name}”");
    }
    if let Some(label) = between("label `<", ">` does not exist in the document") {
        return format!("nothing is labelled “{label}”: the reference leads nowhere");
    }
    if let Some(key) = between("key `", "` does not exist in the bibliography") {
        return format!("there is no source “{key}” in the bibliography");
    }
    if message.starts_with("cannot reference heading without numbering") {
        return "a heading is referred to, but headings are not numbered (gD numbers them)"
            .to_owned();
    }
    if let Some(rest) = message.strip_prefix("file not found (searched at ") {
        let path = rest.trim_end_matches(')');
        let name = std::path::Path::new(path).file_name().map_or_else(
            || path.to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        return format!("there is no file “{name}”");
    }
    if message.starts_with("unclosed string") {
        return "a quotation mark is opened here and never closed".to_owned();
    }
    if message.starts_with("unclosed") {
        return "something is opened here and never closed".to_owned();
    }
    if message.starts_with("condition is always true") {
        return "this loop would never end".to_owned();
    }
    if message.starts_with("the document does not contain a bibliography") {
        return "something is cited, but the document has no bibliography".to_owned();
    }
    if message.starts_with("unexpected closing") {
        return "something is closed here that was never opened".to_owned();
    }
    if message.contains("package") && message.contains("not found") {
        return format!("{message} (packages are not downloaded)");
    }
    if message.starts_with("loop seems to be infinite") {
        return "a loop here never ends".to_owned();
    }
    if message.starts_with("maximum") && message.contains("depth exceeded") {
        return "something here goes on in itself without end".to_owned();
    }
    message.to_owned()
}

/// The deepest node of `source` that holds byte `at`.
fn node_at(source: &Source, at: usize) -> LinkedNode<'_> {
    let mut node = LinkedNode::new(source.root());
    loop {
        let Some(child) = node
            .children()
            .find(|child| child.range().contains(&at) && child.kind() != SyntaxKind::Space)
        else {
            return node;
        };
        node = child;
    }
}

/// What `x` does to the text: what it replaces and with what. Of the
/// thing Typst refuses at `range` (saying `message`): a bracket opened
/// in code and never closed is closed where its paragraph ends; else
/// the thing is taken out — a mark opened and not closed or closed and
/// not opened (what it holds is the user's text and stays), the `@` of
/// a reference (what it names stays, as text: it may be the user's word
/// that a reference begun and left took), or the code behind `#` it is
/// in, the innermost. Where that is all that is on its lines, the lines
/// go with it.
pub(super) fn offending(
    source: &Source,
    range: &Range<usize>,
    message: &str,
) -> Option<(Range<usize>, &'static str)> {
    let text = source.text();
    let marked = text.get(range.clone())?;
    if message.starts_with("unclosed") {
        let closer = match marked {
            "(" => Some(")"),
            "[" => Some("]"),
            "{" => Some("}"),
            "\"" => Some("\""),
            _ => None,
        };
        if let Some(closer) = closer {
            // At the end of the paragraph, or of the line for a string.
            let ends = if closer == "\"" { "\n" } else { "\n\n" };
            let end = text[range.end..]
                .find(ends)
                .map_or(text.len(), |at| range.end + at);
            let at = range.end + text[range.end..end].trim_end().len();
            return Some((at..at, closer));
        }
        // The mark alone; of raw text, all its backticks.
        let first = marked.chars().next()?;
        let length = if first == '`' {
            marked.len() - marked.trim_start_matches('`').len()
        } else if marked.len() <= 3 {
            marked.len()
        } else if matches!(first, '*' | '_' | '$') {
            first.len_utf8()
        } else {
            return None;
        };
        return Some((range.start..range.start + length, ""));
    }
    if message.starts_with("unexpected closing") && !marked.is_empty() && marked.len() <= 3 {
        return Some((range.clone(), ""));
    }
    let mut node = Some(node_at(source, range.start));
    let mut found = None;
    while let Some(at) = node {
        if at.kind() == SyntaxKind::Ref {
            found = Some(at.offset()..at.offset() + 1);
            break;
        }
        if let Some(hash) = at.prev_sibling_with_trivia()
            && hash.kind() == SyntaxKind::Hash
            && hash.range().end == at.offset()
        {
            found = Some(hash.offset()..at.range().end);
            break;
        }
        node = at.parent().cloned();
    }
    let thing = found?;
    // All that is on its lines: the lines go.
    let start = text[..thing.start].rfind('\n').map_or(0, |at| at + 1);
    let end = text[thing.end..]
        .find('\n')
        .map_or(text.len(), |at| thing.end + at + 1);
    let alone =
        text[start..thing.start].trim().is_empty() && text[thing.end..end].trim().is_empty();
    Some((if alone { start..end } else { thing }, ""))
}

impl Viewer {
    /// The first thing Typst refuses in the newest text, if it refused
    /// that text: of the file being edited if there is one there, else
    /// of another.
    pub(super) fn refusal(&self) -> Option<Refusal> {
        let editor = self.editor.as_ref()?;
        // Whatever the pages show: a part typeset alone may show the
        // newest text where the whole of it is refused.
        let refused = self.compiled_revision == Some(self.revision) && !self.problems.is_empty();
        if !refused {
            return None;
        }
        let file = editor.file();
        let errors = || self.diagnostics.iter().filter(|(_, _, error, _)| *error);
        let Some((of, range, _, message)) = errors()
            .find(|(of, ..)| *of == file)
            .or_else(|| errors().next())
        else {
            // Of no place: as Typst says it.
            let first = self.problems.first()?;
            let said = first
                .split_once("error: ")
                .map_or(first.as_str(), |(_, said)| said);
            return Some(Refusal {
                file,
                range: None,
                elsewhere: None,
                line: 0,
                begins: String::new(),
                said: for_writer(said),
                more: self.problems.len() - 1,
            });
        };
        let source = if *of == file {
            editor.source().clone()
        } else {
            self.refused.last.as_ref()?.written(*of).ok()?
        };
        let text = source.text();
        let at = range.start.min(text.len());
        let line = source.lines().byte_to_line(at).unwrap_or(0);
        let start = text[..at].rfind('\n').map_or(0, |at| at + 1);
        let end = text[at..].find('\n').map_or(text.len(), |end| at + end);
        let elsewhere = (*of != file).then(|| {
            self.project.path_of(*of).map_or_else(
                || "another file".to_owned(),
                |path| {
                    let root = self.project.root_dir();
                    let shown = path.strip_prefix(root).unwrap_or(&path);
                    shown.to_string_lossy().into_owned()
                },
            )
        });
        Some(Refusal {
            file: *of,
            range: Some(range.clone()),
            elsewhere,
            line: line + 1,
            begins: excerpt(&text[start..end], at - start),
            said: for_writer(message),
            more: errors().count() - 1,
        })
    }

    /// What the status line says while the newest text is refused.
    pub(super) fn refused_status(&self) -> Option<String> {
        let refusal = self.refusal()?;
        let state = match (&self.document, self.document_revision) {
            (None, _) => "no page yet",
            // A part typeset alone shows what was typed.
            (Some(_), Some(shown)) if shown == self.revision => "the text does not typeset whole",
            (Some(_), _) => "the page is behind the text",
        };
        let more = match refusal.more {
            0 => String::new(),
            more => format!(" (+{more} more)"),
        };
        Some(format!("{state}: {}{more}  · :why", refusal.told()))
    }

    /// A compile landed, made of `world`: what `u` leads back to, and
    /// whether the panel comes up or goes.
    pub(super) fn refused_compiled(&mut self, world: &Arc<ProjectWorld>) {
        self.refused.last = Some(Arc::clone(world));
        if self.problems.is_empty() {
            self.refused.good = Some(Arc::clone(world));
            self.refused.back = None;
        }
        if self.document.is_some() {
            self.refused.away = false;
        }
        let Some(editor) = &self.editor else {
            return;
        };
        // A compile of the same text is wanted already (an equation is
        // to be shown as written): that one says.
        if self.refusal().is_none() || self.edited.is_some() {
            self.refused.up = false;
            return;
        }
        let at_rest = editor.mode() == editor::Mode::Normal && editor.is_idle();
        let unasked = self.document.is_none() && !self.refused.away;
        if at_rest && (self.refused.again || unasked) {
            self.refused.up = true;
        }
        self.refused.again = false;
    }

    /// `:why`: the panel, with what Typst refuses and the ways out.
    pub(super) fn refused_command(&mut self, command: &str) -> bool {
        if command != "why" {
            return false;
        }
        if self.refusal().is_some() {
            self.refused.up = true;
        } else if self.compiled_revision != Some(self.revision) {
            self.notice("the text is being typeset");
        } else {
            self.notice("Typst refuses nothing: the page shows the text as it is");
        }
        true
    }

    /// A key while the panel is up. Returns whether it was the panel's.
    pub(super) fn refused_key(&mut self, press: KeyPress) -> bool {
        if !std::mem::take(&mut self.refused.up) || self.other_panel() {
            return false;
        }
        let Some(refusal) = self.refusal() else {
            return false;
        };
        if self.document.is_none() {
            self.refused.away = true;
        }
        if press.ctrl || press.alt {
            return false;
        }
        match press.key {
            Key::Escape => {}
            Key::Enter => self.go_to_refused(&refusal),
            Key::Char('x') => self.take_out_refused(&refusal),
            Key::Char('u') if self.good_state().is_some() => self.back_to_good(),
            Key::Char('e')
                if self
                    .editor
                    .as_ref()
                    .is_some_and(editor::Editor::is_modified) =>
            {
                self.refused.again = true;
                self.reload();
            }
            _ => return false,
        }
        true
    }

    /// Whether a list that keys are for is up: it came after the panel,
    /// and the panel goes for it.
    fn other_panel(&self) -> bool {
        self.picker.is_some() || self.completion.is_some() || self.history.is_some()
    }

    /// Enter: the cursor to what is refused, in the file it is in.
    fn go_to_refused(&mut self, refusal: &Refusal) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let Some(range) = &refusal.range else {
            self.alert(refusal.said.clone());
            return;
        };
        if editor.file() == refusal.file {
            editor.set_cursor(range.start);
            self.follow_cursor();
        } else {
            self.open_file(refusal.file, Some(range.start));
        }
        self.alert(refusal.told());
    }

    /// What `x` would take out, in the text being edited.
    pub(super) fn refused_thing(&self, refusal: &Refusal) -> Option<(Range<usize>, &'static str)> {
        let editor = self.editor.as_ref()?;
        let at = refusal.range.as_ref()?;
        if editor.file() != refusal.file {
            return None;
        }
        let message = self
            .diagnostics
            .iter()
            .find(|(of, range, error, _)| *error && *of == refusal.file && range == at)
            .map_or("", |(.., message)| message.as_str());
        offending(editor.source(), at, message)
    }

    /// `x`: takes what is refused out of the text.
    fn take_out_refused(&mut self, refusal: &Refusal) {
        let Some((range, with)) = self.refused_thing(refusal) else {
            self.notice("that is nothing to take out by itself: Enter goes there");
            return;
        };
        let Some(editor) = &mut self.editor else {
            return;
        };
        let taken = shortened(&editor.text()[range.clone()], BEGINS);
        match editor.replace(&[(range.clone(), with)]) {
            Ok(()) => {
                editor.set_cursor(range.start.min(editor.text().len()));
                self.text_changed(super::Edited::Command);
                self.refused.again = true;
                self.notice(if with.is_empty() {
                    format!("“{taken}” taken out  (u brings it back)")
                } else {
                    format!("closed with “{with}”  (u takes it out)")
                });
            }
            Err(why) => self.alert(format!("not changed: {why}")),
        }
    }

    /// The step of the undo history whose text is the one the pages
    /// show, if undoing leads there and the text is not that already.
    /// And how many undos away it is. Worked out once for each
    /// revision of the text: the panel asks on every frame.
    fn good_state(&mut self) -> Option<(StateId, usize)> {
        let revision = self.revision;
        if let Some((of, found)) = self.refused.back
            && of == revision
        {
            return found;
        }
        let found = self.find_good_state();
        self.refused.back = Some((revision, found));
        found
    }

    /// See [`Self::good_state`]: the steps of undo are taken one by one,
    /// on a copy of the text.
    fn find_good_state(&self) -> Option<(StateId, usize)> {
        let editor = self.editor.as_ref()?;
        let good = self.refused.good.as_ref()?.written(editor.file()).ok()?;
        if good.text() == editor.text() {
            return None;
        }
        let history = editor.history();
        let mut text = editor.text().to_owned();
        let mut state = history.current();
        for steps in 1..=UNDO_STEPS {
            let changes = history.changes(state)?;
            state = history.parent(state)?;
            // Undone, last to first: each `new` becomes its `old` again.
            for change in changes.iter().rev() {
                let end = change.at + change.new.len();
                if text.get(change.at..end) != Some(change.new.as_str()) {
                    return None;
                }
                text.replace_range(change.at..end, &change.old);
            }
            if text == good.text() {
                return Some((state, steps));
            }
        }
        None
    }

    /// `u` in the panel: undoes back to the text the pages show.
    fn back_to_good(&mut self) {
        let Some((state, steps)) = self.good_state() else {
            return;
        };
        let Some(editor) = &mut self.editor else {
            return;
        };
        match editor.travel(state) {
            Ok(()) => {
                self.text_changed(super::Edited::Command);
                self.stepped();
                self.refused.again = true;
                let undos = if steps == 1 { "undo" } else { "undos" };
                self.notice(format!(
                    "back to the text the page shows, {steps} {undos}  (Ctrl-R redoes them)"
                ));
            }
            Err(why) => self.alert(format!("not changed: {why}")),
        }
    }

    /// The panel, while it is up: what is refused and the ways out.
    pub(super) fn refused_panel(&mut self, scene: &mut Scene, has: &dyn Fn(TextureId) -> bool) {
        if !self.refused.up {
            return;
        }
        if self.other_panel() {
            self.refused.up = false;
            return;
        }
        let Some(refusal) = self.refusal() else {
            self.refused.up = false;
            return;
        };
        let title = match (&self.document, self.document_revision) {
            (None, _) => "No page yet: Typst cannot typeset this text.",
            (Some(_), Some(shown)) if shown == self.revision => {
                "The page shows what was typed, but the whole text does not typeset."
            }
            (Some(_), _) => "The page is behind the text: Typst cannot typeset it as it is.",
        };
        let mut lines = Vec::new();
        if refusal.range.is_some() {
            lines.push(refusal.place());
        }
        lines.push(format!("{}.", refusal.said));
        match refusal.more {
            0 => {}
            1 => lines.push("And one thing more (]d goes to each).".to_owned()),
            more => lines.push(format!("And {more} things more (]d goes to each).")),
        }
        lines.push(String::new());
        if refusal.range.is_some() {
            lines.push("Enter  go there".to_owned());
        }
        if let Some((_, with)) = self.refused_thing(&refusal)
            && !with.is_empty()
        {
            lines.push(format!(
                "x      close it with “{with}” where its paragraph ends"
            ));
        } else if let Some((range, _)) = self.refused_thing(&refusal)
            && let Some(editor) = &self.editor
        {
            let taken = &editor.text()[range.clone()];
            let count = taken.trim_end().lines().count();
            let lines_of = if count > 1 {
                format!(" ({count} lines)")
            } else if !taken.contains(char::is_alphanumeric) {
                " (the rest stays)".to_owned()
            } else {
                String::new()
            };
            lines.push(format!(
                "x      take out “{}”{lines_of}",
                shortened(taken, BEGINS)
            ));
        }
        if let Some((_, steps)) = self.good_state() {
            let undos = if steps == 1 { "undo" } else { "undos" };
            lines.push(format!(
                "u      back to the text the page shows ({steps} {undos})"
            ));
        }
        if self
            .editor
            .as_ref()
            .is_some_and(editor::Editor::is_modified)
        {
            lines.push("e      the file as it was last saved (:e!)".to_owned());
        }
        lines.push("Esc    put this away".to_owned());
        let markup = panel_markup(title, &lines);
        let Some(image) = self.chrome.render(&markup, self.scale) else {
            return;
        };
        let (width, height) = (
            f64::from(image.pixmap.width()),
            f64::from(image.pixmap.height()),
        );
        let margin = (PANEL_MARGIN * self.scale).round();
        let (x, y) = ((f64::from(self.window.0) - width - margin).round(), margin);
        let line = (2.0 * self.scale).round().max(1.0);
        solid(
            scene,
            has,
            ERROR_MARK,
            (x - line, y - line, width + 2.0 * line, height + 2.0 * line),
        );
        scene.quads.push(super::Quad {
            texture: image.id,
            x: x as f32,
            y: y as f32,
            width: width as f32,
            height: height as f32,
        });
        if !has(image.id) {
            scene.images.push(image);
        }
    }

    /// Whether the panel is up.
    #[cfg(test)]
    pub(super) fn refused_up(&self) -> bool {
        self.refused.up
    }
}
