//! i3 IPC wire framing (`"i3-ipc"`, payload length u32 LE, type u32 LE, payload)
//! plus the JSON replies and events built from the window manager state.

use crate::arrange::Arrangement;
use crate::config::{BarColorTriple, BarPosition, Color};
use crate::geom::{Orientation, Rect};
use crate::tree::{NodeId, NodeKind, WindowId};
use crate::wm::{CommandResult, IpcEvent, Wm};
use serde_json::{json, Value};

pub const MAGIC: &[u8; 6] = b"i3-ipc";
pub const HEADER_LEN: usize = 14;
pub const MAX_PAYLOAD: u32 = 16 * 1024 * 1024;

pub const RUN_COMMAND: u32 = 0;
pub const GET_WORKSPACES: u32 = 1;
pub const SUBSCRIBE: u32 = 2;
pub const GET_OUTPUTS: u32 = 3;
pub const GET_TREE: u32 = 4;
pub const GET_MARKS: u32 = 5;
pub const GET_BAR_CONFIG: u32 = 6;
pub const GET_VERSION: u32 = 7;
pub const GET_BINDING_MODES: u32 = 8;
pub const GET_CONFIG: u32 = 9;
pub const SEND_TICK: u32 = 10;

pub const EVENT_BIT: u32 = 0x8000_0000;
pub const EV_WORKSPACE: u32 = 0;
pub const EV_OUTPUT: u32 = 1;
pub const EV_MODE: u32 = 2;
pub const EV_WINDOW: u32 = 3;
pub const EV_BARCONFIG_UPDATE: u32 = 4;
pub const EV_BINDING: u32 = 5;
pub const EV_SHUTDOWN: u32 = 6;
pub const EV_TICK: u32 = 7;

/// Builds a frame. A payload longer than `u32::MAX` bytes gets a saturated
/// length field (never panics); real payloads are far below `MAX_PAYLOAD`.
pub fn encode(msg_type: u32, payload: &str) -> Vec<u8> {
    let len = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&msg_type.to_le_bytes());
    out.extend_from_slice(payload.as_bytes());
    out
}

/// Parses a 14-byte header into `(payload_len, msg_type)`.
pub fn decode_header(h: &[u8]) -> Result<(u32, u32), String> {
    if h.len() != HEADER_LEN {
        return Err(format!("bad header length {}", h.len()));
    }
    if &h[0..6] != MAGIC {
        return Err("bad magic".to_string());
    }
    let len = u32::from_le_bytes([h[6], h[7], h[8], h[9]]);
    let ty = u32::from_le_bytes([h[10], h[11], h[12], h[13]]);
    if len > MAX_PAYLOAD {
        return Err(format!("payload too large: {len}"));
    }
    Ok((len, ty))
}

/// Request type for an `i3-msg -t` name.
pub fn type_from_name(name: &str) -> Option<u32> {
    Some(match name {
        "command" => RUN_COMMAND,
        "get_workspaces" => GET_WORKSPACES,
        "subscribe" => SUBSCRIBE,
        "get_outputs" => GET_OUTPUTS,
        "get_tree" => GET_TREE,
        "get_marks" => GET_MARKS,
        "get_bar_config" => GET_BAR_CONFIG,
        "get_version" => GET_VERSION,
        "get_binding_modes" => GET_BINDING_MODES,
        "get_config" => GET_CONFIG,
        "send_tick" => SEND_TICK,
        _ => return None,
    })
}

/// Event index for a subscribe event name.
pub fn event_index(name: &str) -> Option<u32> {
    Some(match name {
        "workspace" => EV_WORKSPACE,
        "output" => EV_OUTPUT,
        "mode" => EV_MODE,
        "window" => EV_WINDOW,
        "barconfig_update" => EV_BARCONFIG_UPDATE,
        "binding" => EV_BINDING,
        "shutdown" => EV_SHUTDOWN,
        "tick" => EV_TICK,
        _ => return None,
    })
}

pub struct IpcContext<'a> {
    pub config_path: &'a str,
    pub config_text: &'a str,
}

pub enum Request {
    Reply(String),
    Subscribe(Vec<String>),
    Tick(String),
}

fn to_text(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".to_string())
}

fn failure(msg: &str) -> String {
    to_text(&json!({"success": false, "error": msg}))
}

/// Answers one request. `wm` is mutable because RUN_COMMAND mutates it.
pub fn handle(wm: &mut Wm, ctx: &IpcContext, msg_type: u32, payload: &str) -> Request {
    let reply = match msg_type {
        RUN_COMMAND => command_results_json(&wm.run_command(payload)),
        GET_WORKSPACES => to_text(&workspaces_json(wm)),
        SUBSCRIBE => match serde_json::from_str::<Vec<String>>(payload) {
            Ok(events) => return Request::Subscribe(events),
            Err(_) => failure("invalid subscribe payload"),
        },
        GET_OUTPUTS => to_text(&outputs_json(wm)),
        GET_TREE => to_text(&tree_json(wm)),
        GET_MARKS => to_text(&marks_json(wm)),
        GET_BAR_CONFIG => to_text(&bar_config_json(wm, payload)),
        GET_VERSION => to_text(&version_json(ctx)),
        GET_BINDING_MODES => to_text(&binding_modes_json(wm)),
        GET_CONFIG => to_text(&json!({"config": ctx.config_text})),
        SEND_TICK => return Request::Tick(payload.to_string()),
        _ => failure("unsupported message type"),
    };
    Request::Reply(reply)
}

pub fn command_results_json(r: &[CommandResult]) -> String {
    let v: Vec<Value> = r
        .iter()
        .map(|c| match &c.error {
            Some(e) if !c.success => json!({"success": false, "error": e}),
            _ => json!({"success": c.success}),
        })
        .collect();
    to_text(&Value::Array(v))
}

fn rect_json(r: Rect) -> Value {
    json!({"x": r.x, "y": r.y, "width": r.w, "height": r.h})
}

fn output_rect_of(wm: &Wm, output: NodeId) -> Rect {
    match &wm.tree.get(output).kind {
        NodeKind::Output { rect, .. } => *rect,
        _ => Rect::default(),
    }
}

fn output_name_of(wm: &Wm, output: NodeId) -> String {
    match &wm.tree.get(output).kind {
        NodeKind::Output { name, .. } => name.clone(),
        _ => String::new(),
    }
}

fn ws_num(wm: &Wm, ws: NodeId) -> Option<i32> {
    match &wm.tree.get(ws).kind {
        NodeKind::Workspace { num, .. } => *num,
        _ => None,
    }
}

fn is_urgent(wm: &Wm, id: NodeId) -> bool {
    let n = wm.tree.get(id);
    match &n.kind {
        NodeKind::Window(d) => d.urgent,
        _ => n
            .children
            .iter()
            .chain(n.floating.iter())
            .any(|&c| is_urgent(wm, c)),
    }
}

pub fn workspaces_json(wm: &Wm) -> Value {
    let focused = wm.focused_workspace();
    let list: Vec<Value> = wm
        .sorted_workspaces()
        .into_iter()
        .map(|ws| {
            let out = wm.tree.output_of(ws);
            json!({
                "id": ws.0,
                "num": ws_num(wm, ws).unwrap_or(-1),
                "name": wm.workspace_name(ws),
                "visible": wm.is_visible(ws),
                "focused": ws == focused,
                "urgent": is_urgent(wm, ws),
                "rect": rect_json(out.map(|o| output_rect_of(wm, o)).unwrap_or_default()),
                "output": out.map(|o| output_name_of(wm, o)).unwrap_or_default(),
            })
        })
        .collect();
    Value::Array(list)
}

pub fn outputs_json(wm: &Wm) -> Value {
    let list: Vec<Value> = wm
        .outputs()
        .into_iter()
        .enumerate()
        .map(|(i, o)| {
            json!({
                "name": output_name_of(wm, o),
                "active": true,
                "primary": i == 0,
                "current_workspace": wm.workspace_name(wm.visible_workspace(o)),
                "rect": rect_json(output_rect_of(wm, o)),
            })
        })
        .collect();
    Value::Array(list)
}

fn orientation_str(o: Orientation) -> &'static str {
    match o {
        Orientation::Horizontal => "horizontal",
        Orientation::Vertical => "vertical",
    }
}

/// One tree node (recursive). `arr` is computed once per request by the caller.
fn node_json(wm: &Wm, arr: &Arrangement, id: NodeId) -> Value {
    let n = wm.tree.get(id);
    let floating = wm.tree.is_floating(id);
    let out_rect = wm
        .tree
        .output_of(id)
        .map(|o| output_rect_of(wm, o))
        .unwrap_or_default();
    let (ty, name, rect) = match &n.kind {
        NodeKind::Root => ("root", json!("root"), Rect::default()),
        NodeKind::Output { name, rect, .. } => ("output", json!(name), *rect),
        NodeKind::Workspace { name, .. } => ("workspace", json!(name), out_rect),
        NodeKind::Container => ("con", Value::Null, Rect::default()),
        NodeKind::Window(d) => {
            let rect = arr
                .windows
                .iter()
                .find(|p| p.id == d.info.id)
                .map(|p| p.rect)
                .or(d.floating_rect)
                .unwrap_or(d.info.rect);
            ("con", json!(d.info.title), rect)
        }
    };
    let top = matches!(
        n.kind,
        NodeKind::Root | NodeKind::Output { .. } | NodeKind::Workspace { .. }
    );
    let (layout, orientation) = match n.kind {
        NodeKind::Root => ("splith", "none"),
        NodeKind::Output { .. } => ("output", "none"),
        _ => (n.layout.as_str(), orientation_str(n.layout.orientation())),
    };
    let percent = if top || floating {
        Value::Null
    } else {
        json!(n.percent)
    };
    let mut v = json!({
        "id": id.0,
        "type": ty,
        "name": name,
        "layout": layout,
        "orientation": orientation,
        "percent": percent,
        "rect": rect_json(rect),
        "focused": id == wm.focused(),
        "marks": n.marks,
        "focus": n.focus.iter().map(|c| c.0).collect::<Vec<_>>(),
        "urgent": is_urgent(wm, id),
        "fullscreen_mode": u8::from(n.fullscreen),
        "nodes": n.children.iter().map(|&c| node_json(wm, arr, c)).collect::<Vec<_>>(),
        "floating_nodes": n.floating.iter().map(|&c| node_json(wm, arr, c)).collect::<Vec<_>>(),
    });
    match &n.kind {
        NodeKind::Workspace { num, .. } => v["num"] = json!(num.unwrap_or(-1)),
        NodeKind::Window(d) => {
            v["window"] = json!(d.info.id.0 as i64);
            v["window_properties"] = json!({
                "class": d.info.class,
                "instance": d.info.instance,
                "title": d.info.title,
            });
        }
        _ => {}
    }
    v
}

pub fn tree_json(wm: &Wm) -> Value {
    let arr = wm.arrange();
    node_json(wm, &arr, wm.tree.root())
}

/// Sorted names of all marks in the tree.
pub fn marks_json(wm: &Wm) -> Value {
    json!(wm.all_marks())
}

pub fn version_json(ctx: &IpcContext) -> Value {
    json!({
        "major": 4,
        "minor": 22,
        "patch": 0,
        "human_readable": "4.22 (i3win 0.1.0)",
        "loaded_config_file_name": ctx.config_path,
    })
}

fn color_str(c: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}

pub fn bar_config_json(wm: &Wm, payload: &str) -> Value {
    let Some(bar) = wm.config.bar.as_ref() else {
        return if payload.trim().is_empty() {
            json!([])
        } else {
            json!({"success": false, "error": "no such bar"})
        };
    };
    match payload.trim() {
        "" => json!(["bar-0"]),
        "bar-0" => {
            let font = bar.font.as_ref().unwrap_or(&wm.config.font);
            let c = &bar.colors;
            let mut colors = json!({
                "background": color_str(c.background),
                "statusline": color_str(c.statusline),
                "separator": color_str(c.separator),
            });
            let triples: [(&str, &BarColorTriple); 5] = [
                ("focused_workspace", &c.focused_workspace),
                ("active_workspace", &c.active_workspace),
                ("inactive_workspace", &c.inactive_workspace),
                ("urgent_workspace", &c.urgent_workspace),
                ("binding_mode", &c.binding_mode),
            ];
            for (prefix, t) in triples {
                colors[format!("{prefix}_border")] = json!(color_str(t.border));
                colors[format!("{prefix}_bg")] = json!(color_str(t.background));
                colors[format!("{prefix}_text")] = json!(color_str(t.text));
            }
            let position = match bar.position {
                BarPosition::Top => "top",
                BarPosition::Bottom => "bottom",
            };
            json!({
                "id": "bar-0",
                "mode": "dock",
                "position": position,
                "status_command": bar.status_command,
                "font": format!("pango:{} {}", font.family, font.size_pt),
                "workspace_buttons": bar.workspace_buttons,
                "colors": colors,
            })
        }
        _ => json!({"success": false, "error": "no such bar"}),
    }
}

pub fn binding_modes_json(wm: &Wm) -> Value {
    let mut others: Vec<&String> = wm
        .config
        .bindings
        .keys()
        .filter(|k| k.as_str() != "default")
        .collect();
    others.sort();
    let mut v = vec![json!("default")];
    v.extend(others.into_iter().map(|k| json!(k)));
    Value::Array(v)
}

pub fn tick_event(first: bool, payload: &str) -> String {
    to_text(&json!({"first": first, "payload": payload}))
}

fn workspace_by_name(wm: &Wm, arr: &Arrangement, name: &Option<String>) -> Value {
    name.as_deref()
        .and_then(|n| wm.find_workspace(n))
        .map(|ws| node_json(wm, arr, ws))
        .unwrap_or(Value::Null)
}

fn window_node_json(wm: &Wm, arr: &Arrangement, w: WindowId) -> Value {
    match wm.tree.window_node(w) {
        Some(n) => node_json(wm, arr, n),
        None => json!({
            "id": 0, "type": "con", "window": w.0 as i64, "name": null,
            "nodes": [], "floating_nodes": [],
        }),
    }
}

/// Returns (event index, event name, JSON payload) for an event.
pub fn event_json(wm: &Wm, e: &IpcEvent) -> (u32, &'static str, String) {
    match e {
        IpcEvent::Workspace {
            change,
            current,
            old,
        } => {
            let arr = wm.arrange();
            let v = json!({
                "change": change,
                "current": workspace_by_name(wm, &arr, current),
                "old": workspace_by_name(wm, &arr, old),
            });
            (EV_WORKSPACE, "workspace", to_text(&v))
        }
        IpcEvent::Window { change, window } => {
            let arr = wm.arrange();
            let v = json!({
                "change": change,
                "container": window_node_json(wm, &arr, *window),
            });
            (EV_WINDOW, "window", to_text(&v))
        }
        IpcEvent::Mode { name } => (
            EV_MODE,
            "mode",
            to_text(&json!({"change": name, "pango_markup": false})),
        ),
        IpcEvent::Binding { command } => (
            EV_BINDING,
            "binding",
            to_text(&json!({
                "change": "run",
                "binding": {
                    "command": command,
                    "event_state_mask": [],
                    "input_code": 0,
                    "symbol": null,
                    "input_type": "keyboard",
                },
            })),
        ),
        IpcEvent::Output => (
            EV_OUTPUT,
            "output",
            r#"{"change":"unspecified"}"#.to_string(),
        ),
        IpcEvent::Tick { payload } => (EV_TICK, "tick", tick_event(false, payload)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_layout() {
        let b = encode(RUN_COMMAND, "exit");
        assert_eq!(&b[0..6], b"i3-ipc");
        assert_eq!(&b[6..10], &4u32.to_le_bytes());
        assert_eq!(&b[10..14], &0u32.to_le_bytes());
        assert_eq!(&b[14..], b"exit");
    }
    #[test]
    fn decode_roundtrip() {
        let b = encode(GET_TREE, "{}");
        assert_eq!(decode_header(&b[..14]).unwrap(), (2, 4));
    }
    #[test]
    fn decode_rejects_bad_magic_and_huge_len() {
        assert!(decode_header(b"i4-ipc\0\0\0\0\0\0\0\0").is_err());
        let mut h = encode(0, "").to_vec();
        h[6..10].copy_from_slice(&(MAX_PAYLOAD + 1).to_le_bytes());
        assert!(decode_header(&h).is_err());
        assert!(decode_header(b"short").is_err());
    }
    #[test]
    fn names() {
        assert_eq!(type_from_name("get_version"), Some(7));
        assert_eq!(type_from_name("nope"), None);
        assert_eq!(event_index("tick"), Some(7));
    }
}

#[cfg(test)]
mod reply_tests {
    use super::*;
    use crate::testutil::*;
    use crate::tree::WindowId;
    use crate::wm::{Effect, IpcEvent, Wm};

    fn ctx() -> IpcContext<'static> {
        IpcContext {
            config_path: r"C:\cfg",
            config_text: "set $mod Mod1",
        }
    }
    fn reply(wm: &mut Wm, t: u32, p: &str) -> serde_json::Value {
        match handle(wm, &ctx(), t, p) {
            Request::Reply(s) => serde_json::from_str(&s).unwrap(),
            _ => panic!(),
        }
    }
    #[test]
    fn run_command_reply() {
        let mut wm = test_wm();
        let v = reply(&mut wm, RUN_COMMAND, "nop; mode nosuch");
        assert_eq!(v[0]["success"], true);
        assert_eq!(v[1]["success"], false);
        assert!(v[1]["error"].is_string());
    }
    #[test]
    fn get_workspaces_shape() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("workspace 3");
        let v = reply(&mut wm, GET_WORKSPACES, "");
        assert_eq!(v.as_array().unwrap().len(), 2);
        assert_eq!(v[1]["name"], "3");
        assert_eq!(v[1]["num"], 3);
        assert_eq!(v[1]["focused"], true);
        assert_eq!(v[0]["visible"], false);
        assert_eq!(v[0]["rect"]["width"], 1000);
        assert_eq!(v[0]["output"], "O1");
    }
    #[test]
    fn tree_shape() {
        let mut wm = test_wm();
        wm.manage(win(1));
        let v = reply(&mut wm, GET_TREE, "");
        assert_eq!(v["type"], "root");
        let ws = &v["nodes"][0]["nodes"][0];
        assert_eq!(ws["type"], "workspace");
        assert_eq!(ws["layout"], "splith");
        let w = &ws["nodes"][0];
        assert_eq!(w["window"], 1);
        assert_eq!(w["window_properties"]["class"], "c1");
        assert_eq!(w["focused"], true);
    }
    #[test]
    fn version_and_modes_and_config() {
        let mut wm = test_wm();
        assert_eq!(reply(&mut wm, GET_VERSION, "")["major"], 4);
        assert_eq!(
            reply(&mut wm, GET_BINDING_MODES, ""),
            serde_json::json!(["default"])
        );
        assert_eq!(reply(&mut wm, GET_CONFIG, "")["config"], "set $mod Mod1");
        assert_eq!(reply(&mut wm, GET_MARKS, ""), serde_json::json!([]));
        assert_eq!(reply(&mut wm, GET_BAR_CONFIG, ""), serde_json::json!([]));
        assert_eq!(reply(&mut wm, 99, "")["success"], false);
    }
    #[test]
    fn subscribe_and_tick() {
        let mut wm = test_wm();
        assert!(
            matches!(handle(&mut wm, &ctx(), SUBSCRIBE, r#"["workspace","tick"]"#), Request::Subscribe(v) if v == vec!["workspace".to_string(), "tick".to_string()])
        );
        assert!(matches!(
            handle(&mut wm, &ctx(), SUBSCRIBE, "nope"),
            Request::Reply(_)
        ));
        assert!(matches!(handle(&mut wm, &ctx(), SEND_TICK, "hi"), Request::Tick(p) if p == "hi"));
    }
    #[test]
    fn workspace_event_json() {
        let mut wm = test_wm();
        wm.take_effects();
        wm.manage(win(1));
        wm.run_command("workspace 2");
        let e = wm
            .take_effects()
            .into_iter()
            .find_map(|e| match e {
                Effect::Ipc(ev @ IpcEvent::Workspace { .. })
                    if matches!(&ev, IpcEvent::Workspace { change, .. } if change == "focus") =>
                {
                    Some(ev)
                }
                _ => None,
            })
            .unwrap();
        let (idx, name, p) = event_json(&wm, &e);
        assert_eq!((idx, name), (EV_WORKSPACE, "workspace"));
        let v: serde_json::Value = serde_json::from_str(&p).unwrap();
        assert_eq!(v["current"]["name"], "2");
        assert_eq!(v["old"]["name"], "1");
    }
    #[test]
    fn close_event_without_node() {
        let mut wm = test_wm();
        wm.manage(win(7));
        wm.unmanage(WindowId(7));
        let (_, _, p) = event_json(
            &wm,
            &IpcEvent::Window {
                change: "close".into(),
                window: WindowId(7),
            },
        );
        let v: serde_json::Value = serde_json::from_str(&p).unwrap();
        assert_eq!(v["container"]["window"], 7);
    }
    #[test]
    fn unicode_workspace_names_roundtrip() {
        let mut wm = test_wm();
        reply(&mut wm, RUN_COMMAND, r#"workspace "3: “ünï” \"q\" x""#);
        let v = reply(&mut wm, GET_WORKSPACES, "");
        assert_eq!(v[0]["name"], r#"3: “ünï” "q" x"#);
    }
    #[test]
    fn misc_events_and_bar_config() {
        let mut wm = test_wm();
        let (i, n, p) = event_json(&wm, &IpcEvent::Output);
        assert_eq!(
            (i, n, p.as_str()),
            (EV_OUTPUT, "output", r#"{"change":"unspecified"}"#)
        );
        let (i, n, p) = event_json(
            &wm,
            &IpcEvent::Tick {
                payload: "x".into(),
            },
        );
        assert_eq!((i, n), (EV_TICK, "tick"));
        assert_eq!(p, tick_event(false, "x"));
        let (_, _, p) = event_json(
            &wm,
            &IpcEvent::Mode {
                name: "resize".into(),
            },
        );
        assert!(p.contains("\"change\":\"resize\""));
        wm.config = crate::config::parse_config(
            "bar {
}
",
        );
        assert_eq!(
            reply(&mut wm, GET_BAR_CONFIG, ""),
            serde_json::json!(["bar-0"])
        );
        let b = reply(&mut wm, GET_BAR_CONFIG, "bar-0");
        assert_eq!(b["id"], "bar-0");
        assert!(b["colors"]["background"].as_str().unwrap().starts_with('#'));
        assert_eq!(
            reply(&mut wm, GET_BAR_CONFIG, "nope")["error"],
            "no such bar"
        );
    }
}
