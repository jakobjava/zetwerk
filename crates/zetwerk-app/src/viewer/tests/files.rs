//! The files of the user's that the viewer writes: never in part, never
//! over what it must not write over.

use super::*;

/// The file's bytes, or nothing if it cannot be read.
fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

#[cfg(unix)]
fn set_read_only(directory: &Path, read_only: bool) {
    use std::os::unix::fs::PermissionsExt;
    let mode = if read_only { 0o555 } else { 0o755 };
    let _ = std::fs::set_permissions(directory, std::fs::Permissions::from_mode(mode));
}

#[cfg(unix)]
fn set_file_read_only(file: &Path, read_only: bool) {
    use std::os::unix::fs::PermissionsExt;
    let mode = if read_only { 0o444 } else { 0o644 };
    let _ = std::fs::set_permissions(file, std::fs::Permissions::from_mode(mode));
}

#[cfg(unix)]
#[test]
fn a_write_that_fails_leaves_the_file_and_the_text_as_they_were() {
    let (mut viewer, directory) = editing("first\n");
    let main = directory.join("main.typ");
    press(&mut viewer, "Asecond<Esc>");
    // The file cannot be put in place: nothing beside it can be made,
    // and the file itself takes nothing (where it does, it is written
    // over in place: see the test after this one).
    set_read_only(&directory, true);
    set_file_read_only(&main, true);
    let probe = directory.join("probe");
    if std::fs::write(&probe, "").is_ok() {
        // Permissions mean nothing here (run as root): nothing to show.
        let _ = std::fs::remove_file(probe);
        set_read_only(&directory, false);
        let _ = std::fs::remove_dir_all(directory);
        return;
    }
    press(&mut viewer, ":w<CR>");
    assert_eq!(read(&main).as_deref(), Some("first\n"));
    assert_eq!(viewer.text(), Some("firstsecond\n"));
    assert!(
        viewer
            .state()
            .editing
            .is_some_and(|editing| editing.modified)
    );
    // The sources' file is not made in part either.
    press(&mut viewer, ":w!<CR>");
    assert_eq!(read(&main).as_deref(), Some("first\n"));
    set_read_only(&directory, false);
    set_file_read_only(&main, false);
    press(&mut viewer, ":w<CR>");
    assert_eq!(read(&main).as_deref(), Some("firstsecond\n"));
    let _ = std::fs::remove_dir_all(directory);
}

#[cfg(unix)]
#[test]
fn a_file_made_by_edit_is_made_where_it_is_named_and_nowhere_else() {
    // A link that leads nowhere is no file that is not there: making it
    // would make the file the link leads to, which is another.
    let (mut viewer, directory) = editing("Text.\n");
    let target = directory.join("elsewhere.typ");
    let link = directory.join("chapter.typ");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    press(&mut viewer, ":e! chapter.typ<CR>");
    assert!(!target.exists(), "the file behind the link was made");
    // A file that is not there is made, empty.
    press(&mut viewer, ":e! new.typ<CR>");
    assert_eq!(read(&directory.join("new.typ")).as_deref(), Some(""));
    let _ = std::fs::remove_dir_all(directory);
}

#[cfg(unix)]
#[test]
fn the_sources_file_is_whole_or_as_it_was() {
    let entry = "@book{knuth1984, author = {Donald Knuth}, title = {The TeXbook}, year = {1984}}\n";
    let (mut viewer, directory) = editing("Some words.\n\n#bibliography(\"refs.bib\")\n");
    let bib = directory.join("refs.bib");
    std::fs::write(&bib, entry).unwrap();
    set_read_only(&directory, true);
    let probe = directory.join("probe");
    let permissions_hold = std::fs::write(&probe, "").is_err();
    let _ = std::fs::remove_file(probe);
    let added = viewer.agent_add_sources(
        "@article{lovelace1843, author = {Ada Lovelace}, title = {Notes}, year = {1843}}",
    );
    if permissions_hold {
        assert!(added.is_err(), "{added:?}");
        assert_eq!(read(&bib).as_deref(), Some(entry));
        let leftovers: Vec<_> = std::fs::read_dir(&directory)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .filter(|name| name.to_string_lossy().contains("zetwerk-new"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
    set_read_only(&directory, false);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn nothing_is_exported_of_a_state_of_the_history_that_is_only_looked_at() {
    // Looking through the history typesets the state chosen in place of
    // the text; the pages are then said to be compiled. An export asked
    // for then (by an agent, which waits for that) wrote the old state.
    let (mut viewer, directory) = editing("One.\n");
    press(&mut viewer, "Atwo<Esc>");
    compile(&mut viewer);
    press(&mut viewer, "gh<Down>");
    compile(&mut viewer);
    let wanted = super::super::export::Wanted::parse("pdf", "").unwrap();
    assert!(viewer.plan_export(&wanted).is_err());
    assert!(!directory.join("main.pdf").exists());
    // Back at the text: its pages are exported.
    press(&mut viewer, "<Esc>");
    compile(&mut viewer);
    assert!(viewer.plan_export(&wanted).is_ok());
    let _ = std::fs::remove_dir_all(directory);
}

/// The journal of the one file opened with state under `directory`.
fn journal_of(directory: &Path) -> PathBuf {
    std::fs::read_dir(directory.join("state").join("journals"))
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ending| ending == "journal"))
        .unwrap()
}

/// What the status line says, alert or not.
fn said(viewer: &Viewer) -> (String, bool) {
    viewer
        .message
        .as_ref()
        .map(|message| (message.text.clone(), message.alert))
        .unwrap_or_default()
}

#[test]
fn unsaved_changes_brought_back_say_how_old_they_are_until_dropped() {
    // A stray edit left three days ago came back nearly silently: a
    // quiet notice gone at the first key, without its age.
    let (mut viewer, directory) = editing("= Paper\n\nGood text.\n");
    press(&mut viewer, "Gdd");
    drop(viewer);
    let three_days_ago = std::time::SystemTime::now() - Duration::from_mins(72 * 60 + 1);
    std::fs::File::options()
        .append(true)
        .open(journal_of(&directory))
        .and_then(|file| file.set_modified(three_days_ago))
        .unwrap();

    let (mut viewer, opened) = open(&directory);
    assert_eq!(opened, Opened::Restored);
    let (text, alert) = said(&viewer);
    assert!(alert, "{text}");
    assert!(
        text.contains("3 days ago") && text.contains(":e!"),
        "{text}"
    );
    // Long after the message is gone, the status line still says it.
    press(&mut viewer, "gg");
    assert_eq!(
        viewer.file_label(true),
        "main.typ [+ restored, last changed 3 days ago]"
    );
    // `:e!` drops them, and undo brings them back.
    press(&mut viewer, ":e!<CR>");
    assert_eq!(viewer.text(), Some("= Paper\n\nGood text.\n"));
    assert_eq!(viewer.file_label(false), "main.typ");
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some("= Paper\n\n"));
    assert_eq!(viewer.file_label(true), "main.typ [+]");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn leaving_without_saving_drops_the_changes_and_keeps_the_history() {
    // `:q!` kept what was not written in the journal: a stray edit came
    // back days later and broke the document it was left in.
    let (mut viewer, directory) = editing("= Paper\n\nGood text.\n");
    press(&mut viewer, "Gdd:q<CR>");
    assert!(!viewer.wants_quit(), "there are unsaved changes");
    let (text, alert) = said(&viewer);
    assert!(alert && text.contains(":q! drops them"), "{text}");
    press(&mut viewer, ":q!<CR>");
    assert!(viewer.wants_quit());
    drop(viewer);
    // The file is as it was written last, and so is the text when it is
    // opened again: nothing is brought back, nothing is announced.
    let (mut viewer, opened) = open(&directory);
    assert_eq!(opened, Opened::Clean, "{:?}", said(&viewer));
    assert_eq!(viewer.text(), Some("= Paper\n\nGood text.\n"));
    assert_eq!(viewer.file_label(false), "main.typ");
    assert_eq!(
        read(&directory.join("main.typ")).as_deref(),
        Some("= Paper\n\nGood text.\n")
    );
    // What was dropped is in the history still.
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some("= Paper\n\n"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_file_changed_while_unsaved_changes_waited_keeps_both() {
    // The unsaved text was handed over and dropped: only a journal no
    // one can read was left of it.
    let (mut viewer, directory) = editing("Base.\n");
    press(&mut viewer, "AMine.<Esc>");
    drop(viewer);
    std::fs::write(directory.join("main.typ"), "Base. Theirs.\n").unwrap();
    let (mut viewer, opened) = open(&directory);
    assert!(matches!(opened, Opened::Conflict { .. }), "{opened:?}");
    let (text, alert) = said(&viewer);
    assert!(alert && text.contains("u brings back"), "{text}");
    assert_eq!(viewer.text(), Some("Base. Theirs.\n"));
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some("Base.Mine.\n"));
    assert_eq!(
        read(&directory.join("main.typ")).as_deref(),
        Some("Base. Theirs.\n")
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[cfg(unix)]
#[test]
fn the_sources_file_keeps_who_may_read_it() {
    use std::os::unix::fs::PermissionsExt;
    let (mut viewer, directory) = editing("Some words.\n\n#bibliography(\"refs.bib\")\n");
    let bib = directory.join("refs.bib");
    std::fs::write(&bib, "% mine\n").unwrap();
    std::fs::set_permissions(&bib, std::fs::Permissions::from_mode(0o600)).unwrap();
    viewer
        .agent_add_sources(
            "@article{lovelace1843, author = {Ada Lovelace}, title = {Notes}, year = {1843}}",
        )
        .unwrap();
    assert!(read(&bib).unwrap().contains("lovelace1843"));
    let mode = std::fs::metadata(&bib).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "a private bibliography became readable to all");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_word_changed_changes_no_other_byte_of_the_file() {
    // A byte order mark, CRLF line ends, no line end at the end, and a
    // path with blanks and letters beyond ASCII. A lone line feed was
    // put at the end of a file whose lines end in CRLF.
    let directory = directory("");
    let folder = directory.join("mein Ordner é");
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join("Arbeit ü.typ");
    let before = "\u{feff}= Titel\r\n\r\nEin altes Wort.\r\n\r\nDas Ende";
    std::fs::write(&path, before).unwrap();
    let env = Environment::new(FontSet::Embedded);
    let project = Project::open(Arc::clone(&env), &path).unwrap();
    let (editor, opened) = Editor::open(&path, &directory.join("state"), project.main()).unwrap();
    assert_eq!(opened, Opened::Clean);
    let mut viewer = Viewer::new(env, project, (1000, 620), 1.0)
        .unwrap()
        .editing(editor, &opened);
    viewer.state_dir = Some(directory.join("state"));
    compile(&mut viewer);
    press(&mut viewer, ":w<CR>");
    assert_eq!(std::fs::read(&path).unwrap(), before.as_bytes());
    let at = viewer.text().unwrap().find("altes").unwrap();
    viewer.editor.as_mut().unwrap().set_cursor(at);
    press(&mut viewer, "ciwneues<Esc>:w<CR>");
    // As Vim writes it, the text ends with a line break now: one like
    // its others, not a lone line feed after lines that end in CRLF.
    let after = format!("{}\r\n", before.replace("altes", "neues"));
    assert_eq!(read(&path).as_deref(), Some(after.as_str()));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn unsaved_changes_in_a_file_not_open_are_not_forgotten() {
    // The window closed with a chapter unsaved: the next time, the pages
    // showed the chapter as the disk has it, and nothing said that the
    // changes waited until the chapter was opened.
    let directory = directory("= Main\n\n#include \"one.typ\"\n");
    std::fs::write(directory.join("one.typ"), "Chapter text.\n").unwrap();
    let (mut viewer, _) = open(&directory);
    press(&mut viewer, ":e one.typ<CR>");
    press(&mut viewer, "AMore.<Esc>");
    drop(viewer);
    let (mut viewer, opened) = open(&directory);
    assert_eq!(opened, Opened::Clean);
    let (text, alert) = said(&viewer);
    assert!(alert && text.contains("one.typ"), "{text:?}");
    press(&mut viewer, ":e /one.typ<CR>");
    assert_eq!(viewer.text(), Some("Chapter text.More.\n"));
    // Once written, nothing waits.
    press(&mut viewer, ":w<CR>");
    drop(viewer);
    let (viewer, _) = open(&directory);
    assert!(!said(&viewer).0.contains("one.typ"), "{:?}", said(&viewer));
    let _ = std::fs::remove_dir_all(directory);
}

#[cfg(unix)]
#[test]
fn a_file_in_a_directory_that_takes_no_new_file_is_written_over_in_place() {
    // Saving failed in a directory one may not add to, though the file
    // itself was the user's to write.
    let (mut viewer, directory) = editing("first\n");
    let main = directory.join("main.typ");
    press(&mut viewer, "Asecond<Esc>");
    set_read_only(&directory, true);
    let probe = directory.join("probe");
    let permissions_hold = std::fs::write(&probe, "").is_err();
    let _ = std::fs::remove_file(probe);
    press(&mut viewer, ":w<CR>");
    set_read_only(&directory, false);
    if permissions_hold {
        assert_eq!(read(&main).as_deref(), Some("firstsecond\n"));
        let (text, alert) = said(&viewer);
        assert!(!alert && text.contains("in place"), "{text}");
    }
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn lines_typed_into_a_file_of_crlf_lines_get_crlf() {
    // The engine types line feeds; in a file of CR LF lines they were
    // written as they were, and the file had line ends of both kinds.
    let directory = directory("");
    let main = directory.join("main.typ");
    std::fs::write(&main, "= Title\r\n\r\nOld words.\r\n").unwrap();
    let (mut viewer, _) = open(&directory);
    press(&mut viewer, "GoNew line.<CR>And one more.<Esc>:w<CR>");
    assert_eq!(
        std::fs::read(&main).unwrap(),
        b"= Title\r\n\r\nOld words.\r\nNew line.\r\nAnd one more.\r\n"
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_copy_is_written_elsewhere_and_the_file_stays_the_one_edited() {
    let (mut viewer, directory) = editing("Words.\n\n#include \"part.typ\"\n");
    std::fs::write(directory.join("part.typ"), "Part.\n").unwrap();
    compile(&mut viewer);
    press(&mut viewer, "ggAMore.<Esc>");
    let away = directory.join("away");
    std::fs::create_dir_all(&away).unwrap();
    press(&mut viewer, ":w away/copy.typ<CR>");
    let copy = away.join("copy.typ");
    assert_eq!(
        read(&copy).as_deref(),
        Some("Words.More.\n\n#include \"part.typ\"\n")
    );
    let (text, alert) = said(&viewer);
    assert!(!alert && text.contains("main.typ"), "{text}");
    // What the document needs beside it is not copied, and that is said.
    assert!(text.contains("part.typ"), "{text}");
    // The file being edited is still main.typ, with its changes unsaved.
    assert_eq!(viewer.file_label(false), "main.typ");
    assert!(
        viewer
            .state()
            .editing
            .is_some_and(|editing| editing.modified)
    );
    assert_eq!(
        read(&directory.join("main.typ")).as_deref(),
        Some("Words.\n\n#include \"part.typ\"\n")
    );
    // A file there is not written over without !.
    press(&mut viewer, "AAgain.<Esc>:w away/copy.typ<CR>");
    assert!(said(&viewer).1, "{:?}", said(&viewer));
    assert!(!read(&copy).unwrap().contains("Again."));
    press(&mut viewer, ":w! away/copy.typ<CR>");
    assert!(read(&copy).unwrap().contains("Again."));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn saving_as_goes_on_with_the_new_file_and_its_history() {
    let (mut viewer, directory) = editing("Words.\n");
    press(&mut viewer, "AMore.<Esc>:saveas second.typ<CR>");
    let second = directory.join("second.typ");
    assert_eq!(read(&second).as_deref(), Some("Words.More.\n"));
    assert_eq!(
        read(&directory.join("main.typ")).as_deref(),
        Some("Words.\n")
    );
    assert_eq!(viewer.file_label(false), "second.typ");
    // Typing goes on in the new file, which is the document now.
    press(&mut viewer, "AAgain.<Esc>:w<CR>");
    assert_eq!(read(&second).as_deref(), Some("Words.More.Again.\n"));
    compile(&mut viewer);
    assert!(viewer.is_compiled());
    // The history came along.
    press(&mut viewer, "uu");
    assert_eq!(viewer.text(), Some("Words.\n"));
    // A file there is not written over without !.
    press(&mut viewer, ":saveas main.typ<CR>");
    assert!(said(&viewer).1, "{:?}", said(&viewer));
    assert_eq!(
        read(&directory.join("main.typ")).as_deref(),
        Some("Words.\n")
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn moving_takes_the_file_and_its_history_and_writes_over_nothing() {
    let (mut viewer, directory) = editing("Words.\n");
    std::fs::write(directory.join("taken.typ"), "Mine.\n").unwrap();
    press(&mut viewer, "AMore.<Esc>:mv taken.typ<CR>");
    assert!(said(&viewer).1, "{:?}", said(&viewer));
    assert_eq!(
        read(&directory.join("taken.typ")).as_deref(),
        Some("Mine.\n")
    );
    press(&mut viewer, ":mv renamed.typ<CR>");
    assert!(!directory.join("main.typ").exists());
    assert_eq!(
        read(&directory.join("renamed.typ")).as_deref(),
        Some("Words.\n")
    );
    assert_eq!(viewer.file_label(false), "renamed.typ");
    assert!(
        viewer
            .state()
            .editing
            .is_some_and(|editing| editing.modified)
    );
    press(&mut viewer, ":w<CR>");
    assert_eq!(
        read(&directory.join("renamed.typ")).as_deref(),
        Some("Words.More.\n")
    );
    compile(&mut viewer);
    assert!(viewer.is_compiled());
    drop(viewer);
    // Opened again under the new name, the history is there.
    std::fs::rename(directory.join("renamed.typ"), directory.join("main.typ")).unwrap();
    let (mut viewer, _) = open(&directory);
    let (text, _) = said(&viewer);
    assert!(text.contains("history taken over"), "{text}");
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some("Words.\n"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn an_export_writes_over_no_file_it_did_not_write() {
    // `:png 1 cover.png` replaced a file of that name that was there.
    let (mut viewer, directory) = editing("= Cover\n\nText.\n");
    let cover = directory.join("cover.png");
    std::fs::write(&cover, b"someone's picture").unwrap();
    press(&mut viewer, ":png 1 cover.png<CR>");
    assert_eq!(std::fs::read(&cover).unwrap(), b"someone's picture");
    let (text, alert) = said(&viewer);
    assert!(alert && text.contains("cover.png"), "{text}");
    // With ! it is written over; and again, without it, being this
    // window's now.
    press(&mut viewer, ":png! 1 cover.png<CR>");
    assert!(std::fs::read(&cover).unwrap().starts_with(b"\x89PNG"));
    std::fs::remove_file(&cover).unwrap();
    press(&mut viewer, ":pdf out.pdf<CR>");
    press(&mut viewer, ":pdf out.pdf<CR>");
    assert!(!said(&viewer).1, "{:?}", said(&viewer));
    assert!(
        std::fs::read(directory.join("out.pdf"))
            .unwrap()
            .starts_with(b"%PDF")
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_damaged_journal_says_what_is_lost_and_gives_back_what_it_can() {
    let (mut viewer, directory) = editing("Saved.\n");
    press(&mut viewer, "AOne.<Esc>");
    drop(viewer);
    let (mut viewer, _) = open(&directory);
    press(&mut viewer, "ATwo.<Esc>");
    drop(viewer);
    let journal = journal_of(&directory);
    let mut bytes = std::fs::read(&journal).unwrap();
    bytes[20] ^= 0x10;
    std::fs::write(&journal, &bytes).unwrap();
    let (viewer, opened) = open(&directory);
    assert_eq!(opened, Opened::Restored);
    assert_eq!(viewer.text(), Some("Saved.One.Two.\n"));
    let (text, alert) = said(&viewer);
    assert!(alert && text.contains("damaged"), "{text}");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn another_start_for_a_file_open_here_is_said_here() {
    let (mut viewer, directory) = editing("Words.\n");
    let knocks = viewer.knocks();
    assert_eq!(knocks.len(), 1);
    zetwerk_core::knock(&directory.join("main.typ"), &directory.join("state")).unwrap();
    assert!(viewer.knocked(&knocks));
    let (text, alert) = said(&viewer);
    assert!(alert && text.contains("asked to open again"), "{text}");
    // Taking it up removes it: that is no second word.
    assert!(!viewer.knocked(&knocks));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn taking_back_the_first_source_takes_the_file_it_made() {
    // `:bib undo` of the first source left an empty file behind.
    let (mut viewer, directory) = editing("Words.\n\n#bibliography(\"refs.bib\")\n");
    let refs = directory.join("refs.bib");
    viewer
        .agent_add_sources(
            "@book{knuth1984, author = {Donald Knuth}, title = {The TeXbook}, year = {1984}}",
        )
        .unwrap();
    assert!(read(&refs).unwrap().contains("knuth1984"));
    viewer.bib_command("undo");
    assert!(!refs.exists(), "{:?}", read(&refs));
    viewer.bib_command("redo");
    assert!(read(&refs).unwrap().contains("knuth1984"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn notes_go_along_with_a_file_moved() {
    // Notes are kept by the file's name: after `:mv` they stayed behind.
    let (mut viewer, directory) = editing("A paragraph of words.\n");
    press(&mut viewer, ":note look at this<CR>");
    assert_eq!(viewer.notes().len(), 1);
    press(&mut viewer, ":mv moved.typ<CR>");
    assert_eq!(viewer.notes().len(), 1);
    drop(viewer);
    let path = directory.join("moved.typ");
    let env = Environment::new(FontSet::Embedded);
    let project = Project::open(Arc::clone(&env), &path).unwrap();
    let (editor, opened) = Editor::open(&path, &directory.join("state"), project.main()).unwrap();
    let mut viewer = Viewer::new(env, project, (1000, 620), 1.0)
        .unwrap()
        .editing(editor, &opened);
    viewer.state_dir = Some(directory.join("state"));
    let listed = viewer.notes_listed(true);
    assert_eq!(listed.len(), 1);
    let _ = std::fs::remove_dir_all(directory);
}
