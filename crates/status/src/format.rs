//! Format-string substitution, color levels and common block keys.

use crate::i3status_conf::{get, Props, Value};
use i3win_core::i3bar::{Block, MinWidth};

/// Replaces `%name` tokens (longest names first); unknown `%x` stay, `%%` -> `%`.
pub fn substitute(format: &str, vars: &[(&str, String)]) -> String {
    let mut sorted: Vec<&(&str, String)> = vars.iter().collect();
    sorted.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
    let mut out = String::new();
    let mut rest = format;
    while let Some(pos) = rest.find('%') {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 1..];
        if let Some(tail) = after.strip_prefix('%') {
            out.push('%');
            rest = tail;
        } else if let Some((name, val)) = sorted
            .iter()
            .map(|v| (v.0, &v.1))
            .find(|(n, _)| !n.is_empty() && after.starts_with(n))
        {
            out.push_str(val);
            rest = &after[name.len()..];
        } else {
            out.push('%');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Colors {
    pub enabled: bool,
    pub good: String,
    pub degraded: String,
    pub bad: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Normal,
    Good,
    Degraded,
    Bad,
}

/// Colors from `general`, with `color_*` keys of the module overriding.
pub fn colors_from(general: &Props, module: Option<&Props>) -> Colors {
    let lookup = |key: &str| -> Option<&Value> {
        module
            .and_then(|m| get(m, key))
            .or_else(|| get(general, key))
    };
    let text = |key: &str, default: &str| {
        lookup(key)
            .map(Value::as_str)
            .unwrap_or_else(|| default.to_string())
    };
    Colors {
        enabled: lookup("colors").and_then(Value::as_bool).unwrap_or(true),
        good: text("color_good", "#00FF00"),
        degraded: text("color_degraded", "#FFFF00"),
        bad: text("color_bad", "#FF0000"),
    }
}

pub fn make_block(
    name: &str,
    instance: Option<&str>,
    text: String,
    level: Level,
    colors: &Colors,
    module: &Props,
) -> Block {
    let mut b = Block::new(text);
    b.name = Some(name.to_string());
    b.instance = instance.map(str::to_string);
    if colors.enabled {
        b.color = match level {
            Level::Normal => None,
            Level::Good => Some(colors.good.clone()),
            Level::Degraded => Some(colors.degraded.clone()),
            Level::Bad => Some(colors.bad.clone()),
        };
    }
    b.min_width = get(module, "min_width").map(|v| match v {
        Value::Num(n) => MinWidth::Px(*n as i32),
        other => match other.as_f64() {
            Some(n) => MinWidth::Px(n as i32),
            None => MinWidth::Text(other.as_str()),
        },
    });
    b.align = get(module, "align").map(Value::as_str);
    if let Some(s) = get(module, "separator").and_then(Value::as_bool) {
        b.separator = s;
    }
    b.separator_block_width = get(module, "separator_block_width")
        .and_then(Value::as_f64)
        .map(|n| n as i32);
    b.markup = get(module, "markup").map(Value::as_str);
    b
}

/// i3status-style size: one decimal, "binary" (KiB..) or decimal/custom (kB..).
pub fn format_bytes(bytes: u64, prefix: &str) -> String {
    let (base, units): (f64, [&str; 5]) = if prefix == "binary" {
        (1024.0, ["B", "KiB", "MiB", "GiB", "TiB"])
    } else {
        (1000.0, ["B", "kB", "MB", "GB", "TB"])
    };
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= base && i < units.len() - 1 {
        v /= base;
        i += 1;
    }
    format!("{v:.1} {}", units[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitute_longest_first() {
        let v = [("used", "1".to_string()), ("used_percent", "2".to_string())];
        assert_eq!(
            substitute("%used_percent %used %x 100%%", &v),
            "2 1 %x 100%"
        );
    }

    #[test]
    fn make_block_levels_and_common_keys() {
        let colors = Colors {
            enabled: true,
            good: "#0f0".into(),
            degraded: "#ff0".into(),
            bad: "#f00".into(),
        };
        let props = vec![
            ("min_width".into(), Value::Num(80.0)),
            ("align".into(), Value::Str("right".into())),
            ("separator".into(), Value::Bool(false)),
        ];
        let b = make_block(
            "cpu_usage",
            None,
            "CPU 5%".into(),
            Level::Bad,
            &colors,
            &props,
        );
        assert_eq!(b.color.as_deref(), Some("#f00"));
        assert_eq!(b.min_width, Some(MinWidth::Px(80)));
        assert_eq!(b.align.as_deref(), Some("right"));
        assert!(!b.separator);
        assert_eq!(b.name.as_deref(), Some("cpu_usage"));
        let off = Colors {
            enabled: false,
            ..colors
        };
        assert_eq!(
            make_block("x", None, "t".into(), Level::Bad, &off, &vec![]).color,
            None
        );
    }

    #[test]
    fn make_block_text_min_width_and_defaults() {
        let colors = colors_from(&vec![], None);
        let props = vec![("min_width".into(), Value::Str("00:00".into()))];
        let b = make_block("t", Some("i"), "x".into(), Level::Normal, &colors, &props);
        assert_eq!(b.min_width, Some(MinWidth::Text("00:00".into())));
        assert_eq!(b.instance.as_deref(), Some("i"));
        assert!(b.separator);
        assert_eq!(b.color, None);
    }

    #[test]
    fn colors_module_overrides_general() {
        let general = vec![
            ("color_good".into(), Value::Str("#111111".into())),
            ("colors".into(), Value::Bool(false)),
        ];
        let module = vec![("color_good".into(), Value::Str("#222222".into()))];
        let c = colors_from(&general, Some(&module));
        assert_eq!(c.good, "#222222");
        assert!(!c.enabled);
        assert_eq!(c.bad, "#FF0000");
    }

    #[test]
    fn bytes_formatting() {
        assert_eq!(format_bytes(1_610_612_736, "binary"), "1.5 GiB");
        assert_eq!(format_bytes(1_500_000_000, "decimal"), "1.5 GB");
        assert_eq!(format_bytes(512, "binary"), "512.0 B");
    }
}
