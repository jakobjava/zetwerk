//! What git knows of a document's files, asked of the `git` program.
//!
//! Only a little: the branch and whether the file differs from `HEAD`,
//! the file as it was in a commit, the commits that changed it, and a
//! commit of the document's own files. Nothing that changes files under
//! the editor (no checkout, no pull, no merge) and nothing that reaches
//! another machine (no fetch, no push): those belong in a terminal.
//!
//! Each question is one run of `git`, with nothing on its input, no
//! prompt for a password or an editor, a deadline after which it is
//! stopped, and English messages to read. The functions here wait for
//! it: the window asks them on a thread of its own (see
//! `viewer/changes.rs`). Where there is no `git`, no repository, or the
//! file is not in one, they say so and nothing else happens.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long a question may take, and a commit (which may run the
/// repository's hooks).
pub const ASKING: Duration = Duration::from_secs(5);
pub const COMMITTING: Duration = Duration::from_secs(30);

/// Where `git` is asked from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// The directory of the document's main file: git finds the
    /// repository from there.
    pub dir: PathBuf,
    /// Where git stops looking further up for a repository, if it is to
    /// (`GIT_CEILING_DIRECTORIES`): tests make their repositories inside
    /// another and must never find that one.
    pub ceiling: Option<PathBuf>,
    /// Whether the user's and the system's settings are left unread:
    /// for tests, which must not depend on them.
    pub isolated: bool,
}

/// What `git` said.
#[derive(Debug)]
struct Said {
    ok: bool,
    out: String,
    err: String,
}

impl Repo {
    /// Asked from `dir`, as the user's git would be.
    #[must_use]
    pub fn at(dir: &Path) -> Self {
        Self {
            dir: dir.to_owned(),
            ceiling: None,
            isolated: false,
        }
    }

    /// Runs `git` with `args`, stopping it after `deadline`.
    fn run(&self, args: &[&str], deadline: Duration) -> Result<Said, String> {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(&self.dir)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_PAGER", "cat")
            .env("GIT_EDITOR", "true")
            .env("LC_ALL", "C");
        if let Some(ceiling) = &self.ceiling {
            command.env("GIT_CEILING_DIRECTORIES", ceiling);
        }
        if self.isolated {
            command
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1");
        }
        let mut child = command.spawn().map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => "git is not installed".to_owned(),
            _ => format!("git does not start: {err}"),
        })?;
        // Read while it runs: a long answer would fill the pipe and
        // stop it.
        let reader = |pipe: Option<Box<dyn Read + Send>>| {
            std::thread::spawn(move || {
                let mut read = Vec::new();
                if let Some(mut pipe) = pipe {
                    let _ = pipe.read_to_end(&mut read);
                }
                String::from_utf8_lossy(&read).into_owned()
            })
        };
        let out = reader(
            child
                .stdout
                .take()
                .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
        );
        let err = reader(
            child
                .stderr
                .take()
                .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
        );
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() > deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "git took longer than {} s and was stopped",
                        deadline.as_secs()
                    ));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(err) => return Err(format!("git: {err}")),
            }
        };
        Ok(Said {
            ok: status.success(),
            out: out.join().unwrap_or_default(),
            err: err.join().unwrap_or_default(),
        })
    }

    /// Runs `git` with `args`; what it wrote, or the first line of why
    /// it failed.
    fn ask(&self, args: &[&str], deadline: Duration) -> Result<String, String> {
        let said = self.run(args, deadline)?;
        if said.ok {
            Ok(said.out)
        } else {
            Err(first_line(&said.err))
        }
    }
}

/// The first line of what git complained, without its `fatal: `.
fn first_line(err: &str) -> String {
    let line = err
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("git failed");
    line.trim_start_matches("fatal: ")
        .trim_start_matches("error: ")
        .trim()
        .to_owned()
}

/// What git says of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// The branch checked out; `None` with no branch (a commit checked
    /// out by itself).
    pub branch: Option<String>,
    /// Whether the branch has commits yet.
    pub born: bool,
    pub file: FileState,
    /// The repository's own directory (`.git`), whose `HEAD` and
    /// `index` say when this is to be asked again.
    pub git_dir: PathBuf,
    /// The top of the working tree.
    pub top: PathBuf,
}

/// How a file stands to the last commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    /// As in `HEAD`.
    Same,
    /// Different from `HEAD` (or added, not yet committed).
    Changed,
    /// Not known to git.
    Untracked,
    /// Left out by `.gitignore`.
    Ignored,
}

/// Where a revision may be named: not as an option of git.
fn checked(revision: &str) -> Result<&str, String> {
    let revision = revision.trim();
    if revision.is_empty() || revision.starts_with('-') || revision.contains(char::is_whitespace) {
        return Err(format!("not a revision: {revision:?}"));
    }
    Ok(revision)
}

/// `path` as git names it from the directory it is asked from.
fn named(repo: &Repo, path: &Path) -> String {
    let within = path.strip_prefix(&repo.dir).unwrap_or(path);
    within.to_string_lossy().replace('\\', "/")
}

/// The branch, and how `file` stands to the last commit. `Err` where
/// there is no git or no repository.
pub fn status(repo: &Repo, file: &Path) -> Result<Status, String> {
    let places = repo.ask(
        &["rev-parse", "--absolute-git-dir", "--show-toplevel"],
        ASKING,
    )?;
    let mut lines = places.lines();
    let (Some(git_dir), Some(top)) = (lines.next(), lines.next()) else {
        return Err("not in a git repository".to_owned());
    };
    let name = named(repo, file);
    let said = repo.ask(
        &[
            "status",
            "--porcelain=v1",
            "-b",
            "--ignored",
            "--untracked-files=all",
            "--",
            &name,
        ],
        ASKING,
    )?;
    let mut branch = None;
    let mut born = true;
    let mut state = FileState::Same;
    for line in said.lines() {
        if let Some(head) = line.strip_prefix("## ") {
            if let Some(unborn) = head.strip_prefix("No commits yet on ") {
                born = false;
                branch = Some(unborn.trim().to_owned());
            } else if !head.starts_with("HEAD (no branch)") {
                let name = head.split("...").next().unwrap_or(head);
                branch = Some(name.split(' ').next().unwrap_or(name).to_owned());
            }
            continue;
        }
        state = match line.get(..2) {
            Some("??") => FileState::Untracked,
            Some("!!") => FileState::Ignored,
            Some(_) => FileState::Changed,
            None => state,
        };
    }
    Ok(Status {
        branch,
        born,
        file: state,
        git_dir: PathBuf::from(git_dir),
        top: PathBuf::from(top),
    })
}

/// A commit: its short name, its date and what it says of itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub date: String,
    pub subject: String,
}

/// Reads lines of `hash TAB date TAB subject`.
fn commits(said: &str) -> Vec<Commit> {
    said.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            Some(Commit {
                hash: parts.next()?.to_owned(),
                date: parts.next()?.to_owned(),
                subject: parts.next().unwrap_or_default().to_owned(),
            })
        })
        .collect()
}

/// The text of `file` in `revision`, and that commit.
pub fn show(repo: &Repo, revision: &str, file: &Path) -> Result<(String, Commit), String> {
    let revision = checked(revision)?;
    let said = repo.ask(
        &["log", "-1", "--format=%h%x09%as%x09%s", revision, "--"],
        ASKING,
    )?;
    let commit = commits(&said)
        .into_iter()
        .next()
        .ok_or_else(|| format!("no commit {revision}"))?;
    let name = named(repo, file);
    let wanted = format!("{}:./{name}", commit.hash);
    match repo.ask(&["show", &wanted], ASKING) {
        Ok(text) => Ok((text, commit)),
        Err(why)
            if why.contains("exists on disk, but not in") || why.contains("does not exist in") =>
        {
            Err(format!("{name} is not in {revision} ({})", commit.hash))
        }
        Err(why) => Err(why),
    }
}

/// The commits that changed `file`, the newest first.
pub fn log(repo: &Repo, file: &Path, most: usize) -> Result<Vec<Commit>, String> {
    let most = format!("-n{most}");
    let name = named(repo, file);
    let said = repo.ask(
        &["log", &most, "--format=%h%x09%as%x09%s", "--", &name],
        ASKING,
    )?;
    Ok(commits(&said))
}

/// Of `files`, those that differ from the last commit or are new to git:
/// what a commit of them would take. Files left out by `.gitignore` and
/// files outside the working tree are not among them.
pub fn to_commit(repo: &Repo, top: &Path, files: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    // Each as the file system names it, as git names the top.
    let real: Vec<(PathBuf, &PathBuf)> = files
        .iter()
        .filter_map(|file| Some((std::fs::canonicalize(file).ok()?, file)))
        .filter(|(real, _)| real.starts_with(top))
        .collect();
    if real.is_empty() {
        return Ok(Vec::new());
    }
    let named: Vec<String> = real
        .iter()
        .map(|(real, _)| real.to_string_lossy().into_owned())
        .collect();
    let mut args = vec![
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--",
    ];
    args.extend(named.iter().map(String::as_str));
    let said = repo.ask(&args, ASKING)?;
    // Paths in what `-z` says are of the top of the working tree.
    let listed: Vec<PathBuf> = said
        .split('\0')
        .filter(|entry| entry.len() > 3 && !entry.starts_with("!!"))
        .map(|entry| top.join(&entry[3..]))
        .collect();
    let mut found: Vec<PathBuf> = real
        .into_iter()
        .filter(|(real, _)| listed.contains(real))
        .map(|(_, file)| file.clone())
        .collect();
    found.sort();
    found.dedup();
    Ok(found)
}

/// Commits `files` (and nothing else that may be staged) with `message`.
/// Returns what git says of the commit: its branch, name and subject.
pub fn commit(repo: &Repo, files: &[PathBuf], message: &str) -> Result<String, String> {
    if files.is_empty() {
        return Err("nothing to commit".to_owned());
    }
    // Whole paths, so that it does not matter where git is asked from.
    let names: Vec<String> = files
        .iter()
        .map(|file| {
            let real = std::fs::canonicalize(file).unwrap_or_else(|_| file.clone());
            real.to_string_lossy().into_owned()
        })
        .collect();
    let mut add = vec!["add", "--"];
    add.extend(names.iter().map(String::as_str));
    repo.ask(&add, ASKING)?;
    let mut commit = vec!["commit", "--no-edit", "-m", message, "--"];
    commit.extend(names.iter().map(String::as_str));
    let said = repo.run(&commit, COMMITTING)?;
    if !said.ok {
        let why = if said.err.trim().is_empty() {
            &said.out
        } else {
            &said.err
        };
        return Err(first_line(why));
    }
    Ok(said
        .out
        .lines()
        .next()
        .unwrap_or("committed")
        .trim()
        .to_owned())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
pub(crate) mod tests {
    use super::*;

    /// A fresh directory under the temporary one, which a test makes its
    /// repository in: git never looks above it.
    pub(crate) fn scratch(name: &str) -> (Repo, PathBuf) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "zetwerk-git-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (isolated(&dir), dir)
    }

    /// Asked from `dir`, never above it and without the user's settings.
    pub(crate) fn isolated(dir: &Path) -> Repo {
        Repo {
            dir: dir.to_owned(),
            ceiling: dir.parent().map(Path::to_owned),
            isolated: true,
        }
    }

    /// Makes `dir` a repository with a name and an address of its own.
    pub(crate) fn init(repo: &Repo) {
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.name", "Test Writer"],
            &["config", "user.email", "writer@example.invalid"],
            &["config", "commit.gpgsign", "false"],
        ] {
            let said = repo.run(args, ASKING).unwrap();
            assert!(said.ok, "{args:?}: {}", said.err);
        }
    }

    /// Commits all there is, as a writer would in a terminal.
    pub(crate) fn commit_all(repo: &Repo, message: &str) {
        for args in [&["add", "-A"][..], &["commit", "-q", "-m", message]] {
            let said = repo.run(args, ASKING).unwrap();
            assert!(said.ok, "{args:?}: {}", said.err);
        }
    }

    #[test]
    fn outside_a_repository_git_says_so_and_nothing_happens() {
        let (repo, dir) = scratch("none");
        std::fs::write(dir.join("a.typ"), "Text.\n").unwrap();
        let said = status(&repo, &dir.join("a.typ"));
        assert!(said.is_err(), "{said:?}");
        assert!(show(&repo, "HEAD", &dir.join("a.typ")).is_err());
        assert!(log(&repo, &dir.join("a.typ"), 10).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_branch_the_state_of_the_file_and_its_text_in_a_commit() {
        let (repo, dir) = scratch("status");
        init(&repo);
        let file = dir.join("paper.typ");
        std::fs::write(&file, "First.\n").unwrap();
        let said = status(&repo, &file).unwrap();
        assert_eq!(said.branch.as_deref(), Some("main"));
        assert!(!said.born);
        assert_eq!(said.file, FileState::Untracked);
        commit_all(&repo, "First words");
        assert_eq!(status(&repo, &file).unwrap().file, FileState::Same);
        std::fs::write(&file, "Second.\n").unwrap();
        let said = status(&repo, &file).unwrap();
        assert_eq!((said.file, said.born), (FileState::Changed, true));
        assert!(said.git_dir.ends_with(".git"));
        let (text, commit) = show(&repo, "HEAD", &file).unwrap();
        assert_eq!(text, "First.\n");
        assert_eq!(commit.subject, "First words");
        commit_all(&repo, "Second words");
        let (text, _) = show(&repo, "HEAD~1", &file).unwrap();
        assert_eq!(text, "First.\n");
        let listed = log(&repo, &file, 10).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].subject, "Second words");
        // What is not a revision, or names an option, is refused.
        assert!(show(&repo, "--output=x", &file).is_err());
        assert!(show(&repo, "no-such-branch", &file).is_err());
        // A file git does not know is in no commit.
        std::fs::write(dir.join("new.typ"), "New.\n").unwrap();
        let why = show(&repo, "HEAD", &dir.join("new.typ")).unwrap_err();
        assert!(why.contains("not in HEAD"), "{why}");
        std::fs::write(dir.join(".gitignore"), "skip.typ\n").unwrap();
        std::fs::write(dir.join("skip.typ"), "x\n").unwrap();
        assert_eq!(
            status(&repo, &dir.join("skip.typ")).unwrap().file,
            FileState::Ignored
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_commit_takes_the_files_named_and_nothing_else() {
        let (repo, dir) = scratch("commit");
        init(&repo);
        let (paper, refs, other) = (
            dir.join("paper.typ"),
            dir.join("refs.bib"),
            dir.join("notes.txt"),
        );
        std::fs::write(&paper, "One.\n").unwrap();
        std::fs::write(&other, "mine\n").unwrap();
        commit_all(&repo, "Begin");
        std::fs::write(&paper, "Two.\n").unwrap();
        std::fs::write(&refs, "@book{a, title={A}}\n").unwrap();
        std::fs::write(&other, "changed, not the document's\n").unwrap();
        // Staged by the writer: stays staged, is not committed.
        repo.run(&["add", "notes.txt"], ASKING).unwrap();
        let top = status(&repo, &paper).unwrap().top;
        let files = vec![paper.clone(), refs.clone(), dir.join("image.png")];
        let wanted = to_commit(&repo, &top, &files).unwrap();
        assert_eq!(wanted, vec![paper.clone(), refs.clone()]);
        let said = commit(&repo, &wanted, "Paper and sources").unwrap();
        assert!(said.contains("Paper and sources"), "{said}");
        let committed = repo
            .ask(&["show", "--name-only", "--format=", "HEAD"], ASKING)
            .unwrap();
        let mut names: Vec<&str> = committed.lines().collect();
        names.sort_unstable();
        assert_eq!(names, vec!["paper.typ", "refs.bib"]);
        let staged = repo
            .ask(&["diff", "--cached", "--name-only"], ASKING)
            .unwrap();
        assert_eq!(staged.trim(), "notes.txt");
        assert_eq!(
            to_commit(&repo, &top, &files).unwrap(),
            Vec::<PathBuf>::new()
        );
        assert!(commit(&repo, &[], "x").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
