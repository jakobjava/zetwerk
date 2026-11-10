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
  all       Run every spike";

fn main() -> ExitCode {
    let spike = std::env::args().nth(1);
    match spike.as_deref() {
        Some("compile") => compile::run(),
        Some("raster") => raster::run(),
        Some("pagemap") => pagemap::run(),
        Some("predict") => predict::run(),
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
