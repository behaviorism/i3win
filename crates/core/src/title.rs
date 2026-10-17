use crate::markup::{escape_markup, is_valid};

/// Expands a `title_format` (`None` = `%title`). Unknown `%x` stay literal.
/// With `show_marks`, each mark not starting with `_` is appended as ` [mark]`.
/// An explicit `format` is markup: substituted values and marks are escaped and
/// the returned flag is true. Without one, or when the format itself is malformed
/// markup, the result is plain unescaped text and the flag is false.
pub fn format_title(
    format: Option<&str>,
    title: &str,
    class: &str,
    instance: &str,
    machine: &str,
    marks: &[String],
    show_marks: bool,
) -> (String, bool) {
    let expand = |escape: bool| {
        expand_title(
            format.unwrap_or("%title"),
            [title, class, instance, machine],
            marks,
            show_marks,
            escape,
        )
    };
    if format.is_some() {
        let out = expand(true);
        if is_valid(&out) {
            return (out, true);
        }
    }
    (expand(false), false)
}

/// Substitutes `%title %class %instance %machine` (in `values` order) into `fmt`
/// and appends the visible marks, escaping inserted text when `escape` is set.
fn expand_title(
    fmt: &str,
    values: [&str; 4],
    marks: &[String],
    show_marks: bool,
    escape: bool,
) -> String {
    let esc = |s: &str| {
        if escape {
            escape_markup(s)
        } else {
            s.to_string()
        }
    };
    let names = ["%title", "%class", "%instance", "%machine"];
    let mut out = String::new();
    let mut rest = fmt;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        match names.iter().zip(values).find(|(p, _)| rest.starts_with(*p)) {
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
    out
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

    #[test]
    fn malformed_title_format_falls_back_to_unescaped_plain_text() {
        let (t, mk) = format_title(Some("<b>%title & co"), "A&B", "c", "i", "PC", &[], true);
        assert_eq!(t, "<b>A&B & co");
        assert!(!mk);
        let marks = vec!["<m>".to_string()];
        let (t, mk) = format_title(Some("<i>%title"), "T", "c", "i", "PC", &marks, true);
        assert_eq!(t, "<i>T [<m>]");
        assert!(!mk);
    }
}
