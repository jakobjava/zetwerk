//! Running the Typst compiler without letting it take the editor down.

use std::panic::{AssertUnwindSafe, catch_unwind};

use typst::World;
use typst::diag::SourceDiagnostic;
use typst::ecow::EcoVec;
use typst_layout::PagedDocument;

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
///
/// Typst remembers intermediate results in a process-wide cache, which is
/// what makes the next compile incremental. This function does not prune
/// that cache; see [`forget_unused`].
#[must_use]
pub fn compile(world: &dyn World) -> Compiled {
    let result = catch_unwind(AssertUnwindSafe(|| typst::compile::<PagedDocument>(world)));
    let Ok(warned) = result else {
        // The cache may hold results from the interrupted run; drop all of
        // it rather than trust it.
        comemo::evict(0);
        return Compiled {
            document: None,
            errors: EcoVec::new(),
            warnings: EcoVec::new(),
            panicked: true,
        };
    };
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

/// Drops cached results that the last `rounds` calls of this function found
/// unused, to bound the memory a long session takes.
///
/// The cache and its notion of age are shared by everything compiled in the
/// process: the document and the editor's own interface, which is typeset
/// by Typst too. Every call ages every unused result, whoever makes it. A
/// caller that compiles often must therefore pass a large `rounds`, or it
/// ages the document's results away between two document compiles and the
/// next one starts from nothing.
pub fn forget_unused(rounds: usize) {
    comemo::evict(rounds);
}

/// The compiled `document` as the bytes of a PDF file, or what kept it
/// from becoming one, said with the help of `world`.
///
/// Like [`compile`], it never panics.
pub fn export_pdf(document: &PagedDocument, world: &dyn World) -> Result<Vec<u8>, Vec<String>> {
    let options = typst_pdf::PdfOptions::default();
    let exported = catch_unwind(AssertUnwindSafe(|| typst_pdf::pdf(document, &options)));
    match exported {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(errors)) => Err(errors
            .iter()
            .map(|error| crate::describe(world, error))
            .collect()),
        Err(_) => Err(vec!["the PDF exporter crashed on this document".to_owned()]),
    }
}
