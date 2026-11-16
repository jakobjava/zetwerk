//! Puts the lessons of `:tutor` into the program: every Typst file under
//! `lessons/<language>/`, in the order of their names, and the files of
//! `lessons/shared/` that every lesson has beside it. A lesson is added
//! by writing its file (see `viewer::tutor`).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The files of `directory`, by name.
fn files(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    found.sort();
    found
}

fn main() {
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default();
    let root = Path::new(&manifest).join("lessons");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut lessons = String::new();
    let mut shared = String::new();
    for directory in files(&root).into_iter().filter(|path| path.is_dir()) {
        let language = directory
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        for file in files(&directory).into_iter().filter(|path| path.is_file()) {
            let name = file.file_name().unwrap_or_default().to_string_lossy();
            let path = file.display();
            if language == "shared" {
                let _ = writeln!(
                    shared,
                    "    ({name:?}, include_str!({:?})),",
                    path.to_string()
                );
            } else if let Some(stem) = name.strip_suffix(".typ") {
                let _ = writeln!(
                    lessons,
                    "    ({language:?}, {stem:?}, include_str!({:?})),",
                    path.to_string()
                );
            }
        }
    }
    let code = format!(
        "/// The lessons: their language, their name, and their text.\n\
         pub(super) const LESSON_FILES: &[(&str, &str, &str)] = &[\n{lessons}];\n\n\
         /// What every lesson has beside it: names and texts.\n\
         pub(super) const SHARED_FILES: &[(&str, &str)] = &[\n{shared}];\n"
    );
    let out = std::env::var_os("OUT_DIR").unwrap_or_default();
    if let Err(err) = std::fs::write(Path::new(&out).join("lessons.rs"), code) {
        println!("cargo:warning=cannot write the list of lessons: {err}");
    }
}
