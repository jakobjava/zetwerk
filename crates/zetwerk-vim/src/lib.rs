//! Vim's modal editing grammar.
//!
//! This crate turns key presses into edits and cursor movements. It knows
//! nothing about windows, files or Typst: text is reached through the
//! [`Host`] trait, so the whole grammar can be tested against Neovim on
//! plain strings.

mod engine;
pub mod find;
pub mod keys;
mod motion;
mod object;
pub mod pattern;
mod text;
mod took;

pub use engine::{Edit, Effect, Engine, Format, Host, Mode, Place, Seek, Shown};
pub use engine::{Following, keys_after};
pub use find::{Query, Reading};
pub use took::Took;
