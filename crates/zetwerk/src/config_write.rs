//! Changes made in the window, written into the settings file.
//!
//! The file is the user's: it is edited where it says what changed, and
//! nowhere else. Its comments, its order, its blank lines and the way
//! each value is written around stay as they were; a value that changes
//! keeps the comment behind it; what is new goes behind what its section
//! has, and a section that is new behind all. An option that goes back
//! to what holds anyway is taken out, and so is a section that has
//! nothing left and was never said anything of. The file is read anew
//! for every change, so that what was written into it by hand meanwhile
//! is kept, and written whole or not at all (a new file beside it that
//! takes its place). A file that is no TOML is left alone.

use std::path::Path;
use std::sync::Arc;

use toml_edit::{Array, DocumentMut, Item, Table, TableLike, Value};
use zetwerk_app::settings::{Edit, Writer, Written};

/// What writes changes into the settings file at `path`.
pub fn writer(path: &Path) -> Writer {
    let path = path.to_owned();
    Arc::new(move |edits: &[Edit]| write(&path, edits))
}

/// Makes the changes `edits` to the settings file at `path`, which need
/// not be there yet.
pub fn write(path: &Path, edits: &[Edit]) -> Result<(), String> {
    // A file kept elsewhere and linked to stays where it is, and linked.
    let linked = std::fs::canonicalize(path).ok();
    let path = linked.as_deref().unwrap_or(path);
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("{} cannot be read: {err}", path.display())),
    };
    let changed = edited(&text, edits)
        .map_err(|why| format!("{} is left as it is: {why}", path.display()))?;
    if changed == text {
        return Ok(());
    }
    let cannot = |err: std::io::Error| format!("{} cannot be written: {err}", path.display());
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory).map_err(cannot)?;
    }
    // Whoever reads it meanwhile finds the old file or the new one.
    let new = path.with_extension("toml.new");
    std::fs::write(&new, changed.as_bytes()).map_err(cannot)?;
    std::fs::rename(&new, path).map_err(|err| {
        let _ = std::fs::remove_file(&new);
        cannot(err)
    })
}

/// `text` with the changes `edits`, or why they cannot be made.
pub fn edited(text: &str, edits: &[Edit]) -> Result<String, String> {
    let mut document: DocumentMut = text
        .parse()
        .map_err(|err: toml_edit::TomlError| format!("it is no TOML: {}", err.message()))?;
    for edit in edits {
        match &edit.value {
            Some(value) => put(&mut document, &edit.path, value)?,
            None => take(&mut document, &edit.path),
        }
    }
    let mut written = document.to_string();
    if !text.is_empty() && text.ends_with('\n') && !written.ends_with('\n') {
        written.push('\n');
    }
    Ok(written)
}

/// `written` as TOML writes it.
fn value(written: &Written) -> Value {
    match written {
        Written::Bool(on) => Value::from(*on),
        Written::Integer(number) => Value::from(*number),
        Written::Float(number) => Value::from(*number),
        Written::Text(text) => Value::from(text.as_str()),
        Written::List(texts) => Value::from(texts.iter().map(String::as_str).collect::<Array>()),
    }
}

/// Puts `written` at `path`, making the sections it is in where they
/// are not.
fn put(document: &mut DocumentMut, path: &[String], written: &Written) -> Result<(), String> {
    let Some((key, sections)) = path.split_last() else {
        return Ok(());
    };
    let mut table: &mut dyn TableLike = document.as_table_mut();
    for (at, section) in sections.iter().enumerate() {
        if table.get(section).is_none() {
            let mut new = Table::new();
            // `[keys.normal]` stands by itself, without an empty `[keys]`.
            new.set_implicit(at + 1 < sections.len());
            table.insert(section, Item::Table(new));
        }
        table = table
            .get_mut(section)
            .and_then(Item::as_table_like_mut)
            .ok_or_else(|| format!("{} is no section", sections[..=at].join(".")))?;
    }
    let new = value(written);
    match table.get_mut(key) {
        Some(Item::Value(old)) => {
            // As it was written around: the comment behind it stays.
            let decor = old.decor().clone();
            *old = new;
            *old.decor_mut() = decor;
        }
        _ => {
            table.insert(key, Item::Value(new));
        }
    }
    Ok(())
}

/// Takes out what is at `path`, and the sections that are left empty
/// by it and have nothing written in or before them.
fn take(document: &mut DocumentMut, path: &[String]) {
    fn take_in(table: &mut dyn TableLike, path: &[String]) {
        let Some((first, rest)) = path.split_first() else {
            return;
        };
        if rest.is_empty() {
            table.remove(first);
            return;
        }
        let Some(inner) = table.get_mut(first) else {
            return;
        };
        let Some(inner_table) = inner.as_table_like_mut() else {
            return;
        };
        take_in(inner_table, rest);
        let empty = inner_table.is_empty();
        let bare = match inner {
            Item::Table(section) => {
                let decor = section.decor();
                let said = |raw: Option<&toml_edit::RawString>| {
                    raw.and_then(toml_edit::RawString::as_str)
                        .is_some_and(|text| text.contains('#'))
                };
                !said(decor.prefix()) && !said(decor.suffix())
            }
            _ => true,
        };
        if empty && bare {
            table.remove(first);
        }
    }
    take_in(document.as_table_mut(), path);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(path: &[&str], value: Option<Written>) -> Edit {
        Edit {
            path: path.iter().map(|part| (*part).to_owned()).collect(),
            value,
        }
    }

    /// A file as a user writes one: comments, blank lines, an order of
    /// their own, a value written oddly.
    const ODD: &str = "\
# my settings, written by hand
[view]
ui_scale = 1.25   # a little larger: the laptop

[keys.insert]
\"jk\" = \"<Esc>\"

# agents last
[ai]
edit_mode = 'propose'
";

    #[test]
    fn a_change_leaves_every_other_byte_of_the_file() {
        let changed = edited(
            ODD,
            &[edit(&["view", "ui_scale"], Some(Written::Float(1.5)))],
        )
        .unwrap_or_default();
        assert_eq!(changed, ODD.replace("1.25", "1.5"));
        // Something new goes behind what its section has; a new section
        // behind all.
        let changed = edited(
            ODD,
            &[
                edit(&["view", "slides"], Some(Written::Bool(true))),
                edit(&["aids", "numbers"], Some(Written::Text("always".into()))),
            ],
        )
        .unwrap_or_default();
        assert!(
            changed.starts_with(
                "# my settings, written by hand\n[view]\nui_scale = 1.25   # a little larger: the laptop\nslides = true\n"
            ),
            "{changed}"
        );
        assert!(
            changed.ends_with("[aids]\nnumbers = \"always\"\n"),
            "{changed}"
        );
        assert!(changed.contains("edit_mode = 'propose'"));
        // And taken out again, the file is as it was.
        let back = edited(
            &changed,
            &[
                edit(&["view", "slides"], None),
                edit(&["aids", "numbers"], None),
            ],
        )
        .unwrap_or_default();
        assert_eq!(back, ODD);
    }

    #[test]
    fn keys_of_the_user_are_written_in_their_section() {
        let changed = edited(
            ODD,
            &[edit(
                &["keys", "normal", "<C-s>"],
                Some(Written::Text(":w<CR>".into())),
            )],
        )
        .unwrap_or_default();
        assert!(
            changed.contains("[keys.normal]\n\"<C-s>\" = \":w<CR>\"\n"),
            "{changed}"
        );
        assert!(!changed.contains("[keys]\n"), "{changed}");
        let gone = edited(ODD, &[edit(&["keys", "insert", "jk"], None)]).unwrap_or_default();
        assert!(!gone.contains("jk") && !gone.contains("[keys"), "{gone}");
        // A section with a comment of the user's stays, emptied or not.
        let gone = edited(ODD, &[edit(&["ai", "edit_mode"], None)]).unwrap_or_default();
        assert!(gone.contains("# agents last\n[ai]\n"), "{gone}");
    }

    #[test]
    fn a_new_file_and_one_that_is_no_toml() {
        let new = edited(
            "",
            &[edit(
                &["aids", "preset"],
                Some(Written::Text("writing".into())),
            )],
        );
        assert_eq!(new.as_deref(), Ok("[aids]\npreset = \"writing\"\n"));
        let lists = edited(
            "",
            &[edit(
                &["language", "dictionaries"],
                Some(Written::List(vec!["~/d".into()])),
            )],
        );
        assert_eq!(
            lists.as_deref(),
            Ok("[language]\ndictionaries = [\"~/d\"]\n")
        );
        assert!(edited("[view\nui_scale = 1", &[]).is_err());
        let directory =
            std::env::temp_dir().join(format!("zetwerk-config-write-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let path = directory.join("zetwerk/config.toml");
        let written = write(
            &path,
            &[edit(&["typing", "dollar"], Some(Written::Bool(false)))],
        );
        assert_eq!(written, Ok(()));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap_or_default(),
            "[typing]\ndollar = false\n"
        );
        std::fs::write(&path, "[view\n").unwrap_or_default();
        assert!(write(&path, &[edit(&["typing", "dollar"], None)]).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap_or_default(),
            "[view\n"
        );
        let _ = std::fs::remove_dir_all(directory);
    }
}
