use crate::criteria::{parse_criteria, Criteria};
use crate::geom::{Direction, Orientation};
use crate::tree::Layout;

#[derive(Clone, Debug, PartialEq)]
pub enum Toggle {
    Toggle,
    Enable,
    Disable,
}
#[derive(Clone, Debug, PartialEq)]
pub enum WorkspaceTarget {
    Name(String),
    Number(i32),
    Next,
    Prev,
    BackAndForth,
}
#[derive(Clone, Debug, PartialEq)]
pub enum OutputTarget {
    Dir(Direction),
    Name(String),
}
#[derive(Clone, Debug, PartialEq)]
pub enum SplitArg {
    Horizontal,
    Vertical,
    Toggle,
}
#[derive(Clone, Debug, PartialEq)]
pub enum LayoutArg {
    Set(Layout),
    Default,
    ToggleSplit,
    ToggleAll,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Exec(String),
    Focus(Direction),
    FocusParent,
    FocusChild,
    FocusModeToggle,
    FocusFloating,
    FocusTiling,
    FocusMatched,
    FocusOutput(OutputTarget),
    /// Optional "N px" (floating only).
    Move(Direction, Option<i32>),
    MoveToWorkspace(WorkspaceTarget),
    MoveWorkspaceToOutput(OutputTarget),
    Workspace(WorkspaceTarget),
    Split(SplitArg),
    Layout(LayoutArg),
    Fullscreen(Toggle),
    Floating(Toggle),
    Kill,
    /// Width = Horizontal.
    Resize {
        grow: bool,
        axis: Orientation,
        px: Option<i32>,
        ppt: Option<i32>,
    },
    Mode(String),
    Reload,
    Restart,
    Exit,
    Nop,
}
#[derive(Clone, Debug)]
pub struct CommandGroup {
    pub criteria: Option<Criteria>,
    pub commands: Vec<Command>,
}

type Tok = (String, bool);

/// Splits on `sep` outside of double quotes (`\"` and `\` escape inside quotes).
fn split_unquoted(s: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_quote = false;
    let mut chars = s.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        if in_quote && ch == '\\' && matches!(chars.peek(), Some(&(_, '"' | '\\'))) {
            chars.next();
        } else if ch == '"' {
            in_quote = !in_quote;
        } else if ch == sep && !in_quote {
            parts.push(&s[start..i]);
            start = i + ch.len_utf8();
        }
    }
    parts.push(&s[start..]);
    parts
}

/// Whitespace-separated tokens as `(text, was_quoted)`.
fn tokenize(s: &str) -> Result<Vec<Tok>, String> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
        let mut text = String::new();
        let mut quoted = false;
        while let Some(&ch) = chars.peek() {
            if ch.is_whitespace() {
                break;
            }
            chars.next();
            if ch == '"' {
                quoted = true;
                let mut closed = false;
                while let Some(c) = chars.next() {
                    match c {
                        '\\' if matches!(chars.peek(), Some('"' | '\\')) => {
                            if let Some(e) = chars.next() {
                                text.push(e);
                            }
                        }
                        '"' => {
                            closed = true;
                            break;
                        }
                        _ => text.push(c),
                    }
                }
                if !closed {
                    return Err("unterminated quote".into());
                }
            } else {
                text.push(ch);
            }
        }
        out.push((text, quoted));
    }
    Ok(out)
}

pub fn parse_commands(input: &str) -> Result<Vec<CommandGroup>, String> {
    let mut groups = Vec::new();
    for raw in split_unquoted(input, ';') {
        let mut rest = raw.trim();
        if rest.is_empty() {
            continue;
        }
        let mut criteria = None;
        if let Some(after) = rest.strip_prefix('[') {
            let mut in_quote = false;
            let mut end = None;
            let mut chars = after.char_indices().peekable();
            while let Some((i, ch)) = chars.next() {
                if in_quote && ch == '\\' && matches!(chars.peek(), Some(&(_, '"' | '\\'))) {
                    chars.next();
                } else if ch == '"' {
                    in_quote = !in_quote;
                } else if ch == ']' && !in_quote {
                    end = Some(i);
                    break;
                }
            }
            let end = end.ok_or("unterminated '[' in criteria")?;
            criteria = Some(parse_criteria(&after[..end])?);
            rest = after[end + 1..].trim();
        }
        let mut commands = Vec::new();
        for seg in split_unquoted(rest, ',') {
            let seg = seg.trim();
            if seg.is_empty() {
                continue;
            }
            commands.push(parse_one(seg)?);
        }
        if commands.is_empty() {
            continue;
        }
        groups.push(CommandGroup { criteria, commands });
    }
    Ok(groups)
}

fn dir_of(s: &str) -> Option<Direction> {
    match s {
        "left" => Some(Direction::Left),
        "right" => Some(Direction::Right),
        "up" => Some(Direction::Up),
        "down" => Some(Direction::Down),
        _ => None,
    }
}

fn output_target(tok: &Tok) -> OutputTarget {
    match dir_of(&tok.0) {
        Some(d) if !tok.1 => OutputTarget::Dir(d),
        _ => OutputTarget::Name(tok.0.clone()),
    }
}

fn toggle_arg(args: &[Tok], verb: &str) -> Result<Toggle, String> {
    match args.first().map(|t| t.0.as_str()) {
        None | Some("toggle") => Ok(Toggle::Toggle),
        Some("enable") => Ok(Toggle::Enable),
        Some("disable") => Ok(Toggle::Disable),
        Some(o) => Err(format!("bad argument to {verb}: {o}")),
    }
}

fn join(args: &[Tok]) -> String {
    args.iter()
        .map(|t| t.0.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_workspace_target(args: &[Tok]) -> Result<WorkspaceTarget, String> {
    let first = args.first().ok_or("missing workspace")?;
    if !first.1 {
        match first.0.as_str() {
            "next" if args.len() == 1 => return Ok(WorkspaceTarget::Next),
            "prev" if args.len() == 1 => return Ok(WorkspaceTarget::Prev),
            "back_and_forth" if args.len() == 1 => return Ok(WorkspaceTarget::BackAndForth),
            "number" => {
                let joined = join(&args[1..]);
                let digits: String = joined.chars().take_while(|c| c.is_ascii_digit()).collect();
                if digits.is_empty() {
                    return Err(format!("workspace number needs digits: '{joined}'"));
                }
                let n = digits
                    .parse::<i32>()
                    .map_err(|_| format!("workspace number out of range: {digits}"))?;
                return Ok(WorkspaceTarget::Number(n));
            }
            _ => {}
        }
    }
    let name = join(args);
    if name.is_empty() {
        return Err("missing workspace".into());
    }
    Ok(WorkspaceTarget::Name(name))
}

fn parse_int(s: &str) -> Result<i32, String> {
    s.parse::<i32>().map_err(|_| format!("bad number: {s}"))
}

fn parse_resize(args: &[Tok]) -> Result<Command, String> {
    let a = |i: usize| args.get(i).map(|t| t.0.as_str());
    let grow = match a(0) {
        Some("grow") => true,
        Some("shrink") => false,
        Some("set") => return Err("resize set: unsupported".into()),
        o => return Err(format!("bad resize argument: {}", o.unwrap_or(""))),
    };
    let axis = match a(1) {
        Some("width") => Orientation::Horizontal,
        Some("height") => Orientation::Vertical,
        o => return Err(format!("bad resize axis: {}", o.unwrap_or(""))),
    };
    let (mut px, mut ppt) = (None, None);
    let mut i = 2;
    while let Some(tok) = a(i) {
        match tok {
            "or" | "px" | "ppt" => i += 1,
            n => {
                let v = parse_int(n)?;
                match a(i + 1) {
                    Some("ppt") => {
                        ppt = Some(v);
                        i += 2;
                    }
                    Some("px") => {
                        px = Some(v);
                        i += 2;
                    }
                    _ => {
                        px = Some(v);
                        i += 1;
                    }
                }
            }
        }
    }
    if px.is_none() && ppt.is_none() {
        px = Some(10);
        ppt = Some(10);
    }
    Ok(Command::Resize {
        grow,
        axis,
        px,
        ppt,
    })
}

fn parse_move(args: &[Tok]) -> Result<Command, String> {
    let a = |i: usize| args.get(i).map(|t| t.0.as_str());
    if let Some(d) = a(0).and_then(dir_of) {
        let px = match a(1) {
            Some(n) => Some(parse_int(n)?),
            None => None,
        };
        return Ok(Command::Move(d, px));
    }
    let mut i = 0;
    if matches!(a(i), Some("container" | "window")) {
        i += 1;
    }
    if a(i) == Some("workspace") && a(i + 1) == Some("to") {
        i += 2;
        if a(i) == Some("output") {
            i += 1;
        }
        let t = args.get(i).ok_or("move workspace to: missing output")?;
        return Ok(Command::MoveWorkspaceToOutput(output_target(t)));
    }
    if a(i) == Some("to") {
        i += 1;
    }
    if a(i) != Some("workspace") {
        return Err(format!("bad move argument: {}", join(args)));
    }
    Ok(Command::MoveToWorkspace(parse_workspace_target(
        &args[i + 1..],
    )?))
}

fn parse_one(seg: &str) -> Result<Command, String> {
    let toks = tokenize(seg)?;
    let Some(first) = toks.first() else {
        return Err("empty command".into());
    };
    let verb = first.0.as_str();
    let args = &toks[1..];
    let a = |i: usize| args.get(i).map(|t| t.0.as_str());
    match verb {
        "exec" => {
            let mut rest = seg.trim_start().get(4..).unwrap_or("").trim();
            if let Some(r) = rest.strip_prefix("--no-startup-id") {
                if r.is_empty() || r.starts_with(char::is_whitespace) {
                    rest = r.trim();
                }
            }
            if rest.is_empty() {
                return Err("exec: missing command".into());
            }
            if let Ok(t) = tokenize(rest) {
                if t.len() == 1 && t[0].1 {
                    if t[0].0.is_empty() {
                        return Err("exec: missing command".into());
                    }
                    return Ok(Command::Exec(t[0].0.clone()));
                }
            }
            Ok(Command::Exec(rest.to_string()))
        }
        "focus" => match a(0) {
            None => Ok(Command::FocusMatched),
            Some("parent") => Ok(Command::FocusParent),
            Some("child") => Ok(Command::FocusChild),
            Some("mode_toggle") => Ok(Command::FocusModeToggle),
            Some("floating") => Ok(Command::FocusFloating),
            Some("tiling") => Ok(Command::FocusTiling),
            Some("output") => {
                let t = args.get(1).ok_or("focus output: missing target")?;
                Ok(Command::FocusOutput(output_target(t)))
            }
            Some(d) => dir_of(d)
                .map(Command::Focus)
                .ok_or_else(|| format!("bad focus argument: {d}")),
        },
        "move" => parse_move(args),
        "workspace" => {
            let skip = usize::from(a(0) == Some("--no-auto-back-and-forth"));
            Ok(Command::Workspace(parse_workspace_target(&args[skip..])?))
        }
        "split" => match a(0) {
            Some("h" | "horizontal") => Ok(Command::Split(SplitArg::Horizontal)),
            Some("v" | "vertical") => Ok(Command::Split(SplitArg::Vertical)),
            Some("t" | "toggle") => Ok(Command::Split(SplitArg::Toggle)),
            o => Err(format!("bad split argument: {}", o.unwrap_or(""))),
        },
        "layout" => match (a(0), a(1)) {
            (Some("stacking" | "stacked"), _) => {
                Ok(Command::Layout(LayoutArg::Set(Layout::Stacked)))
            }
            (Some("tabbed"), _) => Ok(Command::Layout(LayoutArg::Set(Layout::Tabbed))),
            (Some("splith"), _) => Ok(Command::Layout(LayoutArg::Set(Layout::SplitH))),
            (Some("splitv"), _) => Ok(Command::Layout(LayoutArg::Set(Layout::SplitV))),
            (Some("default"), _) => Ok(Command::Layout(LayoutArg::Default)),
            (Some("toggle"), Some("split")) => Ok(Command::Layout(LayoutArg::ToggleSplit)),
            (Some("toggle"), None | Some("all")) => Ok(Command::Layout(LayoutArg::ToggleAll)),
            _ => Err(format!("bad layout argument: {}", join(args))),
        },
        "fullscreen" => Ok(Command::Fullscreen(toggle_arg(args, "fullscreen")?)),
        "floating" => Ok(Command::Floating(toggle_arg(args, "floating")?)),
        "kill" => Ok(Command::Kill),
        "resize" => parse_resize(args),
        "mode" => match args.first() {
            Some(t) if t.0.is_empty() => Err("mode: missing name".into()),
            Some(t) => Ok(Command::Mode(t.0.clone())),
            None => Err("mode: missing name".into()),
        },
        "reload" => Ok(Command::Reload),
        "restart" => Ok(Command::Restart),
        "exit" => Ok(Command::Exit),
        "nop" => Ok(Command::Nop),
        _ => Err(format!("unknown command: {verb}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(s: &str) -> Command {
        let g = parse_commands(s).unwrap();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].commands.len(), 1);
        g[0].commands[0].clone()
    }
    #[test]
    fn exec_keeps_raw_rest() {
        assert_eq!(
            one("exec --no-startup-id wt -d C:\\"),
            Command::Exec("wt -d C:\\".into())
        );
        assert_eq!(
            one(r#"exec "notepad a.txt""#),
            Command::Exec("notepad a.txt".into())
        );
    }
    #[test]
    fn exec_quoted_path_ending_in_backslash() {
        assert_eq!(one(r#"exec "C:\dir\\""#), Command::Exec(r"C:\dir\".into()));
    }
    #[test]
    fn escaped_quoted_names_round_trip() {
        for name in [r#"my "ws""#, r"a\b", r"x\", r#"q\"z"#, "plain"] {
            let escaped = name.replace('\\', r"\\").replace('"', r#"\""#);
            assert_eq!(
                one(&format!("workspace \"{escaped}\"")),
                Command::Workspace(WorkspaceTarget::Name(name.into()))
            );
        }
    }
    #[test]
    fn single_backslash_before_other_char_stays_literal() {
        assert_eq!(
            one(r#"workspace "C:\Program Files""#),
            Command::Workspace(WorkspaceTarget::Name(r"C:\Program Files".into()))
        );
    }
    #[test]
    fn focus_variants() {
        assert_eq!(one("focus left"), Command::Focus(Direction::Left));
        assert_eq!(one("focus"), Command::FocusMatched);
        assert_eq!(
            one("focus output DISPLAY2"),
            Command::FocusOutput(OutputTarget::Name("DISPLAY2".into()))
        );
    }
    #[test]
    fn workspace_variants() {
        assert_eq!(
            one("workspace 3:www"),
            Command::Workspace(WorkspaceTarget::Name("3:www".into()))
        );
        assert_eq!(
            one("workspace number 3:www"),
            Command::Workspace(WorkspaceTarget::Number(3))
        );
        assert_eq!(
            one(r#"workspace "my ws""#),
            Command::Workspace(WorkspaceTarget::Name("my ws".into()))
        );
        assert_eq!(
            one("workspace back_and_forth"),
            Command::Workspace(WorkspaceTarget::BackAndForth)
        );
        assert_eq!(
            one("move container to workspace number 4"),
            Command::MoveToWorkspace(WorkspaceTarget::Number(4))
        );
        assert_eq!(
            one("move workspace to output right"),
            Command::MoveWorkspaceToOutput(OutputTarget::Dir(Direction::Right))
        );
    }
    #[test]
    fn move_with_px() {
        assert_eq!(
            one("move left 50 px"),
            Command::Move(Direction::Left, Some(50))
        );
        assert_eq!(one("move up"), Command::Move(Direction::Up, None));
    }
    #[test]
    fn layout_and_split() {
        assert_eq!(
            one("layout toggle split"),
            Command::Layout(LayoutArg::ToggleSplit)
        );
        assert_eq!(
            one("layout stacking"),
            Command::Layout(LayoutArg::Set(Layout::Stacked))
        );
        assert_eq!(one("split v"), Command::Split(SplitArg::Vertical));
    }
    #[test]
    fn resize_forms() {
        assert_eq!(
            one("resize grow width 10 px or 10 ppt"),
            Command::Resize {
                grow: true,
                axis: Orientation::Horizontal,
                px: Some(10),
                ppt: Some(10)
            }
        );
        assert_eq!(
            one("resize shrink height 5 ppt"),
            Command::Resize {
                grow: false,
                axis: Orientation::Vertical,
                px: None,
                ppt: Some(5)
            }
        );
        assert_eq!(
            one("resize grow width"),
            Command::Resize {
                grow: true,
                axis: Orientation::Horizontal,
                px: Some(10),
                ppt: Some(10)
            }
        );
    }
    #[test]
    fn chaining_and_criteria() {
        let g =
            parse_commands(r#"[class="firefox"] floating enable, move left; workspace 2"#).unwrap();
        assert_eq!(g.len(), 2);
        assert!(g[0].criteria.is_some());
        assert_eq!(
            g[0].commands,
            vec![
                Command::Floating(Toggle::Enable),
                Command::Move(Direction::Left, None)
            ]
        );
        assert!(g[1].criteria.is_none());
    }
    #[test]
    fn semicolon_inside_quotes_is_not_a_separator() {
        assert_eq!(one(r#"mode "a;b""#), Command::Mode("a;b".into()));
    }
    #[test]
    fn errors() {
        assert!(parse_commands("frobnicate").is_err());
        assert!(parse_commands("exec").is_err());
        assert!(parse_commands("exec \"\"").is_err());
        assert!(parse_commands("workspace \"\"").is_err());
        assert!(parse_commands("mode \"\"").is_err());
        assert!(parse_commands("workspace number abc").is_err());
    }
}
