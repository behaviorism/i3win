use crate::command::SwapTarget;
use crate::tree::{NodeId, NodeKind, WindowId};
use crate::wm::Wm;

const NO_MARK: &str = "No container matched the mark";

impl Wm {
    fn all_nodes(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = vec![self.tree.root()];
        while let Some(n) = stack.pop() {
            out.push(n);
            let node = self.tree.get(n);
            stack.extend(node.children.iter().chain(node.floating.iter()));
        }
        out
    }

    /// Sorted, de-duplicated names of every mark in the tree.
    pub fn all_marks(&self) -> Vec<String> {
        let mut marks: Vec<String> = self
            .all_nodes()
            .into_iter()
            .flat_map(|n| self.tree.get(n).marks.clone())
            .collect();
        marks.sort();
        marks.dedup();
        marks
    }

    fn find_marked(&self, name: &str) -> Option<NodeId> {
        self.all_nodes()
            .into_iter()
            .find(|&n| self.tree.get(n).marks.iter().any(|m| m == name))
    }

    /// The window a node-level event is reported for (itself, or the first
    /// window beneath a container).
    fn event_window(&self, n: NodeId) -> Option<WindowId> {
        self.tree
            .windows_under(n)
            .first()
            .and_then(|&w| self.tree.window_id(w))
    }

    fn emit_node(&mut self, change: &str, n: NodeId) {
        if let Some(w) = self.event_window(n) {
            self.emit_window(change, w);
        }
    }

    pub(crate) fn mark(&mut self, target: NodeId, add: bool, toggle: bool, name: &str) {
        if toggle && self.tree.get(target).marks.iter().any(|m| m == name) {
            self.tree.get_mut(target).marks.retain(|m| m != name);
            self.emit_node("mark", target);
            return;
        }
        // A mark is unique in the tree.
        for n in self.all_nodes() {
            if n != target && self.tree.get(n).marks.iter().any(|m| m == name) {
                self.tree.get_mut(n).marks.retain(|m| m != name);
                self.emit_node("mark", n);
            }
        }
        let marks = &mut self.tree.get_mut(target).marks;
        if !add {
            marks.clear();
        }
        if !marks.iter().any(|m| m == name) {
            marks.push(name.to_string());
        }
        self.emit_node("mark", target);
    }

    pub(crate) fn unmark(&mut self, name: Option<&str>) {
        for n in self.all_nodes() {
            let marks = &mut self.tree.get_mut(n).marks;
            let before = marks.len();
            match name {
                Some(name) => marks.retain(|m| m != name),
                None => marks.clear(),
            }
            if marks.len() != before {
                self.emit_node("mark", n);
            }
        }
    }

    pub(crate) fn move_to_mark(&mut self, target: NodeId, name: &str) -> Result<(), String> {
        let m = self.find_marked(name).ok_or(NO_MARK)?;
        if m == target
            || self.tree.is_ancestor(target, m)
            || matches!(
                self.tree.get(target).kind,
                NodeKind::Root | NodeKind::Output { .. } | NodeKind::Workspace { .. }
            )
        {
            return Ok(());
        }
        let Some(dest_ws) = self.tree.workspace_of(m) else {
            return Err(NO_MARK.into());
        };
        let src = self.tree.workspace_of(target);
        let src_was_focused =
            src.is_some_and(|s| self.is_visible(s) && self.focused_workspace() == s);
        let prev_focus = self.focused();
        let marked_floating = self.tree.is_floating(m);
        if marked_floating && !matches!(self.tree.get(target).kind, NodeKind::Window(_)) {
            return Err("only windows can float".into());
        }
        let moved: Vec<_> = self
            .tree
            .windows_under(target)
            .into_iter()
            .filter_map(|n| self.tree.window_id(n))
            .collect();
        let old_parent = self.tree.parent(target);
        self.tree.detach(target);
        if let Some(p) = old_parent {
            if matches!(self.tree.get(p).kind, NodeKind::Container) {
                self.tree.cleanup(p);
            }
        }
        if marked_floating {
            if let NodeKind::Window(d) = &mut self.tree.get_mut(target).kind {
                if d.floating_rect.is_none() {
                    d.floating_rect = Some(crate::ops::floating::clamp_floating_rect(
                        &self.config,
                        d.info.rect,
                    ));
                }
            }
            self.tree.attach_floating(dest_ws, target);
        } else {
            match (self.tree.parent(m), self.tree.index_in_parent(m)) {
                (Some(p), Some(i)) => self.tree.attach(p, target, Some(i + 1)),
                _ => self.tree.attach(dest_ws, target, None),
            }
        }
        if dest_ws == self.scratch {
            for n in self.tree.windows_under(target) {
                if self.tree.get(n).scratchpad == crate::tree::ScratchState::None {
                    self.tree.get_mut(n).scratchpad = crate::tree::ScratchState::Fresh;
                }
                self.scratch_order.retain(|&x| x != n);
                self.scratch_order.push(n);
            }
        }
        let f = self.tree.focused_descendant(target);
        self.tree.set_focused_within(f, dest_ws);
        // Focus stays on what was focused before, unless the focused window
        // moved to a different hidden workspace (then the source keeps focus).
        let prev_moved = self.tree.contains(prev_focus)
            && (prev_focus == target || self.tree.is_ancestor(target, prev_focus));
        let keep = self.tree.contains(prev_focus)
            && (!prev_moved || src == Some(dest_ws) || self.is_visible(dest_ws));
        if keep {
            self.focused_container = None;
            self.tree.set_focused(prev_focus);
        } else if src_was_focused {
            if let Some(s) = src.filter(|&s| self.tree.contains(s)) {
                let f = self.tree.focused_descendant(s);
                self.tree.set_focused(f);
            }
        }
        if let Some(s) = src.filter(|&s| s != dest_ws) {
            self.remove_if_empty_hidden(s);
        }
        for w in moved {
            self.emit_window("move", w);
        }
        Ok(())
    }

    pub(crate) fn swap_with(&mut self, target: NodeId, with: &SwapTarget) -> Result<(), String> {
        let other = match with {
            SwapTarget::Mark(name) => self.find_marked(name).ok_or(NO_MARK)?,
            SwapTarget::ConId(id) => {
                let n = NodeId(*id);
                if !self.tree.contains(n) {
                    return Err("No container found with that con_id".into());
                }
                n
            }
            SwapTarget::Id(id) => self
                .tree
                .window_node(WindowId(*id))
                .ok_or("No window found with that id")?,
        };
        if other == target {
            return Ok(());
        }
        let is_leaf = |n: NodeId| {
            !matches!(
                self.tree.get(n).kind,
                NodeKind::Root | NodeKind::Output { .. } | NodeKind::Workspace { .. }
            )
        };
        if !is_leaf(target) || !is_leaf(other) {
            return Err("Cannot swap these containers".into());
        }
        if self.tree.is_floating(target) != self.tree.is_floating(other) {
            return Err("Cannot swap a floating and a tiled container".into());
        }
        if self.tree.is_ancestor(target, other) || self.tree.is_ancestor(other, target) {
            return Err("Cannot swap a container with its ancestor".into());
        }
        let focused = self.focused();
        let focused_ws = self.focused_workspace();
        self.tree.swap(target, other);
        if self.tree.is_floating(target) {
            let ra = self.floating_rect_of(target);
            let rb = self.floating_rect_of(other);
            self.set_floating_rect_of(target, rb);
            self.set_floating_rect_of(other, ra);
        }
        // Focus stays on the same window, which may now sit elsewhere.
        if self.tree.contains(focused) && self.focused_container.is_none() {
            if self.tree.workspace_of(focused) == Some(self.scratch) {
                let f = self.tree.focused_descendant(focused_ws);
                self.tree.set_focused(f);
            } else {
                self.tree.set_focused(focused);
            }
        }
        for n in [target, other] {
            self.emit_node("move", n);
        }
        Ok(())
    }

    fn floating_rect_of(&self, n: NodeId) -> Option<crate::geom::Rect> {
        match &self.tree.get(n).kind {
            NodeKind::Window(d) => d.floating_rect,
            _ => None,
        }
    }

    fn set_floating_rect_of(&mut self, n: NodeId, r: Option<crate::geom::Rect>) {
        if let NodeKind::Window(d) = &mut self.tree.get_mut(n).kind {
            d.floating_rect = r;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ipc::marks_json;
    use crate::testutil::*;
    use crate::tree::WindowId;
    use crate::wm::{Effect, IpcEvent};

    #[test]
    fn marks_are_unique_and_replace_add_toggle() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("[id=1] mark a");
        wm.run_command("[id=2] mark a");
        let n1 = wm.tree.window_node(WindowId(1)).unwrap();
        let n2 = wm.tree.window_node(WindowId(2)).unwrap();
        assert!(wm.tree.get(n1).marks.is_empty());
        assert_eq!(wm.tree.get(n2).marks, vec!["a"]);
        wm.run_command("[id=2] mark --add b");
        assert_eq!(wm.tree.get(n2).marks, vec!["a", "b"]);
        wm.run_command("[id=2] mark --add --toggle b");
        assert_eq!(wm.tree.get(n2).marks, vec!["a"]);
        wm.run_command("[id=2] mark c");
        assert_eq!(wm.tree.get(n2).marks, vec!["c"]);
        assert_eq!(marks_json(&wm), serde_json::json!(["c"]));
        wm.run_command("unmark");
        assert_eq!(marks_json(&wm), serde_json::json!([]));
    }

    #[test]
    fn con_mark_criteria_move_to_mark_and_swap() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.manage(win(3));
        wm.run_command("[id=1] mark target");
        assert!(wm.run_command("[con_mark=\"^targ\"] kill")[0].success);
        wm.run_command("[id=3] move container to mark target");
        assert_eq!(ws_desc(&wm), "H[w1 w3 w2]");
        wm.run_command("[id=3] swap container with mark target");
        assert_eq!(ws_desc(&wm), "H[w3 w1 w2]");
        assert!(!wm.run_command("move container to mark nope")[0].success);
        check_invariants(&wm);
    }

    #[test]
    fn mark_emits_window_mark_event() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.take_effects();
        wm.run_command("mark x");
        assert!(wm.take_effects().iter().any(
            |e| matches!(e, Effect::Ipc(IpcEvent::Window { change, .. }) if change == "mark")
        ));
    }

    #[test]
    fn unmark_by_name_and_move_to_mark_across_workspaces() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("mark keep; workspace 2");
        wm.manage(win(2));
        wm.run_command("mark gone; unmark gone");
        assert_eq!(marks_json(&wm), serde_json::json!(["keep"]));
        wm.run_command("move container to mark keep");
        assert_eq!(ws_desc(&wm), "H[]");
        wm.run_command("workspace 1");
        assert_eq!(ws_desc(&wm), "H[w1 w2]");
        check_invariants(&wm);
    }

    #[test]
    fn move_to_mark_keeps_focus_unless_moved_window_focused() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.manage(win(3));
        wm.run_command("[id=2] mark t");
        wm.run_command("[id=1] focus");
        wm.run_command("[id=3] move container to mark t");
        assert_eq!(ws_desc(&wm), "H[w1 w2 w3]");
        assert_eq!(wm.focused_window(), Some(WindowId(1)));
        wm.run_command("[id=1] move container to mark t");
        assert_eq!(ws_desc(&wm), "H[w2 w1 w3]");
        assert_eq!(wm.focused_window(), Some(WindowId(1)));
        check_invariants(&wm);
    }

    #[test]
    fn swap_floating_with_tiled_fails() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("[id=1] floating enable");
        assert!(!wm.run_command("[id=2] swap container with id 1")[0].success);
        assert!(!wm.run_command("swap container with con_id 9999")[0].success);
        check_invariants(&wm);
    }
}
