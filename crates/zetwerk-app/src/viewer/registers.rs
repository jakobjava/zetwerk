//! The register `"+`: the system clipboard, as in Vim.
//!
//! What is yanked or deleted into `"+` is put into the clipboard, for
//! other programs to paste; `"+p` puts in what the clipboard holds, also
//! what another program copied. The clipboard is read when `"+` is typed
//! and written when the register has changed, never otherwise: no key
//! but these touches it.

use super::Viewer;
use crate::editor::Editor;
use crate::keys::{Key, KeyPress};

/// The register that stands for the clipboard.
const CLIPBOARD: char = '+';

/// How the clipboard is read and written: the system's, or in a test
/// one given; and what was last read from it or written to it.
pub(super) struct Clipboard {
    pub(super) read: fn() -> Result<String, String>,
    pub(super) write: fn(&str) -> Result<(), String>,
    known: Option<String>,
    /// Whether the last key was Ctrl-R while typing, which asks for the
    /// name of a register next.
    ctrl_r: bool,
}

impl Default for Clipboard {
    fn default() -> Self {
        Self {
            read: crate::clipboard::text,
            write: crate::clipboard::write,
            known: None,
            ctrl_r: false,
        }
    }
}

/// Whether `press` names the register of the clipboard: `+` behind `"`,
/// or behind Ctrl-R while typing.
pub(super) fn asks_clipboard(clipboard: &mut Clipboard, editor: &Editor, press: KeyPress) -> bool {
    let typing = editor.mode() == crate::editor::Mode::Insert;
    let ctrl_r = typing && press.ctrl && press.key == Key::Char('r');
    let after_ctrl_r = std::mem::replace(&mut clipboard.ctrl_r, ctrl_r) && typing;
    press.key == Key::Char(CLIPBOARD)
        && !press.ctrl
        && (after_ctrl_r
            || editor
                .following()
                .is_some_and(|following| following.typed == "\""))
}

impl Viewer {
    /// `"+` was typed: the register is made to hold what the clipboard
    /// does, for a put that may follow.
    pub(super) fn clipboard_to_register(&mut self) {
        match (self.clipboard.read)() {
            Ok(text) => {
                if let Some(editor) = &mut self.editor {
                    editor.set_register(CLIPBOARD, &text);
                }
                self.clipboard.known = Some(text);
            }
            Err(why) => self.alert(why),
        }
    }

    /// Notes that the clipboard holds `text`, written there by other
    /// means than a key: not to be written again.
    pub(super) fn clipboard_known(&mut self, text: &str) {
        self.clipboard.known = Some(text.to_owned());
    }

    /// After a key: what the key put into `"+` goes into the clipboard.
    pub(super) fn register_to_clipboard(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        let Some((text, _)) = editor.engine().register(CLIPBOARD) else {
            return;
        };
        if self.clipboard.known.as_deref() == Some(text) {
            return;
        }
        let text = text.to_owned();
        let written = (self.clipboard.write)(&text);
        self.clipboard.known = Some(text);
        if let Err(why) = written {
            self.alert(why);
        }
    }
}
