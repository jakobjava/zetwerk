//! Typst integration for zetwerk: the world documents are compiled in,
//! the compile entry point, and the mapping between pages and source.

mod compile;
mod diagnostics;
mod figure;
mod ide;
pub mod local;
mod locate;
mod pagemap;
mod service;
mod world;

pub use compile::{Compiled, compile, export_pdf, export_pdf_pages, forget_unused};
pub use diagnostics::{describe, place};
pub use figure::{FigureArea, figure_area};
pub use ide::{Candidate, bibliography_keys, complete, definition_of, describe_at};
pub use locate::{PagePosition, locate, locate_near};
pub use pagemap::{Atomic, Caret, MappedGlyph, Origin, PageLines, PageMap, TextRun};
pub use service::{Bounds, CompileOutput, Compiler, Scheduler, Urgency, compile_within};
pub use world::{Environment, FontSet, OpenError, Project, ProjectWorld, find_main};
