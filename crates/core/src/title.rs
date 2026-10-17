/// Expands a `title_format` (`None` = `%title`). Unknown `%x` stay literal.
/// With `show_marks`, each mark not starting with `_` is appended as ` [mark]`.
pub fn format_title(
    format: Option<&str>,
    title: &str,
    class: &str,
    instance: &str,
    machine: &str,
    marks: &[String],
    show_marks: bool,
) -> String {
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
                out.push_str(value);
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
            out.push_str(m);
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
        assert_eq!(format_title(None, "T", "c", "i", "PC", &[], true), "T");
        assert_eq!(
            format_title(
                Some("%title (%class/%instance) @%machine %x"),
                "T",
                "c",
                "i",
                "PC",
                &[],
                true
            ),
            "T (c/i) @PC %x"
        );
        let marks = vec!["a".to_string(), "_hidden".to_string(), "b".to_string()];
        assert_eq!(
            format_title(None, "T", "c", "i", "PC", &marks, true),
            "T [a] [b]"
        );
        assert_eq!(format_title(None, "T", "c", "i", "PC", &marks, false), "T");
    }
}
