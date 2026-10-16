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
use i3win_core::keys::KeyCombo;
use i3win_core::tree::{NodeKind, WindowId};
use i3win_core::wm::{Effect, Wm};

use crate::mainloop::{self, MainMsg};
use crate::paths;
use crate::reconcile::{self, Applied, WinOp};
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

fn event_id(ev: PlatformEvent) -> WindowId {
    use PlatformEvent::*;
    match ev {
        Shown(i) | Hidden(i) | Destroyed(i) | Foreground(i) | Minimized(i) | Restored(i)
        | MoveSizeEnd(i) | TitleChanged(i) => i,
    }
}

fn parse_file(path: &Path) -> Result<Config, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    Ok(parse_config(&text))
}

fn default_config() -> Config {
    let path_var = std::env::var("PATH").unwrap_or_default();
    parse_config(&default_config_text(paths::find_on_path(
        "wt.exe", &path_var,
    )))
}

fn spawn(command: &str) {
    let mut cmd = Command::new("cmd.exe");
    cmd.raw_arg(format!("/C start \"\" {command}"))
        .creation_flags(CREATE_NO_WINDOW);
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
    store: HiddenStore,
    config_path: PathBuf,
    last_combos: Vec<KeyCombo>,
    /// Effects queued outside the core (exec lines from the config), flushed first.
    pending: Vec<Effect>,
}

impl App {
    pub fn start(config_path: PathBuf) -> App {
        let config = match paths::load_or_create_config(&config_path) {
            Ok(text) => parse_config(&text),
            Err(e) => {
                log::error!("config: {e}; using the default config");
                default_config()
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
        // Topmost window last so it ends up with focus.
        for id in window::enumerate_top_level().into_iter().rev() {
            Self::try_manage_in(&mut wm, id);
        }
        if let Some(fg) = window::foreground() {
            wm.on_focus_changed(fg);
        }
        App {
            wm,
            applied: Applied::default(),
            tracker: HideTracker::new(),
            decos: Decos::new(),
            store: HiddenStore::open(paths::hidden_path()),
            config_path,
            last_combos: Vec::new(),
            pending,
        }
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
        let expected_hide =
            matches!(ev, PlatformEvent::Hidden(_)) && self.tracker.consume(id, Instant::now());
        match decide(ev, self.is_managed(id), expected_hide) {
            EventDecision::Ignore => {}
            EventDecision::TryManage => Self::try_manage_in(&mut self.wm, id),
            EventDecision::Unmanage => {
                self.wm.unmanage(id);
                self.tracker.forget(id);
                self.store.remove(id);
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
                let frame = floating_frame_from_client(&self.wm.config, scale, r);
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

    /// Handles a cross-thread message. Does not flush.
    pub fn on_main_msg(&mut self, msg: MainMsg) {
        match msg {
            // Shutdown is handled by the window procedure itself.
            MainMsg::Shutdown => {}
            #[cfg(debug_assertions)]
            MainMsg::DebugPanic => panic!("debug panic requested via I3WIN_PANIC_AFTER_SECS"),
            MainMsg::FocusNode(n) => {
                if self.wm.tree.contains(n) {
                    self.wm.focus_node(n);
                }
            }
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
                    Effect::Exec(c) => spawn(&c),
                    Effect::Close(w) => window::close(w),
                    Effect::Focus(w) => focus.push(w),
                    Effect::Exit => mainloop::post(MainMsg::Shutdown),
                    Effect::Reload => self.reload(),
                    Effect::Ipc(_) => {}
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
        for op in reconcile::plan(&mut self.applied, &arr, &managed) {
            match op {
                WinOp::Hide(id) => {
                    if window::hide(id) {
                        self.tracker.expect(id, Instant::now());
                    }
                    self.store.add(id);
                }
                WinOp::Show(id) => {
                    window::show(id);
                    self.store.remove(id);
                }
                WinOp::Place { id, rect, raise } => window::place(id, rect, raise),
            }
        }
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
        // Focus last: focusing a still-hidden window fails.
        for w in focus {
            window::focus(w);
        }
        let combos = self.wm.active_combos();
        if combos != self.last_combos {
            keyboard::set_combos(&combos);
            self.last_combos = combos;
        }
    }

    fn reload(&mut self) {
        match parse_file(&self.config_path) {
            Ok(config) => {
                for w in &config.warnings {
                    log::warn!("config: {w}");
                }
                let always = config.exec_always.clone();
                self.wm.reload_config(config);
                self.pending.extend(always.into_iter().map(Effect::Exec));
                // New geometry settings: re-place everything.
                self.applied.rects.clear();
            }
            Err(e) => log::error!("reload failed: {e}"),
        }
    }

    pub fn shutdown(&mut self) {
        events::uninstall();
        keyboard::uninstall();
        hidden::restore_all_hidden();
        self.decos.destroy_all();
        self.store.clear_file();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hidden_expected_is_ignored() {
        assert_eq!(
            decide(PlatformEvent::Hidden(WindowId(1)), true, true),
            EventDecision::Ignore
        );
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
