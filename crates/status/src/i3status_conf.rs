//! Parser for i3status-style configuration files. Pure text processing.

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Str(String),
    Num(f64),
    Bool(bool),
}

impl Value {
    pub fn as_str(&self) -> String {
        match self {
            Value::Str(s) => s.clone(),
            Value::Num(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            Value::Str(s) => s.trim().parse().ok(),
            Value::Bool(_) => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            Value::Str(s) => match s.as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            },
            Value::Num(_) => None,
        }
    }
}

/// Ordered properties; on lookup a later duplicate wins.
pub type Props = Vec<(String, Value)>;

pub fn get<'a>(p: &'a Props, key: &str) -> Option<&'a Value> {
    p.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModuleRef {
    pub module: String,
    pub instance: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct I3StatusConfig {
    pub general: Props,
    pub order: Vec<ModuleRef>,
    pub blocks: Vec<(ModuleRef, Props)>,
    /// (1-based line number, message)
    pub errors: Vec<(usize, String)>,
}

pub fn module_props<'a>(c: &'a I3StatusConfig, m: &ModuleRef) -> Option<&'a Props> {
    c.blocks.iter().rev().find(|(r, _)| r == m).map(|(_, p)| p)
}

/// Text before a `#` that starts the line or follows whitespace (comment start).
fn cut_comment_naive(s: &str) -> &str {
    let mut prev_ws = true;
    for (i, c) in s.char_indices() {
        if c == '#' && prev_ws {
            return &s[..i];
        }
        prev_ws = c.is_whitespace();
    }
    s
}

/// Parses a quoted string at the start of `s` (which must begin with `"`).
/// Returns the value and the byte index just past the closing quote, or None
/// if unterminated. `\\` is a backslash and `\"` an escaped quote, except a
/// `\"` that is the last `"` on the line, which is a literal backslash
/// followed by the closing quote.
fn parse_quoted(s: &str) -> Option<(String, usize)> {
    let mut out = String::new();
    let mut it = s.char_indices().skip(1).peekable();
    while let Some((i, c)) = it.next() {
        match c {
            '"' => return Some((out, i + 1)),
            '\\' => match it.peek().copied() {
                Some((_, '\\')) => {
                    out.push('\\');
                    it.next();
                }
                Some((j, '"')) => {
                    let rest = &s[j + 1..];
                    if cut_comment_naive(rest).contains('"') {
                        out.push('"');
                        it.next();
                    } else {
                        out.push('\\');
                        return Some((out, j + 1));
                    }
                }
                _ => out.push('\\'),
            },
            _ => out.push(c),
        }
    }
    None
}

/// Removes a trailing comment, ignoring `#` inside quoted strings.
fn strip_comment(line: &str) -> &str {
    let mut prev_ws = true;
    let mut i = 0;
    while i < line.len() {
        let c = line[i..].chars().next().unwrap();
        if c == '"' {
            match parse_quoted(&line[i..]) {
                Some((_, end)) => {
                    i += end;
                    prev_ws = false;
                    continue;
                }
                None => return line,
            }
        }
        if c == '#' && prev_ws {
            return &line[..i];
        }
        prev_ws = c.is_whitespace();
        i += c.len_utf8();
    }
    line
}

fn split_module_ref(s: &str) -> Option<ModuleRef> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    match s.find(char::is_whitespace) {
        None => Some(ModuleRef {
            module: s.to_string(),
            instance: None,
        }),
        Some(i) => {
            let rest = s[i..].trim();
            let instance = if rest.starts_with('"') {
                match parse_quoted(rest) {
                    Some((v, end)) if end == rest.len() => v,
                    _ => return None,
                }
            } else {
                rest.to_string()
            };
            Some(ModuleRef {
                module: s[..i].to_string(),
                instance: Some(instance),
            })
        }
    }
}

fn parse_value(v: &str) -> Option<Value> {
    let v = v.trim();
    if v.starts_with('"') {
        return match parse_quoted(v) {
            Some((s, end)) if end == v.len() => Some(Value::Str(s)),
            _ => None,
        };
    }
    Some(match v {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => match v.parse::<f64>() {
            Ok(n) if !v.is_empty() => Value::Num(n),
            _ => Value::Str(v.to_string()),
        },
    })
}

struct OpenBlock {
    line: usize,
    /// None for `general`.
    target: Option<ModuleRef>,
    props: Props,
    bad: bool,
}

pub fn parse_i3status(text: &str) -> I3StatusConfig {
    let mut c = I3StatusConfig::default();
    let mut open: Option<OpenBlock> = None;
    for (idx, raw) in text.lines().enumerate() {
        let n = idx + 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(b) = open.as_mut() {
            if line == "}" {
                let b = open.take().unwrap();
                if !b.bad {
                    match b.target {
                        None => c.general.extend(b.props),
                        Some(r) => c.blocks.push((r, b.props)),
                    }
                }
            } else if !b.bad {
                match line.split_once('=') {
                    Some((k, v)) if !k.trim().is_empty() => match parse_value(v) {
                        Some(val) => b.props.push((k.trim().to_string(), val)),
                        None => {
                            c.errors
                                .push((n, "unterminated or malformed string".into()));
                            b.bad = true;
                        }
                    },
                    _ => {
                        c.errors
                            .push((n, format!("expected `key = value`, got `{line}`")));
                        b.bad = true;
                    }
                }
            }
            continue;
        }
        if let Some(head) = line.strip_suffix('{') {
            let head = head.trim();
            if head == "general" {
                open = Some(OpenBlock {
                    line: n,
                    target: None,
                    props: Vec::new(),
                    bad: false,
                });
            } else {
                match split_module_ref(head) {
                    Some(r) => {
                        open = Some(OpenBlock {
                            line: n,
                            target: Some(r),
                            props: Vec::new(),
                            bad: false,
                        })
                    }
                    None => {
                        c.errors.push((n, format!("bad block header `{head}`")));
                        // Skip the body of the unparseable block.
                        open = Some(OpenBlock {
                            line: n,
                            target: None,
                            props: Vec::new(),
                            bad: true,
                        });
                    }
                }
            }
        } else if let Some(rest) = line.strip_prefix("order") {
            let rest = rest.trim_start();
            match rest.strip_prefix("+=").map(str::trim) {
                Some(v) if v.starts_with('"') => match parse_quoted(v) {
                    Some((s, end)) if end == v.len() => match split_module_ref(&s) {
                        Some(r) => c.order.push(r),
                        None => c.errors.push((n, "empty order entry".into())),
                    },
                    _ => c.errors.push((n, "malformed order string".into())),
                },
                _ => c.errors.push((n, "expected `order += \"MODULE\"`".into())),
            }
        } else {
            c.errors
                .push((n, format!("line outside a block: `{line}`")));
        }
    }
    if let Some(b) = open {
        c.errors.push((b.line, "missing closing `}`".into()));
    }
    c
}

pub const DEFAULT_I3STATUS_CONF: &str = r#"general {
        colors = true
        interval = 1
}

order += "cpu_usage"
order += "memory"
order += "disk C:"
order += "battery all"
order += "tztime local"

cpu_usage {
        format = "CPU %usage"
}
memory {
        format = "MEM %percentage_used"
}
disk "C:" {
        format = "C: %avail"
        prefix_type = binary
}
battery all {
        format = "BAT %percentage"
        format_down = ""
}
tztime local {
        format = "%Y-%m-%d %H:%M:%S"
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_general_order_blocks() {
        let c = parse_i3status("general {\n colors = true\n interval = 5\n color_good = \"#00FF00\"\n}\norder += \"disk C:\"\norder += \"cpu_usage\"\n# c\ndisk \"C:\" {\n format = \"%free\" # trailing\n}\ncpu_usage {\n max_threshold = 90\n}\n");
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        assert_eq!(get(&c.general, "interval"), Some(&Value::Num(5.0)));
        assert_eq!(get(&c.general, "colors"), Some(&Value::Bool(true)));
        assert_eq!(
            c.order,
            vec![
                ModuleRef {
                    module: "disk".into(),
                    instance: Some("C:".into())
                },
                ModuleRef {
                    module: "cpu_usage".into(),
                    instance: None
                }
            ]
        );
        let d = module_props(&c, &c.order[0]).unwrap();
        assert_eq!(get(d, "format").unwrap().as_str(), "%free");
    }

    #[test]
    fn quoted_instance_with_trailing_backslash() {
        let c = parse_i3status("disk \"C:\\\" {\n format = \"x\"\n}\norder += \"disk C:\\\"\n");
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        assert_eq!(c.blocks[0].0.instance.as_deref(), Some("C:\\"));
        assert_eq!(c.order[0].instance.as_deref(), Some("C:\\"));
    }

    #[test]
    fn bad_block_skipped_and_reported() {
        let c = parse_i3status("cpu_usage {\n format\n}\nmemory {\n format = \"m\"\n}\n");
        assert_eq!(c.errors.len(), 1);
        assert_eq!(c.errors[0].0, 2);
        assert!(module_props(
            &c,
            &ModuleRef {
                module: "memory".into(),
                instance: None
            }
        )
        .is_some());
        assert!(c.blocks.iter().all(|(r, _)| r.module != "cpu_usage"));
    }

    #[test]
    fn default_conf_parses_cleanly() {
        let c = parse_i3status(DEFAULT_I3STATUS_CONF);
        assert!(c.errors.is_empty());
        assert_eq!(c.order.len(), 5);
        assert_eq!(c.blocks.len(), 5);
    }

    #[test]
    fn missing_close_and_outside_block_reported() {
        let c = parse_i3status("stray = 1\nmemory {\n format = \"m\"\n");
        assert_eq!(c.errors.len(), 2);
        assert_eq!(c.errors[0].0, 1);
        assert_eq!(c.errors[1].0, 2);
        assert!(c.blocks.is_empty());
    }

    #[test]
    fn escaped_quote_and_duplicates() {
        let c = parse_i3status("memory {\n format = \"a\\\"b\"\n x = 1\n x = 2\n}\n");
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let p = &c.blocks[0].1;
        assert_eq!(get(p, "format").unwrap().as_str(), "a\"b");
        assert_eq!(get(p, "x"), Some(&Value::Num(2.0)));
    }

    #[test]
    fn value_conversions() {
        assert_eq!(Value::Str("3".into()).as_f64(), Some(3.0));
        assert_eq!(Value::Bool(true).as_f64(), None);
        assert_eq!(Value::Str("true".into()).as_bool(), Some(true));
        assert_eq!(Value::Num(1.5).as_str(), "1.5");
    }
}
