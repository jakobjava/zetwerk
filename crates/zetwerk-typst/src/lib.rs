//! Typst integration for zetwerk: the world documents are compiled in,
//! the compile entry point, and the mapping between pages and source.

mod compile;
mod diagnostics;
mod pagemap;
mod world;

pub use compile::{Compiled, compile};
pub use diagnostics::describe;
pub use pagemap::{Atomic, MappedGlyph, Origin, PageMap, TextRun};
pub use world::{Environment, FontSet, OpenError, Project, ProjectWorld};
