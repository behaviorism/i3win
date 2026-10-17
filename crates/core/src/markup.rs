//! Parser for a small subset of Pango markup, used by status bar text.

use crate::config::{parse_color, Color};

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Style {
    pub bold: bool,
    pub weight: Option<u16>,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub mono: bool,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    /// 1.0 = normal; small 0.833, big 1.2, x-small 0.694, x-large 1.44, sup/sub 0.75.
    /// Absolute sizes (`NNpt`, Pango units) are carried by `size_pt`.
    pub scale: f32,
    pub size_pt: Option<f32>,
    pub family: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub text: String,
    pub style: Style,
}

/// Parses markup into styled runs. Never fails: malformed input yields one run
/// holding the input verbatim (entities not decoded).
pub fn parse_markup(s: &str) -> Vec<Run> {
    parse(s).unwrap_or_else(|| {
        vec![Run {
            text: s.to_string(),
            style: base_style(),
        }]
    })
}

/// Whether `s` parses as markup (`parse_markup` would not fall back to verbatim).
pub fn is_valid(s: &str) -> bool {
    parse(s).is_some()
}

/// The runs' text concatenated.
pub fn plain_text(s: &str) -> String {
    parse_markup(s).iter().map(|r| r.text.as_str()).collect()
}

/// Escapes `& < > " '` as entities.
pub fn escape_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

fn base_style() -> Style {
    Style {
        scale: 1.0,
        ..Default::default()
    }
}

fn push_text(runs: &mut Vec<Run>, text: &str, style: &Style) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = runs.last_mut() {
        if last.style == *style {
            last.text.push_str(text);
            return;
        }
    }
    runs.push(Run {
        text: text.to_string(),
        style: style.clone(),
    });
}

fn parse(s: &str) -> Option<Vec<Run>> {
    let mut runs: Vec<Run> = Vec::new();
    // (tag name, style in effect before the tag opened)
    let mut stack: Vec<(String, Style)> = Vec::new();
    let mut cur = base_style();
    let mut rest = s;
    while !rest.is_empty() {
        let lt = rest.find('<').unwrap_or(rest.len());
        let (text, tail) = rest.split_at(lt);
        if !text.is_empty() {
            push_text(&mut runs, &decode_entities(text)?, &cur);
        }
        rest = tail;
        if rest.is_empty() {
            break;
        }
        // rest starts with '<'
        if let Some(close) = rest.strip_prefix("</") {
            let end = close.find('>')?;
            let name = close[..end].trim();
            let (open, prev) = stack.pop()?;
            if open != name {
                return None;
            }
            cur = prev;
            rest = &close[end + 1..];
        } else {
            let (name, attrs, after) = parse_open_tag(&rest[1..])?;
            let prev = cur.clone();
            apply_tag(&mut cur, name, &attrs);
            stack.push((name.to_string(), prev));
            rest = after;
        }
    }
    if !stack.is_empty() {
        return None;
    }
    Some(runs)
}

/// Parses `name attr='v' ...>` (input is after the `<`). Returns the name,
/// attributes and the text after the `>`.
#[allow(clippy::type_complexity)]
fn parse_open_tag(s: &str) -> Option<(&str, Vec<(&str, &str)>, &str)> {
    let name_end = s.find(|c: char| c.is_whitespace() || c == '>')?;
    let name = &s[..name_end];
    if name.is_empty() || name.contains(['<', '/', '=', '\'', '"']) {
        return None;
    }
    let mut rest = &s[name_end..];
    let mut attrs = Vec::new();
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix('>') {
            return Some((name, attrs, after));
        }
        let key_end = rest.find(|c: char| c.is_whitespace() || c == '=' || c == '>')?;
        let key = &rest[..key_end];
        if key.is_empty() || key.contains(['<', '/', '\'', '"']) {
            return None;
        }
        rest = rest[key_end..].trim_start().strip_prefix('=')?.trim_start();
        let quote = rest.chars().next().filter(|c| *c == '\'' || *c == '"')?;
        let body = &rest[1..];
        let end = body.find(quote)?;
        attrs.push((key, &body[..end]));
        rest = &body[end + 1..];
    }
}

fn decode_entities(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let semi = after.find(';')?;
        let ent = &after[..semi];
        let c = match ent {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let num = ent.strip_prefix('#')?;
                let code = match num.strip_prefix(['x', 'X']) {
                    Some(h) => u32::from_str_radix(h, 16).ok()?,
                    None => num.parse::<u32>().ok()?,
                };
                char::from_u32(code)?
            }
        };
        out.push(c);
        rest = &after[semi + 1..];
    }
    out.push_str(rest);
    Some(out)
}

fn apply_tag(st: &mut Style, name: &str, attrs: &[(&str, &str)]) {
    match name {
        "b" => st.bold = true,
        "i" => st.italic = true,
        "u" => st.underline = true,
        "s" => st.strike = true,
        "small" => st.scale *= 0.833,
        "big" => st.scale *= 1.2,
        "sup" | "sub" => st.scale *= 0.75,
        "tt" => st.mono = true,
        "span" => {
            for (k, v) in attrs {
                apply_span_attr(st, k, v);
            }
        }
        _ => {}
    }
}

fn apply_span_attr(st: &mut Style, key: &str, v: &str) {
    match key {
        "foreground" | "fgcolor" | "color" => {
            if let Some(c) = color(v) {
                st.fg = Some(c);
            }
        }
        "background" | "bgcolor" => {
            if let Some(c) = color(v) {
                st.bg = Some(c);
            }
        }
        "weight" => {
            let w = match v {
                "bold" => Some(700),
                "normal" => Some(400),
                "light" => Some(300),
                "ultrabold" => Some(800),
                "heavy" => Some(900),
                n => n.parse::<u16>().ok(),
            };
            if let Some(w) = w {
                st.weight = Some(w);
                st.bold = w >= 600;
            }
        }
        "style" => match v {
            "italic" | "oblique" => st.italic = true,
            "normal" => st.italic = false,
            _ => {}
        },
        "underline" => match v {
            "single" | "double" | "low" | "error" => st.underline = true,
            "none" => st.underline = false,
            _ => {}
        },
        "strikethrough" => match v {
            "true" => st.strike = true,
            "false" => st.strike = false,
            _ => {}
        },
        "size" => apply_size(st, v),
        "font_family" | "face" => st.family = Some(v.to_string()),
        _ => {}
    }
}

fn apply_size(st: &mut Style, v: &str) {
    let abs = |st: &mut Style, f: f32| {
        st.scale = f;
        st.size_pt = None;
    };
    match v {
        "xx-small" => abs(st, 0.579),
        "x-small" => abs(st, 0.694),
        "small" => abs(st, 0.833),
        "medium" => abs(st, 1.0),
        "large" => abs(st, 1.2),
        "x-large" => abs(st, 1.44),
        "xx-large" => abs(st, 1.728),
        "smaller" => st.scale /= 1.2,
        "larger" => st.scale *= 1.2,
        _ => {
            if let Some(pt) = v.strip_suffix("pt").and_then(|n| n.parse::<f32>().ok()) {
                st.size_pt = Some(pt);
            } else if let Some(p) = v.strip_suffix('%').and_then(|n| n.parse::<f32>().ok()) {
                st.scale *= p / 100.0;
            } else if let Ok(n) = v.parse::<u32>() {
                st.size_pt = Some(n as f32 / 1024.0);
            }
        }
    }
}

fn color(v: &str) -> Option<Color> {
    if let Some(h) = v.strip_prefix('#') {
        if h.len() == 3 && h.is_ascii() {
            let p = |i: usize| u8::from_str_radix(&h[i..i + 1], 16).ok().map(|n| n * 17);
            return Some(Color(p(0)?, p(1)?, p(2)?));
        }
        return parse_color(v);
    }
    Some(match v {
        "black" => Color(0, 0, 0),
        "white" => Color(255, 255, 255),
        "red" => Color(255, 0, 0),
        "green" => Color(0, 128, 0),
        "blue" => Color(0, 0, 255),
        "yellow" => Color(255, 255, 0),
        "cyan" => Color(0, 255, 255),
        "magenta" => Color(255, 0, 255),
        "gray" | "grey" => Color(128, 128, 128),
        "orange" => Color(255, 165, 0),
        "purple" => Color(128, 0, 128),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain_text_of(r: &[Run]) -> String {
        r.iter().map(|x| x.text.as_str()).collect()
    }

    #[test]
    fn plain_and_entities() {
        assert_eq!(
            parse_markup("a &amp; b &#65;&#x42;"),
            vec![Run {
                text: "a & b AB".into(),
                style: Style {
                    scale: 1.0,
                    ..Default::default()
                }
            }]
        );
    }

    #[test]
    fn nested_tags_and_span() {
        let r = parse_markup("x<b>y<i>z</i></b><span foreground='#ff0000' size='large'>w</span>");
        assert_eq!(r.len(), 4);
        assert_eq!(r[1].text, "y");
        assert!(r[1].style.bold && !r[1].style.italic);
        assert_eq!(r[2].text, "z");
        assert!(r[2].style.bold && r[2].style.italic);
        assert_eq!(r[3].style.fg, Some(Color(255, 0, 0)));
        assert!((r[3].style.scale - 1.2).abs() < 1e-6);
    }

    #[test]
    fn unknown_tags_dropped_text_kept() {
        assert_eq!(plain_text_of(&parse_markup("a<blink>b</blink>c")), "abc");
    }

    #[test]
    fn malformed_markup_falls_back_to_plain() {
        for bad in [
            "<b>unclosed",
            "<b>x</i>",
            "a & b",
            "&bogus;",
            "<span foo=>x</span",
            "</b>",
        ] {
            let r = parse_markup(bad);
            assert_eq!(r.len(), 1, "{bad}");
            assert_eq!(r[0].text, bad);
        }
    }

    #[test]
    fn escape_round_trip() {
        let s = "a<b>&\"'";
        assert_eq!(plain_text(&escape_markup(s)), s);
    }

    #[test]
    fn colors_and_sizes() {
        let r = parse_markup("<span color='#f80' bgcolor='orange' size='12pt'>a</span>");
        assert_eq!(r[0].style.fg, Some(Color(255, 136, 0)));
        assert_eq!(r[0].style.bg, Some(Color(255, 165, 0)));
        assert_eq!(r[0].style.size_pt, Some(12.0));
        let r = parse_markup("<span size='10240'>a</span>");
        assert_eq!(r[0].style.size_pt, Some(10.0));
    }

    #[test]
    fn weight_underline_strike_family() {
        let r = parse_markup(
            "<span weight='heavy' underline='low' strikethrough='true' face='Consolas'>a</span>",
        );
        let s = &r[0].style;
        assert!(s.bold && s.underline && s.strike);
        assert_eq!(s.weight, Some(900));
        assert_eq!(s.family.as_deref(), Some("Consolas"));
    }

    #[test]
    fn adjacent_equal_styles_merge() {
        let r = parse_markup("a<blink>b</blink>c");
        assert_eq!(r.len(), 1);
        let r = parse_markup("<tt>a</tt>b<small>c</small>");
        assert_eq!(r.len(), 3);
        assert!(r[0].style.mono);
        assert!((r[2].style.scale - 0.833).abs() < 1e-6);
    }

    #[test]
    fn validity() {
        assert!(is_valid("<b>a</b> &amp; b"));
        assert!(is_valid("plain"));
        assert!(!is_valid("<b>unclosed"));
        assert!(!is_valid("a & b"));
    }
}
