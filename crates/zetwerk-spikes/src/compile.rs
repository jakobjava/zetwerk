//! Spike 1: what does one keystroke cost if it triggers a compile?

use crate::support::{self, Cost, ms, percentile};

/// Single-character edits measured per configuration.
const EDITS: usize = 30;

pub fn run() {
    println!("\n== Spike 1: incremental compile per single-character edit ==");
    println!("   ({EDITS} edits in the middle of the document, embedded fonts)\n");
    println!(
        "{:<26} {:>5} {:>7} {:>9} {:>9} {:>9} {:>9} {:>8}",
        "document", "pages", "threads", "cold ms", "p50 ms", "p90 ms", "cpu p50", "cpu/wall"
    );
    let env = support::environment();
    for doc in support::offline_docs() {
        for threads in [0, 1, 2, 4] {
            // A fresh memoization cache per configuration, so that "cold"
            // means cold and configurations do not help each other.
            comemo::evict(0);
            let Some(row) = measure(&env, &doc.text, threads) else {
                println!("{:<26} failed to compile", doc.name);
                break;
            };
            let threads = if threads == 0 {
                "all".to_owned()
            } else {
                threads.to_string()
            };
            println!(
                "{:<26} {:>5} {:>7} {:>9.1} {:>9.1} {:>9.1} {:>9.1} {:>8.1}",
                doc.name,
                row.pages,
                threads,
                ms(row.cold.wall),
                row.wall_p50,
                row.wall_p90,
                row.cpu_p50,
                row.cpu_p50 / row.wall_p50,
            );
        }
    }
}

struct Row {
    pages: usize,
    cold: Cost,
    wall_p50: f64,
    wall_p90: f64,
    cpu_p50: f64,
}

/// Compiles `text` cold, then after each of [`EDITS`] one-character
/// insertions. `threads == 0` uses rayon's default pool.
fn measure(
    env: &std::sync::Arc<zetwerk_typst::Environment>,
    text: &str,
    threads: usize,
) -> Option<Row> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .ok()?;
    let mut project = support::project(env, text);
    let main = project.main();

    let (document, cold) = support::measure(|| pool.install(|| support::compile(&project)));
    let pages = document?.pages().len();

    // Type into the first ordinary word after the middle of the document.
    let middle = text.len() / 2;
    let anchor = middle + text.get(middle..)?.find(" the ")? + " the".len();

    let mut walls = Vec::with_capacity(EDITS);
    let mut cpus = Vec::with_capacity(EDITS);
    for index in 0..EDITS {
        project.edit(main, anchor + index..anchor + index, "x")?;
        let (document, cost) = support::measure(|| pool.install(|| support::compile(&project)));
        document?;
        walls.push(ms(cost.wall));
        cpus.push(ms(cost.cpu));
    }
    Some(Row {
        pages,
        cold,
        wall_p50: percentile(&mut walls, 50.0),
        wall_p90: percentile(&mut walls, 90.0),
        cpu_p50: percentile(&mut cpus, 50.0),
    })
}
