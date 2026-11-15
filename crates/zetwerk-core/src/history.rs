//! The undo tree.
//!
//! Every state the text has been in is a node; the changes that led from a
//! node's parent to it hang on the node. Undo walks to the parent, redo to
//! the child most recently left. Editing after an undo starts a new branch
//! and leaves the old one in the tree, so nothing that was ever typed is
//! out of reach.

use std::ops::Range;

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

impl Step<'_> {
    /// The part of the text, as it is after the step, that the step
    /// replaced: from the first byte any of its changes touched to the last.
    /// A step that changed nothing still has a place, and gives the empty
    /// range there. `None` for a step without changes.
    #[must_use]
    pub fn extent(&self) -> Option<Range<usize>> {
        let ordered: Vec<&Change> = match self.direction {
            Direction::Forward => self.changes.iter().collect(),
            Direction::Backward => self.changes.iter().rev().collect(),
        };
        let mut extent: Option<Range<usize>> = None;
        for change in ordered {
            let (from, to) = match self.direction {
                Direction::Forward => (change.old.len(), change.new.len()),
                Direction::Backward => (change.new.len(), change.old.len()),
            };
            let (start, end) = match &extent {
                None => (change.at, change.at + from),
                Some(so_far) => (
                    so_far.start.min(change.at),
                    so_far.end.max(change.at + from),
                ),
            };
            extent = Some(start..end - from + to);
        }
        extent
    }

    /// The stretches of the text, as it is after the step, that the step
    /// made different: what to show someone who asks what it did. They
    /// are in the order of the text and apart from each other.
    ///
    /// A stretch holds text the step put there. Where the step only took
    /// text away, the stretch is empty and stands at the place the text
    /// was taken from. What a change replaced by the same (the letters a
    /// replacement has in common with what it replaced, at its start and
    /// at its end) is not part of a stretch; changes that touch each
    /// other are one stretch.
    #[must_use]
    pub fn ranges(&self) -> Vec<Range<usize>> {
        self.ranges_with(Vec::new())
    }

    /// Like [`Step::ranges`], for the last of several steps taken one
    /// after the other: `earlier` are the stretches the steps before it
    /// made different, in the text as it was before this step (in order
    /// and apart, as `ranges` gives them). They are moved along with the
    /// text and joined with what this step changed.
    #[must_use]
    pub fn ranges_with(&self, earlier: Vec<Range<usize>>) -> Vec<Range<usize>> {
        let ordered: Vec<&Change> = match self.direction {
            Direction::Forward => self.changes.iter().collect(),
            Direction::Backward => self.changes.iter().rev().collect(),
        };
        let mut ranges = earlier;
        for change in ordered {
            let (gone, come) = match self.direction {
                Direction::Forward => (change.old.as_str(), change.new.as_str()),
                Direction::Backward => (change.new.as_str(), change.old.as_str()),
            };
            let (same_start, same_end) = in_common(gone, come);
            let start = change.at + same_start;
            let (gone, come) = (
                gone.len() - same_start - same_end,
                come.len() - same_start - same_end,
            );
            if gone == 0 && come == 0 {
                continue;
            }
            // What was replaced, in the text before the change, grown by
            // every stretch that reaches it: they become one.
            let mut joined = start..start + gone;
            let mut after = Vec::new();
            let mut moved = Vec::with_capacity(ranges.len() + 1);
            for range in ranges {
                if range.end < joined.start {
                    moved.push(range);
                } else if range.start > start + gone {
                    after.push(range.start + come - gone..range.end + come - gone);
                } else {
                    joined = joined.start.min(range.start)..joined.end.max(range.end);
                }
            }
            moved.push(joined.start..joined.end + come - gone);
            moved.append(&mut after);
            ranges = moved;
        }
        ranges
    }
}

/// How many bytes `a` and `b` have in common at their start, and how many
/// of the rest at their end, counting whole characters only.
fn in_common(a: &str, b: &str) -> (usize, usize) {
    let mut start = a.bytes().zip(b.bytes()).take_while(|(a, b)| a == b).count();
    while !(a.is_char_boundary(start) && b.is_char_boundary(start)) {
        start -= 1;
    }
    let (a, b) = (&a[start..], &b[start..]);
    let mut end = a
        .bytes()
        .rev()
        .zip(b.bytes().rev())
        .take_while(|(a, b)| a == b)
        .count();
    while !(a.is_char_boundary(a.len() - end) && b.is_char_boundary(b.len() - end)) {
        end -= 1;
    }
    (start, end)
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

    /// The steps that lead from `from` to `to`, in the order they are to
    /// be taken: those of [`History::path`], each with its changes and
    /// the way they go. `None` if either state does not exist.
    ///
    /// This only reads: it is how to tell what the text of another state
    /// is, or what going there would change, without going there.
    #[must_use]
    pub fn steps(&self, from: StateId, to: StateId) -> Option<Vec<Step<'_>>> {
        let mut at = from;
        self.path(from, to)?
            .into_iter()
            .map(|next| {
                let step = if self.parent(at) == Some(next) {
                    Step {
                        changes: self.changes(at)?,
                        direction: Direction::Backward,
                        to: next,
                    }
                } else {
                    Step {
                        changes: self.changes(next)?,
                        direction: Direction::Forward,
                        to: next,
                    }
                };
                at = next;
                Some(step)
            })
            .collect()
    }

    /// The line of history the text is on, oldest first: the states from
    /// the root to the current one, which undo walks back through, and
    /// from there on those that redo leads to. Every other state is on a
    /// branch: text that was undone and then changed differently.
    #[must_use]
    pub fn line(&self) -> Vec<StateId> {
        let mut line = vec![self.current];
        let mut at = self.current;
        while let Some(parent) = self.parent(at) {
            line.push(parent);
            at = parent;
        }
        line.reverse();
        at = self.current;
        while let Some(next) = self.redo_target(at) {
            line.push(next);
            at = next;
        }
        line
    }

    /// The history with fewer, coarser steps: each state that `keep` does
    /// not keep and that has exactly one child is left out, and its child
    /// takes over its changes. The root, the current state, the states
    /// where branches part and the ends of branches always stay, and so
    /// does every text that stays reachable: only the steps in between
    /// go. Returns the history and, by old id, the new id of each state
    /// that stayed; the order of the states is kept.
    #[must_use]
    pub fn thinned(&self, keep: impl Fn(StateId) -> bool) -> (Self, Vec<Option<StateId>>) {
        let len = self.nodes.len();
        let mut children = vec![0_usize; len];
        let mut only_child = vec![None; len];
        for (id, node) in self.nodes.iter().enumerate() {
            if let Some(parent) = node.parent.and_then(|parent| usize::try_from(parent).ok()) {
                children[parent] += 1;
                only_child[parent] = Some(id);
            }
        }
        let stays = |id: usize| {
            id == 0 || id as StateId == self.current || children[id] != 1 || keep(id as StateId)
        };
        let mut nodes: Vec<Node> = Vec::with_capacity(len);
        let mut map = vec![None; len];
        // The new id of the nearest state that stays, going up from each
        // state, and the changes from there that a state left out passes
        // on to its child.
        let mut anchor: Vec<StateId> = vec![ROOT; len];
        let mut carried: Vec<Vec<Change>> = vec![Vec::new(); len];
        for (id, node) in self.nodes.iter().enumerate() {
            let Some(parent) = node.parent.and_then(|parent| usize::try_from(parent).ok()) else {
                map[id] = Some(nodes.len() as StateId);
                anchor[id] = nodes.len() as StateId;
                nodes.push(Node {
                    parent: None,
                    changes: node.changes.clone(),
                    redo: None,
                });
                continue;
            };
            let mut changes = std::mem::take(&mut carried[parent]);
            for change in &node.changes {
                let merged = changes.last_mut().is_some_and(|last| last.absorb(change));
                if !merged {
                    changes.push(change.clone());
                }
            }
            if stays(id) {
                let new = nodes.len() as StateId;
                nodes.push(Node {
                    parent: Some(anchor[parent]),
                    changes,
                    redo: None,
                });
                map[id] = Some(new);
                anchor[id] = new;
            } else {
                anchor[id] = anchor[parent];
                carried[id] = changes;
            }
        }
        // Redo leads where it led, to the first state on the way that
        // stayed.
        for (id, node) in self.nodes.iter().enumerate() {
            let (Some(new), Some(mut target)) = (map[id], node.redo) else {
                continue;
            };
            while let Some(at) = usize::try_from(target).ok().filter(|&at| !stays(at)) {
                let Some(next) = only_child[at] else {
                    break;
                };
                target = next as StateId;
            }
            let target = usize::try_from(target).ok().and_then(|at| map[at]);
            if let Some(node) = usize::try_from(new).ok().and_then(|new| nodes.get_mut(new)) {
                node.redo = target;
            }
        }
        let current = usize::try_from(self.current)
            .ok()
            .and_then(|current| map[current])
            .unwrap_or(ROOT);
        (Self { nodes, current }, map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_knows_what_part_of_the_text_it_replaces() {
        let change = |at, old: &str, new: &str| Change {
            at,
            old: old.to_owned(),
            new: new.to_owned(),
        };
        // "abcdef" -> "aXXcdef" -> "aXXcdYf"
        let changes = [change(1, "b", "XX"), change(5, "e", "Y")];
        let forward = Step {
            changes: &changes,
            direction: Direction::Forward,
            to: 1,
        };
        assert_eq!(forward.extent(), Some(1..6));
        let backward = Step {
            changes: &changes,
            direction: Direction::Backward,
            to: 0,
        };
        assert_eq!(backward.extent(), Some(1..5));
        // A change of nothing still has a place.
        let nothing = [change(3, "", "")];
        let step = Step {
            changes: &nothing,
            direction: Direction::Backward,
            to: 0,
        };
        assert_eq!(step.extent(), Some(3..3));
    }

    #[test]
    fn a_step_knows_the_stretches_it_made_different() {
        let change = |at, old: &str, new: &str| Change {
            at,
            old: old.to_owned(),
            new: new.to_owned(),
        };
        let step = |changes, direction| Step {
            changes,
            direction,
            to: 0,
        };
        // "one two three four" -> "one 2 three four" -> "one 2 three"
        let changes = [change(4, "two", "2"), change(11, " four", "")];
        let forward = step(&changes, Direction::Forward);
        // What was put in, and the place something was taken from.
        assert_eq!(forward.ranges(), vec![4..5, 11..11]);
        let backward = step(&changes, Direction::Backward);
        assert_eq!(backward.ranges(), vec![4..7, 13..18]);
        assert_eq!(step(&[], Direction::Forward).ranges(), vec![]);

        // Typing on and deleting what was typed are one stretch; so are
        // changes that only touch.
        let changes = [
            change(3, "", "abc"),
            change(5, "c", ""),
            change(5, "", "d"),
            change(2, "x", ""),
        ];
        assert_eq!(step(&changes, Direction::Forward).ranges(), vec![2..5]);
        assert_eq!(step(&changes, Direction::Backward).ranges(), vec![2..3]);

        // What a replacement has in common with what it replaced did not
        // change, and a replacement by the same changed nothing.
        let changes = [change(0, "the grüne cat", "the große cat")];
        assert_eq!(step(&changes, Direction::Forward).ranges(), vec![6..9]);
        assert_eq!(step(&changes, Direction::Backward).ranges(), vec![6..9]);
        let changes = [change(2, "same", "same")];
        assert_eq!(step(&changes, Direction::Forward).ranges(), vec![]);
        // `é` and `è` share their first byte: characters are not cut.
        let changes = [change(0, "é", "è")];
        assert_eq!(step(&changes, Direction::Forward).ranges(), vec![0..2]);
    }

    #[test]
    fn stretches_of_earlier_steps_move_along_with_the_text() {
        let change = |at, old: &str, new: &str| Change {
            at,
            old: old.to_owned(),
            new: new.to_owned(),
        };
        // "abcdefgh": first "g" goes, then "XY" comes in before "c".
        let first = [change(6, "g", "")];
        let second = [change(2, "", "XY")];
        let step = |changes| Step {
            changes,
            direction: Direction::Forward,
            to: 0,
        };
        let ranges = step(&first).ranges();
        assert_eq!(ranges, vec![6..6]);
        assert_eq!(step(&second).ranges_with(ranges), vec![2..4, 8..8]);
        // A later step that reaches an earlier stretch joins it.
        let third = [change(4, "cdef", "")];
        assert_eq!(step(&third).ranges_with(vec![2..4, 8..8]), vec![2..4]);
    }

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

        // The same ways as steps: undone on the way up, made on the way
        // down, and nothing moved by asking.
        let steps = history.steps(c, e).unwrap_or_default();
        let ways: Vec<(Direction, StateId)> =
            steps.iter().map(|step| (step.direction, step.to)).collect();
        assert_eq!(
            ways,
            vec![
                (Direction::Backward, b),
                (Direction::Backward, a),
                (Direction::Forward, d),
                (Direction::Forward, e),
            ]
        );
        assert_eq!(steps[0].changes, history.changes(c).unwrap_or_default());
        assert_eq!(steps[2].changes, history.changes(d).unwrap_or_default());
        assert_eq!(history.steps(c, c), Some(vec![]));
        assert_eq!(history.steps(99, c), None);
        assert_eq!(history.current(), e);

        // The text is on the line through `d`; `b` and `c` are a branch.
        assert_eq!(history.line(), vec![ROOT, a, d, e]);
        assert!(history.step_to(d) && history.step_to(a));
        assert_eq!(history.line(), vec![ROOT, a, d, e], "redo leads back");
        assert!(history.set_redo_target(a, b));
        assert_eq!(history.line(), vec![ROOT, a, b, c]);
    }
}
