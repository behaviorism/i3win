use crate::config::{Border, GapsOverride};
use crate::geom::{Orientation, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub isize);

#[derive(Clone, Debug, PartialEq)]
pub struct WindowInfo {
    pub id: WindowId,
    pub title: String,
    pub class: String,
    pub instance: String,
    /// The window's current visible frame.
    pub rect: Rect,
    pub wants_float: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    SplitH,
    SplitV,
    Tabbed,
    Stacked,
}

impl Layout {
    pub fn orientation(self) -> Orientation {
        match self {
            Layout::SplitH | Layout::Tabbed => Orientation::Horizontal,
            Layout::SplitV | Layout::Stacked => Orientation::Vertical,
        }
    }

    pub fn letter(self) -> char {
        match self {
            Layout::SplitH => 'H',
            Layout::SplitV => 'V',
            Layout::Tabbed => 'T',
            Layout::Stacked => 'S',
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Layout::SplitH => "splith",
            Layout::SplitV => "splitv",
            Layout::Tabbed => "tabbed",
            Layout::Stacked => "stacked",
        }
    }
}

#[derive(Clone, Debug)]
pub struct WindowData {
    pub info: WindowInfo,
    pub floating_rect: Option<Rect>,
    pub urgent: bool,
    /// Value of `Wm::urgent_counter` when the window last became urgent.
    pub urgent_since: u64,
}

#[derive(Clone, Debug)]
pub enum NodeKind {
    Root,
    Output {
        name: String,
        rect: Rect,
        scale: f32,
        primary: bool,
    },
    Workspace {
        name: String,
        num: Option<i32>,
    },
    Container,
    Window(WindowData),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScratchState {
    None,
    Fresh,
    Changed,
}

impl ScratchState {
    pub fn as_str(self) -> &'static str {
        match self {
            ScratchState::None => "none",
            ScratchState::Fresh => "fresh",
            ScratchState::Changed => "changed",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub parent: Option<NodeId>,
    /// Tiling children in visual order.
    pub children: Vec<NodeId>,
    /// Only used on Workspace nodes.
    pub floating: Vec<NodeId>,
    /// Focus stack over `children` and `floating`, most recent first.
    pub focus: Vec<NodeId>,
    pub layout: Layout,
    pub percent: f64,
    pub fullscreen: bool,
    /// Runtime per-workspace gaps override (Workspace nodes only).
    pub gaps: GapsOverride,
    /// Per-window override of default_border/default_floating_border.
    pub border: Option<Border>,
    pub title_format: Option<String>,
    /// Some(padding_px) when title_window_icon is on.
    pub title_icon: Option<i32>,
    pub marks: Vec<String>,
    pub scratchpad: ScratchState,
    /// Floating windows with this flag follow workspace switches on their output.
    pub sticky: bool,
}

pub struct Tree {
    nodes: Vec<Option<Node>>,
    root: NodeId,
}

impl Default for Tree {
    fn default() -> Self {
        Tree::new()
    }
}

impl Tree {
    pub fn new() -> Tree {
        let mut t = Tree {
            nodes: Vec::new(),
            root: NodeId(0),
        };
        t.root = t.new_node(NodeKind::Root, Layout::SplitH);
        t
    }

    pub fn root(&self) -> NodeId {
        self.root
    }

    /// Panics on an unknown or freed id (internal invariant); use `contains` to check.
    pub fn get(&self, id: NodeId) -> &Node {
        self.nodes[id.0 as usize].as_ref().expect("freed NodeId")
    }

    /// Panics on an unknown or freed id (internal invariant); use `contains` to check.
    pub fn get_mut(&mut self, id: NodeId) -> &mut Node {
        self.nodes[id.0 as usize].as_mut().expect("freed NodeId")
    }

    pub fn contains(&self, id: NodeId) -> bool {
        matches!(self.nodes.get(id.0 as usize), Some(Some(_)))
    }

    pub fn new_node(&mut self, kind: NodeKind, layout: Layout) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(Some(Node {
            id,
            kind,
            parent: None,
            children: Vec::new(),
            floating: Vec::new(),
            focus: Vec::new(),
            layout,
            percent: 1.0,
            fullscreen: false,
            gaps: GapsOverride::default(),
            border: None,
            title_format: None,
            title_icon: None,
            marks: Vec::new(),
            scratchpad: ScratchState::None,
            sticky: false,
        }));
        id
    }

    pub fn attach(&mut self, parent: NodeId, child: NodeId, index: Option<usize>) {
        let p = self.get_mut(parent);
        let idx = index.unwrap_or(p.children.len()).min(p.children.len());
        p.children.insert(idx, child);
        p.focus.push(child);
        let n = p.children.len() as f64;
        let siblings: Vec<NodeId> = p.children.iter().copied().filter(|&c| c != child).collect();
        for s in siblings {
            self.get_mut(s).percent *= (n - 1.0) / n;
        }
        let c = self.get_mut(child);
        c.parent = Some(parent);
        c.percent = 1.0 / n;
    }

    pub fn attach_floating(&mut self, ws: NodeId, child: NodeId) {
        let w = self.get_mut(ws);
        w.floating.push(child);
        w.focus.push(child);
        self.get_mut(child).parent = Some(ws);
    }

    pub fn detach(&mut self, child: NodeId) {
        let Some(parent) = self.get(child).parent else {
            return;
        };
        let p = self.get_mut(parent);
        let was_tiling = p.children.contains(&child);
        p.children.retain(|&c| c != child);
        p.floating.retain(|&c| c != child);
        p.focus.retain(|&c| c != child);
        if was_tiling {
            let sibs = p.children.clone();
            let sum: f64 = sibs.iter().map(|&s| self.get(s).percent).sum();
            for &s in &sibs {
                let n = self.get_mut(s);
                n.percent = if sum > 1e-12 {
                    n.percent / sum
                } else {
                    1.0 / sibs.len() as f64
                };
            }
        }
        self.get_mut(child).parent = None;
    }

    pub fn remove(&mut self, id: NodeId) {
        self.detach(id);
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            if let Some(node) = self.nodes[n.0 as usize].take() {
                stack.extend(node.children);
                stack.extend(node.floating);
            }
        }
    }

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.get(id).parent
    }

    pub fn index_in_parent(&self, id: NodeId) -> Option<usize> {
        let p = self.get(id).parent?;
        self.get(p).children.iter().position(|&c| c == id)
    }

    pub fn is_floating(&self, id: NodeId) -> bool {
        match self.get(id).parent {
            Some(p) => self.get(p).floating.contains(&id),
            None => false,
        }
    }

    pub fn ancestors(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut cur = self.get(id).parent;
        while let Some(p) = cur {
            out.push(p);
            cur = self.get(p).parent;
        }
        out
    }

    pub fn is_ancestor(&self, anc: NodeId, id: NodeId) -> bool {
        self.ancestors(id).contains(&anc)
    }

    pub fn workspace_of(&self, id: NodeId) -> Option<NodeId> {
        let mut cur = Some(id);
        while let Some(n) = cur {
            if matches!(self.get(n).kind, NodeKind::Workspace { .. }) {
                return Some(n);
            }
            cur = self.get(n).parent;
        }
        None
    }

    pub fn output_of(&self, id: NodeId) -> Option<NodeId> {
        let mut cur = Some(id);
        while let Some(n) = cur {
            if matches!(self.get(n).kind, NodeKind::Output { .. }) {
                return Some(n);
            }
            cur = self.get(n).parent;
        }
        None
    }

    pub fn window_node(&self, w: WindowId) -> Option<NodeId> {
        self.nodes.iter().flatten().find_map(|n| match &n.kind {
            NodeKind::Window(d) if d.info.id == w => Some(n.id),
            _ => None,
        })
    }

    pub fn window_id(&self, id: NodeId) -> Option<WindowId> {
        match &self.get(id).kind {
            NodeKind::Window(d) => Some(d.info.id),
            _ => None,
        }
    }

    pub fn windows_under(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        self.collect_windows(id, &mut out);
        out
    }

    fn collect_windows(&self, id: NodeId, out: &mut Vec<NodeId>) {
        let n = self.get(id);
        if matches!(n.kind, NodeKind::Window(_)) {
            out.push(id);
        }
        for &c in n.children.iter().chain(n.floating.iter()) {
            self.collect_windows(c, out);
        }
    }

    fn move_to_front(&mut self, parent: NodeId, child: NodeId) {
        let f = &mut self.get_mut(parent).focus;
        f.retain(|&c| c != child);
        f.insert(0, child);
    }

    pub fn set_focused(&mut self, id: NodeId) {
        let mut cur = id;
        while let Some(p) = self.get(cur).parent {
            self.move_to_front(p, cur);
            cur = p;
        }
    }

    pub fn set_focused_within(&mut self, id: NodeId, stop: NodeId) {
        let mut cur = id;
        while cur != stop {
            let Some(p) = self.get(cur).parent else { break };
            self.move_to_front(p, cur);
            cur = p;
        }
    }

    pub fn focused_descendant(&self, id: NodeId) -> NodeId {
        let mut cur = id;
        while let Some(&next) = self.get(cur).focus.first() {
            cur = next;
        }
        cur
    }

    pub fn focused_tiling_descendant(&self, id: NodeId) -> NodeId {
        let mut cur = id;
        loop {
            let n = self.get(cur);
            match n.focus.iter().find(|c| n.children.contains(c)) {
                Some(&next) => cur = next,
                None => return cur,
            }
        }
    }

    pub fn cleanup(&mut self, id: NodeId) {
        let mut cur = id;
        while self.contains(cur)
            && matches!(self.get(cur).kind, NodeKind::Container)
            && self.get(cur).children.is_empty()
        {
            let parent = self.get(cur).parent;
            self.remove(cur);
            match parent {
                Some(p) => cur = p,
                None => break,
            }
        }
    }

    pub fn wrap(&mut self, id: NodeId, layout: Layout) -> NodeId {
        let c = self.new_node(NodeKind::Container, layout);
        let Some(p) = self.get(id).parent else {
            self.attach(c, id, None);
            return c;
        };
        let percent = self.get(id).percent;
        let pn = self.get_mut(p);
        if let Some(i) = pn.children.iter().position(|&x| x == id) {
            pn.children[i] = c;
        } else if let Some(i) = pn.floating.iter().position(|&x| x == id) {
            pn.floating[i] = c;
        }
        if let Some(i) = pn.focus.iter().position(|&x| x == id) {
            pn.focus[i] = c;
        }
        let cn = self.get_mut(c);
        cn.parent = Some(p);
        cn.percent = percent;
        cn.children = vec![id];
        cn.focus = vec![id];
        let inner = self.get_mut(id);
        inner.parent = Some(c);
        inner.percent = 1.0;
        c
    }

    pub fn swap(&mut self, a: NodeId, b: NodeId) {
        if a == b {
            return;
        }
        let (Some(pa), Some(pb)) = (self.get(a).parent, self.get(b).parent) else {
            return;
        };
        if self.is_ancestor(a, b) || self.is_ancestor(b, a) {
            return;
        }
        // Each node sits in its parent's tiled or floating list.
        let (fl_a, fl_b) = (self.is_floating(a), self.is_floating(b));
        let list = |t: &Tree, p: NodeId, fl: bool| {
            let n = t.get(p);
            if fl {
                n.floating.clone()
            } else {
                n.children.clone()
            }
        };
        let ia = list(self, pa, fl_a).iter().position(|&x| x == a);
        let ib = list(self, pb, fl_b).iter().position(|&x| x == b);
        let (Some(ia), Some(ib)) = (ia, ib) else {
            return;
        };
        let fa = self.get(pa).focus.iter().position(|&x| x == a);
        let fb = self.get(pb).focus.iter().position(|&x| x == b);
        let put = |t: &mut Tree, p: NodeId, fl: bool, i: usize, x: NodeId| {
            let n = t.get_mut(p);
            if fl {
                n.floating[i] = x;
            } else {
                n.children[i] = x;
            }
        };
        put(self, pa, fl_a, ia, b);
        put(self, pb, fl_b, ib, a);
        if let Some(fa) = fa {
            self.get_mut(pa).focus[fa] = b;
        }
        if let Some(fb) = fb {
            self.get_mut(pb).focus[fb] = a;
        }
        self.get_mut(a).parent = Some(pb);
        self.get_mut(b).parent = Some(pa);
        let (qa, qb) = (self.get(a).percent, self.get(b).percent);
        self.get_mut(a).percent = qb;
        self.get_mut(b).percent = qa;
    }

    pub fn describe(&self, id: NodeId) -> String {
        let n = self.get(id);
        match &n.kind {
            NodeKind::Window(d) => d.info.title.clone(),
            _ => {
                let kids: Vec<String> = n.children.iter().map(|&c| self.describe(c)).collect();
                format!("{}[{}]", n.layout.letter(), kids.join(" "))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Tree, NodeId) {
        let mut t = Tree::new();
        let o = t.new_node(
            NodeKind::Output {
                name: "O".into(),
                rect: Rect::new(0, 0, 1000, 500),
                scale: 1.0,
                primary: false,
            },
            Layout::SplitH,
        );
        let ws = t.new_node(
            NodeKind::Workspace {
                name: "1".into(),
                num: Some(1),
            },
            Layout::SplitH,
        );
        let root = t.root();
        t.attach(root, o, None);
        t.attach(o, ws, None);
        (t, ws)
    }

    fn win(t: &mut Tree, n: isize) -> NodeId {
        t.new_node(
            NodeKind::Window(WindowData {
                info: WindowInfo {
                    id: WindowId(n),
                    title: format!("w{n}"),
                    class: format!("c{n}"),
                    instance: format!("i{n}"),
                    rect: Rect::new(0, 0, 400, 300),
                    wants_float: false,
                },
                floating_rect: None,
                urgent: false,
                urgent_since: 0,
            }),
            Layout::SplitH,
        )
    }

    #[test]
    fn attach_gives_equal_percents() {
        let (mut t, ws) = fixture();
        let a = win(&mut t, 1);
        let b = win(&mut t, 2);
        let c = win(&mut t, 3);
        t.attach(ws, a, None);
        t.attach(ws, b, None);
        t.attach(ws, c, None);
        for n in [a, b, c] {
            assert!((t.get(n).percent - 1.0 / 3.0).abs() < 1e-9);
        }
    }

    #[test]
    fn attach_at_index() {
        let (mut t, ws) = fixture();
        let a = win(&mut t, 1);
        let b = win(&mut t, 2);
        t.attach(ws, a, None);
        t.attach(ws, b, Some(0));
        assert_eq!(t.describe(ws), "H[w2 w1]");
    }

    #[test]
    fn detach_renormalizes() {
        let (mut t, ws) = fixture();
        let a = win(&mut t, 1);
        let b = win(&mut t, 2);
        let c = win(&mut t, 3);
        t.attach(ws, a, None);
        t.attach(ws, b, None);
        t.attach(ws, c, None);
        t.get_mut(a).percent = 0.5;
        t.get_mut(b).percent = 0.25;
        t.get_mut(c).percent = 0.25;
        t.detach(a);
        assert!((t.get(b).percent - 0.5).abs() < 1e-9 && (t.get(c).percent - 0.5).abs() < 1e-9);
        assert_eq!(t.get(a).parent, None);
    }

    #[test]
    fn set_focused_updates_whole_path() {
        let (mut t, ws) = fixture();
        let a = win(&mut t, 1);
        let b = win(&mut t, 2);
        t.attach(ws, a, None);
        t.attach(ws, b, None);
        t.set_focused(b);
        assert_eq!(t.focused_descendant(t.root()), b);
        t.set_focused(a);
        assert_eq!(t.get(ws).focus, vec![a, b]);
    }

    #[test]
    fn cleanup_removes_empty_containers_but_not_workspace() {
        let (mut t, ws) = fixture();
        let c1 = t.new_node(NodeKind::Container, Layout::SplitV);
        t.attach(ws, c1, None);
        let c2 = t.new_node(NodeKind::Container, Layout::SplitH);
        t.attach(c1, c2, None);
        t.cleanup(c2);
        assert!(!t.contains(c1) && !t.contains(c2) && t.contains(ws));
        assert_eq!(t.describe(ws), "H[]");
    }

    #[test]
    fn wrap_keeps_slot() {
        let (mut t, ws) = fixture();
        let a = win(&mut t, 1);
        let b = win(&mut t, 2);
        t.attach(ws, a, None);
        t.attach(ws, b, None);
        t.get_mut(a).percent = 0.7;
        t.get_mut(b).percent = 0.3;
        let c = t.wrap(b, Layout::SplitV);
        assert_eq!(t.describe(ws), "H[w1 V[w2]]");
        assert!((t.get(c).percent - 0.3).abs() < 1e-9);
        assert!((t.get(b).percent - 1.0).abs() < 1e-9);
    }

    #[test]
    fn swap_across_parents() {
        let (mut t, ws) = fixture();
        let a = win(&mut t, 1);
        let b = win(&mut t, 2);
        let c = win(&mut t, 3);
        t.attach(ws, a, None);
        t.attach(ws, b, None);
        let v = t.wrap(b, Layout::SplitV);
        t.attach(v, c, None);
        t.swap(a, c);
        assert_eq!(t.describe(ws), "H[w3 V[w2 w1]]");
    }

    #[test]
    fn swap_same_parent_keeps_percents_with_slots() {
        let (mut t, ws) = fixture();
        let a = win(&mut t, 1);
        let b = win(&mut t, 2);
        t.attach(ws, a, None);
        t.attach(ws, b, None);
        t.get_mut(a).percent = 0.7;
        t.get_mut(b).percent = 0.3;
        t.swap(a, b);
        assert_eq!(t.describe(ws), "H[w2 w1]");
        assert!((t.get(b).percent - 0.7).abs() < 1e-9);
        assert!((t.get(a).percent - 0.3).abs() < 1e-9);
        assert_eq!(t.get(ws).focus, vec![b, a]);
    }

    #[test]
    fn floating_omitted_from_describe_and_in_windows_under() {
        let (mut t, ws) = fixture();
        let a = win(&mut t, 1);
        let f = win(&mut t, 2);
        t.attach(ws, a, None);
        t.attach_floating(ws, f);
        assert_eq!(t.describe(ws), "H[w1]");
        assert!(t.is_floating(f));
        assert_eq!(t.windows_under(ws), vec![a, f]);
        t.set_focused(f);
        assert_eq!(t.focused_tiling_descendant(ws), a);
    }

    #[test]
    fn queries() {
        let (mut t, ws) = fixture();
        let a = win(&mut t, 1);
        t.attach(ws, a, None);
        assert_eq!(t.workspace_of(a), Some(ws));
        assert_eq!(t.window_node(WindowId(1)), Some(a));
        assert!(t.is_ancestor(ws, a));
        assert_eq!(t.ancestors(a).last().copied(), Some(t.root()));
    }
}
