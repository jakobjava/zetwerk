//! What a document does across sessions: saving, recovery, conflicts.

use std::path::{Path, PathBuf};

use zetwerk_core::{DiskError, DiskStatus, Document, OpenError, Opened, Replace};

/// A scratch directory holding a `work` directory for documents and a
/// `state` directory for journals.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("zetwerk-doc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("work")).unwrap_or_else(|err| panic!("{err}"));
        Self { root }
    }

    fn file(&self) -> PathBuf {
        self.root.join("work").join("paper.typ")
    }

    fn state(&self) -> PathBuf {
        self.root.join("state")
    }

    fn write(&self, text: &str) {
        std::fs::write(self.file(), text).unwrap_or_else(|err| panic!("{err}"));
    }

    fn read(&self) -> String {
        std::fs::read_to_string(self.file()).unwrap_or_else(|err| panic!("{err}"))
    }

    fn open(&self) -> (Document<String>, Opened) {
        Document::open(&self.file(), &self.state(), |text| text)
            .unwrap_or_else(|err| panic!("{err}"))
    }

    /// The journal files (not locks) currently in the state directory.
    fn journals(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(self.state().join("journals"))
            .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default();
        files.retain(|path| path.extension().is_some_and(|ext| ext != "lock"));
        files.sort();
        files
    }

    fn journal(&self) -> PathBuf {
        self.journals()
            .into_iter()
            .find(|path| path.extension().is_some_and(|ext| ext == "journal"))
            .unwrap_or_else(|| panic!("a journal exists"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn insert(document: &mut Document<String>, at: usize, text: &str) {
    document
        .edit(&[Replace::insert(at, text)], false)
        .unwrap_or_else(|err| panic!("{err}"));
}

#[test]
fn edit_undo_redo_and_save() {
    let scratch = Scratch::new("basics");
    scratch.write("Hello");
    let (mut document, opened) = scratch.open();
    assert_eq!(opened, Opened::Clean);
    assert!(!document.is_modified());

    insert(&mut document, 5, ", world");
    assert_eq!(document.text(), "Hello, world");
    assert!(document.is_modified());
    assert_eq!(scratch.read(), "Hello", "nothing is written without saving");

    assert!(document.undo().is_ok_and(|state| state.is_some()));
    assert_eq!(document.text(), "Hello");
    assert!(!document.is_modified(), "back at the saved state");
    assert!(document.undo().is_ok_and(|state| state.is_none()));
    assert!(document.redo().is_ok_and(|state| state.is_some()));
    assert!(document.redo().is_ok_and(|state| state.is_none()));

    document.save(false).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(scratch.read(), "Hello, world");
    assert!(!document.is_modified());
    assert_eq!(document.revision(), 3);
}

#[test]
fn joined_edits_are_one_undo_step() {
    let scratch = Scratch::new("join");
    scratch.write("");
    let (mut document, _) = scratch.open();
    for (at, letter) in ["w", "o", "r", "d"].iter().enumerate() {
        document
            .edit(&[Replace::insert(at, *letter)], true)
            .unwrap_or_else(|err| panic!("{err}"));
    }
    assert_eq!(document.text(), "word");
    assert_eq!(document.history().len(), 2, "the root and one step");
    assert!(document.undo().is_ok());
    assert_eq!(document.text(), "");

    // Joining never reaches into the saved state.
    assert!(document.redo().is_ok());
    document.save(false).unwrap_or_else(|err| panic!("{err}"));
    document
        .edit(&[Replace::insert(4, "s")], true)
        .unwrap_or_else(|err| panic!("{err}"));
    assert!(document.undo().is_ok());
    assert_eq!(document.text(), "word");
    assert!(!document.is_modified());
}

#[test]
fn an_invalid_edit_changes_nothing() {
    let scratch = Scratch::new("invalid");
    scratch.write("abc");
    let (mut document, _) = scratch.open();
    let result = document.edit(&[Replace::insert(1, "x"), Replace::delete(9..12)], false);
    assert!(result.is_err());
    assert_eq!((document.text(), document.revision()), ("abc", 0));
    assert_eq!(document.history().len(), 1);
}

#[test]
fn unsaved_changes_come_back_with_their_history() {
    let scratch = Scratch::new("restore");
    scratch.write("one");
    {
        let (mut document, _) = scratch.open();
        insert(&mut document, 3, " two");
        insert(&mut document, 7, " three");
        assert!(document.undo().is_ok());
        insert(&mut document, 7, " drei");
        // The session ends here without saving: a crash.
    }
    assert_eq!(scratch.read(), "one");
    let (mut document, opened) = scratch.open();
    assert_eq!(opened, Opened::Restored);
    assert_eq!(document.text(), "one two drei");
    assert!(document.is_modified());
    assert_eq!(document.history().len(), 4, "both branches survive");

    // Undo leads all the way back to what is in the file.
    assert!(document.undo().is_ok() && document.undo().is_ok());
    assert_eq!(document.text(), "one");
    assert!(!document.is_modified());
    // The abandoned branch is still there.
    assert!(document.goto(2).is_ok());
    assert_eq!(document.text(), "one two three");
}

#[test]
fn a_saved_session_reopens_clean_and_keeps_undo() {
    let scratch = Scratch::new("persistent-undo");
    scratch.write("draft");
    {
        let (mut document, _) = scratch.open();
        insert(&mut document, 5, " v2");
        document.save(false).unwrap_or_else(|err| panic!("{err}"));
    }
    let (mut document, opened) = scratch.open();
    assert_eq!(opened, Opened::Clean);
    assert!(!document.is_modified());
    assert!(document.undo().is_ok_and(|state| state.is_some()));
    assert_eq!(document.text(), "draft");
    assert!(document.is_modified());
}

#[test]
fn a_crash_between_writing_the_file_and_noting_it_is_not_a_conflict() {
    let scratch = Scratch::new("save-gap");
    scratch.write("a");
    {
        let (mut document, _) = scratch.open();
        insert(&mut document, 1, "b");
        insert(&mut document, 2, "c");
        assert!(document.undo().is_ok());
        // The file now gets the text of a state the journal knows, but the
        // journal never hears of it.
        scratch.write("ab");
        assert!(document.redo().is_ok());
    }
    let (document, opened) = scratch.open();
    assert_eq!(opened, Opened::Restored);
    assert_eq!(document.text(), "abc");
    assert!(document.is_modified());
}

#[test]
fn a_file_changed_while_closed_wins_when_nothing_was_unsaved() {
    let scratch = Scratch::new("external");
    scratch.write("mine");
    {
        let (mut document, _) = scratch.open();
        insert(&mut document, 4, "!");
        document.save(false).unwrap_or_else(|err| panic!("{err}"));
    }
    scratch.write("theirs");
    let (mut document, opened) = scratch.open();
    assert_eq!(opened, Opened::ChangedWhileClosed);
    assert_eq!(document.text(), "theirs");
    assert!(!document.is_modified());
    assert_eq!(scratch.read(), "theirs", "the file is never touched");
    // What zetwerk had is not dropped: undo brings it back.
    assert_eq!(document.history().len(), 3);
    assert!(document.undo().is_ok_and(|state| state.is_some()));
    assert_eq!(document.text(), "mine!");
    assert!(document.undo().is_ok_and(|state| state.is_some()));
    assert_eq!(document.text(), "mine");
}

#[test]
fn a_file_emptied_while_closed_keeps_the_text_in_reach() {
    // `zetwerk paper.typ` makes an empty file where there is none (git
    // checkout, an agent): the journal's text must survive that.
    let scratch = Scratch::new("emptied");
    scratch.write("precious");
    drop(scratch.open());
    scratch.write("");
    let (document, opened) = scratch.open();
    assert_eq!(opened, Opened::ChangedWhileClosed);
    assert_eq!(document.text(), "");
    drop(document);
    // Not even a second opening loses it.
    let (mut again, opened) = scratch.open();
    assert_eq!(opened, Opened::Clean);
    assert!(again.undo().is_ok_and(|state| state.is_some()));
    assert_eq!(again.text(), "precious");
    drop(again);
    let (document, opened) = scratch.open();
    assert_eq!(opened, Opened::Restored);
    assert_eq!(document.text(), "precious");
}

#[test]
fn a_conflict_keeps_both_versions() {
    let scratch = Scratch::new("conflict");
    scratch.write("base");
    {
        let (mut document, _) = scratch.open();
        insert(&mut document, 4, " + my unsaved work");
    }
    scratch.write("base + their change");
    let (mut document, opened) = scratch.open();
    let Opened::Conflict { recovered, journal } = opened else {
        panic!("expected a conflict, got {opened:?}")
    };
    assert_eq!(recovered, "base + my unsaved work");
    assert_eq!(document.text(), "base + their change");
    assert!(!document.is_modified());
    // The unsaved version is one undo away, not only in a journal that
    // nobody can read.
    assert!(document.undo().is_ok_and(|state| state.is_some()));
    assert_eq!(document.text(), "base + my unsaved work");
    assert!(document.redo().is_ok_and(|state| state.is_some()));
    assert!(
        journal.exists(),
        "the old journal is kept at {}",
        journal.display()
    );
    assert_eq!(
        scratch.read(),
        "base + their change",
        "the file is never touched"
    );
    drop(document);

    // Opening again is calm: the kept journal is not picked up again.
    let (_, opened) = scratch.open();
    assert_eq!(opened, Opened::Clean);
    assert!(journal.exists());
}

#[test]
fn a_deleted_file_does_not_take_its_text_with_it() {
    let scratch = Scratch::new("deleted");
    scratch.write("precious");
    {
        let (mut document, _) = scratch.open();
        insert(&mut document, 8, " words");
    }
    std::fs::remove_file(scratch.file()).unwrap_or_else(|err| panic!("{err}"));
    let (mut document, opened) = scratch.open();
    assert_eq!(opened, Opened::Restored);
    assert_eq!(document.text(), "precious words");
    assert!(document.is_modified());
    document.save(false).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(scratch.read(), "precious words");
}

#[test]
fn a_new_file_starts_empty_and_is_created_on_save() {
    let scratch = Scratch::new("new");
    let (mut document, opened) = scratch.open();
    assert_eq!(opened, Opened::Clean);
    assert_eq!(document.text(), "");
    assert!(!scratch.file().exists());
    insert(&mut document, 0, "first words");
    document.save(false).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(scratch.read(), "first words");
}

#[test]
fn a_garbage_journal_is_set_aside_not_trusted_and_not_deleted() {
    let scratch = Scratch::new("garbage");
    scratch.write("text");
    drop(scratch.open());
    let journal = scratch.journal();
    std::fs::write(&journal, b"this is not a journal").unwrap_or_else(|err| panic!("{err}"));

    let (document, opened) = scratch.open();
    let Opened::JournalUnusable { journal: kept } = opened else {
        panic!("expected an unusable journal, got {opened:?}")
    };
    assert_eq!(document.text(), "text");
    assert_eq!(
        std::fs::read(&kept).ok(),
        Some(b"this is not a journal".to_vec())
    );
}

#[test]
fn a_journal_torn_mid_record_loses_only_that_record() {
    let scratch = Scratch::new("torn");
    scratch.write("");
    {
        let (mut document, _) = scratch.open();
        insert(&mut document, 0, "first ");
        insert(&mut document, 6, "second");
    }
    let journal = scratch.journal();
    let bytes = std::fs::read(&journal).unwrap_or_else(|err| panic!("{err}"));
    std::fs::write(&journal, &bytes[..bytes.len() - 3]).unwrap_or_else(|err| panic!("{err}"));

    let (document, opened) = scratch.open();
    assert_eq!(opened, Opened::Restored);
    assert_eq!(document.text(), "first ");
}

#[test]
fn saving_refuses_to_overwrite_someone_elses_change() {
    let scratch = Scratch::new("save-conflict");
    scratch.write("original");
    let (mut document, _) = scratch.open();
    insert(&mut document, 8, " mine");
    assert_eq!(document.disk_status().ok(), Some(DiskStatus::Unchanged));

    scratch.write("original theirs");
    assert_eq!(document.disk_status().ok(), Some(DiskStatus::Changed));
    assert!(matches!(
        document.save(false),
        Err(DiskError::ChangedOnDisk)
    ));
    assert_eq!(scratch.read(), "original theirs");
    assert!(document.is_modified());

    document.save(true).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(scratch.read(), "original mine");
    assert_eq!(document.disk_status().ok(), Some(DiskStatus::Unchanged));
}

#[test]
fn saving_recreates_a_file_that_vanished() {
    let scratch = Scratch::new("vanished");
    scratch.write("here");
    let (mut document, _) = scratch.open();
    std::fs::remove_file(scratch.file()).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(document.disk_status().ok(), Some(DiskStatus::Missing));
    document.save(false).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(scratch.read(), "here");
}

#[test]
fn reloading_is_an_undoable_step() {
    let scratch = Scratch::new("reload");
    scratch.write("alpha beta gamma");
    let (mut document, _) = scratch.open();
    insert(&mut document, 0, "my ");
    scratch.write("alpha BETA gamma");

    document.reload().unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(document.text(), "alpha BETA gamma");
    assert!(!document.is_modified());
    // What was in the buffer is one undo away.
    assert!(document.undo().is_ok());
    assert_eq!(document.text(), "my alpha beta gamma");

    std::fs::remove_file(scratch.file()).unwrap_or_else(|err| panic!("{err}"));
    assert!(matches!(document.reload(), Err(DiskError::Missing)));
    assert_eq!(document.text(), "my alpha beta gamma");
}

#[test]
fn a_byte_order_mark_is_kept_but_is_not_text() {
    let scratch = Scratch::new("bom");
    scratch.write("\u{feff}marked");
    let (mut document, _) = scratch.open();
    assert_eq!(document.text(), "marked");
    insert(&mut document, 6, "!");
    document.save(false).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(scratch.read(), "\u{feff}marked!");
    drop(document);
    let (_, opened) = scratch.open();
    assert_eq!(opened, Opened::Clean);
}

#[test]
fn a_file_that_is_not_text_is_refused() {
    let scratch = Scratch::new("binary");
    std::fs::write(scratch.file(), [0xff, 0xfe, 0x00]).unwrap_or_else(|err| panic!("{err}"));
    let result = Document::open(&scratch.file(), &scratch.state(), |text: String| text);
    assert!(matches!(result, Err(OpenError::NotUtf8(_))));
}

#[test]
fn one_file_cannot_be_open_twice() {
    let scratch = Scratch::new("lock");
    scratch.write("shared");
    let (first, _) = scratch.open();
    let second = Document::open(&scratch.file(), &scratch.state(), |text: String| text);
    assert!(matches!(second, Err(OpenError::AlreadyOpen(_))));
    drop(first);
    let _ = scratch.open();
}

#[cfg(unix)]
#[test]
fn a_symbolic_link_is_the_file_it_points_to() {
    let scratch = Scratch::new("symlink");
    scratch.write("target");
    let link = scratch.root.join("work").join("link.typ");
    std::os::unix::fs::symlink(scratch.file(), &link).unwrap_or_else(|err| panic!("{err}"));

    let open_link = || Document::open(&link, &scratch.state(), |text: String| text);
    let (mut document, _) = open_link().unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(
        document.path(),
        scratch.file().canonicalize().unwrap_or_default().as_path()
    );
    // Same file, same lock.
    let direct = Document::open(&scratch.file(), &scratch.state(), |text: String| text);
    assert!(matches!(direct, Err(OpenError::AlreadyOpen(_))));

    insert(&mut document, 6, " changed");
    document.save(false).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(scratch.read(), "target changed");
    let is_link = std::fs::symlink_metadata(&link).is_ok_and(|meta| meta.is_symlink());
    assert!(is_link, "saving through a link must not replace the link");
}

#[test]
fn the_journal_is_compacted_on_open() {
    let scratch = Scratch::new("compact");
    scratch.write("");
    let size = |path: &Path| std::fs::metadata(path).map_or(0, |meta| meta.len());
    {
        let (mut document, _) = scratch.open();
        for index in 0..500 {
            document
                .edit(&[Replace::insert(index, "x")], true)
                .unwrap_or_else(|err| panic!("{err}"));
        }
    }
    let before = size(&scratch.journal());
    let (document, _) = scratch.open();
    let after = size(&scratch.journal());
    assert_eq!(document.text().len(), 500);
    assert_eq!(document.history().len(), 2);
    assert!(
        after * 5 < before,
        "journal went from {before} to {after} bytes"
    );
}

#[test]
fn restored_changes_say_how_old_they_are_until_saved() {
    let scratch = Scratch::new("restored-age");
    scratch.write("one");
    {
        let (mut document, _) = scratch.open();
        assert_eq!(document.restored(), None);
        insert(&mut document, 3, " two");
    }
    let three_days_ago = std::time::SystemTime::now() - std::time::Duration::from_hours(72);
    std::fs::File::options()
        .append(true)
        .open(scratch.journal())
        .and_then(|file| file.set_modified(three_days_ago))
        .unwrap_or_else(|err| panic!("{err}"));
    let (document, opened) = scratch.open();
    assert_eq!(opened, Opened::Restored);
    assert_eq!(document.restored(), Some(three_days_ago));
    drop(document);
    // Opening again, without a change, does not make them younger.
    let (mut document, _) = scratch.open();
    assert_eq!(document.restored(), Some(three_days_ago));
    document.save(false).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(document.restored(), None);
    drop(document);
    let (document, opened) = scratch.open();
    assert_eq!(opened, Opened::Clean);
    assert_eq!(document.restored(), None);
}

#[test]
fn leftovers_of_a_killed_save_are_removed_only_if_provably_ours() {
    let scratch = Scratch::new("leftovers");
    scratch.write("one");
    {
        let (mut document, _) = scratch.open();
        insert(&mut document, 3, " two");
    }
    let work = scratch.root.join("work");
    // What a save of "one two", killed just before its rename, leaves.
    let ours = work.join(".paper.typ.Ab3dE9");
    std::fs::write(&ours, "one two").unwrap_or_else(|err| panic!("{err}"));
    // The same name pattern, but text this document never held.
    let foreign = work.join(".paper.typ.zzzzzz");
    std::fs::write(&foreign, "someone else's notes").unwrap_or_else(|err| panic!("{err}"));
    // Other dotfiles are none of our business whatever they contain.
    let unrelated = work.join(".paper.typ.swp");
    std::fs::write(&unrelated, "one two").unwrap_or_else(|err| panic!("{err}"));

    let (document, opened) = scratch.open();
    assert_eq!(opened, Opened::Restored);
    assert_eq!(document.text(), "one two");
    assert!(!ours.exists(), "our own leftover is cleaned up");
    assert!(
        foreign.exists() && unrelated.exists(),
        "other files are left alone"
    );
}

#[test]
fn unsaved_changes_of_a_closed_file_can_be_asked_about() {
    let scratch = Scratch::new("pending");
    let pending = || zetwerk_core::has_unsaved_changes(&scratch.file(), &scratch.state());
    scratch.write("one");
    assert!(!pending(), "no journal at all");
    {
        let (mut document, _) = scratch.open();
        assert!(!pending());
        insert(&mut document, 3, " two");
        assert!(pending(), "asked while open");
    }
    assert!(pending());
    // A file that has what the journal has holds nothing back.
    scratch.write("one two");
    assert!(!pending());
    scratch.write("other");
    assert!(pending());
    {
        let (mut document, _) = scratch.open();
        document.save(true).unwrap_or_else(|err| panic!("{err}"));
    }
    assert!(!pending());
}

#[test]
fn lines_typed_into_a_file_of_crlf_lines_end_in_crlf() {
    // The text had CR LF where the file had it and a lone LF where a line
    // was typed: the file ended up with line ends of both kinds.
    let scratch = Scratch::new("crlf");
    scratch.write("= Title\r\n\r\nOld words.\r\n");
    let (mut document, _) = scratch.open();
    // The text has line feeds alone, as everything that edits it expects.
    assert_eq!(document.text(), "= Title\n\nOld words.\n");
    let end = document.text().len();
    insert(&mut document, end, "New line.\n");
    document.save(false).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(scratch.read(), "= Title\r\n\r\nOld words.\r\nNew line.\r\n");
    // Opened again it is the same, and nothing is unsaved.
    drop(document);
    let (document, opened) = scratch.open();
    assert_eq!(opened, Opened::Clean);
    assert_eq!(document.text(), "= Title\n\nOld words.\nNew line.\n");
}

#[test]
fn a_file_with_line_ends_of_both_kinds_is_kept_as_it_is() {
    let scratch = Scratch::new("mixed");
    let mixed = "one\r\ntwo\nthree\r\n";
    scratch.write(mixed);
    let (mut document, _) = scratch.open();
    assert_eq!(document.text(), mixed);
    document.save(true).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(scratch.read(), mixed);
    // A carriage return that ends no line is no CR LF file either.
    let odd = "one\r\ntwo\r\r\n";
    scratch.write(odd);
    document.reload().unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(document.text(), odd);
    document.save(true).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(scratch.read(), odd);
}

#[test]
fn a_journal_from_before_crlf_was_taken_apart_still_fits_its_file() {
    // Journals of CR LF files hold the carriage returns in their text, and
    // their hashes are of the file's bytes: they must still match. Such a
    // journal is made here as a file of mixed line ends makes it, whose
    // text is kept as it is.
    let scratch = Scratch::new("crlf-old");
    scratch.write("one\r\ntwo\n");
    {
        let (mut document, _) = scratch.open();
        document
            .edit(&[Replace::new(8..9, "\r\n")], false)
            .unwrap_or_else(|err| panic!("{err}"));
        document.save(false).unwrap_or_else(|err| panic!("{err}"));
        insert(&mut document, 10, "three\r\n");
    }
    assert_eq!(scratch.read(), "one\r\ntwo\r\n");
    // The file is all CR LF now; the unsaved line is still back.
    let (mut document, opened) = scratch.open();
    assert_eq!(opened, Opened::Restored);
    assert_eq!(document.text(), "one\r\ntwo\r\nthree\r\n");
    // What is typed now is written with a carriage return, as the rest.
    let end = document.text().len();
    insert(&mut document, end, "four\n");
    document.save(false).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(scratch.read(), "one\r\ntwo\r\nthree\r\nfour\r\n");
    drop(document);
    let (_, opened) = scratch.open();
    assert_eq!(opened, Opened::Clean);
}

/// Copies of a journal kept aside as `why`.
fn kept_as(scratch: &Scratch, why: &str) -> Vec<PathBuf> {
    scratch
        .journals()
        .into_iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.to_string_lossy().starts_with(why))
        })
        .collect()
}

#[test]
fn a_journal_damaged_before_its_snapshot_still_gives_back_the_text() {
    // Damage early in a journal took everything with it: the history
    // and the unsaved text were set aside, out of reach without tools.
    let scratch = Scratch::new("damaged-early");
    scratch.write("Saved.\n");
    {
        let (mut document, _) = scratch.open();
        for word in ["one ", "two ", "three "] {
            insert(&mut document, 0, word);
        }
    }
    {
        // Opened again: the journal begins with the history, then the
        // snapshot of the text; then the session's own edits.
        let (mut document, opened) = scratch.open();
        assert_eq!(opened, Opened::Restored);
        insert(&mut document, 0, "four ");
    }
    let journal = scratch.journal();
    let mut bytes = std::fs::read(&journal).unwrap_or_else(|err| panic!("{err}"));
    // A bit of the first record, the first step of the history.
    bytes[20] ^= 0x10;
    std::fs::write(&journal, &bytes).unwrap_or_else(|err| panic!("{err}"));

    let (mut document, opened) = scratch.open();
    assert_eq!(opened, Opened::Restored);
    assert_eq!(document.text(), "four three two one Saved.\n");
    let said = document.remarks().join("; ");
    assert!(said.contains("damaged") && said.contains("lost"), "{said}");
    // The journal as it was is kept.
    let kept = kept_as(&scratch, "damaged");
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(std::fs::read(&kept[0]).ok(), Some(bytes));
    // Undo goes back to the text the journal still knows, and from there
    // to the file's.
    assert_eq!(document.undo().ok().flatten(), Some(1));
    assert_eq!(document.text(), "three two one Saved.\n");
    assert_eq!(document.undo().ok().flatten(), Some(0));
    assert_eq!(document.text(), "Saved.\n");
}

#[test]
fn a_journal_damaged_in_the_middle_is_kept_as_it_was() {
    // The records after the damage were lost without a trace: opening
    // wrote the journal anew from what came before.
    let scratch = Scratch::new("damaged-middle");
    scratch.write("");
    {
        let (mut document, _) = scratch.open();
        insert(&mut document, 0, "first ");
        insert(&mut document, 6, "second ");
        insert(&mut document, 13, "third");
    }
    let journal = scratch.journal();
    let mut bytes = std::fs::read(&journal).unwrap_or_else(|err| panic!("{err}"));
    // The last byte of the second edit's record: "second ".
    let at = bytes
        .windows(7)
        .position(|window| window == b"second ")
        .unwrap_or_else(|| panic!("the edit is in the journal"));
    bytes[at + 3] ^= 0x01;
    std::fs::write(&journal, &bytes).unwrap_or_else(|err| panic!("{err}"));

    let (document, opened) = scratch.open();
    assert_eq!(opened, Opened::Restored);
    assert_eq!(document.text(), "first ");
    let said = document.remarks().join("; ");
    assert!(said.contains("damaged"), "{said}");
    let kept = kept_as(&scratch, "damaged");
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(std::fs::read(&kept[0]).ok(), Some(bytes));
    // A write cut short is no damage, and keeps nothing aside.
    drop(document);
    let journal = scratch.journal();
    let bytes = std::fs::read(&journal).unwrap_or_else(|err| panic!("{err}"));
    std::fs::write(&journal, &bytes[..bytes.len() - 2]).unwrap_or_else(|err| panic!("{err}"));
    let (document, _) = scratch.open();
    assert!(document.remarks().is_empty(), "{:?}", document.remarks());
    assert_eq!(kept_as(&scratch, "damaged").len(), 1);
}

/// Opens `path` with the journals of `scratch`.
fn open_at(scratch: &Scratch, path: &Path) -> (Document<String>, Opened) {
    Document::open(path, &scratch.state(), |text| text).unwrap_or_else(|err| panic!("{err}"))
}

#[test]
fn a_moved_file_keeps_its_history() {
    // Documents moved to another folder lost their undo history and their
    // unsaved changes: both stayed behind under the old name.
    let scratch = Scratch::new("moved");
    scratch.write("Text.\n");
    {
        let (mut document, _) = scratch.open();
        insert(&mut document, 0, "Saved ");
        document.save(false).unwrap_or_else(|err| panic!("{err}"));
        insert(&mut document, 0, "Unsaved ");
    }
    let elsewhere = scratch.root.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap_or_else(|err| panic!("{err}"));
    let moved = elsewhere.join("renamed.typ");
    std::fs::rename(scratch.file(), &moved).unwrap_or_else(|err| panic!("{err}"));

    let (mut document, opened) = open_at(&scratch, &moved);
    assert_eq!(opened, Opened::Restored);
    assert_eq!(document.text(), "Unsaved Saved Text.\n");
    let said = document.remarks().join("; ");
    assert!(
        said.contains("history taken over from") && said.contains("paper.typ"),
        "{said}"
    );
    assert!(document.undo().ok().flatten().is_some());
    assert!(document.undo().ok().flatten().is_some());
    assert_eq!(document.text(), "Text.\n");
    drop(document);
    // The old journal is kept, marked: it is not taken over again.
    assert_eq!(kept_as(&scratch, "moved").len(), 1);
    let again = scratch.root.join("again.typ");
    std::fs::copy(&moved, &again).unwrap_or_else(|err| panic!("{err}"));
    let (document, _) = open_at(&scratch, &again);
    assert!(document.remarks().is_empty(), "{:?}", document.remarks());
}

#[test]
fn a_copied_file_does_not_take_the_history_of_its_original() {
    let scratch = Scratch::new("copied");
    scratch.write("Text.\n");
    {
        let (mut document, _) = scratch.open();
        insert(&mut document, 0, "More. ");
        document.save(false).unwrap_or_else(|err| panic!("{err}"));
    }
    let copy = scratch.root.join("copy.typ");
    std::fs::copy(scratch.file(), &copy).unwrap_or_else(|err| panic!("{err}"));
    let (document, opened) = open_at(&scratch, &copy);
    assert_eq!(opened, Opened::Clean);
    assert!(document.history().is_empty());
    assert!(document.remarks().is_empty(), "{:?}", document.remarks());
}

#[test]
fn of_two_histories_that_fit_none_is_taken_over() {
    let scratch = Scratch::new("two-fit");
    let (one, two) = (scratch.root.join("one.typ"), scratch.root.join("two.typ"));
    for path in [&one, &two] {
        std::fs::write(path, "Same.\n").unwrap_or_else(|err| panic!("{err}"));
        let (mut document, _) = open_at(&scratch, path);
        insert(&mut document, 0, "x");
        document.undo().unwrap_or_else(|err| panic!("{err}"));
    }
    std::fs::remove_file(&one).unwrap_or_else(|err| panic!("{err}"));
    std::fs::remove_file(&two).unwrap_or_else(|err| panic!("{err}"));
    scratch.write("Same.\n");
    let (document, _) = scratch.open();
    assert!(document.history().is_empty());
    let said = document.remarks().join("; ");
    assert!(said.contains("none was taken over"), "{said}");
}

#[test]
fn moving_takes_the_journal_along_and_writes_over_nothing() {
    let scratch = Scratch::new("rename");
    scratch.write("Text.\n");
    let (mut document, _) = scratch.open();
    insert(&mut document, 0, "Unsaved ");
    let taken = scratch.root.join("work").join("taken.typ");
    std::fs::write(&taken, "Someone else's.\n").unwrap_or_else(|err| panic!("{err}"));
    let refused = document.rename(&taken);
    assert!(
        matches!(&refused, Err(DiskError::Io(err)) if err.kind() == std::io::ErrorKind::AlreadyExists),
        "{refused:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&taken).ok().as_deref(),
        Some("Someone else's.\n")
    );
    let to = scratch.root.join("work").join("new name.typ");
    assert!(matches!(document.rename(&to), Ok(None)));
    assert!(!scratch.file().exists());
    assert_eq!(
        std::fs::read_to_string(&to).ok().as_deref(),
        Some("Text.\n")
    );
    assert!(document.is_modified());
    drop(document);
    // The history and the unsaved text are the new name's.
    let (mut document, opened) = open_at(&scratch, &to);
    assert_eq!(opened, Opened::Restored);
    assert_eq!(document.text(), "Unsaved Text.\n");
    assert!(document.remarks().is_empty(), "{:?}", document.remarks());
    assert!(document.undo().ok().flatten().is_some());
    // Nothing is left of the old name's journal.
    let journals: Vec<PathBuf> = scratch
        .journals()
        .into_iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "journal"))
        .collect();
    assert_eq!(journals.len(), 1, "{journals:?}");
}

#[test]
fn saving_as_goes_on_with_the_new_file_and_leaves_the_old_as_written() {
    let scratch = Scratch::new("saveas");
    scratch.write("Text.\n");
    let (mut document, _) = scratch.open();
    insert(&mut document, 0, "Unsaved ");
    let to = scratch.root.join("work").join("copy.typ");
    std::fs::write(&to, "There.\n").unwrap_or_else(|err| panic!("{err}"));
    assert!(document.save_as(&to, false).is_err());
    assert_eq!(
        std::fs::read_to_string(&to).ok().as_deref(),
        Some("There.\n")
    );
    document
        .save_as(&to, true)
        .unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(
        std::fs::read_to_string(&to).ok().as_deref(),
        Some("Unsaved Text.\n")
    );
    assert_eq!(scratch.read(), "Text.\n");
    assert!(!document.is_modified());
    insert(&mut document, 0, "More. ");
    document.save(false).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(
        std::fs::read_to_string(&to).ok().as_deref(),
        Some("More. Unsaved Text.\n")
    );
    assert_eq!(document.undo().ok().flatten(), Some(1));
    drop(document);
    // The old file opens as it was written, with nothing brought back.
    let (document, opened) = scratch.open();
    assert_eq!(opened, Opened::Clean);
    assert_eq!(document.text(), "Text.\n");
    // A copy is written, and the document stays with its file.
    let copy = scratch.root.join("work").join("second.typ");
    document
        .write_copy(&copy, false)
        .unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(
        std::fs::read_to_string(&copy).ok().as_deref(),
        Some("Text.\n")
    );
    assert!(document.write_copy(&copy, false).is_err());
    let file = scratch.file().canonicalize().unwrap_or_default();
    assert_eq!(document.path(), file.as_path());
}

#[cfg(unix)]
#[test]
fn a_file_in_a_directory_that_takes_no_new_file_is_written_in_place() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("in-place");
    scratch.write("Text.\n");
    let (mut document, _) = scratch.open();
    insert(&mut document, 0, "More. ");
    let work = scratch.root.join("work");
    let mode = |mode| std::fs::set_permissions(&work, std::fs::Permissions::from_mode(mode));
    mode(0o555).unwrap_or_else(|err| panic!("{err}"));
    if std::fs::write(work.join("probe"), "").is_ok() {
        // Permissions mean nothing here (run as root).
        let _ = mode(0o755);
        return;
    }
    let saved = document.save(false);
    let _ = mode(0o755);
    saved.unwrap_or_else(|err| panic!("{err}"));
    assert!(document.wrote_in_place());
    assert_eq!(scratch.read(), "More. Text.\n");
    assert!(!document.is_modified());
}

#[test]
fn thinning_folds_old_steps_and_keeps_every_saved_text() {
    let scratch = Scratch::new("thin");
    scratch.write("");
    let (mut document, _) = scratch.open();
    let mut saved = Vec::new();
    for step in 0..60 {
        let end = document.text().len();
        insert(&mut document, end, &format!("{step} "));
        if step % 20 == 5 {
            document.save(false).unwrap_or_else(|err| panic!("{err}"));
            saved.push(document.text().to_owned());
        }
    }
    // A branch left behind.
    assert!(document.undo().ok().flatten().is_some());
    insert(&mut document, 0, "branch ");
    let text = document.text().to_owned();
    let before = document.history().len();
    let map = document.thin(10).unwrap_or_else(|err| panic!("{err}"));
    let after = document.history().len();
    assert!(after < before / 2, "{before} -> {after}");
    assert_eq!(document.text(), text);
    // Every saved text and the end of the branch left are still there.
    let texts: Vec<String> = (0..after as u64)
        .filter_map(|state| document.text_at(state))
        .collect();
    for saved in &saved {
        assert!(texts.contains(saved), "{saved:?} is gone");
    }
    assert!(texts.iter().any(|text| text.ends_with("59 ")));
    assert_eq!(map.iter().flatten().count(), after);
    // The journal says the same.
    let history = document.history().clone();
    drop(document);
    let (document, opened) = scratch.open();
    assert_eq!(opened, Opened::Restored);
    assert_eq!(document.history(), &history);
    assert_eq!(document.text(), text);
}

/// Opening a long history of a long text. Run by hand, in release:
/// `cargo test --release --workspace --offline -- --ignored --nocapture
/// opening_a_long_history`.
#[test]
#[ignore = "a measurement, not a check"]
fn opening_a_long_history() {
    use std::hash::{Hash as _, Hasher as _};
    use std::time::Instant;
    const STEPS: usize = 5000;
    let article = include_str!("../../zetwerk-corpus/docs/article.typ");
    // About 300 pages.
    let text = article.repeat(200);
    let scratch = Scratch::new("long");
    scratch.write(&text);
    {
        let (mut document, _) = scratch.open();
        let mut at = 7_usize;
        for step in 0..STEPS {
            at = (at * 7919 + step) % document.text().len();
            while !document.text().is_char_boundary(at) {
                at -= 1;
            }
            insert(&mut document, at, "word ");
            if step % 100 == 99 {
                document.save(false).unwrap_or_else(|err| panic!("{err}"));
            }
        }
    }
    let journal = std::fs::metadata(scratch.journal()).map_or(0, |meta| meta.len());
    println!(
        "{} KB of text, {STEPS} steps, a journal of {} KB",
        text.len() / 1024,
        journal / 1024
    );
    let time = |what: &str, scratch: &Scratch| {
        let started = Instant::now();
        let (document, opened) = scratch.open();
        println!(
            "{what}: opened in {:.0} ms ({opened:?}, {} states)",
            started.elapsed().as_secs_f64() * 1000.0,
            document.history().len()
        );
    };
    time("as left", &scratch);
    // Changed while closed, to a text no state had: every state is a
    // candidate, as before.
    scratch.write(&format!("{}x", scratch.read()));
    time("changed while closed", &scratch);
    // What looking at every state's text costs, as opening did before:
    // a step back and a hash of the whole text, for every state.
    let (mut document, _) = scratch.open();
    let started = Instant::now();
    let mut steps = 0;
    while document.undo().ok().flatten().is_some() {
        let bytes = document.text().as_bytes().to_vec();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hasher);
        std::hint::black_box(hasher.finish());
        steps += 1;
    }
    println!(
        "a step and a hash of each of {steps} states: {:.0} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
}
