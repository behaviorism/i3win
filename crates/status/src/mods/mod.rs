//! Built-in i3status modules.

pub mod battery;
pub mod cpu;
pub mod disk;
pub mod files;
pub mod memory;
pub mod net;
pub mod time;
pub mod volume;

use crate::format::{colors_from, make_block, Level};
use crate::i3status_conf::{get, ModuleRef, Props, Value};
use i3win_core::i3bar::Block;

pub use battery::battery_text;
pub use cpu::cpu_text;
pub use disk::disk_text;
pub use files::{path_exists_text, read_file_text};
pub use memory::memory_text;
pub use net::{ethernet_text, pick_adapter, wireless_text, Adapter};
pub use time::time_text;
pub use volume::volume_text;

pub trait Module: Send {
    /// Module name for the block's `name`.
    fn name(&self) -> &str;
    fn instance(&self) -> Option<&str>;
    /// Per-block `interval` (seconds) override.
    fn interval(&self) -> Option<f64>;
    /// Reads the system and formats.
    fn render(&mut self) -> Block;
}

enum Kind {
    Cpu(Option<(u64, u64, u64)>),
    Memory,
    Disk(String),
    Battery,
    Time,
    Ethernet(String),
    Wireless(String),
    Volume,
    PathExists(String, String),
    ReadFile(String, String),
}

struct Builtin {
    name: String,
    instance: Option<String>,
    props: Props,
    general: Props,
    kind: Kind,
}

impl Module for Builtin {
    fn name(&self) -> &str {
        &self.name
    }

    fn instance(&self) -> Option<&str> {
        self.instance.as_deref()
    }

    fn interval(&self) -> Option<f64> {
        get(&self.props, "interval").and_then(Value::as_f64)
    }

    fn render(&mut self) -> Block {
        let props = &self.props;
        let (text, level) = match &mut self.kind {
            Kind::Cpu(prev) => {
                let now = cpu::read_cpu_times();
                let usage = match (*prev, now) {
                    (Some(p), Some(n)) => cpu::cpu_percent(p, n),
                    _ => 0.0,
                };
                *prev = now;
                cpu_text(usage, props)
            }
            Kind::Memory => match memory::read_memory() {
                Some((total, avail)) => memory_text(total, avail, props),
                None => ("memory: error".to_string(), Level::Bad),
            },
            Kind::Disk(root) => match disk::read_disk(root) {
                Some((total, free, avail)) => disk_text(total, free, avail, props),
                None => (format!("{root}: error"), Level::Bad),
            },
            Kind::Battery => battery_text(battery::read_battery(), props),
            Kind::Time => (time_text(chrono::Utc::now(), props), Level::Normal),
            Kind::Ethernet(wanted) => {
                let list = net::read_adapters();
                let adapters: Vec<Adapter> = list.into_iter().map(|(a, _)| a).collect();
                ethernet_text(pick_adapter(&adapters, wanted, false), props)
            }
            Kind::Wireless(wanted) => {
                let list = net::read_adapters();
                let adapters: Vec<Adapter> = list.iter().map(|(a, _)| a.clone()).collect();
                let picked = pick_adapter(&adapters, wanted, true);
                let wifi = picked
                    .and_then(|p| adapters.iter().position(|a| std::ptr::eq(a, p)))
                    .and_then(|i| net::read_wifi(&list[i].1));
                wireless_text(
                    picked,
                    wifi.as_ref().map(|w| w.0.as_str()),
                    wifi.as_ref().map(|w| w.1),
                    props,
                )
            }
            Kind::Volume => volume_text(volume::read_volume(), props),
            Kind::PathExists(path, title) => {
                path_exists_text(files::path_exists(path), title, props)
            }
            Kind::ReadFile(path, title) => {
                read_file_text(files::read_file(path).as_deref(), title, props)
            }
        };
        let colors = colors_from(&self.general, Some(&self.props));
        make_block(
            &self.name,
            self.instance.as_deref(),
            text,
            level,
            &colors,
            &self.props,
        )
    }
}

struct Unsupported {
    name: String,
    instance: Option<String>,
}

impl Module for Unsupported {
    fn name(&self) -> &str {
        &self.name
    }

    fn instance(&self) -> Option<&str> {
        self.instance.as_deref()
    }

    fn interval(&self) -> Option<f64> {
        None
    }

    fn render(&mut self) -> Block {
        let mut b = Block::new(format!("unsupported: {}", self.name));
        b.name = Some(self.name.clone());
        b.instance = self.instance.clone();
        b.color = Some("#FF0000".to_string());
        b
    }
}

fn wanted_adapter(m: &ModuleRef) -> String {
    m.instance.clone().unwrap_or_else(|| "_first_".to_string())
}

pub fn build_module(m: &ModuleRef, props: &Props, general: &Props) -> Box<dyn Module> {
    let kind = match m.module.as_str() {
        "cpu_usage" => Kind::Cpu(cpu::read_cpu_times()),
        "memory" => Kind::Memory,
        "disk" => {
            // i3status takes a mount path; on Windows accept a drive root, else C:.
            let root = match m.instance.as_deref() {
                Some(i) if i.len() >= 2 && i.as_bytes()[1] == b':' => {
                    format!("{}\\", &i[..2])
                }
                _ => "C:\\".to_string(),
            };
            Kind::Disk(root)
        }
        "battery" => Kind::Battery,
        "tztime" | "time" => Kind::Time,
        "ethernet" => Kind::Ethernet(wanted_adapter(m)),
        "wireless" => Kind::Wireless(wanted_adapter(m)),
        "volume" => Kind::Volume,
        "path_exists" | "read_file" => {
            let inst = m.instance.clone().unwrap_or_default();
            let path = get(props, "path")
                .map(Value::as_str)
                .unwrap_or_else(|| inst.clone());
            let title = get(props, "title").map(Value::as_str).unwrap_or(inst);
            if m.module == "path_exists" {
                Kind::PathExists(path, title)
            } else {
                Kind::ReadFile(path, title)
            }
        }
        _ => {
            return Box::new(Unsupported {
                name: m.module.clone(),
                instance: m.instance.clone(),
            })
        }
    };
    Box::new(Builtin {
        name: m.module.clone(),
        instance: m.instance.clone(),
        props: props.clone(),
        general: general.clone(),
        kind,
    })
}

#[cfg(test)]
pub(crate) mod testutil {
    use crate::i3status_conf::{Props, Value};

    pub fn p(kv: &[(&str, &str)]) -> Props {
        kv.iter()
            .map(|(k, v)| (k.to_string(), Value::Str(v.to_string())))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::battery::{BatStatus, BatteryState};
    use super::testutil::p;
    use super::*;

    fn ad(name: &str, up: bool, wl: bool, ip: Option<&str>) -> Adapter {
        Adapter {
            name: name.into(),
            up,
            wireless: wl,
            ipv4: ip.map(Into::into),
            speed_bps: 1_000_000_000,
        }
    }

    #[test]
    fn adapter_pick_and_text() {
        let l = vec![
            ad("Ethernet", false, false, None),
            ad("Ethernet 2", true, false, Some("10.0.0.2")),
            ad("Wi-Fi", true, true, Some("192.168.1.5")),
        ];
        assert_eq!(
            pick_adapter(&l, "_first_", false).unwrap().name,
            "Ethernet 2"
        );
        assert_eq!(pick_adapter(&l, "wi-fi", true).unwrap().name, "Wi-Fi");
        assert_eq!(
            ethernet_text(pick_adapter(&l, "_first_", false), &p(&[])),
            ("E: 10.0.0.2 (1000 Mbit/s)".into(), Level::Good)
        );
        assert_eq!(ethernet_text(None, &p(&[])), ("E: down".into(), Level::Bad));
        assert_eq!(
            wireless_text(
                pick_adapter(&l, "_first_", true),
                Some("home"),
                Some(70),
                &p(&[])
            )
            .0,
            "W: (70% at home) 192.168.1.5"
        );
        assert_eq!(wireless_text(None, None, None, &p(&[])).0, "W: down");
    }

    #[test]
    fn volume_and_files() {
        assert_eq!(
            volume_text(Some((42, false)), &p(&[("format", "V %volume")])).0,
            "V 42%"
        );
        assert_eq!(
            volume_text(Some((42, true)), &p(&[("format_muted", "M")])).0,
            "M"
        );
        assert_eq!(volume_text(None, &p(&[])).1, Level::Degraded);
        assert_eq!(
            path_exists_text(true, "VPN", &p(&[])),
            ("VPN: yes".into(), Level::Good)
        );
        assert_eq!(
            path_exists_text(false, "VPN", &p(&[("format_down", "VPN down")])),
            ("VPN down".into(), Level::Bad)
        );
        assert_eq!(
            read_file_text(
                Some(
                    "hello
world"
                ),
                "f",
                &p(&[("max_characters", "3")])
            )
            .0,
            "f: hel"
        );
        assert_eq!(read_file_text(None, "f", &p(&[])).1, Level::Bad);
    }

    #[test]
    fn cpu_thresholds() {
        assert_eq!(
            cpu_text(12.4, &p(&[("format", "CPU %usage")])),
            ("CPU 12%".into(), Level::Normal)
        );
        assert_eq!(cpu_text(91.0, &p(&[])).1, Level::Degraded);
        assert_eq!(cpu_text(96.0, &p(&[])).1, Level::Bad);
        assert_eq!(
            cpu_text(80.0, &p(&[("degraded_threshold", "75")])).1,
            Level::Degraded
        );
    }

    #[test]
    fn cpu_math() {
        assert!((cpu::cpu_percent((100, 300, 100), (150, 400, 200)) - 75.0).abs() < 1e-9);
        assert_eq!(cpu::cpu_percent((1, 1, 1), (1, 1, 1)), 0.0);
    }

    #[test]
    fn memory_and_disk() {
        let g = 1u64 << 30;
        assert_eq!(
            memory_text(
                8 * g,
                2 * g,
                &p(&[("format", "%percentage_used %available")])
            )
            .0,
            "75.0% 2.0 GiB"
        );
        assert_eq!(
            memory_text(8 * g, g / 2, &p(&[("threshold_critical", "10%")])).1,
            Level::Bad
        );
        assert_eq!(
            memory_text(8 * g, 2 * g, &p(&[("threshold_degraded", "3G")])).1,
            Level::Degraded
        );
        assert_eq!(memory_text(8 * g, 2 * g, &p(&[])).0, "6.0 GiB / 8.0 GiB");
        assert_eq!(
            disk_text(
                100 * g,
                40 * g,
                40 * g,
                &p(&[("format", "%percentage_free")])
            )
            .0,
            "40.0%"
        );
        assert_eq!(
            disk_text(100 * g, 4 * g, 4 * g, &p(&[("low_threshold", "5")])).1,
            Level::Bad
        );
    }

    #[test]
    fn battery_down_and_states() {
        assert_eq!(battery_text(None, &p(&[])).0, "No battery");
        assert_eq!(battery_text(None, &p(&[("format_down", "")])).0, "");
        let s = BatteryState {
            percent: 7,
            status: BatStatus::Discharging,
            seconds_left: Some(3660),
        };
        assert_eq!(
            battery_text(Some(s.clone()), &p(&[])),
            ("BAT 7% 01:01".into(), Level::Bad)
        );
        let c = BatteryState {
            percent: 50,
            status: BatStatus::Charging,
            seconds_left: None,
        };
        assert_eq!(
            battery_text(Some(c), &p(&[("format", "%status %percentage")])).0,
            "CHR 50%"
        );
    }

    #[test]
    fn time_with_timezone() {
        use chrono::TimeZone;
        let t = chrono::Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap();
        assert_eq!(
            time_text(t, &p(&[("format", "%H:%M"), ("timezone", "Europe/Zagreb")])),
            "13:00"
        );
        assert_eq!(
            time_text(
                t,
                &p(&[("format", "%H:%M"), ("timezone", "America/New_York")])
            ),
            "07:00"
        );
    }

    #[test]
    fn unknown_module_is_red_block() {
        let mut m = build_module(
            &ModuleRef {
                module: "load".into(),
                instance: None,
            },
            &vec![],
            &vec![],
        );
        let b = m.render();
        assert_eq!(b.full_text, "unsupported: load");
        assert_eq!(b.color.as_deref(), Some("#FF0000"));
    }

    #[test]
    fn builtin_modules_render() {
        for name in [
            "cpu_usage",
            "memory",
            "disk",
            "battery",
            "tztime",
            "ethernet",
            "wireless",
            "volume",
            "path_exists",
            "read_file",
        ] {
            let mut m = build_module(
                &ModuleRef {
                    module: name.into(),
                    instance: None,
                },
                &vec![],
                &vec![],
            );
            assert_eq!(m.render().name.as_deref(), Some(name));
        }
    }
}
