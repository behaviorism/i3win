//! battery: GetSystemPowerStatus.

use crate::format::{substitute, Level};
use crate::i3status_conf::{get, Props, Value};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatStatus {
    Charging,
    Discharging,
    Full,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatteryState {
    pub percent: u8,
    pub status: BatStatus,
    pub seconds_left: Option<u64>,
}

fn hhmm(secs: u64) -> String {
    format!("{:02}:{:02}", secs / 3600, secs % 3600 / 60)
}

pub fn battery_text(state: Option<BatteryState>, props: &Props) -> (String, Level) {
    let text_of = |key: &str, default: &str| {
        get(props, key)
            .map(Value::as_str)
            .unwrap_or_else(|| default.to_string())
    };
    let Some(s) = state else {
        return (text_of("format_down", "No battery"), Level::Normal);
    };
    let status = match s.status {
        BatStatus::Charging => text_of("status_chr", "CHR"),
        BatStatus::Discharging => text_of("status_bat", "BAT"),
        BatStatus::Full => text_of("status_full", "FULL"),
        BatStatus::Unknown => text_of("status_unk", "UNK"),
    };
    let remaining = s.seconds_left.map(hhmm).unwrap_or_default();
    let emptytime = match (s.status, s.seconds_left) {
        (BatStatus::Discharging, Some(secs)) => {
            let at = chrono::Local::now() + chrono::Duration::seconds(secs as i64);
            at.format("%H:%M").to_string()
        }
        _ => String::new(),
    };
    let low = get(props, "low_threshold")
        .and_then(Value::as_f64)
        .unwrap_or(10.0);
    let by_time = text_of("threshold_type", "percentage") == "time";
    let is_low = if by_time {
        s.seconds_left.is_some_and(|t| (t as f64) / 60.0 < low)
    } else {
        (s.percent as f64) < low
    };
    let level = if s.status == BatStatus::Discharging && is_low {
        Level::Bad
    } else {
        Level::Normal
    };
    let format = text_of("format", "%status %percentage %remaining");
    let text = substitute(
        &format,
        &[
            ("status", status),
            ("percentage", format!("{}%", s.percent)),
            ("remaining", remaining),
            ("emptytime", emptytime),
        ],
    );
    (text, level)
}

pub fn read_battery() -> Option<BatteryState> {
    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut s).ok()? };
    if s.BatteryFlag & 128 != 0 || s.BatteryFlag == 255 || s.BatteryLifePercent == 255 {
        return None;
    }
    let status = if s.BatteryFlag & 8 != 0 {
        BatStatus::Charging
    } else if s.ACLineStatus == 1 && s.BatteryLifePercent >= 100 {
        BatStatus::Full
    } else if s.ACLineStatus == 0 {
        BatStatus::Discharging
    } else {
        BatStatus::Unknown
    };
    let seconds_left = (s.BatteryLifeTime != u32::MAX).then_some(s.BatteryLifeTime as u64);
    Some(BatteryState {
        percent: s.BatteryLifePercent,
        status,
        seconds_left,
    })
}
