//! The clipboard and what comes through it: images and tables pasted
//! on the page, tables shaped by keys and copied out as cells.

use super::*;

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
        ("gH", "  table.header([a], [b]),\n  [c], [d],", "[b]"),
        ("gH", "  [a], [b],\n  [c], [d],", "[b]"),
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
    press(&mut viewer, "gsgs/c<CR>gY");
    assert_eq!(*COPIED.lock().unwrap(), ["a\tb\nc\td\n"]);
    assert!(said(&viewer).contains("2 rows of 2"), "{}", said(&viewer));
    // It is not written again after the key.
    press(&mut viewer, "l");
    assert_eq!(COPIED.lock().unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(directory);
}
