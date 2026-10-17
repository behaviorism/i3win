//! The application: glue between platform events and the core window manager.
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use i3win_core::arrange::{floating_frame_from_client, Arrangement};
use i3win_core::config::{default_config_text, parse_config, BarMode, Config};
use i3win_core::geom::Rect;
use i3win_core::i3bar::Block;
use i3win_core::ipc::{self, IpcContext, Request, EV_TICK};
use i3win_core::keys::{KeyCombo, Modifiers};
use i3win_core::tree::{NodeId, NodeKind, WindowId};
use i3win_core::wm::{Effect, Wm};
use i3win_core::DropTarget;

use crate::drag::{self, Drag, DragKind, NativeDrag, PercentSnapshot, ReleaseWatch};
use crate::ipc_server;
use crate::mainloop::{self, MainMsg};
use crate::paths;
use crate::reconcile::{self, Applied, WinOp};
use crate::status::{cmd_args, exit_block, StatusProcess};
use crate::win::bar::Bars;
use crate::win::deco::{self, Decos};
use crate::win::events::{self, PlatformEvent};
use crate::win::hidden::{self, HiddenStore, HideTracker};
use crate::win::mouse::{self, Button, MouseEvent};
use crate::win::overlay::Overlay;
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
    MoveSizeStart,
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
        (E::MoveSizeStart(_), true) => MoveSizeStart,
        (E::MoveSizeStart(_), false) => Ignore,
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
        | MoveSizeStart(i) | MoveSizeEnd(i) | TitleChanged(i) => i,
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

/// The status process of one bar and the blocks it last produced.
#[derive(Default)]
struct BarStatus {
    process: Option<StatusProcess>,
    blocks: Vec<Block>,
}

/// The entry for `bar` (the owner of a click), if any.
#[cfg_attr(not(test), allow(dead_code))] // exercised by tests
pub(crate) fn route_click<'a, T>(statuses: &'a HashMap<String, T>, bar: &str) -> Option<&'a T> {
    statuses.get(bar)
}

/// What the mouse hook tests presses against: floating placements first, most
/// recently focused first, then the rest in arrangement order.
fn mouse_snapshot(wm: &Wm, arr: &Arrangement, foreground_managed: bool) -> mouse::Snapshot {
    let focus_rank = |id: WindowId| {
        let node = wm.tree.window_node(id)?;
        let ws = wm.tree.parent(node)?;
        wm.tree.get(ws).focus.iter().position(|&n| n == node)
    };
    let mut floating: Vec<(usize, mouse::Hit)> = Vec::new();
    let mut tiled = Vec::new();
    for p in &arr.windows {
        let hit = mouse::Hit {
            id: p.id,
            rect: p.rect,
            floating: p.floating,
        };
        if p.floating {
            floating.push((focus_rank(p.id).unwrap_or(usize::MAX), hit));
        } else {
            tiled.push(hit);
        }
    }
    floating.sort_by_key(|(rank, _)| *rank);
    mouse::Snapshot {
        hits: floating.into_iter().map(|(_, h)| h).chain(tiled).collect(),
        modifier: wm.config.floating_modifier,
        want_moves: wm.config.focus_follows_mouse,
        foreground_managed,
    }
}

/// A mouse drag in progress.
struct DragSession {
    drag: Drag,
    button: Button,
    /// Started from a title bar or tab (the deco window holds the capture).
    from_titlebar: bool,
    /// Percents to restore on cancel (tiled resize only).
    percents: PercentSnapshot,
    /// The drop target last previewed (tiling drag only).
    target: Option<DropTarget>,
    /// Missed button-up detection (arms once the button is seen down).
    watch: ReleaseWatch,
    /// A throttled tiled-resize position the drag timer still has to apply.
    pending: Option<(i32, i32)>,
}

/// Whether `button` is up right now (logical button, honoring swapped buttons).
fn button_up(button: Button) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_SWAPBUTTON};
    // SAFETY: plain Win32 queries.
    unsafe {
        let swapped = GetSystemMetrics(SM_SWAPBUTTON) != 0;
        let vk = match (button, swapped) {
            (Button::Left, false) | (Button::Right, true) => VK_LBUTTON,
            (Button::Right, false) | (Button::Left, true) => VK_RBUTTON,
            (Button::Middle, _) => VK_MBUTTON,
        };
        GetAsyncKeyState(i32::from(vk.0)) >= 0
    }
}

pub struct App {
    pub wm: Wm,
    applied: Applied,
    tracker: HideTracker,
    decos: Decos,
    bars: Bars,
    /// Blocks changed since the bars were last redrawn.
    bars_dirty: bool,
    /// Whether the modifier watched for Hide-mode bars is currently held.
    modifier_held: bool,
    /// One status process and its latest blocks per configured bar id.
    statuses: HashMap<String, BarStatus>,
    store: HiddenStore,
    config_path: PathBuf,
    last_combos: Vec<(KeyCombo, bool)>,
    last_watch: Option<Modifiers>,
    /// Effects queued outside the core (exec lines from the config), flushed first.
    pending: Vec<Effect>,
    /// Last successfully read config text, served by GET_CONFIG.
    config_text: String,
    /// IPC pipe name, exported to exec'd children as I3SOCK.
    pipe_name: String,
    drag: Option<DragSession>,
    /// DWM frame of a tiled window at the start of its native move/size loop.
    move_start: HashMap<WindowId, Rect>,
    overlay: Overlay,
    /// Time base for drag throttling.
    epoch: Instant,
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
            bars_dirty: false,
            modifier_held: false,
            store: HiddenStore::open(paths::hidden_path()),
            config_path,
            last_combos: Vec::new(),
            last_watch: None,
            pending,
            config_text,
            pipe_name,
            statuses: HashMap::new(),
            drag: None,
            move_start: HashMap::new(),
            overlay: Overlay::default(),
            epoch: Instant::now(),
        };
        app.restart_status();
        app
    }

    /// Kills every running status process and starts the configured ones.
    fn restart_status(&mut self) {
        for (_, mut old) in self.statuses.drain() {
            if let Some(p) = old.process.as_mut() {
                p.kill();
            }
        }
        for bar in &self.wm.config.bars {
            let mut st = BarStatus::default();
            if let Some(cmd) = bar.status_command.as_deref() {
                match StatusProcess::spawn(cmd, bar.id.clone()) {
                    Ok(p) => st.process = Some(p),
                    Err(e) => {
                        log::error!("status_command {cmd:?}: {e}");
                        st.blocks = vec![exit_block()];
                    }
                }
            }
            self.statuses.insert(bar.id.clone(), st);
        }
    }

    /// The status entry for `bar` when `generation` is its live process.
    fn live_status(&mut self, bar: &str, generation: u64) -> Option<&mut BarStatus> {
        self.statuses
            .get_mut(bar)
            .filter(|s| s.process.as_ref().map(|p| p.generation()) == Some(generation))
    }

    fn all_blocks(&self) -> HashMap<String, Vec<Block>> {
        self.statuses
            .iter()
            .map(|(k, s)| (k.clone(), s.blocks.clone()))
            .collect()
    }

    /// Ids of the bars whose status process asked for click events.
    fn click_bars(&self) -> HashSet<String> {
        self.statuses
            .iter()
            .filter(|(_, s)| s.process.as_ref().is_some_and(|p| p.wants_clicks()))
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// Mouse hook events in order, then the latest coalesced move after them.
    /// Flushes once when anything changed.
    pub fn on_mouse(&mut self, events: Vec<MouseEvent>, latest_move: Option<(i32, i32)>) {
        let mut changed = false;
        for ev in events {
            changed |= self.on_mouse_event(ev);
        }
        if let Some((x, y)) = latest_move {
            // Only the newest position is checked for a missed button-up: an older
            // queued move may precede the real release in the same batch.
            changed |= if self.release_missed(x, y) {
                self.end_drag(x, y)
            } else {
                self.step_drag(x, y)
            };
        }
        if changed {
            self.flush();
        }
    }

    /// Returns whether a flush is needed.
    fn on_mouse_event(&mut self, ev: MouseEvent) -> bool {
        match ev {
            MouseEvent::Press {
                button, x, y, id, ..
            } => match self.wm.tree.window_node(id) {
                Some(node) => {
                    // A modifier-click focuses the window, as in i3.
                    self.wm.focus_node(node);
                    self.begin_drag(node, button, x, y, false);
                    true
                }
                None => false,
            },
            MouseEvent::TitlePress { x, y, node } => {
                self.begin_drag(node, Button::Left, x, y, true);
                false
            }
            MouseEvent::Move { x, y } => self.step_drag(x, y),
            MouseEvent::Release { button, x, y } => {
                if self.drag.as_ref().is_some_and(|s| s.button == button) {
                    self.end_drag(x, y)
                } else {
                    false
                }
            }
            MouseEvent::Cancel => self.cancel_drag(),
        }
    }

    fn begin_drag(&mut self, node: NodeId, button: Button, x: i32, y: i32, from_titlebar: bool) {
        let started = if self.drag.is_none() && self.wm.tree.contains(node) {
            drag::start_drag(&self.wm, node, button, x, y, from_titlebar)
        } else {
            None
        };
        let Some(d) = started else {
            if from_titlebar {
                deco::end_title_drag();
            }
            return;
        };
        let percents = match d.kind {
            DragKind::ResizeTiled { .. } => drag::snapshot_percents(&self.wm, node),
            _ => Vec::new(),
        };
        self.drag = Some(DragSession {
            drag: d,
            button,
            from_titlebar,
            percents,
            target: None,
            watch: ReleaseWatch::default(),
            pending: None,
        });
        mouse::set_drag_active(true);
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// The drop target under (x, y): a workspace button on a bar, else the core's pick.
    fn drop_target_at(&self, node: NodeId, x: i32, y: i32) -> DropTarget {
        match self.bars.workspace_at(x, y) {
            Some(name) => DropTarget::Workspace(name),
            None => self.wm.drop_target_at(node, x, y),
        }
    }

    /// Applies a pointer move to the drag; returns whether a flush is needed.
    fn step_drag(&mut self, x: i32, y: i32) -> bool {
        let now = self.now_ms();
        let Some(mut s) = self.drag.take() else {
            return false;
        };
        let changed = if s.drag.kind == DragKind::Tiling {
            s.drag.last = (x, y);
            let t = self.drop_target_at(s.drag.node, x, y);
            if s.target.as_ref() != Some(&t) {
                match self.wm.drop_preview_rect(&t) {
                    Some(r) => self
                        .overlay
                        .show(r, self.wm.config.colors.focused.indicator),
                    None => self.overlay.hide(),
                }
                s.target = Some(t);
            }
            false
        } else {
            let changed = drag::drag_step(&mut self.wm, &mut s.drag, x, y, now);
            s.pending = drag::pending_step(&s.drag, x, y).then_some((x, y));
            if s.pending.is_some() {
                mainloop::arm_drag_timer();
            }
            changed
        };
        self.drag = Some(s);
        changed
    }

    /// The drag timer fired: applies a throttled tiled-resize step still owed.
    pub fn on_drag_timer(&mut self) {
        let Some((x, y)) = self.drag.as_ref().and_then(|s| s.pending) else {
            return;
        };
        if self.step_drag(x, y) {
            self.flush();
        }
    }

    /// Whether the drag's button was released without a release message (see
    /// `ReleaseWatch`); a hook drag ending this way is force-released in the hook.
    fn release_missed(&mut self, x: i32, y: i32) -> bool {
        let Some(s) = self.drag.as_mut() else {
            return false;
        };
        let missed = s.watch.on_move(!button_up(s.button));
        if missed && !s.from_titlebar {
            let _ = mouse::force_release(x, y);
        }
        missed
    }

    /// Ends the drag at (x, y): last move applied, tiling drag dropped.
    fn end_drag(&mut self, x: i32, y: i32) -> bool {
        let Some(mut s) = self.drag.take() else {
            return false;
        };
        mouse::set_drag_active(false);
        self.overlay.hide();
        if s.from_titlebar {
            deco::end_title_drag();
        }
        let target = if s.drag.kind == DragKind::Tiling {
            Some(self.drop_target_at(s.drag.node, x, y))
        } else {
            // Not throttled: the final position always lands.
            drag::drag_step(&mut self.wm, &mut s.drag, x, y, u64::MAX);
            None
        };
        drag::finish_drag(&mut self.wm, &s.drag, target);
        true
    }

    /// Escape: undoes the drag in progress.
    fn cancel_drag(&mut self) -> bool {
        let Some(s) = self.drag.take() else {
            return false;
        };
        mouse::set_drag_active(false);
        self.overlay.hide();
        if s.from_titlebar {
            deco::end_title_drag();
        }
        drag::cancel_drag(&mut self.wm, &s.drag, &s.percents);
        true
    }

    /// The watched modifier of Hide-mode bars went down or up: redraw only the bars.
    pub fn on_modifier_state(&mut self, held: bool) {
        if self.modifier_held != held {
            self.modifier_held = held;
            self.refresh_bars();
        }
    }

    /// Redraws only the bars with the current blocks.
    fn refresh_bars(&mut self) {
        self.bars_dirty = false;
        let arr = self.wm.arrange();
        let blocks = self.all_blocks();
        self.bars.update(
            &self.wm,
            &arr,
            &blocks,
            &self.click_bars(),
            self.modifier_held,
        );
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
            EventDecision::MoveSizeStart => self.on_move_size_start(id),
            EventDecision::MoveSizeEnd => self.on_move_size_end(id),
        }
        if let PlatformEvent::Foreground(fg) = ev {
            // Between flushes too: a menu or unmanaged window in front blocks drags at once.
            mouse::set_foreground_managed(self.is_managed(fg));
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
            self.on_tiled_move_size_end(id, node);
        }
    }

    /// A native move/size loop begins: remember a tiled window's frame and keep
    /// reconcile from fighting the loop.
    fn on_move_size_start(&mut self, id: WindowId) {
        let Some(node) = self.wm.tree.window_node(id) else {
            return;
        };
        if self.wm.tree.is_floating(node) {
            return;
        }
        if let Some(r) = window::frame_rect(id) {
            self.move_start.insert(id, r);
            self.applied.frozen.insert(id);
        }
    }

    /// A native loop of a tiled window ends: a border resize adjusts the tree, a
    /// title-bar move drops (when allowed); anything else snaps back on the flush.
    fn on_tiled_move_size_end(&mut self, id: WindowId, node: NodeId) {
        self.applied.frozen.remove(&id);
        // Snap back unless the tree changes below (a changed tree re-places anyway).
        reconcile::invalidate(&mut self.applied, id);
        let (Some(start), Some(end)) = (self.move_start.remove(&id), window::frame_rect(id)) else {
            return;
        };
        match drag::classify_native(start, end) {
            NativeDrag::Resized(edges) => {
                for (dir, delta) in edges {
                    self.wm.resize_edge(node, dir, delta);
                }
            }
            NativeDrag::Moved => {
                if self.wm.config.tiling_drag.titlebar {
                    self.refresh_pointer();
                    if let Some((x, y)) = self.wm.pointer {
                        let t = self.drop_target_at(node, x, y);
                        self.wm.drop_window(node, &t);
                    }
                }
            }
        }
    }

    /// Publishes the cursor position for `move position mouse`.
    fn refresh_pointer(&mut self) {
        let mut p = windows::Win32::Foundation::POINT::default();
        // SAFETY: p is a valid out pointer for the synchronous call.
        let ok = unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p) };
        self.wm.pointer = ok.ok().map(|()| (p.x, p.y));
    }

    /// Handles a bound key combo. Does not flush.
    pub fn on_binding(&mut self, combo: KeyCombo, release: bool) {
        self.refresh_pointer();
        self.wm.binding_pressed(combo, release);
    }

    /// Handles a cross-thread message. Does not flush; returns whether the caller
    /// must (status messages only refresh the bars themselves).
    pub fn on_main_msg(&mut self, msg: MainMsg) -> bool {
        match msg {
            MainMsg::StatusUpdate(bar, g, blocks) => {
                if let Some(st) = self.live_status(&bar, g) {
                    st.blocks = blocks;
                    self.bars_dirty = true;
                }
                return false;
            }
            MainMsg::StatusExited(bar, g) => {
                if let Some(st) = self.live_status(&bar, g) {
                    if let Some(mut p) = st.process.take() {
                        p.kill();
                    }
                    st.blocks = vec![exit_block()];
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
                self.refresh_pointer();
                self.wm.run_command(&c);
            }
            MainMsg::StatusClick { bar, click } => {
                if let Some(p) = self.statuses.get_mut(&bar).and_then(|s| s.process.as_mut()) {
                    p.click(&click);
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
        self.refresh_pointer();
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
        // Before the bars: a new watch re-syncs `modifier_held` to its baseline.
        let watch = self
            .wm
            .config
            .bars
            .iter()
            .find(|b| self.wm.bar_state_of(&b.id).0 == BarMode::Hide)
            .and_then(|b| b.modifier);
        if watch != self.last_watch {
            self.modifier_held = keyboard::set_modifier_watch(watch);
            self.last_watch = watch;
        }
        let blocks = self.all_blocks();
        self.bars.update(
            &self.wm,
            &arr,
            &blocks,
            &self.click_bars(),
            self.modifier_held,
        );
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
        let fg_managed = window::foreground().is_some_and(|fg| self.is_managed(fg));
        mouse::set_snapshot(mouse_snapshot(&self.wm, &arr, fg_managed));
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
        mouse::uninstall();
        hidden::restore_all_hidden();
        for (_, mut st) in self.statuses.drain() {
            if let Some(p) = st.process.as_mut() {
                p.kill();
            }
        }
        self.bars.destroy_all();
        self.decos.destroy_all();
        self.overlay.destroy();
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
    fn click_routes_to_owning_bar() {
        use super::route_click;
        let mut ids = std::collections::HashMap::new();
        ids.insert("bar-0".to_string(), 0u8);
        ids.insert("bar-1".to_string(), 1u8);
        assert_eq!(route_click(&ids, "bar-1"), Some(&1));
        assert_eq!(route_click(&ids, "bar-9"), None);
    }
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
    fn mouse_snapshot_puts_last_focused_floating_first() {
        use i3win_core::tree::WindowInfo;
        use i3win_core::wm::OutputInfo;
        let mut config = default_config();
        config.floating_modifier = Some(Modifiers::ALT);
        config.focus_follows_mouse = false;
        let out = OutputInfo {
            name: "O1".into(),
            rect: Rect::new(0, 0, 1000, 500),
            scale: 1.0,
            primary: false,
        };
        let mut wm = Wm::new(config, vec![out]);
        for n in 1..=3 {
            wm.manage(WindowInfo {
                id: WindowId(n),
                title: format!("w{n}"),
                class: format!("c{n}"),
                instance: format!("i{n}"),
                rect: Rect::new(0, 0, 400, 300),
                wants_float: n >= 2,
            });
        }
        // 3 was managed (and focused) last, so it is above 2 despite being placed after it.
        let s = mouse_snapshot(&wm, &wm.arrange(), true);
        let ids: Vec<(isize, bool)> = s.hits.iter().map(|h| (h.id.0, h.floating)).collect();
        assert_eq!(ids, vec![(3, true), (2, true), (1, false)]);
        wm.on_focus_changed(WindowId(2));
        let s = mouse_snapshot(&wm, &wm.arrange(), false);
        assert!(!s.foreground_managed);
        assert_eq!(s.hits[0].id, WindowId(2));
        assert_eq!(s.modifier, Some(Modifiers::ALT));
        assert!(!s.want_moves);
    }
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
