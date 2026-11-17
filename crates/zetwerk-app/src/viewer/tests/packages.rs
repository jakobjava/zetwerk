//! Packages without a terminal: a document that lacks one says so and
//! fetches it at the writer's word (from a stand-in for the registry on
//! 127.0.0.1), and never otherwise; `:packages` says what is there.
//! Every test keeps its packages in a directory of its own.

use std::sync::mpsc;
use std::time::Instant;

use zetwerk_typst::packages::stand_in::{Registry, manifest, package};
use zetwerk_typst::packages::{PackageDirs, find, parse_spec};

use super::*;
use crate::viewer::packages::{Download, Settings};

const TOUYING: &str = "#import \"@preview/touying:0.8.0\": *\n#slide[Hello]\n";
const SLIDE: &str = "#let slide(body) = body\n#let title-slide() = none\n";

/// A viewer editing `text` as `main.typ`, with its packages kept in a
/// directory of its own (the second), and `download` as the settings
/// say.
fn opened(text: &str, download: Download) -> (Viewer, PathBuf, PathBuf) {
    let directory = directory(text);
    let packages = directory.join("packages");
    let path = directory.join("main.typ");
    let dirs = PackageDirs::within(&packages);
    let env = Environment::with_packages(FontSet::Embedded, dirs.clone());
    let project = Project::open(Arc::clone(&env), &path).unwrap();
    let (editor, opened) = Editor::open(&path, &directory.join("state"), project.main()).unwrap();
    let mut viewer = Viewer::new(env, project, (1000, 620), 1.0)
        .unwrap_or_else(|| panic!("viewer sets up"))
        .editing(editor, &opened);
    viewer.state_dir = Some(directory.join("state"));
    viewer.set_packages(&Settings { download, dirs });
    compile(&mut viewer);
    (viewer, directory, packages)
}

fn dirs(packages: &Path) -> PackageDirs {
    PackageDirs::within(packages)
}

fn said(viewer: &Viewer) -> String {
    viewer
        .message
        .as_ref()
        .map(|message| message.text.clone())
        .unwrap_or_default()
}

/// The request lines the stand-in was asked.
fn asked(registry: &Registry) -> Vec<String> {
    registry
        .asked()
        .iter()
        .map(|asked| asked.lines().next().unwrap_or("").to_owned())
        .collect()
}

/// Puts the package `name` at `version` among the fetched ones.
fn fetched_already(packages: &Path, name: &str, version: &str, lib: &str, more: &str) {
    let dir = packages.join("cache/preview").join(name).join(version);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("typst.toml"), manifest(name, version, more)).unwrap();
    std::fs::write(dir.join("lib.typ"), lib).unwrap();
}

#[test]
fn a_document_that_lacks_a_package_says_so_and_g_fetches_it() {
    let (path, archive) = package("touying", "0.8.0", SLIDE);
    let registry = Registry::serve(vec![(path, archive)], None).unwrap();
    let (mut viewer, directory, packages) = opened(TOUYING, Download::Ask);
    viewer.packages_from(&registry.url);
    // Opened on a machine that does not have it: no page, and the panel
    // says why, by itself.
    assert!(viewer.document.is_none());
    assert!(viewer.refused_up());
    let status = viewer.refused_status().unwrap();
    assert!(
        status.contains("this document uses touying 0.8.0, which is not on this machine"),
        "{status}"
    );
    let lines = viewer.package_lines();
    assert!(
        lines[0].starts_with("g      fetch touying 0.8.0 from packages.typst.org"),
        "{lines:?}"
    );
    assert!(
        lines[1].contains("nothing of the document is sent"),
        "{lines:?}"
    );
    // Nothing went anywhere for all that.
    assert_eq!(asked(&registry), Vec::<String>::new());
    assert!(!viewer.packages_progress(), "nothing is fetched unasked");
    assert_eq!(asked(&registry), Vec::<String>::new());

    press(&mut viewer, "g");
    assert_eq!(
        asked(&registry),
        ["GET /preview/touying-0.8.0.tar.gz HTTP/1.1"]
    );
    assert!(find(&dirs(&packages), &parse_spec("touying:0.8.0").unwrap()).is_some());
    assert!(
        said(&viewer).contains("touying 0.8.0 fetched into"),
        "{}",
        said(&viewer)
    );
    assert_eq!(viewer.take_edited(), Some(Edited::Command));
    // And the document is typeset, by itself.
    compile(&mut viewer);
    assert!(viewer.document.is_some(), "{:?}", viewer.problems);
    assert_eq!(viewer.problems, Vec::<String>::new());
    assert_eq!(viewer.package_lines(), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn with_downloads_off_nothing_is_fetched_and_the_panel_says_why() {
    let (path, archive) = package("touying", "0.8.0", SLIDE);
    let registry = Registry::serve(vec![(path, archive)], None).unwrap();
    let (mut viewer, directory, packages) = opened(TOUYING, Download::Never);
    viewer.packages_from(&registry.url);
    let lines = viewer.package_lines();
    assert!(lines[0].contains("fetching is off"), "{lines:?}");
    assert!(!viewer.package_offered());
    press(&mut viewer, ":package get<CR>");
    assert!(
        said(&viewer).contains("download = \"never\""),
        "{}",
        said(&viewer)
    );
    press(&mut viewer, ":package search slides<CR>");
    press(&mut viewer, ":package updates<CR>");
    assert_eq!(asked(&registry), Vec::<String>::new(), "never means never");
    assert!(find(&dirs(&packages), &parse_spec("touying:0.8.0").unwrap()).is_none());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_standing_word_fetches_by_itself_but_not_what_an_agent_imported() {
    let (path, archive) = package("touying", "0.8.0", SLIDE);
    let (cetz_path, cetz) = package("cetz", "0.3.2", "#let canvas(body) = body\n");
    let registry = Registry::serve(vec![(path, archive), (cetz_path, cetz)], None).unwrap();
    let (mut viewer, directory, packages) = opened(TOUYING, Download::Always);
    viewer.packages_from(&registry.url);
    assert!(viewer.packages_progress(), "fetched, to be typeset again");
    assert_eq!(asked(&registry).len(), 1);
    compile(&mut viewer);
    assert!(viewer.document.is_some(), "{:?}", viewer.problems);
    assert!(!viewer.packages_progress());

    // An agent writes an import of what is not here: the writer is
    // asked, even so.
    let keys = keys::parse("ggO#import \"@preview/cetz:0.3.2\"<Esc>").unwrap();
    viewer.keys_of_agent(&keys);
    compile(&mut viewer);
    assert!(
        viewer.missing().iter().any(|spec| spec.name == "cetz"),
        "{:?}",
        viewer.problems
    );
    assert!(!viewer.packages_progress());
    assert_eq!(
        asked(&registry).len(),
        1,
        "an agent's import is not fetched"
    );
    let lines = viewer.package_lines();
    assert!(
        lines.iter().any(|line| line.contains("an agent wrote")),
        "{lines:?}"
    );
    // Nor can its keys fetch it.
    viewer.keys_of_agent(&keys::parse(":package get cetz:0.3.2<CR>").unwrap());
    assert!(
        said(&viewer).contains("an agent cannot fetch"),
        "{}",
        said(&viewer)
    );
    assert_eq!(asked(&registry).len(), 1);
    // The writer can.
    press(&mut viewer, ":why<CR>g");
    assert_eq!(asked(&registry).len(), 2);
    assert!(find(&dirs(&packages), &parse_spec("cetz:0.3.2").unwrap()).is_some());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_fetch_runs_beside_the_window_says_how_far_it_came_and_is_abandoned() {
    // Large enough, and slow enough, to be abandoned while it comes.
    let mut state: u64 = 0x2545_f491_4f6c_dd1d;
    let noise: Vec<u8> = (0..200_000)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect();
    let toml = manifest("slow", "1.0.0", "");
    let slow = zetwerk_typst::packages::stand_in::archive(&[
        ("typst.toml", toml.as_bytes()),
        ("lib.typ", b"#let x = 1\n"),
        ("noise.bin", &noise),
    ]);
    let registry = Registry::serve(
        vec![("/preview/slow-1.0.0.tar.gz".to_owned(), slow)],
        Some(Duration::from_millis(5)),
    )
    .unwrap();
    let (mut viewer, directory, packages) = opened("Text.\n", Download::Ask);
    viewer.packages_from(&registry.url);
    let (woke, woken) = mpsc::channel();
    viewer.packages_in_background(move || {
        let _ = woke.send(());
    });
    press(&mut viewer, ":package get slow:1.0.0<CR>");
    assert!(
        said(&viewer).starts_with("fetching slow 1.0.0"),
        "{}",
        said(&viewer)
    );
    // The window is told as bytes come.
    let began = Instant::now();
    while !said(&viewer).contains("KiB") && began.elapsed() < Duration::from_secs(20) {
        let _ = woken.recv_timeout(Duration::from_secs(1));
        viewer.packages_progress();
    }
    assert!(
        said(&viewer).contains("Ctrl-C abandons"),
        "{}",
        said(&viewer)
    );
    // A second one waits for none: one at a time.
    press(&mut viewer, ":package get other:1.0.0<CR>");
    assert!(
        said(&viewer).contains("one fetch at a time"),
        "{}",
        said(&viewer)
    );
    press(&mut viewer, "<C-c>");
    assert!(said(&viewer).contains("abandoned"), "{}", said(&viewer));
    assert!(!viewer.packages_progress());
    // Nothing of it stays, half or whole.
    let cache = packages.join("cache/preview/slow");
    let began = Instant::now();
    while std::fs::read_dir(&cache).is_ok_and(|mut entries| entries.next().is_some())
        && began.elapsed() < Duration::from_secs(10)
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(find(&dirs(&packages), &parse_spec("slow:1.0.0").unwrap()).is_none());
    assert!(
        std::fs::read_dir(&cache).map_or(true, |mut entries| entries.next().is_none()),
        "nothing half unpacked is left"
    );

    // One that is not abandoned is there once the window is woken.
    let (path, archive) = package("quick", "1.0.0", "#let y = 2\n");
    let quick = Registry::serve(vec![(path, archive)], None).unwrap();
    viewer.packages_from(&quick.url);
    press(&mut viewer, ":package get quick:1.0.0<CR>");
    let began = Instant::now();
    let mut compile_now = false;
    while !compile_now && began.elapsed() < Duration::from_secs(20) {
        let _ = woken.recv_timeout(Duration::from_secs(1));
        compile_now = viewer.packages_progress();
    }
    assert!(compile_now, "{}", said(&viewer));
    assert!(find(&dirs(&packages), &parse_spec("quick:1.0.0").unwrap()).is_some());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn packages_are_listed_described_and_removed_when_asked_twice() {
    let text = "#import \"@preview/touying:0.8.0\": *\n#import \"@preview/cetz:0.3.2\"\n\
                #slide[Hello]\n#cetz.canvas[]\n";
    let (mut viewer, directory, packages) = opened("", Download::Ask);
    fetched_already(&packages, "touying", "0.8.0", SLIDE, "");
    fetched_already(&packages, "physica", "0.9.3", "#let dd = 1\n", "");
    let mine = packages.join("data/local/mine/1.0.0");
    std::fs::create_dir_all(&mine).unwrap();
    std::fs::write(mine.join("typst.toml"), manifest("mine", "1.0.0", "")).unwrap();
    std::fs::write(mine.join("lib.typ"), "").unwrap();
    std::fs::write(directory.join("main.typ"), text).unwrap();
    viewer.file_changed(&directory.join("main.typ"));
    compile(&mut viewer);

    press(&mut viewer, ":packages<CR>");
    let (title, items, _) = viewer.picker.as_ref().unwrap().listing();
    assert_eq!(title, "packages");
    assert_eq!(
        items[..4],
        [
            "cetz 0.3.2\tused here · not on this machine",
            "touying 0.8.0\tused here · fetched",
            "@local/mine 1.0.0\tyour own",
            "physica 0.9.3\tfetched",
        ]
    );
    // What one is, and what of it the document uses.
    press(&mut viewer, "<Down><CR>");
    let (title, items, chosen) = viewer.picker.as_ref().unwrap().listing();
    assert_eq!(title, "package");
    assert_eq!(items[0], "touying 0.8.0: The touying package.");
    assert!(
        items
            .iter()
            .any(|item| item.contains("by A. Writer · MIT · begins at lib.typ"))
    );
    assert!(
        items
            .iter()
            .any(|item| item.starts_with("the document uses slide (in main.typ)")),
        "{items:?}"
    );
    assert!(items[chosen].starts_with("remove it from this machine"));
    // Removing is asked first; "no" is what Enter takes.
    press(&mut viewer, "<CR>");
    let (title, items, chosen) = viewer.picker.as_ref().unwrap().listing();
    assert_eq!((title, chosen), ("remove?", 0));
    assert_eq!(items[0], "no, keep it");
    press(&mut viewer, "<Esc>");
    press(&mut viewer, "<Esc><Esc>");
    let touying = parse_spec("touying:0.8.0").unwrap();
    assert!(find(&dirs(&packages), &touying).is_some());
    press(&mut viewer, ":package remove touying:0.8.0<CR><Down><CR>");
    assert!(find(&dirs(&packages), &touying).is_none());
    assert!(
        said(&viewer).contains("touying 0.8.0 removed"),
        "{}",
        said(&viewer)
    );
    // The writer's own are never removed.
    press(&mut viewer, ":package remove! @local/mine:1.0.0<CR>");
    assert!(mine.is_dir());
    assert!(said(&viewer).contains("not removed"), "{}", said(&viewer));
    // An agent learns the same, and nothing is fetched for it.
    let listed = viewer.packages_listed();
    assert_eq!(listed["imported"][0]["package"], "@preview/cetz:0.3.2");
    assert_eq!(listed["imported"][0]["on_machine"], false);
    assert_eq!(listed["imported"][0]["uses"], serde_json::json!(["canvas"]));
    assert_eq!(listed["on_machine"][0]["package"], "@local/mine:1.0.0");
    assert_eq!(listed["on_machine"][0]["kept"], "own");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn newer_versions_and_new_packages_come_from_the_registrys_list() {
    let index = br#"[
        {"name": "touying", "version": "0.8.0", "description": "Slides"},
        {"name": "touying", "version": "0.9.1", "description": "Slides"},
        {"name": "cetz", "version": "0.4.2", "description": "Drawing with a canvas"}
    ]"#;
    let (newer, newer_archive) = package("touying", "0.9.1", SLIDE);
    let (cetz, cetz_archive) = package("cetz", "0.4.2", "#let canvas(b) = b\n");
    let registry = Registry::serve(
        vec![
            ("/preview/index.json".to_owned(), index.to_vec()),
            (newer, newer_archive),
            (cetz, cetz_archive),
        ],
        None,
    )
    .unwrap();
    let (mut viewer, directory, packages) = opened(TOUYING, Download::Ask);
    fetched_already(&packages, "touying", "0.8.0", SLIDE, "");
    viewer.project.packages_changed();
    viewer.file_changed(&directory.join("main.typ"));
    compile(&mut viewer);
    viewer.packages_from(&registry.url);

    press(&mut viewer, ":package updates<CR>");
    assert_eq!(asked(&registry), ["GET /preview/index.json HTTP/1.1"]);
    let (title, items, chosen) = viewer.picker.as_ref().unwrap().listing();
    assert_eq!(title, "newer versions");
    assert!(items[0].starts_with("as packages.typst.org listed them just now"));
    assert!(
        items[chosen].starts_with("touying 0.8.0 → 0.9.1"),
        "{items:?}"
    );
    press(&mut viewer, "<CR>");
    assert_eq!(
        viewer.text(),
        Some("#import \"@preview/touying:0.9.1\": *\n#slide[Hello]\n")
    );
    let dirs = dirs(&packages);
    assert!(find(&dirs, &parse_spec("touying:0.9.1").unwrap()).is_some());
    assert!(
        find(&dirs, &parse_spec("touying:0.8.0").unwrap()).is_some(),
        "the old stays"
    );
    // One step back.
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some(TOUYING));

    // The list is kept, with its date: asked once.
    press(&mut viewer, ":package search canvas<CR>");
    let (title, items, chosen) = viewer.picker.as_ref().unwrap().listing();
    assert_eq!(title, "found");
    assert_eq!(items[chosen], "cetz 0.4.2\tDrawing with a canvas");
    press(&mut viewer, "<CR>");
    assert_eq!(
        viewer.text(),
        Some(
            "#import \"@preview/touying:0.8.0\": *\n#import \"@preview/cetz:0.4.2\"\n#slide[Hello]\n"
        )
    );
    assert!(find(&dirs, &parse_spec("cetz:0.4.2").unwrap()).is_some());
    let lines = asked(&registry);
    assert_eq!(
        lines,
        [
            "GET /preview/index.json HTTP/1.1",
            "GET /preview/touying-0.9.1.tar.gz HTTP/1.1",
            "GET /preview/cetz-0.4.2.tar.gz HTTP/1.1",
        ]
    );
    // Added twice: it is there already.
    press(&mut viewer, ":package add cetz<CR>");
    assert!(
        said(&viewer).contains("imported already"),
        "{}",
        said(&viewer)
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn local_packages_just_work_and_templates_are_begun_from() {
    let (mut viewer, directory, packages) = opened("", Download::Ask);
    let mine = packages.join("data/local/mine/1.0.0");
    std::fs::create_dir_all(&mine).unwrap();
    std::fs::write(mine.join("typst.toml"), manifest("mine", "1.0.0", "")).unwrap();
    std::fs::write(mine.join("lib.typ"), "#let greet = [Hello]\n").unwrap();
    let template = "[template]\npath = \"template\"\nentrypoint = \"main.typ\"\n";
    fetched_already(
        &packages,
        "thesis",
        "1.0.0",
        "#let thesis(body) = body\n",
        template,
    );
    let thesis = packages.join("cache/preview/thesis/1.0.0/template");
    std::fs::create_dir_all(&thesis).unwrap();
    std::fs::write(
        thesis.join("main.typ"),
        "#import \"@preview/thesis:1.0.0\": thesis\n#import \"@local/mine:1.0.0\": greet\n\
         #show: thesis\n#greet\n",
    )
    .unwrap();
    std::fs::write(thesis.join("refs.bib"), "@book{a, title={A}}\n").unwrap();
    std::fs::write(directory.join("refs.bib"), "mine\n").unwrap();
    std::fs::write(thesis.join("logo.svg"), "<svg/>").unwrap();

    press(&mut viewer, ":new<CR>");
    let (title, items, _) = viewer.picker.as_ref().unwrap().listing();
    assert_eq!(title, "begin with");
    let at = items
        .iter()
        .position(|item| item.starts_with("thesis 1.0.0\t"))
        .unwrap();
    for _ in 0..at {
        press(&mut viewer, "<Down>");
    }
    press(&mut viewer, "<CR>");
    assert!(
        viewer
            .text()
            .unwrap()
            .starts_with("#import \"@preview/thesis:1.0.0\""),
        "{:?} {}",
        viewer.text(),
        said(&viewer)
    );
    assert!(
        said(&viewer).contains("beside it: logo.svg"),
        "{}",
        said(&viewer)
    );
    assert_eq!(
        std::fs::read_to_string(directory.join("refs.bib")).unwrap(),
        "mine\n"
    );
    compile(&mut viewer);
    assert!(viewer.document.is_some(), "{:?}", viewer.problems);
    // Its text goes back with one undo.
    press(&mut viewer, "<Esc>u");
    assert_eq!(viewer.text(), Some(""));
    let _ = std::fs::remove_dir_all(directory);
}
