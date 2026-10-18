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

    /// Removes mark `name` (every mark when `None`) from `scope`, or from
    /// every node in the tree when `scope` is `None`.
    pub(crate) fn unmark(&mut self, scope: Option<NodeId>, name: Option<&str>) {
        let nodes = match scope {
            Some(n) => vec![n],
            None => self.all_nodes(),
        };
        for n in nodes {
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
        if matches!(
            self.tree.get(m).kind,
            NodeKind::Root | NodeKind::Output { .. }
        ) {
            return Err("Cannot move a container to a mark on an output".into());
        }
        let mark_is_workspace = matches!(self.tree.get(m).kind, NodeKind::Workspace { .. });
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
        let target_floating = self.tree.is_floating(target);
        if marked_floating && !matches!(self.tree.get(target).kind, NodeKind::Window(_)) {
            return Err("only windows can float".into());
        }
        let moved: Vec<_> = self
            .tree
            .windows_under(target)
            .into_iter()
            .filter_map(|n| self.tree.window_id(n))
            .collect();
        let src_rect = src
            .and_then(|s| self.tree.output_of(s))
            .map(|o| self.output_rect(o));
        let old_parent = self.tree.parent(target);
        self.tree.detach(target);
        if let Some(p) = old_parent {
            if matches!(self.tree.get(p).kind, NodeKind::Container) {
                self.tree.cleanup(p);
            }
        }
        if mark_is_workspace {
            // A marked workspace receives the container like `move to workspace`.
            if target_floating {
                self.tree.attach_floating(m, target);
                self.shift_to_dest_output(target, src_rect, dest_ws);
            } else {
                self.insert_tiled(m, target);
            }
        } else if marked_floating {
            if let NodeKind::Window(d) = &mut self.tree.get_mut(target).kind {
                if d.floating_rect.is_none() {
                    d.floating_rect = Some(crate::ops::floating::clamp_floating_rect(
                        &self.config,
                        d.info.rect,
                    ));
                }
            }
            self.tree.attach_floating(dest_ws, target);
            self.shift_to_dest_output(target, src_rect, dest_ws);
        } else {
            match (self.tree.parent(m), self.tree.index_in_parent(m)) {
                (Some(p), Some(i)) => self.tree.attach(p, target, Some(i + 1)),
                _ => self.tree.attach(dest_ws, target, None),
            }
        }
        self.on_node_rehomed(target, src, dest_ws);
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

    /// Moves the floating window `n` from the output rect `src_rect` onto the
    /// output holding `dest_ws`, keeping its relative offset.
    fn shift_to_dest_output(
        &mut self,
        n: NodeId,
        src_rect: Option<crate::geom::Rect>,
        dest_ws: NodeId,
    ) {
        let to = self.tree.output_of(dest_ws).map(|o| self.output_rect(o));
        if let (Some(from), Some(to)) = (src_rect, to) {
            if from != to {
                self.shift_floating(n, from, to);
            }
        }
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
        let target_ws = self.tree.workspace_of(target);
        let other_ws = self.tree.workspace_of(other);
        self.tree.swap(target, other);
        if let (Some(tw), Some(ow)) = (target_ws, other_ws) {
            self.on_node_rehomed(target, Some(tw), ow);
            self.on_node_rehomed(other, Some(ow), tw);
        }
        if self.tree.is_floating(target) {
            let ra = self.floating_rect_of(target);
            let rb = self.floating_rect_of(other);
            // Each window takes the other's slot, so the swapped rects are
            // already absolute positions on the new output; only clamp.
            let clamp = |wm: &Self, r: Option<crate::geom::Rect>| {
                r.map(|r| crate::ops::floating::clamp_floating_rect(&wm.config, r))
            };
            let (ra, rb) = (clamp(self, ra), clamp(self, rb));
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
    use crate::tree::{ScratchState, WindowId};
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

    #[test]
    fn move_to_mark_on_workspace_of_other_output() {
        let mut wm = crate::wm::Wm::new(
            crate::config::Config::default(),
            vec![out("A", 0, 1000), out("B", 1000, 1000)],
        );
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("focus output B; mark wsm; focus output A");
        let b = wm.outputs()[1];
        assert!(wm.run_command("[id=1] move container to mark wsm")[0].success);
        let n1 = wm.tree.window_node(WindowId(1)).unwrap();
        assert_eq!(wm.tree.workspace_of(n1), Some(wm.visible_workspace(b)));
        assert_eq!(wm.tree.describe(wm.visible_workspace(b)), "H[w1]");
        check_invariants(&wm);
    }

    #[test]
    fn move_to_mark_on_workspace_same_output() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("workspace 5; mark wsm");
        assert!(wm.run_command("[id=1] move container to mark wsm")[0].success);
        assert_eq!(wm.workspace_name(wm.focused_workspace()), "5");
        assert_eq!(ws_desc(&wm), "H[w1]");
        let a = wm.arrange();
        assert!(a.windows.iter().any(|p| p.id == WindowId(1)));
        assert!(!a.hidden.contains(&WindowId(1)));
        check_invariants(&wm);
    }

    #[test]
    fn move_floating_to_mark_on_workspace_stays_floating() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("floating enable; workspace 5; mark wsm");
        assert!(wm.run_command("[id=1] move container to mark wsm")[0].success);
        let n1 = wm.tree.window_node(WindowId(1)).unwrap();
        assert!(wm.tree.is_floating(n1));
        assert_eq!(wm.tree.workspace_of(n1), Some(wm.focused_workspace()));
        check_invariants(&wm);
    }

    #[test]
    fn move_to_mark_on_output_is_refused() {
        let mut wm = test_wm();
        wm.manage(win(1));
        let o = wm.outputs()[0];
        wm.tree.get_mut(o).marks.push("om".into());
        assert!(!wm.run_command("[id=1] move container to mark om")[0].success);
        assert_eq!(ws_desc(&wm), "H[w1]");
        check_invariants(&wm);
    }

    #[test]
    fn swap_floating_with_hidden_scratch_window_keeps_scratchpad_usable() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("[id=2] move scratchpad; [id=2] mark sp; [id=1] floating enable");
        assert!(wm.run_command("[id=1] swap container with mark sp")[0].success);
        let n1 = wm.tree.window_node(WindowId(1)).unwrap();
        let n2 = wm.tree.window_node(WindowId(2)).unwrap();
        assert_eq!(wm.tree.workspace_of(n1), Some(wm.scratch_workspace()));
        assert_eq!(wm.tree.get(n1).scratchpad, ScratchState::Fresh);
        assert_eq!(wm.tree.get(n2).scratchpad, ScratchState::None);
        assert!(wm.run_command("scratchpad show")[0].success);
        assert_eq!(wm.tree.workspace_of(n1), Some(wm.focused_workspace()));
        assert_eq!(wm.focused_window(), Some(WindowId(1)));
        check_invariants(&wm);
    }

    #[test]
    fn unmark_with_criteria_clears_only_matched() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("[id=1] mark --add a; [id=1] mark --add c; [id=2] mark b");
        wm.run_command("[id=1] unmark b");
        assert_eq!(marks_json(&wm), serde_json::json!(["a", "b", "c"]));
        wm.run_command("[id=1] unmark c");
        assert_eq!(marks_json(&wm), serde_json::json!(["a", "b"]));
        wm.run_command("[id=1] unmark");
        assert_eq!(marks_json(&wm), serde_json::json!(["b"]));
        wm.run_command("[id=1] mark a; unmark");
        assert_eq!(marks_json(&wm), serde_json::json!([]));
    }

    #[test]
    fn move_to_mark_clears_fullscreen_and_scratch_state() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("fullscreen enable; workspace 2");
        wm.manage(win(2));
        wm.run_command("fullscreen enable; mark t");
        wm.run_command("[id=1] move container to mark t");
        let ws = wm.focused_workspace();
        let fs = wm
            .tree
            .windows_under(ws)
            .into_iter()
            .filter(|&n| wm.tree.get(n).fullscreen)
            .count();
        assert_eq!(fs, 1);
        wm.manage(win(3));
        wm.run_command("[id=3] move scratchpad");
        wm.run_command("[id=3] move container to mark t");
        let n3 = wm.tree.window_node(WindowId(3)).unwrap();
        assert_eq!(wm.tree.get(n3).scratchpad, ScratchState::None);
        assert!(!wm.tree.is_floating(n3));
        assert!(!wm.run_command("scratchpad show")[0].success);
        check_invariants(&wm);
    }

    #[test]
    fn swap_across_workspaces_clears_fullscreen() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("fullscreen enable; workspace 2");
        wm.manage(win(2));
        wm.run_command("[id=2] swap container with id 1");
        for id in [1, 2] {
            let n = wm.tree.window_node(WindowId(id)).unwrap();
            assert!(!wm.tree.get(n).fullscreen, "w{id}");
        }
        check_invariants(&wm);
    }

    #[test]
    fn con_mark_and_con_id_target_containers() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("split v");
        wm.manage(win(3));
        assert_eq!(ws_desc(&wm), "H[w1 V[w2 w3]]");
        wm.run_command("focus parent; mark x; [id=1] focus");
        assert!(wm.run_command("[con_mark=x] layout tabbed")[0].success);
        assert_eq!(ws_desc(&wm), "H[w1 T[w2 w3]]");
        let c = wm
            .tree
            .parent(wm.tree.window_node(WindowId(2)).unwrap())
            .unwrap();
        wm.run_command(&format!("[con_id={}] layout stacking", c.0));
        assert_eq!(ws_desc(&wm), "H[w1 S[w2 w3]]");
        // A container has no window properties to match.
        wm.run_command("[con_mark=x class=.] layout splitv");
        assert_eq!(ws_desc(&wm), "H[w1 S[w2 w3]]");
        check_invariants(&wm);
    }

    #[test]
    fn floating_move_to_mark_on_other_output_shifts_rect() {
        for mark_ws in [true, false] {
            let mut wm = crate::wm::Wm::new(
                crate::config::Config::default(),
                vec![out("A", 0, 1000), out("B", 1000, 1000)],
            );
            wm.manage(win(1));
            wm.run_command("floating enable");
            let n1 = wm.tree.window_node(WindowId(1)).unwrap();
            if let crate::tree::NodeKind::Window(d) = &mut wm.tree.get_mut(n1).kind {
                let r = d.floating_rect.unwrap();
                d.floating_rect = Some(crate::geom::Rect::new(100, 50, r.w.min(300), r.h.min(200)));
            }
            wm.run_command("focus output B");
            if mark_ws {
                wm.run_command("mark t");
            } else {
                wm.manage(win(2));
                wm.run_command("floating enable; mark t");
            }
            wm.run_command("focus output A");
            assert!(wm.run_command("[id=1] move container to mark t")[0].success);
            let n1 = wm.tree.window_node(WindowId(1)).unwrap();
            let x = match &wm.tree.get(n1).kind {
                crate::tree::NodeKind::Window(d) => d.floating_rect.unwrap().x,
                _ => unreachable!(),
            };
            assert_eq!(x, 1100, "mark_ws={mark_ws}");
            check_invariants(&wm);
        }
    }

    #[test]
    fn floating_swap_across_outputs_keeps_windows_on_new_outputs() {
        let mut wm = crate::wm::Wm::new(
            crate::config::Config::default(),
            vec![out("A", 0, 1000), out("B", 1000, 1000)],
        );
        wm.manage(win(1));
        wm.run_command("floating enable");
        wm.run_command("focus output B");
        wm.manage(win(2));
        wm.run_command("floating enable; mark t");
        assert!(wm.run_command("[id=1] swap container with mark t")[0].success);
        let x = |wm: &crate::wm::Wm, id| {
            let n = wm.tree.window_node(WindowId(id)).unwrap();
            match &wm.tree.get(n).kind {
                crate::tree::NodeKind::Window(d) => d.floating_rect.unwrap().x,
                _ => unreachable!(),
            }
        };
        assert!(x(&wm, 1) >= 1000, "w1 now on B");
        assert!(x(&wm, 2) < 1000, "w2 now on A");
        check_invariants(&wm);
    }
}
