//! tztime / time: strftime with optional IANA timezone.

use crate::i3status_conf::{get, Props, Value};
use chrono::{DateTime, Utc};
use std::fmt::Write;

fn fmt_with<Tz: chrono::TimeZone>(dt: DateTime<Tz>, format: &str) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let mut out = String::new();
    // An invalid strftime spec makes Display fail; fall back to the raw format.
    if write!(out, "{}", dt.format(format)).is_err() {
        return format.to_string();
    }
    out
}

pub fn time_text(now_utc: DateTime<Utc>, props: &Props) -> String {
    let format = get(props, "format")
        .map(Value::as_str)
        .unwrap_or_else(|| "%Y-%m-%d %H:%M:%S".to_string());
    let tz = get(props, "timezone")
        .map(Value::as_str)
        .and_then(|n| n.parse::<chrono_tz::Tz>().ok());
    match tz {
        Some(tz) => fmt_with(now_utc.with_timezone(&tz), &format),
        None => fmt_with(now_utc.with_timezone(&chrono::Local), &format),
    }
}
