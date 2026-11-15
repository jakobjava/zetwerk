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
#[test]
fn a_write_that_fails_leaves_the_file_and_the_text_as_they_were() {
    let (mut viewer, directory) = editing("first\n");
    let main = directory.join("main.typ");
    press(&mut viewer, "Asecond<Esc>");
    // The file cannot be put in place: nothing beside it can be made.
    set_read_only(&directory, true);
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
    press(&mut viewer, "Gdd:q!<CR>");
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
fn a_file_changed_while_unsaved_changes_waited_keeps_both() {
    // The unsaved text was handed over and dropped: only a journal no
    // one can read was left of it.
    let (mut viewer, directory) = editing("Base.\n");
    press(&mut viewer, "AMine.<Esc>:q!<CR>");
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
