use crate::markup::escape_markup;

/// Expands a `title_format` (`None` = `%title`). Unknown `%x` stay literal.
/// With `show_marks`, each mark not starting with `_` is appended as ` [mark]`.
/// An explicit `format` is markup: substituted values and marks are escaped and
/// the returned flag is true. Without one the result is plain text.
pub fn format_title(
    format: Option<&str>,
    title: &str,
    class: &str,
    instance: &str,
    machine: &str,
    marks: &[String],
    show_marks: bool,
) -> (String, bool) {
    let markup = format.is_some();
    let esc = |s: &str| {
        if markup {
            escape_markup(s)
        } else {
            s.to_string()
        }
    };
    let fmt = format.unwrap_or("%title");
    let mut out = String::new();
    let mut rest = fmt;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let table = [
            ("%title", title),
            ("%class", class),
            ("%instance", instance),
            ("%machine", machine),
        ];
        match table.iter().find(|(p, _)| rest.starts_with(p)) {
            Some((p, value)) => {
                out.push_str(&esc(value));
                rest = &rest[p.len()..];
            }
            None => {
                out.push('%');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    if show_marks {
        for m in marks.iter().filter(|m| !m.starts_with('_')) {
            out.push_str(" [");
            out.push_str(&esc(m));
            out.push(']');
        }
    }
    (out, markup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_title_placeholders_and_marks() {
        assert_eq!(format_title(None, "T", "c", "i", "PC", &[], true).0, "T");
        assert_eq!(
            format_title(
                Some("%title (%class/%instance) @%machine %x"),
                "T",
                "c",
                "i",
                "PC",
                &[],
                true
            )
            .0,
            "T (c/i) @PC %x"
        );
        let marks = vec!["a".to_string(), "_hidden".to_string(), "b".to_string()];
        assert_eq!(
            format_title(None, "T", "c", "i", "PC", &marks, true).0,
            "T [a] [b]"
        );
        assert_eq!(
            format_title(None, "T", "c", "i", "PC", &marks, false).0,
            "T"
        );
    }

    #[test]
    fn title_format_escapes_values_and_flags_markup() {
        let (t, mk) = format_title(Some("<b>%title</b>"), "a<b>&", "c", "i", "PC", &[], true);
        assert_eq!(t, "<b>a&lt;b&gt;&amp;</b>");
        assert!(mk);
        let (t, mk) = format_title(None, "a<b>", "c", "i", "PC", &[], true);
        assert_eq!(t, "a<b>");
        assert!(!mk);
        let marks = vec!["<m>".to_string()];
        let (t, _) = format_title(Some("%title"), "T", "c", "i", "PC", &marks, true);
        assert_eq!(t, "T [&lt;m&gt;]");
    }
}
