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
