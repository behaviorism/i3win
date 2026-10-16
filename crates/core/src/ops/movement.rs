use crate::geom::{sanitize_floating, Direction};
use crate::tree::{Layout, NodeId, NodeKind};
use crate::wm::{IpcEvent, Wm};

/// How the walk up the tree for a directional move ended.
enum Outcome {
    Done,
    Output,
}

impl Wm {
    pub fn move_direction(
        &mut self,
        target: NodeId,
        dir: Direction,
        px: Option<i32>,
    ) -> Result<(), String> {
        let n = target;
        if matches!(self.tree.get(n).kind, NodeKind::Workspace { .. }) {
            return Ok(());
        }
        if self.tree.is_floating(n) {
            let d = px.unwrap_or(10);
            if let NodeKind::Window(w) = &mut self.tree.get_mut(n).kind {
                if let Some(r) = w.floating_rect.as_mut() {
                    match dir {
                        Direction::Left => r.x = r.x.saturating_sub(d),
                        Direction::Right => r.x = r.x.saturating_add(d),
                        Direction::Up => r.y = r.y.saturating_sub(d),
                        Direction::Down => r.y = r.y.saturating_add(d),
                    }
                    *r = sanitize_floating(*r);
                }
            }
            self.emit_moved(n);
            return Ok(());
        }
        if self.tree.parent(n).is_none() {
            return Ok(());
        }
        let old_focused_ws = self.workspace_name(self.focused_workspace()).to_string();
        let mut new_ws = None;
        if matches!(self.walk_move(n, dir), Outcome::Output) {
            match self.move_to_adjacent_output(n, dir) {
                Some(ws2) => new_ws = Some(ws2),
                None => return Ok(()),
            }
        }
        self.focused_container = None;
        if new_ws.is_some() {
            self.tree.set_focused(n);
        } else {
            self.focus_in_place(n);
        }
        if matches!(self.tree.get(n).kind, NodeKind::Container) {
            let leaf = self.tree.focused_descendant(n);
            self.focused_container = Some((n, leaf));
        }
        if let Some(ws2) = new_ws {
            self.note_workspace_focus(old_focused_ws, ws2);
        }
        self.emit_moved(n);
        Ok(())
    }

    fn emit_moved(&mut self, n: NodeId) {
        for w in self.tree.windows_under(n) {
            if let Some(id) = self.tree.window_id(w) {
                self.emit(IpcEvent::Window {
                    change: "move".into(),
                    window: id,
                });
            }
        }
    }

    fn cleanup_container(&mut self, p: Option<NodeId>) {
        if let Some(p) = p {
            if self.tree.contains(p) && matches!(self.tree.get(p).kind, NodeKind::Container) {
                self.tree.cleanup(p);
            }
        }
    }

    fn walk_move(&mut self, n: NodeId, dir: Direction) -> Outcome {
        let forward = dir.is_forward();
        let mut cur = n;
        loop {
            let Some(p) = self.tree.parent(cur) else {
                return Outcome::Done;
            };
            let is_ws = matches!(self.tree.get(p).kind, NodeKind::Workspace { .. });
            if self.tree.get(p).layout.orientation() != dir.orientation() {
                if is_ws {
                    if self.tree.get(p).children.len() <= 1 {
                        return Outcome::Output;
                    }
                    self.reorient_workspace(p, dir);
                } else {
                    cur = p;
                }
                continue;
            }
            let Some(i) = self.tree.index_in_parent(cur) else {
                return Outcome::Done;
            };
            if cur == n {
                let neighbor = if forward {
                    self.tree.get(p).children.get(i + 1).copied()
                } else {
                    i.checked_sub(1)
                        .and_then(|k| self.tree.get(p).children.get(k).copied())
                };
                match neighbor {
                    Some(nb) if matches!(self.tree.get(nb).kind, NodeKind::Window(_)) => {
                        self.tree.swap(n, nb);
                    }
                    Some(nb) => {
                        let old = self.tree.parent(n);
                        self.tree.detach(n);
                        let at = if forward { Some(0) } else { None };
                        self.tree.attach(nb, n, at);
                        self.cleanup_container(old);
                    }
                    None if is_ws => return Outcome::Output,
                    None => {
                        cur = p;
                        continue;
                    }
                }
                return Outcome::Done;
            }
            let old = self.tree.parent(n);
            self.tree.detach(n);
            let ci = self.tree.index_in_parent(cur).unwrap_or(i);
            self.tree
                .attach(p, n, Some(if forward { ci + 1 } else { ci }));
            self.cleanup_container(old);
            return Outcome::Done;
        }
    }

    /// Wraps all tiling children of `ws` in one container that keeps the
    /// workspace's current layout, then flips the workspace's own layout.
    fn reorient_workspace(&mut self, ws: NodeId, dir: Direction) {
        let old_layout = self.tree.get(ws).layout;
        let c = self.tree.new_node(NodeKind::Container, old_layout);
        let kids = self.tree.get(ws).children.clone();
        let focus = self.tree.get(ws).focus.clone();
        let mut new_ws_focus = Vec::new();
        let mut c_focus = Vec::new();
        let mut inserted = false;
        for f in focus {
            if kids.contains(&f) {
                c_focus.push(f);
                if !inserted {
                    new_ws_focus.push(c);
                    inserted = true;
                }
            } else {
                new_ws_focus.push(f);
            }
        }
        for &k in &kids {
            self.tree.get_mut(k).parent = Some(c);
        }
        let cn = self.tree.get_mut(c);
        cn.children = kids;
        cn.focus = c_focus;
        cn.parent = Some(ws);
        cn.percent = 1.0;
        let w = self.tree.get_mut(ws);
        w.children = vec![c];
        w.focus = new_ws_focus;
        w.layout = match dir {
            Direction::Left | Direction::Right => Layout::SplitH,
            Direction::Up | Direction::Down => Layout::SplitV,
        };
    }

    /// Moves `n` to the visible workspace of the output in `dir`.
    /// Returns the destination workspace, or None when there is no such output.
    fn move_to_adjacent_output(&mut self, n: NodeId, dir: Direction) -> Option<NodeId> {
        let from = self.tree.output_of(n)?;
        let o = self.output_in_direction(from, dir)?;
        let ws2 = self.visible_workspace(o);
        let old = self.tree.parent(n);
        self.tree.detach(n);
        self.cleanup_container(old);
        let at = if dir.is_forward() { Some(0) } else { None };
        self.tree.attach(ws2, n, at);
        Some(ws2)
    }
}

#[cfg(test)]
mod tests {
    use crate::config::Config;
    use crate::testutil::*;
    use crate::tree::{Layout, NodeKind, WindowId};
    use crate::wm::Wm;

    #[test]
    fn swap_with_window_neighbor() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("move left");
        assert_eq!(ws_desc(&wm), "H[w2 w1]");
        assert_eq!(wm.focused_window(), Some(WindowId(2)));
    }
    #[test]
    fn edge_is_noop() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("move right");
        assert_eq!(ws_desc(&wm), "H[w1 w2]");
    }
    #[test]
    fn move_into_container_neighbor() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        let n1 = wm.tree.window_node(WindowId(1)).unwrap();
        let c = wm.tree.wrap(n1, Layout::SplitV);
        wm.tree.set_focused(n1);
        wm.manage(win(3));
        wm.tree
            .set_focused(wm.tree.window_node(WindowId(2)).unwrap());
        wm.run_command("move left");
        assert_eq!(ws_desc(&wm), "H[V[w1 w3 w2]]");
        let _ = c;
    }
    #[test]
    fn move_out_of_vertical_container() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        let n2 = wm.tree.window_node(WindowId(2)).unwrap();
        wm.tree.wrap(n2, Layout::SplitV);
        wm.tree.set_focused(n2);
        wm.manage(win(3));
        wm.run_command("move right");
        assert_eq!(ws_desc(&wm), "H[w1 V[w2] w3]");
    }
    #[test]
    fn container_emptied_by_move_is_cleaned() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        let n2 = wm.tree.window_node(WindowId(2)).unwrap();
        wm.tree.wrap(n2, Layout::SplitV);
        wm.tree.set_focused(n2);
        wm.run_command("move left");
        assert_eq!(ws_desc(&wm), "H[w1 w2]");
    }
    #[test]
    fn vertical_move_reorients_workspace() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.manage(win(3));
        wm.run_command("move down");
        assert_eq!(ws_desc(&wm), "V[H[w1 w2] w3]");
    }
    #[test]
    fn floating_move_shifts_rect() {
        let mut wm = test_wm();
        let mut f = win(1);
        f.wants_float = true;
        wm.manage(f);
        wm.run_command("move left 50 px");
        let n = wm.tree.window_node(WindowId(1)).unwrap();
        match &wm.tree.get(n).kind {
            NodeKind::Window(d) => assert_eq!(d.floating_rect.unwrap().x, 250),
            _ => panic!(),
        }
    }
    #[test]
    fn floating_move_huge_px_does_not_overflow() {
        let mut wm = test_wm();
        let mut f = win(1);
        f.wants_float = true;
        wm.manage(f);
        let r = wm.run_command("move right 2147483647 px");
        assert!(r[0].success);
        let r = wm.run_command("move left 2147483647 px");
        assert!(r[0].success);
    }
    #[test]
    fn cross_output_event_old_is_focused_workspace() {
        let mut wm = Wm::new(
            Config::default(),
            vec![out("A", 0, 1000), out("B", 1000, 1000)],
        );
        wm.manage(win(1));
        wm.take_effects();
        wm.run_command("move right");
        assert!(wm.take_effects().contains(&crate::wm::Effect::Ipc(
            crate::wm::IpcEvent::Workspace {
                change: "focus".into(),
                current: Some("2".into()),
                old: Some("1".into()),
            }
        )));
    }
    #[test]
    fn move_across_outputs() {
        let mut wm = Wm::new(
            Config::default(),
            vec![out("A", 0, 1000), out("B", 1000, 1000)],
        );
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("move right");
        wm.run_command("move right");
        let b = wm.outputs()[1];
        assert_eq!(wm.tree.describe(wm.visible_workspace(b)), "H[w2]");
        assert_eq!(wm.focused_window(), Some(WindowId(2)));
    }
}
