//! Typst integration for zetwerk: the world documents are compiled in,
//! the compile entry point, and the mapping between pages and source.

mod compile;
mod diagnostics;
mod locate;
mod pagemap;
mod service;
mod world;

pub use compile::{Compiled, compile, forget_unused};
pub use diagnostics::describe;
pub use locate::{PagePosition, locate};
pub use pagemap::{Atomic, MappedGlyph, Origin, PageMap, TextRun};
pub use service::{CompileOutput, Compiler, Scheduler, Urgency};
pub use world::{Environment, FontSet, OpenError, Project, ProjectWorld};
