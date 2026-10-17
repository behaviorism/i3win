use crate::command::Toggle;
use crate::geom::{sanitize_floating, Rect};
use crate::tree::{NodeId, NodeKind, ScratchState};
use crate::wm::{IpcEvent, Wm};

fn want(t: &Toggle, current: bool) -> bool {
    match t {
        Toggle::Toggle => !current,
        Toggle::Enable => true,
        Toggle::Disable => false,
    }
}

impl Wm {
    pub fn set_floating(&mut self, target: NodeId, t: &Toggle) -> Result<(), String> {
        match self.tree.get(target).kind {
            NodeKind::Window(_) => {}
            NodeKind::Container => return Err("only windows can float".into()),
            _ => return Ok(()),
        }
        let Some(ws) = self.tree.workspace_of(target) else {
            return Ok(());
        };
        let floating = self.tree.is_floating(target);
        let enable = want(t, floating);
        if ws == self.scratch {
            return if enable {
                Ok(())
            } else {
                Err("Can't disable floating for container in internal workspace".into())
            };
        }
        if enable == floating {
            return Ok(());
        }
        if !enable {
            self.tree.get_mut(target).scratchpad = ScratchState::None;
        }
        let old_parent = self.tree.parent(target);
        self.tree.detach(target);
        if let Some(p) = old_parent {
            if matches!(self.tree.get(p).kind, NodeKind::Container) {
                self.tree.cleanup(p);
            }
        }
        if enable {
            let out_rect = self
                .tree
                .output_of(ws)
                .and_then(|o| match &self.tree.get(o).kind {
                    NodeKind::Output { rect, .. } => Some(*rect),
                    _ => None,
                });
            if let NodeKind::Window(d) = &mut self.tree.get_mut(target).kind {
                if d.floating_rect.is_none() {
                    let r = d.info.rect;
                    d.floating_rect = Some(sanitize_floating(match out_rect {
                        Some(o) => {
                            // i64 math: 80% of the output without overflow.
                            let cap = |v: i32| (i64::from(v) * 4 / 5) as i32;
                            Rect::new(0, 0, r.w.min(cap(o.w)), r.h.min(cap(o.h))).centered_in(&o)
                        }
                        None => r,
                    }));
                }
            }
            self.tree.attach_floating(ws, target);
        } else {
            self.insert_tiled(ws, target);
        }
        self.focused_container = None;
        self.focus_in_place(target);
        if let Some(w) = self.tree.window_id(target) {
            self.emit(IpcEvent::Window {
                change: "floating".into(),
                window: w,
            });
        }
        Ok(())
    }

    pub fn set_fullscreen(&mut self, target: NodeId, t: &Toggle) -> Result<(), String> {
        match self.tree.get(target).kind {
            NodeKind::Window(_) => {}
            NodeKind::Container => return Err("only windows can be fullscreen".into()),
            _ => return Ok(()),
        }
        let enable = want(t, self.tree.get(target).fullscreen);
        if enable {
            if let Some(ws) = self.tree.workspace_of(target) {
                for n in self.tree.windows_under(ws) {
                    self.tree.get_mut(n).fullscreen = false;
                }
            }
            self.tree.get_mut(target).fullscreen = true;
            self.focused_container = None;
            self.focus_in_place(target);
        } else {
            self.tree.get_mut(target).fullscreen = false;
        }
        if let Some(w) = self.tree.window_id(target) {
            self.emit(IpcEvent::Window {
                change: "fullscreen_mode".into(),
                window: w,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::geom::Rect;
    use crate::testutil::*;
    use crate::tree::{Layout, NodeKind, WindowId};

    #[test]
    fn floating_toggle_roundtrip() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("floating toggle");
        assert_eq!(ws_desc(&wm), "H[w1]");
        assert!(wm
            .tree
            .is_floating(wm.tree.window_node(WindowId(2)).unwrap()));
        wm.run_command("floating toggle");
        assert_eq!(ws_desc(&wm), "H[w1 w2]");
    }
    #[test]
    fn floating_rect_is_remembered() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("floating enable");
        wm.on_floating_moved(WindowId(1), Rect::new(10, 10, 300, 200));
        wm.run_command("floating disable");
        wm.run_command("floating enable");
        let n = wm.tree.window_node(WindowId(1)).unwrap();
        match &wm.tree.get(n).kind {
            NodeKind::Window(d) => {
                assert_eq!(d.floating_rect, Some(Rect::new(10, 10, 300, 200)))
            }
            _ => panic!(),
        }
    }
    #[test]
    fn floating_container_errors() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        let n2 = wm.tree.window_node(WindowId(2)).unwrap();
        wm.tree.wrap(n2, Layout::SplitV);
        wm.run_command("focus parent");
        assert!(!wm.run_command("floating enable")[0].success);
    }
    #[test]
    fn fullscreen_is_exclusive_per_workspace() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("fullscreen enable");
        wm.run_command("[class=\"c1\"] fullscreen enable");
        let fs = |w: isize, wm: &crate::wm::Wm| {
            wm.tree
                .get(wm.tree.window_node(WindowId(w)).unwrap())
                .fullscreen
        };
        assert!(fs(1, &wm) && !fs(2, &wm));
        wm.run_command("move container to workspace 3");
        assert!(!fs(1, &wm));
    }
}
