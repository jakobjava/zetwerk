//! Files: hashing them, replacing them atomically, locking them.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

/// A BLAKE3 hash of a file's bytes.
pub type Hash = [u8; 32];

#[must_use]
pub fn hash(bytes: &[u8]) -> Hash {
    *blake3::hash(bytes).as_bytes()
}

/// Reads a file, or `None` if it does not exist.
pub fn read(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// Replaces the file at `path` with `bytes` so that at every moment, crash
/// or not, the path names either the complete old file or the complete new
/// one: the bytes go to a temporary file in the same directory, are forced
/// to disk, and are renamed over the path; then the directory is forced to
/// disk too. An existing file's permissions are kept.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = atomic_write_file::AtomicWriteFile::open(path)?;
    file.write_all(bytes)?;
    file.commit()
}

/// Writes `bytes` over the file at `path`, which must exist, in place, and
/// forces them to disk. Unlike [`write_atomically`] a crash can leave the
/// file cut short; it is for a file whose directory takes no new file.
pub fn write_in_place(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).open(path)?;
    file.write_all(bytes)?;
    file.set_len(bytes.len() as u64)?;
    file.sync_all()
}

/// Gives the file at `from` the name `to` as well, never writing over
/// anything at `to` ([`io::ErrorKind::AlreadyExists`]). Where the two
/// names cannot share the file (another file system), `to` is made a
/// complete copy, with the same permissions, before this returns.
pub fn link_or_copy(from: &Path, to: &Path) -> io::Result<()> {
    match std::fs::hard_link(from, to) {
        Ok(()) => return Ok(()),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => return Err(err),
        Err(_) => {}
    }
    let bytes = std::fs::read(from)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(to)?;
    let copied = file
        .write_all(&bytes)
        .and_then(|()| file.set_permissions(std::fs::metadata(from)?.permissions()))
        .and_then(|()| file.sync_all());
    if copied.is_err() {
        let _ = std::fs::remove_file(to);
    }
    copied
}

/// The temporary files an interrupted [`write_atomically`] to `path` can
/// have left next to it: regular files named `.<file name>.<six characters>`.
///
/// On Linux the temporary file is nameless until it is complete, and gets a
/// name only for the instant before it is renamed into place, so leftovers
/// are rare; elsewhere every interrupted write leaves one.
pub fn leftovers(path: &Path) -> Vec<std::path::PathBuf> {
    let (Some(directory), Some(name)) = (path.parent(), path.file_name()) else {
        return Vec::new();
    };
    let prefix = format!(".{}.", name.to_string_lossy());
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let suffix = name.to_string_lossy();
            let Some(suffix) = suffix.strip_prefix(&prefix) else {
                return false;
            };
            suffix.len() == 6
                && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
                && entry.file_type().is_ok_and(|kind| kind.is_file())
        })
        .map(|entry| entry.path())
        .collect()
}

/// Creates a directory, and its parents, that only the user can enter.
pub fn create_private_dir(path: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(path)
}

/// Exclusive ownership of a document across processes, held while this
/// value lives. The operating system releases it when the process ends,
/// however it ends.
pub struct Lock {
    _file: File,
}

impl Lock {
    /// Takes the lock at `path`. `None` if another process holds it.
    pub fn acquire(path: &Path) -> io::Result<Option<Self>> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(err)) => Err(err),
        }
    }
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

    fn scratch(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("zetwerk-disk-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        create_private_dir(&dir).unwrap_or_else(|err| panic!("{err}"));
        Scratch(dir)
    }

    #[test]
    fn atomic_write_replaces_and_creates() {
        let dir = scratch("write");
        let path = dir.join("file.typ");
        assert_eq!(read(&path).ok(), Some(None));
        write_atomically(&path, b"one").unwrap_or_else(|err| panic!("{err}"));
        write_atomically(&path, b"two").unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(read(&path).ok(), Some(Some(b"two".to_vec())));
        // No temporary file is left next to it.
        let entries = std::fs::read_dir(&*dir).map_or(0, Iterator::count);
        assert_eq!(entries, 1);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_keeps_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("mode");
        let path = dir.join("file.typ");
        std::fs::write(&path, "old").unwrap_or_else(|err| panic!("{err}"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640))
            .unwrap_or_else(|err| panic!("{err}"));
        write_atomically(&path, b"new").unwrap_or_else(|err| panic!("{err}"));
        let mode = std::fs::metadata(&path).map(|meta| meta.permissions().mode() & 0o777);
        assert_eq!(mode.ok(), Some(0o640));
    }

    #[test]
    fn a_lock_is_exclusive_until_dropped() {
        let dir = scratch("lock");
        let path = dir.join("doc.lock");
        let first = Lock::acquire(&path).unwrap_or_else(|err| panic!("{err}"));
        assert!(first.is_some());
        let second = Lock::acquire(&path).unwrap_or_else(|err| panic!("{err}"));
        assert!(second.is_none());
        drop(first);
        let third = Lock::acquire(&path).unwrap_or_else(|err| panic!("{err}"));
        assert!(third.is_some());
    }
}
