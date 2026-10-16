use crate::tree::{NodeId, NodeKind};
use crate::wm::Wm;

#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceButton {
    pub name: String,
    pub focused: bool,
    pub visible: bool,
    pub urgent: bool,
}

/// Buttons for the workspaces of `output`, in `children` order.
pub fn workspace_buttons(wm: &Wm, output: NodeId) -> Vec<WorkspaceButton> {
    let focused = wm.focused_workspace();
    let visible = wm.visible_workspace(output);
    wm.tree
        .get(output)
        .children
        .iter()
        .map(|&ws| WorkspaceButton {
            name: wm.workspace_name(ws).to_string(),
            focused: ws == focused,
            visible: ws == visible,
            urgent: wm
                .tree
                .windows_under(ws)
                .into_iter()
                .any(|w| matches!(&wm.tree.get(w).kind, NodeKind::Window(d) if d.urgent)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::testutil::{out, win};

    #[test]
    fn buttons_reflect_state() {
        let mut wm = Wm::new(
            Config::default(),
            vec![out("A", 0, 1000), out("B", 1000, 1000)],
        );
        wm.manage(win(1));
        wm.run_command("workspace 3");
        wm.manage(win(2));
        let a = wm.outputs()[0];
        assert_eq!(
            workspace_buttons(&wm, a),
            vec![
                WorkspaceButton {
                    name: "1".into(),
                    focused: false,
                    visible: false,
                    urgent: false
                },
                WorkspaceButton {
                    name: "3".into(),
                    focused: true,
                    visible: true,
                    urgent: false
                },
            ]
        );
        let b = wm.outputs()[1];
        assert_eq!(
            workspace_buttons(&wm, b),
            vec![WorkspaceButton {
                name: "2".into(),
                focused: false,
                visible: true,
                urgent: false
            }]
        );
    }
}
