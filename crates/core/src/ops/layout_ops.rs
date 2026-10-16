use crate::command::{LayoutArg, SplitArg};
use crate::geom::Orientation;
use crate::tree::{Layout, NodeId, NodeKind};
use crate::wm::Wm;

const MIN_PERCENT: f64 = 0.05;
const EPS: f64 = 1e-9;
const MIN_FLOAT_SIZE: i64 = 50;

fn clamp_i32(v: i64) -> i32 {
    v.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

impl Wm {
    pub fn split(&mut self, target: NodeId, arg: &SplitArg) -> Result<(), String> {
        let t = self.tree.get(target);
        let is_ws = matches!(t.kind, NodeKind::Workspace { .. });
        let is_window = matches!(t.kind, NodeKind::Window(_));
        if !is_ws && !is_window && !matches!(t.kind, NodeKind::Container) {
            return Ok(());
        }
        if self.tree.is_floating(target) {
            return Ok(());
        }
        let layout = match arg {
            SplitArg::Horizontal => Layout::SplitH,
            SplitArg::Vertical => Layout::SplitV,
            SplitArg::Toggle => {
                let basis = if is_ws {
                    t.layout
                } else {
                    match t.parent {
                        Some(p) => self.tree.get(p).layout,
                        None => t.layout,
                    }
                };
                match basis {
                    Layout::SplitV | Layout::Stacked => Layout::SplitH,
                    _ => Layout::SplitV,
                }
            }
        };
        if is_window {
            self.focused_container = None;
        }
        if is_ws {
            if self.tree.get(target).children.len() <= 1 {
                self.tree.get_mut(target).layout = layout;
            }
        } else if is_window {
            match self.tree.parent(target) {
                Some(p) if self.tree.get(p).children.len() == 1 => {
                    self.tree.get_mut(p).layout = layout;
                }
                Some(_) => {
                    self.tree.wrap(target, layout);
                }
                None => {}
            }
        } else {
            self.tree.get_mut(target).layout = layout;
        }
        Ok(())
    }

    pub fn set_layout(&mut self, target: NodeId, arg: &LayoutArg) -> Result<(), String> {
        let c = match self.tree.get(target).kind {
            NodeKind::Container | NodeKind::Workspace { .. } => target,
            NodeKind::Window(_) => {
                if self.tree.is_floating(target) {
                    return Ok(());
                }
                match self.tree.parent(target) {
                    Some(p) => p,
                    None => return Ok(()),
                }
            }
            _ => return Ok(()),
        };
        let cur = self.tree.get(c).layout;
        let new = match arg {
            LayoutArg::Set(l) => *l,
            LayoutArg::Default => Layout::SplitH,
            LayoutArg::ToggleSplit => {
                if cur == Layout::SplitH {
                    Layout::SplitV
                } else {
                    Layout::SplitH
                }
            }
            LayoutArg::ToggleAll => match cur {
                Layout::Stacked => Layout::Tabbed,
                Layout::Tabbed => Layout::SplitH,
                Layout::SplitH => Layout::SplitV,
                Layout::SplitV => Layout::Stacked,
            },
        };
        self.tree.get_mut(c).layout = new;
        Ok(())
    }

    pub fn resize(
        &mut self,
        target: NodeId,
        grow: bool,
        axis: Orientation,
        px: Option<i32>,
        ppt: Option<i32>,
    ) -> Result<(), String> {
        let Some(ws) = self.tree.workspace_of(target) else {
            return Err("No container found to resize".into());
        };
        let out_size = self
            .tree
            .output_of(ws)
            .and_then(|o| match &self.tree.get(o).kind {
                NodeKind::Output { rect, .. } => Some(match axis {
                    Orientation::Horizontal => rect.w,
                    Orientation::Vertical => rect.h,
                }),
                _ => None,
            });
        if self.tree.is_floating(target) {
            return self.resize_floating(target, grow, axis, px, ppt, out_size);
        }
        let want = match axis {
            Orientation::Horizontal => Layout::SplitH,
            Orientation::Vertical => Layout::SplitV,
        };
        let mut found = None;
        let mut c = target;
        while c != ws {
            let Some(p) = self.tree.parent(c) else { break };
            let pn = self.tree.get(p);
            if pn.layout == want && pn.children.len() >= 2 && pn.children.contains(&c) {
                found = Some((c, p));
                break;
            }
            c = p;
        }
        let Some((c, p)) = found else {
            return Err("No container found to resize".into());
        };
        let mut delta = match (ppt, px) {
            (Some(ppt), _) => ppt as f64 / 100.0,
            (None, Some(px)) => match out_size {
                Some(s) if s > 0 => px as f64 / s as f64,
                _ => return Err("cannot resize: output has no size".into()),
            },
            (None, None) => return Err("resize needs a size".into()),
        };
        if !grow {
            delta = -delta;
        }
        let sibs = self.tree.get(p).children.clone();
        let n = sibs.len();
        let old = self.tree.get(c).percent;
        let new = old + delta;
        if !new.is_finite()
            || new < MIN_PERCENT - EPS
            || new > 1.0 - MIN_PERCENT * (n as f64 - 1.0) + EPS
        {
            return Err("cannot resize further".into());
        }
        let denom = 1.0 - old;
        if denom <= 1e-12 {
            return Err("cannot resize further".into());
        }
        let factor = (1.0 - new) / denom;
        let mut updates = Vec::with_capacity(n);
        for &s in sibs.iter().filter(|&&s| s != c) {
            let v = self.tree.get(s).percent * factor;
            if !v.is_finite() || v < MIN_PERCENT - EPS {
                return Err("cannot resize further".into());
            }
            updates.push((s, v));
        }
        for (s, v) in updates {
            self.tree.get_mut(s).percent = v;
        }
        self.tree.get_mut(c).percent = new;
        Ok(())
    }

    fn resize_floating(
        &mut self,
        target: NodeId,
        grow: bool,
        axis: Orientation,
        px: Option<i32>,
        ppt: Option<i32>,
        out_size: Option<i32>,
    ) -> Result<(), String> {
        let d: i64 = match (px, ppt) {
            (Some(px), _) => px as i64,
            (None, Some(ppt)) => (ppt as i64).saturating_mul(out_size.unwrap_or(0) as i64) / 100,
            (None, None) => return Err("resize needs a size".into()),
        };
        let NodeKind::Window(w) = &mut self.tree.get_mut(target).kind else {
            return Ok(());
        };
        let Some(r) = w.floating_rect.as_mut() else {
            return Ok(());
        };
        let (pos, size) = match axis {
            Orientation::Horizontal => (&mut r.x, &mut r.w),
            Orientation::Vertical => (&mut r.y, &mut r.h),
        };
        let signed = if grow { d } else { d.saturating_neg() };
        let new_size = (*size as i64)
            .saturating_add(signed)
            .clamp(MIN_FLOAT_SIZE, i32::MAX as i64);
        let actual = new_size - *size as i64;
        *pos = clamp_i32((*pos as i64).saturating_sub(actual / 2));
        *size = clamp_i32(new_size);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::geom::Rect;
    use crate::testutil::*;
    use crate::tree::{NodeKind, WindowId};
    use crate::wm::Wm;

    #[test]
    fn split_wraps_when_siblings_exist() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("split v");
        wm.manage(win(3));
        assert_eq!(ws_desc(&wm), "H[w1 V[w2 w3]]");
    }
    #[test]
    fn split_on_only_child_changes_parent() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("split v");
        wm.manage(win(2));
        assert_eq!(ws_desc(&wm), "V[w1 w2]");
    }
    #[test]
    fn split_keeps_window_focused() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("split v");
        assert_eq!(wm.focused_window(), Some(WindowId(2)));
    }
    #[test]
    fn layout_applies_to_parent() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("layout tabbed");
        assert_eq!(ws_desc(&wm), "T[w1 w2]");
        wm.run_command("layout toggle split");
        assert_eq!(ws_desc(&wm), "H[w1 w2]");
        wm.run_command("layout toggle split");
        assert_eq!(ws_desc(&wm), "V[w1 w2]");
        wm.run_command("layout toggle all");
        assert_eq!(ws_desc(&wm), "S[w1 w2]");
    }
    #[test]
    fn resize_grow_width_ppt() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("resize grow width 10 px or 10 ppt");
        let p = |w: isize, wm: &Wm| {
            wm.tree
                .get(wm.tree.window_node(WindowId(w)).unwrap())
                .percent
        };
        assert!((p(2, &wm) - 0.6).abs() < 1e-9 && (p(1, &wm) - 0.4).abs() < 1e-9);
    }
    #[test]
    fn resize_height_finds_vertical_ancestor() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("split v");
        wm.manage(win(2));
        assert!(!wm.run_command("resize grow width 10 ppt")[0].success);
        assert!(wm.run_command("resize grow height 10 ppt")[0].success);
    }
    #[test]
    fn resize_clamps_and_keeps_sum() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.manage(win(3));
        for _ in 0..20 {
            let _ = wm.run_command("resize grow width 10 ppt");
        }
        let ws = wm.focused_workspace();
        let sum: f64 = wm
            .tree
            .get(ws)
            .children
            .iter()
            .map(|&c| wm.tree.get(c).percent)
            .sum();
        assert!((sum - 1.0).abs() < 1e-9);
        for &c in &wm.tree.get(ws).children {
            let p = wm.tree.get(c).percent;
            assert!(p.is_finite() && p >= 0.05 - 1e-9);
        }
        assert!(!wm.run_command("resize grow width 10 ppt")[0].success);
    }
    #[test]
    fn resize_floating_px() {
        let mut wm = test_wm();
        let mut f = win(1);
        f.wants_float = true;
        wm.manage(f);
        wm.run_command("resize grow width 20 px or 10 ppt");
        let n = wm.tree.window_node(WindowId(1)).unwrap();
        match &wm.tree.get(n).kind {
            NodeKind::Window(d) => {
                assert_eq!(d.floating_rect.unwrap(), Rect::new(290, 100, 420, 300))
            }
            _ => panic!(),
        }
    }
    #[test]
    fn resize_extreme_values_are_safe() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        assert!(!wm.run_command("resize grow width 2147483647 ppt")[0].success);
        assert!(!wm.run_command("resize shrink width 2147483647 px")[0].success);
        let mut f = win(3);
        f.wants_float = true;
        wm.manage(f);
        wm.run_command("resize grow width 2147483647 px");
        wm.run_command("resize shrink width 2147483647 px");
        wm.run_command("resize grow height 2147483647 ppt");
    }
}

#[cfg(test)]
mod fix_tests {
    use crate::testutil::*;
    use crate::tree::{Layout, WindowId};

    #[test]
    fn split_on_selected_container_keeps_selection() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        let n2 = wm.tree.window_node(WindowId(2)).unwrap();
        let c = wm.tree.wrap(n2, Layout::SplitV);
        wm.manage(win(3));
        wm.run_command("focus parent");
        assert_eq!(wm.focused(), c);
        wm.run_command("split v");
        assert_eq!(wm.focused(), c);
    }
    #[test]
    fn resize_landing_exactly_on_min_is_accepted() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        let ws = wm.focused_workspace();
        let ch = wm.tree.get(ws).children.clone();
        wm.tree.get_mut(ch[0]).percent = 0.15;
        wm.tree.get_mut(ch[1]).percent = 0.85;
        let r = wm.run_command("resize shrink width 10 ppt");
        assert!(r[0].success, "{:?}", r[0].error);
    }
}
