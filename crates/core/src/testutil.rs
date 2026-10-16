use crate::config::Config;
use crate::geom::Rect;
use crate::tree::{WindowId, WindowInfo};
use crate::wm::{OutputInfo, Wm};

pub fn out(name: &str, x: i32, w: i32) -> OutputInfo {
    OutputInfo {
        name: name.into(),
        rect: Rect::new(x, 0, w, 500),
        scale: 1.0,
    }
}

pub fn win(n: isize) -> WindowInfo {
    WindowInfo {
        id: WindowId(n),
        title: format!("w{n}"),
        class: format!("c{n}"),
        instance: format!("i{n}"),
        rect: Rect::new(0, 0, 400, 300),
        wants_float: false,
    }
}

pub fn test_wm() -> Wm {
    Wm::new(Config::default(), vec![out("O1", 0, 1000)])
}

pub fn ws_desc(wm: &Wm) -> String {
    wm.tree.describe(wm.focused_workspace())
}

/// Asserts structural invariants of the whole tree.
pub fn check_invariants(wm: &Wm) {
    use crate::tree::NodeKind;
    fn walk(wm: &Wm, n: crate::tree::NodeId) {
        let node = wm.tree.get(n);
        match node.kind {
            NodeKind::Root | NodeKind::Output { .. } => {}
            NodeKind::Workspace { .. } | NodeKind::Container => {
                if !node.children.is_empty() {
                    let sum: f64 = node.children.iter().map(|&c| wm.tree.get(c).percent).sum();
                    assert!((sum - 1.0).abs() < 1e-9, "percent sum {sum} at {n:?}");
                }
                if matches!(node.kind, NodeKind::Container) {
                    assert!(
                        !node.children.is_empty() || !node.floating.is_empty(),
                        "empty container {n:?}"
                    );
                }
            }
            NodeKind::Window(_) => {}
        }
        for &c in node.children.iter().chain(node.floating.iter()) {
            walk(wm, c);
        }
    }
    walk(wm, wm.tree.root());
    for o in wm.outputs() {
        let vis = *wm
            .tree
            .get(o)
            .focus
            .first()
            .expect("output has a visible workspace");
        for &ws in &wm.tree.get(o).children {
            if ws != vis {
                let n = wm.tree.get(ws);
                assert!(
                    !n.children.is_empty() || !n.floating.is_empty(),
                    "hidden workspace {} is empty",
                    wm.workspace_name(ws)
                );
            }
        }
    }
}
