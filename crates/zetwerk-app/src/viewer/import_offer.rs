//! A name Typst knows nothing of that a package on this machine has:
//! `#skeletize(…)` pasted from alchemist's manual into a document that
//! does not import alchemist, or imports it as a module only
//! (`#import "@preview/alchemist:0.2.0"`, as `:package add` writes it).
//!
//! The status line says which package has it, and the `:why` panel
//! offers `i`, which makes the document import it, in one undo step:
//!
//! - a package the document imports already, in the version it imports:
//!   the name is added to what the import names (`: a, b` gets `, c`);
//!   an import of the module that nothing uses as a module
//!   (`alchemist.…`) is made one of all it has (`: *`);
//! - else, the one package on this machine that has it, its newest
//!   version: `#import "…": *` among the document's imports, as its
//!   manual writes it, so that the other names of what was pasted are
//!   known too. Where several packages have it, none is chosen.
//!
//! Nothing is fetched or asked of the network: only the packages on the
//! machine are looked into. What a package has is read once for each
//! version (a version does not change) and only while Typst refuses a
//! name.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use typst::syntax::package::PackageSpec;
use typst::syntax::{FileId, LinkedNode, Side, Source, SyntaxKind};
use zetwerk_typst::ProjectWorld;
use zetwerk_typst::packages::{self as typst_packages, Import, Imported, PackageDirs, said};

use super::{Edited, Viewer};

/// What is kept between compiles.
#[derive(Default)]
pub(super) struct ImportOffers {
    /// Of each package's directory, the names it has.
    exports: HashMap<PathBuf, BTreeSet<String>>,
    /// Of the first thing Typst refuses, if `i` imports it.
    offer: Option<Offer>,
}

/// What `i` does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Offer {
    /// Where the name Typst refuses is.
    file: FileId,
    range: Range<usize>,
    name: String,
    /// The package that has it.
    spec: PackageSpec,
    /// What is written, and where.
    at: usize,
    insert: String,
    /// What the panel says `i` does.
    pub(super) does: String,
}

/// Where a line of its own that imports a package goes in `text`, whose
/// imports are `imports`: behind the last import that has a line of its
/// own, else at the top. And whether a line break goes before it.
pub(super) fn import_place(text: &str, imports: &[Import]) -> (usize, &'static str) {
    let at = imports
        .iter()
        .filter(|import| {
            let start = text[..import.literal.start]
                .rfind('\n')
                .map_or(0, |at| at + 1);
            text[start..].starts_with("#import")
        })
        .map(|import| {
            text[import.literal.end..]
                .find('\n')
                .map_or(text.len(), |end| import.literal.end + end + 1)
        })
        .max()
        .unwrap_or(0);
    let newline = if at > 0 && !text[..at].ends_with('\n') {
        "\n"
    } else {
        ""
    };
    (at, newline)
}

/// The import of `source` whose string is at `literal`.
fn import_node<'a>(source: &'a Source, literal: &Range<usize>) -> Option<LinkedNode<'a>> {
    let leaf = LinkedNode::new(source.root()).leaf_at(literal.start + 1, Side::After)?;
    std::iter::successors(Some(leaf), |node| node.parent().cloned())
        .find(|node| node.kind() == SyntaxKind::ModuleImport)
}

impl ImportOffers {
    /// Whether the package in `dir` has `name`.
    fn has(&mut self, dir: &Path, name: &str) -> bool {
        self.exports
            .entry(dir.to_owned())
            .or_insert_with(|| {
                typst_packages::manifest(dir).map_or_else(
                    |_| BTreeSet::new(),
                    |manifest| typst_packages::exports(dir, &manifest.package.entrypoint),
                )
            })
            .contains(name)
    }
}

/// What `i` writes: of which package, where, what, and what the panel
/// says it does.
type Writing = (PackageSpec, usize, String, String);

/// What the imports of a text make of a name a package has.
enum InImports {
    /// None of the packages imported has it.
    Nowhere,
    /// One has it, and its import cannot be made to name it.
    Stuck,
    /// One has it: this makes its import name it.
    Writes(Writing),
}

impl ImportOffers {
    /// Of the packages `source` imports (`imports`), the first that has
    /// `name`, and what makes the import name it.
    fn in_imports(
        &mut self,
        source: &Source,
        imports: &[Import],
        dirs: &PackageDirs,
        name: &str,
    ) -> InImports {
        let text = source.text();
        for import in imports {
            let Some((dir, _)) = typst_packages::find(dirs, &import.spec) else {
                continue;
            };
            if !self.has(&dir, name) {
                continue;
            }
            let spec = &import.spec;
            let shown = said(spec);
            let Some(node) = import_node(source, &import.literal) else {
                return InImports::Stuck;
            };
            let writing = match &import.names {
                Imported::All => None,
                Imported::Items(_) => node
                    .children()
                    .find(|child| child.kind() == SyntaxKind::ImportItems)
                    .map(|list| {
                        let before = text[..list.range().end].trim_end();
                        let comma = if before.ends_with(',') { " " } else { ", " };
                        (
                            spec.clone(),
                            before.len(),
                            format!("{comma}{name}"),
                            format!("add “{name}” to what the import of {shown} names"),
                        )
                    }),
                Imported::Module(module) => {
                    let as_module =
                        typst_packages::used(source.root(), import, &self.exports[&dir]);
                    let renamed = *module != spec.name.as_str();
                    Some(if as_module.is_empty() && !renamed {
                        (
                            spec.clone(),
                            node.range().end,
                            ": *".to_owned(),
                            format!("import all {shown} has (“: *” after its #import)"),
                        )
                    } else {
                        let (at, newline) = import_place(text, imports);
                        (
                            spec.clone(),
                            at,
                            format!("{newline}#import \"{spec}\": {name}\n"),
                            format!("#import \"{spec}\": {name}"),
                        )
                    })
                }
            };
            return writing.map_or(InImports::Stuck, InImports::Writes);
        }
        InImports::Nowhere
    }
}

impl Viewer {
    /// A compile made of `world` landed: what `i` would import, if the
    /// first thing Typst refuses is a name a package here has.
    pub(super) fn import_offer_compiled(&mut self, world: &ProjectWorld) {
        self.import_offers.offer = None;
        self.import_offers.offer = self.find_import_offer(world);
    }

    fn find_import_offer(&mut self, world: &ProjectWorld) -> Option<Offer> {
        let refusal = self.refusal()?;
        let (file, range) = (refusal.file(), refusal.range()?.clone());
        let name = self
            .diagnostics
            .iter()
            .find(|(of, at, error, _)| *error && *of == file && *at == range)?
            .3
            .strip_prefix("unknown variable: ")?
            .to_owned();
        let source = world.written(file).ok()?;
        if source.text().get(range.clone())? != name {
            return None;
        }
        let text = source.text();
        let imports = typst_packages::imports(source.root());
        let dirs = self.project.packages().clone();
        let offer = |spec: &PackageSpec, at: usize, insert: String, does: String| Offer {
            file,
            range: range.clone(),
            name: name.clone(),
            spec: spec.clone(),
            at,
            insert,
            does,
        };
        // A package imported already: that one, or none.
        match self
            .import_offers
            .in_imports(&source, &imports, &dirs, &name)
        {
            InImports::Nowhere => {}
            InImports::Stuck => return None,
            InImports::Writes((spec, at, insert, does)) => {
                return Some(offer(&spec, at, insert, does));
            }
        }
        // The one package here that has it, in its newest version.
        let imported: HashSet<(String, String)> = imports
            .iter()
            .map(|import| {
                (
                    import.spec.namespace.to_string(),
                    import.spec.name.to_string(),
                )
            })
            .collect();
        let mut seen = HashSet::new();
        let mut having = Vec::new();
        for kept in typst_packages::on_machine(&dirs) {
            let key = (kept.spec.namespace.to_string(), kept.spec.name.to_string());
            if imported.contains(&key) || !seen.insert(key) {
                continue;
            }
            if self.import_offers.has(&kept.dir, &name) {
                having.push(kept.spec);
            }
        }
        let [spec] = having.as_slice() else {
            return None;
        };
        let (at, newline) = import_place(text, &imports);
        Some(offer(
            spec,
            at,
            format!("{newline}#import \"{spec}\": *\n"),
            format!("#import \"{spec}\": *"),
        ))
    }

    /// What `i` would import, where it is of the text being edited and
    /// of the first thing Typst refuses.
    pub(super) fn import_offered(&self) -> Option<&Offer> {
        let offer = self.import_offers.offer.as_ref()?;
        let editor = self.editor.as_ref()?;
        let refusal = self.refusal()?;
        (editor.file() == offer.file
            && refusal.file() == offer.file
            && refusal.range() == Some(&offer.range))
        .then_some(offer)
    }

    /// What the status line and the panel add to what Typst says, where
    /// a package has the name it refuses.
    pub(super) fn import_said(&self, file: FileId, range: &Range<usize>) -> Option<String> {
        let offer = self.import_offers.offer.as_ref()?;
        (offer.file == file && offer.range == *range)
            .then(|| format!("{} has it", said(&offer.spec)))
    }

    /// `i`: imports the name Typst refuses. Returns whether the text
    /// changed.
    pub(super) fn import_refused(&mut self) -> bool {
        let Some(offer) = self.import_offered().cloned() else {
            return false;
        };
        let Some(editor) = &mut self.editor else {
            return false;
        };
        let cursor = editor.cursor();
        if let Err(why) = editor.replace(&[(offer.at..offer.at, offer.insert.as_str())]) {
            self.alert(format!("not changed: {why}"));
            return false;
        }
        // The cursor stays by what it was by.
        let cursor = if offer.at <= cursor {
            cursor + offer.insert.len()
        } else {
            cursor
        };
        editor.set_cursor(cursor.min(editor.text().len()));
        self.text_changed(Edited::Command);
        self.import_offers.offer = None;
        self.notice(format!(
            "“{}” imported from {}  (u takes it back)",
            offer.name,
            said(&offer.spec)
        ));
        true
    }
}
