//! Pure rules for i3blocks-style command blocks: environment, `$VAR`
//! expansion, shell selection, output parsing and exit-code handling.
//! Process spawning lives elsewhere.

use crate::i3blocks_conf::Section;
use i3win_core::i3bar::{Block, Click, MinWidth};
use serde_json::Value;
use std::path::Path;

/// Largest pixel value accepted from block output (matches core's parser).
const MAX_PX: f64 = 10_000.0;

#[derive(Clone, Debug, PartialEq)]
pub enum Interval {
    Secs(f64),
    Once,
    Repeat,
    Persist,
    /// Run at start, then only on click or signal.
    None,
}

pub fn parse_interval(v: Option<&str>) -> Interval {
    match v.map(str::trim) {
        None | Some("") => Interval::None,
        Some("once") => Interval::Once,
        Some("repeat") => Interval::Repeat,
        Some("persist") => Interval::Persist,
        Some(n) => match n.parse::<f64>() {
            Ok(s) if s.is_finite() && s > 0.0 => Interval::Secs(s),
            _ => Interval::None,
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OutFormat {
    Raw,
    Json,
}

fn set_env(env: &mut Vec<(String, String)>, k: &str, v: String) {
    match env.iter_mut().find(|(key, _)| key == k) {
        Some(slot) => slot.1 = v,
        None => env.push((k.to_string(), v)),
    }
}

pub fn block_env(
    section: &Section,
    script_dir: &Path,
    click: Option<&Click>,
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();
    for (k, _) in &section.props {
        if let Some(v) = section.get(k) {
            set_env(&mut env, k, v.to_string());
        }
    }
    let instance = section.get("instance").unwrap_or("").to_string();
    set_env(
        &mut env,
        "SCRIPT_DIR",
        script_dir.to_string_lossy().into_owned(),
    );
    set_env(&mut env, "name", section.name.clone());
    set_env(&mut env, "BLOCK_NAME", section.name.clone());
    set_env(&mut env, "instance", instance.clone());
    set_env(&mut env, "BLOCK_INSTANCE", instance);
    set_env(
        &mut env,
        "BLOCK_INTERVAL",
        section.get("interval").unwrap_or("").to_string(),
    );
    if let Some(c) = click {
        let n = |v: i32| v.to_string();
        set_env(&mut env, "button", c.button.to_string());
        set_env(&mut env, "x", n(c.x));
        set_env(&mut env, "y", n(c.y));
        set_env(&mut env, "relative_x", n(c.relative_x));
        set_env(&mut env, "relative_y", n(c.relative_y));
        set_env(&mut env, "width", n(c.width));
        set_env(&mut env, "height", n(c.height));
        set_env(&mut env, "modifiers", c.modifiers.join(","));
        set_env(&mut env, "BLOCK_BUTTON", c.button.to_string());
        set_env(&mut env, "BLOCK_X", n(c.x));
        set_env(&mut env, "BLOCK_Y", n(c.y));
    }
    env
}

fn lookup<'a>(env: &'a [(String, String)], name: &str) -> &'a str {
    env.iter()
        .rev()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
        .unwrap_or("")
}

/// sh-like expansion for `cmd.exe` command lines: `$VAR`/`${VAR}` from `env`
/// (unknown -> empty), `'...'` literal with the quotes removed, `\$` -> `$`,
/// `$$` -> `pid`. Double quotes are kept (cmd needs them); `$` expands inside.
pub fn expand_sh_vars(cmd: &str, env: &[(String, String)], pid: u32) -> String {
    let chars: Vec<char> = cmd.chars().collect();
    let mut out = String::new();
    let mut in_double = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' if !in_double => {
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    out.push(chars[i]);
                    i += 1;
                }
                i += 1; // closing quote (or end of input)
                continue;
            }
            '"' => {
                in_double = !in_double;
                out.push(c);
            }
            '\\' if chars.get(i + 1) == Some(&'$') => {
                out.push('$');
                i += 1;
            }
            '$' => match chars.get(i + 1) {
                Some('$') => {
                    out.push_str(&pid.to_string());
                    i += 1;
                }
                Some('{') => match chars[i + 2..].iter().position(|&ch| ch == '}') {
                    Some(len) => {
                        let name: String = chars[i + 2..i + 2 + len].iter().collect();
                        out.push_str(lookup(env, &name));
                        i += len + 2;
                    }
                    None => out.push('$'),
                },
                Some(&n) if n == '_' || n.is_ascii_alphabetic() => {
                    let mut j = i + 1;
                    while j < chars.len() && (chars[j] == '_' || chars[j].is_ascii_alphanumeric()) {
                        j += 1;
                    }
                    let name: String = chars[i + 1..j].iter().collect();
                    out.push_str(lookup(env, &name));
                    i = j;
                    continue;
                }
                _ => out.push('$'),
            },
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

/// `cmd`/`cmd.exe` in any case, or empty (the default shell).
pub fn is_cmd_shell(shell: &str) -> bool {
    let s = shell.trim();
    s.is_empty() || s.eq_ignore_ascii_case("cmd") || s.eq_ignore_ascii_case("cmd.exe")
}

/// Program, arguments, and whether the arguments are a raw command line.
pub fn shell_command(shell: &str, cmd: &str) -> (String, Vec<String>, bool) {
    if is_cmd_shell(shell) {
        return (
            "cmd".into(),
            vec!["/S".into(), "/C".into(), format!("\"{cmd}\"")],
            true,
        );
    }
    match shell {
        "pwsh" | "powershell" => (
            shell.into(),
            vec!["-NoProfile".into(), "-Command".into(), cmd.into()],
            false,
        ),
        other => (other.into(), vec!["-c".into(), cmd.into()], false),
    }
}

fn px_f(f: f64) -> Option<i32> {
    f.is_finite().then(|| f.round().clamp(0.0, MAX_PX) as i32)
}

fn px_str(s: &str) -> Option<i32> {
    s.trim().parse::<f64>().ok().and_then(px_f)
}

fn opt(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

fn truthy(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

/// Sets one property from its text form; an empty value unsets it.
fn set_prop(b: &mut Block, key: &str, v: &str) {
    match key {
        "full_text" => b.full_text = v.to_string(),
        "short_text" => b.short_text = opt(v),
        "color" => b.color = opt(v),
        "background" => b.background = opt(v),
        "border" => b.border = opt(v),
        "name" => b.name = opt(v),
        "instance" => b.instance = opt(v),
        "align" => b.align = opt(v),
        "markup" => b.markup = opt(v),
        "urgent" => b.urgent = truthy(v).unwrap_or(false),
        "separator" => b.separator = truthy(v).unwrap_or(true),
        "separator_block_width" => b.separator_block_width = px_str(v),
        "min_width" => {
            b.min_width = match (v.is_empty(), px_str(v)) {
                (true, _) => None,
                (false, Some(px)) => Some(MinWidth::Px(px)),
                (false, None) => Some(MinWidth::Text(v.to_string())),
            }
        }
        "border_top" => b.border_top = px_str(v),
        "border_bottom" => b.border_bottom = px_str(v),
        "border_left" => b.border_left = px_str(v),
        "border_right" => b.border_right = px_str(v),
        _ => {}
    }
}

/// Property per output line, in i3blocks order.
const RAW_LINES: [&str; 17] = [
    "full_text",
    "short_text",
    "color",
    "background",
    "border",
    "min_width",
    "align",
    "name",
    "instance",
    "urgent",
    "separator",
    "separator_block_width",
    "markup",
    "border_top",
    "border_bottom",
    "border_left",
    "border_right",
];

pub fn apply_raw_output(block: &mut Block, out: &str) {
    let mut lines = out.lines();
    // No output at all still replaces the text.
    block.full_text = lines.next().unwrap_or("").to_string();
    for (key, line) in RAW_LINES.iter().skip(1).zip(lines) {
        set_prop(block, key, line);
    }
}

pub fn apply_json_line(block: &mut Block, line: &str) -> bool {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(line.trim()) else {
        return false;
    };
    for (key, val) in &map {
        let text = match val {
            Value::String(s) => s.clone(),
            Value::Bool(b) => b.to_string(),
            Value::Null => String::new(),
            Value::Number(n) => match (key.as_str(), n.as_f64()) {
                ("min_width", Some(f)) => match px_f(f) {
                    Some(px) => {
                        block.min_width = Some(MinWidth::Px(px));
                        continue;
                    }
                    None => continue,
                },
                _ => n.to_string(),
            },
            _ => continue,
        };
        // A JSON string min_width is always a text, even if numeric.
        if key == "min_width" && !text.is_empty() {
            if let Value::String(s) = val {
                block.min_width = Some(MinWidth::Text(s.clone()));
                continue;
            }
        }
        set_prop(block, key, &text);
    }
    true
}

pub fn exit_code_block(block: &mut Block, cmd: &str, code: i32) {
    let msg = match code {
        0 => return,
        33 => {
            block.urgent = true;
            return;
        }
        126 => format!("Error: Command '{cmd}' not executable"),
        127 => format!("Error: Command '{cmd}' not found or missing dependency"),
        n => format!("Error: Command '{cmd}' exited unexpectedly with code {n}"),
    };
    block.full_text = msg;
    block.short_text = None;
    block.urgent = true;
}

pub fn static_block(section: &Section) -> Block {
    let mut b = Block::new("");
    for (key, _) in &section.props {
        if let Some(v) = section.get(key) {
            set_prop(&mut b, key, v);
        }
    }
    if let Some(label) = section.get("label") {
        b.full_text = format!("{label}{}", b.full_text);
    }
    b.name = Some(section.name.clone());
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sec(name: &str, kv: &[(&str, &str)]) -> Section {
        Section {
            name: name.into(),
            props: kv
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn expand_sh_rules() {
        let env = vec![
            ("DISK".to_string(), "C:".to_string()),
            ("SCRIPT_DIR".to_string(), r"C:\s".to_string()),
        ];
        assert_eq!(
            expand_sh_vars("$SCRIPT_DIR\\disk.bat $DISK ${DISK}x $NOPE.", &env, 7),
            r"C:\s\disk.bat C: C:x ."
        );
        assert_eq!(
            expand_sh_vars("echo '$DISK' \\$DISK $$", &env, 7),
            "echo $DISK $DISK 7"
        );
        assert_eq!(expand_sh_vars("echo \"$DISK\"", &env, 7), "echo \"C:\"");
        assert_eq!(expand_sh_vars("a $ b ${DISK", &env, 7), "a $ b ${DISK");
    }

    #[test]
    fn env_has_props_and_click() {
        let s = sec(
            "disk",
            &[("interval", "5"), ("DISK", "C:"), ("instance", "c")],
        );
        let click = Click {
            name: Some("disk".into()),
            instance: Some("c".into()),
            button: 3,
            modifiers: vec!["Shift".into()],
            x: 1,
            y: 2,
            relative_x: 3,
            relative_y: 4,
            output_x: 0,
            output_y: 0,
            width: 5,
            height: 6,
        };
        let e = block_env(&s, Path::new(r"C:\cfg"), Some(&click));
        let get = |k: &str| e.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str());
        assert_eq!(get("DISK"), Some("C:"));
        assert_eq!(get("SCRIPT_DIR"), Some(r"C:\cfg"));
        assert_eq!(get("BLOCK_NAME"), Some("disk"));
        assert_eq!(get("BLOCK_INSTANCE"), Some("c"));
        assert_eq!(get("BLOCK_INTERVAL"), Some("5"));
        assert_eq!(get("button"), Some("3"));
        assert_eq!(get("BLOCK_BUTTON"), Some("3"));
        assert_eq!(get("relative_x"), Some("3"));
        assert_eq!(get("modifiers"), Some("Shift"));
        assert_eq!(e.iter().filter(|(k, _)| k == "instance").count(), 1);
    }

    #[test]
    fn own_prop_overrides_global_in_env() {
        let s = sec("a", &[("K", "global"), ("K", "own")]);
        let e = block_env(&s, Path::new("."), None);
        assert_eq!(e.iter().filter(|(k, _)| k == "K").count(), 1);
        assert!(e.contains(&("K".to_string(), "own".to_string())));
        assert!(!e.iter().any(|(k, _)| k == "button"));
    }

    #[test]
    fn shells() {
        assert_eq!(
            shell_command("cmd", "echo hi"),
            (
                "cmd".into(),
                vec!["/S".into(), "/C".into(), "\"echo hi\"".into()],
                true
            )
        );
        assert_eq!(
            shell_command("bash", "echo $X"),
            ("bash".into(), vec!["-c".into(), "echo $X".into()], false)
        );
        assert_eq!(shell_command("pwsh", "Get-Date").1[1], "-Command");
    }

    #[test]
    fn cmd_shell_aliases() {
        for sh in ["", "CMD", "cmd.exe", "Cmd.EXE"] {
            let (prog, args, raw) = shell_command(sh, "echo hi");
            assert_eq!(prog, "cmd", "{sh}");
            assert_eq!(args, vec!["/S", "/C", "\"echo hi\""], "{sh}");
            assert!(raw, "{sh}");
        }
        assert!(is_cmd_shell(" cmd "));
        assert!(!is_cmd_shell("bash"));
    }

    #[test]
    fn raw_and_json_output() {
        let mut b = static_block(&sec("w", &[]));
        apply_raw_output(&mut b, "full\nshort\n#ff0000\n#000000\n");
        assert_eq!(
            (
                b.full_text.as_str(),
                b.short_text.as_deref(),
                b.color.as_deref(),
                b.background.as_deref()
            ),
            ("full", Some("short"), Some("#ff0000"), Some("#000000"))
        );
        assert!(apply_json_line(
            &mut b,
            r#"{"full_text":"j","urgent":true}"#
        ));
        assert_eq!(b.full_text, "j");
        assert!(b.urgent);
        assert!(!apply_json_line(&mut b, "not json"));
        assert!(!apply_json_line(&mut b, "[1]"));
    }

    #[test]
    fn raw_full_line_order_and_unset() {
        let mut b = Block::new("");
        let out =
            "t\ns\n#111\n#222\n#333\n40.6\ncenter\nnm\ninst\ntrue\nfalse\n7\npango\n1\n2\n3\n4\n";
        apply_raw_output(&mut b, out);
        assert_eq!(b.min_width, Some(MinWidth::Px(41)));
        assert_eq!(b.align.as_deref(), Some("center"));
        assert_eq!(b.name.as_deref(), Some("nm"));
        assert_eq!(b.instance.as_deref(), Some("inst"));
        assert!(b.urgent && !b.separator);
        assert_eq!(b.separator_block_width, Some(7));
        assert_eq!(b.markup.as_deref(), Some("pango"));
        assert_eq!(
            (b.border_top, b.border_bottom, b.border_left, b.border_right),
            (Some(1), Some(2), Some(3), Some(4))
        );
        apply_raw_output(&mut b, "t2\n\n\n\n\nwide text\n\n\n\n\n\nx\n");
        assert_eq!(b.short_text, None);
        assert_eq!(b.color, None);
        assert_eq!(b.min_width, Some(MinWidth::Text("wide text".into())));
        assert!(!b.urgent && b.separator);
        assert_eq!(b.separator_block_width, None);
    }

    #[test]
    fn json_lenient_numbers() {
        let mut b = Block::new("x");
        assert!(apply_json_line(
            &mut b,
            r#"{"min_width":12.4,"border_top":"junk","separator":false,"separator_block_width":3.5}"#
        ));
        assert_eq!(b.min_width, Some(MinWidth::Px(12)));
        assert_eq!(b.border_top, None);
        assert!(!b.separator);
        assert_eq!(b.separator_block_width, Some(4));
        assert!(apply_json_line(&mut b, r#"{"min_width":"100%"}"#));
        assert_eq!(b.min_width, Some(MinWidth::Text("100%".into())));
    }

    #[test]
    fn exit_codes_match_i3blocks() {
        let mut b = static_block(&sec("w", &[("full_text", "x")]));
        exit_code_block(&mut b, "foo.sh", 0);
        assert!(!b.urgent);
        exit_code_block(&mut b, "foo.sh", 33);
        assert!(b.urgent && b.full_text == "x");
        exit_code_block(&mut b, "foo.sh", 2);
        assert_eq!(
            b.full_text,
            "Error: Command 'foo.sh' exited unexpectedly with code 2"
        );
        exit_code_block(&mut b, "foo.sh", 127);
        assert_eq!(
            b.full_text,
            "Error: Command 'foo.sh' not found or missing dependency"
        );
        exit_code_block(&mut b, "foo.sh", 126);
        assert_eq!(b.full_text, "Error: Command 'foo.sh' not executable");
    }

    #[test]
    fn label_and_interval() {
        assert_eq!(
            static_block(&sec("a", &[("label", "L: "), ("full_text", "x")])).full_text,
            "L: x"
        );
        assert_eq!(parse_interval(Some("persist")), Interval::Persist);
        assert_eq!(parse_interval(Some("0")), Interval::None);
        assert_eq!(parse_interval(None), Interval::None);
        assert_eq!(parse_interval(Some("2.5")), Interval::Secs(2.5));
        assert_eq!(parse_interval(Some("once")), Interval::Once);
        assert_eq!(parse_interval(Some("repeat")), Interval::Repeat);
        let s = static_block(&sec("a", &[("color", "#fff")]));
        assert!(s.separator && s.name.as_deref() == Some("a"));
    }
}
