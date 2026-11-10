//! How much of the machine zetwerk may use.

use std::path::Path;
use std::time::Duration;

/// The user's choice between responsiveness and battery life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PowerMode {
    /// [`PowerMode::Saver`] on battery, [`PowerMode::Performance`] otherwise.
    #[default]
    Auto,
    Saver,
    Performance,
}

impl PowerMode {
    /// Parses the name used on the command line.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "auto" => Some(Self::Auto),
            "saver" => Some(Self::Saver),
            "performance" => Some(Self::Performance),
            _ => None,
        }
    }

    /// The limits this mode means right now.
    #[must_use]
    pub fn budget(self) -> Budget {
        let saver = match self {
            Self::Saver => true,
            Self::Performance => false,
            Self::Auto => on_battery(Path::new("/sys/class/power_supply")),
        };
        let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        if saver {
            Budget {
                debounce: Duration::from_millis(500),
                // Opening a long document on two threads takes twice as long
                // as on four for about the same total work, so going lower
                // would cost time without saving energy.
                compile_threads: cores.clamp(1, 4),
                raster_threads: cores.clamp(1, 2),
            }
        } else {
            Budget {
                debounce: Duration::from_millis(200),
                // Only the first compile of a document uses more than one
                // core, and it is the one the user waits for.
                compile_threads: cores,
                raster_threads: cores.clamp(1, 4),
            }
        }
    }
}

/// Limits derived from a [`PowerMode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// How long typing must pause before the document is compiled.
    pub debounce: Duration,
    pub compile_threads: usize,
    pub raster_threads: usize,
}

/// Whether the machine runs on battery: it has a mains supply and none is
/// online. Desktops, and systems where this cannot be read, are not on
/// battery.
fn on_battery(supplies: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(supplies) else {
        return false;
    };
    let mains: Vec<bool> = entries
        .flatten()
        .filter(|entry| {
            std::fs::read_to_string(entry.path().join("type"))
                .is_ok_and(|kind| matches!(kind.trim(), "Mains" | "USB"))
        })
        .filter_map(|entry| std::fs::read_to_string(entry.path().join("online")).ok())
        .map(|online| online.trim() == "1")
        .collect();
    !mains.is_empty() && !mains.contains(&true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory that is removed again when the test is done.
    struct Scratch(std::path::PathBuf);

    impl std::ops::Deref for Scratch {
        type Target = std::path::Path;

        fn deref(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn supplies(name: &str, entries: &[(&str, &str, &str)]) -> Scratch {
        let dir = std::env::temp_dir().join(format!("zetwerk-power-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (supply, kind, online) in entries {
            let path = dir.join(supply);
            std::fs::create_dir_all(&path).unwrap_or_else(|err| panic!("{err}"));
            std::fs::write(path.join("type"), kind).unwrap_or_else(|err| panic!("{err}"));
            std::fs::write(path.join("online"), online).unwrap_or_else(|err| panic!("{err}"));
        }
        Scratch(dir)
    }

    #[test]
    fn unplugged_laptop_is_on_battery() {
        let dir = supplies(
            "unplugged",
            &[("AC", "Mains\n", "0\n"), ("BAT0", "Battery\n", "")],
        );
        assert!(on_battery(&dir));
    }

    #[test]
    fn any_online_supply_means_mains() {
        let dir = supplies("usb", &[("AC", "Mains\n", "0\n"), ("ucsi", "USB\n", "1\n")]);
        assert!(!on_battery(&dir));
    }

    #[test]
    fn desktops_and_unreadable_systems_are_not_on_battery() {
        assert!(!on_battery(&supplies("desktop", &[])));
        assert!(!on_battery(Path::new("/nonexistent/zetwerk")));
    }

    #[test]
    fn saver_is_gentler_than_performance() {
        let (saver, performance) = (PowerMode::Saver.budget(), PowerMode::Performance.budget());
        assert!(saver.debounce > performance.debounce);
        assert!(saver.compile_threads <= performance.compile_threads);
        assert!(saver.raster_threads <= performance.raster_threads);
        assert_eq!(PowerMode::from_name("saver"), Some(PowerMode::Saver));
        assert_eq!(PowerMode::from_name("turbo"), None);
    }
}
