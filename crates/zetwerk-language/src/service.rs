//! Checking in the background.
//!
//! One thread owns the [`Checker`]: the dictionary, the cache, the
//! connection to a LanguageTool server. It sleeps until it is asked
//! something and wakes whoever asked when it has an answer, as the
//! compile thread does; nothing is polled and no timer runs.

use std::cell::RefCell;
use std::collections::{BTreeSet, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use crate::Settings;
use crate::checker::{Checked, Checker, Job, Scope};
use crate::dictionary::Speller;

/// Something asked of the thread.
enum Message {
    /// Go by these settings from now on.
    Settings(Settings),
    /// These are the user's own words from now on.
    Words(Arc<BTreeSet<String>>),
    /// Check this; the answers go to whoever holds the [`Service`].
    Check(Job),
    /// Check this, whatever else is asked meanwhile, and send the last
    /// answer here.
    Now(Job, Sender<Checked>),
}

/// The dictionary the thread last checked with, for whoever wants to
/// ask it something at once.
type Shared = Arc<Mutex<Option<Arc<Speller>>>>;

/// A background thread that checks sources.
pub struct Service {
    requests: Option<Sender<Message>>,
    results: Receiver<Checked>,
    speller: Shared,
    thread: Option<JoinHandle<()>>,
}

/// A way to ask a [`Service`] from another thread and wait there for
/// the answer.
#[derive(Clone)]
pub struct Handle {
    requests: Sender<Message>,
}

impl Handle {
    /// Has `job` done and waits for its last answer. `None` if the
    /// service is gone.
    #[must_use]
    pub fn check(&self, job: Job) -> Option<Checked> {
        let (reply, answer) = mpsc::channel();
        self.requests.send(Message::Now(job, reply)).ok()?;
        answer.recv().ok()
    }
}

/// Whether a job that was asked for makes `job`, which is under way,
/// pointless: it is for another text, or what a window shows is asked
/// for again. A window that moves does not stop a search the user
/// asked for.
fn overtakes(newer: &Job, job: &Job) -> bool {
    let shown = |job: &Job| matches!(job.scope, Scope::Window(_));
    newer.text != job.text || shown(job) || !shown(newer)
}

impl Service {
    /// Starts the thread, which goes by `settings`. `wake` is called
    /// from it whenever an answer is ready to be collected with
    /// [`Service::poll`].
    pub fn spawn(settings: Settings, wake: impl Fn() + Send + 'static) -> std::io::Result<Self> {
        let (request_tx, request_rx) = mpsc::channel::<Message>();
        let (result_tx, result_rx) = mpsc::channel();
        let speller = Shared::default();
        let shared = Arc::clone(&speller);
        let thread = thread::Builder::new()
            .name("zetwerk-language".into())
            .spawn(move || {
                let mut checker = Checker::new(settings);
                // What was asked while something else was being done.
                let waiting: RefCell<VecDeque<Message>> = RefCell::default();
                loop {
                    let next = waiting.borrow_mut().pop_front();
                    let Some(message) = next.or_else(|| request_rx.recv().ok()) else {
                        break;
                    };
                    match message {
                        Message::Settings(settings) => checker.configure(settings),
                        Message::Words(words) => checker.set_words(words),
                        Message::Now(job, reply) => {
                            checker.check(&job, &|| false, &mut |checked| {
                                if checked.done {
                                    let _ = reply.send(checked);
                                }
                            });
                        }
                        Message::Check(job) => {
                            let stop = || {
                                waiting.borrow_mut().extend(request_rx.try_iter());
                                waiting.borrow().iter().any(|message| match message {
                                    Message::Settings(_) | Message::Words(_) => true,
                                    Message::Check(newer) => overtakes(newer, &job),
                                    Message::Now(..) => false,
                                })
                            };
                            if stop() {
                                continue;
                            }
                            checker.check(&job, &stop, &mut |checked| {
                                if result_tx.send(checked).is_ok() {
                                    wake();
                                }
                            });
                        }
                    }
                    if let Ok(mut shared) = shared.lock() {
                        *shared = checker.speller();
                    }
                }
            })?;
        Ok(Self {
            requests: Some(request_tx),
            results: result_rx,
            speller,
            thread: Some(thread),
        })
    }

    fn send(&self, message: Message) {
        if let Some(requests) = &self.requests {
            // Sending only fails if the thread is gone, which shows as
            // answers no longer arriving.
            let _ = requests.send(message);
        }
    }

    /// Has the thread go by `settings` from now on.
    pub fn configure(&self, settings: Settings) {
        self.send(Message::Settings(settings));
    }

    /// Has it take `words` for the user's own words from now on.
    pub fn set_words(&self, words: Arc<BTreeSet<String>>) {
        self.send(Message::Words(words));
    }

    /// Queues `job`. A job for another text, or another look at what a
    /// window shows, takes the place of one that is under way.
    pub fn check(&self, job: Job) {
        self.send(Message::Check(job));
    }

    /// The answers that came since the last call, the oldest first.
    pub fn poll(&self) -> Vec<Checked> {
        self.results.try_iter().collect()
    }

    /// Waits for the next answer, however long it takes. For where
    /// nobody is woken: tests, a document without a window.
    #[must_use]
    pub fn wait(&self) -> Option<Checked> {
        self.results.recv().ok()
    }

    /// The dictionary spelling was last checked with: for asking it
    /// what a word may have been meant to be, at once.
    #[must_use]
    pub fn speller(&self) -> Option<Arc<Speller>> {
        self.speller.lock().ok().and_then(|speller| speller.clone())
    }

    /// A way to ask from another thread.
    #[must_use]
    pub fn handle(&self) -> Option<Handle> {
        let requests = self.requests.clone()?;
        Some(Handle { requests })
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        // Closing the channel ends the thread's loop after the paragraph
        // in progress.
        self.requests = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use typst_syntax::Source;

    use super::*;
    use crate::dictionary::tests::fixtures;

    fn settings() -> Settings {
        Settings {
            spelling: true,
            directories: vec![fixtures()],
            ..Settings::default()
        }
    }

    fn job(serial: u64, text: &str, scope: Scope) -> Job {
        Job {
            serial,
            text: serial,
            source: Source::detached(text),
            language: "en".to_owned(),
            scope,
            suggest: false,
        }
    }

    #[test]
    fn answers_wake_the_asker_and_are_collected() {
        let woken = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&woken);
        let service = Service::spawn(settings(), move || {
            count.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        assert!(service.poll().is_empty() && service.speller().is_none());
        service.check(job(1, "A wrnog word.", Scope::All));
        let checked = service.wait().unwrap();
        assert_eq!(
            (checked.serial, checked.findings.len(), checked.done),
            (1, 1, true)
        );
        assert_eq!(woken.load(Ordering::SeqCst), 1);
        // Once there is a dictionary it can be asked at once.
        let handle = service.handle().unwrap();
        assert!(
            handle
                .check(job(2, "Good.", Scope::All))
                .is_some_and(|c| c.findings.is_empty())
        );
        let speller = service.speller().unwrap();
        assert!(speller.suggest("helo", 5).contains(&"hello".to_owned()));
        // Answers asked for by a handle go to the handle alone.
        assert!(service.poll().is_empty());
    }

    #[test]
    fn settings_and_words_hold_for_what_is_asked_after_them() {
        let service = Service::spawn(Settings::default(), || {}).unwrap();
        let handle = service.handle().unwrap();
        let count = |serial| {
            let checked = handle.check(job(serial, "Typst is wrnog.", Scope::All));
            checked.unwrap().findings.len()
        };
        assert_eq!(count(1), 0);
        service.configure(settings());
        assert_eq!(count(2), 2);
        service.set_words(Arc::new(["Typst".to_owned()].into()));
        assert_eq!(count(3), 1);
    }

    #[test]
    fn a_newer_text_takes_the_place_of_an_older_one() {
        let window = |serial, text: u64| Job {
            text,
            ..job(serial, "A wrnog word.", Scope::Window(0..5))
        };
        let search = |serial, text: u64| Job {
            text,
            ..job(
                serial,
                "A wrnog word.",
                Scope::Next {
                    from: 0,
                    forward: true,
                },
            )
        };
        // Another text makes any job pointless.
        assert!(overtakes(&window(2, 9), &search(1, 1)));
        assert!(overtakes(&window(2, 9), &window(1, 1)));
        // The window moved: the old window is not looked at.
        assert!(overtakes(&window(2, 1), &window(1, 1)));
        // But a search the user asked for goes on.
        assert!(!overtakes(&window(2, 1), &search(1, 1)));
        assert!(overtakes(&search(2, 1), &search(1, 1)));
    }

    #[test]
    fn dropping_the_service_ends_its_thread() {
        let service = Service::spawn(settings(), || {}).unwrap();
        service.check(job(1, "Some text.", Scope::All));
        drop(service);
    }
}
