//! Compiling in the background, and deciding when to.
//!
//! Two independent parts:
//!
//! - [`Scheduler`] decides *when* a compile is due. It is plain state with
//!   no clock and no thread of its own, so the event loop can ask it for the
//!   next deadline and sleep until then.
//! - [`Compiler`] owns the thread that compiles. It never runs more than one
//!   compile at a time and always prefers the newest snapshot it was given.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::{Compiled, ProjectWorld, compile, forget_unused};

/// How many prunings of Typst's cache a result may go unused before it is
/// dropped. A document compile uses everything the next one can reuse, so
/// this mostly decides how long results for undone states are kept.
const UNUSED_ROUNDS: usize = 10;

/// How soon a change should be compiled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Urgency {
    /// As soon as possible: nothing on screen reflects the change yet.
    Now,
    /// Once input has paused. Further requests push the compile back.
    WhenIdle,
}

/// Decides when the next compile is due.
#[derive(Debug)]
pub struct Scheduler {
    debounce: Duration,
    due: Option<Instant>,
}

impl Scheduler {
    /// A scheduler that compiles `debounce` after the last
    /// [`Urgency::WhenIdle`] request.
    #[must_use]
    pub fn new(debounce: Duration) -> Self {
        Self {
            debounce,
            due: None,
        }
    }

    /// Changes the idle delay for requests from now on.
    pub fn set_debounce(&mut self, debounce: Duration) {
        self.debounce = debounce;
    }

    /// Notes that the document changed at `now`.
    pub fn request(&mut self, urgency: Urgency, now: Instant) {
        let due = match urgency {
            Urgency::Now => now,
            Urgency::WhenIdle => now + self.debounce,
        };
        // An urgent request is never delayed by a later lazy one.
        self.due = Some(match (urgency, self.due) {
            (Urgency::WhenIdle, Some(earlier)) if earlier <= now => earlier,
            _ => due,
        });
    }

    /// When the pending compile should start, if one is pending.
    #[must_use]
    pub fn deadline(&self) -> Option<Instant> {
        self.due
    }

    /// Returns `true` once, when the pending compile has become due.
    pub fn take_due(&mut self, now: Instant) -> bool {
        match self.due {
            Some(due) if due <= now => {
                self.due = None;
                true
            }
            _ => false,
        }
    }
}

/// The result of compiling one snapshot.
pub struct CompileOutput {
    /// The revision passed to [`Compiler::compile`].
    pub revision: u64,
    /// The snapshot that was compiled. Diagnostics and page maps must be
    /// resolved against this, not against the project's current state.
    pub world: Arc<ProjectWorld>,
    pub compiled: Compiled,
    /// Wall-clock time the compile took.
    pub duration: Duration,
}

/// A background thread that compiles snapshots.
pub struct Compiler {
    requests: Option<Sender<(u64, ProjectWorld)>>,
    results: Receiver<CompileOutput>,
    thread: Option<JoinHandle<()>>,
}

impl Compiler {
    /// Starts the compile thread.
    ///
    /// `threads` caps how many cores a compile may use. Only the first
    /// compile of a document parallelises; incremental ones run on one core
    /// regardless. `wake` is called from the compile thread whenever a
    /// result is ready to be collected with [`Compiler::poll`].
    pub fn spawn(threads: usize, wake: impl Fn() + Send + 'static) -> std::io::Result<Self> {
        let (request_tx, request_rx) = mpsc::channel::<(u64, ProjectWorld)>();
        let (result_tx, result_rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("zetwerk-compile".into())
            .spawn(move || {
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(threads.max(1))
                    .thread_name(|index| format!("zetwerk-compile-{index}"))
                    .build()
                    .ok();
                while let Ok(mut request) = request_rx.recv() {
                    // Skip straight to the newest snapshot: compiling a state
                    // the user has already typed past is wasted work.
                    while let Ok(newer) = request_rx.try_recv() {
                        request = newer;
                    }
                    let (revision, world) = request;
                    let start = Instant::now();
                    let compiled = match &pool {
                        Some(pool) => pool.install(|| compile(&world)),
                        None => compile(&world),
                    };
                    forget_unused(UNUSED_ROUNDS);
                    let output = CompileOutput {
                        revision,
                        world: Arc::new(world),
                        compiled,
                        duration: start.elapsed(),
                    };
                    if result_tx.send(output).is_err() {
                        break;
                    }
                    wake();
                }
            })?;
        Ok(Self {
            requests: Some(request_tx),
            results: result_rx,
            thread: Some(thread),
        })
    }

    /// Queues `world` for compilation. If a compile is running, this one
    /// starts when it finishes, unless a newer snapshot arrives first.
    pub fn compile(&self, revision: u64, world: ProjectWorld) {
        if let Some(requests) = &self.requests {
            // Sending only fails if the thread is gone, which `poll`
            // callers notice as results no longer arriving.
            let _ = requests.send((revision, world));
        }
    }

    /// The newest finished compile, if any finished since the last call.
    /// Older results that were overtaken are dropped.
    pub fn poll(&self) -> Option<CompileOutput> {
        self.results.try_iter().last()
    }
}

impl Drop for Compiler {
    fn drop(&mut self) {
        // Closing the channel ends the thread's loop after the compile in
        // progress; wait for it so the process does not exit mid-compile.
        self.requests = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::{Environment, FontSet, Project};

    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn idle_requests_push_the_deadline_back() {
        let start = Instant::now();
        let mut scheduler = Scheduler::new(SECOND);
        scheduler.request(Urgency::WhenIdle, start);
        scheduler.request(Urgency::WhenIdle, start + SECOND / 2);
        assert_eq!(scheduler.deadline(), Some(start + SECOND * 3 / 2));
        assert!(!scheduler.take_due(start + SECOND));
        assert!(scheduler.take_due(start + SECOND * 2));
        assert_eq!(scheduler.deadline(), None);
        assert!(!scheduler.take_due(start + SECOND * 3));
    }

    #[test]
    fn urgent_request_is_due_immediately_and_not_delayed() {
        let start = Instant::now();
        let mut scheduler = Scheduler::new(SECOND);
        scheduler.request(Urgency::WhenIdle, start);
        scheduler.request(Urgency::Now, start);
        scheduler.request(Urgency::WhenIdle, start);
        assert!(scheduler.take_due(start));
    }

    fn project(text: &str) -> Project {
        let env = Environment::new(FontSet::Embedded);
        Project::in_memory(env, PathBuf::from("/"), "main.typ", text.to_owned())
            .unwrap_or_else(|| panic!("valid main file name"))
    }

    fn wait_for(compiler: &Compiler) -> CompileOutput {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(output) = compiler.poll() {
                return output;
            }
            assert!(Instant::now() < deadline, "no compile result within 30 s");
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn compiles_in_the_background_and_wakes() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&wakes);
        let compiler = Compiler::spawn(2, move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap_or_else(|err| panic!("thread spawns: {err}"));

        compiler.compile(7, project("Hello").snapshot());
        let output = wait_for(&compiler);
        assert_eq!(output.revision, 7);
        assert_eq!(
            output.compiled.document.map(|doc| doc.pages().len()),
            Some(1)
        );
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn errors_are_reported_not_fatal() {
        let compiler =
            Compiler::spawn(1, || {}).unwrap_or_else(|err| panic!("thread spawns: {err}"));
        compiler.compile(1, project("#undefined()").snapshot());
        let output = wait_for(&compiler);
        assert!(output.compiled.document.is_none());
        assert_eq!(output.compiled.errors.len(), 1);

        // The thread is still alive and compiles the fixed document.
        compiler.compile(2, project("fixed").snapshot());
        assert!(wait_for(&compiler).compiled.document.is_some());
    }

    #[test]
    fn the_newest_snapshot_wins() {
        let compiler =
            Compiler::spawn(1, || {}).unwrap_or_else(|err| panic!("thread spawns: {err}"));
        for revision in 1..=50 {
            compiler.compile(
                revision,
                project(&format!("revision {revision}")).snapshot(),
            );
        }
        // Whatever was skipped in between, the last result is the last
        // request.
        let mut last = wait_for(&compiler).revision;
        while last != 50 {
            last = wait_for(&compiler).revision;
        }
    }
}
