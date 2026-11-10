//! Measurements behind zetwerk's design decisions (milestone M0).
//!
//! Run in release mode: `cargo run --release -p zetwerk-spikes -- <spike>`.
//! Everything uses the embedded fonts, so results are comparable between
//! machines apart from raw speed.

mod compile;
mod pagemap;
mod predict;
mod raster;
mod support;

use std::process::ExitCode;

const USAGE: &str = "\
Usage: zetwerk-spikes <spike>

Spikes:
  compile   Incremental compile latency and CPU time per single-character edit
  raster    Full-page versus region rasterization time
  pagemap   Share of glyphs that are editable on the page
  predict   Accuracy of predicting a keystroke instead of compiling it
  all       Run every spike

  corpus <directory>
            Write the test documents as files, to open them in zetwerk";

fn main() -> ExitCode {
    let spike = std::env::args().nth(1);
    match spike.as_deref() {
        Some("compile") => compile::run(),
        Some("raster") => raster::run(),
        Some("pagemap") => pagemap::run(),
        Some("predict") => predict::run(),
        Some("corpus") => {
            let Some(directory) = std::env::args_os().nth(2) else {
                eprintln!("{USAGE}");
                return ExitCode::FAILURE;
            };
            if let Err(err) = write_corpus(std::path::Path::new(&directory)) {
                eprintln!("cannot write the corpus: {err}");
                return ExitCode::FAILURE;
            }
        }
        Some("all") => {
            compile::run();
            raster::run();
            pagemap::run();
            predict::run();
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

fn write_corpus(directory: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;
    let documents = [
        ("paper.typ", zetwerk_corpus::PAPER.to_owned()),
        ("article.typ", zetwerk_corpus::ARTICLE.to_owned()),
        ("slides.typ", zetwerk_corpus::SLIDES.to_owned()),
        ("thesis-25.typ", zetwerk_corpus::thesis(25)),
        ("thesis-60.typ", zetwerk_corpus::thesis(60)),
    ];
    for (name, text) in documents {
        let path = directory.join(name);
        std::fs::write(&path, text)?;
        println!("{}", path.display());
    }
    Ok(())
}
