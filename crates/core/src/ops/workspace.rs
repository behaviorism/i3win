use crate::command::{OutputTarget, WorkspaceTarget};
use crate::geom::{Direction, Rect};
use crate::ops::floating::clamp_floating_rect;
use crate::tree::{Layout, NodeId, NodeKind};
use crate::wm::{workspace_num, IpcEvent, OutputInfo, Wm};

fn sort_key(num: Option<i32>, name: &str) -> (bool, i32, String) {
    (num.is_none(), num.unwrap_or(0), name.to_string())
}

impl Wm {
    fn workspace_key(&self, ws: NodeId) -> (bool, i32, String) {
        match &self.tree.get(ws).kind {
            NodeKind::Workspace { name, num } => sort_key(*num, name),
            _ => (true, 0, String::new()),
        }
    }

    pub(crate) fn output_rect(&self, output: NodeId) -> Rect {
        match &self.tree.get(output).kind {
            NodeKind::Output { rect, .. } => *rect,
            _ => Rect::default(),
        }
    }

    fn output_name(&self, output: NodeId) -> &str {
        match &self.tree.get(output).kind {
            NodeKind::Output { name, .. } => name,
            _ => "",
        }
    }

    /// Shifts the floating window `n` from output rect `from` to `to` and
    /// keeps it inside `to`.
    pub(crate) fn shift_floating(&mut self, n: NodeId, from: Rect, to: Rect) {
        let dx = i64::from(to.x) - i64::from(from.x);
        let dy = i64::from(to.y) - i64::from(from.y);
        let sat = |v: i32, d: i64| {
            (i64::from(v) + d).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
        };
        if let NodeKind::Window(d) = &mut self.tree.get_mut(n).kind {
            if let Some(r) = d.floating_rect.as_mut() {
                r.x = sat(r.x, dx);
                r.y = sat(r.y, dy);
                r.x = r.x.clamp(to.x, to.x.max(to.right().saturating_sub(r.w)));
                r.y = r.y.clamp(to.y, to.y.max(to.bottom().saturating_sub(r.h)));
                *r = clamp_floating_rect(&self.config, *r);
            }
        }
    }

    fn shift_workspace_floating(&mut self, ws: NodeId, from: Rect, to: Rect) {
        for f in self.tree.get(ws).floating.clone() {
            self.shift_floating(f, from, to);
        }
    }

    /// Output a newly created workspace `name` belongs on: the first existing
    /// output configured for it, else the focused output.
    pub(crate) fn output_for_new_workspace(&self, name: &str) -> NodeId {
        self.configured_output(name)
            .unwrap_or_else(|| self.focused_output())
    }

    /// First existing output that `workspace_output` assigns workspace `name` to.
    fn configured_output(&self, name: &str) -> Option<NodeId> {
        self.config
            .workspace_outputs
            .iter()
            .find(|(ws, _)| ws == name)
            .and_then(|(_, outs)| {
                outs.iter().find_map(|o| {
                    self.outputs()
                        .into_iter()
                        .find(|&n| self.output_name(n) == o)
                })
            })
    }

    fn is_workspace_empty(&self, ws: NodeId) -> bool {
        let n = self.tree.get(ws);
        n.children.is_empty() && n.floating.is_empty()
    }

    /// Removes `ws` if it is empty and not visible; emits the "empty" event.
    pub(crate) fn remove_if_empty_hidden(&mut self, ws: NodeId) {
        if self.tree.contains(ws)
            && ws != self.scratch
            && self.is_workspace_empty(ws)
            && !self.is_visible(ws)
        {
            let name = self.workspace_name(ws).to_string();
            self.tree.remove(ws);
            self.emit(IpcEvent::Workspace {
                change: "empty".into(),
                current: Some(name),
                old: None,
            });
        }
    }

    /// Attaches `ws` to `output` keeping numbered-then-named sort order.
    pub(crate) fn attach_workspace_sorted(&mut self, output: NodeId, ws: NodeId) {
        let new_key = self.workspace_key(ws);
        let idx = self
            .tree
            .get(output)
            .children
            .iter()
            .position(|&c| self.workspace_key(c) > new_key);
        self.tree.attach(output, ws, idx);
    }

    /// All workspaces ordered by (num with None last, name).
    pub(crate) fn sorted_workspaces(&self) -> Vec<NodeId> {
        let mut all: Vec<NodeId> = self
            .outputs()
            .into_iter()
            .flat_map(|o| self.tree.get(o).children.clone())
            .collect();
        all.sort_by_key(|&w| self.workspace_key(w));
        all
    }

    /// Re-attaches every sticky floating window of `from` to `to` (same rect).
    fn move_sticky(&mut self, from: NodeId, to: NodeId) {
        if from == self.scratch || to == self.scratch {
            return;
        }
        let focused = self.focused();
        for n in self.tree.get(from).floating.clone() {
            if !self.tree.get(n).sticky {
                continue;
            }
            self.tree.detach(n);
            self.tree.attach_floating(to, n);
            if n == focused {
                let focus = &mut self.tree.get_mut(to).focus;
                focus.retain(|&c| c != n);
                focus.insert(0, n);
            }
        }
    }

    pub fn show_workspace(&mut self, ws: NodeId) {
        let old_global = self.focused_workspace();
        let old_global_name = self.workspace_name(old_global).to_string();
        let Some(out) = self.tree.output_of(ws) else {
            return;
        };
        let old_vis = self.visible_workspace(out);
        if old_vis != ws {
            self.move_sticky(old_vis, ws);
        }
        let f = self.tree.focused_descendant(ws);
        self.tree.set_focused(f);
        if old_vis != ws {
            self.remove_if_empty_hidden(old_vis);
        }
        if old_global != ws {
            self.remove_if_empty_hidden(old_global);
            self.note_workspace_focus(old_global_name, ws);
        }
    }

    /// Records `old_name` as the previous workspace and emits the
    /// workspace "focus" event for `ws`.
    pub(crate) fn note_workspace_focus(&mut self, old_name: String, ws: NodeId) {
        self.prev_workspace = Some(old_name.clone());
        let current = self.workspace_name(ws).to_string();
        self.emit(IpcEvent::Workspace {
            change: "focus".into(),
            current: Some(current),
            old: Some(old_name),
        });
    }

    pub(crate) fn resolve_workspace(&mut self, t: &WorkspaceTarget) -> Option<NodeId> {
        match t {
            WorkspaceTarget::Name(s) => Some(self.find_or_create_workspace(s)),
            WorkspaceTarget::Number(n) => {
                let existing = self.sorted_workspaces().into_iter().find(|&w| {
                    matches!(&self.tree.get(w).kind,
                        NodeKind::Workspace { num: Some(k), .. } if k == n)
                });
                match existing {
                    Some(w) => Some(w),
                    None => Some(self.find_or_create_workspace(&n.to_string())),
                }
            }
            WorkspaceTarget::Next | WorkspaceTarget::Prev => {
                let all = self.sorted_workspaces();
                if all.len() < 2 {
                    return None;
                }
                let cur = self.focused_workspace();
                let i = all.iter().position(|&w| w == cur)?;
                let j = if matches!(t, WorkspaceTarget::Next) {
                    (i + 1) % all.len()
                } else {
                    (i + all.len() - 1) % all.len()
                };
                Some(all[j])
            }
            WorkspaceTarget::BackAndForth => {
                let name = self.prev_workspace.clone()?;
                Some(self.find_or_create_workspace(&name))
            }
        }
    }

    pub(crate) fn find_or_create_workspace(&mut self, name: &str) -> NodeId {
        match self.find_workspace(name) {
            Some(w) => w,
            None => {
                let out = self.output_for_new_workspace(name);
                self.create_workspace(out, name)
            }
        }
    }

    /// `workspace [--no-auto-back-and-forth] TARGET`: with
    /// `workspace_auto_back_and_forth`, naming the focused workspace acts as
    /// `workspace back_and_forth`.
    pub(crate) fn workspace_command(
        &mut self,
        t: &WorkspaceTarget,
        no_auto_back_and_forth: bool,
    ) -> Result<(), String> {
        if self.config.workspace_auto_back_and_forth && !no_auto_back_and_forth {
            let cur = self.focused_workspace();
            let is_current = match (t, &self.tree.get(cur).kind) {
                (WorkspaceTarget::Name(s), NodeKind::Workspace { name, .. }) => s == name,
                (WorkspaceTarget::Number(n), NodeKind::Workspace { num, .. }) => *num == Some(*n),
                _ => false,
            };
            if is_current {
                return self.switch_workspace(&WorkspaceTarget::BackAndForth);
            }
        }
        self.switch_workspace(t)
    }

    /// `rename workspace [OLD] to NEW`.
    pub(crate) fn rename_workspace(&mut self, old: Option<&str>, new: &str) -> Result<(), String> {
        if new.starts_with("__") {
            return Err("Workspace names starting with __ are reserved".into());
        }
        let (old_name, ws) = match old {
            Some(o) => (
                o.to_string(),
                self.find_workspace(o)
                    .ok_or_else(|| format!("Workspace {o} not found"))?,
            ),
            None => {
                let ws = self.focused_workspace();
                if ws == self.scratch {
                    return Err("Workspace __i3_scratch not found".into());
                }
                (self.workspace_name(ws).to_string(), ws)
            }
        };
        if self.find_workspace(new).is_some() {
            return Err(format!("New workspace name \"{new}\" already in use"));
        }
        let was_focused = self.focused_workspace() == ws;
        let src_out = self.tree.output_of(ws).ok_or("workspace has no output")?;
        let dest_out = self.configured_output(new).unwrap_or(src_out);
        let focus_idx = self.tree.get(src_out).focus.iter().position(|&c| c == ws);
        if let NodeKind::Workspace { name, num } = &mut self.tree.get_mut(ws).kind {
            *name = new.to_string();
            *num = workspace_num(new);
        }
        if self.prev_workspace.as_deref() == Some(old_name.as_str()) {
            self.prev_workspace = Some(new.to_string());
        }
        if dest_out == src_out {
            self.tree.detach(ws);
            self.attach_workspace_sorted(src_out, ws);
            if let Some(i) = focus_idx {
                let focus = &mut self.tree.get_mut(src_out).focus;
                focus.retain(|&c| c != ws);
                focus.insert(i.min(focus.len()), ws);
            }
        } else {
            self.relocate_workspace(ws, src_out, dest_out);
            if was_focused {
                self.show_workspace(ws);
            }
        }
        self.emit(IpcEvent::Workspace {
            change: "rename".into(),
            current: Some(new.to_string()),
            old: None,
        });
        Ok(())
    }

    pub fn switch_workspace(&mut self, t: &WorkspaceTarget) -> Result<(), String> {
        if let Some(ws) = self.resolve_workspace(t) {
            self.show_workspace(ws);
        }
        Ok(())
    }

    pub fn move_to_workspace(&mut self, target: NodeId, t: &WorkspaceTarget) -> Result<(), String> {
        if matches!(self.tree.get(target).kind, NodeKind::Workspace { .. }) {
            return Ok(());
        }
        if self.tree.workspace_of(target).is_none() {
            return Ok(());
        }
        let Some(dest) = self.resolve_workspace(t) else {
            return Ok(());
        };
        self.move_to_workspace_node(target, dest)
    }

    pub fn move_to_output(&mut self, target: NodeId, t: &OutputTarget) -> Result<(), String> {
        let o = self.resolve_output(t).ok_or("no such output")?;
        let dest = self.visible_workspace(o);
        self.move_to_workspace_node(target, dest)
    }

    fn move_to_workspace_node(&mut self, target: NodeId, dest: NodeId) -> Result<(), String> {
        self.move_node_to_workspace(target, dest, false)
    }

    /// Moves `target` to workspace `dest`; a tiled node goes next to the
    /// focused tiled window, or (`append`) to the end of the workspace root.
    pub(crate) fn move_node_to_workspace(
        &mut self,
        target: NodeId,
        dest: NodeId,
        append: bool,
    ) -> Result<(), String> {
        if matches!(self.tree.get(target).kind, NodeKind::Workspace { .. }) {
            return Ok(());
        }
        let Some(src) = self.tree.workspace_of(target) else {
            return Ok(());
        };
        if dest == src {
            return Ok(());
        }
        let was_floating = self.tree.is_floating(target);
        let src_rect = self.tree.output_of(src).map(|o| self.output_rect(o));
        let src_was_focused = self.is_visible(src) && self.focused_workspace() == src;
        let old_parent = self.tree.parent(target);
        let moved: Vec<_> = self
            .tree
            .windows_under(target)
            .into_iter()
            .filter_map(|n| self.tree.window_id(n))
            .collect();
        self.tree.detach(target);
        if let Some(p) = old_parent {
            if matches!(self.tree.get(p).kind, NodeKind::Container) {
                self.tree.cleanup(p);
            }
        }
        if was_floating {
            self.tree.attach_floating(dest, target);
            let dest_rect = self.tree.output_of(dest).map(|o| self.output_rect(o));
            if let (Some(from), Some(to)) = (src_rect, dest_rect) {
                self.shift_floating(target, from, to);
            }
        } else if append {
            self.tree.attach(dest, target, None);
        } else {
            self.insert_tiled(dest, target);
        }
        self.on_node_rehomed(target, Some(src), dest);
        let f = self.tree.focused_descendant(target);
        self.tree.set_focused_within(f, dest);
        if src_was_focused {
            let f = self.tree.focused_descendant(src);
            self.tree.set_focused(f);
        }
        self.remove_if_empty_hidden(src);
        for w in moved {
            self.emit(IpcEvent::Window {
                change: "move".into(),
                window: w,
            });
        }
        Ok(())
    }

    pub(crate) fn output_in_direction(&self, from: NodeId, dir: Direction) -> Option<NodeId> {
        let (fx, fy) = self.output_rect(from).center();
        self.outputs()
            .into_iter()
            .filter(|&o| o != from)
            .filter_map(|o| {
                let (cx, cy) = self.output_rect(o).center();
                let (primary, secondary) = match dir {
                    Direction::Left => (fx - cx, (fy - cy).abs()),
                    Direction::Right => (cx - fx, (fy - cy).abs()),
                    Direction::Up => (fy - cy, (fx - cx).abs()),
                    Direction::Down => (cy - fy, (fx - cx).abs()),
                };
                (primary > 0).then_some((primary, secondary, o))
            })
            .min_by_key(|&(p, s, _)| (p, s))
            .map(|(_, _, o)| o)
    }

    fn resolve_output(&self, t: &OutputTarget) -> Option<NodeId> {
        match t {
            OutputTarget::Dir(d) => self.output_in_direction(self.focused_output(), *d),
            OutputTarget::Name(n) => self
                .outputs()
                .into_iter()
                .find(|&o| self.output_name(o) == n),
        }
    }

    pub fn focus_output(&mut self, t: &OutputTarget) -> Result<(), String> {
        let o = self.resolve_output(t).ok_or("no such output")?;
        let old = self.focused_workspace();
        let ws = self.visible_workspace(o);
        let f = self.tree.focused_descendant(ws);
        self.tree.set_focused(f);
        if old != ws {
            let old_name = self.workspace_name(old).to_string();
            self.note_workspace_focus(old_name, ws);
        }
        Ok(())
    }

    /// Moves `ws` from `src_out` to `dest_out`, first giving `src_out` a
    /// placeholder workspace if `ws` was its only one, and carrying floating
    /// windows along.
    fn relocate_workspace(&mut self, ws: NodeId, src_out: NodeId, dest_out: NodeId) {
        if self.tree.get(src_out).children.len() == 1 {
            let mut n = 1;
            while self.find_workspace(&n.to_string()).is_some() {
                n += 1;
            }
            self.create_workspace(src_out, &n.to_string());
        }
        let (from, to) = (self.output_rect(src_out), self.output_rect(dest_out));
        self.tree.detach(ws);
        self.attach_workspace_sorted(dest_out, ws);
        self.shift_workspace_floating(ws, from, to);
    }

    pub fn move_workspace_to_output(&mut self, t: &OutputTarget) -> Result<(), String> {
        let ws = self.focused_workspace();
        let dest = self.resolve_output(t).ok_or("no such output")?;
        let Some(src_out) = self.tree.output_of(ws) else {
            return Ok(());
        };
        if dest == src_out {
            return Ok(());
        }
        self.relocate_workspace(ws, src_out, dest);
        self.show_workspace(ws);
        let name = self.workspace_name(ws).to_string();
        self.emit(IpcEvent::Workspace {
            change: "move".into(),
            current: Some(name),
            old: None,
        });
        Ok(())
    }

    pub fn set_outputs(&mut self, outputs: Vec<OutputInfo>) {
        if outputs.is_empty() {
            return;
        }
        let names: Vec<String> = outputs.iter().map(|o| o.name.clone()).collect();
        let focused_out = self.focused_output();
        for o in outputs {
            let existing = self
                .outputs()
                .into_iter()
                .find(|&n| self.output_name(n) == o.name);
            match existing {
                Some(n) => {
                    let old_rect = self.output_rect(n);
                    if old_rect != o.rect {
                        for ws in self.tree.get(n).children.clone() {
                            self.shift_workspace_floating(ws, old_rect, o.rect);
                        }
                    }
                    self.tree.get_mut(n).kind = NodeKind::Output {
                        name: o.name,
                        rect: o.rect,
                        scale: o.scale,
                        primary: o.primary,
                    };
                }
                None => {
                    let root = self.tree.root();
                    let node = self.tree.new_node(
                        NodeKind::Output {
                            name: o.name.clone(),
                            rect: o.rect,
                            scale: o.scale,
                            primary: o.primary,
                        },
                        Layout::SplitH,
                    );
                    self.tree.attach(root, node, None);
                    let ws_name = self.initial_workspace_name(&o.name, &names);
                    self.create_workspace(node, &ws_name);
                }
            }
        }
        // Keep the scratch output last in the focus stack so it can never
        // become the focused output.
        let root = self.tree.root();
        if let Some(so) = self
            .tree
            .get(root)
            .children
            .iter()
            .copied()
            .find(|&o| self.is_scratch_output(o))
        {
            let f = &mut self.tree.get_mut(root).focus;
            f.retain(|&c| c != so);
            f.push(so);
        }
        let gone: Vec<NodeId> = self
            .outputs()
            .into_iter()
            .filter(|&o| !names.iter().any(|n| n == self.output_name(o)))
            .collect();
        let dest = self
            .outputs()
            .into_iter()
            .find(|&o| names.iter().any(|n| n == self.output_name(o)));
        let focused_vanished = gone.contains(&focused_out);
        let former_visible = self.visible_workspace(focused_out);
        for o in gone {
            for ws in self.tree.get(o).children.clone() {
                match dest {
                    Some(d) if !self.is_workspace_empty(ws) => {
                        let (from, to) = (self.output_rect(o), self.output_rect(d));
                        self.tree.detach(ws);
                        self.attach_workspace_sorted(d, ws);
                        self.shift_workspace_floating(ws, from, to);
                    }
                    _ => {
                        let name = self.workspace_name(ws).to_string();
                        self.emit(IpcEvent::Workspace {
                            change: "empty".into(),
                            current: Some(name),
                            old: None,
                        });
                    }
                }
            }
            self.tree.remove(o);
        }
        if focused_vanished && self.tree.contains(former_visible) {
            self.show_workspace(former_visible);
        } else if focused_vanished {
            let first = self.outputs()[0];
            let f = self.tree.focused_descendant(self.visible_workspace(first));
            self.tree.set_focused(f);
        }
        self.emit(IpcEvent::Output);
    }
}

#[cfg(test)]
mod tests {
    use crate::config::Config;
    use crate::testutil::*;
    use crate::tree::{NodeKind, WindowId};
    use crate::wm::{IpcEvent, Wm};

    #[test]
    fn move_container_to_output() {
        let mut wm = Wm::new(
            Config::default(),
            vec![out("A", 0, 1000), out("B", 1000, 1000)],
        );
        wm.manage(win(1));
        wm.manage(win(2));
        assert!(wm.run_command("move container to output right")[0].success);
        let b_ws = wm.visible_workspace(wm.outputs()[1]);
        assert_eq!(wm.tree.describe(b_ws), "H[w2]");
        assert_eq!(wm.focused_output(), wm.outputs()[0]);
        assert!(wm.run_command("move container to output B")[0].success);
        assert!(!wm.run_command("move container to output nope")[0].success);
    }

    #[test]
    fn switch_creates_and_removes_empty() {
        let mut wm = test_wm();
        wm.run_command("workspace 3");
        assert_eq!(wm.workspace_name(wm.focused_workspace()), "3");
        assert!(wm.find_workspace("1").is_none());
    }
    #[test]
    fn switch_keeps_nonempty_and_hides_it() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("workspace 2");
        let ws1 = wm.find_workspace("1").unwrap();
        assert!(!wm.is_visible(ws1));
        assert_eq!(wm.focused_window(), None);
        wm.run_command("workspace back_and_forth");
        assert_eq!(wm.focused_window(), Some(WindowId(1)));
    }
    #[test]
    fn number_matches_named_workspace() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("workspace 3:www");
        wm.manage(win(2));
        wm.run_command("workspace 1");
        wm.run_command("workspace number 3");
        assert_eq!(wm.workspace_name(wm.focused_workspace()), "3:www");
    }
    #[test]
    fn next_prev_wrap_in_sorted_order() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("workspace 10");
        wm.manage(win(2));
        wm.run_command("workspace 2");
        wm.manage(win(3));
        wm.run_command("workspace next");
        assert_eq!(wm.workspace_name(wm.focused_workspace()), "10");
        wm.run_command("workspace next");
        assert_eq!(wm.workspace_name(wm.focused_workspace()), "1");
        wm.run_command("workspace prev");
        assert_eq!(wm.workspace_name(wm.focused_workspace()), "10");
    }
    #[test]
    fn move_container_to_workspace() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("move container to workspace 5");
        assert_eq!(ws_desc(&wm), "H[w1]");
        assert_eq!(wm.focused_window(), Some(WindowId(1)));
        assert_eq!(wm.tree.describe(wm.find_workspace("5").unwrap()), "H[w2]");
    }
    #[test]
    fn focus_output_by_direction() {
        let mut wm = Wm::new(
            Config::default(),
            vec![out("A", 0, 1000), out("B", 1000, 1000)],
        );
        wm.run_command("focus output right");
        assert_eq!(wm.workspace_name(wm.focused_workspace()), "2");
        assert!(!wm.run_command("focus output right")[0].success);
    }
    #[test]
    fn rename_workspace() {
        let mut wm = test_wm();
        wm.manage(win(1));
        assert!(wm.run_command("rename workspace to \"5: mail\"")[0].success);
        let ws = wm.focused_workspace();
        assert_eq!(wm.workspace_name(ws), "5: mail");
        assert!(matches!(
            wm.tree.get(ws).kind,
            NodeKind::Workspace { num: Some(5), .. }
        ));
        wm.run_command("workspace 2");
        assert!(wm.run_command("rename workspace \"5: mail\" to 7")[0].success);
        assert!(wm.find_workspace("7").is_some());
    }
    #[test]
    fn rename_to_existing_name_fails() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("workspace 2");
        wm.manage(win(2));
        let r = wm.run_command("rename workspace to 1");
        assert!(!r[0].success);
        assert_eq!(wm.workspace_name(wm.focused_workspace()), "2");
        assert!(!wm.run_command("rename workspace nope to x")[0].success);
        assert!(!wm.run_command("rename workspace to __x")[0].success);
    }
    #[test]
    fn rename_updates_prev_workspace_and_emits() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("workspace 2");
        wm.take_effects();
        wm.run_command("rename workspace 1 to 9");
        assert!(wm
            .take_effects()
            .contains(&crate::wm::Effect::Ipc(IpcEvent::Workspace {
                change: "rename".into(),
                current: Some("9".into()),
                old: None
            })));
        wm.run_command("workspace back_and_forth");
        assert_eq!(wm.workspace_name(wm.focused_workspace()), "9");
    }
    #[test]
    fn rename_onto_assigned_output_relocates() {
        let mut wm = Wm::new(
            Config::default(),
            vec![out("A", 0, 1000), out("B", 1000, 1000)],
        );
        wm.config.workspace_outputs = vec![("7".into(), vec!["B".into()])];
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("[id=2] floating enable");
        let n2 = wm.tree.window_node(WindowId(2)).unwrap();
        let r = wm.run_command("rename workspace to 7");
        assert!(r[0].success, "{:?}", r[0]);
        let (a, b) = (wm.outputs()[0], wm.outputs()[1]);
        let ws = wm.find_workspace("7").unwrap();
        assert_eq!(wm.tree.output_of(ws), Some(b));
        assert_eq!(wm.workspace_name(wm.visible_workspace(b)), "7");
        assert_eq!(wm.focused_workspace(), ws);
        assert_eq!(wm.workspace_name(wm.visible_workspace(a)), "1");
        assert_eq!(wm.tree.workspace_of(n2), Some(ws));
        let r = wm.arrange();
        let p = r.windows.iter().find(|p| p.id == WindowId(2)).unwrap();
        assert!(p.rect.x >= 1000, "floating window moved to B: {:?}", p.rect);
        check_invariants(&wm);
    }
    #[test]
    fn auto_back_and_forth() {
        let cfg = Config {
            workspace_auto_back_and_forth: true,
            ..Config::default()
        };
        let mut wm = Wm::new(cfg, vec![out("O1", 0, 1000)]);
        wm.manage(win(1));
        wm.run_command("workspace 2");
        wm.manage(win(2));
        wm.run_command("workspace 2");
        assert_eq!(wm.workspace_name(wm.focused_workspace()), "1");
        wm.run_command("workspace --no-auto-back-and-forth 1");
        assert_eq!(wm.workspace_name(wm.focused_workspace()), "1");
    }
    #[test]
    fn move_workspace_to_output() {
        let mut wm = Wm::new(
            Config::default(),
            vec![out("A", 0, 1000), out("B", 1000, 1000)],
        );
        wm.manage(win(1));
        wm.run_command("move workspace to output right");
        let b = wm.outputs()[1];
        assert_eq!(wm.workspace_name(wm.visible_workspace(b)), "1");
        assert_eq!(wm.focused_window(), Some(WindowId(1)));
        let a = wm.outputs()[0];
        assert_eq!(wm.workspace_name(wm.visible_workspace(a)), "3");
    }
    #[test]
    fn sticky_floating_follows_workspace_switch() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("[id=2] floating enable, sticky enable");
        wm.run_command("workspace 3");
        let n2 = wm.tree.window_node(WindowId(2)).unwrap();
        assert_eq!(wm.workspace_name(wm.tree.workspace_of(n2).unwrap()), "3");
        assert!(wm.arrange().windows.iter().any(|p| p.id == WindowId(2)));
        wm.run_command("[id=2] sticky toggle");
        wm.run_command("workspace 1");
        assert_eq!(wm.workspace_name(wm.tree.workspace_of(n2).unwrap()), "3");
    }

    #[test]
    fn sticky_tiled_window_stays() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("sticky enable");
        wm.run_command("workspace 3");
        let n = wm.tree.window_node(WindowId(1)).unwrap();
        assert_eq!(wm.workspace_name(wm.tree.workspace_of(n).unwrap()), "1");
    }

    #[test]
    fn removed_output_moves_workspaces() {
        let mut wm = Wm::new(
            Config::default(),
            vec![out("A", 0, 1000), out("B", 1000, 1000)],
        );
        wm.run_command("focus output right");
        wm.manage(win(1));
        wm.set_outputs(vec![out("A", 0, 1000)]);
        assert_eq!(wm.outputs().len(), 1);
        assert!(wm.find_workspace("2").is_some());
        // I8: the vanished focused output's workspace (with the window) stays focused.
        assert_eq!(wm.workspace_name(wm.focused_workspace()), "2");
        assert_eq!(wm.focused_window(), Some(WindowId(1)));
        check_invariants(&wm);
    }
    #[test]
    fn added_output_gets_workspace() {
        let mut wm = test_wm();
        wm.set_outputs(vec![out("O1", 0, 1000), out("B", 1000, 1000)]);
        assert_eq!(wm.outputs().len(), 2);
        let b = wm.outputs()[1];
        assert_eq!(wm.workspace_name(wm.visible_workspace(b)), "2");
    }
    #[test]
    fn focus_matched_switches_workspace() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("workspace 2");
        wm.run_command("[class=\"c1\"] focus");
        assert_eq!(wm.focused_window(), Some(WindowId(1)));
        assert_eq!(wm.workspace_name(wm.focused_workspace()), "1");
    }
    fn two_out() -> Wm {
        Wm::new(
            Config::default(),
            vec![out("A", 0, 1000), out("B", 1000, 1000)],
        )
    }
    #[test]
    fn i5_floating_rect_follows_rehomed_workspace() {
        let mut wm = two_out();
        wm.run_command("focus output right");
        let mut i = win(1);
        i.wants_float = true;
        wm.manage(i);
        wm.run_command("move right 500 px");
        wm.set_outputs(vec![out("A", 0, 1000)]);
        let n = wm.tree.window_node(WindowId(1)).unwrap();
        let r = match &wm.tree.get(n).kind {
            crate::tree::NodeKind::Window(d) => d.floating_rect.unwrap(),
            _ => panic!(),
        };
        let a = crate::geom::Rect::new(0, 0, 1000, 500);
        assert!(r.x >= a.x && r.right() <= a.right(), "{r:?}");
        assert!(r.y >= a.y && r.bottom() <= a.bottom(), "{r:?}");
        check_invariants(&wm);
    }
    #[test]
    fn i5_floating_window_moved_to_other_output_workspace() {
        let mut wm = two_out();
        let mut i = win(1);
        i.wants_float = true;
        wm.manage(i);
        wm.run_command("workspace 7");
        wm.run_command("focus output right");
        wm.run_command("[class=\"c1\"] move container to workspace 2");
        let n = wm.tree.window_node(WindowId(1)).unwrap();
        let r = match &wm.tree.get(n).kind {
            crate::tree::NodeKind::Window(d) => d.floating_rect.unwrap(),
            _ => panic!(),
        };
        assert!(r.x >= 1000 && r.right() <= 2000, "{r:?}");
    }
    #[test]
    fn i7_workspace_created_on_configured_output() {
        let cfg = crate::config::parse_config("workspace 5 output B\n");
        let mut wm = Wm::new(cfg, vec![out("A", 0, 1000), out("B", 1000, 1000)]);
        wm.run_command("focus output right");
        wm.run_command("workspace 7");
        assert!(wm.find_workspace("5").is_none());
        wm.run_command("focus output left");
        wm.run_command("workspace 5");
        let ws5 = wm.find_workspace("5").unwrap();
        assert_eq!(wm.tree.output_of(ws5), Some(wm.outputs()[1]));
        assert_eq!(wm.focused_workspace(), ws5);
        check_invariants(&wm);
    }
    #[test]
    fn i8_all_outputs_renamed_keeps_windows_visible() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.set_outputs(vec![out("B", 0, 1000)]);
        assert_eq!(wm.focused_window(), Some(WindowId(1)));
        assert_eq!(wm.workspace_name(wm.focused_workspace()), "1");
        check_invariants(&wm);
    }
}
