//! Shaping a text run the way Typst does, as far as an outsider can.
//!
//! A text item on the page records its font, size, language and glyphs, but
//! not the OpenType features or spacing rules that produced them. The
//! predictor therefore shapes with defaults and *checks*: if shaping an
//! item's current text does not reproduce the item's current glyphs, the item
//! was produced with settings we cannot see, and the predictor declines.

use std::ops::Range;
use std::str::FromStr;

use rustybuzz::{BufferFlags, Direction, UnicodeBuffer};
use typst::layout::Em;
use typst::text::{Glyph, TextItem};

/// One glyph as the shaper produced it, before justification or tracking.
#[derive(Debug, Clone, PartialEq)]
pub struct Shaped {
    pub id: u16,
    pub x_advance: Em,
    pub x_offset: Em,
    pub y_offset: Em,
    /// The bytes of the shaped text this glyph stands for. Glyphs of one
    /// cluster all carry the cluster's whole range, as in Typst.
    pub range: Range<usize>,
}

/// Shapes `text` left-to-right in the font and language of `item`, with
/// default OpenType features.
pub fn shape(item: &TextItem, text: &str) -> Vec<Shaped> {
    let mut buffer = UnicodeBuffer::new();
    buffer.push_str(text);
    let mut language = item.lang.as_str().to_owned();
    if let Some(region) = item.region {
        language.push('-');
        language.push_str(region.as_str());
    }
    if let Ok(language) = rustybuzz::Language::from_str(&language) {
        buffer.set_language(language);
    }
    buffer.set_direction(Direction::LeftToRight);
    buffer.guess_segment_properties();
    buffer.set_flags(BufferFlags::REMOVE_DEFAULT_IGNORABLES);

    let font = &item.font;
    let buffer = rustybuzz::shape(font.rusty(), &[], buffer);
    let infos = buffer.glyph_infos();
    let positions = buffer.glyph_positions();

    infos
        .iter()
        .zip(positions)
        .enumerate()
        .map(|(index, (info, position))| {
            let start = info.cluster as usize;
            // The range ends where the next cluster starts.
            let end = infos[index + 1..]
                .iter()
                .map(|next| next.cluster as usize)
                .find(|&cluster| cluster != start)
                .unwrap_or(text.len());
            Shaped {
                // Typst stores glyph ids as `u16`; fonts cannot have more.
                id: u16::try_from(info.glyph_id).unwrap_or(0),
                x_advance: font.to_em(position.x_advance),
                x_offset: font.to_em(position.x_offset),
                y_offset: font.to_em(position.y_offset),
                range: start..end,
            }
        })
        .collect()
}

/// Whether shaping an item's own text gave back the item's own `glyphs`, in
/// identity and text coverage. Advances are allowed to differ: that is where
/// justification and tracking live.
pub fn reproduces(glyphs: &[Glyph], shaped: &[Shaped]) -> bool {
    shaped.len() == glyphs.len()
        && shaped
            .iter()
            .zip(glyphs)
            .all(|(ours, theirs)| ours.id == theirs.id && ours.range == theirs.range())
}
