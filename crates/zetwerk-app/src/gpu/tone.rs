//! The colours of the pages on screen: as the document has them, light
//! on dark for the night, or on sepia paper.
//!
//! Nothing is rasterized anew for it. The compositor maps the colour of
//! every pixel that lies on a page through one affine map (see
//! [`Tone::map`]), in the shader, as it draws: what a page costs to show
//! stays the same. The map inverts the lightness and keeps the hue, so a
//! red heading is a light red at night, not cyan; and since it is affine
//! it commutes with blending, so what is laid over a page (the cursor, a
//! selection, a tint) comes out as the page with it would: an overlay
//! made for white paper stays as readable on the dark one. Pictures keep
//! their colours (a negative of a photograph is no picture of anything),
//! a little darker at night.
//!
//! What is exported, and the pages agents are shown, are not drawn here:
//! they stay as the document is.

use std::ops::Range;

/// How the pages are coloured on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum Tone {
    /// As the document has them.
    #[default]
    Day,
    /// Light on dark.
    Night,
    /// Dark brown on warm paper.
    Sepia,
}

/// White paper and black ink become these, at night and on sepia.
const NIGHT_PAPER: [u8; 3] = [0x26, 0x28, 0x2b];
const NIGHT_INK: [u8; 3] = [0xd9, 0xd4, 0xc7];
const SEPIA_PAPER: [u8; 3] = [0xf3, 0xea, 0xd6];
const SEPIA_INK: [u8; 3] = [0x3b, 0x2f, 0x22];
/// How much of how coloured a colour is stays: a little less than all,
/// or light colours on dark paper glare.
const NIGHT_CHROMA: f32 = 0.8;
const SEPIA_CHROMA: f32 = 0.85;
/// What the colours of pictures are multiplied by at night.
const NIGHT_PICTURES: f32 = 0.85;
/// How much each of red, green and blue counts for how light a colour
/// is (Rec. 709).
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// How many pages, and how many pictures, a frame tones at most: more
/// pictures than that are toned as the rest of the page.
pub const MOST_PAGES: usize = 16;
pub const MOST_PICTURES: usize = 16;

/// What of a frame is paper: which quads are drawn on the pages, where
/// the pages and the pictures on them are, and how they are coloured.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Paper {
    pub tone: Tone,
    /// The quads, by their place in the frame's list, that are drawn on
    /// pages: where they lie on a page, they take its tone. What is
    /// drawn over the pages for the interface (panels, the status line)
    /// is not among them.
    pub quads: Vec<Range<usize>>,
    /// The pages on screen, as x, y, width and height in pixels.
    pub pages: Vec<[f32; 4]>,
    /// The pictures on them: they keep their colours.
    pub pictures: Vec<[f32; 4]>,
}

impl Paper {
    /// Whether the `index`th quad of the frame takes the tone.
    #[must_use]
    pub fn tones(&self, index: usize) -> bool {
        self.tone != Tone::Day && self.quads.iter().any(|range| range.contains(&index))
    }
}

impl Tone {
    /// The map of a straight (not premultiplied) colour, channels from 0
    /// to 1: its rows, each applied to the colour, and what is added.
    /// `None` for the day, which maps nothing.
    #[must_use]
    pub fn map(self) -> Option<([[f32; 3]; 3], [f32; 3])> {
        let unit = |rgb: [u8; 3]| rgb.map(|c| f32::from(c) / 255.0);
        // What is light (Y = 1) becomes `paper`, what is dark (Y = 0)
        // `ink`, and how coloured it is stays, mostly: c' = ink +
        // (paper − ink)·Y + k·(c − Y), where Y is luma·c. That is
        // c' = A·c + ink with A = k·I + (paper − ink − k)·lumaᵀ. At night
        // the paper is the dark one, which inverts the lightness.
        let (paper, ink, chroma) = match self {
            Self::Day => return None,
            Self::Night => (NIGHT_PAPER, NIGHT_INK, NIGHT_CHROMA),
            Self::Sepia => (SEPIA_PAPER, SEPIA_INK, SEPIA_CHROMA),
        };
        let (paper, ink) = (unit(paper), unit(ink));
        let mut rows = [[0.0; 3]; 3];
        for (channel, row) in rows.iter_mut().enumerate() {
            let along = paper[channel] - ink[channel] - chroma;
            for (other, value) in row.iter_mut().enumerate() {
                *value = along * LUMA[other] + if other == channel { chroma } else { 0.0 };
            }
        }
        Some((rows, ink))
    }

    /// `rgba` (straight, not premultiplied) as it looks on a page of this
    /// tone, outside pictures: what the compositor makes of it. For
    /// whatever is drawn on a page but not among [`Paper::quads`], so as
    /// to match what is.
    #[must_use]
    pub fn apply(self, rgba: [u8; 4]) -> [u8; 4] {
        let Some((rows, add)) = self.map() else {
            return rgba;
        };
        let colour = [rgba[0], rgba[1], rgba[2]].map(|c| f32::from(c) / 255.0);
        let mut out = rgba;
        for (channel, row) in rows.iter().enumerate() {
            let value = row[0] * colour[0] + row[1] * colour[1] + row[2] * colour[2] + add[channel];
            out[channel] = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
        out
    }

    /// What the colours of pictures are multiplied by.
    #[must_use]
    pub fn pictures(self) -> f32 {
        match self {
            Self::Night => NIGHT_PICTURES,
            Self::Day | Self::Sepia => 1.0,
        }
    }
}

/// The uniform the shader reads: the map (three rows and what is added),
/// then how many pages and pictures there are, whether to map at all and
/// what pictures are multiplied by, then the pages and the pictures. All
/// of it vectors of four, as uniforms are laid out.
#[must_use]
pub(super) fn uniform(paper: Option<&Paper>) -> Vec<f32> {
    let mut out = Vec::with_capacity(4 * (5 + MOST_PAGES + MOST_PICTURES));
    let map = paper.and_then(|paper| paper.tone.map());
    let (rows, add) = map.unwrap_or((
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        [0.0; 3],
    ));
    for row in rows {
        out.extend([row[0], row[1], row[2], 0.0]);
    }
    out.extend([add[0], add[1], add[2], 0.0]);
    let (pages, pictures) = paper.map_or((&[][..], &[][..]), |paper| {
        let pages = &paper.pages[..paper.pages.len().min(MOST_PAGES)];
        let pictures = &paper.pictures[..paper.pictures.len().min(MOST_PICTURES)];
        (pages, pictures)
    });
    let on = if map.is_some() { 1.0 } else { 0.0 };
    let dim = paper.map_or(1.0, |paper| paper.tone.pictures());
    out.extend([pages.len() as f32, pictures.len() as f32, on, dim]);
    for (list, most) in [(pages, MOST_PAGES), (pictures, MOST_PICTURES)] {
        for rect in list {
            out.extend(rect);
        }
        out.extend(std::iter::repeat_n(0.0, 4 * (most - list.len())));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_becomes_the_paper_and_black_the_ink() {
        let white = [0xff, 0xff, 0xff, 0xff];
        let black = [0, 0, 0, 0xff];
        let rgb = |rgba: [u8; 4]| [rgba[0], rgba[1], rgba[2]];
        assert_eq!(rgb(Tone::Night.apply(white)), NIGHT_PAPER);
        assert_eq!(rgb(Tone::Night.apply(black)), NIGHT_INK);
        assert_eq!(rgb(Tone::Sepia.apply(white)), SEPIA_PAPER);
        assert_eq!(rgb(Tone::Sepia.apply(black)), SEPIA_INK);
        assert_eq!(Tone::Day.apply(white), white);
        // How opaque a colour is stays.
        assert_eq!(Tone::Night.apply([0, 0, 0, 0x40])[3], 0x40);
    }

    #[test]
    fn colours_keep_their_hue_at_night() {
        // A red heading is a light red, a blue link a light blue: not
        // what an inversion of each channel makes of them (cyan, yellow).
        let red = Tone::Night.apply([0xcc, 0x24, 0x1d, 0xff]);
        assert!(red[0] > red[1] + 40 && red[0] > red[2] + 40, "{red:?}");
        assert!(red[0] > 0xc0, "light: {red:?}");
        let blue = Tone::Night.apply([0x07, 0x66, 0x78, 0xff]);
        assert!(blue[2] > blue[0] + 30, "{blue:?}");
        // Grey is grey, lighter where it was darker.
        let dark = Tone::Night.apply([0x40, 0x40, 0x40, 0xff]);
        let light = Tone::Night.apply([0xc0, 0xc0, 0xc0, 0xff]);
        assert!(dark[0] > light[0]);
    }

    #[test]
    fn the_uniform_is_as_long_as_the_shader_reads_it() {
        let paper = Paper {
            tone: Tone::Night,
            quads: vec![0..3],
            pages: vec![[0.0, 0.0, 10.0, 10.0]; 20],
            pictures: vec![[1.0, 1.0, 2.0, 2.0]],
        };
        let made = uniform(Some(&paper));
        assert_eq!(made.len(), 4 * (5 + MOST_PAGES + MOST_PICTURES));
        // At most as many pages as there is room for.
        assert_eq!(made[16..20], [16.0, 1.0, 1.0, NIGHT_PICTURES]);
        assert_eq!(uniform(None)[16..20], [0.0, 0.0, 0.0, 1.0]);
        assert!(paper.tones(2) && !paper.tones(3));
    }
}
