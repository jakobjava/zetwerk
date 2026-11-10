//! Following a document's files as they change on disk.

use std::path::Path;

use zetwerk_typst::{Environment, FontSet, Project};

/// A directory that is removed again when the test is done.
struct Scratch(std::path::PathBuf);

impl std::ops::Deref for Scratch {
    type Target = std::path::Path;

    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A fresh directory holding `main.typ`, which includes `chapter.typ`.
fn document(name: &str) -> Scratch {
    let directory =
        std::env::temp_dir().join(format!("zetwerk-reload-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap_or_else(|err| panic!("{err}"));
    write(
        &directory.join("main.typ"),
        "Main.\n#include \"chapter.typ\"\n",
    );
    write(&directory.join("chapter.typ"), "Chapter one.");
    Scratch(directory)
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap_or_else(|err| panic!("{err}"));
}

fn open(directory: &Path) -> Project {
    Project::open(
        Environment::new(FontSet::Embedded),
        &directory.join("main.typ"),
    )
    .unwrap_or_else(|err| panic!("{err}"))
}

/// Compiles and returns the text of every text item on the first page.
fn page_text(project: &Project) -> String {
    use typst::layout::{Frame, FrameItem};
    fn collect(frame: &Frame, out: &mut String) {
        for (_, item) in frame.items() {
            match item {
                FrameItem::Text(text) => out.push_str(&text.text),
                FrameItem::Group(group) => collect(&group.frame, out),
                _ => {}
            }
        }
    }
    let document = zetwerk_typst::compile(&project.snapshot())
        .document
        .unwrap_or_else(|| panic!("document compiles"));
    let mut text = String::new();
    collect(&document.pages()[0].frame, &mut text);
    text
}

#[test]
fn dependencies_are_the_files_a_compile_read() {
    let directory = document("dependencies");
    let project = open(&directory);
    assert_eq!(project.dependencies(), [directory.join("main.typ")]);
    let _ = page_text(&project);
    let mut dependencies = project.dependencies();
    dependencies.sort();
    assert_eq!(
        dependencies,
        [directory.join("chapter.typ"), directory.join("main.typ")]
    );
}

#[test]
fn a_changed_main_file_is_read_again() {
    let directory = document("main");
    let main = directory.join("main.typ");
    let mut project = open(&directory);
    assert!(page_text(&project).contains("Main."));

    write(&main, "Rewritten.\n#include \"chapter.typ\"\n");
    assert_eq!(project.file_changed(&main).ok(), Some(true));
    assert!(page_text(&project).contains("Rewritten."));
    // Being told again without a change is not a reason to compile.
    assert_eq!(project.file_changed(&main).ok(), Some(false));
}

#[test]
fn a_changed_included_file_is_read_again() {
    let directory = document("included");
    let chapter = directory.join("chapter.typ");
    let mut project = open(&directory);
    assert!(page_text(&project).contains("Chapter one."));

    write(&chapter, "Chapter two.");
    // Without being told, the project keeps what it read.
    assert!(page_text(&project).contains("Chapter one."));
    assert_eq!(project.file_changed(&chapter).ok(), Some(true));
    assert!(page_text(&project).contains("Chapter two."));
}

#[test]
fn files_the_document_does_not_use_are_ignored() {
    let directory = document("unrelated");
    let mut project = open(&directory);
    let _ = page_text(&project);
    let other = directory.join("notes.txt");
    write(&other, "unrelated");
    assert_eq!(project.file_changed(&other).ok(), Some(false));
    assert_eq!(
        project.file_changed(Path::new("/elsewhere/main.typ")).ok(),
        Some(false)
    );
}

#[test]
fn a_vanished_main_file_is_an_error_and_keeps_the_text() {
    let directory = document("vanished");
    let main = directory.join("main.typ");
    let mut project = open(&directory);
    std::fs::remove_file(&main).unwrap_or_else(|err| panic!("{err}"));
    assert!(project.file_changed(&main).is_err());
    assert!(page_text(&project).contains("Main."));
}
