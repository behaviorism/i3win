use crate::tree::{NodeId, NodeKind, WindowInfo};
use crate::wm::Wm;
use regex::Regex;

const FOCUSED: &str = "__focused__";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UrgentPick {
    Latest,
    Oldest,
}

#[derive(Clone, Debug, Default)]
pub struct Criteria {
    pub class: Option<Regex>,
    pub instance: Option<Regex>,
    pub title: Option<Regex>,
    pub con_mark: Option<Regex>,
    pub con_id: Option<u32>,
    pub id: Option<isize>,
    pub workspace: Option<Regex>,
    pub floating: bool,
    pub tiling: bool,
    pub urgent: Option<UrgentPick>,
    /// `__focused__` values: equal to the focused window's / workspace's value.
    pub focused_class: bool,
    pub focused_instance: bool,
    pub focused_title: bool,
    pub focused_workspace: bool,
    pub focused_con: bool,
}

/// Parses the text between `[` and `]`, e.g. `class="^firefox$" title=foo`.
pub fn parse_criteria(inner: &str) -> Result<Criteria, String> {
    let mut c = Criteria::default();
    let mut chars = inner.chars().peekable();
    let mut any = false;
    loop {
        while chars.peek().is_some_and(|ch| ch.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
        let mut key = String::new();
        while let Some(&ch) = chars.peek() {
            if ch == '=' || ch.is_whitespace() {
                break;
            }
            key.push(ch);
            chars.next();
        }
        if chars.peek() != Some(&'=') {
            match key.as_str() {
                "floating" => c.floating = true,
                "tiling" => c.tiling = true,
                _ => return Err(format!("expected '=' after criteria key '{key}'")),
            }
            any = true;
            continue;
        }
        chars.next();
        let mut value = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            let mut closed = false;
            while let Some(ch) = chars.next() {
                match ch {
                    '\\' if matches!(chars.peek(), Some('"' | '\\')) => {
                        if let Some(e) = chars.next() {
                            value.push(e);
                        }
                    }
                    '"' => {
                        closed = true;
                        break;
                    }
                    _ => value.push(ch),
                }
            }
            if !closed {
                return Err(format!("unterminated quote in value of '{key}'"));
            }
        } else {
            while let Some(&ch) = chars.peek() {
                if ch.is_whitespace() {
                    break;
                }
                value.push(ch);
                chars.next();
            }
        }
        let focused = value == FOCUSED;
        let re = |value: &str| Regex::new(value).map_err(|e| format!("bad regex for '{key}': {e}"));
        match key.as_str() {
            "class" if focused => c.focused_class = true,
            "instance" if focused => c.focused_instance = true,
            "title" if focused => c.focused_title = true,
            "workspace" if focused => c.focused_workspace = true,
            "con_id" if focused => c.focused_con = true,
            "class" => c.class = Some(re(&value)?),
            "instance" => c.instance = Some(re(&value)?),
            "title" => c.title = Some(re(&value)?),
            "con_mark" => c.con_mark = Some(re(&value)?),
            "workspace" => c.workspace = Some(re(&value)?),
            "con_id" => {
                c.con_id = Some(value.parse().map_err(|_| format!("bad con_id '{value}'"))?)
            }
            "id" => c.id = Some(value.parse().map_err(|_| format!("bad id '{value}'"))?),
            "urgent" => {
                c.urgent = Some(match value.as_str() {
                    "latest" => UrgentPick::Latest,
                    "oldest" => UrgentPick::Oldest,
                    _ => return Err(format!("bad urgent value '{value}'")),
                })
            }
            _ => return Err(format!("unknown criteria key '{key}'")),
        }
        any = true;
    }
    if !any {
        return Err("empty criteria".into());
    }
    Ok(c)
}

impl Criteria {
    /// Every present regex must match (search semantics).
    pub fn matches(&self, class: &str, instance: &str, title: &str) -> bool {
        let ok = |re: &Option<Regex>, s: &str| re.as_ref().is_none_or(|r| r.is_match(s));
        ok(&self.class, class) && ok(&self.instance, instance) && ok(&self.title, title)
    }

    /// True when no key beyond class/instance/title is set (what `matches` can evaluate).
    pub fn uses_only_window_info(&self) -> bool {
        self.con_mark.is_none()
            && self.con_id.is_none()
            && self.id.is_none()
            && self.workspace.is_none()
            && !self.floating
            && !self.tiling
            && self.urgent.is_none()
            && !(self.focused_class
                || self.focused_instance
                || self.focused_title
                || self.focused_workspace
                || self.focused_con)
    }

    pub fn has_urgent(&self) -> bool {
        self.urgent.is_some()
    }

    /// Matches a node that already exists in the tree; every key except
    /// `urgent` (which is a selection, see `select_matches`).
    pub fn matches_node(&self, wm: &Wm, node: NodeId) -> bool {
        let n = wm.tree.get(node);
        let win = match &n.kind {
            NodeKind::Window(d) => Some(&d.info),
            _ => None,
        };
        let focused_win = match &wm.tree.get(wm.focused()).kind {
            NodeKind::Window(d) => Some(&d.info),
            _ => None,
        };
        let re_ok = |re: &Option<Regex>, f: fn(&WindowInfo) -> &str| {
            re.as_ref()
                .is_none_or(|r| win.is_some_and(|w| r.is_match(f(w))))
        };
        let eq_ok = |flag: bool, f: fn(&WindowInfo) -> &str| {
            !flag || matches!((win, focused_win), (Some(w), Some(fw)) if f(w) == f(fw))
        };
        if !(re_ok(&self.class, |w| &w.class)
            && re_ok(&self.instance, |w| &w.instance)
            && re_ok(&self.title, |w| &w.title)
            && eq_ok(self.focused_class, |w| &w.class)
            && eq_ok(self.focused_instance, |w| &w.instance)
            && eq_ok(self.focused_title, |w| &w.title))
        {
            return false;
        }
        if let Some(re) = &self.con_mark {
            if !n.marks.iter().any(|m| re.is_match(m)) {
                return false;
            }
        }
        if self.con_id.is_some_and(|id| id != node.0) {
            return false;
        }
        if self
            .id
            .is_some_and(|id| wm.tree.window_id(node).is_none_or(|w| w.0 != id))
        {
            return false;
        }
        if self.workspace.is_some() || self.focused_workspace {
            let Some(ws) = wm.tree.workspace_of(node) else {
                return false;
            };
            let name = wm.workspace_name(ws);
            if self.workspace.as_ref().is_some_and(|r| !r.is_match(name)) {
                return false;
            }
            if self.focused_workspace && ws != wm.focused_workspace() {
                return false;
            }
        }
        if self.floating && !wm.tree.is_floating(node) {
            return false;
        }
        if self.tiling && wm.tree.is_floating(node) {
            return false;
        }
        !self.focused_con || node == wm.focused()
    }
}

/// Windows and split containers in tree order (tiled before floating).
fn windows_and_containers(wm: &Wm) -> Vec<NodeId> {
    fn walk(wm: &Wm, n: NodeId, out: &mut Vec<NodeId>) {
        let node = wm.tree.get(n);
        if matches!(node.kind, NodeKind::Window(_) | NodeKind::Container)
            && !wm.tree.fully_parked(n)
        {
            out.push(n);
        }
        for &c in node.children.iter().chain(node.floating.iter()) {
            walk(wm, c, out);
        }
    }
    let mut out = Vec::new();
    walk(wm, wm.tree.root(), &mut out);
    out
}

/// All windows matching `c` (split containers too when `con_mark` or
/// `con_id` is given: every other key is a window property); with
/// `urgent=latest|oldest` only the most / least recently urgent window among
/// the matches.
pub fn select_matches(wm: &Wm, c: &Criteria) -> Vec<NodeId> {
    let candidates = if c.con_mark.is_some() || c.con_id.is_some() {
        windows_and_containers(wm)
    } else {
        wm.tree.windows_under(wm.tree.root())
    };
    let matched = candidates.into_iter().filter(|&n| c.matches_node(wm, n));
    let Some(pick) = c.urgent else {
        return matched.collect();
    };
    let urgent_since = |n: NodeId| match &wm.tree.get(n).kind {
        NodeKind::Window(d) if d.urgent => Some(d.urgent_since),
        _ => None,
    };
    let urgent = matched.filter_map(|n| urgent_since(n).map(|s| (s, n)));
    let best = match pick {
        UrgentPick::Latest => urgent.max_by_key(|&(s, _)| s),
        UrgentPick::Oldest => urgent.min_by_key(|&(s, _)| s),
    };
    best.map(|(_, n)| n).into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{test_wm, win};
    use crate::tree::WindowId;

    #[test]
    fn quoted_and_bare_values() {
        let c = parse_criteria(r#"class="^fire fox$" title=Mozilla"#).unwrap();
        assert!(c.matches("fire fox", "MozillaWindowClass", "Page - Mozilla"));
        assert!(!c.matches("firefox", "x", "Mozilla"));
    }
    #[test]
    fn all_fields_must_match() {
        let c = parse_criteria(r#"class="code" instance="Chrome_WidgetWin_1""#).unwrap();
        assert!(c.matches("code", "Chrome_WidgetWin_1", "t"));
        assert!(!c.matches("code", "Other", "t"));
    }
    #[test]
    fn escaped_quote() {
        let c = parse_criteria(r#"title="say \"hi\"""#).unwrap();
        assert!(c.matches("", "", r#"say "hi""#));
    }
    #[test]
    fn escaped_backslash_ends_quoted_value() {
        let c = parse_criteria(r#"title="a\\\\" class=x"#).unwrap();
        assert!(c.matches("x", "", r"a\"));
    }
    #[test]
    fn new_criteria_keys() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("[id=2] floating enable");
        let n1 = wm.tree.window_node(WindowId(1)).unwrap();
        let n2 = wm.tree.window_node(WindowId(2)).unwrap();
        let c = parse_criteria("floating").unwrap();
        assert!(c.matches_node(&wm, n2) && !c.matches_node(&wm, n1));
        let c = parse_criteria("tiling").unwrap();
        assert!(c.matches_node(&wm, n1));
        let c = parse_criteria(&format!("con_id={}", n1.0)).unwrap();
        assert!(c.matches_node(&wm, n1) && !c.matches_node(&wm, n2));
        let c = parse_criteria("workspace=\"^1$\"").unwrap();
        assert!(c.matches_node(&wm, n1));
        let c = parse_criteria("class=__focused__").unwrap();
        assert!(c.matches_node(&wm, wm.focused()));
        assert!(!c.matches_node(&wm, n1));
    }
    #[test]
    fn con_mark_matches_marks() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        let n1 = wm.tree.window_node(WindowId(1)).unwrap();
        let n2 = wm.tree.window_node(WindowId(2)).unwrap();
        wm.tree.get_mut(n1).marks.push("foo".into());
        let c = parse_criteria("con_mark=\"^fo+$\"").unwrap();
        assert!(c.matches_node(&wm, n1) && !c.matches_node(&wm, n2));
    }
    #[test]
    fn urgent_latest_and_oldest() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.manage(win(3));
        wm.run_command("workspace 2");
        wm.on_focus_changed(WindowId(2)); // becomes urgent (hidden workspace)
        wm.on_focus_changed(WindowId(1));
        let latest = select_matches(&wm, &parse_criteria("urgent=latest").unwrap());
        assert_eq!(latest, vec![wm.tree.window_node(WindowId(1)).unwrap()]);
        let oldest = select_matches(&wm, &parse_criteria("urgent=oldest").unwrap());
        assert_eq!(oldest, vec![wm.tree.window_node(WindowId(2)).unwrap()]);
    }
    #[test]
    fn urgent_since_stable_across_repeat_focus_events() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("workspace 2");
        wm.on_focus_changed(WindowId(2));
        let n2 = wm.tree.window_node(WindowId(2)).unwrap();
        let since = |wm: &Wm| match &wm.tree.get(n2).kind {
            NodeKind::Window(d) => d.urgent_since,
            _ => unreachable!(),
        };
        let first = since(&wm);
        wm.on_focus_changed(WindowId(1));
        wm.on_focus_changed(WindowId(2));
        assert_eq!(since(&wm), first);
    }
    #[test]
    fn errors() {
        assert!(parse_criteria("").is_err());
        assert!(parse_criteria("con_mark=x").is_ok());
        assert!(parse_criteria("bogus=x").is_err());
        assert!(parse_criteria("urgent=sometimes").is_err());
        assert!(parse_criteria("con_id=abc").is_err());
        assert!(parse_criteria(r#"class="(""#).is_err());
    }
}
