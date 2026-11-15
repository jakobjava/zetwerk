//! Sources an agent proposes, where the user wants agents' changes
//! proposed (`:agents propose`): nothing is written until the user takes
//! them. They are first in `:bib`, each a row of its own; Enter on one
//! adds it, or drops it. The agent may withdraw them as any proposal;
//! they are numbered with the proposals of text.

use super::Viewer;
use super::assist::Target;
use super::sources_add::Added;
use crate::bib::found::{self, Found};

/// Sources an agent proposed to add, at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposed {
    pub id: u64,
    /// As the agent gave them.
    pub text: String,
    /// Whether those that are there already are to be added all the
    /// same.
    pub anyway: bool,
    /// Who, when, what, of each: as they are listed.
    pub said: Vec<String>,
}

impl Viewer {
    /// Puts the sources `found` in `text` before the user, as proposal
    /// of their own.
    pub(super) fn propose_sources(&mut self, text: &str, anyway: bool, found: &[Found]) -> Added {
        let id = self.next_proposal;
        self.next_proposal += 1;
        let said: Vec<String> = found
            .iter()
            .map(|found| format!("{}  {}", found.entry.key, found.entry.line()))
            .collect();
        self.notice(format!(
            "an agent proposes {} for the bibliography: :bib shows it, to add or not",
            if said.len() == 1 {
                "a source".to_owned()
            } else {
                format!("{} sources", said.len())
            }
        ));
        self.sources_kept.proposed.push(Proposed {
            id,
            text: text.to_owned(),
            anyway,
            said,
        });
        Added {
            proposal: Some(id),
            ..Added::default()
        }
    }

    /// The sources agents proposed and the user has not decided on:
    /// their numbers and what they are, as given.
    #[must_use]
    pub fn proposed_sources(&self) -> Vec<(u64, String)> {
        let proposed = self.sources_kept.proposed.iter();
        proposed.map(|one| (one.id, one.text.clone())).collect()
    }

    /// Drops the proposal of sources `id`, for the agent that made it.
    /// Returns whether there was one.
    pub(super) fn withdraw_proposed_sources(&mut self, id: u64) -> bool {
        let before = self.sources_kept.proposed.len();
        self.sources_kept.proposed.retain(|one| one.id != id);
        self.sources_kept.proposed.len() < before
    }

    /// The rows of `:bib` for what agents propose.
    pub(super) fn proposed_rows(&self) -> Vec<(String, Target)> {
        let mut rows = Vec::new();
        for one in &self.sources_kept.proposed {
            for said in &one.said {
                rows.push((
                    format!("an agent proposes: {said}\tEnter: add it or not"),
                    Target::Command(format!("bib proposed {}", one.id)),
                ));
            }
        }
        rows
    }

    /// The commands about what agents propose: `proposed` (what can be
    /// done with one), `accept`, `reject`. Returns `None` where `what`
    /// is none of them.
    pub(super) fn proposed_bib_command(
        &mut self,
        what: &str,
        rest: &str,
    ) -> Option<Result<(), String>> {
        if !matches!(what, "proposed" | "accept" | "reject") {
            return None;
        }
        let Ok(id) = rest.trim().parse::<u64>() else {
            return Some(Err(format!("bib {what}: the number of the proposal")));
        };
        let at = self
            .sources_kept
            .proposed
            .iter()
            .position(|one| one.id == id);
        let Some(at) = at else {
            return Some(Err("that proposal is not there any more".to_owned()));
        };
        Some(match what {
            "proposed" => {
                let one = &self.sources_kept.proposed[at];
                let mut items: Vec<(String, Target)> = vec![
                    (
                        "add it to the bibliography".to_owned(),
                        Target::Command(format!("bib accept {id}")),
                    ),
                    (
                        "do not add it".to_owned(),
                        Target::Command(format!("bib reject {id}")),
                    ),
                ];
                items.extend(one.said.iter().map(|said| (said.clone(), Target::Nothing)));
                self.open_picker_for("an agent proposes", items, 0, "bib".to_owned(), None);
                Ok(())
            }
            "accept" => {
                let one = self.sources_kept.proposed.remove(at);
                let (found, _) = found::read(&one.text);
                self.add_agents_sources(&one.text, &found, one.anyway)
                    .and_then(|_| self.list_sources())
            }
            _ => {
                self.sources_kept.proposed.remove(at);
                self.notice("not added");
                self.list_sources()
            }
        })
    }
}
