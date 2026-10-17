use crate::config::FocusWrapping;
use crate::geom::Direction;
use crate::tree::{NodeId, NodeKind};
use crate::wm::Wm;

impl Wm {
    pub fn focus_direction(&mut self, from: NodeId, dir: Direction) -> Result<(), String> {
        self.focused_container = None;
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
            self.focus_in_place(floating[j]);
            return Ok(());
        }
        let wrapping = self.config.focus_wrapping;
        let mut outermost: Option<NodeId> = None;
        let mut cur = from;
        loop {
            if matches!(self.tree.get(cur).kind, NodeKind::Workspace { .. }) {
                let adjacent = wrapping != FocusWrapping::Workspace
                    && self
                        .tree
                        .output_of(cur)
                        .and_then(|o| self.output_in_direction(o, dir))
                        .is_some();
                if wrapping == FocusWrapping::No || adjacent {
                    self.focus_adjacent_output(cur, dir);
                } else if let Some(p) = outermost {
                    self.wrap_focus(p, dir);
                }
                return Ok(());
            }
            let Some(p) = self.tree.parent(cur) else {
                return Ok(());
            };
            if self.tree.get(p).layout.orientation() == dir.orientation() {
                let multi = self.tree.get(p).children.len() >= 2;
                if multi {
                    outermost = Some(p);
                }
                if let Some(i) = self.tree.index_in_parent(cur) {
                    let sibling = if dir.is_forward() {
                        self.tree.get(p).children.get(i + 1)
                    } else {
                        i.checked_sub(1)
                            .and_then(|k| self.tree.get(p).children.get(k))
                    };
                    if let Some(&s) = sibling {
                        let f = self.tree.focused_tiling_descendant(s);
                        self.focus_in_place(f);
                        return Ok(());
                    }
                }
                if multi && wrapping == FocusWrapping::Force {
                    self.wrap_focus(p, dir);
                    return Ok(());
                }
            }
            cur = p;
        }
    }

    /// Focuses the focused tiling descendant of `p`'s first (forward) or
    /// last (backward) child.
    fn wrap_focus(&mut self, p: NodeId, dir: Direction) {
        let children = &self.tree.get(p).children;
        let target = if dir.is_forward() {
            children.first()
        } else {
            children.last()
        };
        if let Some(&c) = target {
            let f = self.tree.focused_tiling_descendant(c);
            self.focus_in_place(f);
        }
    }

    /// `focus next|prev [sibling]`: cycles through the focused node's
    /// siblings, wrapping around.
    pub fn focus_next(&mut self, from: NodeId, forward: bool, sibling: bool) {
        self.focused_container = None;
        let siblings: Vec<NodeId> = if self.tree.is_floating(from) {
            match self.tree.workspace_of(from) {
                Some(ws) => self.tree.get(ws).floating.clone(),
                None => return,
            }
        } else {
            match self.tree.parent(from) {
                Some(p) => self.tree.get(p).children.clone(),
                None => return,
            }
        };
        let Some(i) = siblings.iter().position(|&n| n == from) else {
            return;
        };
        let len = siblings.len();
        let s = siblings[if forward {
            (i + 1) % len
        } else {
            (i + len - 1) % len
        }];
        if sibling || self.tree.is_floating(s) {
            self.focus_in_place(s);
            if matches!(self.tree.get(s).kind, NodeKind::Container) {
                let leaf = self.tree.focused_descendant(s);
                self.focused_container = Some((s, leaf));
            }
        } else {
            let f = self.tree.focused_tiling_descendant(s);
            self.focus_in_place(f);
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
                self.focus_in_place(from);
                let leaf = self.tree.focused_descendant(p);
                self.focused_container = Some((p, leaf));
            }
        }
    }

    pub fn focus_child(&mut self, from: NodeId) {
        let n = self.tree.get(from);
        if matches!(n.kind, NodeKind::Container) {
            if let Some(&c) = n.focus.first() {
                self.focus_in_place(c);
                let leaf = self.tree.focused_descendant(c);
                self.focused_container =
                    matches!(self.tree.get(c).kind, NodeKind::Container).then_some((c, leaf));
            }
        }
    }

    /// `floating`: None toggles layer, Some(true) floating, Some(false) tiling.
    pub fn focus_layer(&mut self, from: NodeId, floating: Option<bool>) {
        self.focused_container = None;
        let Some(ws) = self.tree.workspace_of(from) else {
            return;
        };
        let want_floating = floating.unwrap_or_else(|| !self.tree.is_floating(from));
        if want_floating {
            let n = self.tree.get(ws);
            if let Some(&f) = n.focus.iter().find(|c| n.floating.contains(c)) {
                self.focus_in_place(f);
            }
        } else {
            let f = self.tree.focused_tiling_descendant(ws);
            if f != ws {
                self.focus_in_place(f);
            }
        }
    }

    /// Makes `n` focused, switching to its workspace (with workspace
    /// bookkeeping) when needed. Emits no OS focus effect.
    pub(crate) fn focus_target(&mut self, n: NodeId) {
        self.focused_container = None;
        if !self.tree.contains(n) {
            return;
        }
        if self.tree.workspace_of(n) == Some(self.scratch) {
            if let Some(w) = self.tree.windows_under(n).first().copied() {
                self.show_scratch_window(w);
            }
            return;
        }
        let old_ws = self.focused_workspace();
        let old_name = self.workspace_name(old_ws).to_string();
        let mut noted = false;
        if let Some(ws) = self.tree.workspace_of(n) {
            if !self.is_visible(ws) {
                self.show_workspace(ws);
                noted = true;
            }
        }
        let f = self.tree.focused_descendant(n);
        self.tree.set_focused(f);
        let new_ws = self.focused_workspace();
        if !noted && new_ws != old_ws {
            self.note_workspace_focus(old_name, new_ws);
        }
    }

    pub fn focus_node(&mut self, n: NodeId) {
        if !self.tree.contains(n) {
            return;
        }
        let before = self.focus_leaf_window();
        self.focus_target(n);
        let after = self.focus_leaf_window();
        if after != before {
            if let Some(w) = after {
                self.push_focus(w);
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
    fn wm_with(fw: &str) -> Wm {
        let cfg = crate::config::parse_config(&format!("focus_wrapping {fw}\n"));
        let mut wm = Wm::new(cfg, vec![out("O1", 0, 1000)]);
        wm.manage(win(1));
        wm.manage(win(2));
        wm.manage(win(3));
        wm
    }
    #[test]
    fn focus_wrapping_modes() {
        let mut wm = wm_with("yes");
        wm.run_command("focus right");
        assert_eq!(wm.focused_window(), Some(WindowId(1)));
        let mut wm = wm_with("no");
        wm.run_command("focus right");
        assert_eq!(wm.focused_window(), Some(WindowId(3)));
        let mut wm = wm_with("workspace");
        wm.run_command("focus right");
        assert_eq!(wm.focused_window(), Some(WindowId(1)));
    }
    #[test]
    fn focus_wrapping_force_stays_in_parent() {
        let mut wm = wm_with("force");
        wm.run_command("split v");
        wm.manage(win(4));
        wm.run_command("focus down");
        assert_eq!(wm.focused_window(), Some(WindowId(3)));
    }
    #[test]
    fn focus_wrapping_yes_prefers_other_output() {
        let cfg = crate::config::parse_config("focus_wrapping yes\n");
        let mut wm = Wm::new(cfg, vec![out("A", 0, 1000), out("B", 1000, 1000)]);
        wm.manage(win(1));
        wm.run_command("focus right");
        assert_eq!(wm.focused_output(), wm.outputs()[1]);
    }
    #[test]
    fn focus_next_prev() {
        let mut wm = wm_with("yes");
        wm.run_command("focus next");
        assert_eq!(wm.focused_window(), Some(WindowId(1)));
        wm.run_command("focus prev");
        assert_eq!(wm.focused_window(), Some(WindowId(3)));
    }
    #[test]
    fn focus_next_sibling_selects_container() {
        let mut wm = wm_with("yes");
        let n3 = wm.tree.window_node(WindowId(3)).unwrap();
        let c = wm.tree.wrap(n3, Layout::SplitV);
        wm.tree
            .set_focused(wm.tree.window_node(WindowId(1)).unwrap());
        wm.run_command("focus next sibling");
        assert_eq!(wm.focused(), wm.tree.window_node(WindowId(2)).unwrap());
        wm.run_command("focus next sibling");
        assert_eq!(wm.focused(), c);
        wm.run_command("focus prev");
        assert_eq!(wm.focused_window(), Some(WindowId(2)));
    }
    #[test]
    fn left_right_in_split() {
        let mut wm = Wm::new(
            crate::config::parse_config("focus_wrapping no\n"),
            vec![out("O1", 0, 1000)],
        );
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
    fn selection_cleared_by_explicit_focus() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        let n2 = wm.tree.window_node(WindowId(2)).unwrap();
        let c = wm.tree.wrap(n2, Layout::SplitV);
        wm.run_command("focus parent");
        assert_eq!(wm.focused(), c);
        wm.focus_node(n2);
        assert_eq!(wm.focused(), n2);
        wm.run_command("focus parent");
        wm.focus_node(n2);
        assert_eq!(wm.focused(), n2);
        wm.run_command("focus parent");
        wm.run_command("focus tiling");
        assert_eq!(wm.focused(), n2);
    }
    #[test]
    fn selection_ignored_when_leaf_leaves_container() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        let n2 = wm.tree.window_node(WindowId(2)).unwrap();
        wm.tree.wrap(n2, Layout::SplitV);
        wm.run_command("focus parent");
        let ws = wm.focused_workspace();
        wm.tree.detach(n2);
        wm.tree.attach(ws, n2, None);
        wm.tree.set_focused(n2);
        assert_eq!(wm.focused(), n2);
    }
    #[test]
    fn unmanage_leaf_under_selected_container_refocuses() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        let n2 = wm.tree.window_node(WindowId(2)).unwrap();
        let c = wm.tree.wrap(n2, Layout::SplitV);
        let n1 = wm.tree.window_node(WindowId(1)).unwrap();
        wm.tree.detach(n1);
        wm.tree.attach(c, n1, None);
        wm.tree.set_focused(n2);
        wm.run_command("focus parent");
        assert_eq!(wm.focused(), c);
        wm.take_effects();
        wm.unmanage(WindowId(2));
        assert!(wm.take_effects().contains(&Effect::Focus(WindowId(1))));
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
