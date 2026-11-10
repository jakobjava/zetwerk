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
    let (document, opened) = scratch.open();
    assert_eq!(opened, Opened::ChangedWhileClosed);
    assert_eq!(document.text(), "theirs");
    assert_eq!(document.history().len(), 1);
    assert!(!document.is_modified());
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
    let (document, opened) = scratch.open();
    let Opened::Conflict { recovered, journal } = opened else {
        panic!("expected a conflict, got {opened:?}")
    };
    assert_eq!(recovered, "base + my unsaved work");
    assert_eq!(document.text(), "base + their change");
    assert!(!document.is_modified());
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
