//! The clipboard and what comes through it: images and tables pasted
//! on the page, tables shaped by keys and copied out as cells.

use super::*;
use crate::clipboard::{Copied, News};

/// What the message line says.
fn said(viewer: &Viewer) -> String {
    viewer
        .message
        .as_ref()
        .map(|message| message.text.clone())
        .unwrap_or_default()
}

/// The text of the cell the cursor is in, brackets and all.
fn cell_of_cursor(viewer: &Viewer) -> String {
    let editor = viewer.editor.as_ref().unwrap();
    let (source, cursor) = (editor.source(), editor.cursor());
    let table = crate::typst_text::table_at(source, cursor + 1).unwrap();
    let cell = table
        .cell_at(cursor + 1)
        .or_else(|| table.cell_at(cursor))
        .unwrap();
    source.text()[table.cells[cell].clone()].to_owned()
}

/// The text from the call of the table to its end.
fn table(viewer: &Viewer) -> String {
    let text = viewer.text().unwrap();
    let start = text.find("#table(").unwrap();
    let end = text[start..].find("\n)").unwrap();
    text[start..start + end + 2].to_owned()
}

const TABLE: &str = "Before.\n\n#table(\n  columns: 2,\n  [a], [b],\n  [c], [d],\n)\n\nAfter.\n";

#[test]
fn a_table_is_shaped_by_keys_on_the_page() {
    let (mut viewer, directory) = editing(TABLE);
    press(&mut viewer, "gsgs/b<CR>");
    // (keys, the table after them, the cell the cursor is in)
    let steps: &[(&str, &str, &str)] = &[
        ("]r", "  [a], [b],\n  [], [],\n  [c], [d],", "[]"),
        ("ugg/b<CR>[r", "  [], [],\n  [a], [b],\n  [c], [d],", "[]"),
        (
            "ugg/b<CR>]c",
            "columns: 3,\n  [a], [b], [],\n  [c], [d], [],",
            "[]",
        ),
        (
            "ugg/b<CR>[c",
            "columns: 3,\n  [a], [], [b],\n  [c], [], [d],",
            "[]",
        ),
        ("ugg/b<CR>]R", "  [c], [d],\n  [a], [b],", "[b]"),
        // The cursor went along: up again.
        ("[R", "  [a], [b],\n  [c], [d],", "[b]"),
        ("[C", "  [b], [a],\n  [d], [c],", "[b]"),
        ("]C", "  [a], [b],\n  [c], [d],", "[b]"),
        ("]h", "  table.header([a], [b]),\n  [c], [d],", "[b]"),
        ("[h", "  [a], [b],\n  [c], [d],", "[b]"),
        ("]x", "columns: 2,\n  [c], [d],\n)", "[d]"),
        ("ugg/b<CR>]X", "columns: 1,\n  [a],\n  [c],\n)", "[a]"),
    ];
    for (keys, wanted, cell) in steps {
        press(&mut viewer, keys);
        let now = table(&viewer);
        assert!(
            now.contains(wanted),
            "{keys}: {now}\nnot with\n{wanted}\n({})",
            said(&viewer)
        );
        assert_eq!(cell_of_cursor(&viewer), *cell, "{keys}: {now}");
        assert!(
            said(&viewer).contains("u takes it back"),
            "{keys}: {}",
            said(&viewer)
        );
    }
    // Each was one step: all of them undone, the table is as it was.
    press(&mut viewer, "uuuuuuuuuuuuuuuu");
    assert_eq!(viewer.text(), Some(TABLE));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_table_is_shaped_by_commands_and_refused_where_it_cannot_be() {
    let (mut viewer, directory) = editing(TABLE);
    press(&mut viewer, "gsgs/d<CR>:table row add above<CR>");
    assert!(table(&viewer).contains("  [a], [b],\n  [], [],\n  [c], [d],"));
    press(&mut viewer, "ugg/d<CR>:table row move down<CR>");
    assert!(said(&viewer).contains("last row"), "{}", said(&viewer));
    assert_eq!(viewer.text(), Some(TABLE));
    // The commands there were before still are.
    press(&mut viewer, ":table column<CR>");
    assert!(table(&viewer).contains("columns: 3"));
    // Out of a table, and in one with more to it than cells.
    press(&mut viewer, "u/After<CR>]r");
    assert!(said(&viewer).contains("no table"), "{}", said(&viewer));
    let other = "#table(columns: 2, table.cell(colspan: 2)[wide], [x], [y])\n";
    let (mut viewer, directory2) = editing(other);
    press(&mut viewer, "gsgs/x<CR>]r");
    assert!(
        said(&viewer).contains("more to it than cells"),
        "{}",
        said(&viewer)
    );
    assert_eq!(viewer.text(), Some(other));
    let _ = std::fs::remove_dir_all(directory);
    let _ = std::fs::remove_dir_all(directory2);
}

/// What a test's clipboard was given, in place of the system's.
static COPIED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

#[test]
fn a_table_is_copied_out_as_cells_for_a_spreadsheet() {
    let (mut viewer, directory) = editing(TABLE);
    viewer.clipboard.write = |text| {
        COPIED.lock().unwrap().push(text.to_owned());
        Ok(())
    };
    press(&mut viewer, "gsgs/c<CR>]y");
    assert_eq!(*COPIED.lock().unwrap(), ["a\tb\nc\td\n"]);
    assert!(said(&viewer).contains("2 rows of 2"), "{}", said(&viewer));
    // It is not written again after the key.
    press(&mut viewer, "l");
    assert_eq!(COPIED.lock().unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(directory);
}

/// A PNG of `wide` by `high` pixels.
fn png(wide: u32, high: u32) -> Vec<u8> {
    let image = image::RgbImage::from_pixel(wide, high, image::Rgb([30, 90, 160]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

/// A clipboard that holds a small screenshot.
#[allow(
    clippy::unnecessary_wraps,
    reason = "it stands for a read that can fail"
)]
fn screenshot(_: crate::clipboard::Selection) -> Result<Copied, String> {
    Ok(Copied {
        types: vec!["image/png".to_owned()],
        image: Some(("image/png".to_owned(), png(40, 20))),
        ..Copied::default()
    })
}

/// The files in `directory` whose names end in `ending`.
fn files_ending(directory: &Path, ending: &str) -> Vec<String> {
    let mut found: Vec<String> = std::fs::read_dir(directory)
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.ends_with(ending))
        .collect();
    found.sort();
    found
}

#[test]
fn an_image_on_the_clipboard_becomes_a_figure_whose_caption_is_typed() {
    let text = "= Results\n\nThe plot shows it.\n\nAfter.\n";
    let (mut viewer, directory) = editing(text);
    viewer.paste.read = Some(screenshot);
    // Nothing yanked yet: `p` puts what was copied.
    press(&mut viewer, "gsgs/shows<CR>p");
    assert_eq!(files_ending(&directory, ".png"), ["results.png"]);
    assert_eq!(viewer.editor.as_ref().unwrap().mode(), editor::Mode::Insert);
    assert!(
        said(&viewer).contains("results.png written (40×20 pixels"),
        "{}",
        said(&viewer)
    );
    press(&mut viewer, "A plot<Esc>");
    assert_eq!(
        viewer.text(),
        Some(
            "= Results\n\nThe plot shows it.\n\n#figure(\n  image(\"results.png\"),\n  \
             caption: [A plot],\n)\n\nAfter.\n"
        )
    );
    compile(&mut viewer);
    assert!(viewer.diagnostics.is_empty(), "{:?}", viewer.diagnostics);
    // `P` puts it above; the same image again is the same file.
    press(&mut viewer, "gg/After<CR>P<Esc>");
    assert_eq!(files_ending(&directory, ".png"), ["results.png"]);
    assert!(
        viewer
            .text()
            .unwrap()
            .ends_with("caption: [],\n)\n\nAfter.\n")
    );
    // Undone: the figure goes, the file stays, and that is said.
    press(&mut viewer, "u");
    assert!(
        said(&viewer).contains("results.png stays"),
        "{}",
        said(&viewer)
    );
    press(&mut viewer, "uu");
    assert_eq!(viewer.text(), Some(text));
    assert_eq!(files_ending(&directory, ".png"), ["results.png"]);
    let _ = std::fs::remove_dir_all(directory);
}

/// The number of the last change of a test's clipboard.
static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[allow(
    clippy::unnecessary_wraps,
    reason = "it stands for news that can be none"
)]
fn news() -> Option<News> {
    Some(News {
        serial: SERIAL.load(std::sync::atomic::Ordering::Relaxed),
        ours: false,
    })
}

#[test]
fn p_puts_what_was_copied_last_here_or_elsewhere() {
    let (mut viewer, directory) = editing("One two.\n");
    viewer.paste.read = Some(|_| Ok(Copied::text("copied ")));
    viewer.paste.news = news;
    // Yanked after the clipboard was copied: the yank.
    press(&mut viewer, "gsgs0yiwP");
    assert_eq!(viewer.text(), Some("OneOne two.\n"));
    // Copied elsewhere after that: what was copied, and again.
    SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    press(&mut viewer, "u0P");
    assert_eq!(viewer.text(), Some("copied One two.\n"));
    press(&mut viewer, "0P");
    assert_eq!(viewer.text(), Some("copied copied One two.\n"));
    // And yanked again: the yank.
    press(&mut viewer, "/two<CR>yiwuu0P");
    assert_eq!(viewer.text(), Some("twoOne two.\n"), "{}", said(&viewer));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn cells_on_the_clipboard_become_a_table() {
    let (mut viewer, directory) = editing("Prices.\n\nAfter.\n");
    viewer.paste.read = Some(|_| Ok(Copied::text("Item\tPrice\nTea #1\t1.50\nCake\t12\n")));
    press(&mut viewer, "gsgs/Prices<CR>pTea and cake<Esc>");
    assert_eq!(
        viewer.text(),
        Some(
            "Prices.\n\n#figure(\n  table(\n    columns: 2,\n    align: (left, right),\n    \
             table.header([Item], [Price]),\n    [Tea \\#1], [1.50],\n    [Cake], [12],\n  ),\n  \
             caption: [Tea and cake],\n)\n\nAfter.\n"
        )
    );
    compile(&mut viewer);
    assert!(viewer.diagnostics.is_empty(), "{:?}", viewer.diagnostics);
    // A web page's table, where it is offered.
    viewer.paste.read = Some(|_| {
        Ok(Copied {
            types: vec!["text/html".to_owned(), "text/plain".to_owned()],
            text: Some("a\tb\n".to_owned()),
            html: Some("<table><tr><td>x &amp; y</td><td>z</td></tr></table>".to_owned()),
            ..Copied::default()
        })
    });
    press(&mut viewer, "G:paste<CR><Esc>");
    assert!(
        viewer.text().unwrap().contains("[x & y], [z],"),
        "{}",
        viewer.text().unwrap()
    );
    // `:paste text` puts the words as they are.
    viewer.paste.read = Some(|_| Ok(Copied::text("a\tb")));
    press(&mut viewer, "u:paste table<CR><Esc>");
    assert!(viewer.text().unwrap().contains("[a], [b],"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn cells_pasted_into_a_table_fill_it_from_the_cursor_on() {
    let (mut viewer, directory) = editing(TABLE);
    viewer.paste.read = Some(|_| Ok(Copied::text("1\t2\t3\n4\t5\t6\n")));
    // Over cells that hold something: asked first; no is the default.
    press(&mut viewer, "gsgs/d<CR>p");
    assert!(viewer.picker.is_some());
    press(&mut viewer, "<CR>");
    assert_eq!(viewer.text(), Some(TABLE));
    press(&mut viewer, "gg/d<CR>p<Up><CR>");
    assert_eq!(
        table(&viewer),
        "#table(\n  columns: 4,\n  [a], [b], [], [],\n  [c], [1], [2], [3],\n  [], [4], [5], [6],\n)"
    );
    assert_eq!(cell_of_cursor(&viewer), "[1]");
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some(TABLE));
    // Into empty cells: at once; by the command, in the source too.
    let empty = "#table(\n  columns: 2,\n  [x], [],\n  [], [],\n)\n";
    let (mut viewer, directory2) = editing(empty);
    viewer.paste.read = Some(|_| Ok(Copied::text("1\t2\n")));
    press(&mut viewer, "3G$hh:paste<CR>");
    assert_eq!(
        viewer.text(),
        Some("#table(\n  columns: 3,\n  [x], [1], [2],\n  [], [], [],\n)\n"),
        "{}",
        said(&viewer)
    );
    let _ = std::fs::remove_dir_all(directory);
    let _ = std::fs::remove_dir_all(directory2);
}

/// The files a test's clipboard holds.
static FILES: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

#[test]
fn files_copied_are_figures_copied_in_or_named_where_they_are() {
    let (mut viewer, directory) = editing("Text.\n");
    std::fs::write(
        directory.join("figures.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"4\" height=\"4\"/>",
    )
    .unwrap();
    // As a file manager copies them: an image beside the document is
    // named where it is; what is no image is passed over.
    *FILES.lock().unwrap() = vec![
        std::env::temp_dir().join("zetwerk-not-there.pdf"),
        directory.join("figures.svg"),
    ];
    viewer.paste.read = Some(|_| {
        Ok(Copied {
            types: vec!["text/uri-list".to_owned()],
            files: FILES.lock().unwrap().clone(),
            ..Copied::default()
        })
    });
    press(&mut viewer, "gsgs:paste<CR><Esc>");
    assert!(
        viewer.text().unwrap().contains("image(\"figures.svg\")"),
        "{}",
        said(&viewer)
    );
    assert_eq!(files_ending(&directory, ".svg"), ["figures.svg"]);
    // Only a PDF: not an image.
    *FILES.lock().unwrap() = vec![directory.join("paper.pdf")];
    press(&mut viewer, ":paste<CR>");
    assert!(said(&viewer).contains("PDF"), "{}", said(&viewer));
    // From outside the document's folder: copied in, under its name.
    let outside =
        std::env::temp_dir().join(format!("zetwerk-paste-outside-{}", std::process::id()));
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("My Photo.png"), png(10, 10)).unwrap();
    *FILES.lock().unwrap() = vec![outside.join("My Photo.png")];
    press(&mut viewer, ":paste<CR><Esc>");
    assert!(
        viewer.text().unwrap().contains("image(\"my-photo.png\")"),
        "{}",
        said(&viewer)
    );
    assert!(directory.join("my-photo.png").is_file());
    assert!(outside.join("My Photo.png").is_file());
    let _ = std::fs::remove_dir_all(outside);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_large_screenshot_is_offered_smaller() {
    let (mut viewer, directory) = editing("Text.\n");
    viewer.paste.read = Some(|_| {
        Ok(Copied {
            types: vec!["image/png".to_owned()],
            image: Some(("image/png".to_owned(), png(3000, 1000))),
            ..Copied::default()
        })
    });
    press(&mut viewer, "gsgsp");
    assert!(viewer.picker.is_some(), "{}", said(&viewer));
    assert_eq!(viewer.text(), Some("Text.\n"));
    press(&mut viewer, "<CR>");
    let written = files_ending(&directory, ".png");
    assert_eq!(written.len(), 1);
    let bytes = std::fs::read(directory.join(&written[0])).unwrap();
    assert_eq!(crate::image_file::dimensions(&bytes), Some((2000, 667)));
    assert!(said(&viewer).contains("2000×667"), "{}", said(&viewer));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn keys_wait_for_the_clipboard_read_aside_and_go_on_in_their_order() {
    let (mut viewer, directory) = editing("One.\n");
    viewer.paste.read = Some(|_| {
        std::thread::sleep(Duration::from_millis(50));
        Ok(Copied::text("x"))
    });
    viewer.paste_in_background(|| {});
    press(&mut viewer, "gsgs\"+P");
    // Read aside: what is typed waits.
    press(&mut viewer, "Aa<Esc>");
    assert_eq!(viewer.text(), Some("One.\n"));
    let begun = Instant::now();
    while !viewer.paste_progress() {
        assert!(begun.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(viewer.text(), Some("xOne.a\n"));
    // Escape gives a wait up; what follows it goes on.
    press(&mut viewer, "\"+P<Esc>Ab<Esc>");
    assert_eq!(viewer.text(), Some("xOne.ab\n"));
    assert!(viewer.paste.reading.is_none());
    let _ = std::fs::remove_dir_all(directory);
}

/// What a test's primary selection was given.
static PRIMARY: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

#[test]
fn the_register_star_is_the_primary_selection() {
    let (mut viewer, directory) = editing("one two\n");
    viewer.paste.write_primary = |text| {
        PRIMARY.lock().unwrap().push(text.to_owned());
        Ok(())
    };
    viewer.paste.read = Some(|selection| {
        assert_eq!(selection, crate::clipboard::Selection::Primary);
        Ok(Copied::text("sel "))
    });
    press(&mut viewer, "\"*yiw");
    assert_eq!(*PRIMARY.lock().unwrap(), ["one"]);
    press(&mut viewer, "0\"*P");
    assert_eq!(viewer.text(), Some("sel one two\n"));
    // Nothing else reads it: `"+` and plain keys do not.
    press(&mut viewer, "yiwx");
    assert_eq!(PRIMARY.lock().unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn latex_pasted_in_running_text_comes_as_typst() {
    let (mut viewer, directory) = editing("First.\n\nAfter.\n");
    viewer.paste.read = Some(|_| Ok(Copied::text(" so \\(\\alpha \\leq 1\\) holds")));
    press(&mut viewer, "gsgs/First<CR>$\"+p");
    let text = viewer.text().unwrap();
    assert!(text.contains("$alpha <= 1$ holds"), "{text}");
    assert!(!text.contains('\\'), "{text}");
    compile(&mut viewer);
    assert!(viewer.diagnostics.is_empty(), "{:?}", viewer.diagnostics);
    // Words with no LaTeX in them come as they are.
    viewer.paste.read = Some(|_| Ok(Copied::text(" plain words")));
    press(&mut viewer, "\"+p");
    assert!(viewer.text().unwrap().contains(" plain words"));
    let _ = std::fs::remove_dir_all(directory);
}
