//! path_exists and read_file modules.

use crate::format::{substitute, Level};
use crate::i3status_conf::{get, Props, Value};

fn text_of(props: &Props, key: &str, default: &str) -> String {
    get(props, key)
        .map(Value::as_str)
        .unwrap_or_else(|| default.to_string())
}

pub fn path_exists_text(exists: bool, title: &str, props: &Props) -> (String, Level) {
    let format = if !exists && get(props, "format_down").is_some() {
        text_of(props, "format_down", "")
    } else {
        text_of(props, "format", "%title: %status")
    };
    let vars = [
        ("title", title.to_string()),
        ("status", if exists { "yes" } else { "no" }.to_string()),
    ];
    let level = if exists { Level::Good } else { Level::Bad };
    (substitute(&format, &vars), level)
}

pub fn read_file_text(content: Option<&str>, title: &str, props: &Props) -> (String, Level) {
    let Some(content) = content else {
        let vars = [("title", title.to_string())];
        let format = text_of(props, "format_bad", "%title: missing");
        return (substitute(&format, &vars), Level::Bad);
    };
    let max = get(props, "max_characters")
        .and_then(Value::as_f64)
        .map(|n| n.max(0.0) as usize)
        .unwrap_or(255);
    let line: String = content
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(max)
        .collect();
    let vars = [("title", title.to_string()), ("content", line)];
    let format = text_of(props, "format", "%title: %content");
    (substitute(&format, &vars), Level::Normal)
}

pub fn path_exists(path: &str) -> bool {
    std::path::Path::new(path).exists()
}

/// Reads at most the first 64 KiB of the file as lossy UTF-8.
pub fn read_file(path: &str) -> Option<String> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(64 * 1024)
        .read_to_end(&mut buf)
        .ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}
