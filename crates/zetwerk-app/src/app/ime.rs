//! Keys and input methods, from winit's events to the viewer.
//!
//! Text reaches the window in three ways: a key that types a character
//! (`KeyEvent::text`, with dead keys, compose and `AltGr` already applied),
//! a key that types several (some compose sequences, input tools that
//! send words as keys), and what an input method commits
//! (`Ime::Commit`). The first is a key like any other; the other two are
//! text typed, which [`Viewer::text_typed`] types as keys. While an input
//! method composes, the viewer shows what it composes
//! ([`Viewer::preedit`]) and Escape gives that up, not insert mode.
//!
//! An input method is let work only where text is typed (see
//! [`Viewer::wants_input_method`]): it is turned off when insert mode is
//! left and on when it is entered again, as Vim's plugins for input
//! methods do, so that `j` in normal mode moves whatever the input method
//! would make of it. [`Ime::sync`] says what the window is to be told.
//!
//! On a layout of another script (Russian, Greek), a key in normal mode
//! that types a letter of that script is the key in its place on a US
//! keyboard ([`by_position`]): `о` on the key of `j` is `j`. Latin letters
//! are never changed, so German, French or Dvorak layouts keep their own
//! places for Vim's letters.

use winit::event::{Ime as ImeEvent, KeyEvent};
use winit::keyboard::{Key as WinitKey, KeyCode, ModifiersState, NamedKey, PhysicalKey};

use super::{Session, alt_held, shifted, translate};
use crate::keys::{Key, KeyPress};
use crate::scripts;
use crate::viewer::Viewer;

/// What the window has told the input method last.
#[derive(Debug, Default)]
pub(crate) struct Ime {
    allowed: Option<bool>,
    area: Option<(i64, i64, i64, i64)>,
}

/// Something to tell the window about input methods.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Call {
    /// Let an input method work, or not. Turning one off and on again
    /// makes it drop what it composes.
    Allow(bool),
    /// Where the cursor is, for the input method's list of candidates:
    /// `(x, y, width, height)` in device pixels.
    Area(f64, f64, f64, f64),
}

impl Ime {
    /// What the window is to be told so that the input method works where
    /// and only where the viewer takes text, drops what it composes where
    /// the viewer has given that up, and shows its candidates at the
    /// cursor. Asked after every frame: nothing when nothing changed.
    pub(crate) fn sync(&mut self, viewer: &mut Viewer) -> Vec<Call> {
        let mut calls = Vec::new();
        let wanted = viewer.wants_input_method();
        if viewer.take_input_method_reset() && self.allowed == Some(true) {
            calls.push(Call::Allow(false));
            self.allowed = Some(false);
        }
        if self.allowed != Some(wanted) {
            calls.push(Call::Allow(wanted));
            self.allowed = Some(wanted);
            // Turned on, it is to be told where again.
            self.area = None;
        }
        if wanted && let Some((x, y, width, height)) = viewer.input_area() {
            #[allow(clippy::cast_possible_truncation, reason = "pixels")]
            let rounded = (x as i64, y as i64, width as i64, height as i64);
            if self.area != Some(rounded) {
                self.area = Some(rounded);
                calls.push(Call::Area(x, y, width, height));
            }
        }
        calls
    }
}

/// An input method's event.
pub(crate) fn event(viewer: &mut Viewer, event: ImeEvent) {
    match event {
        ImeEvent::Enabled => viewer.input_method(true),
        ImeEvent::Disabled => viewer.input_method(false),
        ImeEvent::Preedit(text, cursor) => viewer.preedit(&text, cursor),
        // Late, after the window has left typing (an input method that
        // commits what it composed when it is turned off): not to be
        // taken for commands.
        ImeEvent::Commit(text) if !viewer.wants_input_method() => {
            viewer.preedit("", None);
            let _ = text;
        }
        ImeEvent::Commit(text) => viewer.text_typed(&text),
    }
}

/// A key pressed, as much of it as is needed here.
pub(crate) struct Pressed<'a> {
    pub key: &'a WinitKey,
    /// Where the key is on the keyboard.
    pub code: PhysicalKey,
    /// What it typed, if it typed something.
    pub text: Option<&'a str>,
    pub modifiers: ModifiersState,
}

impl<'a> Pressed<'a> {
    pub(crate) fn of(event: &'a KeyEvent, modifiers: ModifiersState) -> Self {
        Self {
            key: &event.logical_key,
            code: event.physical_key,
            text: event.text.as_deref(),
            modifiers,
        }
    }
}

/// A key was pressed.
pub(crate) fn keyboard(viewer: &mut Viewer, pressed: &Pressed) {
    let ctrl = pressed.modifiers.control_key();
    // Escape while an input method composes gives that up, and stays in
    // insert mode. (Most input methods take Escape themselves then.)
    if viewer.composing() && *pressed.key == WinitKey::Named(NamedKey::Escape) {
        viewer.end_composition(true);
        return;
    }
    // Several characters from one key: text typed.
    if let Some(text) = pressed.text.filter(|_| !ctrl)
        && text.chars().nth(1).is_some()
        && !text.chars().any(char::is_control)
        && matches!(pressed.key, WinitKey::Character(_) | WinitKey::Dead(_))
    {
        viewer.text_typed(text);
        return;
    }
    let Some(key) = translate(pressed.key, pressed.text, ctrl) else {
        return;
    };
    let key = shifted(key, pressed.modifiers.shift_key());
    let alt = alt_held(
        pressed.text,
        ctrl,
        pressed.modifiers.alt_key(),
        cfg!(target_os = "macos"),
    );
    let press = KeyPress { key, ctrl, alt };
    viewer.key(by_position(viewer, press, pressed.code));
}

/// `press`, or where it types a letter of another script than the Latin
/// and the viewer wants keys by their places (see
/// [`Viewer::by_position`]), the key in its place on a US keyboard.
pub(crate) fn by_position(viewer: &Viewer, press: KeyPress, code: PhysicalKey) -> KeyPress {
    let Key::Char(c) = press.key else {
        return press;
    };
    if !scripts::is_other_script(c) || !viewer.by_position(press) {
        return press;
    }
    // Shift (or Caps Lock) shows in the letter typed.
    match us_key(code, c.is_uppercase()) {
        Some(us) => KeyPress {
            key: Key::Char(us),
            ..press
        },
        None => press,
    }
}

/// The character the key at `code` types on a US keyboard, with Shift if
/// `shift`; for the keys that have letters on other layouts.
fn us_key(code: PhysicalKey, shift: bool) -> Option<char> {
    let PhysicalKey::Code(code) = code else {
        return None;
    };
    let (plain, shifted) = match code {
        KeyCode::KeyA => ('a', 'A'),
        KeyCode::KeyB => ('b', 'B'),
        KeyCode::KeyC => ('c', 'C'),
        KeyCode::KeyD => ('d', 'D'),
        KeyCode::KeyE => ('e', 'E'),
        KeyCode::KeyF => ('f', 'F'),
        KeyCode::KeyG => ('g', 'G'),
        KeyCode::KeyH => ('h', 'H'),
        KeyCode::KeyI => ('i', 'I'),
        KeyCode::KeyJ => ('j', 'J'),
        KeyCode::KeyK => ('k', 'K'),
        KeyCode::KeyL => ('l', 'L'),
        KeyCode::KeyM => ('m', 'M'),
        KeyCode::KeyN => ('n', 'N'),
        KeyCode::KeyO => ('o', 'O'),
        KeyCode::KeyP => ('p', 'P'),
        KeyCode::KeyQ => ('q', 'Q'),
        KeyCode::KeyR => ('r', 'R'),
        KeyCode::KeyS => ('s', 'S'),
        KeyCode::KeyT => ('t', 'T'),
        KeyCode::KeyU => ('u', 'U'),
        KeyCode::KeyV => ('v', 'V'),
        KeyCode::KeyW => ('w', 'W'),
        KeyCode::KeyX => ('x', 'X'),
        KeyCode::KeyY => ('y', 'Y'),
        KeyCode::KeyZ => ('z', 'Z'),
        KeyCode::BracketLeft => ('[', '{'),
        KeyCode::BracketRight => (']', '}'),
        KeyCode::Semicolon => (';', ':'),
        KeyCode::Quote => ('\'', '"'),
        KeyCode::Comma => (',', '<'),
        KeyCode::Period => ('.', '>'),
        KeyCode::Slash => ('/', '?'),
        KeyCode::Backquote => ('`', '~'),
        _ => return None,
    };
    Some(if shift { shifted } else { plain })
}

impl Session {
    /// Tells the window what [`Ime::sync`] says it is to be told.
    pub(super) fn sync_input_method(&mut self) {
        for call in self.ime.sync(&mut self.viewer) {
            match call {
                Call::Allow(allowed) => self.window.set_ime_allowed(allowed),
                Call::Area(x, y, width, height) => self.window.set_ime_cursor_area(
                    winit::dpi::PhysicalPosition::new(x, y),
                    winit::dpi::PhysicalSize::new(width, height),
                ),
            }
        }
    }
}
