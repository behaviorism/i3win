use crate::geom::Direction;
use crate::tree::{NodeId, NodeKind};
use crate::wm::{Effect, Wm};

impl Wm {
    pub fn focus_direction(&mut self, from: NodeId, dir: Direction) -> Result<(), String> {
        if self.tree.is_floating(from) {
            let Some(ws) = self.tree.workspace_of(from) else {
                return Ok(());
            };
            let floating = self.tree.get(ws).floating.clone();
            let Some(i) = floating.iter().position(|&n| n == from) else {
                return Ok(());
            };
            let len = floating.len();
            let j = if dir.is_forward() {
                (i + 1) % len
            } else {
                (i + len - 1) % len
            };
            self.tree.set_focused(floating[j]);
            return Ok(());
        }
        let mut cur = from;
        loop {
            if matches!(self.tree.get(cur).kind, NodeKind::Workspace { .. }) {
                self.focus_adjacent_output(cur, dir);
                return Ok(());
            }
            let Some(p) = self.tree.parent(cur) else {
                return Ok(());
            };
            if self.tree.get(p).layout.orientation() == dir.orientation() {
                if let Some(i) = self.tree.index_in_parent(cur) {
                    let sibling = if dir.is_forward() {
                        self.tree.get(p).children.get(i + 1)
                    } else {
                        i.checked_sub(1)
                            .and_then(|k| self.tree.get(p).children.get(k))
                    };
                    if let Some(&s) = sibling {
                        let f = self.tree.focused_tiling_descendant(s);
                        self.tree.set_focused(f);
                        return Ok(());
                    }
                }
            }
            cur = p;
        }
    }

    fn focus_adjacent_output(&mut self, ws: NodeId, dir: Direction) {
        let Some(out) = self.tree.output_of(ws) else {
            return;
        };
        let Some(o) = self.output_in_direction(out, dir) else {
            return;
        };
        let old = self.focused_workspace();
        let vis = self.visible_workspace(o);
        let f = self.tree.focused_tiling_descendant(vis);
        self.tree.set_focused(f);
        if old != vis {
            let old_name = self.workspace_name(old).to_string();
            self.note_workspace_focus(old_name, vis);
        }
    }

    pub fn focus_parent(&mut self, from: NodeId) {
        if let Some(p) = self.tree.parent(from) {
            if matches!(self.tree.get(p).kind, NodeKind::Container) {
                self.tree.set_focused(from);
                let leaf = self.tree.focused_descendant(p);
                self.focused_container = Some((p, leaf));
            }
        }
    }

    pub fn focus_child(&mut self, from: NodeId) {
        let n = self.tree.get(from);
        if matches!(n.kind, NodeKind::Container) {
            if let Some(&c) = n.focus.first() {
                self.tree.set_focused(c);
                let leaf = self.tree.focused_descendant(c);
                self.focused_container =
                    matches!(self.tree.get(c).kind, NodeKind::Container).then_some((c, leaf));
            }
        }
    }

    /// `floating`: None toggles layer, Some(true) floating, Some(false) tiling.
    pub fn focus_layer(&mut self, from: NodeId, floating: Option<bool>) {
        let Some(ws) = self.tree.workspace_of(from) else {
            return;
        };
        let want_floating = floating.unwrap_or_else(|| !self.tree.is_floating(from));
        if want_floating {
            let n = self.tree.get(ws);
            if let Some(&f) = n.focus.iter().find(|c| n.floating.contains(c)) {
                self.tree.set_focused(f);
            }
        } else {
            let f = self.tree.focused_tiling_descendant(ws);
            if f != ws {
                self.tree.set_focused(f);
            }
        }
    }

    pub fn focus_node(&mut self, n: NodeId) {
        if !self.tree.contains(n) {
            return;
        }
        let before = self.focused_window();
        if let Some(ws) = self.tree.workspace_of(n) {
            if !self.is_visible(ws) {
                self.show_workspace(ws);
            }
        }
        let f = self.tree.focused_descendant(n);
        self.tree.set_focused(f);
        let after = self.focused_window();
        if after != before {
            if let Some(w) = after {
                self.effects.push(Effect::Focus(w));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::Config;
    use crate::testutil::*;
    use crate::tree::{Layout, WindowId};
    use crate::wm::{Effect, Wm};

    fn focus_title(wm: &Wm) -> String {
        wm.tree.describe(wm.focused())
    }
    #[test]
    fn left_right_in_split() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.manage(win(3));
        wm.run_command("focus left");
        assert_eq!(focus_title(&wm), "w2");
        wm.run_command("focus left");
        wm.run_command("focus left");
        assert_eq!(focus_title(&wm), "w1"); // stops at edge
    }
    #[test]
    fn up_down_walks_up_to_matching_orientation() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        let n2 = wm.tree.window_node(WindowId(2)).unwrap();
        wm.tree.wrap(n2, Layout::SplitV);
        wm.tree.set_focused(n2);
        wm.manage(win(3)); // H[w1 V[w2 w3]], focus w3
        wm.run_command("focus left");
        assert_eq!(focus_title(&wm), "w1");
        wm.run_command("focus right");
        assert_eq!(focus_title(&wm), "w3"); // remembers focus inside V
        wm.run_command("focus up");
        assert_eq!(focus_title(&wm), "w2");
    }
    #[test]
    fn tabbed_uses_left_right() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        let ws = wm.focused_workspace();
        wm.tree.get_mut(ws).layout = Layout::Tabbed;
        wm.run_command("focus left");
        assert_eq!(focus_title(&wm), "w1");
        wm.run_command("focus down");
        assert_eq!(focus_title(&wm), "w1");
    }
    #[test]
    fn crosses_to_other_output() {
        let mut wm = Wm::new(
            Config::default(),
            vec![out("A", 0, 1000), out("B", 1000, 1000)],
        );
        wm.manage(win(1));
        wm.run_command("focus output right");
        wm.manage(win(2));
        wm.run_command("focus output left");
        wm.run_command("focus right");
        assert_eq!(wm.focused_window(), Some(WindowId(2)));
    }
    #[test]
    fn parent_child() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        let n2 = wm.tree.window_node(WindowId(2)).unwrap();
        let c = wm.tree.wrap(n2, Layout::SplitV);
        wm.run_command("focus parent");
        assert_eq!(wm.focused(), c);
        wm.run_command("focus parent");
        assert_eq!(wm.focused(), c); // never focuses the workspace
        wm.run_command("focus child");
        assert_eq!(wm.focused_window(), Some(WindowId(2)));
    }
    #[test]
    fn mode_toggle() {
        let mut wm = test_wm();
        wm.manage(win(1));
        let mut f = win(2);
        f.wants_float = true;
        wm.manage(f);
        wm.run_command("focus mode_toggle");
        assert_eq!(wm.focused_window(), Some(WindowId(1)));
        wm.run_command("focus mode_toggle");
        assert_eq!(wm.focused_window(), Some(WindowId(2)));
    }
    #[test]
    fn focus_node_switches_workspace() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("workspace 2");
        wm.manage(win(2));
        wm.take_effects();
        let n1 = wm.tree.window_node(WindowId(1)).unwrap();
        wm.focus_node(n1);
        assert_eq!(wm.workspace_name(wm.focused_workspace()), "1");
        assert!(wm.take_effects().contains(&Effect::Focus(WindowId(1))));
    }
}
