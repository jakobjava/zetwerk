//! Turning compiler diagnostics into text a person can act on.

use std::fmt::Write;

use typst::diag::{Severity, SourceDiagnostic};
use typst::syntax::{DiagSpan, VirtualRoot};
use typst::{World, WorldExt};

/// Formats a diagnostic as `path:line:column: severity: message`, followed by
/// one indented line per hint. Lines and columns are 1-based. The location is
/// omitted when the diagnostic does not point into a file.
#[must_use]
pub fn describe(world: &dyn World, diagnostic: &SourceDiagnostic) -> String {
    let severity = match diagnostic.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    };
    let mut out = String::new();
    if let Some(location) = locate(world, diagnostic.span) {
        let _ = write!(out, "{location}: ");
    }
    let _ = write!(out, "{severity}: {}", diagnostic.message);
    for hint in &diagnostic.hints {
        let _ = write!(out, "\n  hint: {}", hint.v);
    }
    out
}

fn locate(world: &dyn World, span: DiagSpan) -> Option<String> {
    let id = span.id()?;
    let range = world.range(span)?;
    let source = world.source(id).ok()?;
    let (line, column) = source.lines().byte_to_line_column(range.start)?;
    let path = id.vpath().get_without_slash();
    Some(match id.root() {
        VirtualRoot::Package(package) => {
            format!("{package}/{path}:{}:{}", line + 1, column + 1)
        }
        VirtualRoot::Project => format!("{path}:{}:{}", line + 1, column + 1),
    })
}
