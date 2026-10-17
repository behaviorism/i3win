use crate::command::WorkspaceTarget;
use crate::criteria::{parse_criteria, Criteria};
use crate::keycodes::x11_keycode_to_scancode;
use crate::keys::{parse_combo, scancode_combo, split_combo, KeyCombo, Modifiers};
use crate::tree::Layout;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u8, pub u8, pub u8);

/// Parses `#rrggbb` or `#rrggbbaa` (alpha ignored).
pub fn parse_color(s: &str) -> Option<Color> {
    let h = s.strip_prefix('#')?;
    if !h.is_ascii() || (h.len() != 6 && h.len() != 8) {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some(Color(p(0)?, p(2)?, p(4)?))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorClass {
    pub border: Color,
    pub background: Color,
    pub text: Color,
    pub indicator: Color,
    pub child_border: Color,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClientColors {
    pub focused: ColorClass,
    pub focused_inactive: ColorClass,
    pub unfocused: ColorClass,
    pub urgent: ColorClass,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BarColorTriple {
    pub border: Color,
    pub background: Color,
    pub text: Color,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BarColors {
    pub background: Color,
    pub statusline: Color,
    pub separator: Color,
    pub focused_workspace: BarColorTriple,
    pub active_workspace: BarColorTriple,
    pub inactive_workspace: BarColorTriple,
    pub urgent_workspace: BarColorTriple,
    pub binding_mode: BarColorTriple,
    pub focused_background: Option<Color>,
    pub focused_statusline: Option<Color>,
    pub focused_separator: Option<Color>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarPosition {
    Top,
    Bottom,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Font {
    pub family: String,
    pub size_pt: f32,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BarMode {
    #[default]
    Dock,
    Hide,
    Invisible,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HiddenState {
    #[default]
    Hide,
    Show,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarBinding {
    pub button: u8,
    pub release: bool,
    pub command: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct BarConfig {
    pub id: String,
    pub outputs: Vec<String>,
    pub mode: BarMode,
    pub position: BarPosition,
    pub status_command: Option<String>,
    pub font: Option<Font>,
    pub height: Option<i32>,
    pub workspace_buttons: bool,
    pub colors: BarColors,
    pub hidden_state: HiddenState,
    /// Key that reveals a hidden bar; `None` for `modifier none`.
    pub modifier: Option<Modifiers>,
    pub separator_symbol: Option<String>,
    pub strip_workspace_numbers: bool,
    pub strip_workspace_name: bool,
    pub binding_mode_indicator: bool,
    /// Minimum workspace button width in px.
    pub workspace_min_width: i32,
    /// top, right, bottom, left in px.
    pub padding: [i32; 4],
    pub bindings: Vec<BarBinding>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Border {
    Normal(i32),
    Pixel(i32),
    None,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    pub combo: KeyCombo,
    pub command: String,
    pub release: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Gaps {
    pub inner: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct GapsOverride {
    pub inner: Option<i32>,
    pub top: Option<i32>,
    pub right: Option<i32>,
    pub bottom: Option<i32>,
    pub left: Option<i32>,
}
impl Gaps {
    /// Each `Some` field of `o` replaces the corresponding field.
    pub fn with(self, o: &GapsOverride) -> Gaps {
        Gaps {
            inner: o.inner.unwrap_or(self.inner),
            top: o.top.unwrap_or(self.top),
            right: o.right.unwrap_or(self.right),
            bottom: o.bottom.unwrap_or(self.bottom),
            left: o.left.unwrap_or(self.left),
        }
    }
}
impl GapsOverride {
    /// `o`'s `Some` fields take precedence over `self`'s.
    pub fn merge(&self, o: &GapsOverride) -> GapsOverride {
        GapsOverride {
            inner: o.inner.or(self.inner),
            top: o.top.or(self.top),
            right: o.right.or(self.right),
            bottom: o.bottom.or(self.bottom),
            left: o.left.or(self.left),
        }
    }
}

/// Parses `inner|outer|horizontal|vertical|top|right|bottom|left N`.
fn parse_gaps_override(toks: &[&str]) -> Result<GapsOverride, String> {
    let [which, n] = toks else {
        return Err(format!("unsupported gaps form: {}", toks.join(" ")));
    };
    let n: i32 = n
        .parse()
        .map_err(|_| format!("gaps: invalid value '{n}'"))?;
    let mut o = GapsOverride::default();
    match *which {
        "inner" => o.inner = Some(n),
        "outer" => {
            o.top = Some(n);
            o.right = Some(n);
            o.bottom = Some(n);
            o.left = Some(n);
        }
        "horizontal" => {
            o.left = Some(n);
            o.right = Some(n);
        }
        "vertical" => {
            o.top = Some(n);
            o.bottom = Some(n);
        }
        "top" => o.top = Some(n),
        "right" => o.right = Some(n),
        "bottom" => o.bottom = Some(n),
        "left" => o.left = Some(n),
        _ => return Err(format!("unsupported gaps form: {which} {n}")),
    }
    Ok(o)
}

#[derive(Clone, Debug)]
pub struct Config {
    pub bindings: HashMap<String, Vec<Binding>>, // key "default" always present
    pub exec: Vec<String>,
    pub exec_always: Vec<String>,
    pub font: Font,
    pub default_border: Border,
    pub default_floating_border: Border,
    pub workspace_layout: Layout,
    pub for_window: Vec<(Criteria, String)>,
    pub assigns: Vec<(Criteria, WorkspaceTarget)>,
    pub workspace_outputs: Vec<(String, Vec<String>)>,
    pub gaps: Gaps,
    pub workspace_gaps: Vec<(String, GapsOverride)>,
    pub colors: ClientColors,
    pub bars: Vec<BarConfig>,
    pub focus_follows_mouse: bool,
    /// The config text has a `focus_follows_mouse` line.
    pub focus_follows_mouse_explicit: bool,
    pub mouse_warping: MouseWarping,
    pub floating_modifier: Option<Modifiers>,
    pub tiling_drag: TilingDrag,
    pub show_marks: bool,
    pub focus_wrapping: FocusWrapping,
    pub workspace_auto_back_and_forth: bool,
    pub popup_during_fullscreen: PopupDuringFullscreen,
    pub focus_on_window_activation: FocusOnActivation,
    pub floating_minimum_size: (i32, i32),
    pub floating_maximum_size: (i32, i32),
    pub hide_edge_borders: HideEdgeBorders,
    pub smart_borders: SmartBorders,
    pub smart_gaps: SmartGaps,
    pub no_focus: Vec<Criteria>,
    /// Mode names declared with `--pango_markup`; the bar draws them as markup.
    pub mode_markup: HashSet<String>,
    pub warnings: Vec<String>, // "line N: message"
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseWarping {
    Output,
    Container,
    None,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TilingDrag {
    pub modifier: bool,
    pub titlebar: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusWrapping {
    Yes,
    No,
    Force,
    Workspace,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupDuringFullscreen {
    Smart,
    Ignore,
    LeaveFullscreen,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusOnActivation {
    Smart,
    Urgent,
    Focus,
    None,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HideEdgeBorders {
    None,
    Vertical,
    Horizontal,
    Both,
    Smart,
    SmartNoGaps,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmartBorders {
    Off,
    On,
    NoGaps,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmartGaps {
    Off,
    On,
    InverseOuter,
}

fn parse_bool(key: &str, args: &str) -> Result<bool, String> {
    match args {
        "yes" | "true" | "on" | "enable" | "active" => Ok(true),
        "no" | "false" | "off" | "disable" | "inactive" => Ok(false),
        _ => Err(format!("{key}: invalid value '{args}'; keeping default")),
    }
}

fn parse_modifiers(args: &str) -> Result<Modifiers, String> {
    let mut mods = Modifiers::default();
    for m in args.split('+') {
        mods = mods
            | match m.trim() {
                "Mod1" => Modifiers::ALT,
                "Mod4" => Modifiers::WIN,
                "Shift" => Modifiers::SHIFT,
                "Control" | "Ctrl" => Modifiers::CTRL,
                other => {
                    return Err(format!(
                        "floating_modifier: unknown modifier '{other}'; keeping default"
                    ))
                }
            };
    }
    Ok(mods)
}

/// Parses `W x H`, `WxH` or a single number (used for both). `-1` becomes 0
/// (no limit) and `0` becomes `default` for that dimension.
fn parse_size(key: &str, args: &str, default: (i32, i32)) -> Result<(i32, i32), String> {
    let bad = || format!("{key}: invalid size '{args}'; keeping default");
    let compact: String = args.split_whitespace().collect();
    let (w, h) = match compact.split_once('x') {
        Some((w, h)) => (w, h),
        None => (compact.as_str(), compact.as_str()),
    };
    let dim = |s: &str, def: i32| -> Result<i32, String> {
        match s.parse::<i32>().map_err(|_| bad())? {
            -1 => Ok(0),
            0 => Ok(def),
            n if n > 0 => Ok(n),
            _ => Err(bad()),
        }
    };
    Ok((dim(w, default.0)?, dim(h, default.1)?))
}

fn parse_tiling_drag(args: &str) -> Result<TilingDrag, String> {
    let toks: Vec<&str> = args.split_whitespace().collect();
    if toks == ["off"] {
        return Ok(TilingDrag {
            modifier: false,
            titlebar: false,
        });
    }
    let mut drag = TilingDrag {
        modifier: false,
        titlebar: false,
    };
    for t in &toks {
        match *t {
            "modifier" => drag.modifier = true,
            "titlebar" => drag.titlebar = true,
            _ => {
                return Err(format!(
                    "tiling_drag: invalid value '{args}'; keeping default"
                ))
            }
        }
    }
    if toks.is_empty() {
        return Err("tiling_drag: missing value; keeping default".to_string());
    }
    Ok(drag)
}

fn c(hex: u32) -> Color {
    Color((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}
fn class(border: u32, background: u32, text: u32, indicator: u32, child_border: u32) -> ColorClass {
    ColorClass {
        border: c(border),
        background: c(background),
        text: c(text),
        indicator: c(indicator),
        child_border: c(child_border),
    }
}
fn triple(border: u32, background: u32, text: u32) -> BarColorTriple {
    BarColorTriple {
        border: c(border),
        background: c(background),
        text: c(text),
    }
}

impl Default for ClientColors {
    fn default() -> Self {
        ClientColors {
            focused: class(0x4c7899, 0x285577, 0xffffff, 0x2e9ef4, 0x285577),
            focused_inactive: class(0x333333, 0x5f676a, 0xffffff, 0x484e50, 0x5f676a),
            unfocused: class(0x333333, 0x222222, 0x888888, 0x292d2e, 0x222222),
            urgent: class(0x2f343a, 0x900000, 0xffffff, 0x900000, 0x900000),
        }
    }
}
impl Default for BarColors {
    fn default() -> Self {
        BarColors {
            background: c(0x000000),
            statusline: c(0xffffff),
            separator: c(0x666666),
            focused_workspace: triple(0x4c7899, 0x285577, 0xffffff),
            active_workspace: triple(0x333333, 0x5f676a, 0xffffff),
            inactive_workspace: triple(0x333333, 0x222222, 0x888888),
            urgent_workspace: triple(0x2f343a, 0x900000, 0xffffff),
            binding_mode: triple(0x2f343a, 0x900000, 0xffffff),
            focused_background: None,
            focused_statusline: None,
            focused_separator: None,
        }
    }
}
impl Default for BarConfig {
    fn default() -> Self {
        BarConfig {
            id: "bar-0".to_string(),
            outputs: Vec::new(),
            mode: BarMode::Dock,
            position: BarPosition::Bottom,
            status_command: None,
            font: None,
            height: None,
            workspace_buttons: true,
            colors: BarColors::default(),
            hidden_state: HiddenState::Hide,
            modifier: Some(Modifiers::WIN),
            separator_symbol: None,
            strip_workspace_numbers: false,
            strip_workspace_name: false,
            binding_mode_indicator: true,
            workspace_min_width: 0,
            padding: [0; 4],
            bindings: Vec::new(),
        }
    }
}
impl Default for Config {
    fn default() -> Self {
        parse_config("")
    }
}

/// Maps i3's generic font family names to Windows font faces.
pub fn resolve_font_family(name: &str) -> &str {
    match name.to_ascii_lowercase().as_str() {
        "monospace" => "Consolas",
        "sans" | "sans-serif" => "Segoe UI",
        "serif" => "Times New Roman",
        _ => name,
    }
}

pub fn default_config_text(has_wt: bool) -> String {
    let t = include_str!("default_config");
    if has_wt {
        t.to_string()
    } else {
        t.replace("exec wt", "exec cmd")
    }
}

/// Returns the remainder of `s` after skipping `n` whitespace-separated tokens, trimmed.
fn skip_tokens(s: &str, n: usize) -> &str {
    let mut rest = s.trim_start();
    for _ in 0..n {
        rest = rest
            .find([' ', '\t'])
            .map_or("", |i| rest[i..].trim_start());
    }
    rest.trim()
}

fn first_token(s: &str) -> &str {
    s.split([' ', '\t']).next().unwrap_or("")
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').to_string()
}

fn parse_font(args: &str) -> Result<Font, String> {
    let toks: Vec<&str> = args.split_whitespace().collect();
    let (size, fam) = toks
        .split_last()
        .ok_or_else(|| "font: missing arguments".to_string())?;
    let size_pt: f32 = size
        .parse()
        .ok()
        .filter(|s: &f32| s.is_finite() && *s > 0.0)
        .ok_or_else(|| format!("font: invalid size '{size}'"))?;
    let family = fam.join(" ");
    let family = family.strip_prefix("pango:").unwrap_or(&family).to_string();
    if family.is_empty() {
        return Err("font: missing family".to_string());
    }
    Ok(Font { family, size_pt })
}

fn parse_border(args: &str) -> Result<Border, String> {
    let toks: Vec<&str> = args.split_whitespace().collect();
    let num = |i: usize| -> Option<i32> { toks.get(i)?.parse().ok().filter(|n| *n >= 0) };
    match toks.first().copied() {
        Some("normal") => match toks.get(1) {
            None => Ok(Border::Normal(2)),
            Some(_) => num(1)
                .map(Border::Normal)
                .ok_or_else(|| format!("invalid border width: {args}")),
        },
        Some("pixel") => num(1)
            .map(Border::Pixel)
            .ok_or_else(|| format!("invalid border width: {args}")),
        Some("none") => Ok(Border::None),
        _ => Err(format!("invalid border style: {args}")),
    }
}

fn parse_colors(args: &str, min: usize, max: usize) -> Result<Vec<Color>, String> {
    let cols: Vec<Color> = args
        .split_whitespace()
        .map(|t| parse_color(t).ok_or_else(|| format!("invalid color '{t}'")))
        .collect::<Result<_, _>>()?;
    if cols.len() < min || cols.len() > max {
        return Err(format!("expected {min}-{max} colors, got {}", cols.len()));
    }
    Ok(cols)
}

fn parse_triple(args: &str) -> Result<BarColorTriple, String> {
    let v = parse_colors(args, 3, 3)?;
    Ok(BarColorTriple {
        border: v[0],
        background: v[1],
        text: v[2],
    })
}

fn parse_class(args: &str, def: ColorClass) -> Result<ColorClass, String> {
    let v = parse_colors(args, 3, 5)?;
    Ok(ColorClass {
        border: v[0],
        background: v[1],
        text: v[2],
        indicator: v.get(3).copied().unwrap_or(def.indicator),
        child_border: v.get(4).copied().unwrap_or(v[1]),
    })
}

/// Splits `[criteria] rest` into the criteria and the trimmed remainder.
fn split_criteria(s: &str) -> Result<(Criteria, &str), String> {
    let s = s.trim_start();
    let body = s
        .strip_prefix('[')
        .ok_or_else(|| "expected [criteria]".to_string())?;
    let mut in_quote = false;
    let mut end = None;
    for (i, ch) in body.char_indices() {
        match ch {
            '"' => in_quote = !in_quote,
            ']' if !in_quote => {
                end = Some(i);
                break;
            }
            _ => {}
        }
    }
    let end = end.ok_or_else(|| "unterminated [criteria]".to_string())?;
    Ok((parse_criteria(&body[..end])?, body[end + 1..].trim()))
}

enum Block {
    Mode(String),
    Bar,
    BarColors,
    Unknown,
}

pub fn parse_config(text: &str) -> Config {
    let mut cfg = Config {
        bindings: HashMap::new(),
        exec: Vec::new(),
        exec_always: Vec::new(),
        font: Font {
            family: "monospace".into(),
            size_pt: 8.0,
        },
        default_border: Border::Normal(2),
        default_floating_border: Border::Normal(2),
        workspace_layout: Layout::SplitH,
        for_window: Vec::new(),
        assigns: Vec::new(),
        workspace_outputs: Vec::new(),
        gaps: Gaps::default(),
        workspace_gaps: Vec::new(),
        colors: ClientColors::default(),
        bars: Vec::new(),
        focus_follows_mouse: true,
        focus_follows_mouse_explicit: false,
        mouse_warping: MouseWarping::Output,
        floating_modifier: None,
        tiling_drag: TilingDrag {
            modifier: true,
            titlebar: true,
        },
        show_marks: true,
        focus_wrapping: FocusWrapping::Yes,
        workspace_auto_back_and_forth: false,
        popup_during_fullscreen: PopupDuringFullscreen::Smart,
        focus_on_window_activation: FocusOnActivation::Smart,
        floating_minimum_size: (75, 50),
        floating_maximum_size: (0, 0),
        hide_edge_borders: HideEdgeBorders::None,
        smart_borders: SmartBorders::Off,
        smart_gaps: SmartGaps::Off,
        no_focus: Vec::new(),
        mode_markup: HashSet::new(),
        warnings: Vec::new(),
    };
    cfg.bindings.insert("default".to_string(), Vec::new());

    let text = text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .replace("\r\n", "\n");
    // Logical lines (continuations joined), tagged with their starting line number.
    let mut lines: Vec<(usize, String)> = Vec::new();
    let mut pending: Option<(usize, String)> = None;
    for (i, raw) in text.split('\n').enumerate() {
        let (start, mut acc) = pending.take().unwrap_or((i + 1, String::new()));
        match raw.strip_suffix('\\') {
            Some(head) => {
                acc.push_str(head);
                pending = Some((start, acc));
            }
            None => {
                acc.push_str(raw);
                lines.push((start, acc));
            }
        }
    }
    if let Some(p) = pending {
        lines.push(p);
    }

    let mut vars: Vec<(String, String)> = Vec::new();
    let mut stack: Vec<Block> = Vec::new();
    for (no, raw) in lines {
        let line = raw.trim().to_string();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // For `set $name value`, only the value is substituted, never the name.
        let split = if first_token(&line) == "set" {
            let after_set = skip_tokens(&line, 1);
            let name_len = first_token(after_set).len();
            line.len() - after_set.len() + name_len
        } else {
            0
        };
        let (head, tail) = line.split_at(split);
        let mut tail = tail.to_string();
        for (name, value) in &vars {
            if tail.contains(name.as_str()) {
                tail = tail.replace(name.as_str(), value);
            }
        }
        let line = format!("{head}{tail}");
        let line = line.trim();
        if let Err(msg) = handle_line(&mut cfg, &mut stack, &mut vars, line) {
            cfg.warnings.push(format!("line {no}: {msg}"));
        }
    }
    dedupe_bar_ids(&mut cfg);
    cfg
}

/// Bar ids route state and clicks, so they must be unique: a later bar whose id
/// (explicit, or colliding with an automatic `bar-N`) is taken becomes `ID-2`,
/// `ID-3`, ... (skipping any id used elsewhere), with a warning.
fn dedupe_bar_ids(cfg: &mut Config) {
    let all: Vec<String> = cfg.bars.iter().map(|b| b.id.clone()).collect();
    let mut used: HashSet<String> = HashSet::new();
    for i in 0..cfg.bars.len() {
        let id = cfg.bars[i].id.clone();
        if used.insert(id.clone()) {
            continue;
        }
        let new_id = (2..)
            .map(|n| format!("{id}-{n}"))
            .find(|c| !used.contains(c) && !all.contains(c))
            .unwrap_or_default();
        cfg.warnings
            .push(format!("bar: duplicate id '{id}' renamed to '{new_id}'"));
        used.insert(new_id.clone());
        cfg.bars[i].id = new_id;
    }
}

fn handle_line(
    cfg: &mut Config,
    stack: &mut Vec<Block>,
    vars: &mut Vec<(String, String)>,
    line: &str,
) -> Result<(), String> {
    if line == "}" {
        stack.pop();
        return Ok(());
    }
    let kw = first_token(line);
    let args = skip_tokens(line, 1);
    match stack.last() {
        Some(Block::Unknown) => {
            if line.ends_with('{') {
                stack.push(Block::Unknown);
            }
            Ok(())
        }
        Some(Block::Mode(name)) => {
            if kw == "bindsym" || kw == "bindcode" {
                let name = name.clone();
                if kw == "bindsym" {
                    bindsym(cfg, &name, args)
                } else {
                    bindcode(cfg, &name, args)
                }
            } else {
                Err(format!("unsupported line in mode block: {line}"))
            }
        }
        Some(Block::Bar) => {
            if kw == "colors" && args == "{" {
                stack.push(Block::BarColors);
                return Ok(());
            }
            bar_key(cfg, kw, args)
        }
        Some(Block::BarColors) => bar_color_key(cfg, kw, args),
        None => top_level(cfg, stack, vars, kw, args, line),
    }
}

fn bindsym(cfg: &mut Config, mode: &str, args: &str) -> Result<(), String> {
    bind(cfg, mode, args, false)
}

fn bindcode(cfg: &mut Config, mode: &str, args: &str) -> Result<(), String> {
    bind(cfg, mode, args, true)
}

fn bind(cfg: &mut Config, mode: &str, args: &str, by_code: bool) -> Result<(), String> {
    let kw = if by_code { "bindcode" } else { "bindsym" };
    let mut rest = args;
    let mut release = false;
    loop {
        let tok = first_token(rest);
        if tok.starts_with("--") {
            release |= tok == "--release";
            rest = skip_tokens(rest, 1);
        } else {
            break;
        }
    }
    let combo_text = first_token(rest);
    let command = skip_tokens(rest, 1);
    if combo_text.is_empty() || command.is_empty() {
        return Err(format!("{kw}: expected COMBO COMMAND"));
    }
    let combo = if by_code {
        let (mods, code) = split_combo(combo_text)?;
        let code: u16 = code
            .parse()
            .map_err(|_| format!("bindcode: invalid keycode '{code}'"))?;
        let (scancode, extended) = x11_keycode_to_scancode(code)
            .ok_or_else(|| format!("bindcode: unknown keycode {code}"))?;
        scancode_combo(mods, scancode, extended)
    } else {
        parse_combo(combo_text)?
    };
    cfg.bindings
        .entry(mode.to_string())
        .or_default()
        .push(Binding {
            combo,
            command: command.to_string(),
            release,
        });
    Ok(())
}

fn bar_key(cfg: &mut Config, kw: &str, args: &str) -> Result<(), String> {
    let Some(bar) = cfg.bars.last_mut() else {
        return Err("bar: directive outside a bar block".to_string());
    };
    match kw {
        "id" => bar.id = unquote(args),
        "output" => bar.outputs.push(unquote(args)),
        "mode" => {
            bar.mode = match args {
                "dock" => BarMode::Dock,
                "hide" => BarMode::Hide,
                "invisible" => BarMode::Invisible,
                _ => return Err(format!("bar: invalid mode '{args}'")),
            }
        }
        "position" => {
            bar.position = match args {
                "top" => BarPosition::Top,
                "bottom" => BarPosition::Bottom,
                _ => return Err(format!("bar: invalid position '{args}'")),
            }
        }
        "status_command" => bar.status_command = Some(args.to_string()).filter(|s| !s.is_empty()),
        "font" => bar.font = Some(parse_font(args)?),
        "height" => {
            bar.height = Some(
                args.parse()
                    .map_err(|_| format!("bar: invalid height '{args}'"))?,
            )
        }
        "workspace_buttons" => {
            bar.workspace_buttons = match args {
                "yes" | "true" | "on" => true,
                "no" | "false" | "off" => false,
                _ => return Err(format!("bar: invalid workspace_buttons '{args}'")),
            }
        }
        "hidden_state" => {
            bar.hidden_state = match args {
                "hide" => HiddenState::Hide,
                "show" => HiddenState::Show,
                _ => return Err(format!("bar: invalid hidden_state '{args}'")),
            }
        }
        "modifier" => {
            bar.modifier = if args == "none" {
                None
            } else {
                Some(parse_modifiers(args).map_err(|_| format!("bar: invalid modifier '{args}'"))?)
            }
        }
        "separator_symbol" => bar.separator_symbol = Some(unquote(args)).filter(|s| !s.is_empty()),
        "strip_workspace_numbers" => bar.strip_workspace_numbers = bar_bool(kw, args)?,
        "strip_workspace_name" => bar.strip_workspace_name = bar_bool(kw, args)?,
        "binding_mode_indicator" => bar.binding_mode_indicator = bar_bool(kw, args)?,
        "workspace_min_width" => {
            bar.workspace_min_width = parse_px(args)
                .filter(|n| *n >= 0)
                .ok_or_else(|| format!("bar: invalid workspace_min_width '{args}'"))?
        }
        "padding" => {
            bar.padding =
                parse_padding(args).ok_or_else(|| format!("bar: invalid padding '{args}'"))?
        }
        "tray_output" | "tray_padding" => {}
        "bindsym" => {
            let mut rest = args;
            let mut release = false;
            while first_token(rest).starts_with("--") {
                release |= first_token(rest) == "--release";
                rest = skip_tokens(rest, 1);
            }
            let button = first_token(rest)
                .strip_prefix("button")
                .and_then(|n| n.parse::<u8>().ok())
                .filter(|n| *n >= 1)
                .ok_or_else(|| format!("bar bindsym: expected buttonN, got '{rest}'"))?;
            let command = skip_tokens(rest, 1);
            if command.is_empty() {
                return Err("bar bindsym: missing command".to_string());
            }
            bar.bindings.push(BarBinding {
                button,
                release,
                command: command.to_string(),
            });
        }
        _ => return Err(format!("unknown bar directive: {kw}")),
    }
    Ok(())
}

fn bar_bool(kw: &str, args: &str) -> Result<bool, String> {
    parse_bool(kw, args).map_err(|e| format!("bar: {e}"))
}

fn parse_px(s: &str) -> Option<i32> {
    s.strip_suffix("px").unwrap_or(s).trim().parse().ok()
}

/// CSS-style 1 to 4 value padding: top right bottom left.
fn parse_padding(args: &str) -> Option<[i32; 4]> {
    let v: Vec<i32> = args
        .split_whitespace()
        .map(parse_px)
        .collect::<Option<_>>()?;
    match v[..] {
        [a] => Some([a, a, a, a]),
        [v, h] => Some([v, h, v, h]),
        [t, h, b] => Some([t, h, b, h]),
        [t, r, b, l] => Some([t, r, b, l]),
        _ => None,
    }
}

fn bar_color_key(cfg: &mut Config, kw: &str, args: &str) -> Result<(), String> {
    let Some(bar) = cfg.bars.last_mut() else {
        return Err("bar colors: directive outside a bar block".to_string());
    };
    let colors = &mut bar.colors;
    let one = |args: &str| parse_colors(args, 1, 1).map(|v| v[0]);
    match kw {
        "background" => colors.background = one(args)?,
        "statusline" => colors.statusline = one(args)?,
        "separator" => colors.separator = one(args)?,
        "focused_workspace" => colors.focused_workspace = parse_triple(args)?,
        "active_workspace" => colors.active_workspace = parse_triple(args)?,
        "inactive_workspace" => colors.inactive_workspace = parse_triple(args)?,
        "urgent_workspace" => colors.urgent_workspace = parse_triple(args)?,
        "binding_mode" => colors.binding_mode = parse_triple(args)?,
        "focused_background" => colors.focused_background = Some(one(args)?),
        "focused_statusline" => colors.focused_statusline = Some(one(args)?),
        "focused_separator" => colors.focused_separator = Some(one(args)?),
        _ => return Err(format!("unknown bar colors directive: {kw}")),
    }
    Ok(())
}

fn top_level(
    cfg: &mut Config,
    stack: &mut Vec<Block>,
    vars: &mut Vec<(String, String)>,
    kw: &str,
    args: &str,
    line: &str,
) -> Result<(), String> {
    match kw {
        "set" => {
            let name = first_token(args);
            if !name.starts_with('$') || name.len() < 2 {
                return Err("set: expected $name value".to_string());
            }
            vars.retain(|(n, _)| n != name);
            vars.push((name.to_string(), skip_tokens(args, 1).to_string()));
            vars.sort_by_key(|(n, _)| std::cmp::Reverse(n.len()));
        }
        "bindsym" => bindsym(cfg, "default", args)?,
        "bindcode" => bindcode(cfg, "default", args)?,
        "mode" => {
            let name = args
                .strip_suffix('{')
                .ok_or_else(|| "mode: expected `{` on the same line".to_string())?;
            let name = name.trim();
            let (pango, name) = match name.strip_prefix("--pango_markup") {
                Some(rest) if rest.starts_with(char::is_whitespace) => (true, rest.trim_start()),
                _ => (false, name),
            };
            let name = unquote(name);
            if pango {
                cfg.mode_markup.insert(name.clone());
            }
            cfg.bindings.entry(name.clone()).or_default();
            stack.push(Block::Mode(name));
        }
        "bar" if args == "{" => {
            cfg.bars.push(BarConfig {
                id: format!("bar-{}", cfg.bars.len()),
                ..BarConfig::default()
            });
            stack.push(Block::Bar);
        }
        "font" => cfg.font = parse_font(args).map_err(|e| format!("{e}; keeping default"))?,
        "exec" | "exec_always" => {
            let mut cmd = args;
            if first_token(cmd) == "--no-startup-id" {
                cmd = skip_tokens(cmd, 1);
            }
            if cmd.is_empty() {
                return Err(format!("{kw}: missing command"));
            }
            let list = if kw == "exec" {
                &mut cfg.exec
            } else {
                &mut cfg.exec_always
            };
            list.push(cmd.to_string());
        }
        "default_border" | "new_window" => cfg.default_border = parse_border(args)?,
        "default_floating_border" => cfg.default_floating_border = parse_border(args)?,
        "workspace_layout" => {
            cfg.workspace_layout = match args {
                "default" => Layout::SplitH,
                "stacking" | "stacked" => Layout::Stacked,
                "tabbed" => Layout::Tabbed,
                _ => return Err(format!("workspace_layout: invalid value '{args}'")),
            }
        }
        "for_window" => {
            let (crit, cmd) = split_criteria(args)?;
            if crit.has_urgent() {
                return Err("for_window: the urgent criterion is not supported".to_string());
            }
            if cmd.is_empty() {
                return Err("for_window: missing command".to_string());
            }
            cfg.for_window.push((crit, cmd.to_string()));
        }
        "assign" => {
            let (crit, rest) = split_criteria(args)?;
            if !crit.uses_only_window_info() {
                return Err(
                    "assign: only class, instance and title criteria are supported".to_string(),
                );
            }
            let mut toks: Vec<&str> = rest.split_whitespace().collect();
            if matches!(toks.first(), Some(&"\u{2192}") | Some(&"->")) {
                toks.remove(0);
            }
            if toks.first() == Some(&"workspace") {
                toks.remove(0);
            }
            let target = if toks.first() == Some(&"number") {
                let n = toks
                    .get(1)
                    .and_then(|t| unquote(t).parse().ok())
                    .ok_or_else(|| "assign: invalid workspace number".to_string())?;
                WorkspaceTarget::Number(n)
            } else {
                let name = unquote(&toks.join(" "));
                if name.is_empty() {
                    return Err("assign: missing workspace".to_string());
                }
                WorkspaceTarget::Name(name)
            };
            cfg.assigns.push((crit, target));
        }
        "workspace" => {
            let toks: Vec<&str> = args.split_whitespace().collect();
            let gaps_at = toks.iter().position(|t| *t == "gaps").filter(|i| {
                *i >= 1
                    && toks
                        .iter()
                        .position(|t| *t == "output")
                        .is_none_or(|o| *i < o)
            });
            if let Some(i) = gaps_at {
                let name = unquote(&toks[..i].join(" "));
                let o = parse_gaps_override(&toks[i + 1..])?;
                match cfg.workspace_gaps.iter_mut().find(|(n, _)| *n == name) {
                    Some((_, cur)) => *cur = cur.merge(&o),
                    None => cfg.workspace_gaps.push((name, o)),
                }
                return Ok(());
            }
            let i = toks
                .iter()
                .position(|t| *t == "output")
                .filter(|i| *i >= 1 && *i + 1 < toks.len())
                .ok_or_else(|| "workspace: expected NAME output OUTPUT...".to_string())?;
            let name = unquote(&toks[..i].join(" "));
            if name.starts_with("__") {
                return Err("Workspace names starting with __ are reserved".into());
            }
            let outs = toks[i + 1..].iter().map(|o| unquote(o)).collect();
            cfg.workspace_outputs.push((name, outs));
        }
        "gaps" => {
            let toks: Vec<&str> = args.split_whitespace().collect();
            let o = parse_gaps_override(&toks)?;
            cfg.gaps = cfg.gaps.with(&o);
        }
        "focus_follows_mouse" => {
            cfg.focus_follows_mouse = parse_bool(kw, args)?;
            cfg.focus_follows_mouse_explicit = true;
        }
        "mouse_warping" => {
            cfg.mouse_warping = match args {
                "output" => MouseWarping::Output,
                "container" => MouseWarping::Container,
                "none" => MouseWarping::None,
                _ => return Err(format!("mouse_warping: invalid value '{args}'")),
            }
        }
        "floating_modifier" => cfg.floating_modifier = Some(parse_modifiers(args)?),
        "tiling_drag" => cfg.tiling_drag = parse_tiling_drag(args)?,
        "show_marks" => cfg.show_marks = parse_bool(kw, args)?,
        "focus_wrapping" => {
            cfg.focus_wrapping = match args {
                "yes" | "true" | "on" => FocusWrapping::Yes,
                "no" | "false" | "off" => FocusWrapping::No,
                "force" => FocusWrapping::Force,
                "workspace" => FocusWrapping::Workspace,
                _ => return Err(format!("focus_wrapping: invalid value '{args}'")),
            }
        }
        "workspace_auto_back_and_forth" => {
            cfg.workspace_auto_back_and_forth = parse_bool(kw, args)?
        }
        "popup_during_fullscreen" => {
            cfg.popup_during_fullscreen = match args {
                "smart" => PopupDuringFullscreen::Smart,
                "ignore" => PopupDuringFullscreen::Ignore,
                "leave_fullscreen" => PopupDuringFullscreen::LeaveFullscreen,
                _ => return Err(format!("popup_during_fullscreen: invalid value '{args}'")),
            }
        }
        "focus_on_window_activation" => {
            cfg.focus_on_window_activation = match args {
                "smart" => FocusOnActivation::Smart,
                "urgent" => FocusOnActivation::Urgent,
                "focus" => FocusOnActivation::Focus,
                "none" => FocusOnActivation::None,
                _ => {
                    return Err(format!(
                        "focus_on_window_activation: invalid value '{args}'"
                    ))
                }
            }
        }
        "floating_minimum_size" => cfg.floating_minimum_size = parse_size(kw, args, (75, 50))?,
        "floating_maximum_size" => cfg.floating_maximum_size = parse_size(kw, args, (0, 0))?,
        "hide_edge_borders" => {
            cfg.hide_edge_borders = match args {
                "none" => HideEdgeBorders::None,
                "vertical" => HideEdgeBorders::Vertical,
                "horizontal" => HideEdgeBorders::Horizontal,
                "both" => HideEdgeBorders::Both,
                "smart" => HideEdgeBorders::Smart,
                "smart_no_gaps" => HideEdgeBorders::SmartNoGaps,
                _ => return Err(format!("hide_edge_borders: invalid value '{args}'")),
            }
        }
        "smart_borders" => {
            cfg.smart_borders = match args {
                "on" | "yes" | "true" => SmartBorders::On,
                "off" | "no" | "false" => SmartBorders::Off,
                "no_gaps" => SmartBorders::NoGaps,
                _ => return Err(format!("smart_borders: invalid value '{args}'")),
            }
        }
        "smart_gaps" => {
            cfg.smart_gaps = match args {
                "on" | "yes" | "true" => SmartGaps::On,
                "off" | "no" | "false" => SmartGaps::Off,
                "inverse_outer" => SmartGaps::InverseOuter,
                _ => return Err(format!("smart_gaps: invalid value '{args}'")),
            }
        }
        "no_focus" => {
            let (crit, rest) = split_criteria(args)?;
            if !rest.is_empty() {
                return Err("no_focus: unexpected text after criteria".to_string());
            }
            cfg.no_focus.push(crit);
        }
        "client.focused" => cfg.colors.focused = parse_class(args, cfg.colors.focused)?,
        "client.focused_inactive" => {
            cfg.colors.focused_inactive = parse_class(args, cfg.colors.focused_inactive)?
        }
        "client.unfocused" => cfg.colors.unfocused = parse_class(args, cfg.colors.unfocused)?,
        "client.urgent" => cfg.colors.urgent = parse_class(args, cfg.colors.urgent)?,
        "client.placeholder" | "client.background" => {}
        _ => {
            if line.ends_with('{') {
                stack.push(Block::Unknown);
            }
            return Err(format!("unknown directive: {line}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::Modifiers;
    use crate::tree::Layout;

    #[test]
    fn multiple_bars_ids_and_outputs() {
        let c = parse_config(
            "bar {
status_command a
}
bar {
id top
position top
output DISPLAY2
output primary
}
",
        );
        assert_eq!(c.bars.len(), 2);
        assert_eq!(c.bars[0].id, "bar-0");
        assert_eq!(c.bars[1].id, "top");
        assert_eq!(c.bars[1].outputs, vec!["DISPLAY2", "primary"]);
    }

    #[test]
    fn ffm_explicit_flag() {
        assert!(!parse_config("").focus_follows_mouse_explicit);
        assert!(
            parse_config(
                "focus_follows_mouse yes
"
            )
            .focus_follows_mouse_explicit
        );
    }

    #[test]
    fn new_options_defaults() {
        let c = parse_config("");
        assert!(c.focus_follows_mouse);
        assert_eq!(c.mouse_warping, MouseWarping::Output);
        assert_eq!(c.floating_modifier, None);
        assert_eq!(
            c.tiling_drag,
            TilingDrag {
                modifier: true,
                titlebar: true
            }
        );
        assert!(c.show_marks);
        assert_eq!(c.focus_wrapping, FocusWrapping::Yes);
        assert!(!c.workspace_auto_back_and_forth);
        assert_eq!(c.popup_during_fullscreen, PopupDuringFullscreen::Smart);
        assert_eq!(c.focus_on_window_activation, FocusOnActivation::Smart);
        assert_eq!(c.floating_minimum_size, (75, 50));
        assert_eq!(c.floating_maximum_size, (0, 0));
        assert_eq!(c.hide_edge_borders, HideEdgeBorders::None);
        assert_eq!(c.smart_borders, SmartBorders::Off);
        assert_eq!(c.smart_gaps, SmartGaps::Off);
    }
    #[test]
    fn new_options_parse() {
        let c = parse_config(
            "set $mod Mod4\nfocus_follows_mouse no\nmouse_warping none\nfloating_modifier $mod\n\
             tiling_drag titlebar\nshow_marks no\nfocus_wrapping workspace\n\
             workspace_auto_back_and_forth yes\npopup_during_fullscreen leave_fullscreen\n\
             focus_on_window_activation urgent\nfloating_minimum_size 100 x 80\n\
             floating_maximum_size 800x600\nhide_edge_borders smart_no_gaps\n\
             smart_borders no_gaps\nsmart_gaps inverse_outer\nno_focus [class=\"^x$\"]\n",
        );
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        assert!(!c.focus_follows_mouse);
        assert_eq!(c.mouse_warping, MouseWarping::None);
        assert_eq!(c.floating_modifier, Some(Modifiers::WIN));
        assert_eq!(
            c.tiling_drag,
            TilingDrag {
                modifier: false,
                titlebar: true
            }
        );
        assert!(!c.show_marks);
        assert_eq!(c.focus_wrapping, FocusWrapping::Workspace);
        assert!(c.workspace_auto_back_and_forth);
        assert_eq!(
            c.popup_during_fullscreen,
            PopupDuringFullscreen::LeaveFullscreen
        );
        assert_eq!(c.focus_on_window_activation, FocusOnActivation::Urgent);
        assert_eq!(c.floating_minimum_size, (100, 80));
        assert_eq!(c.floating_maximum_size, (800, 600));
        assert_eq!(c.hide_edge_borders, HideEdgeBorders::SmartNoGaps);
        assert_eq!(c.smart_borders, SmartBorders::NoGaps);
        assert_eq!(c.smart_gaps, SmartGaps::InverseOuter);
        assert_eq!(c.no_focus.len(), 1);
        assert_eq!(
            parse_config("tiling_drag off\n").tiling_drag,
            TilingDrag {
                modifier: false,
                titlebar: false
            }
        );
        assert_eq!(
            parse_config("floating_minimum_size -1 x -1\n").floating_minimum_size,
            (0, 0)
        );
        assert_eq!(
            parse_config("floating_modifier Mod4+Shift\n").floating_modifier,
            Some(Modifiers::WIN | Modifiers::SHIFT)
        );
        assert_eq!(
            parse_config("floating_maximum_size -1 x -1\n").floating_maximum_size,
            (0, 0)
        );
        assert_eq!(
            parse_config("floating_minimum_size 0 x 0\n").floating_minimum_size,
            (75, 50)
        );
        assert_eq!(
            parse_config("smart_borders yes\n").smart_borders,
            SmartBorders::On
        );
    }
    #[test]
    fn bad_new_option_values_warn_and_keep_default() {
        let c = parse_config("focus_wrapping sideways\nmouse_warping x\n");
        assert_eq!(c.warnings.len(), 2);
        assert_eq!(c.focus_wrapping, FocusWrapping::Yes);
    }

    #[test]
    fn defaults() {
        let c = Config::default();
        assert_eq!(
            c.font,
            Font {
                family: "monospace".into(),
                size_pt: 8.0
            }
        );
        assert_eq!(c.default_border, Border::Normal(2));
        assert!(c.bindings.contains_key("default") && c.bars.is_empty());
        assert_eq!(c.colors.focused.background, Color(0x28, 0x55, 0x77));
    }
    #[test]
    fn defaults_match_i3() {
        let c = parse_config("bar {\n}\n");
        assert_eq!(c.bars[0].position, BarPosition::Bottom);
        assert_eq!(
            c.font,
            Font {
                family: "monospace".into(),
                size_pt: 8.0
            }
        );
        assert_eq!(c.default_border, Border::Normal(2));
    }
    #[test]
    fn font_family_aliases() {
        assert_eq!(resolve_font_family("monospace"), "Consolas");
        assert_eq!(resolve_font_family("Monospace"), "Consolas");
        assert_eq!(resolve_font_family("sans"), "Segoe UI");
        assert_eq!(resolve_font_family("sans-serif"), "Segoe UI");
        assert_eq!(resolve_font_family("serif"), "Times New Roman");
        assert_eq!(resolve_font_family("DejaVu Sans Mono"), "DejaVu Sans Mono");
    }
    #[test]
    fn default_config_text_is_i3s() {
        let t = default_config_text(true);
        for line in [
            "set $mod Mod1",
            "font pango:monospace 8",
            "floating_modifier $mod",
            "tiling_drag modifier titlebar",
            "focus_follows_mouse no",
            "bindsym $mod+Return exec wt",
            "bindsym $mod+Shift+e exit",
            "bindsym $mod+r mode \"resize\"",
            "status_command i3win-status",
        ] {
            assert!(t.contains(line), "missing: {line}");
        }
        assert!(!t.contains("position"));
        assert!(t.contains("delete\n# this file and restart i3win to regenerate it."));
        assert!(t.contains(
            "i3win addition: click-to-focus for new Windows users; i3's default is yes."
        ));
        assert!(!t.contains("re-run i3win") && !t.contains("as in i3's own default"));
        assert!(default_config_text(false).contains("bindsym $mod+Return exec cmd"));
        let c = parse_config(&t);
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    }
    #[test]
    fn bindcode_and_release_parse() {
        let c = parse_config("set $mod Mod1\nbindcode $mod+38 nop a\nbindsym --release $mod+x nop b\nbindcode 300 nop\n");
        let b = &c.bindings["default"];
        assert_eq!(b[0].combo, scancode_combo(Modifiers::ALT, 0x1E, false));
        assert!(!b[0].release);
        assert!(b[1].release);
        assert_eq!(c.warnings.len(), 1);
    }
    #[test]
    fn variables_and_bindsym() {
        let c = parse_config("set $mod Mod4\nbindsym $mod+Return exec wt\n");
        let b = &c.bindings["default"][0];
        assert_eq!(b.combo, crate::keys::parse_combo("Mod4+Return").unwrap());
        assert_eq!(b.command, "exec wt");
    }
    #[test]
    fn longest_variable_first() {
        let c = parse_config("set $m Mod1\nset $mod Mod4\nbindsym $mod+a nop\n");
        assert_eq!(c.bindings["default"][0].combo.mods, Modifiers::WIN);
    }
    #[test]
    fn mode_pango_markup_flag() {
        let c = parse_config(
            "mode --pango_markup \"<b>resize</b>\" {\nbindsym Escape mode default\n}\n",
        );
        assert!(c.mode_markup.contains("<b>resize</b>"));
        assert!(c.bindings.contains_key("<b>resize</b>"));
        let c = parse_config("mode \"plain\" {\nbindsym Escape mode default\n}\n");
        assert!(c.mode_markup.is_empty());
    }
    #[test]
    fn modes() {
        let c = parse_config("mode \"resize\" {\n bindsym Escape mode \"default\"\n}\n");
        assert_eq!(c.bindings["resize"][0].command, "mode \"default\"");
    }
    #[test]
    fn bar_options_parse() {
        let c = parse_config("bar {\nmode hide\nhidden_state show\nmodifier Mod1\nseparator_symbol \" | \"\nstrip_workspace_numbers yes\nbinding_mode_indicator no\nworkspace_min_width 40\npadding 2px 4px\ntray_output primary\nbindsym button4 workspace prev\nbindsym --release button3 nop\ncolors {\nfocused_background #111111\n}\n}\n");
        let b = &c.bars[0];
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        assert_eq!(b.mode, BarMode::Hide);
        assert_eq!(b.hidden_state, HiddenState::Show);
        assert_eq!(b.modifier, Some(Modifiers::ALT));
        assert_eq!(b.separator_symbol.as_deref(), Some(" | "));
        assert!(b.strip_workspace_numbers && !b.binding_mode_indicator);
        assert_eq!(b.workspace_min_width, 40);
        assert_eq!(b.padding, [2, 4, 2, 4]);
        assert_eq!(b.bindings.len(), 2);
        assert!(b.bindings[1].release);
        assert_eq!(b.bindings[0].button, 4);
        assert_eq!(b.colors.focused_background, Some(Color(0x11, 0x11, 0x11)));
    }

    #[test]
    fn bar_padding_forms_and_modifier_none() {
        assert_eq!(parse_padding("3"), Some([3; 4]));
        assert_eq!(parse_padding("1 2 3"), Some([1, 2, 3, 2]));
        assert_eq!(parse_padding("1 2 3 4px"), Some([1, 2, 3, 4]));
        assert_eq!(parse_padding("1 2 3 4 5"), None);
        let c = parse_config("bar {\nmodifier none\n}\n");
        assert_eq!(c.bars[0].modifier, None);
    }

    #[test]
    fn bar_block() {
        let c = parse_config("bar {\n position bottom\n status_command i3win-status --x\n colors {\n background #111111\n focused_workspace #010203 #040506 #070809\n }\n}\n");
        let b = &c.bars[0];
        assert_eq!(b.position, BarPosition::Bottom);
        assert_eq!(b.status_command.as_deref(), Some("i3win-status --x"));
        assert_eq!(b.colors.background, Color(0x11, 0x11, 0x11));
        assert_eq!(b.colors.focused_workspace.text, Color(7, 8, 9));
    }
    #[test]
    fn font_borders_gaps_layout() {
        let c = parse_config("font pango:DejaVu Sans Mono 10.5\ndefault_border pixel 3\ngaps inner 10\ngaps outer 4\nworkspace_layout tabbed\n");
        assert_eq!(
            c.font,
            Font {
                family: "DejaVu Sans Mono".into(),
                size_pt: 10.5
            }
        );
        assert_eq!(c.default_border, Border::Pixel(3));
        assert_eq!(
            c.gaps,
            Gaps {
                inner: 10,
                top: 4,
                right: 4,
                bottom: 4,
                left: 4
            }
        );
        assert_eq!(c.workspace_layout, Layout::Tabbed);
    }
    #[test]
    fn gaps_forms() {
        let c = parse_config("gaps inner 10\ngaps outer 5\ngaps horizontal 7\ngaps top 1\nworkspace 2 gaps inner 0\nworkspace 2 gaps left 3\n");
        assert_eq!(
            c.gaps,
            Gaps {
                inner: 10,
                top: 1,
                right: 7,
                bottom: 5,
                left: 7
            }
        );
        assert_eq!(
            c.workspace_gaps,
            vec![(
                "2".to_string(),
                GapsOverride {
                    inner: Some(0),
                    left: Some(3),
                    ..Default::default()
                }
            )]
        );
    }
    #[test]
    fn client_colors_child_border_defaults_to_background() {
        let c = parse_config("client.focused #111111 #222222 #333333\n");
        assert_eq!(c.colors.focused.child_border, Color(0x22, 0x22, 0x22));
        assert_eq!(c.colors.focused.indicator, Color(0x2e, 0x9e, 0xf4));
    }
    #[test]
    fn for_window_assign_workspace_output() {
        let c = parse_config("for_window [class=\"calc\"] floating enable\nassign [class=\"slack\"] → number 3\nworkspace 1 output DISPLAY1 DISPLAY2\n");
        assert_eq!(c.for_window[0].1, "floating enable");
        assert_eq!(c.assigns[0].1, WorkspaceTarget::Number(3));
        assert_eq!(
            c.workspace_outputs[0],
            (
                "1".to_string(),
                vec!["DISPLAY1".to_string(), "DISPLAY2".to_string()]
            )
        );
    }
    #[test]
    fn assign_and_for_window_reject_unsupported_criteria() {
        let c = parse_config("assign [con_mark=x] 3\n");
        assert_eq!(c.warnings.len(), 1, "{:?}", c.warnings);
        assert!(c.assigns.is_empty());
        let c = parse_config("assign [floating] 3\n");
        assert_eq!(c.warnings.len(), 1, "{:?}", c.warnings);
        assert!(c.assigns.is_empty());
        let c = parse_config("for_window [urgent=latest] floating enable\n");
        assert_eq!(c.warnings.len(), 1, "{:?}", c.warnings);
        assert!(c.for_window.is_empty());
        let c = parse_config("for_window [floating] border none\nassign [class=x] 3\n");
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    }
    #[test]
    fn bad_lines_warn_not_panic() {
        let c = parse_config("bogus line\nbindsym Mod7+a nop\nfont x y\n");
        assert_eq!(c.warnings.len(), 3);
    }
    #[test]
    fn parses_crlf_tabs_and_bom() {
        let c = parse_config(
            "\u{feff}set $mod Mod1\r\nbindsym\t$mod+a   nop  \r\nexec_always \\\r\n  wt\r\n",
        );
        assert_eq!(c.bindings["default"].len(), 1);
        assert_eq!(c.exec_always, vec!["wt".to_string()]);
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    }
    #[test]
    fn default_config_parses_cleanly() {
        let c = parse_config(&default_config_text(true));
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        assert!(c.bindings["default"].len() >= 50 && c.bindings["resize"].len() == 11);
        assert_eq!(c.bars[0].status_command.as_deref(), Some("i3win-status"));
        assert!(default_config_text(false).contains("exec cmd"));
    }

    #[test]
    fn bar_id_is_unquoted() {
        let c = parse_config(
            "bar {
id \"top\"
}
",
        );
        assert_eq!(c.bars[0].id, "top");
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    }

    #[test]
    fn duplicate_bar_ids_are_renamed_with_warning() {
        let c = parse_config(
            "bar {
id top
}
bar {
id top
}
bar {
id top
}
bar {
id bar-4
}
bar {
}
",
        );
        let ids: Vec<&str> = c.bars.iter().map(|b| b.id.as_str()).collect();
        // The fifth bar's automatic id `bar-4` collides with the explicit one.
        assert_eq!(ids, ["top", "top-2", "top-3", "bar-4", "bar-4-2"]);
        assert_eq!(c.warnings.len(), 3, "{:?}", c.warnings);
        assert!(c.warnings[0].contains("duplicate id 'top'"));
        // A rename skips ids taken by later bars.
        let c = parse_config(
            "bar {
id a
}
bar {
id a
}
bar {
id a-2
}
",
        );
        let ids: Vec<&str> = c.bars.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, ["a", "a-3", "a-2"]);
    }
}
