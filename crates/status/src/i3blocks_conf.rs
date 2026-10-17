//! Parser for i3blocks (1.5) configuration files. Pure text processing.

#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub name: String,
    /// Globals first, then the section's own; keys are case-sensitive.
    pub props: Vec<(String, String)>,
}

impl Section {
    /// Last occurrence wins, so own keys override globals.
    pub fn get(&self, k: &str) -> Option<&str> {
        self.props
            .iter()
            .rev()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Clone, Debug, Default)]
pub struct I3BlocksConfig {
    pub sections: Vec<Section>,
    /// (1-based line number, message)
    pub errors: Vec<(usize, String)>,
}

pub fn parse_i3blocks(text: &str) -> I3BlocksConfig {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut c = I3BlocksConfig::default();
    let mut globals: Vec<(String, String)> = Vec::new();
    for (idx, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            if let Some(name) = rest.strip_suffix(']') {
                c.sections.push(Section {
                    name: name.trim().to_string(),
                    props: globals.clone(),
                });
                continue;
            }
        } else if let Some((k, v)) = line.split_once('=') {
            let k = k.trim();
            if !k.is_empty() && !k.contains(char::is_whitespace) {
                let pair = (k.to_string(), v.trim().to_string());
                match c.sections.last_mut() {
                    Some(s) => s.props.push(pair),
                    None => globals.push(pair),
                }
                continue;
            }
        }
        c.errors
            .push((idx + 1, "expected key=value or [section]".to_string()));
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globals_and_sections() {
        let c = parse_i3blocks("# x\nseparator_block_width=15\ninterval=5\n\n[cpu_usage]\nformat=CPU %usage\n\n[weather]\ncommand=curl -s wttr.in/?format=1\ninterval=600\nDISK=C:\n");
        assert!(c.errors.is_empty());
        assert_eq!(c.sections.len(), 2);
        assert_eq!(c.sections[1].get("interval"), Some("600"));
        assert_eq!(c.sections[0].get("interval"), Some("5"));
        assert_eq!(c.sections[1].get("separator_block_width"), Some("15"));
        assert_eq!(c.sections[1].get("DISK"), Some("C:"));
        assert_eq!(
            c.sections[1].get("command"),
            Some("curl -s wttr.in/?format=1")
        );
    }

    #[test]
    fn i3blocks_bad_line_reported() {
        let c = parse_i3blocks("[a]\nfull_text=x\nnonsense line\n[b]\nfull_text=y\n");
        assert_eq!(
            c.errors,
            vec![(3, "expected key=value or [section]".to_string())]
        );
        assert_eq!(c.sections.len(), 2);
    }

    #[test]
    fn utf8_bom_is_stripped() {
        let c = parse_i3blocks("\u{feff}interval=5\n[a]\nfull_text=x\n");
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        assert_eq!(c.sections.len(), 1);
        assert_eq!(c.sections[0].get("interval"), Some("5"));
    }
}
