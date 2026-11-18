//! An image pasted into a document becomes a file beside it: where it
//! goes, what it is called, how large it is, and a smaller one for a
//! screenshot too large for any page.
//!
//! It goes where the document's images are (the folder most of its
//! `image("…")` calls name; else the document's own), which is where the
//! list of files of the figure insert finds it. It is called after the
//! nearest heading or caption above it, else after the time; a file
//! that is there is never written over.

use std::path::{Path, PathBuf};

/// A screenshot whose longest side is longer than this is offered
/// smaller: a page of text is 16 cm wide, and 2000 pixels over it is
/// more than 300 to the inch, as much as any printer makes.
pub const LARGEST: u32 = 2400;
/// And made so long.
pub const SMALLER: u32 = 2000;

/// `words` as part of the name of a file: lower case letters and
/// digits, apart by `-`, not too long.
#[must_use]
pub fn slug(words: &str) -> String {
    let mut slug = String::new();
    for word in words
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        let word: String = word
            .to_lowercase()
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect();
        if word.is_empty() || slug.len() + word.len() > 40 {
            continue;
        }
        if !slug.is_empty() {
            slug.push('-');
        }
        slug.push_str(&word);
    }
    slug
}

/// What to call an image put in at byte `at` of `text`: after the
/// nearest heading or caption above it that has words.
#[must_use]
pub fn name_near(text: &str, at: usize) -> Option<String> {
    let before = text.get(..at.min(text.len()))?;
    before.lines().rev().find_map(|line| {
        let line = line.trim();
        let words = if let Some(heading) = line.strip_prefix('=') {
            heading.trim_start_matches('=')
        } else {
            let caption = &line[line.find("caption: [")? + "caption: [".len()..];
            &caption[..caption.find(']').unwrap_or(caption.len())]
        };
        let slug = slug(words);
        (!slug.is_empty()).then_some(slug)
    })
}

/// A name after the time: `pasted-2026-11-16-153012` (UTC).
#[must_use]
pub fn stamp(now: std::time::SystemTime) -> String {
    let seconds = now
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    // Days since 1970 as a date (Howard Hinnant's way).
    let shifted = days.cast_signed() + 719_468;
    let era = shifted.div_euclid(146_097);
    let of_era = shifted.rem_euclid(146_097);
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let day_of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "pasted-{year:04}-{month:02}-{day:02}-{:02}{:02}{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// The folder images of `text` are in, as a path from the document's
/// folder: the one most of its `image("…")` calls name (where it is
/// there in `folder`), else the document's own (empty).
#[must_use]
pub fn images_folder(text: &str, folder: &Path) -> PathBuf {
    let mut counts: Vec<(PathBuf, usize)> = Vec::new();
    for (at, _) in text.match_indices("image(\"") {
        let rest = &text[at + "image(\"".len()..];
        let Some(end) = rest.find('"') else {
            continue;
        };
        let named = Path::new(&rest[..end]);
        let Some(parent) = named.parent().filter(|parent| !parent.is_absolute()) else {
            continue;
        };
        match counts.iter_mut().find(|(seen, _)| seen == parent) {
            Some((_, count)) => *count += 1,
            None => counts.push((parent.to_path_buf(), 1)),
        }
    }
    counts.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    counts
        .into_iter()
        .map(|(parent, _)| parent)
        .find(|parent| parent.as_os_str().is_empty() || folder.join(parent).is_dir())
        .unwrap_or_default()
}

/// A file in `folder` called `stem` with `ending` that is not there yet:
/// `stem.ending`, else `stem-2.ending` and on.
#[must_use]
pub fn free_path(folder: &Path, stem: &str, ending: &str) -> PathBuf {
    let mut number = 1;
    loop {
        let name = if number == 1 {
            format!("{stem}.{ending}")
        } else {
            format!("{stem}-{number}.{ending}")
        };
        let path = folder.join(name);
        if !path.exists() {
            return path;
        }
        number += 1;
    }
}

/// How many pixels wide and high the image of `bytes` is; `None` for a
/// drawing (SVG) or what is not read.
#[must_use]
pub fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// Whether an image so large is offered smaller.
#[must_use]
pub fn is_large((wide, high): (u32, u32)) -> bool {
    wide.max(high) > LARGEST
}

/// The image of `bytes` made so that its longest side is [`SMALLER`],
/// in the format it was in (PNG, else JPEG).
pub fn smaller(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|err| err.to_string())?;
    let format = reader.format().unwrap_or(image::ImageFormat::Png);
    let decoded = reader.decode().map_err(|err| err.to_string())?;
    let small = decoded.resize(SMALLER, SMALLER, image::imageops::FilterType::Triangle);
    let format = if format == image::ImageFormat::Jpeg {
        image::ImageFormat::Jpeg
    } else {
        image::ImageFormat::Png
    };
    let mut written = std::io::Cursor::new(Vec::new());
    let small = if format == image::ImageFormat::Jpeg {
        image::DynamicImage::ImageRgb8(small.to_rgb8())
    } else {
        small
    };
    small
        .write_to(&mut written, format)
        .map_err(|err| err.to_string())?;
    Ok(written.into_inner())
}

/// A number of bytes as people say it: `245 KB`, `3.1 MB`.
#[must_use]
pub fn size(bytes: usize) -> String {
    match bytes {
        0..1_000 => format!("{bytes} bytes"),
        1_000..1_000_000 => format!("{} KB", bytes.div_ceil(1000)),
        _ => format!("{:.1} MB", bytes as f64 / 1e6),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::*;

    #[test]
    fn a_pasted_image_is_named_after_what_is_above_it_or_the_time() {
        let text = "= Results\n\nSome text.\n\n#figure(image(\"a.png\"), caption: [The Big plot, 2nd])\n\nMore.";
        assert_eq!(
            name_near(text, text.len()).as_deref(),
            Some("the-big-plot-2nd")
        );
        assert_eq!(name_near(text, 20).as_deref(), Some("results"));
        assert_eq!(name_near("Nothing here.", 5), None);
        assert_eq!(slug("Über die Größe: 3 Fälle!"), "ber-die-gre-3-flle");
        let day = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_794_843_012);
        assert_eq!(stamp(day), "pasted-2026-11-16-153012");
    }

    #[test]
    fn an_image_goes_where_the_others_are_and_writes_over_nothing() {
        let directory =
            std::env::temp_dir().join(format!("zetwerk-image-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(directory.join("figures")).unwrap();
        let text = "#image(\"figures/a.png\") #image(\"figures/b.png\") #image(\"c.png\") #image(\"gone/d.png\")";
        assert_eq!(images_folder(text, &directory), PathBuf::from("figures"));
        assert_eq!(
            images_folder("#image(\"gone/d.png\")", &directory),
            PathBuf::new()
        );
        assert_eq!(images_folder("", &directory), PathBuf::new());
        let first = free_path(&directory, "plot", "png");
        assert_eq!(first, directory.join("plot.png"));
        std::fs::write(&first, b"x").unwrap();
        assert_eq!(
            free_path(&directory, "plot", "png"),
            directory.join("plot-2.png")
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_large_screenshot_is_made_smaller() {
        let big = image::RgbImage::from_pixel(3000, 1500, image::Rgb([200, 30, 30]));
        let mut png = std::io::Cursor::new(Vec::new());
        big.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let png = png.into_inner();
        assert_eq!(dimensions(&png), Some((3000, 1500)));
        assert!(is_large((3000, 1500)) && !is_large((1920, 1080)));
        let small = smaller(&png).unwrap();
        assert_eq!(dimensions(&small), Some((2000, 1000)));
        assert_eq!(dimensions(b"<svg/>"), None);
        assert_eq!(size(512), "512 bytes");
        assert_eq!(size(245_100), "246 KB");
        assert_eq!(size(3_100_000), "3.1 MB");
    }
}
