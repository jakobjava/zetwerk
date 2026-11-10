//! Rasterizing tiles on background threads.

use std::collections::{HashSet, VecDeque};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};

use tiny_skia::Pixmap;

use crate::{Tile, TileKey};

/// A rasterized tile.
pub struct Rasterized {
    pub key: TileKey,
    pub pixmap: Pixmap,
}

/// A pool of threads that turn [`Tile`]s into pixels.
///
/// The pool works through the tiles most recently asked for and forgets
/// queued tiles that are no longer wanted, so scrolling quickly past a page
/// does not leave a backlog of tiles nobody will see.
pub struct Rasterizer {
    shared: Arc<Shared>,
    results: Receiver<Rasterized>,
    threads: Vec<JoinHandle<()>>,
}

struct Shared {
    state: Mutex<State>,
    work: Condvar,
}

#[derive(Default)]
struct State {
    queue: VecDeque<Tile>,
    /// Tiles a thread is rasterizing right now.
    running: HashSet<TileKey>,
    closed: bool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // The state is only ever mutated in single, complete steps, so it
        // is consistent even if a thread panicked while holding the lock.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Rasterizer {
    /// Starts `threads` rasterizer threads. `wake` is called from one of
    /// them whenever a tile is ready to be collected with
    /// [`Rasterizer::poll`].
    pub fn spawn(threads: usize, wake: impl Fn() + Send + Sync + 'static) -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            work: Condvar::new(),
        });
        let (result_tx, results) = mpsc::channel();
        let wake = Arc::new(wake);
        let threads = (0..threads.max(1))
            .map(|index| {
                let shared = Arc::clone(&shared);
                let result_tx = result_tx.clone();
                let wake = Arc::clone(&wake);
                thread::Builder::new()
                    .name(format!("zetwerk-raster-{index}"))
                    .spawn(move || {
                        while let Some(tile) = next(&shared) {
                            let pixmap = tile.render();
                            shared.lock().running.remove(&tile.key);
                            if result_tx
                                .send(Rasterized {
                                    key: tile.key,
                                    pixmap,
                                })
                                .is_err()
                            {
                                break;
                            }
                            wake();
                        }
                    })
            })
            .collect::<std::io::Result<Vec<_>>>()?;
        Ok(Self {
            shared,
            results,
            threads,
        })
    }

    /// Replaces the set of tiles to rasterize with `wanted`, in order of
    /// priority. Tiles already being rasterized are not started twice.
    pub fn request(&self, wanted: impl IntoIterator<Item = Tile>) {
        let mut state = self.shared.lock();
        let mut seen = state.running.clone();
        state.queue = wanted
            .into_iter()
            .filter(|tile| seen.insert(tile.key))
            .collect();
        drop(state);
        self.shared.work.notify_all();
    }

    /// Whether `key` is queued or being rasterized.
    #[must_use]
    pub fn is_pending(&self, key: TileKey) -> bool {
        let state = self.shared.lock();
        state.running.contains(&key) || state.queue.iter().any(|tile| tile.key == key)
    }

    /// Whether nothing is queued or being rasterized.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        let state = self.shared.lock();
        state.running.is_empty() && state.queue.is_empty()
    }

    /// The tiles finished since the last call.
    pub fn poll(&self) -> Vec<Rasterized> {
        self.results.try_iter().collect()
    }
}

/// Blocks until there is a tile to rasterize. `None` when shutting down.
fn next(shared: &Shared) -> Option<Tile> {
    let mut state = shared.lock();
    loop {
        if state.closed {
            return None;
        }
        if let Some(tile) = state.queue.pop_front() {
            state.running.insert(tile.key);
            return Some(tile);
        }
        state = shared
            .work
            .wait(state)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

impl Drop for Rasterizer {
    fn drop(&mut self) {
        {
            let mut state = self.shared.lock();
            state.closed = true;
            state.queue.clear();
        }
        self.shared.work.notify_all();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}
