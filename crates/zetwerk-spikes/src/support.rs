//! Shared plumbing for the spikes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use typst_layout::PagedDocument;
use zetwerk_typst::{Environment, FontSet, Project};

/// A named corpus document.
pub struct Doc {
    pub name: &'static str,
    pub text: String,
}

/// The documents that need no network access.
pub fn offline_docs() -> Vec<Doc> {
    vec![
        Doc {
            name: "paper (2 p, ragged)",
            text: zetwerk_corpus::PAPER.to_owned(),
        },
        Doc {
            name: "article (1 p, justified)",
            text: zetwerk_corpus::ARTICLE.to_owned(),
        },
        Doc {
            name: "thesis-25 (justified)",
            text: zetwerk_corpus::thesis(25),
        },
        Doc {
            name: "thesis-60 (justified)",
            text: zetwerk_corpus::thesis(60),
        },
    ]
}

pub fn environment() -> Arc<Environment> {
    Environment::new(FontSet::Embedded)
}

/// Opens `text` as an in-memory project.
///
/// # Panics
/// If the fixed main file name were not a valid path, which it is.
pub fn project(env: &Arc<Environment>, text: &str) -> Project {
    let root = std::env::temp_dir();
    #[allow(clippy::expect_used, reason = "constant, valid file name")]
    Project::in_memory(Arc::clone(env), root, "main.typ", text.to_owned())
        .expect("main.typ is a valid virtual path")
}

/// Compiles the project, returning the document if it compiled.
pub fn compile(project: &Project) -> Option<PagedDocument> {
    let world = project.snapshot();
    let compiled = zetwerk_typst::compile(&world);
    if compiled.document.is_none() {
        for error in &compiled.errors {
            eprintln!("  {}", zetwerk_typst::describe(&world, error));
        }
    }
    compiled.document
}

/// Wall-clock and CPU time of `f`. CPU time is summed over all threads of
/// the process, so it shows what a compile costs the battery, not just how
/// long the user waits.
pub fn measure<T>(f: impl FnOnce() -> T) -> (T, Cost) {
    let cpu_before = process_cpu_time();
    let start = Instant::now();
    let value = f();
    let wall = start.elapsed();
    let cpu = process_cpu_time().saturating_sub(cpu_before);
    (value, Cost { wall, cpu })
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Cost {
    pub wall: Duration,
    pub cpu: Duration,
}

/// On-CPU time of every thread in this process, from the scheduler's
/// nanosecond counters. Linux only; zero elsewhere.
fn process_cpu_time() -> Duration {
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else {
        return Duration::ZERO;
    };
    let nanos: u64 = tasks
        .flatten()
        .filter_map(|task| std::fs::read_to_string(task.path().join("schedstat")).ok())
        .filter_map(|stat| stat.split_whitespace().next()?.parse::<u64>().ok())
        .sum();
    Duration::from_nanos(nanos)
}

pub fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// The `p`-th percentile (0–100) of `values`, by nearest rank.
pub fn percentile(values: &mut [f64], p: f64) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    values.sort_by(f64::total_cmp);
    let rank = ((p / 100.0) * (values.len() - 1) as f64).round() as usize;
    values[rank.min(values.len() - 1)]
}
