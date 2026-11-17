//! What script a character is of, as far as typing needs to know: whether
//! a layout that typed it has Vim's letters in other places (Greek,
//! Cyrillic), and whether it is text or a symbol in an equation.

/// Whether `c` is a letter of the Latin script (with its accents and
/// ligatures), the script Vim's commands are written in.
#[must_use]
pub fn is_latin(c: char) -> bool {
    matches!(
        c,
        'a'..='z'
            | 'A'..='Z'
            | '\u{aa}'
            | '\u{ba}'
            | '\u{c0}'..='\u{24f}'
            | '\u{250}'..='\u{2af}'
            | '\u{1d00}'..='\u{1dbf}'
            | '\u{1e00}'..='\u{1eff}'
            | '\u{2c60}'..='\u{2c7f}'
            | '\u{a720}'..='\u{a7ff}'
            | '\u{ab30}'..='\u{ab6f}'
            | '\u{fb00}'..='\u{fb06}'
            | '\u{ff21}'..='\u{ff3a}'
            | '\u{ff41}'..='\u{ff5a}'
    )
}

/// Whether `c` is a Greek letter: in an equation, a symbol like a Latin
/// one.
#[must_use]
pub fn is_greek(c: char) -> bool {
    matches!(c, '\u{370}'..='\u{3ff}' | '\u{1f00}'..='\u{1fff}')
}

/// Whether `c` is a letter that a keyboard with Vim's letters elsewhere
/// types where Vim's are: one of another script than the Latin. On a
/// Russian layout the key of `j` types `о`; on a German one `ö` is no
/// letter of Vim's anywhere, and the keys of `y` and `z` are Vim's `y`
/// and `z` wherever they are.
#[must_use]
pub fn is_other_script(c: char) -> bool {
    c.is_alphabetic() && !is_latin(c)
}

/// Whether `c` is a letter that is text in an equation, not a symbol: a
/// letter of neither the Latin nor the Greek script (a Chinese character,
/// a Hebrew or a Cyrillic letter). Typst reads two of them together as
/// a name, so they go in quotes.
#[must_use]
pub fn is_text_in_math(c: char) -> bool {
    c.is_alphabetic() && !is_latin(c) && !is_greek(c) && !is_math_letter(c)
}

/// Letters that are symbols of mathematics although of no Latin or Greek
/// block: the letterlike symbols (ℵ, ℏ, ℓ, the cardinals' aleph) and the
/// mathematical alphabets.
fn is_math_letter(c: char) -> bool {
    matches!(c, '\u{2100}'..='\u{214f}' | '\u{1d400}'..='\u{1d7ff}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_as_typing_sees_them() {
        // Latin letters with their accents are Vim's, wherever they are.
        for c in ['j', 'ö', 'ß', 'é', 'ñ', 'ø', 'ĳ', 'ẞ'] {
            assert!(is_latin(c) && !is_other_script(c), "{c}");
        }
        // Greek, Cyrillic, Hebrew and Arabic letters are not.
        for c in ['ξ', 'о', 'Ж', 'ש', 'ب'] {
            assert!(is_other_script(c), "{c}");
        }
        // Signs are not letters at all.
        for c in [';', '§', '°', '€'] {
            assert!(!is_other_script(c), "{c}");
        }
        // In an equation, a Greek letter is a symbol; a Chinese character
        // or a Cyrillic letter is text.
        assert!(!is_text_in_math('α') && !is_text_in_math('x') && !is_text_in_math('ℵ'));
        assert!(is_text_in_math('中') && is_text_in_math('ж') && is_text_in_math('か'));
        assert!(!is_text_in_math('1') && !is_text_in_math('+'));
    }
}
