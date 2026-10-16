use regex::Regex;

#[derive(Clone, Debug)]
pub struct Criteria {
    pub class: Option<Regex>,
    pub instance: Option<Regex>,
    pub title: Option<Regex>,
}

/// Parses the text between `[` and `]`, e.g. `class="^firefox$" title=foo`.
pub fn parse_criteria(inner: &str) -> Result<Criteria, String> {
    let mut c = Criteria {
        class: None,
        instance: None,
        title: None,
    };
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
        if chars.next() != Some('=') {
            return Err(format!("expected '=' after criteria key '{key}'"));
        }
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
        let re = Regex::new(&value).map_err(|e| format!("bad regex for '{key}': {e}"))?;
        match key.as_str() {
            "class" => c.class = Some(re),
            "instance" => c.instance = Some(re),
            "title" => c.title = Some(re),
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn errors() {
        assert!(parse_criteria("").is_err());
        assert!(parse_criteria("con_mark=x").is_err());
        assert!(parse_criteria(r#"class="(""#).is_err());
    }
}
