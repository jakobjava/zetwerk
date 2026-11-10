//! Running the Typst compiler without letting it take the editor down.

use std::panic::{AssertUnwindSafe, catch_unwind};

use typst::World;
use typst::diag::SourceDiagnostic;
use typst::ecow::EcoVec;
use typst_layout::PagedDocument;

/// How many compiles a memoized result may go unused before it is dropped.
/// Low enough to bound memory in a long session, high enough that undo/redo
/// and back-and-forth edits still hit the cache.
const CACHE_MAX_AGE: usize = 10;

/// The outcome of one compile.
pub struct Compiled {
    /// The laid-out document, or `None` if compilation failed.
    pub document: Option<PagedDocument>,
    /// Errors that prevented a document from being produced.
    pub errors: EcoVec<SourceDiagnostic>,
    /// Warnings, present on success and on failure.
    pub warnings: EcoVec<SourceDiagnostic>,
    /// Set if the compiler itself panicked. The document is `None` and the
    /// user's text is unaffected; this is a Typst bug, not a document error.
    pub panicked: bool,
}

/// Compiles `world` to pages.
///
/// Never panics: a panic inside Typst is caught and reported through
/// [`Compiled::panicked`], because losing the editor to a compiler bug would
/// mean losing the user's session.
#[must_use]
pub fn compile(world: &dyn World) -> Compiled {
    let result = catch_unwind(AssertUnwindSafe(|| typst::compile::<PagedDocument>(world)));
    let Ok(warned) = result else {
        // The memoization cache may hold results from the interrupted run;
        // drop all of it rather than trust it.
        comemo::evict(0);
        return Compiled {
            document: None,
            errors: EcoVec::new(),
            warnings: EcoVec::new(),
            panicked: true,
        };
    };
    comemo::evict(CACHE_MAX_AGE);
    let (document, errors) = match warned.output {
        Ok(document) => (Some(document), EcoVec::new()),
        Err(errors) => (None, errors),
    };
    Compiled {
        document,
        errors,
        warnings: warned.warnings,
        panicked: false,
    }
}
