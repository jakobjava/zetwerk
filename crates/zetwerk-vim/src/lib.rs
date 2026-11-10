//! Vim's modal editing grammar.
//!
//! This crate turns key presses into edits and cursor movements. It knows
//! nothing about windows, files or Typst: text is reached through the
//! [`Host`] trait, so the whole grammar can be tested against Neovim on
//! plain strings.

mod engine;
pub mod keys;
mod motion;
mod object;
mod text;

pub use engine::{Edit, Effect, Engine, Host, Mode};
