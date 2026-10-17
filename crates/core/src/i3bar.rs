//! i3bar status protocol: block parsing and click-event encoding.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// Largest pixel value accepted from a status line.
const MAX_PX: f64 = 10_000.0;

/// A JSON number as whole pixels: rounded and clamped to `0..=MAX_PX`.
fn px_of(v: &Value) -> Option<i32> {
    v.as_f64()
        .filter(|f| f.is_finite())
        .map(|f| f.round().clamp(0.0, MAX_PX) as i32)
}

/// Pixel fields accept any JSON number; anything else is ignored rather than
/// failing the whole status line.
fn lenient_px<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i32>, D::Error> {
    Ok(px_of(&Value::deserialize(d)?))
}

/// `min_width`: a number of pixels (any JSON number) or a text; other values are ignored.
fn lenient_min_width<'de, D: Deserializer<'de>>(d: D) -> Result<Option<MinWidth>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(t) => Some(MinWidth::Text(t)),
        v => px_of(&v).map(MinWidth::Px),
    })
}

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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_px"
    )]
    pub separator_block_width: Option<i32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_min_width"
    )]
    pub min_width: Option<MinWidth>,
    /// "left" (default), "center" or "right".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<String>,
    /// "pango" or "none".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markup: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_px"
    )]
    pub border_top: Option<i32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_px"
    )]
    pub border_right: Option<i32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_px"
    )]
    pub border_bottom: Option<i32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_px"
    )]
    pub border_left: Option<i32>,
}

impl Block {
    /// A block with i3 protocol defaults (`separator` true, all else unset).
    pub fn new(full_text: impl Into<String>) -> Block {
        Block {
            full_text: full_text.into(),
            separator: true,
            ..Block::default()
        }
    }
}

/// i3bar `min_width`: a pixel count, or a text whose rendered width is used.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MinWidth {
    Px(i32),
    Text(String),
}

/// A click event as sent to the status process (i3bar protocol).
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Click {
    pub name: Option<String>,
    pub instance: Option<String>,
    pub button: u8,
    pub modifiers: Vec<String>,
    pub x: i32,
    pub y: i32,
    pub relative_x: i32,
    pub relative_y: i32,
    pub output_x: i32,
    pub output_y: i32,
    pub width: i32,
    pub height: i32,
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

    pub fn encode(&mut self, c: &Click) -> String {
        let obj = serde_json::to_string(c).unwrap_or_else(|_| "{}".into());
        let prefix = if self.started { "," } else { "[\n" };
        self.started = true;
        format!("{prefix}{obj}\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_new_has_i3_defaults() {
        let b = Block::new("hi");
        assert_eq!(b.full_text, "hi");
        assert!(b.separator);
        assert!(!b.urgent);
        assert_eq!(b.color, None);
        assert_eq!(b.min_width, None);
    }

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
        let c = Click {
            name: Some("cpu".into()),
            button: 1,
            x: 10,
            y: 5,
            ..Default::default()
        };
        assert!(w.encode(&c).starts_with("[\n{"));
        assert!(w.encode(&Click::default()).starts_with(",{"));
    }

    #[test]
    fn new_block_fields_parse() {
        let mut p = StatusParser::new();
        p.feed_line(r#"{"version":1}"#);
        p.feed_line("[");
        let b = p.feed_line(r#"[{"full_text":"x","min_width":"100%","align":"center","markup":"pango","border_top":2},{"full_text":"y","min_width":50}]"#).unwrap();
        assert_eq!(b[0].min_width, Some(MinWidth::Text("100%".into())));
        assert_eq!(b[0].align.as_deref(), Some("center"));
        assert_eq!(b[0].markup.as_deref(), Some("pango"));
        assert_eq!(b[0].border_top, Some(2));
        assert_eq!(b[1].min_width, Some(MinWidth::Px(50)));
    }

    #[test]
    fn click_json_has_i3_fields() {
        let mut w = ClickWriter::new();
        let c = Click {
            name: Some("cpu".into()),
            instance: None,
            button: 1,
            modifiers: vec!["Shift".into()],
            x: 10,
            y: 20,
            relative_x: 3,
            relative_y: 4,
            output_x: 10,
            output_y: 5,
            width: 50,
            height: 18,
        };
        let s = w.encode(&c);
        assert!(s.starts_with("[\n"));
        let v: serde_json::Value =
            serde_json::from_str(s.trim_start_matches("[\n").trim()).unwrap();
        for k in [
            "name",
            "instance",
            "button",
            "modifiers",
            "x",
            "y",
            "relative_x",
            "relative_y",
            "output_x",
            "output_y",
            "width",
            "height",
        ] {
            assert!(v.get(k).is_some(), "missing {k}");
        }
        assert!(w.encode(&c).starts_with(','));
    }

    #[test]
    fn click_deserializes_with_missing_fields() {
        let c: Click = serde_json::from_str(r#"{"name":"a","button":3}"#).unwrap();
        assert_eq!(c.name.as_deref(), Some("a"));
        assert_eq!(c.button, 3);
        assert!(c.modifiers.is_empty());
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

    #[test]
    fn numeric_fields_accept_floats() {
        let mut p = StatusParser::new();
        p.feed_line(r#"{"version":1}"#);
        p.feed_line("[");
        let b = p
            .feed_line(r#"[{"full_text":"x","min_width":50.5,"border_top":1.0,"separator_block_width":9.7}]"#)
            .unwrap();
        assert_eq!(b[0].min_width, Some(MinWidth::Px(51)));
        assert_eq!(b[0].border_top, Some(1));
        assert_eq!(b[0].separator_block_width, Some(10));
    }

    #[test]
    fn numeric_fields_clamp_and_ignore_junk() {
        let mut p = StatusParser::new();
        p.feed_line(r#"{"version":1}"#);
        p.feed_line("[");
        let b = p
            .feed_line(r#"[{"full_text":"x","min_width":true,"border_left":-3,"border_right":1e9,"border_bottom":"2","separator_block_width":null}]"#)
            .unwrap();
        assert_eq!(b[0].min_width, None);
        assert_eq!(b[0].border_left, Some(0));
        assert_eq!(b[0].border_right, Some(10_000));
        assert_eq!(b[0].border_bottom, None);
        assert_eq!(b[0].separator_block_width, None);
    }
}
