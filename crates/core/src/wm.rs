use crate::command::{parse_commands, Command, WorkspaceTarget};
use crate::config::Config;
use crate::geom::Rect;
use crate::keys::KeyCombo;
use crate::tree::{Layout, NodeId, NodeKind, Tree, WindowData, WindowId, WindowInfo};

#[derive(Clone, Debug, PartialEq)]
pub struct OutputInfo {
    pub name: String,
    pub rect: Rect,
    pub scale: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum IpcEvent {
    /// change: "focus" | "init" | "empty" | "move"
    Workspace {
        change: String,
        current: Option<String>,
        old: Option<String>,
    },
    /// change: "new" | "close" | "focus" | "title" | "move" | "floating" | "fullscreen_mode"
    Window {
        change: String,
        window: WindowId,
    },
    Mode {
        name: String,
    },
    Binding {
        command: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Exec(String),
    Close(WindowId),
    Focus(WindowId),
    Exit,
    Reload,
    Ipc(IpcEvent),
}

#[derive(Clone, Debug, PartialEq)]
pub struct CommandResult {
    pub success: bool,
    pub error: Option<String>,
}

pub struct Wm {
    pub tree: Tree,
    pub config: Config,
    pub mode: String,
    pub(crate) prev_workspace: Option<String>,
    /// (container, leaf): `focus parent`/`focus child` selection. Only valid
    /// while the focus-path leaf is still `leaf`, so any other focus change
    /// implicitly invalidates it.
    pub(crate) focused_container: Option<(NodeId, NodeId)>,
    pub(crate) effects: Vec<Effect>,
}

fn workspace_num(name: &str) -> Option<i32> {
    let digits: String = name.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn is_window_command(cmd: &Command) -> bool {
    !matches!(
        cmd,
        Command::Exec(_)
            | Command::Reload
            | Command::Restart
            | Command::Exit
            | Command::Mode(_)
            | Command::Nop
    )
}

impl Wm {
    pub fn new(config: Config, outputs: Vec<OutputInfo>) -> Wm {
        let mut wm = Wm {
            tree: Tree::new(),
            config,
            mode: "default".into(),
            prev_workspace: None,
            focused_container: None,
            effects: Vec::new(),
        };
        let outputs = if outputs.is_empty() {
            vec![OutputInfo {
                name: "default".into(),
                rect: Rect::new(0, 0, 1920, 1080),
                scale: 1.0,
            }]
        } else {
            outputs
        };
        let root = wm.tree.root();
        let mut output_nodes = Vec::new();
        for o in outputs {
            let node = wm.tree.new_node(
                NodeKind::Output {
                    name: o.name,
                    rect: o.rect,
                    scale: o.scale,
                },
                Layout::SplitH,
            );
            wm.tree.attach(root, node, None);
            output_nodes.push(node);
        }
        let output_names: Vec<String> = output_nodes
            .iter()
            .filter_map(|&n| match &wm.tree.get(n).kind {
                NodeKind::Output { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        for node in output_nodes {
            let oname = match &wm.tree.get(node).kind {
                NodeKind::Output { name, .. } => name.clone(),
                _ => String::new(),
            };
            let name = wm.initial_workspace_name(&oname, &output_names);
            wm.create_workspace(node, &name);
        }
        wm.effects.clear();
        wm
    }

    /// Name for the first workspace of a new output, skipping names
    /// reserved (via `workspace_outputs`) for other existing outputs.
    pub(crate) fn initial_workspace_name(&self, oname: &str, output_names: &[String]) -> String {
        let assigned = self
            .config
            .workspace_outputs
            .iter()
            .find(|(ws, outs)| outs.iter().any(|o| o == oname) && self.find_workspace(ws).is_none())
            .map(|(ws, _)| ws.clone());
        assigned.unwrap_or_else(|| {
            let mut n = 1;
            while self.find_workspace(&n.to_string()).is_some()
                || self.config.workspace_outputs.iter().any(|(ws, outs)| {
                    *ws == n.to_string()
                        && outs.iter().any(|o| o != oname && output_names.contains(o))
                })
            {
                n += 1;
            }
            n.to_string()
        })
    }

    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    pub fn focused(&self) -> NodeId {
        let leaf = self.tree.focused_descendant(self.tree.root());
        match self.focused_container {
            Some((c, l))
                if l == leaf && self.tree.contains(c) && self.tree.is_ancestor(c, leaf) =>
            {
                c
            }
            _ => leaf,
        }
    }

    pub fn focused_window(&self) -> Option<WindowId> {
        self.tree.window_id(self.focused())
    }

    pub fn focused_workspace(&self) -> NodeId {
        // Invariant: every output always has a visible workspace.
        self.tree
            .workspace_of(self.focused())
            .expect("focus path always reaches a workspace")
    }

    pub fn focused_output(&self) -> NodeId {
        // Invariant: Wm::new always creates at least one output.
        self.tree.get(self.tree.root()).focus[0]
    }

    pub fn visible_workspace(&self, output: NodeId) -> NodeId {
        self.tree.get(output).focus[0]
    }

    pub fn is_visible(&self, ws: NodeId) -> bool {
        match self.tree.output_of(ws) {
            Some(o) => self.tree.get(o).focus.first() == Some(&ws),
            None => false,
        }
    }

    pub fn workspace_name(&self, ws: NodeId) -> &str {
        match &self.tree.get(ws).kind {
            NodeKind::Workspace { name, .. } => name,
            _ => "",
        }
    }

    pub fn find_workspace(&self, name: &str) -> Option<NodeId> {
        self.outputs().into_iter().find_map(|o| {
            self.tree
                .get(o)
                .children
                .iter()
                .copied()
                .find(|&w| self.workspace_name(w) == name)
        })
    }

    pub fn outputs(&self) -> Vec<NodeId> {
        self.tree.get(self.tree.root()).children.clone()
    }

    pub(crate) fn emit(&mut self, e: IpcEvent) {
        self.effects.push(Effect::Ipc(e));
    }

    fn emit_window(&mut self, change: &str, window: WindowId) {
        self.emit(IpcEvent::Window {
            change: change.into(),
            window,
        });
    }

    pub(crate) fn create_workspace(&mut self, output: NodeId, name: &str) -> NodeId {
        let num = workspace_num(name);
        let ws = self.tree.new_node(
            NodeKind::Workspace {
                name: name.into(),
                num,
            },
            self.config.workspace_layout,
        );
        self.attach_workspace_sorted(output, ws);
        self.emit(IpcEvent::Workspace {
            change: "init".into(),
            current: Some(name.into()),
            old: None,
        });
        ws
    }

    pub(crate) fn insert_tiled(&mut self, ws: NodeId, node: NodeId) {
        let f = self.tree.focused_tiling_descendant(ws);
        if f == ws {
            self.tree.attach(ws, node, None);
        } else if matches!(self.tree.get(f).kind, NodeKind::Window(_)) {
            match (self.tree.parent(f), self.tree.index_in_parent(f)) {
                (Some(p), Some(i)) => self.tree.attach(p, node, Some(i + 1)),
                _ => self.tree.attach(ws, node, None),
            }
        } else {
            self.tree.attach(f, node, None);
        }
    }

    fn resolve_assign(&mut self, target: Option<&WorkspaceTarget>) -> NodeId {
        let name = match target {
            Some(WorkspaceTarget::Number(n)) => {
                let existing = self.outputs().into_iter().find_map(|o| {
                    self.tree.get(o).children.iter().copied().find(|&w| {
                        matches!(&self.tree.get(w).kind,
                            NodeKind::Workspace { num: Some(k), .. } if k == n)
                    })
                });
                if let Some(ws) = existing {
                    return ws;
                }
                n.to_string()
            }
            Some(WorkspaceTarget::Name(s)) => s.clone(),
            _ => return self.focused_workspace(),
        };
        match self.find_workspace(&name) {
            Some(ws) => ws,
            None => {
                let out = self.focused_output();
                self.create_workspace(out, &name)
            }
        }
    }

    pub fn manage(&mut self, info: WindowInfo) {
        if self.tree.window_node(info.id).is_some() {
            return;
        }
        let assigned = self
            .config
            .assigns
            .iter()
            .find(|(c, _)| c.matches(&info.class, &info.instance, &info.title))
            .map(|(_, t)| t.clone());
        let ws = self.resolve_assign(assigned.as_ref());
        self.focused_container = None;
        let id = info.id;
        let wants_float = info.wants_float;
        let rect = info.rect;
        let (class, instance, title) = (
            info.class.clone(),
            info.instance.clone(),
            info.title.clone(),
        );
        let node = self.tree.new_node(
            NodeKind::Window(WindowData {
                info,
                floating_rect: None,
                urgent: false,
            }),
            Layout::SplitH,
        );
        if wants_float {
            let out_rect = self
                .tree
                .output_of(ws)
                .and_then(|o| match &self.tree.get(o).kind {
                    NodeKind::Output { rect, .. } => Some(*rect),
                    _ => None,
                })
                .unwrap_or(rect);
            if let NodeKind::Window(d) = &mut self.tree.get_mut(node).kind {
                d.floating_rect = Some(rect.centered_in(&out_rect));
            }
            self.tree.attach_floating(ws, node);
        } else {
            self.insert_tiled(ws, node);
        }
        if ws == self.focused_workspace() {
            self.tree.set_focused(node);
            self.effects.push(Effect::Focus(id));
        } else {
            self.tree.set_focused_within(node, ws);
        }
        let cmds: Vec<String> = self
            .config
            .for_window
            .iter()
            .filter(|(c, _)| c.matches(&class, &instance, &title))
            .map(|(_, cmd)| cmd.clone())
            .collect();
        for text in cmds {
            let Ok(groups) = parse_commands(&text) else {
                continue;
            };
            for g in groups {
                for c in &g.commands {
                    let _ = self.exec_command(c, node);
                }
            }
        }
        self.emit_window("new", id);
    }

    pub fn unmanage(&mut self, id: WindowId) {
        let Some(node) = self.tree.window_node(id) else {
            return;
        };
        let ws = self.tree.workspace_of(node);
        let was_focused = self.tree.focused_descendant(self.tree.root()) == node;
        let parent = self.tree.parent(node);
        self.tree.get_mut(node).fullscreen = false;
        self.tree.remove(node);
        if let Some(p) = parent {
            if matches!(self.tree.get(p).kind, NodeKind::Container) {
                self.tree.cleanup(p);
            }
        }
        if let Some(ws) = ws {
            let n = self.tree.get(ws);
            if n.children.is_empty() && n.floating.is_empty() && !self.is_visible(ws) {
                let name = self.workspace_name(ws).to_string();
                self.tree.remove(ws);
                self.emit(IpcEvent::Workspace {
                    change: "empty".into(),
                    current: Some(name),
                    old: None,
                });
            } else if was_focused {
                let f = self.tree.focused_descendant(ws);
                self.tree.set_focused(f);
                if let Some(w) = self.tree.window_id(f) {
                    self.effects.push(Effect::Focus(w));
                }
            }
        }
        self.emit_window("close", id);
    }

    pub fn on_focus_changed(&mut self, id: WindowId) {
        self.focused_container = None;
        let Some(node) = self.tree.window_node(id) else {
            return;
        };
        let Some(ws) = self.tree.workspace_of(node) else {
            return;
        };
        if self.is_visible(ws) {
            self.tree.set_focused(node);
            if let NodeKind::Window(d) = &mut self.tree.get_mut(node).kind {
                d.urgent = false;
            }
            self.emit_window("focus", id);
        } else {
            if let NodeKind::Window(d) = &mut self.tree.get_mut(node).kind {
                d.urgent = true;
            }
            if let Some(w) = self.focused_window() {
                self.effects.push(Effect::Focus(w));
            }
        }
    }

    pub fn on_title_changed(&mut self, id: WindowId, title: String) {
        let Some(node) = self.tree.window_node(id) else {
            return;
        };
        if let NodeKind::Window(d) = &mut self.tree.get_mut(node).kind {
            d.info.title = title;
        }
        self.emit_window("title", id);
    }

    pub fn on_floating_moved(&mut self, id: WindowId, rect: Rect) {
        let Some(node) = self.tree.window_node(id) else {
            return;
        };
        if !self.tree.is_floating(node) {
            return;
        }
        if let NodeKind::Window(d) = &mut self.tree.get_mut(node).kind {
            d.floating_rect = Some(rect);
        }
    }

    pub fn run_command(&mut self, input: &str) -> Vec<CommandResult> {
        let groups = match parse_commands(input) {
            Ok(g) => g,
            Err(e) => {
                return vec![CommandResult {
                    success: false,
                    error: Some(e),
                }]
            }
        };
        let before = self.focused_window();
        let mut results = Vec::new();
        for g in groups {
            let matched: Option<Vec<NodeId>> = g.criteria.as_ref().map(|c| {
                self.tree
                    .windows_under(self.tree.root())
                    .into_iter()
                    .filter(|&n| match &self.tree.get(n).kind {
                        NodeKind::Window(d) => {
                            c.matches(&d.info.class, &d.info.instance, &d.info.title)
                        }
                        _ => false,
                    })
                    .collect()
            });
            for cmd in &g.commands {
                let mut targets = match &matched {
                    Some(t) => t.clone(),
                    None => vec![self.focused()],
                };
                // Non-window commands run once, not once per matched window.
                if !is_window_command(cmd) {
                    targets.truncate(1);
                }
                let mut error = None;
                for t in targets {
                    if !self.tree.contains(t) {
                        continue;
                    }
                    if let Err(e) = self.exec_command(cmd, t) {
                        error.get_or_insert(e);
                    }
                }
                results.push(CommandResult {
                    success: error.is_none(),
                    error,
                });
            }
        }
        let after = self.focused_window();
        if after != before {
            if let Some(w) = after {
                self.effects.push(Effect::Focus(w));
            }
        }
        results
    }

    pub(crate) fn exec_command(&mut self, cmd: &Command, target: NodeId) -> Result<(), String> {
        match cmd {
            Command::Exec(s) => self.effects.push(Effect::Exec(s.clone())),
            Command::Kill => {
                for n in self.tree.windows_under(target) {
                    if let Some(w) = self.tree.window_id(n) {
                        self.effects.push(Effect::Close(w));
                    }
                }
            }
            Command::Mode(name) => {
                if name != "default" && !self.config.bindings.contains_key(name) {
                    return Err("unknown mode".into());
                }
                self.mode = name.clone();
                self.emit(IpcEvent::Mode { name: name.clone() });
            }
            Command::Reload | Command::Restart => self.effects.push(Effect::Reload),
            Command::Exit => self.effects.push(Effect::Exit),
            Command::Nop => {}
            Command::FocusMatched => {
                self.focused_container = None;
                if let Some(ws) = self.tree.workspace_of(target) {
                    if !self.is_visible(ws) {
                        self.show_workspace(ws);
                    }
                }
                self.tree.set_focused(target);
            }
            Command::FocusOutput(t) => return self.focus_output(t),
            Command::MoveToWorkspace(t) => return self.move_to_workspace(target, t),
            Command::MoveWorkspaceToOutput(t) => return self.move_workspace_to_output(t),
            Command::Workspace(t) => return self.switch_workspace(t),
            Command::Focus(d) => return self.focus_direction(target, *d),
            Command::FocusParent => self.focus_parent(target),
            Command::FocusChild => self.focus_child(target),
            Command::FocusModeToggle => self.focus_layer(target, None),
            Command::FocusFloating => self.focus_layer(target, Some(true)),
            Command::FocusTiling => self.focus_layer(target, Some(false)),
            Command::Move(d, px) => return self.move_direction(target, *d, *px),
            Command::Split(a) => return self.split(target, a),
            Command::Layout(a) => return self.set_layout(target, a),
            Command::Resize {
                grow,
                axis,
                px,
                ppt,
            } => return self.resize(target, *grow, *axis, *px, *ppt),
            Command::Fullscreen(t) => return self.set_fullscreen(target, t),
            Command::Floating(t) => return self.set_floating(target, t),
        }
        Ok(())
    }

    pub fn binding_pressed(&mut self, combo: KeyCombo) -> bool {
        let command = self
            .config
            .bindings
            .get(&self.mode)
            .and_then(|bs| bs.iter().find(|b| b.combo == combo))
            .map(|b| b.command.clone());
        match command {
            Some(command) => {
                self.emit(IpcEvent::Binding {
                    command: command.clone(),
                });
                self.run_command(&command);
                true
            }
            None => false,
        }
    }

    pub fn active_combos(&self) -> Vec<KeyCombo> {
        self.config
            .bindings
            .get(&self.mode)
            .map(|bs| bs.iter().map(|b| b.combo).collect())
            .unwrap_or_default()
    }

    pub fn reload_config(&mut self, config: Config) {
        self.config = config;
        if self.mode != "default" {
            self.mode = "default".into();
            self.emit(IpcEvent::Mode {
                name: "default".into(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::Config;
    use crate::geom::Rect;
    use crate::testutil::*;
    use crate::tree::{Layout, NodeKind, WindowId};
    use crate::wm::*;

    #[test]
    fn new_creates_numbered_workspaces_per_output() {
        let wm = Wm::new(
            Config::default(),
            vec![out("A", 0, 1000), out("B", 1000, 1000)],
        );
        let names: Vec<_> = wm
            .outputs()
            .iter()
            .map(|&o| wm.workspace_name(wm.visible_workspace(o)).to_string())
            .collect();
        assert_eq!(names, vec!["1", "2"]);
    }
    #[test]
    fn workspace_output_assignment_respected() {
        let cfg = crate::config::parse_config("workspace 5 output B\n");
        let wm = Wm::new(cfg, vec![out("A", 0, 1000), out("B", 1000, 1000)]);
        assert_eq!(
            wm.workspace_name(wm.visible_workspace(wm.outputs()[1])),
            "5"
        );
    }
    #[test]
    fn manage_inserts_after_focused_and_focuses() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.tree
            .set_focused(wm.tree.window_node(WindowId(1)).unwrap());
        wm.manage(win(3));
        assert_eq!(ws_desc(&wm), "H[w1 w3 w2]");
        assert_eq!(wm.focused_window(), Some(WindowId(3)));
        assert!(wm.take_effects().contains(&Effect::Focus(WindowId(3))));
    }
    #[test]
    fn manage_twice_is_ignored() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(1));
        assert_eq!(ws_desc(&wm), "H[w1]");
    }
    #[test]
    fn wants_float_goes_floating_centered() {
        let mut wm = test_wm();
        let mut i = win(1);
        i.wants_float = true;
        wm.manage(i);
        let n = wm.tree.window_node(WindowId(1)).unwrap();
        assert!(wm.tree.is_floating(n));
        match &wm.tree.get(n).kind {
            NodeKind::Window(d) => assert_eq!(d.floating_rect, Some(Rect::new(300, 100, 400, 300))),
            _ => panic!(),
        }
    }
    #[test]
    fn assign_sends_to_other_workspace_without_switching() {
        let cfg = crate::config::parse_config("assign [class=\"c1\"] 4\n");
        let mut wm = Wm::new(cfg, vec![out("O1", 0, 1000)]);
        wm.manage(win(1));
        assert_eq!(wm.workspace_name(wm.focused_workspace()), "1");
        let ws4 = wm.find_workspace("4").unwrap();
        assert_eq!(wm.tree.describe(ws4), "H[w1]");
    }
    #[test]
    fn for_window_runs_commands() {
        let cfg = crate::config::parse_config("for_window [class=\"c1\"] kill\n");
        let mut wm = Wm::new(cfg, vec![out("O1", 0, 1000)]);
        wm.manage(win(1));
        assert!(wm.take_effects().contains(&Effect::Close(WindowId(1))));
    }
    #[test]
    fn unmanage_refocuses_previous() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.take_effects();
        wm.unmanage(WindowId(2));
        assert_eq!(wm.focused_window(), Some(WindowId(1)));
        assert!(wm.take_effects().contains(&Effect::Focus(WindowId(1))));
    }
    #[test]
    fn unmanage_cleans_nested_empty_containers() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        let ws = wm.focused_workspace();
        wm.tree.get_mut(ws).layout = Layout::Tabbed;
        let n2 = wm.tree.window_node(WindowId(2)).unwrap();
        wm.tree.wrap(n2, Layout::SplitV);
        wm.unmanage(WindowId(2));
        assert_eq!(ws_desc(&wm), "T[w1]");
        assert_eq!(wm.focused_window(), Some(WindowId(1)));
    }
    #[test]
    fn focus_stealing_from_hidden_workspace_marks_urgent() {
        let cfg = crate::config::parse_config("assign [class=\"c2\"] 2\n");
        let mut wm = Wm::new(cfg, vec![out("O1", 0, 1000)]);
        wm.manage(win(1));
        wm.manage(win(2));
        wm.take_effects();
        wm.on_focus_changed(WindowId(2));
        let n2 = wm.tree.window_node(WindowId(2)).unwrap();
        match &wm.tree.get(n2).kind {
            NodeKind::Window(d) => assert!(d.urgent),
            _ => panic!(),
        }
        assert!(wm.take_effects().contains(&Effect::Focus(WindowId(1))));
    }
    #[test]
    fn bindings_and_modes() {
        let cfg = crate::config::parse_config(
            "bindsym Mod1+r mode \"resize\"\nmode \"resize\" {\nbindsym Escape mode \"default\"\n}\n",
        );
        let mut wm = Wm::new(cfg, vec![out("O1", 0, 1000)]);
        assert!(wm.binding_pressed(crate::keys::parse_combo("Mod1+r").unwrap()));
        assert_eq!(wm.mode, "resize");
        assert_eq!(
            wm.active_combos(),
            vec![crate::keys::parse_combo("Escape").unwrap()]
        );
        assert!(!wm.binding_pressed(crate::keys::parse_combo("Mod1+x").unwrap()));
    }
    #[test]
    fn run_command_reports_errors_and_effects() {
        let mut wm = test_wm();
        assert!(!wm.run_command("frob")[0].success);
        let r = wm.run_command("exec notepad; mode nosuch");
        assert!(r[0].success && !r[1].success);
        assert!(wm.take_effects().contains(&Effect::Exec("notepad".into())));
    }
    #[test]
    fn commands_on_empty_workspace_do_not_panic() {
        let mut wm = test_wm();
        for c in [
            "kill",
            "move left",
            "split v",
            "floating toggle",
            "fullscreen",
            "move container to workspace 2",
            "resize grow width",
            "layout tabbed",
            "focus parent",
            "focus child",
            "focus left",
            "focus mode_toggle",
        ] {
            let _ = wm.run_command(c);
        }
        assert_eq!(wm.workspace_name(wm.focused_workspace()), "1");
    }

    #[test]
    fn workspace_output_assignment_not_stolen() {
        let cfg = crate::config::parse_config(
            "workspace 1 output B
",
        );
        let wm = Wm::new(cfg, vec![out("A", 0, 1000), out("B", 1000, 1000)]);
        let names: Vec<_> = wm
            .outputs()
            .iter()
            .map(|&o| wm.workspace_name(wm.visible_workspace(o)).to_string())
            .collect();
        assert_eq!(names, vec!["2", "1"]);
    }
    #[test]
    fn non_window_commands_run_once_per_command() {
        let mut wm = test_wm();
        let mut a = win(1);
        a.class = "c".into();
        let mut b = win(2);
        b.class = "c".into();
        wm.manage(a);
        wm.manage(b);
        wm.take_effects();
        wm.run_command("[class=\"c\"] exec foo");
        let n = wm
            .take_effects()
            .iter()
            .filter(|e| **e == Effect::Exec("foo".into()))
            .count();
        assert_eq!(n, 1);
    }
    #[test]
    fn workspaces_sorted_on_insert() {
        let mut wm = test_wm();
        let o = wm.outputs()[0];
        wm.create_workspace(o, "foo");
        wm.create_workspace(o, "10");
        wm.create_workspace(o, "2");
        let names: Vec<_> = wm
            .tree
            .get(o)
            .children
            .iter()
            .map(|&w| wm.workspace_name(w).to_string())
            .collect();
        assert_eq!(names, vec!["1", "2", "10", "foo"]);
    }
    #[test]
    fn unmanage_last_window_on_hidden_workspace_removes_it() {
        let cfg = crate::config::parse_config(
            "assign [class=\"c2\"] 2
",
        );
        let mut wm = Wm::new(cfg, vec![out("O1", 0, 1000)]);
        wm.manage(win(2));
        wm.take_effects();
        wm.unmanage(WindowId(2));
        assert!(wm.find_workspace("2").is_none());
        assert!(wm
            .take_effects()
            .contains(&Effect::Ipc(IpcEvent::Workspace {
                change: "empty".into(),
                current: Some("2".into()),
                old: None
            })));
    }
}
