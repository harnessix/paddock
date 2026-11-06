//! The Accessibility mirror of the key guest window, the pure part: the
//! tree of [`Node`]s that the guest agent reads over Accessibility four
//! levels deep and sends in an `ax.tree` event, the cap that cuts whole
//! subtrees, the diff between two trees so the host rebuilds only the lists
//! that changed, the element ids that stay stable across a diff, and the
//! [`Mirror`] state of the host: the last tree, the id of each element, the
//! window it belongs to, and whether the agent is away. A press or a set
//! names an element id; an id that the last tree does not have is stale and
//! the request is dropped, so an element that VoiceOver pressed can never
//! press another control in the guest. Tests run on every system.
//!
//! `ax_host.rs` builds the `NSAccessibilityElement`s from the changes,
//! `src/guest/ax_tree.rs` reads, presses and sets in the guest.

use super::crop::Rect;
use super::proto::{Frame, Point};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How deep the guest reads: the children of the window, and three
/// levels below them.
pub const DEPTH: usize = 4;
/// The cap of an `ax.tree` event on the wire: the guest cuts whole
/// subtrees until the JSON of the nodes fits (a quarter of `LINE_MAX`).
pub const CAP: usize = 64 * 1024;
/// The most nodes the guest reads for one tree: each node is five
/// Accessibility calls, and a web view can hold thousands.
pub const MAX_NODES: usize = 1500;

/// One element of the mirror.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Node {
    /// The Accessibility role, `AXButton`, as the guest reports it. The
    /// host gives its element the same role.
    pub role: String,
    /// The title, or the description when there is no title.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// The value as text: the text of a field, `1` for a checked box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// The frame of the element relative to its window, in guest
    /// points, origin top left.
    pub frame: Frame,
    /// The indices of the element in the `AXChildren` arrays from the
    /// window down. `ax.press` and `ax.set` name it, and it equals the
    /// position of the node in this tree.
    pub path: Vec<usize>,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub focused: bool,
    /// The children, empty for a leaf and for a subtree that the cap or
    /// the depth cut.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Node {
    /// The nodes of a tree, the children included.
    pub fn count(nodes: &[Node]) -> usize {
        nodes.iter().map(|n| 1 + Node::count(&n.children)).sum()
    }

    /// The node at a position: the index in each list.
    pub fn at<'a>(nodes: &'a [Node], position: &[usize]) -> Option<&'a Node> {
        let (first, rest) = position.split_first()?;
        let node = nodes.get(*first)?;
        if rest.is_empty() {
            Some(node)
        } else {
            Node::at(&node.children, rest)
        }
    }

    /// The deepest node whose frame holds the point (relative to the
    /// window), for the hit test of the host window.
    pub fn hit(nodes: &[Node], p: Point) -> Option<&Node> {
        for n in nodes.iter().rev() {
            let f = n.frame;
            if p.x >= f.x && p.x < f.x + f.w && p.y >= f.y && p.y < f.y + f.h {
                return Node::hit(&n.children, p).or(Some(n));
            }
        }
        None
    }

    /// The deepest focused node.
    pub fn focused(nodes: &[Node]) -> Option<&Node> {
        for n in nodes {
            if let Some(f) = Node::focused(&n.children) {
                return Some(f);
            }
            if n.focused {
                return Some(n);
            }
        }
        None
    }

    /// A short name for a log line: the role and the title.
    pub fn name(&self) -> String {
        if self.title.is_empty() {
            self.role.clone()
        } else {
            format!("{} {}", self.role, self.title) // not user text
        }
    }
}

/// True when every node's path is its position, which the guest's press
/// and set depend on, and the tree is at most [`DEPTH`] deep.
pub fn well_formed(nodes: &[Node]) -> bool {
    fn check(nodes: &[Node], position: &mut Vec<usize>) -> bool {
        if position.len() >= DEPTH && !nodes.is_empty() {
            return false;
        }
        for (i, n) in nodes.iter().enumerate() {
            position.push(i);
            let ok = n.path == *position && check(&n.children, position);
            position.pop();
            if !ok {
                return false;
            }
        }
        true
    }
    check(nodes, &mut Vec::new())
}

// ---- The cap --------------------------------------------------------------

/// The bytes of the nodes on the wire.
pub fn wire_size(nodes: &[Node]) -> usize {
    serde_json::to_vec(nodes).map(|v| v.len()).unwrap_or(0)
}

/// Cut whole subtrees until the nodes fit `limit` bytes: the deepest
/// lists first, the last ones first. A cut node keeps its role, title
/// and value and loses its children. True when something was cut.
pub fn cap(nodes: &mut Vec<Node>, limit: usize) -> bool {
    let mut cut = false;
    while wire_size(nodes) > limit {
        let done = (0..DEPTH).rev().any(|depth| cut_at_depth(nodes, depth));
        if !done {
            nodes.clear();
            return true;
        }
        cut = true;
    }
    cut
}

/// Cut the last subtree `depth` levels down. True when one was.
fn cut_at_depth(nodes: &mut [Node], depth: usize) -> bool {
    for n in nodes.iter_mut().rev() {
        if n.children.is_empty() {
            continue;
        }
        if depth == 0 {
            n.children.clear();
            return true;
        }
        if cut_at_depth(&mut n.children, depth - 1) {
            return true;
        }
    }
    false
}

// ---- The diff -------------------------------------------------------------

/// What the host changes in its elements between two trees. The
/// positions are paths.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// The list at this position differs in count or roles: build its
    /// elements again, with new ids. The empty position is the list of
    /// the window's children.
    Rebuild(Vec<usize>),
    /// The node at this position kept its role and changed a fact.
    Update(Vec<usize>),
}

impl Change {
    /// A short text for the log.
    pub fn kind(&self) -> String {
        match self {
            Change::Rebuild(p) if p.is_empty() => "rebuild the tree".into(),
            Change::Rebuild(p) => format!("rebuild the list at {p:?}"), // not user text
            Change::Update(p) => format!("update {p:?}"),               // not user text
        }
    }
}

/// The changes from the tree `old` to the tree `new`.
pub fn diff(old: &[Node], new: &[Node]) -> Vec<Change> {
    let mut out = Vec::new();
    diff_list(old, new, &[], &mut out);
    out
}

fn diff_list(old: &[Node], new: &[Node], position: &[usize], out: &mut Vec<Change>) {
    let aligned = old.len() == new.len()
        && old
            .iter()
            .zip(new)
            .all(|(a, b)| a.role == b.role && a.path == b.path);
    if !aligned {
        out.push(Change::Rebuild(position.to_vec()));
        return;
    }
    for (i, (a, b)) in old.iter().zip(new).enumerate() {
        let mut here = position.to_vec();
        here.push(i);
        if a.title != b.title
            || a.value != b.value
            || a.enabled != b.enabled
            || a.focused != b.focused
            || !a.frame.near(&b.frame, 0.5)
        {
            out.push(Change::Update(here.clone()));
        }
        if !a.children.is_empty() || !b.children.is_empty() {
            diff_list(&a.children, &b.children, &here, out);
        }
    }
}

// ---- The mirror state of the host ------------------------------------------

/// The mirror state in the session record, for `paddock app status
/// --json` and the Mac check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxRecord {
    /// The guest window the tree belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<u32>,
    /// The elements of the tree.
    pub elements: usize,
    /// An Accessibility client asked for the children of a session
    /// window: the host keeps the tree current.
    pub watched: bool,
    /// The control connection of the agent is down.
    pub away: bool,
}

/// Why a press or a set was dropped instead of sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dropped {
    /// No tree: nothing to press in the guest.
    NoTree,
    /// The last tree has no element with that id: the element is from
    /// an earlier tree.
    Stale,
    /// The element is disabled in the last tree.
    Disabled,
    /// The control connection of the agent is down.
    Away,
}

/// The last tree of the key window, the ids of its elements, and
/// whether the agent is away. A timeout of the `ax.tree` request, a
/// lost connection and a new `hello` keep the last tree; only an
/// `ax.tree` event replaces it, and the close of its window or the
/// app's exit removes it.
#[derive(Debug, Default)]
pub struct Mirror {
    window: Option<u32>,
    nodes: Vec<Node>,
    /// The element id at each path of the last tree.
    ids: BTreeMap<Vec<usize>, u64>,
    next_id: u64,
    watched: bool,
    away: bool,
}

impl Mirror {
    /// The window of the last tree.
    pub fn window(&self) -> Option<u32> {
        self.window
    }

    /// The nodes of the last tree, empty without one.
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn watched(&self) -> bool {
        self.watched
    }

    pub fn away(&self) -> bool {
        self.away
    }

    /// An Accessibility client asked for the children of a session
    /// window: from now on the host keeps the tree current.
    pub fn wanted(&mut self) {
        self.watched = true;
    }

    /// The state for the session record.
    pub fn record(&self) -> AxRecord {
        AxRecord {
            window: self.window,
            elements: self.ids.len(),
            watched: self.watched,
            away: self.away,
        }
    }

    /// The id of every path of the last tree.
    pub fn ids(&self) -> &BTreeMap<Vec<usize>, u64> {
        &self.ids
    }

    /// The id of the element at a path of the last tree.
    pub fn id_of(&self, path: &[usize]) -> Option<u64> {
        self.ids.get(path).copied()
    }

    /// The node of an element id, when the last tree has it.
    pub fn node_of(&self, id: u64) -> Option<&Node> {
        let path = self.ids.iter().find(|(_, v)| **v == id)?.0;
        Node::at(&self.nodes, path)
    }

    /// An `ax.tree` event: the tree replaces the last one. The changes
    /// for the host elements, a full rebuild when the window differs.
    /// The ids of the nodes that a change does not rebuild stay.
    pub fn tree(&mut self, window: u32, nodes: Vec<Node>) -> Vec<Change> {
        self.away = false;
        let changes = if self.window == Some(window) {
            diff(&self.nodes, &nodes)
        } else {
            vec![Change::Rebuild(Vec::new())]
        };
        self.window = Some(window);
        self.nodes = nodes;
        for c in &changes {
            if let Change::Rebuild(position) = c {
                // The list's own parent keeps its element and its id.
                self.ids
                    .retain(|path, _| path.len() <= position.len() || !path.starts_with(position));
                let list = if position.is_empty() {
                    &self.nodes
                } else {
                    Node::at(&self.nodes, position)
                        .map(|n| n.children.as_slice())
                        .unwrap_or(&[])
                };
                let mut fresh = Vec::new();
                paths_of(list, &mut fresh);
                for path in fresh {
                    self.next_id += 1;
                    self.ids.insert(path, self.next_id);
                }
            }
        }
        changes
    }

    /// The `ax.tree` request timed out or failed: the last tree stays.
    pub fn timeout(&mut self) -> Vec<Change> {
        Vec::new()
    }

    /// The control connection closed: the last tree stays, presses are
    /// dropped until the agent is back.
    pub fn agent_gone(&mut self) -> Vec<Change> {
        self.away = true;
        Vec::new()
    }

    /// A new `hello`: presses go again, the tree stays until the next
    /// event.
    pub fn agent_back(&mut self) -> Vec<Change> {
        self.away = false;
        Vec::new()
    }

    /// The window of the tree closed, or the app exited: no tree. True
    /// when there was one.
    pub fn clear(&mut self) -> bool {
        let had = self.window.take().is_some();
        self.nodes.clear();
        self.ids.clear();
        had
    }

    /// The window closed: the tree goes when it was this window's.
    pub fn window_closed(&mut self, id: u32) -> bool {
        if self.window == Some(id) {
            self.clear()
        } else {
            false
        }
    }

    /// A press on the element with this id: the window, the path and
    /// the role for the `ax.press` request, or why it is dropped.
    pub fn press(&self, id: u64) -> Result<(u32, &Node), Dropped> {
        let window = self.window.ok_or(Dropped::NoTree)?;
        let node = self.node_of(id).ok_or(Dropped::Stale)?;
        if !node.enabled {
            return Err(Dropped::Disabled);
        }
        if self.away {
            return Err(Dropped::Away);
        }
        Ok((window, node))
    }

    /// A set of the value of the element with this id: the same checks
    /// as a press.
    pub fn set(&self, id: u64) -> Result<(u32, &Node), Dropped> {
        self.press(id)
    }
}

/// The host screen rectangle of an element: the host window's rectangle
/// (AppKit, origin bottom left) plus the element's frame relative to the
/// window (guest, origin top left).
pub fn host_rect(window: Rect, rel: Frame) -> Rect {
    Rect::new(
        window.x + rel.x,
        window.y + window.h - rel.y - rel.h,
        rel.w,
        rel.h,
    )
}

/// A host screen point as a point relative to the window, guest axes.
pub fn window_point(window: Rect, x: f64, y: f64) -> Point {
    Point {
        x: x - window.x,
        y: window.y + window.h - y,
    }
}

/// Every path of a tree, parents before children.
fn paths_of(nodes: &[Node], out: &mut Vec<Vec<usize>>) {
    for n in nodes {
        out.push(n.path.clone());
        paths_of(&n.children, out);
    }
}

#[cfg(test)]
#[path = "axmirror_tests.rs"]
mod tests;
