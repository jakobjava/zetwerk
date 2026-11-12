//! Typst integration for zetwerk: the world documents are compiled in,
//! the compile entry point, and the mapping between pages and source.

mod compile;
mod diagnostics;
mod ide;
mod locate;
mod pagemap;
mod service;
mod world;

pub use compile::{Compiled, compile, export_pdf, forget_unused};
pub use diagnostics::{describe, place};
pub use ide::{Candidate, complete, definition_of, describe_at};
pub use locate::{PagePosition, locate};
pub use pagemap::{Atomic, Caret, MappedGlyph, Origin, PageLines, PageMap, TextRun};
pub use service::{CompileOutput, Compiler, Scheduler, Urgency};
pub use world::{Environment, FontSet, OpenError, Project, ProjectWorld};
