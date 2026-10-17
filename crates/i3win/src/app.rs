//! The application: glue between platform events and the core window manager.
use std::cell::RefCell;
use std::collections::HashSet;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use i3win_core::arrange::floating_frame_from_client;
use i3win_core::config::{default_config_text, parse_config, Config};
use i3win_core::geom::Rect;
use i3win_core::i3bar::Block;
use i3win_core::ipc::{self, IpcContext, Request, EV_TICK};
use i3win_core::keys::KeyCombo;
use i3win_core::tree::{NodeKind, WindowId};
use i3win_core::wm::{Effect, Wm};

use crate::ipc_server;
use crate::mainloop::{self, MainMsg};
use crate::paths;
use crate::reconcile::{self, Applied, WinOp};
use crate::status::{cmd_args, exit_block, StatusProcess};
use crate::win::bar::Bars;
use crate::win::deco::Decos;
use crate::win::events::{self, PlatformEvent};
use crate::win::hidden::{self, HiddenStore, HideTracker};
use crate::win::window::{self, Manage};
use crate::win::{keyboard, monitors};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

thread_local! {
    /// The single application instance, owned by the main (UI) thread.
    pub static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

#[derive(Debug, PartialEq, Eq)]
pub enum EventDecision {
    Ignore,
    TryManage,
    Unmanage,
    Focus,
    Title,
    MoveSizeEnd,
}

/// Pure event routing table.
pub(crate) fn decide(ev: PlatformEvent, managed: bool, expected_hide: bool) -> EventDecision {
    use EventDecision::*;
    use PlatformEvent as E;
    match (ev, managed) {
        (E::Shown(_), false) => TryManage,
        (E::Shown(_), true) => Ignore,
        (E::TitleChanged(_), true) => Title,
        (E::TitleChanged(_), false) => TryManage,
        (E::Hidden(_), true) if expected_hide => Ignore,
        (E::Hidden(_), true) => Unmanage,
        (E::Hidden(_), false) => Ignore,
        (E::Destroyed(_) | E::Minimized(_), true) => Unmanage,
        (E::Destroyed(_) | E::Minimized(_), false) => Ignore,
        (E::Restored(_), false) => TryManage,
        (E::Restored(_), true) => Ignore,
        (E::Foreground(_), true) => Focus,
        (E::Foreground(_), false) => TryManage,
        (E::MoveSizeEnd(_), true) => MoveSizeEnd,
        (E::MoveSizeEnd(_), false) => Ignore,
    }
}

/// Event routing including what we know about our own hides: a Hidden event for a
/// window in `applied.hidden` is never an unmanage, and a Foreground event for one
/// is noise caused by our own hiding.
pub(crate) fn route(
    ev: PlatformEvent,
    managed: bool,
    tracked_hide: bool,
    applied: &Applied,
) -> EventDecision {
    let hidden_by_us = applied.hidden.contains(&event_id(ev));
    match ev {
        PlatformEvent::Foreground(_) if hidden_by_us => EventDecision::Ignore,
        PlatformEvent::Hidden(_) => decide(ev, managed, tracked_hide || hidden_by_us),
        _ => decide(ev, managed, false),
    }
}

/// Only the last queued focus request matters, and only while it still names the
/// window at the end of the core's focus path.
pub(crate) fn focus_to_apply(requests: &[WindowId], leaf: Option<WindowId>) -> Option<WindowId> {
    requests.last().copied().filter(|w| Some(*w) == leaf)
}

fn event_id(ev: PlatformEvent) -> WindowId {
    use PlatformEvent::*;
    match ev {
        Shown(i) | Hidden(i) | Destroyed(i) | Foreground(i) | Minimized(i) | Restored(i)
        | MoveSizeEnd(i) | TitleChanged(i) => i,
    }
}

fn parse_file(path: &Path) -> Result<(Config, String), String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    Ok((parse_config(&text), text))
}

fn default_config() -> Config {
    let path_var = std::env::var("PATH").unwrap_or_default();
    parse_config(&default_config_text(paths::find_on_path(
        "wt.exe", &path_var,
    )))
}

fn spawn(command: &str, pipe_name: &str) {
    let mut cmd = Command::new("cmd.exe");
    cmd.raw_arg(cmd_args(&format!("start \"\" {command}")))
        .creation_flags(CREATE_NO_WINDOW)
        .env("I3SOCK", pipe_name);
    if let Some(home) = std::env::var_os("USERPROFILE") {
        cmd.current_dir(home);
    }
    if let Err(e) = cmd.spawn() {
        log::error!("exec {command:?}: {e}");
    }
}

pub struct App {
    pub wm: Wm,
    applied: Applied,
    tracker: HideTracker,
    decos: Decos,
    bars: Bars,
    /// Latest status blocks (empty until the status process feeds them).
    blocks: Vec<Block>,
    /// Blocks changed since the bars were last redrawn.
    bars_dirty: bool,
    status: Option<StatusProcess>,
    store: HiddenStore,
    config_path: PathBuf,
    last_combos: Vec<KeyCombo>,
    /// Effects queued outside the core (exec lines from the config), flushed first.
    pending: Vec<Effect>,
    /// Last successfully read config text, served by GET_CONFIG.
    config_text: String,
    /// IPC pipe name, exported to exec'd children as I3SOCK.
    pipe_name: String,
}

impl App {
    pub fn start(config_path: PathBuf, pipe_name: String) -> App {
        let (config, config_text) = match paths::load_or_create_config(&config_path) {
            Ok(text) => (parse_config(&text), text),
            Err(e) => {
                log::error!("config: {e}; using the default config");
                (default_config(), String::new())
            }
        };
        for w in &config.warnings {
            log::warn!("config: {w}");
        }
        let pending: Vec<Effect> = config
            .exec
            .iter()
            .chain(config.exec_always.iter())
            .cloned()
            .map(Effect::Exec)
            .collect();
        let mut wm = Wm::new(config, monitors::enumerate());
        wm.machine = std::env::var("COMPUTERNAME").unwrap_or_default();
        // Topmost window last so it ends up with focus.
        for id in window::enumerate_top_level().into_iter().rev() {
            Self::try_manage_in(&mut wm, id);
        }
        if let Some(fg) = window::foreground() {
            wm.on_focus_changed(fg);
        }
        let mut app = App {
            wm,
            applied: Applied::default(),
            tracker: HideTracker::new(),
            decos: Decos::new(),
            bars: Bars::new(),
            blocks: Vec::new(),
            bars_dirty: false,
            store: HiddenStore::open(paths::hidden_path()),
            config_path,
            last_combos: Vec::new(),
            pending,
            config_text,
            pipe_name,
            status: None,
        };
        app.restart_status();
        app
    }

    /// Kills the running status process (if any) and starts the configured one.
    fn restart_status(&mut self) {
        if let Some(mut old) = self.status.take() {
            old.kill();
        }
        let Some(cmd) = self
            .wm
            .config
            .bar
            .as_ref()
            .and_then(|b| b.status_command.clone())
        else {
            self.blocks.clear();
            return;
        };
        match StatusProcess::spawn(&cmd) {
            Ok(p) => self.status = Some(p),
            Err(e) => {
                log::error!("status_command {cmd:?}: {e}");
                self.blocks = vec![exit_block()];
            }
        }
    }

    fn status_gen(&self) -> Option<u64> {
        self.status.as_ref().map(|s| s.generation())
    }

    /// Redraws only the bars with the current blocks.
    fn refresh_bars(&mut self) {
        self.bars_dirty = false;
        let arr = self.wm.arrange();
        self.bars.update(&self.wm, &arr, &self.blocks);
    }

    fn try_manage_in(wm: &mut Wm, id: WindowId) {
        let Some(p) = window::props(id) else {
            return;
        };
        let wants_float = match window::classify(&p) {
            Manage::No => return,
            Manage::Tile => false,
            Manage::Float => true,
        };
        if let Some(info) = window::info(id, wants_float) {
            wm.manage(info);
        }
    }

    fn is_managed(&self, id: WindowId) -> bool {
        self.wm.tree.window_node(id).is_some()
    }

    /// Handles one platform event. Does not flush; the caller flushes once per batch.
    pub fn on_event(&mut self, ev: PlatformEvent) {
        let id = event_id(ev);
        let tracked_hide =
            matches!(ev, PlatformEvent::Hidden(_)) && self.tracker.consume(id, Instant::now());
        match route(ev, self.is_managed(id), tracked_hide, &self.applied) {
            EventDecision::Ignore => {}
            EventDecision::TryManage => Self::try_manage_in(&mut self.wm, id),
            EventDecision::Unmanage => {
                // A window we hid must never be stranded: show it before forgetting it.
                if !matches!(ev, PlatformEvent::Destroyed(_))
                    && self.store.contains(id)
                    && window::is_window(id)
                {
                    window::show(id);
                }
                self.wm.unmanage(id);
                self.tracker.forget(id);
                self.store.remove(id);
                self.store.persist_if_dirty();
            }
            EventDecision::Focus => self.wm.on_focus_changed(id),
            EventDecision::Title => {
                if let Some(p) = window::props(id) {
                    self.wm.on_title_changed(id, p.title);
                }
            }
            EventDecision::MoveSizeEnd => self.on_move_size_end(id),
        }
    }

    fn on_move_size_end(&mut self, id: WindowId) {
        let Some(node) = self.wm.tree.window_node(id) else {
            return;
        };
        if self.wm.tree.is_floating(node) {
            let scale = self
                .wm
                .tree
                .output_of(node)
                .and_then(|o| match &self.wm.tree.get(o).kind {
                    NodeKind::Output { scale, .. } => Some(*scale),
                    _ => None,
                })
                .unwrap_or(1.0);
            if let Some(r) = window::frame_rect(id) {
                let border = self
                    .wm
                    .tree
                    .get(node)
                    .border
                    .unwrap_or(self.wm.config.default_floating_border);
                let frame = floating_frame_from_client(&self.wm.config, border, scale, r);
                self.wm.on_floating_moved(id, frame);
            }
        } else {
            // Tiled windows snap back on the next flush.
            reconcile::invalidate(&mut self.applied, id);
        }
    }

    /// Handles a bound key combo. Does not flush.
    pub fn on_binding(&mut self, combo: KeyCombo) {
        self.wm.binding_pressed(combo);
    }

    /// Handles a cross-thread message. Does not flush; returns whether the caller
    /// must (status messages only refresh the bars themselves).
    pub fn on_main_msg(&mut self, msg: MainMsg) -> bool {
        match msg {
            MainMsg::StatusUpdate(g, blocks) => {
                if self.status_gen() == Some(g) {
                    self.blocks = blocks;
                    self.bars_dirty = true;
                }
                return false;
            }
            MainMsg::StatusExited(g) => {
                if self.status_gen() == Some(g) {
                    if let Some(mut p) = self.status.take() {
                        p.kill();
                    }
                    self.blocks = vec![exit_block()];
                    self.bars_dirty = true;
                }
                return false;
            }
            // Shutdown is handled by the window procedure itself.
            MainMsg::Shutdown => {}
            #[cfg(debug_assertions)]
            MainMsg::DebugPanic => panic!("debug panic requested via I3WIN_PANIC_AFTER_SECS"),
            MainMsg::FocusNode(n) => {
                if self.wm.tree.contains(n) {
                    self.wm.focus_node(n);
                }
            }
            MainMsg::Ipc {
                msg_type,
                payload,
                reply,
            } => return self.on_ipc(msg_type, &payload, &reply),
            MainMsg::RunCommand(c) => {
                self.wm.run_command(&c);
            }
            MainMsg::StatusClick {
                name,
                instance,
                button,
                x,
                y,
            } => {
                if let Some(p) = self.status.as_mut() {
                    p.click(name.as_deref(), instance.as_deref(), button, x, y);
                }
                return false;
            }
        }
        true
    }

    /// Returns true when a full flush is needed.
    fn on_ipc(
        &mut self,
        msg_type: u32,
        payload: &str,
        reply: &std::sync::mpsc::Sender<String>,
    ) -> bool {
        let path = self.config_path.display().to_string();
        let ctx = IpcContext {
            config_path: &path,
            config_text: &self.config_text,
        };
        match ipc::handle(&mut self.wm, &ctx, msg_type, payload) {
            Request::Reply(s) => {
                let _ = reply.send(s);
            }
            Request::Tick(p) => {
                let _ = reply.send(r#"{"success":true}"#.to_string());
                ipc_server::broadcast(EV_TICK, "tick", &ipc::tick_event(false, &p));
            }
            // The connection thread handles SUBSCRIBE itself.
            Request::Subscribe(_) => {
                let _ = reply.send(r#"{"success":false,"error":"unexpected subscribe"}"#.into());
            }
        }
        ipc_needs_flush(msg_type, self.wm.has_effects() || !self.pending.is_empty())
    }

    /// Ends a batch of main messages: one full flush if requested (it redraws the
    /// bars too), otherwise at most one bar refresh for coalesced status updates.
    pub fn finish_main_msgs(&mut self, need_flush: bool) {
        if need_flush {
            self.flush();
        } else if self.bars_dirty {
            self.refresh_bars();
        }
    }

    /// Re-reads monitor geometry and re-places every window.
    pub fn refresh_outputs(&mut self) {
        self.wm.set_outputs(monitors::enumerate());
        self.applied.rects.clear();
        self.flush();
    }

    /// Applies effects, then the arrangement, then focus.
    pub fn flush(&mut self) {
        let mut focus = Vec::new();
        loop {
            let mut effects = std::mem::take(&mut self.pending);
            effects.extend(self.wm.take_effects());
            if effects.is_empty() {
                break;
            }
            for e in effects {
                match e {
                    Effect::Exec(c) => spawn(&c, &self.pipe_name),
                    Effect::Close(w) => window::close(w),
                    Effect::Focus(w) => focus.push(w),
                    Effect::Exit => mainloop::post(MainMsg::Shutdown),
                    Effect::Reload => self.reload(),
                    Effect::Ipc(ev) => {
                        let (i, n, p) = ipc::event_json(&self.wm, &ev);
                        ipc_server::broadcast(i, n, &p);
                    }
                }
            }
        }
        let arr = self.wm.arrange();
        let managed: HashSet<WindowId> = arr
            .windows
            .iter()
            .map(|p| p.id)
            .chain(arr.hidden.iter().copied())
            .collect();
        let ops = reconcile::plan(&mut self.applied, &arr, &managed);
        // Write-ahead: persist every pending hide before issuing any of them.
        for op in &ops {
            if let WinOp::Hide(id) = op {
                self.store.add(*id);
            }
        }
        self.store.persist_if_dirty();
        for op in ops {
            match op {
                WinOp::Hide(id) => {
                    self.tracker.expect(id, Instant::now());
                    if !window::hide(id) {
                        // Not hidden after all: do not claim it, retry on a later flush.
                        self.applied.hidden.remove(&id);
                        self.store.remove(id);
                    }
                }
                WinOp::Show(id) => {
                    window::show(id);
                    self.store.remove(id);
                }
                WinOp::Place { id, rect, raise } => window::place(id, rect, raise),
            }
        }
        self.store.persist_if_dirty();
        let outputs: Vec<(Rect, f32)> = self
            .wm
            .outputs()
            .into_iter()
            .filter_map(|o| match &self.wm.tree.get(o).kind {
                NodeKind::Output { rect, scale, .. } => Some((*rect, *scale)),
                _ => None,
            })
            .collect();
        self.decos.reconcile(&arr, &self.wm.config, |r| {
            let (cx, cy) = r.center();
            outputs
                .iter()
                .find(|(o, _)| cx >= o.x && cx < o.right() && cy >= o.y && cy < o.bottom())
                .map_or(1.0, |(_, s)| *s)
        });
        self.bars.update(&self.wm, &arr, &self.blocks);
        self.bars_dirty = false;
        // Focus last: focusing a still-hidden window fails.
        let leaf = self
            .wm
            .tree
            .window_id(self.wm.tree.focused_descendant(self.wm.tree.root()));
        if let Some(w) = focus_to_apply(&focus, leaf) {
            window::focus(w);
        } else if leaf.is_none() {
            self.park_foreground();
        }
        let combos = self.wm.active_combos();
        if combos != self.last_combos {
            keyboard::set_combos(&combos);
            self.last_combos = combos;
        }
    }

    /// With nothing to focus, keep typing out of windows we have hidden.
    fn park_foreground(&self) {
        let main = mainloop::main_hwnd();
        if main == 0 {
            return;
        }
        if window::foreground().is_some_and(|fg| self.applied.hidden.contains(&fg)) {
            window::focus(WindowId(main));
        }
    }

    fn reload(&mut self) {
        match parse_file(&self.config_path) {
            Ok((config, text)) => {
                self.config_text = text;
                for w in &config.warnings {
                    log::warn!("config: {w}");
                }
                let always = config.exec_always.clone();
                self.wm.reload_config(config);
                self.pending.extend(always.into_iter().map(Effect::Exec));
                // New geometry settings: re-place everything.
                self.applied.rects.clear();
                self.restart_status();
            }
            Err(e) => log::error!("reload failed: {e}"),
        }
    }

    pub fn shutdown(&mut self) {
        ipc_server::broadcast_shutdown();
        events::uninstall();
        keyboard::uninstall();
        hidden::restore_all_hidden();
        if let Some(mut p) = self.status.take() {
            p.kill();
        }
        self.bars.destroy_all();
        self.decos.destroy_all();
        self.store.clear_file();
    }
}

/// Only commands (or leftover effects) need a full flush; queries must not
/// re-stack windows.
fn ipc_needs_flush(msg_type: u32, has_effects: bool) -> bool {
    msg_type == i3win_core::ipc::RUN_COMMAND || has_effects
}

#[cfg(test)]
mod tests {
    #[test]
    fn ipc_flush_decision() {
        use super::ipc_needs_flush;
        assert!(ipc_needs_flush(i3win_core::ipc::RUN_COMMAND, false));
        assert!(ipc_needs_flush(i3win_core::ipc::GET_TREE, true));
        assert!(!ipc_needs_flush(i3win_core::ipc::GET_TREE, false));
        assert!(!ipc_needs_flush(i3win_core::ipc::SEND_TICK, false));
    }

    use super::*;
    #[test]
    fn hidden_expected_is_ignored() {
        assert_eq!(
            decide(PlatformEvent::Hidden(WindowId(1)), true, true),
            EventDecision::Ignore
        );
    }
    #[test]
    fn hidden_for_window_we_hid_is_ignored_without_tracker_entry() {
        let mut applied = Applied::default();
        applied.hidden.insert(WindowId(1));
        assert_eq!(
            route(PlatformEvent::Hidden(WindowId(1)), true, false, &applied),
            EventDecision::Ignore
        );
        // An id we did not hide still unmanages.
        assert_eq!(
            route(PlatformEvent::Hidden(WindowId(2)), true, false, &applied),
            EventDecision::Unmanage
        );
    }
    #[test]
    fn foreground_of_hidden_window_is_ignored() {
        let mut applied = Applied::default();
        applied.hidden.insert(WindowId(1));
        assert_eq!(
            route(
                PlatformEvent::Foreground(WindowId(1)),
                true,
                false,
                &applied
            ),
            EventDecision::Ignore
        );
        assert_eq!(
            route(
                PlatformEvent::Foreground(WindowId(2)),
                true,
                false,
                &applied
            ),
            EventDecision::Focus
        );
    }
    #[test]
    fn only_last_matching_focus_is_applied() {
        let (a, b) = (WindowId(1), WindowId(2));
        assert_eq!(focus_to_apply(&[a, b], Some(b)), Some(b));
        assert_eq!(focus_to_apply(&[a, b], Some(a)), None);
        assert_eq!(focus_to_apply(&[a], None), None);
        assert_eq!(focus_to_apply(&[], Some(a)), None);
    }
    #[test]
    fn hidden_unexpected_unmanages() {
        assert_eq!(
            decide(PlatformEvent::Hidden(WindowId(1)), true, false),
            EventDecision::Unmanage
        );
    }
    #[test]
    fn title_change_on_unmanaged_retries_manage() {
        assert_eq!(
            decide(PlatformEvent::TitleChanged(WindowId(1)), false, false),
            EventDecision::TryManage
        );
    }
    #[test]
    fn minimize_unmanages_restore_manages() {
        assert_eq!(
            decide(PlatformEvent::Minimized(WindowId(1)), true, false),
            EventDecision::Unmanage
        );
        assert_eq!(
            decide(PlatformEvent::Restored(WindowId(1)), false, false),
            EventDecision::TryManage
        );
    }
    #[test]
    fn movesize_only_for_managed() {
        assert_eq!(
            decide(PlatformEvent::MoveSizeEnd(WindowId(1)), false, false),
            EventDecision::Ignore
        );
    }
}
