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
pub enum BorderArg {
    Normal(Option<i32>),
    Pixel(Option<i32>),
    None,
    Toggle(Option<i32>),
}
#[derive(Clone, Debug, PartialEq)]
pub enum TitleIconArg {
    On,
    Off,
    Toggle,
    Padding(i32),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GapsWhich {
    Inner,
    Outer,
    Horizontal,
    Vertical,
    Top,
    Right,
    Bottom,
    Left,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GapsScope {
    Current,
    All,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GapsOp {
    Set,
    Plus,
    Minus,
    Toggle,
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
    Workspace {
        target: WorkspaceTarget,
        no_auto_back_and_forth: bool,
    },
    RenameWorkspace {
        old: Option<String>,
        new: String,
    },
    Split(SplitArg),
    Layout(LayoutArg),
    Fullscreen(Toggle),
    Floating(Toggle),
    Sticky(Toggle),
    Kill,
    /// Width = Horizontal.
    Resize {
        grow: bool,
        axis: Orientation,
        px: Option<i32>,
        ppt: Option<i32>,
    },
    Border(BorderArg),
    TitleFormat(String),
    TitleWindowIcon(TitleIconArg),
    Gaps {
        which: GapsWhich,
        scope: GapsScope,
        op: GapsOp,
        px: i32,
    },
    Mode(String),
    Reload,
    Restart,
    Exit,
    Nop,
    Mark {
        add: bool,
        toggle: bool,
        name: String,
    },
    Unmark(Option<String>),
    MoveToMark(String),
    MoveScratchpad,
    ScratchpadShow,
    Swap(SwapTarget),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwapTarget {
    Mark(String),
    ConId(u32),
    Id(isize),
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
    if name.starts_with("__") {
        return Err("Workspace names starting with __ are reserved".into());
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
    if a(i) == Some("scratchpad") && a(i + 1).is_none() {
        return Ok(Command::MoveScratchpad);
    }
    if a(i) == Some("mark") {
        let name = a(i + 1).ok_or("move to mark: missing name")?;
        return Ok(Command::MoveToMark(name.to_string()));
    }
    if a(i) != Some("workspace") {
        return Err(format!("bad move argument: {}", join(args)));
    }
    Ok(Command::MoveToWorkspace(parse_workspace_target(
        &args[i + 1..],
    )?))
}

fn parse_mark(args: &[Tok]) -> Result<Command, String> {
    let (mut add, mut toggle) = (false, false);
    let mut name = None;
    for (t, _) in args {
        match t.as_str() {
            "--add" => add = true,
            "--replace" => add = false,
            "--toggle" => toggle = true,
            _ if name.is_none() => name = Some(t.clone()),
            _ => return Err(format!("bad mark argument: {}", join(args))),
        }
    }
    let name = name.ok_or("mark: missing name")?;
    Ok(Command::Mark { add, toggle, name })
}

fn parse_swap(args: &[Tok]) -> Result<Command, String> {
    let a = |i: usize| args.get(i).map(|t| t.0.as_str());
    if a(0) != Some("container") || a(1) != Some("with") {
        return Err(format!("bad swap argument: {}", join(args)));
    }
    let v = a(3).ok_or("swap: missing value")?;
    match a(2) {
        Some("mark") => Ok(Command::Swap(SwapTarget::Mark(v.to_string()))),
        Some("con_id") => Ok(Command::Swap(SwapTarget::ConId(
            v.parse().map_err(|_| format!("bad con_id: {v}"))?,
        ))),
        Some("id") => Ok(Command::Swap(SwapTarget::Id(
            v.parse().map_err(|_| format!("bad id: {v}"))?,
        ))),
        o => Err(format!("bad swap target: {}", o.unwrap_or(""))),
    }
}

fn parse_gaps(args: &[Tok]) -> Result<Command, String> {
    let a = |i: usize| args.get(i).map(|t| t.0.as_str());
    let which = match a(0) {
        Some("inner") => GapsWhich::Inner,
        Some("outer") => GapsWhich::Outer,
        Some("horizontal") => GapsWhich::Horizontal,
        Some("vertical") => GapsWhich::Vertical,
        Some("top") => GapsWhich::Top,
        Some("right") => GapsWhich::Right,
        Some("bottom") => GapsWhich::Bottom,
        Some("left") => GapsWhich::Left,
        o => return Err(format!("bad gaps target: {}", o.unwrap_or(""))),
    };
    let scope = match a(1) {
        Some("current") => GapsScope::Current,
        Some("all") => GapsScope::All,
        o => return Err(format!("bad gaps scope: {}", o.unwrap_or(""))),
    };
    let op = match a(2) {
        Some("set") => GapsOp::Set,
        Some("plus") => GapsOp::Plus,
        Some("minus") => GapsOp::Minus,
        Some("toggle") => GapsOp::Toggle,
        o => return Err(format!("bad gaps operation: {}", o.unwrap_or(""))),
    };
    let v = a(3).ok_or("gaps: missing amount")?;
    let px = parse_int(v.strip_suffix("px").unwrap_or(v))?;
    Ok(Command::Gaps {
        which,
        scope,
        op,
        px,
    })
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
            let no_auto_back_and_forth = a(0) == Some("--no-auto-back-and-forth");
            let skip = usize::from(no_auto_back_and_forth);
            Ok(Command::Workspace {
                target: parse_workspace_target(&args[skip..])?,
                no_auto_back_and_forth,
            })
        }
        "rename" => {
            if a(0) != Some("workspace") {
                return Err(format!("bad rename argument: {}", a(0).unwrap_or("")));
            }
            let to = args
                .iter()
                .enumerate()
                .skip(1)
                .find(|(_, t)| !t.1 && t.0 == "to")
                .map(|(i, _)| i)
                .ok_or("rename workspace: missing 'to'")?;
            let old = (to > 1).then(|| join(&args[1..to]));
            let new = join(&args[to + 1..]);
            if new.is_empty() {
                return Err("rename workspace: missing new name".into());
            }
            if new.starts_with("__") {
                return Err("Workspace names starting with __ are reserved".into());
            }
            Ok(Command::RenameWorkspace { old, new })
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
        "sticky" => Ok(Command::Sticky(toggle_arg(args, "sticky")?)),
        "kill" => Ok(Command::Kill),
        "border" => {
            let n = match a(1) {
                Some(v) => Some(parse_int(v.strip_suffix("px").unwrap_or(v))?),
                None => None,
            };
            match a(0) {
                Some("normal") => Ok(Command::Border(BorderArg::Normal(n))),
                Some("pixel") => Ok(Command::Border(BorderArg::Pixel(n))),
                Some("none") => Ok(Command::Border(BorderArg::None)),
                Some("toggle") => Ok(Command::Border(BorderArg::Toggle(n))),
                o => Err(format!("bad border argument: {}", o.unwrap_or(""))),
            }
        }
        "title_format" => {
            if args.is_empty() {
                return Err("title_format: missing format".into());
            }
            Ok(Command::TitleFormat(join(args)))
        }
        "title_window_icon" => match a(0) {
            Some("on" | "yes" | "enable" | "true") => {
                Ok(Command::TitleWindowIcon(TitleIconArg::On))
            }
            Some("off" | "no" | "disable" | "false") => {
                Ok(Command::TitleWindowIcon(TitleIconArg::Off))
            }
            Some("toggle") => Ok(Command::TitleWindowIcon(TitleIconArg::Toggle)),
            Some("padding") => {
                let v = a(1).ok_or("title_window_icon padding: missing value")?;
                let n = parse_int(v.strip_suffix("px").unwrap_or(v))?;
                Ok(Command::TitleWindowIcon(TitleIconArg::Padding(n)))
            }
            o => Err(format!(
                "bad title_window_icon argument: {}",
                o.unwrap_or("")
            )),
        },
        "resize" => parse_resize(args),
        "gaps" => parse_gaps(args),
        "mode" => match args.first() {
            Some(t) if t.0.is_empty() => Err("mode: missing name".into()),
            Some(t) => Ok(Command::Mode(t.0.clone())),
            None => Err("mode: missing name".into()),
        },
        "reload" => Ok(Command::Reload),
        "restart" => Ok(Command::Restart),
        "exit" => Ok(Command::Exit),
        "mark" => parse_mark(args),
        "unmark" => Ok(Command::Unmark(a(0).map(str::to_string))),
        "swap" => parse_swap(args),
        "nop" => Ok(Command::Nop),
        "scratchpad" if a(0) == Some("show") && a(1).is_none() => Ok(Command::ScratchpadShow),
        _ => Err(format!("unknown command: {verb}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(target: WorkspaceTarget) -> Command {
        Command::Workspace {
            target,
            no_auto_back_and_forth: false,
        }
    }
    #[test]
    fn rename_and_no_auto_flag_parse() {
        assert_eq!(
            one("rename workspace to \"5: mail\""),
            Command::RenameWorkspace {
                old: None,
                new: "5: mail".into()
            }
        );
        assert_eq!(
            one("rename workspace \"a b\" to c"),
            Command::RenameWorkspace {
                old: Some("a b".into()),
                new: "c".into()
            }
        );
        assert!(parse_commands("rename workspace to __x").is_err());
        assert_eq!(
            one("workspace --no-auto-back-and-forth 1"),
            Command::Workspace {
                target: WorkspaceTarget::Name("1".into()),
                no_auto_back_and_forth: true
            }
        );
    }
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
                ws(WorkspaceTarget::Name(name.into()))
            );
        }
    }
    #[test]
    fn single_backslash_before_other_char_stays_literal() {
        assert_eq!(
            one(r#"workspace "C:\Program Files""#),
            ws(WorkspaceTarget::Name(r"C:\Program Files".into()))
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
            ws(WorkspaceTarget::Name("3:www".into()))
        );
        assert_eq!(
            one("workspace number 3:www"),
            ws(WorkspaceTarget::Number(3))
        );
        assert_eq!(
            one(r#"workspace "my ws""#),
            ws(WorkspaceTarget::Name("my ws".into()))
        );
        assert_eq!(
            one("workspace back_and_forth"),
            ws(WorkspaceTarget::BackAndForth)
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
