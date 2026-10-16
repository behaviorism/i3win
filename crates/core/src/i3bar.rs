//! i3bar status protocol: block parsing and click-event encoding.

use serde::{Deserialize, Serialize};

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Block {
    pub full_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    #[serde(default)]
    pub urgent: bool,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub separator: bool,
    /// Default 9 when drawing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub separator_block_width: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ParserMode {
    Start,
    Plain,
    Json,
}

pub struct StatusParser {
    mode: ParserMode,
    pub click_events: bool,
}

impl Default for StatusParser {
    fn default() -> Self {
        Self::new()
    }
}

impl StatusParser {
    pub fn new() -> Self {
        StatusParser {
            mode: ParserMode::Start,
            click_events: false,
        }
    }

    /// Feeds one line of status output. Returns the new block set, if the
    /// line carried one. Never panics, whatever the input.
    pub fn feed_line(&mut self, line: &str) -> Option<Vec<Block>> {
        if self.mode == ParserMode::Start {
            let header = line.trim();
            let parsed = if header.starts_with('{') {
                serde_json::from_str::<serde_json::Value>(header).ok()
            } else {
                None
            };
            match parsed {
                Some(v)
                    if v.as_object().is_some_and(|o| {
                        o.get("version").is_some_and(|x| x.is_i64() || x.is_u64())
                    }) =>
                {
                    self.click_events = v
                        .get("click_events")
                        .and_then(|c| c.as_bool())
                        .unwrap_or(false);
                    self.mode = ParserMode::Json;
                    return None;
                }
                _ => self.mode = ParserMode::Plain,
            }
        }
        match self.mode {
            ParserMode::Json => {
                let t = line.trim();
                if t == "[" {
                    return None;
                }
                let t = t.strip_prefix(',').unwrap_or(t).trim();
                serde_json::from_str::<Vec<Block>>(t).ok()
            }
            _ => Some(vec![Block {
                full_text: line.trim_end().to_string(),
                separator: true,
                ..Default::default()
            }]),
        }
    }
}

pub struct ClickWriter {
    started: bool,
}

impl Default for ClickWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl ClickWriter {
    pub fn new() -> Self {
        ClickWriter { started: false }
    }

    pub fn encode(
        &mut self,
        name: Option<&str>,
        instance: Option<&str>,
        button: u8,
        x: i32,
        y: i32,
    ) -> String {
        let obj = serde_json::json!({
            "name": name,
            "instance": instance,
            "button": button,
            "x": x,
            "y": y,
        });
        let prefix = if self.started { "," } else { "[\n" };
        self.started = true;
        format!("{prefix}{obj}\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_mode() {
        let mut p = StatusParser::new();
        assert_eq!(p.feed_line("hello\r").unwrap()[0].full_text, "hello");
        assert_eq!(p.feed_line("world").unwrap()[0].full_text, "world");
    }

    #[test]
    fn json_mode_with_header() {
        let mut p = StatusParser::new();
        assert!(p
            .feed_line(r#"{"version":1,"click_events":true}"#)
            .is_none());
        assert!(p.click_events);
        assert!(p.feed_line("[").is_none());
        let b = p
            .feed_line(r##"[{"full_text":"CPU 5%","color":"#ff0000","name":"cpu","extra":1}]"##)
            .unwrap();
        assert_eq!(b[0].color.as_deref(), Some("#ff0000"));
        assert!(b[0].separator);
        let b = p
            .feed_line(r#",[{"full_text":"a","separator":false},{"full_text":"b"}]"#)
            .unwrap();
        assert_eq!(b.len(), 2);
        assert!(!b[0].separator);
    }

    #[test]
    fn bad_lines_keep_going() {
        let mut p = StatusParser::new();
        p.feed_line(r#"{"version":1}"#);
        p.feed_line("[");
        assert!(p.feed_line(r#",[{"full_text":"#).is_none());
        assert!(p.feed_line("garbage").is_none());
        assert_eq!(
            p.feed_line(r#",[{"full_text":"ok"}]"#).unwrap()[0].full_text,
            "ok"
        );
    }

    #[test]
    fn click_writer_framing() {
        let mut w = ClickWriter::new();
        assert_eq!(
            w.encode(Some("cpu"), None, 1, 10, 5),
            "[\n{\"button\":1,\"instance\":null,\"name\":\"cpu\",\"x\":10,\"y\":5}\n"
        );
        assert!(w.encode(None, None, 3, 0, 0).starts_with(",{"));
    }

    #[test]
    fn block_serializes_compactly() {
        let b = Block {
            full_text: "x".into(),
            separator: true,
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_string(&b).unwrap(),
            r#"{"full_text":"x","urgent":false}"#
        );
    }

    #[test]
    fn arbitrary_input_never_panics() {
        let mut p = StatusParser::new();
        let huge = "é".repeat(200_000);
        for l in [
            "",
            "{",
            "{\"version\":\"x\"}",
            "日本語",
            "\u{0}",
            &huge,
            ",",
            ",,",
            "[[[",
        ] {
            let _ = p.feed_line(l);
        }
        let mut p = StatusParser::new();
        p.feed_line(r#"{"version":1}"#);
        for l in ["", "日本語,", &huge, ",", ",,[", "]", "null", "[1,2]"] {
            let _ = p.feed_line(l);
        }
    }
}
