use crate::geom::Rect;
use crate::ops::floating::clamp_floating_rect;
use crate::tree::{NodeId, NodeKind, ScratchState};
use crate::wm::{IpcEvent, Wm};

const NO_SCRATCHPAD: &str = "No scratchpad windows";
const NOT_SCRATCHPAD: &str = "Matched window is not a scratchpad window";

impl Wm {
    fn scratch_state(&self, n: NodeId) -> ScratchState {
        self.tree.get(n).scratchpad
    }

    /// `move scratchpad`: every window under `target` becomes a floating
    /// scratchpad window on `__i3_scratch`.
    pub(crate) fn move_to_scratchpad(&mut self, target: NodeId) -> Result<(), String> {
        for n in self.tree.windows_under(target) {
            self.send_to_scratch(n, true);
        }
        Ok(())
    }

    /// Moves window `n` to the scratch workspace. `fresh` resets the window
    /// to the `Fresh` state (a newly moved window); hiding a shown
    /// scratchpad window keeps its state so its last position is kept.
    fn send_to_scratch(&mut self, n: NodeId, fresh: bool) {
        let Some(ws) = self.tree.workspace_of(n) else {
            return;
        };
        if fresh {
            self.tree.get_mut(n).scratchpad = ScratchState::Fresh;
        }
        if ws == self.scratch {
            return;
        }
        let was_focused = self.tree.focused_descendant(self.tree.root()) == n;
        // Size to keep: the floating rect, else the current tiled size.
        let tiled = self.tree.window_id(n).and_then(|id| {
            self.arrange()
                .windows
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.rect)
        });
        if let NodeKind::Window(d) = &mut self.tree.get_mut(n).kind {
            if d.floating_rect.is_none() {
                d.floating_rect = Some(clamp_floating_rect(
                    &self.config,
                    tiled.unwrap_or(d.info.rect),
                ));
            }
        }
        let old_parent = self.tree.parent(n);
        self.tree.get_mut(n).fullscreen = false;
        self.tree.detach(n);
        if let Some(p) = old_parent {
            if matches!(self.tree.get(p).kind, NodeKind::Container) {
                self.tree.cleanup(p);
            }
        }
        self.tree.attach_floating(self.scratch, n);
        self.scratch_order.retain(|&x| x != n);
        self.scratch_order.push(n);
        self.focused_container = None;
        if was_focused && self.is_visible(ws) {
            let f = self.tree.focused_descendant(ws);
            self.tree.set_focused(f);
        }
        self.remove_if_empty_hidden(ws);
        if let Some(w) = self.tree.window_id(n) {
            self.emit(IpcEvent::Window {
                change: "move".into(),
                window: w,
            });
        }
    }

    /// Shows scratchpad window `n` floating on the focused workspace and
    /// focuses it.
    pub(crate) fn show_scratch_window(&mut self, n: NodeId) {
        let dest = self.focused_workspace();
        let src = self.tree.workspace_of(n);
        let out_rect = self
            .tree
            .output_of(dest)
            .and_then(|o| match &self.tree.get(o).kind {
                NodeKind::Output { rect, .. } => Some(*rect),
                _ => None,
            });
        let state = self.scratch_state(n);
        if src != Some(dest) {
            let old_parent = self.tree.parent(n);
            self.tree.detach(n);
            if let Some(p) = old_parent {
                if matches!(self.tree.get(p).kind, NodeKind::Container) {
                    self.tree.cleanup(p);
                }
            }
            self.tree.attach_floating(dest, n);
        }
        if let NodeKind::Window(d) = &mut self.tree.get_mut(n).kind {
            let fallback = d.info.rect;
            let r = clamp_floating_rect(&self.config, d.floating_rect.unwrap_or(fallback));
            d.floating_rect = Some(match out_rect {
                Some(o) if state == ScratchState::Fresh => {
                    clamp_floating_rect(&self.config, Rect::new(0, 0, o.w / 2, o.h * 3 / 4))
                        .centered_in(&o)
                }
                _ => r,
            });
        }
        self.tree.get_mut(n).scratchpad = ScratchState::Changed;
        self.scratch_order.retain(|&x| x != n);
        self.focused_container = None;
        self.tree.set_focused(n);
        if let Some(src) = src {
            if src != dest {
                self.remove_if_empty_hidden(src);
            }
        }
        if let Some(w) = self.tree.window_id(n) {
            self.emit(IpcEvent::Window {
                change: "floating".into(),
                window: w,
            });
        }
    }

    /// `scratchpad show` without criteria (i3 semantics).
    pub(crate) fn scratchpad_show(&mut self) -> Result<(), String> {
        let focused_ws = self.focused_workspace();
        let leaf = self.tree.focused_descendant(self.tree.root());
        if matches!(self.tree.get(leaf).kind, NodeKind::Window(_))
            && self.scratch_state(leaf) != ScratchState::None
            && self.tree.workspace_of(leaf) == Some(focused_ws)
        {
            self.send_to_scratch(leaf, false);
            return Ok(());
        }
        let visible = self
            .tree
            .get(focused_ws)
            .focus
            .iter()
            .copied()
            .find(|&c| self.scratch_state(c) != ScratchState::None && self.tree.is_floating(c));
        if let Some(v) = visible {
            self.focused_container = None;
            self.tree.set_focused(v);
            return Ok(());
        }
        // A scratchpad window visible on another output: bring it here.
        let elsewhere = self
            .outputs()
            .into_iter()
            .map(|o| self.visible_workspace(o))
            .filter(|&w| w != focused_ws)
            .find_map(|w| {
                self.tree.get(w).focus.iter().copied().find(|&c| {
                    self.scratch_state(c) != ScratchState::None && self.tree.is_floating(c)
                })
            });
        if let Some(n) = elsewhere {
            self.show_scratch_window(n);
            return Ok(());
        }
        self.scratch_order.retain(|&x| self.tree.contains(x));
        let hidden = self
            .scratch_order
            .iter()
            .rev()
            .copied()
            .find(|&x| self.tree.workspace_of(x) == Some(self.scratch));
        match hidden {
            Some(n) => {
                self.show_scratch_window(n);
                Ok(())
            }
            None => Err(NO_SCRATCHPAD.into()),
        }
    }

    /// `[criteria] scratchpad show`: toggles each matched scratchpad window.
    pub(crate) fn scratchpad_show_matched(&mut self, matched: &[NodeId]) -> Result<(), String> {
        let mut error = None;
        for &n in matched {
            if !self.tree.contains(n) {
                continue;
            }
            if !matches!(self.tree.get(n).kind, NodeKind::Window(_))
                || self.scratch_state(n) == ScratchState::None
            {
                error.get_or_insert_with(|| NOT_SCRATCHPAD.to_string());
                continue;
            }
            if self.tree.workspace_of(n) == Some(self.focused_workspace()) {
                self.send_to_scratch(n, false);
            } else {
                self.show_scratch_window(n);
            }
        }
        match error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::geom::Rect;
    use crate::ipc::{outputs_json, tree_json, workspaces_json};
    use crate::testutil::*;
    use crate::tree::{NodeKind, WindowId};

    #[test]
    fn scratchpad_round_trip() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("[id=2] move scratchpad");
        assert_eq!(ws_desc(&wm), "H[w1]");
        let n2 = wm.tree.window_node(WindowId(2)).unwrap();
        assert_eq!(wm.tree.workspace_of(n2), Some(wm.scratch_workspace()));
        wm.run_command("scratchpad show");
        assert_eq!(wm.tree.workspace_of(n2), Some(wm.focused_workspace()));
        assert!(wm.tree.is_floating(n2));
        assert_eq!(wm.focused_window(), Some(WindowId(2)));
        wm.run_command("scratchpad show");
        assert_eq!(wm.tree.workspace_of(n2), Some(wm.scratch_workspace()));
        assert!(!test_wm().run_command("scratchpad show")[0].success);
        check_invariants(&wm);
    }
    #[test]
    fn scratchpad_windows_are_in_arrangement_hidden() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("move scratchpad");
        let a = wm.arrange();
        assert!(a.hidden.contains(&WindowId(1)));
        assert!(a.windows.iter().all(|p| p.id != WindowId(1)));
    }
    #[test]
    fn scratch_output_is_invisible_everywhere() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("move scratchpad");
        assert_eq!(wm.outputs().len(), 1);
        let outs = outputs_json(&wm);
        assert!(outs.as_array().unwrap().iter().all(|o| o["name"] != "__i3"));
        let wss = workspaces_json(&wm);
        assert!(wss
            .as_array()
            .unwrap()
            .iter()
            .all(|w| w["name"] != "__i3_scratch"));
        wm.run_command("workspace next");
        assert_ne!(wm.workspace_name(wm.focused_workspace()), "__i3_scratch");
        assert!(tree_json(&wm).to_string().contains("__i3_scratch"));
        wm.set_outputs(vec![out("O1", 0, 800)]);
        assert!(wm.tree.contains(wm.scratch_workspace()));
    }
    #[test]
    fn scratchpad_show_with_criteria_toggles_matching() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("[id=2] move scratchpad");
        wm.run_command("[id=2] scratchpad show");
        assert_eq!(wm.focused_window(), Some(WindowId(2)));
        wm.run_command("[id=2] scratchpad show");
        assert_eq!(wm.focused_window(), Some(WindowId(1)));
        assert!(!wm.run_command("[id=1] scratchpad show")[0].success);
    }
    #[test]
    fn scratchpad_fresh_centers_then_keeps_rect_and_state_in_json() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("floating enable");
        wm.on_floating_moved(WindowId(1), Rect::new(5, 5, 400, 300));
        wm.run_command("move container to scratchpad");
        assert!(tree_json(&wm)
            .to_string()
            .contains("\"scratchpad_state\":\"fresh\""));
        wm.run_command("scratchpad show");
        let n = wm.tree.window_node(WindowId(1)).unwrap();
        let rect = |wm: &crate::wm::Wm| match &wm.tree.get(n).kind {
            NodeKind::Window(d) => d.floating_rect.unwrap(),
            _ => unreachable!(),
        };
        assert_eq!(
            rect(&wm),
            Rect::new(0, 0, 500, 375).centered_in(&Rect::new(0, 0, 1000, 500))
        );
        wm.on_floating_moved(WindowId(1), Rect::new(10, 20, 400, 300));
        wm.run_command("scratchpad show");
        wm.run_command("scratchpad show");
        assert_eq!(rect(&wm), Rect::new(10, 20, 400, 300));
        assert!(tree_json(&wm)
            .to_string()
            .contains("\"scratchpad_state\":\"changed\""));
    }
    #[test]
    fn floating_disable_ends_scratchpad_and_focus_command_shows_it() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("[id=2] move scratchpad");
        wm.run_command("[id=2] focus");
        assert_eq!(wm.focused_window(), Some(WindowId(2)));
        wm.run_command("floating disable");
        assert_eq!(ws_desc(&wm), "H[w1 w2]");
        assert!(!wm.run_command("[id=2] scratchpad show")[0].success);
        check_invariants(&wm);
    }
    #[test]
    fn closing_focused_scratch_hides_and_focuses_next() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("move scratchpad");
        assert_eq!(wm.focused_window(), Some(WindowId(1)));
        wm.unmanage(WindowId(2));
        assert!(!wm.run_command("scratchpad show")[0].success);
    }
    #[test]
    fn scratch_workspace_is_never_visible_or_focused_via_marks_and_swap() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.manage(win(3));
        wm.run_command("[id=3] move scratchpad");
        wm.run_command("[id=3] mark sp");
        let real = wm.focused_output();
        assert!(!wm.is_visible(wm.scratch_workspace()));
        wm.run_command("[id=1] move to mark sp");
        assert_eq!(wm.focused_output(), real);
        assert_eq!(wm.focused_window(), Some(WindowId(2)));
        wm.run_command("[id=2] swap container with mark sp");
        assert_eq!(wm.focused_output(), real);
        assert_ne!(wm.workspace_name(wm.focused_workspace()), "__i3_scratch");
        wm.manage(win(4));
        assert_eq!(
            wm.tree
                .workspace_of(wm.tree.window_node(WindowId(4)).unwrap()),
            Some(wm.focused_workspace())
        );
        check_invariants(&wm);
    }
    #[test]
    fn floating_disable_refused_on_hidden_scratch_window() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("move scratchpad");
        let r = wm.run_command("[id=1] floating disable");
        assert!(!r[0].success);
        assert!(!wm.run_command("[id=1] floating toggle")[0].success);
        let n = wm.tree.window_node(WindowId(1)).unwrap();
        assert_eq!(wm.tree.workspace_of(n), Some(wm.scratch_workspace()));
        assert!(wm.tree.is_floating(n));
        assert!(wm.run_command("scratchpad show")[0].success);
    }
    #[test]
    fn scratchpad_show_visible_on_other_output_moves_here() {
        let mut wm = crate::wm::Wm::new(
            crate::config::Config::default(),
            vec![out("A", 0, 1000), out("B", 1000, 1000)],
        );
        wm.manage(win(1));
        wm.run_command("move scratchpad");
        wm.run_command("scratchpad show");
        wm.run_command("focus output B");
        wm.run_command("scratchpad show");
        let n = wm.tree.window_node(WindowId(1)).unwrap();
        assert_eq!(wm.tree.workspace_of(n), Some(wm.focused_workspace()));
        assert_eq!(wm.focused_window(), Some(WindowId(1)));
    }
    #[test]
    fn reserved_workspace_names_rejected() {
        let mut wm = test_wm();
        assert!(!wm.run_command("workspace __foo")[0].success);
        wm.manage(win(1));
        assert!(!wm.run_command("move to workspace __i3_scratch")[0].success);
        let c = crate::config::parse_config(
            "workspace __x output A
",
        );
        assert_eq!(c.warnings.len(), 1);
    }

    #[test]
    fn scratchpad_show_respects_floating_maximum_size() {
        let mut wm = crate::wm::Wm::new(
            crate::config::parse_config("floating_maximum_size 300 x 200\n"),
            vec![out("O1", 0, 1000)],
        );
        wm.manage(win(1));
        wm.run_command("move container to scratchpad");
        wm.run_command("scratchpad show");
        let n = wm.tree.window_node(WindowId(1)).unwrap();
        match &wm.tree.get(n).kind {
            NodeKind::Window(d) => {
                let r = d.floating_rect.unwrap();
                assert!(r.w <= 300 && r.h <= 200, "{r:?}");
            }
            _ => unreachable!(),
        }
    }
}
