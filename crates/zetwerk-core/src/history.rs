//! The undo tree.
//!
//! Every state the text has been in is a node; the changes that led from a
//! node's parent to it hang on the node. Undo walks to the parent, redo to
//! the child most recently left. Editing after an undo starts a new branch
//! and leaves the old one in the tree, so nothing that was ever typed is
//! out of reach.

use crate::change::{Change, Direction};

/// Identifies a state in the history. The root, the text as first loaded,
/// is state 0.
pub type StateId = u64;

pub const ROOT: StateId = 0;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Node {
    parent: Option<StateId>,
    /// What turned the parent's text into this state's text.
    changes: Vec<Change>,
    /// The child redo goes to: the one most recently created or left.
    redo: Option<StateId>,
}

/// A step from one state to a neighbouring one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step<'a> {
    /// The changes to apply in `direction`.
    pub changes: &'a [Change],
    pub direction: Direction,
    /// The state reached by the step.
    pub to: StateId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct History {
    nodes: Vec<Node>,
    current: StateId,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    /// A history with only the root state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            nodes: vec![Node {
                parent: None,
                changes: Vec::new(),
                redo: None,
            }],
            current: ROOT,
        }
    }

    fn node(&self, id: StateId) -> Option<&Node> {
        self.nodes.get(usize::try_from(id).ok()?)
    }

    fn node_mut(&mut self, id: StateId) -> Option<&mut Node> {
        self.nodes.get_mut(usize::try_from(id).ok()?)
    }

    #[must_use]
    pub fn current(&self) -> StateId {
        self.current
    }

    /// Number of states, the root included.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the history holds nothing but the root.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.len() <= 1
    }

    #[must_use]
    pub fn contains(&self, id: StateId) -> bool {
        self.node(id).is_some()
    }

    #[must_use]
    pub fn parent(&self, id: StateId) -> Option<StateId> {
        self.node(id)?.parent
    }

    /// The changes that led from `id`'s parent to `id`.
    #[must_use]
    pub fn changes(&self, id: StateId) -> Option<&[Change]> {
        Some(&self.node(id)?.changes)
    }

    /// The child of `id` that redo goes to, if it has children.
    #[must_use]
    pub fn redo_target(&self, id: StateId) -> Option<StateId> {
        self.node(id)?.redo
    }

    /// Makes `child` the state redo goes to from `id`. Returns `false`, and
    /// does nothing, if `child` is not a child of `id`.
    pub fn set_redo_target(&mut self, id: StateId, child: StateId) -> bool {
        if self.parent(child) != Some(id) {
            return false;
        }
        self.node_mut(id).is_some_and(|node| {
            node.redo = Some(child);
            true
        })
    }

    /// Whether the current state can take further changes instead of them
    /// forming a new state: it is not the root and nothing was built on it.
    #[must_use]
    pub fn can_extend(&self) -> bool {
        // A state gets a redo target with its first child and keeps one.
        self.current != ROOT && self.redo_target(self.current).is_none()
    }

    /// Records `changes` as a new state after the current one and moves to
    /// it. Returns the new state's id.
    pub fn push(&mut self, changes: Vec<Change>) -> StateId {
        let id = self.nodes.len() as StateId;
        self.nodes.push(Node {
            parent: Some(self.current),
            changes,
            redo: None,
        });
        if let Some(parent) = self.node_mut(self.current) {
            parent.redo = Some(id);
        }
        self.current = id;
        id
    }

    /// Adds `changes` to the current state, so that one undo takes them back
    /// together with what the state already holds. Returns `false`, and does
    /// nothing, if the current state cannot be extended.
    pub fn extend(&mut self, changes: Vec<Change>) -> bool {
        if !self.can_extend() {
            return false;
        }
        let current = self.current;
        self.node_mut(current).is_some_and(|node| {
            for change in changes {
                // Keep a run of keystrokes as one change, not hundreds.
                let merged = node
                    .changes
                    .last_mut()
                    .is_some_and(|last| last.absorb(&change));
                if !merged {
                    node.changes.push(change);
                }
            }
            true
        })
    }

    /// The step that undoes the current state, if it is not the root.
    #[must_use]
    pub fn undo_step(&self) -> Option<Step<'_>> {
        let node = self.node(self.current)?;
        Some(Step {
            changes: &node.changes,
            direction: Direction::Backward,
            to: node.parent?,
        })
    }

    /// The step that redoes what was last undone from the current state.
    #[must_use]
    pub fn redo_step(&self) -> Option<Step<'_>> {
        let to = self.node(self.current)?.redo?;
        Some(Step {
            changes: &self.node(to)?.changes,
            direction: Direction::Forward,
            to,
        })
    }

    /// Moves to a neighbouring state: the parent, or a child. Returns
    /// `false`, and stays, if `to` is neither. Moving to the parent makes
    /// the state left behind the one redo returns to.
    pub fn step_to(&mut self, to: StateId) -> bool {
        let from = self.current;
        if self.parent(from) == Some(to) {
            if let Some(parent) = self.node_mut(to) {
                parent.redo = Some(from);
            }
        } else if self.parent(to) != Some(from) {
            return false;
        }
        self.current = to;
        true
    }

    /// The states to step through to get from `from` to `to`, `to` included:
    /// up to their common ancestor, then down. `None` if either state does
    /// not exist.
    #[must_use]
    pub fn path(&self, from: StateId, to: StateId) -> Option<Vec<StateId>> {
        let ancestors = |mut id: StateId| -> Option<Vec<StateId>> {
            let mut chain = vec![id];
            while let Some(parent) = self.node(id)?.parent {
                chain.push(parent);
                id = parent;
            }
            Some(chain)
        };
        let (up, down) = (ancestors(from)?, ancestors(to)?);
        // Both chains end at the root; drop what they share, keeping the
        // meeting point at the end of `up`.
        let shared = up
            .iter()
            .rev()
            .zip(down.iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let mut path: Vec<StateId> = up[1..=up.len() - shared].to_vec();
        path.extend(down[..down.len() - shared].iter().rev());
        Some(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(text: &str) -> Vec<Change> {
        vec![Change {
            at: 0,
            old: String::new(),
            new: text.to_owned(),
        }]
    }

    #[test]
    fn undo_and_redo_walk_the_branch_last_used() {
        let mut history = History::new();
        assert!(history.undo_step().is_none() && history.redo_step().is_none());
        let a = history.push(change("a"));
        let b = history.push(change("b"));
        assert_eq!(history.undo_step().map(|step| step.to), Some(a));
        assert!(history.step_to(a));
        // A new edit after undo: a second branch from `a`.
        let c = history.push(change("c"));
        assert_eq!(history.parent(c), Some(a));
        assert!(history.step_to(a));
        assert_eq!(history.redo_step().map(|step| step.to), Some(c));
        // Visiting the old branch makes it the redo target again.
        assert!(history.step_to(b));
        assert!(history.step_to(a));
        assert_eq!(history.redo_step().map(|step| step.to), Some(b));
        assert_eq!(history.len(), 4);
    }

    #[test]
    fn only_neighbours_can_be_stepped_to() {
        let mut history = History::new();
        let a = history.push(change("a"));
        let b = history.push(change("b"));
        assert!(!history.step_to(ROOT));
        assert!(!history.step_to(b));
        assert!(!history.step_to(99));
        assert_eq!(history.current(), b);
        assert!(history.step_to(a) && history.step_to(ROOT));
    }

    #[test]
    fn a_state_with_children_or_the_root_cannot_be_extended() {
        let mut history = History::new();
        assert!(!history.extend(change("x")));
        let a = history.push(change("a"));
        assert!(history.extend(change("b")));
        // Both insert at 0, the second before the first: two changes.
        assert_eq!(history.changes(a).map(<[Change]>::len), Some(2));
        let typed_on = vec![Change {
            at: 1,
            old: String::new(),
            new: "c".into(),
        }];
        assert!(history.extend(typed_on));
        assert_eq!(
            history.changes(a).map(<[Change]>::len),
            Some(2),
            "merged into the last"
        );
        history.push(change("c"));
        assert!(history.step_to(a));
        assert!(!history.extend(change("d")));
    }

    #[test]
    #[allow(
        clippy::many_single_char_names,
        reason = "the states of the diagram below"
    )]
    fn paths_go_up_to_the_common_ancestor_and_down() {
        // root - a - b - c
        //          \ d - e
        let mut history = History::new();
        let a = history.push(change("a"));
        let b = history.push(change("b"));
        let c = history.push(change("c"));
        assert!(history.step_to(b) && history.step_to(a));
        let d = history.push(change("d"));
        let e = history.push(change("e"));

        assert_eq!(history.path(c, e), Some(vec![b, a, d, e]));
        assert_eq!(history.path(e, a), Some(vec![d, a]));
        assert_eq!(history.path(ROOT, c), Some(vec![a, b, c]));
        assert_eq!(history.path(c, c), Some(vec![]));
        assert_eq!(history.path(c, 99), None);
    }
}
