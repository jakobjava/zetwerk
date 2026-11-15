//! Compiling in the background, and deciding when to.
//!
//! Two independent parts:
//!
//! - [`Scheduler`] decides *when* a compile is due. It is plain state with
//!   no clock and no thread of its own, so the event loop can ask it for the
//!   next deadline and sleep until then.
//! - [`Compiler`] owns the thread that compiles. It never runs more than one
//!   compile at a time and always prefers the newest snapshot it was given.
//!   A compile that a newer text has waited behind for long (see
//!   [`Bounds`]), or that the user stops, is given up on: Typst cannot be
//!   stopped, so it runs on to its end on a thread of its own, at the
//!   lowest priority, and what it makes is dropped.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
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

    /// Forgets the pending compile: what was asked for is given up on.
    pub fn cancel(&mut self) {
        self.due = None;
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

/// When a compile that a newer text waits behind is given up on, and how
/// many given up on may still be running.
///
/// Typst cannot be stopped in the middle of a compile, and a thread
/// cannot be killed: a compile given up on runs to its end at the lowest
/// priority, on its own thread, and what it makes is dropped. The newer
/// text goes to a thread of its own meanwhile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    /// A compile a newer text waits behind is given up on once it has
    /// run this long, and longer than [`Bounds::usual_times`] what the
    /// last compile that ended took: a document that always takes long
    /// is not compiled twice at once for it.
    pub overtake_after: Duration,
    pub usual_times: u32,
    /// Before any compile ended (opening a document, where all of it is
    /// typeset for the first time) the newer text waits this long.
    pub first_after: Duration,
    /// How many compiles given up on may be running at a time. Beyond
    /// that, the newer text waits for one of them to end.
    pub abandoned: usize,
}

impl Default for Bounds {
    /// Measured on the laptop zetwerk is made on (see
    /// `viewer/tests/waits.rs` in `zetwerk-app`): a one-character change
    /// of a three-hundred-page document compiles in about 0.4 s, opening it
    /// takes 2–3 s with all cores, and either takes about twice as long on
    /// battery.
    fn default() -> Self {
        Self {
            overtake_after: Duration::from_secs(3),
            usual_times: 4,
            first_after: Duration::from_secs(20),
            abandoned: 2,
        }
    }
}

/// How long dropping a [`Compiler`] waits for its threads to end. A
/// compile under way is of no use to anyone then, and cannot be stopped:
/// longer than this, and it is left to end by itself (or with the process).
const EXIT_WAIT: Duration = Duration::from_millis(100);

/// What is asked of a compile thread: which request it is, the revision,
/// and the snapshot to compile.
struct Request {
    number: u64,
    revision: u64,
    world: ProjectWorld,
}

/// What a compile thread shares with the [`Compiler`] that owns it.
#[derive(Default)]
struct Slot {
    /// The newest snapshot asked for and not yet begun.
    waiting: Option<Request>,
    /// When the compile under way began.
    running: Option<Instant>,
    /// No more is asked of the thread: it ends after the compile under
    /// way, and drops what it makes of that.
    closed: bool,
    /// The thread has ended, or is about to, holding nothing any more.
    ended: bool,
    /// The threads that compile for it, as the system numbers them: to
    /// be given the lowest priority once it is given up on.
    threads: Vec<String>,
}

struct Shared {
    slot: Mutex<Slot>,
    ready: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Slot> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One compile thread.
struct Worker {
    shared: Arc<Shared>,
}

impl Worker {
    /// Tells the thread to end after what it is doing, and to drop it.
    /// Returns what was waiting for it, and since when it compiles, if it
    /// does.
    fn close(&self) -> (Option<Request>, Option<Instant>) {
        let mut slot = self.shared.lock();
        slot.closed = true;
        let waiting = slot.waiting.take();
        let running = slot.running;
        drop(slot);
        self.shared.ready.notify_all();
        (waiting, running)
    }

    fn running(&self) -> Option<Instant> {
        self.shared.lock().running
    }

    fn has_ended(&self) -> bool {
        self.shared.lock().ended
    }

    /// Gives the threads of a worker given up on the lowest priority,
    /// so that the one that took its place is not slowed down by it. Off
    /// the window's thread, and only where the system has `renice`.
    fn lower(&self) {
        let threads = self.shared.lock().threads.clone();
        if threads.is_empty() {
            return;
        }
        let _ = thread::Builder::new()
            .name("zetwerk-renice".into())
            .spawn(move || {
                let _ = std::process::Command::new("renice")
                    .args(["-n", "19", "-p"])
                    .args(&threads)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            });
    }
}

/// The number the system knows the calling thread by, where it says so.
fn thread_number() -> Option<String> {
    let link = std::fs::read_link("/proc/thread-self").ok()?;
    Some(link.file_name()?.to_string_lossy().into_owned())
}

/// What every compile thread of a [`Compiler`] is given.
#[derive(Clone)]
struct Common {
    threads: usize,
    results: Sender<CompileOutput>,
    answered: Arc<AtomicU64>,
    wake: Arc<Mutex<Box<dyn Fn() + Send>>>,
    ended: Sender<()>,
}

impl Common {
    fn wake(&self) {
        (self.wake.lock().unwrap_or_else(PoisonError::into_inner))();
    }

    /// Starts a compile thread.
    fn spawn(&self) -> std::io::Result<Worker> {
        let shared = Arc::new(Shared {
            slot: Mutex::new(Slot::default()),
            ready: Condvar::new(),
        });
        let common = self.clone();
        let own = Arc::clone(&shared);
        thread::Builder::new()
            .name("zetwerk-compile".into())
            .spawn(move || {
                common.work(&own);
                own.lock().ended = true;
                // Whoever waits for it to end may go on: a text held
                // back for it, or the window that closes.
                let _ = common.ended.send(());
                common.wake();
            })?;
        Ok(Worker { shared })
    }

    /// What a compile thread does until it is closed.
    fn work(&self, shared: &Arc<Shared>) {
        let numbers = Arc::clone(shared);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(self.threads.max(1))
            .thread_name(|index| format!("zetwerk-compile-{index}"))
            .start_handler(move |_| {
                if let Some(number) = thread_number() {
                    numbers.lock().threads.push(number);
                }
            })
            .build()
            .ok();
        if let Some(number) = thread_number() {
            shared.lock().threads.push(number);
        }
        loop {
            let request = {
                let mut slot = shared.lock();
                loop {
                    if slot.closed {
                        return;
                    }
                    if let Some(request) = slot.waiting.take() {
                        slot.running = Some(Instant::now());
                        break request;
                    }
                    slot = shared
                        .ready
                        .wait(slot)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            };
            let Request {
                number,
                revision,
                world,
            } = request;
            let start = Instant::now();
            let compiled = match &pool {
                Some(pool) => pool.install(|| compile(&world)),
                None => compile(&world),
            };
            let duration = start.elapsed();
            let given_up = {
                let mut slot = shared.lock();
                slot.running = None;
                slot.closed
            };
            if given_up {
                // Nobody waits for it; and what it used is not to push
                // out of Typst's cache what the compiles that took its
                // place use.
                return;
            }
            forget_unused(UNUSED_ROUNDS);
            let output = CompileOutput {
                revision,
                world: Arc::new(world),
                compiled,
                duration,
            };
            if self.results.send(output).is_err() {
                return;
            }
            // Only now: whoever sees this finds the result there.
            self.answered.fetch_max(number, Ordering::AcqRel);
            self.wake();
        }
    }
}

/// Background threads that compile snapshots: one at a time, the newest
/// first, and others only for compiles given up on (see [`Bounds`]).
pub struct Compiler {
    common: Common,
    results: Receiver<CompileOutput>,
    /// Told whenever a compile thread ends.
    ended: Receiver<()>,
    /// How many compiles were asked for, and the number of the last one
    /// that is answered (or was overtaken by one that is).
    asked: AtomicU64,
    /// The thread that compiles what is asked now, if one may.
    current: Option<Worker>,
    /// Asked for while no thread could take it: it waits for a compile
    /// given up on to end.
    held: Option<Request>,
    /// Threads of compiles given up on, still running.
    abandoned: Vec<(Worker, Instant)>,
    bounds: Bounds,
    /// What the last compile that ended took.
    usual: Option<Duration>,
}

impl Compiler {
    /// Starts the compile thread.
    ///
    /// `threads` caps how many cores a compile may use. Only the first
    /// compile of a document parallelises; incremental ones run on one core
    /// regardless. `wake` is called from the compile thread whenever a
    /// result is ready to be collected with [`Compiler::poll`], and
    /// whenever a compile given up on has ended.
    pub fn spawn(threads: usize, wake: impl Fn() + Send + 'static) -> std::io::Result<Self> {
        let (results_tx, results) = mpsc::channel();
        let (ended_tx, ended) = mpsc::channel();
        let common = Common {
            threads,
            results: results_tx,
            answered: Arc::new(AtomicU64::new(0)),
            wake: Arc::new(Mutex::new(Box::new(wake))),
            ended: ended_tx,
        };
        let current = common.spawn()?;
        Ok(Self {
            common,
            results,
            ended,
            asked: AtomicU64::new(0),
            current: Some(current),
            held: None,
            abandoned: Vec::new(),
            bounds: Bounds::default(),
            usual: None,
        })
    }

    /// Gives up on compiles by `bounds` from now on.
    pub fn set_bounds(&mut self, bounds: Bounds) {
        self.bounds = bounds;
    }

    /// Queues `world` for compilation. If a compile is running, this one
    /// starts when it finishes, unless a newer snapshot arrives first, or
    /// it is given up on for this one (see [`Compiler::overtake`]).
    pub fn compile(&mut self, revision: u64, world: ProjectWorld) {
        let number = self.asked.fetch_add(1, Ordering::AcqRel) + 1;
        self.give(Request {
            number,
            revision,
            world,
        });
    }

    /// Hands `request` to the thread that compiles now, or holds it until
    /// there may be one.
    fn give(&mut self, request: Request) {
        self.prune();
        // Beside no more compiles given up on than the bound: one more
        // may be where the user stopped it (see `abandon`), and the
        // text then waits for one of them to end.
        if self.current.is_none() && self.abandoned.len() <= self.bounds.abandoned {
            // Should no thread start, `poll` callers notice as results
            // no longer arriving.
            self.current = self.common.spawn().ok();
        }
        let Some(current) = &self.current else {
            self.held = Some(request);
            return;
        };
        self.held = None;
        // Skip straight to the newest snapshot: compiling a state the user
        // has already typed past is wasted work.
        current.shared.lock().waiting = Some(request);
        current.shared.ready.notify_all();
    }

    /// Forgets the threads of compiles given up on that have ended.
    fn prune(&mut self) {
        self.abandoned.retain(|(worker, _)| !worker.has_ended());
    }

    /// Whether every compile asked for is finished, and its result (or
    /// that of a newer one) there for [`Compiler::poll`]. Ask before
    /// polling: what is finished between the two is then not missed.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.common.answered.load(Ordering::Acquire) == self.asked.load(Ordering::Acquire)
    }

    /// The newest finished compile, if any finished since the last call.
    /// Older results that were overtaken are dropped. A text held back
    /// for a compile given up on goes on to be compiled here, once that
    /// has ended.
    pub fn poll(&mut self) -> Option<CompileOutput> {
        self.prune();
        if let Some(held) = self.held.take() {
            self.give(held);
        }
        let output = self.results.try_iter().last();
        if let Some(output) = &output {
            self.usual = Some(output.duration);
        }
        output
    }

    /// Since when the compile under way runs, if one does.
    #[must_use]
    pub fn running(&self) -> Option<Instant> {
        self.current.as_ref().and_then(Worker::running)
    }

    /// Since when a text waits for a compile given up on to end, if one
    /// does: since that began.
    #[must_use]
    pub fn held_since(&self) -> Option<Instant> {
        self.held.as_ref()?;
        self.abandoned.iter().map(|(_, since)| *since).min()
    }

    /// When the compile under way is to be given up on for the newer text
    /// that waits behind it, if one waits and it may be.
    #[must_use]
    pub fn overtake_at(&self) -> Option<Instant> {
        // As many given up on as may be: the text waits for one of them
        // to end, which wakes the window.
        if self.abandoned.len() >= self.bounds.abandoned {
            return None;
        }
        let current = self.current.as_ref()?;
        let slot = current.shared.lock();
        let (Some(since), Some(_)) = (slot.running, &slot.waiting) else {
            return None;
        };
        let after = match self.usual {
            Some(usual) => self
                .bounds
                .overtake_after
                .max(usual * self.bounds.usual_times),
            None => self.bounds.first_after,
        };
        Some(since + after)
    }

    /// Gives up on the compile under way if a newer text has waited
    /// behind it long enough (see [`Compiler::overtake_at`]), and as long
    /// as no more than [`Bounds::abandoned`] are given up on at a time:
    /// the newer text goes to a thread of its own. Returns whether it did.
    pub fn overtake(&mut self, now: Instant) -> bool {
        self.prune();
        if self.overtake_at().is_none_or(|at| at > now) {
            return false;
        }
        let Some(current) = self.current.take() else {
            return false;
        };
        let (waiting, running) = current.close();
        current.lower();
        self.abandoned.push((current, running.unwrap_or(now)));
        if let Some(waiting) = waiting {
            self.give(waiting);
        }
        true
    }

    /// Gives up on the compile under way, and on what waits for it, at
    /// the user's word: nothing asked for until now is compiled. Returns
    /// how long it had run, if a compile was under way or held back.
    pub fn abandon(&mut self) -> Option<Duration> {
        self.prune();
        let held = self.held_since();
        self.held = None;
        let ran = match self.current.take() {
            Some(current) => {
                let (_, running) = current.close();
                match running {
                    Some(since) => {
                        current.lower();
                        self.abandoned.push((current, since));
                        Some(since)
                    }
                    // It was waiting for work: it ends at once.
                    None => None,
                }
            }
            None => None,
        };
        // Nothing asked for is to come.
        let asked = self.asked.load(Ordering::Acquire);
        self.common.answered.fetch_max(asked, Ordering::AcqRel);
        ran.or(held).map(|since| since.elapsed())
    }

    /// How many compiles given up on are still running.
    #[must_use]
    pub fn abandoned(&mut self) -> usize {
        self.prune();
        self.abandoned.len()
    }
}

impl Drop for Compiler {
    fn drop(&mut self) {
        // What is compiled now is of use to nobody, and a compile cannot
        // be stopped: the threads are told to end, and are waited for
        // only a moment. Closing the window does not wait for a compile
        // that runs long; it ends with the process.
        let workers: Vec<Worker> = self
            .current
            .take()
            .into_iter()
            .chain(self.abandoned.drain(..).map(|(worker, _)| worker))
            .collect();
        for worker in &workers {
            worker.close();
        }
        let until = Instant::now() + EXIT_WAIT;
        while !workers.iter().all(Worker::has_ended) {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() || self.ended.recv_timeout(left).is_err() {
                break;
            }
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

    fn wait_for(compiler: &mut Compiler) -> CompileOutput {
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
        let mut compiler = Compiler::spawn(2, move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap_or_else(|err| panic!("thread spawns: {err}"));

        compiler.compile(7, project("Hello").snapshot());
        let output = wait_for(&mut compiler);
        assert_eq!(output.revision, 7);
        assert_eq!(
            output.compiled.document.map(|doc| doc.pages().len()),
            Some(1)
        );
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn errors_are_reported_not_fatal() {
        let mut compiler =
            Compiler::spawn(1, || {}).unwrap_or_else(|err| panic!("thread spawns: {err}"));
        compiler.compile(1, project("#undefined()").snapshot());
        let output = wait_for(&mut compiler);
        assert!(output.compiled.document.is_none());
        assert_eq!(output.compiled.errors.len(), 1);

        // The thread is still alive and compiles the fixed document.
        compiler.compile(2, project("fixed").snapshot());
        assert!(wait_for(&mut compiler).compiled.document.is_some());
    }

    #[test]
    fn the_newest_snapshot_wins() {
        let mut compiler =
            Compiler::spawn(1, || {}).unwrap_or_else(|err| panic!("thread spawns: {err}"));
        for revision in 1..=50 {
            compiler.compile(
                revision,
                project(&format!("revision {revision}")).snapshot(),
            );
        }
        // Whatever was skipped in between, the last result is the last
        // request.
        let mut last = wait_for(&mut compiler).revision;
        while last != 50 {
            last = wait_for(&mut compiler).revision;
        }
    }

    /// A text that takes long to compile: a loop of `rounds` rounds.
    fn slow(rounds: usize) -> ProjectWorld {
        project(&format!(
            "#let total = 0\n#for i in range({rounds}) {{ total += calc.rem(i, 7) }}\n#total"
        ))
        .snapshot()
    }

    /// Rounds that take a few tenths of a second, here and on a busy
    /// machine: long enough for what is asked behind them to be seen
    /// waiting, or not.
    const SLOW: usize = 400_000;

    /// Waits until `compiler` compiles.
    fn until_running(compiler: &Compiler) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while compiler.running().is_none() {
            assert!(Instant::now() < deadline, "no compile began within 30 s");
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Waits until no compile given up on runs.
    fn until_all_ended(compiler: &mut Compiler) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while compiler.abandoned() > 0 {
            assert!(Instant::now() < deadline, "compiles given up on still run");
            thread::sleep(Duration::from_millis(2));
        }
    }

    /// How nice the thread numbered `number` of this process is.
    fn niceness(number: &str) -> Option<i32> {
        let stat = std::fs::read_to_string(format!("/proc/self/task/{number}/stat")).ok()?;
        // Behind the name in parentheses: the state is the third field,
        // the niceness the nineteenth.
        let rest = &stat[stat.rfind(')')? + 2..];
        rest.split(' ').nth(16)?.parse().ok()
    }

    #[test]
    fn a_compile_overtaken_runs_on_aside_and_the_newer_text_does_not_wait() {
        let mut compiler =
            Compiler::spawn(1, || {}).unwrap_or_else(|err| panic!("thread spawns: {err}"));
        compiler.compile(1, slow(SLOW * 4));
        until_running(&compiler);
        compiler.compile(2, project("fixed").snapshot());
        // Not before its time: the first compile of a document may take
        // long.
        let at = compiler
            .overtake_at()
            .unwrap_or_else(|| panic!("a text waits"));
        assert!(
            at >= compiler.running().unwrap_or_else(Instant::now) + Bounds::default().first_after
        );
        assert!(!compiler.overtake(Instant::now()));
        // And then the newer text goes to a thread of its own.
        assert!(compiler.overtake(at));
        assert_eq!(compiler.abandoned(), 1);
        let output = wait_for(&mut compiler);
        assert_eq!(output.revision, 2);
        assert!(compiler.is_idle());
        // The one given up on is not waited for, and runs at the lowest
        // priority.
        let numbers = compiler.abandoned[0].0.shared.lock().threads.clone();
        assert_ne!(numbers.len(), 0, "the threads are known");
        let lowered = Instant::now() + Duration::from_secs(5);
        while !numbers
            .iter()
            .all(|number| niceness(number).is_none_or(|nice| nice == 19))
        {
            assert!(Instant::now() < lowered, "not lowered: {numbers:?}");
            thread::sleep(Duration::from_millis(5));
        }
        // What it makes is nobody's.
        until_all_ended(&mut compiler);
        assert!(compiler.poll().is_none());
    }

    #[test]
    fn a_document_that_always_takes_long_is_not_compiled_twice_at_once() {
        let mut compiler =
            Compiler::spawn(1, || {}).unwrap_or_else(|err| panic!("thread spawns: {err}"));
        compiler.compile(1, project("first").snapshot());
        wait_for(&mut compiler);
        compiler.usual = Some(Duration::from_secs(10));
        compiler.compile(2, slow(SLOW));
        until_running(&compiler);
        compiler.compile(3, project("fixed").snapshot());
        let since = compiler.running().unwrap_or_else(Instant::now);
        let at = compiler
            .overtake_at()
            .unwrap_or_else(|| panic!("a text waits"));
        assert_eq!(at, since + Duration::from_secs(40));
        assert!(!compiler.overtake(since + Duration::from_secs(39)));
        // Without the overtaking, the newer text is compiled after.
        assert_eq!(wait_for(&mut compiler).revision, 3);
    }

    #[test]
    fn no_more_than_the_bound_are_given_up_on_and_the_text_waits_for_them() {
        let mut compiler =
            Compiler::spawn(1, || {}).unwrap_or_else(|err| panic!("thread spawns: {err}"));
        compiler.set_bounds(Bounds {
            abandoned: 1,
            ..Bounds::default()
        });
        let later = Instant::now() + Duration::from_secs(3600);
        compiler.compile(1, slow(SLOW));
        until_running(&compiler);
        compiler.compile(2, slow(SLOW));
        assert!(compiler.overtake(later));
        until_running(&compiler);
        compiler.compile(3, project("three").snapshot());
        // One given up on already: the newer text waits its turn.
        assert!(!compiler.overtake(later));
        // Stopped by the user, it is given up on too; and the text asked
        // for next waits for one of the two to end.
        assert!(compiler.abandon().is_some());
        assert!(compiler.is_idle());
        assert_eq!(compiler.abandoned(), 2);
        compiler.compile(4, project("four").snapshot());
        assert!(compiler.held_since().is_some());
        assert_eq!(wait_for(&mut compiler).revision, 4);
        until_all_ended(&mut compiler);
        assert!(compiler.poll().is_none());
    }

    #[test]
    fn stopping_drops_the_compile_and_what_waits_for_it() {
        let mut compiler =
            Compiler::spawn(1, || {}).unwrap_or_else(|err| panic!("thread spawns: {err}"));
        assert_eq!(compiler.abandon(), None);
        compiler.compile(1, slow(SLOW));
        until_running(&compiler);
        compiler.compile(2, project("two").snapshot());
        assert!(!compiler.is_idle());
        let ran = compiler.abandon();
        assert!(ran.is_some());
        assert!(compiler.is_idle());
        assert_eq!(compiler.running(), None);
        until_all_ended(&mut compiler);
        assert!(compiler.poll().is_none());
        // What is asked afterwards is compiled as ever.
        compiler.compile(3, project("three").snapshot());
        assert_eq!(wait_for(&mut compiler).revision, 3);
    }

    #[test]
    fn dropping_does_not_wait_for_a_compile_that_runs_long() {
        let ended = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&ended);
        let mut compiler = Compiler::spawn(1, move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap_or_else(|err| panic!("thread spawns: {err}"));
        compiler.compile(1, slow(SLOW * 2));
        until_running(&compiler);
        let dropping = Instant::now();
        drop(compiler);
        let dropped = dropping.elapsed();
        assert!(
            dropped < EXIT_WAIT + Duration::from_millis(100),
            "dropping took {dropped:?}"
        );
        // The compile was not over: it ends by itself, and says so.
        assert_eq!(ended.load(Ordering::SeqCst), 0);
        let deadline = Instant::now() + Duration::from_secs(30);
        while ended.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < deadline, "the compile never ended");
            thread::sleep(Duration::from_millis(5));
        }
    }
}
