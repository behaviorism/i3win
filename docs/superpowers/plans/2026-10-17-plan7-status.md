# Plan 7: i3win-status (i3status and i3blocks configs) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn `i3win-status` from a fixed five-block printer into a configurable status generator that reads either an i3status-style config (built-in modules) or an i3blocks config (custom command blocks plus built-in modules), with clicks and signals.

**Architecture:** All parsing, formatting, expansion and scheduling decisions are pure functions in the `i3win-status` crate (`crates/status`), unit-tested; Win32 readers (CPU, memory, disk, battery, network, Wi-Fi, volume) and process spawning are thin. One writer thread prints i3bar lines; each command block has its own reader thread.

**Tech Stack:** Rust 2021; `windows` 0.58 (features added: `Win32_NetworkManagement_IpHelper`, `Win32_NetworkManagement_WiFi`, `Win32_Networking_WinSock`, `Win32_Media_Audio`, `Win32_Media_Audio_Endpoints`, `Win32_System_Com`, `Win32_UI_WindowsAndMessaging`, `Win32_System_JobObjects`, `Win32_System_Pipes`); `chrono`, `chrono-tz` (new), `serde_json`, `i3win-core` (for `Block`).

**Spec:** `docs/superpowers/specs/2026-10-17-i3win-v2-design.md` (sections "Status: i3win-status", "i3blocks config (custom blocks)").

## Global Constraints

- Same as Plan 5's (pure where possible, `windows` 0.58, no crash on bad config, clippy/fmt/tests green, commit trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`).
- Config discovery order (first existing wins): `-c PATH` (format detected from content) → `I3BLOCKS_CONFIG` → `%USERPROFILE%\.config\i3win\i3blocks.conf` → `%USERPROFILE%\.config\i3blocks\config` → `%USERPROFILE%\.i3blocks.conf` (i3blocks format) → `%USERPROFILE%\.config\i3win\i3status.conf` (i3status format). None exists → write `i3status.conf` with the default below and use it.
- Behavior of i3blocks features follows i3blocks 1.5 exactly where the spec says so: exit codes, no timeout, no overlapping runs, raw-format line order, persist clicks, env.
- i3blocks error text (from its source): `full_text` = `Error: Command 'CMD' not executable` (exit 126), `Error: Command 'CMD' not found or missing dependency` (127), `Error: Command 'CMD' exited unexpectedly with code N` (other nonzero except 33), plus `urgent: true`. CMD is the `command` value as written.
- Output: i3bar protocol header `{"version":1,"click_events":true}` then `[`, one JSON array per line, `,` prefix after the first. `output_format = "none"` prints plain text lines joined by `general.separator` (default `" | "`) and sends no header.
- Writer prints at most one line per 50 ms (coalesce updates).
- Never block forever on a child: readers are threads; on our exit, every child process tree dies (put children in a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`).

## Review Focus

1. A block whose command never exits must not stall other blocks or the writer (Task 7 test `slow_block_does_not_block_others`).
2. A config file with a parse error in the middle still shows every valid block plus a red `config error line N` block (Task 1 test `bad_block_skipped_and_reported`, Task 2 test `i3blocks_bad_line_reported`).
3. `$VAR` expansion under `cmd` must not expand inside single quotes and must keep `\$` literal (Task 6 test `expand_sh_rules`).
4. Clicks for a block that is still running are dropped, not queued, and don't spawn a second process (Task 7 test `click_while_running_is_dropped`).
5. A missing battery / network adapter / audio device shows the module's down text (i3status defaults `"No battery"`, `"E: down"`, `"W: down"`; volume `"♪: --"`), never a crash (Task 4 test `battery_down_and_states`, Task 5 tests `adapter_pick_and_text`, `volume_and_files`).

---

### Task 1: i3status config parser

**Files:** Create `crates/status/src/i3status_conf.rs`; modify `crates/status/src/main.rs` (`mod`)

**Interfaces:**
- Produces:

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum Value { Str(String), Num(f64), Bool(bool) }
impl Value { pub fn as_str(&self) -> String; pub fn as_f64(&self) -> Option<f64>; pub fn as_bool(&self) -> Option<bool> }
pub type Props = Vec<(String, Value)>;                    // order kept; later duplicate wins on lookup
pub fn get<'a>(p: &'a Props, key: &str) -> Option<&'a Value>
#[derive(Clone, Debug, PartialEq)]
pub struct ModuleRef { pub module: String, pub instance: Option<String> }   // "disk C:" -> module "disk", instance "C:"; "cpu_usage" -> instance None
#[derive(Clone, Debug, Default)]
pub struct I3StatusConfig { pub general: Props, pub order: Vec<ModuleRef>, pub blocks: Vec<(ModuleRef, Props)>, pub errors: Vec<(usize, String)> }
pub fn parse_i3status(text: &str) -> I3StatusConfig
pub fn module_props<'a>(c: &'a I3StatusConfig, m: &ModuleRef) -> Option<&'a Props>
pub const DEFAULT_I3STATUS_CONF: &str;   // see below
```

- Syntax: `#` comments (whole line, or after whitespace), blank lines, `general {`, `MODULE [INSTANCE] {` (INSTANCE may be quoted; inside quotes `\"` is an escaped quote and `\\` a backslash, except that a `\"` which is the last `"` on the line is a literal backslash followed by the closing quote, so `disk "C:\" {` gives `C:\`), `key = value` lines (value: quoted string, number, `true`/`false`, else bare string), `}`, `order += "MODULE [INSTANCE]"`. Errors (missing `}`, `key` without `=`, line outside a block) are recorded in `errors` with the 1-based line number; the offending block is skipped, parsing continues.
- `DEFAULT_I3STATUS_CONF` reproduces v1's output:

```
general {
        colors = true
        interval = 1
}

order += "cpu_usage"
order += "memory"
order += "disk C:"
order += "battery all"
order += "tztime local"

cpu_usage {
        format = "CPU %usage"
}
memory {
        format = "MEM %percentage_used"
}
disk "C:" {
        format = "C: %avail"
        prefix_type = binary
}
battery all {
        format = "BAT %percentage"
        format_down = ""
}
tztime local {
        format = "%Y-%m-%d %H:%M:%S"
}
```

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn parses_general_order_blocks() {
    let c = parse_i3status("general {\n colors = true\n interval = 5\n color_good = \"#00FF00\"\n}\norder += \"disk C:\"\norder += \"cpu_usage\"\n# c\ndisk \"C:\" {\n format = \"%free\" # trailing\n}\ncpu_usage {\n max_threshold = 90\n}\n");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    assert_eq!(get(&c.general, "interval"), Some(&Value::Num(5.0)));
    assert_eq!(get(&c.general, "colors"), Some(&Value::Bool(true)));
    assert_eq!(c.order, vec![ModuleRef { module: "disk".into(), instance: Some("C:".into()) }, ModuleRef { module: "cpu_usage".into(), instance: None }]);
    let d = module_props(&c, &c.order[0]).unwrap();
    assert_eq!(get(d, "format").unwrap().as_str(), "%free");
}
#[test]
fn quoted_instance_with_trailing_backslash() {
    let c = parse_i3status("disk \"C:\\\" {\n format = \"x\"\n}\norder += \"disk C:\\\"\n");
    assert_eq!(c.blocks[0].0.instance.as_deref(), Some("C:\\"));
}
#[test]
fn bad_block_skipped_and_reported() {
    let c = parse_i3status("cpu_usage {\n format\n}\nmemory {\n format = \"m\"\n}\n");
    assert_eq!(c.errors.len(), 1);
    assert_eq!(c.errors[0].0, 2);
    assert!(module_props(&c, &ModuleRef { module: "memory".into(), instance: None }).is_some());
}
#[test]
fn default_conf_parses_cleanly() {
    let c = parse_i3status(DEFAULT_I3STATUS_CONF);
    assert!(c.errors.is_empty());
    assert_eq!(c.order.len(), 5);
}
```

- [ ] **Step 2–4:** `cargo test -p i3win-status` FAIL → implement → PASS.
- [ ] **Step 5: Commit** `feat(status): i3status config parser`.

---

### Task 2: i3blocks config parser, format detection, discovery

**Files:** Create `crates/status/src/i3blocks_conf.rs`, `crates/status/src/discover.rs`

**Interfaces:**
- Produces:

```rust
// i3blocks_conf.rs
#[derive(Clone, Debug, PartialEq)]
pub struct Section { pub name: String, pub props: Vec<(String, String)> }   // props = globals first, then own (own override on lookup); keys case-sensitive
impl Section { pub fn get(&self, k: &str) -> Option<&str> }                 // last occurrence wins
#[derive(Clone, Debug, Default)]
pub struct I3BlocksConfig { pub sections: Vec<Section>, pub errors: Vec<(usize, String)> }
pub fn parse_i3blocks(text: &str) -> I3BlocksConfig
// discover.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format { I3Status, I3Blocks }
pub fn detect_format(text: &str) -> Format     // first non-comment, non-blank line: starts with '[' or is "key=value" with no '{' -> I3Blocks; else I3Status
pub fn discover(cli: Option<&Path>, env_i3blocks: Option<&str>, home: &Path, exists: &dyn Fn(&Path) -> bool) -> (PathBuf, Option<Format>)
//   returns the path and Some(format) when the location fixes it; for -c PATH returns None (caller detects from content).
//   When nothing exists, returns (home/.config/i3win/i3status.conf, Some(I3Status)) — the caller writes the default there.
```

- i3blocks syntax (1.5): lines `[name]` start a section; `key=value` (whitespace around `=` trimmed; value is the rest of the line verbatim, no quote processing); `#` comment lines; blank lines. `key=value` before any section = global, prepended to every section's props. A line that is neither → error with line number, skipped.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn globals_and_sections() {
    let c = parse_i3blocks("# x\nseparator_block_width=15\ninterval=5\n\n[cpu_usage]\nformat=CPU %usage\n\n[weather]\ncommand=curl -s wttr.in/?format=1\ninterval=600\nDISK=C:\n");
    assert!(c.errors.is_empty());
    assert_eq!(c.sections.len(), 2);
    assert_eq!(c.sections[1].get("interval"), Some("600"));
    assert_eq!(c.sections[0].get("interval"), Some("5"));
    assert_eq!(c.sections[1].get("separator_block_width"), Some("15"));
    assert_eq!(c.sections[1].get("DISK"), Some("C:"));
    assert_eq!(c.sections[1].get("command"), Some("curl -s wttr.in/?format=1"));
}
#[test]
fn i3blocks_bad_line_reported() {
    let c = parse_i3blocks("[a]\nfull_text=x\nnonsense line\n[b]\nfull_text=y\n");
    assert_eq!(c.errors, vec![(3, "expected key=value or [section]".to_string())]);
    assert_eq!(c.sections.len(), 2);
}
#[test]
fn detect_and_discover() {
    assert_eq!(detect_format("# c\n[cpu]\n"), Format::I3Blocks);
    assert_eq!(detect_format("interval=5\n"), Format::I3Blocks);
    assert_eq!(detect_format("general {\n}\n"), Format::I3Status);
    let home = Path::new("H");
    let only = |p: &'static str| move |q: &Path| q == Path::new(p);
    assert_eq!(discover(None, None, home, &only(r"H\.config\i3blocks\config")), (PathBuf::from(r"H\.config\i3blocks\config"), Some(Format::I3Blocks)));
    assert_eq!(discover(None, Some("E"), home, &|_| true), (PathBuf::from("E"), Some(Format::I3Blocks)));
    assert_eq!(discover(Some(Path::new("X")), Some("E"), home, &|_| true), (PathBuf::from("X"), None));
    assert_eq!(discover(None, None, home, &|_| false), (PathBuf::from(r"H\.config\i3win\i3status.conf"), Some(Format::I3Status)));
    assert_eq!(discover(None, None, home, &only(r"H\.config\i3win\i3blocks.conf")).1, Some(Format::I3Blocks));
}
```

- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5: Commit** `feat(status): i3blocks config parser and config discovery`.

---

### Task 3: Format engine and common block keys

**Files:** Create `crates/status/src/format.rs`

**Interfaces:**
- Produces:

```rust
pub fn substitute(format: &str, vars: &[(&str, String)]) -> String   // replaces %name tokens, longest names first; unknown %x stay; "%%" -> "%"
pub struct Colors { pub enabled: bool, pub good: String, pub degraded: String, pub bad: String }  // defaults: true, "#00FF00", "#FFFF00", "#FF0000"
pub fn colors_from(general: &Props, module: Option<&Props>) -> Colors   // module color_good/... override general
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Level { Normal, Good, Degraded, Bad }
pub fn make_block(name: &str, instance: Option<&str>, text: String, level: Level, colors: &Colors, module: &Props) -> Block
//   sets name/instance; color per level when colors.enabled (Normal = None); applies common keys from `module`:
//   min_width (number -> Px, else Text), align, separator (bool), separator_block_width, markup ("pango"/"none"), and
//   color overrides happen via colors_from. Uses i3win_core::i3bar::Block.
pub fn format_bytes(bytes: u64, prefix: &str /*"binary"|"decimal"|"custom"*/) -> String   // i3status style: "12.3 GiB" (binary), "13.2 GB" (decimal), one decimal, units B KiB MiB GiB TiB / B kB MB GB TB
```

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn substitute_longest_first() {
    let v = [("used", "1".to_string()), ("used_percent", "2".to_string())];
    assert_eq!(substitute("%used_percent %used %x 100%%", &v), "2 1 %x 100%");
}
#[test]
fn make_block_levels_and_common_keys() {
    let colors = Colors { enabled: true, good: "#0f0".into(), degraded: "#ff0".into(), bad: "#f00".into() };
    let props = vec![("min_width".into(), Value::Num(80.0)), ("align".into(), Value::Str("right".into())), ("separator".into(), Value::Bool(false))];
    let b = make_block("cpu_usage", None, "CPU 5%".into(), Level::Bad, &colors, &props);
    assert_eq!(b.color.as_deref(), Some("#f00"));
    assert_eq!(b.min_width, Some(MinWidth::Px(80)));
    assert_eq!(b.align.as_deref(), Some("right"));
    assert!(!b.separator);
    assert_eq!(b.name.as_deref(), Some("cpu_usage"));
    let off = Colors { enabled: false, ..colors };
    assert_eq!(make_block("x", None, "t".into(), Level::Bad, &off, &vec![]).color, None);
}
#[test]
fn bytes_formatting() {
    assert_eq!(format_bytes(1_610_612_736, "binary"), "1.5 GiB");
    assert_eq!(format_bytes(1_500_000_000, "decimal"), "1.5 GB");
    assert_eq!(format_bytes(512, "binary"), "512.0 B");
}
```

- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5: Commit** `feat(status): format engine and common block keys`.

---

### Task 4: Modules: cpu_usage, memory, disk, battery, tztime/time

**Files:** Create `crates/status/src/mods/mod.rs`, `mods/cpu.rs`, `mods/memory.rs`, `mods/disk.rs`, `mods/battery.rs`, `mods/time.rs`; move readers from `modules.rs` into them (delete `modules.rs` when empty); `Cargo.toml` adds `chrono-tz = "0.10"`

**Interfaces:**
- Produces:

```rust
pub trait Module: Send {
    fn name(&self) -> &str;                 // module name for the block's `name`
    fn instance(&self) -> Option<&str>;
    fn interval(&self) -> Option<f64>;      // per-block `interval` (seconds) override
    fn render(&mut self) -> Block;          // reads the system and formats
}
pub fn build_module(m: &ModuleRef, props: &Props, general: &Props) -> Box<dyn Module>   // unknown module -> a module that renders red "unsupported: NAME"
// pure formatters (tested), readers injected:
pub fn cpu_text(usage: f64, props: &Props) -> (String, Level)              // %usage = "NN%" (rounded integer); default format "%usage"
pub fn memory_text(total: u64, avail: u64, props: &Props) -> (String, Level)  // %used %free %available %total %percentage_used %percentage_free %percentage_available; threshold_degraded / threshold_critical: "10%" or "1G" free; default format "%used / %total"
pub fn disk_text(total: u64, free: u64, avail: u64, props: &Props) -> (String, Level) // %free %used %total %avail %percentage_free %percentage_used %percentage_avail; low_threshold + threshold_type (percentage_free|percentage_avail|bytes_free|bytes_avail; default percentage_avail) -> Bad; default format "%free"
pub fn battery_text(state: Option<BatteryState>, props: &Props) -> (String, Level)  // struct BatteryState { pub percent: u8, pub status: BatStatus, pub seconds_left: Option<u64> } enum BatStatus { Charging, Discharging, Full, Unknown }
//   %status (status_chr default "CHR", status_bat "BAT", status_full "FULL", status_unk "UNK"), %percentage ("NN%"), %remaining ("HH:MM"), %emptytime ("HH:MM" local time when empty)
//   low_threshold (default 10) + threshold_type percentage|time -> Bad when discharging; None -> format_down (default "No battery"); default format "%status %percentage %remaining"
pub fn time_text(now_utc: chrono::DateTime<chrono::Utc>, props: &Props) -> String   // `format` strftime (default "%Y-%m-%d %H:%M:%S"); `timezone` IANA name via chrono-tz, else local; tztime and time share it
```

- Thresholds follow i3status: cpu Degraded when ≥ `degraded_threshold` (default 90), Bad when ≥ `max_threshold` (default 95), else Normal. memory: Degraded when available < threshold_degraded, Bad when < threshold_critical (no default thresholds → Normal). disk: Bad when below `low_threshold`. battery: Bad when discharging and below threshold.
- Readers: v1's `read_cpu_times`, `read_mem_used_pct` (extend to return total/avail via `GlobalMemoryStatusEx`), `read_disk_free` (extend: `GetDiskFreeSpaceExW` total/free/avail), `read_battery` (extend: `GetSystemPowerStatus` BatteryLifeTime → seconds_left; `BatteryFlag & 128` = no battery → None; `& 8` charging; ACLineStatus=1 and 100% → Full).

- [ ] **Step 1: Failing tests**

```rust
fn p(kv: &[(&str, &str)]) -> Props { kv.iter().map(|(k, v)| (k.to_string(), Value::Str(v.to_string()))).collect() }
#[test]
fn cpu_thresholds() {
    assert_eq!(cpu_text(12.4, &p(&[("format", "CPU %usage")])), ("CPU 12%".into(), Level::Normal));
    assert_eq!(cpu_text(91.0, &p(&[])).1, Level::Degraded);
    assert_eq!(cpu_text(96.0, &p(&[])).1, Level::Bad);
    assert_eq!(cpu_text(80.0, &p(&[("degraded_threshold", "75")])).1, Level::Degraded);
}
#[test]
fn memory_and_disk() {
    let g = 1u64 << 30;
    assert_eq!(memory_text(8 * g, 2 * g, &p(&[("format", "%percentage_used %available")])).0, "75.0% 2.0 GiB");
    assert_eq!(memory_text(8 * g, g / 2, &p(&[("threshold_critical", "10%")])).1, Level::Bad);
    assert_eq!(disk_text(100 * g, 40 * g, 40 * g, &p(&[("format", "%percentage_free")])).0, "40.0%");
    assert_eq!(disk_text(100 * g, 4 * g, 4 * g, &p(&[("low_threshold", "5")])).1, Level::Bad);
}
#[test]
fn battery_down_and_states() {
    assert_eq!(battery_text(None, &p(&[])).0, "No battery");
    assert_eq!(battery_text(None, &p(&[("format_down", "")])).0, "");
    let s = BatteryState { percent: 7, status: BatStatus::Discharging, seconds_left: Some(3660) };
    assert_eq!(battery_text(Some(s.clone()), &p(&[])), ("BAT 7% 01:01".into(), Level::Bad));
    let c = BatteryState { percent: 50, status: BatStatus::Charging, seconds_left: None };
    assert_eq!(battery_text(Some(c), &p(&[("format", "%status %percentage")])).0, "CHR 50%");
}
#[test]
fn time_with_timezone() {
    use chrono::TimeZone;
    let t = chrono::Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap();
    assert_eq!(time_text(t, &p(&[("format", "%H:%M"), ("timezone", "Europe/Zagreb")])), "13:00");
    assert_eq!(time_text(t, &p(&[("format", "%H:%M"), ("timezone", "America/New_York")])), "07:00");
}
#[test]
fn unknown_module_is_red_block() {
    let mut m = build_module(&ModuleRef { module: "load".into(), instance: None }, &vec![], &vec![]);
    let b = m.render();
    assert_eq!(b.full_text, "unsupported: load");
    assert_eq!(b.color.as_deref(), Some("#FF0000"));
}
```

- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5: Commit** `feat(status): cpu, memory, disk, battery, time modules`.

---

### Task 5: Modules: ethernet, wireless, volume, path_exists, read_file

**Files:** Create `crates/status/src/mods/net.rs`, `mods/volume.rs`, `mods/files.rs`; `Cargo.toml` features

**Interfaces:**
- Produces:

```rust
pub struct Adapter { pub name: String /*FriendlyName*/, pub up: bool, pub wireless: bool, pub ipv4: Option<String>, pub speed_bps: u64 }
pub fn pick_adapter<'a>(list: &'a [Adapter], wanted: &str, wireless: bool) -> Option<&'a Adapter>  // "_first_" = first up adapter of that kind (skipping loopback/tunnel), else by FriendlyName (case-insensitive)
pub fn ethernet_text(a: Option<&Adapter>, props: &Props) -> (String, Level)   // %ip, %speed ("1000 Mbit/s"); default format_up "E: %ip (%speed)", format_down "E: down"; up = Good, down = Bad
pub fn wireless_text(a: Option<&Adapter>, essid: Option<&str>, quality: Option<u32>, props: &Props) -> (String, Level)  // %ip %essid %quality ("NN%"); default format_up "W: (%quality at %essid) %ip", format_down "W: down"
pub fn volume_text(level: Option<(u32 /*0-100*/, bool /*muted*/)>, props: &Props) -> (String, Level)  // %volume ("NN%"); format default "♪: %volume" (Normal); format_muted default "♪: muted (%volume)" (Degraded); no device -> "♪: --" (Degraded)
pub fn path_exists_text(exists: bool, title: &str, props: &Props) -> (String, Level) // %title, %status ("yes"/"no"); default format "%title: %status"; format_down if !exists and set; Good/Bad
pub fn read_file_text(content: Option<&str>, title: &str, props: &Props) -> (String, Level) // %title, %content (first line, truncated to max_characters, default 255); default format "%title: %content" (Normal); unreadable file -> format_bad, default "%title: missing" (Bad)
```

- Readers: `GetAdaptersAddresses(AF_UNSPEC, GAA_FLAG_SKIP_ANYCAST|MULTICAST|DNS_SERVER)` → name, `OperStatus == IfOperStatusUp`, `IfType == IF_TYPE_IEEE80211` (71) wireless, first IPv4 unicast, `TransmitLinkSpeed`; skip `IfType` 24 (loopback) and 131 (tunnel). Wi-Fi: `WlanOpenHandle`/`WlanEnumInterfaces`/`WlanQueryInterface(wlan_intf_opcode_current_connection)` → `dot11Ssid`, `wlanSignalQuality`; match the interface to the adapter by GUID (`AdapterName` is the GUID string). Volume: COM (`CoInitializeEx` MTA on the module's thread) `IMMDeviceEnumerator::GetDefaultAudioEndpoint(eRender, eConsole)` → `IAudioEndpointVolume::GetMasterVolumeLevelScalar`/`GetMute`; any failure → None. The `NAME` instance of `volume NAME` is ignored (always the default device; documented).

- [ ] **Step 1: Failing tests**

```rust
fn ad(name: &str, up: bool, wl: bool, ip: Option<&str>) -> Adapter { Adapter { name: name.into(), up, wireless: wl, ipv4: ip.map(Into::into), speed_bps: 1_000_000_000 } }
#[test]
fn adapter_pick_and_text() {
    let l = vec![ad("Ethernet", false, false, None), ad("Ethernet 2", true, false, Some("10.0.0.2")), ad("Wi-Fi", true, true, Some("192.168.1.5"))];
    assert_eq!(pick_adapter(&l, "_first_", false).unwrap().name, "Ethernet 2");
    assert_eq!(pick_adapter(&l, "wi-fi", true).unwrap().name, "Wi-Fi");
    assert_eq!(ethernet_text(pick_adapter(&l, "_first_", false), &p(&[])), ("E: 10.0.0.2 (1000 Mbit/s)".into(), Level::Good));
    assert_eq!(ethernet_text(None, &p(&[])), ("E: down".into(), Level::Bad));
    assert_eq!(wireless_text(pick_adapter(&l, "_first_", true), Some("home"), Some(70), &p(&[])).0, "W: (70% at home) 192.168.1.5");
    assert_eq!(wireless_text(None, None, None, &p(&[])).0, "W: down");
}
#[test]
fn volume_and_files() {
    assert_eq!(volume_text(Some((42, false)), &p(&[("format", "V %volume")])).0, "V 42%");
    assert_eq!(volume_text(Some((42, true)), &p(&[("format_muted", "M")])).0, "M");
    assert_eq!(path_exists_text(true, "VPN", &p(&[])), ("VPN: yes".into(), Level::Good));
    assert_eq!(read_file_text(Some("hello\nworld"), "f", &p(&[("max_characters", "3")])).0, "f: hel");
    assert_eq!(read_file_text(None, "f", &p(&[])).1, Level::Bad);
}
```

(`p` as in Task 4; put shared test helpers in `mods/mod.rs` `#[cfg(test)] pub(crate) mod testutil`.)

- [ ] **Step 2–4:** FAIL → implement → PASS; `cargo build -p i3win-status`.
- [ ] **Step 5: Commit** `feat(status): network, wireless, volume, path_exists, read_file modules`.

---

### Task 6: Command blocks: env, variable expansion, shell, output parsing, exit codes

**Files:** Create `crates/status/src/command_block.rs`

**Interfaces:**
- Produces (pure parts):

```rust
#[derive(Clone, Debug, PartialEq)] pub enum Interval { Secs(f64), Once, Repeat, Persist, None }
pub fn parse_interval(v: Option<&str>) -> Interval     // missing or "0" -> None (run at start, then only on click/signal); "once"; "repeat"; "persist"; N -> Secs
#[derive(Clone, Copy, Debug, PartialEq)] pub enum OutFormat { Raw, Json }
pub fn block_env(section: &Section, script_dir: &Path, click: Option<&Click>) -> Vec<(String, String)>
//   every prop (globals then own; own wins) as KEY=VALUE; SCRIPT_DIR; name=<section name>; BLOCK_NAME; instance & BLOCK_INSTANCE (from `instance` prop or ""); BLOCK_INTERVAL (= interval prop or "");
//   with a click: button, x, y, relative_x, relative_y, width, height, modifiers (comma-joined), and legacy BLOCK_BUTTON, BLOCK_X, BLOCK_Y
pub fn expand_sh_vars(cmd: &str, env: &[(String, String)], pid: u32) -> String
//   sh rules: $VAR / ${VAR} from env (unknown -> ""), '...' literal (quotes removed), \$ -> $, $$ -> pid; double quotes kept as-is (cmd needs them), with $ expanded inside
pub fn shell_command(shell: &str, cmd: &str) -> (String /*program*/, Vec<String> /*args*/, bool /*raw cmdline*/)
//   "cmd" (default) -> ("cmd", ["/S","/C", "\"<cmd>\""], true)  (raw command line, v1 quoting rule)
//   "pwsh" / "powershell" -> (shell, ["-NoProfile","-Command", cmd], false); anything else -> (shell, ["-c", cmd], false)
//   Expansion by expand_sh_vars happens only for "cmd"; other shells get `cmd` untouched.
pub fn apply_raw_output(block: &mut Block, out: &str)   // lines -> full_text, short_text, color, background, border, min_width, align, name, instance, urgent, separator, separator_block_width, markup, border_top, border_bottom, border_left, border_right; extra lines ignored; empty line = property unset
pub fn apply_json_line(block: &mut Block, line: &str) -> bool   // object keys update properties; false if not a JSON object
pub fn exit_code_block(block: &mut Block, cmd: &str, code: i32)
//   0 -> unchanged; 33 -> urgent=true; 126/127/other -> full_text "Error: Command 'CMD' not executable" / "...not found or missing dependency" / "...exited unexpectedly with code N", short_text None, urgent=true
pub fn static_block(section: &Section) -> Block        // section props mapped to block properties (full_text, color, ...); `label` prefixed to full_text; name = section name
```

- [ ] **Step 1: Failing tests**

```rust
fn sec(name: &str, kv: &[(&str, &str)]) -> Section { Section { name: name.into(), props: kv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() } }
#[test]
fn expand_sh_rules() {
    let env = vec![("DISK".to_string(), "C:".to_string()), ("SCRIPT_DIR".to_string(), r"C:\s".to_string())];
    assert_eq!(expand_sh_vars("$SCRIPT_DIR\\disk.bat $DISK ${DISK}x $NOPE.", &env, 7), r"C:\s\disk.bat C: C:x .");
    assert_eq!(expand_sh_vars("echo '$DISK' \\$DISK $$", &env, 7), "echo $DISK $DISK 7");
    assert_eq!(expand_sh_vars("echo \"$DISK\"", &env, 7), "echo \"C:\"");
}
#[test]
fn env_has_props_and_click() {
    let s = sec("disk", &[("interval", "5"), ("DISK", "C:"), ("instance", "c")]);
    let click = Click { name: Some("disk".into()), instance: Some("c".into()), button: 3, modifiers: vec!["Shift".into()], x: 1, y: 2, relative_x: 3, relative_y: 4, output_x: 0, output_y: 0, width: 5, height: 6 };
    let e = block_env(&s, Path::new(r"C:\cfg"), Some(&click));
    let get = |k: &str| e.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str());
    assert_eq!(get("DISK"), Some("C:"));
    assert_eq!(get("SCRIPT_DIR"), Some(r"C:\cfg"));
    assert_eq!(get("BLOCK_NAME"), Some("disk"));
    assert_eq!(get("BLOCK_INSTANCE"), Some("c"));
    assert_eq!(get("BLOCK_INTERVAL"), Some("5"));
    assert_eq!(get("button"), Some("3"));
    assert_eq!(get("BLOCK_BUTTON"), Some("3"));
    assert_eq!(get("relative_x"), Some("3"));
    assert_eq!(get("modifiers"), Some("Shift"));
}
#[test]
fn shells() {
    assert_eq!(shell_command("cmd", "echo hi"), ("cmd".into(), vec!["/S".into(), "/C".into(), "\"echo hi\"".into()], true));
    assert_eq!(shell_command("bash", "echo $X"), ("bash".into(), vec!["-c".into(), "echo $X".into()], false));
    assert_eq!(shell_command("pwsh", "Get-Date").1[1], "-Command");
}
#[test]
fn raw_and_json_output() {
    let mut b = static_block(&sec("w", &[]));
    apply_raw_output(&mut b, "full\nshort\n#ff0000\n#000000\n");
    assert_eq!((b.full_text.as_str(), b.short_text.as_deref(), b.color.as_deref(), b.background.as_deref()), ("full", Some("short"), Some("#ff0000"), Some("#000000")));
    assert!(apply_json_line(&mut b, r#"{"full_text":"j","urgent":true}"#));
    assert_eq!(b.full_text, "j");
    assert!(b.urgent);
    assert!(!apply_json_line(&mut b, "not json"));
}
#[test]
fn exit_codes_match_i3blocks() {
    let mut b = static_block(&sec("w", &[("full_text", "x")]));
    exit_code_block(&mut b, "foo.sh", 33);
    assert!(b.urgent && b.full_text == "x");
    exit_code_block(&mut b, "foo.sh", 2);
    assert_eq!(b.full_text, "Error: Command 'foo.sh' exited unexpectedly with code 2");
    exit_code_block(&mut b, "foo.sh", 127);
    assert_eq!(b.full_text, "Error: Command 'foo.sh' not found or missing dependency");
    exit_code_block(&mut b, "foo.sh", 126);
    assert_eq!(b.full_text, "Error: Command 'foo.sh' not executable");
}
#[test]
fn label_and_interval() {
    assert_eq!(static_block(&sec("a", &[("label", "L: "), ("full_text", "x")])).full_text, "L: x");
    assert_eq!(parse_interval(Some("persist")), Interval::Persist);
    assert_eq!(parse_interval(Some("0")), Interval::None);
    assert_eq!(parse_interval(None), Interval::None);
    assert_eq!(parse_interval(Some("2.5")), Interval::Secs(2.5));
}
```

- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5: Commit** `feat(status): i3blocks command block rules (env, expansion, shells, output, exit codes)`.

---

### Task 7: Runner: scheduler, writer, processes, clicks

**Files:** Create `crates/status/src/runner.rs`, `crates/status/src/process.rs`; rewrite `crates/status/src/main.rs`

**Interfaces:**
- Consumes: Tasks 1–6.
- Produces:

```rust
// runner.rs (pure scheduling core, tested with a fake clock)
pub enum Slot { Module(Box<dyn Module>), Command(CommandSlot), Static(Block) }
pub struct CommandSlot { pub section: Section, pub interval: Interval, pub format: OutFormat, pub running: bool, pub next_due: Option<f64>, pub signal: Option<u32> }
pub struct Schedule { pub slots: Vec<Slot>, pub blocks: Vec<Block> }   // blocks[i] = last output of slot i
impl Schedule {
    pub fn due(&self, now: f64) -> Vec<usize>                      // slots whose next_due <= now and that are not running
    pub fn on_started(&mut self, i: usize);                        // running = true
    pub fn on_finished(&mut self, i: usize, now: f64);             // running = false; next_due = now + secs (Secs), None (Once/None), now (Repeat)
    pub fn on_click(&mut self, i: usize) -> ClickAction             // enum ClickAction { Spawn, SendToStdin, Drop }: Persist -> SendToStdin; running -> Drop; else Spawn
    pub fn on_signal(&mut self, sig: u32) -> Vec<usize>            // non-persist, not running, signal == sig
    pub fn refresh_all(&mut self) -> Vec<usize>                    // non-persist, not running
}
pub fn slot_for_click(s: &Schedule, name: Option<&str>, instance: Option<&str>) -> Option<usize>   // first slot whose block name and instance match
// process.rs
pub fn spawn_block(prog: &str, args: &[String], raw_cmdline: bool, env: &[(String, String)], job: HANDLE, persist: bool) -> io::Result<Child>
//   CREATE_NO_WINDOW; stdin = pipe for persist, null otherwise; stdout piped; process assigned to `job`
```

- main: parse args (`-c PATH`, `--signal N`, `--refresh-all`; the latter two are Task 8); discover config; build slots (i3status order → modules; i3blocks sections → Task 8 decides built-in vs command vs static); create the Job Object; print the header; event loop on one `mpsc` channel of `Event { Tick, Output(i, String), Exited(i, i32), Click(Click), Signal(u32), RefreshAll }`: a ticker thread sends `Tick` every 100 ms; module slots render on their interval (general `interval`, per-module override); command slots spawn when due; a reader thread per running child sends each stdout line (`Output`) and on EOF the exit code (`Exited`); persist slots apply each line immediately; non-persist slots collect output and apply it on exit (raw: whole output; json: each line), then `exit_code_block`. A stdin reader thread parses i3bar click JSON lines (skip the leading `[` and `,`) into `Click` and sends `Click`. The writer emits a line when blocks changed and ≥ 50 ms since the last line. Config errors from Tasks 1/2 become red `config error line N` blocks first in the line.

- [ ] **Step 1: Failing tests** (pure `Schedule`)

```rust
fn cmd_slot(interval: Interval) -> Slot { Slot::Command(CommandSlot { section: sec("x", &[]), interval, format: OutFormat::Raw, running: false, next_due: Some(0.0), signal: Some(3) }) }
#[test]
fn slow_block_does_not_block_others() {
    let mut s = Schedule { slots: vec![cmd_slot(Interval::Secs(1.0)), cmd_slot(Interval::Secs(1.0))], blocks: vec![Block::default(), Block::default()] };
    assert_eq!(s.due(0.0), vec![0, 1]);
    s.on_started(0); s.on_started(1);
    s.on_finished(1, 0.2);
    assert_eq!(s.due(1.3), vec![1]); // slot 0 still running: not due again, slot 1 unaffected
}
#[test]
fn click_while_running_is_dropped() {
    let mut s = Schedule { slots: vec![cmd_slot(Interval::None), cmd_slot(Interval::Persist)], blocks: vec![Block::default(); 2] };
    assert_eq!(s.on_click(0), ClickAction::Spawn);
    s.on_started(0);
    assert_eq!(s.on_click(0), ClickAction::Drop);
    assert_eq!(s.on_click(1), ClickAction::SendToStdin);
}
#[test]
fn intervals_after_finish() {
    let mut s = Schedule { slots: vec![cmd_slot(Interval::Once), cmd_slot(Interval::Repeat), cmd_slot(Interval::Secs(5.0))], blocks: vec![Block::default(); 3] };
    for i in 0..3 { s.on_started(i); s.on_finished(i, 10.0); }
    assert_eq!(s.due(10.0), vec![1]);
    assert_eq!(s.due(15.0), vec![1, 2]);
}
#[test]
fn signals_and_refresh() {
    let mut s = Schedule { slots: vec![cmd_slot(Interval::None), cmd_slot(Interval::Persist)], blocks: vec![Block::default(); 2] };
    assert_eq!(s.on_signal(3), vec![0]);
    assert_eq!(s.on_signal(4), Vec::<usize>::new());
    assert_eq!(s.refresh_all(), vec![0]);
}
```

(`Block::default()` needs a manual `impl Default for Block` in core with `separator: true` (a derive would give false). The stdin click parser deserializes `i3win_core::i3bar::Click`; add `Deserialize` (with `#[serde(default)]` on every field) to `Click` if Plan 6 didn't.)

- [ ] **Step 2–4:** FAIL → implement → `cargo test --workspace` PASS. Manual smoke: `cargo run -p i3win-status -- -c <temp i3blocks file with [t]\ncommand=echo hi\ninterval=1>` prints the header and lines containing `"full_text":"hi"` (run it for 3 s, then stop it).
- [ ] **Step 5: Commit** `feat(status): scheduler, writer, block processes and clicks`.

---

### Task 8: Built-in modules in i3blocks.conf, signals, default config, docs

**Files:** Modify `crates/status/src/main.rs`, create `crates/status/src/signal.rs`; modify `docs/manual-checks.md`, `scripts/live-checks.ps1`

**Interfaces:**
- Produces:

```rust
pub fn classify_section(s: &Section) -> SectionKind   // enum SectionKind { Command, Module(ModuleRef), Static }
//   has `command` -> Command; else name (split at first space into module + instance) is a known module
//   (cpu_usage memory disk battery tztime time ethernet wireless volume path_exists read_file) -> Module; else Static
pub fn section_props(s: &Section) -> Props            // i3blocks values as Value::Str for the module formatters
// signal.rs
pub const SIGNAL_CLASS: &str = "i3win_status_sig";
pub fn listen(tx: Sender<Event>)        // thread: registers SIGNAL_CLASS, creates a message-only window (HWND_MESSAGE), pumps messages; WM_APP+1 wParam=N -> Event::Signal(N); WM_APP+2 -> Event::RefreshAll
pub fn send_signal(n: u32) -> usize     // FindWindowExW(HWND_MESSAGE, prev, SIGNAL_CLASS, None) loop, PostMessageW to each; returns count
pub fn send_refresh_all() -> usize
```

- `i3win-status --signal N` / `--refresh-all`: send and exit immediately with code 0 (even if 0 instances found; print the count to stderr).
- Module sections from i3blocks.conf get the section's other props (e.g. `format`), the i3blocks globals as `general` (so `interval` global applies).
- When the default `i3status.conf` is written (no config found), log nothing to stdout (stdout is the protocol).

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn classify_sections() {
    assert_eq!(classify_section(&sec("weather", &[("command", "x")])), SectionKind::Command);
    assert_eq!(classify_section(&sec("disk C:", &[("format", "%free")])), SectionKind::Module(ModuleRef { module: "disk".into(), instance: Some("C:".into()) }));
    assert_eq!(classify_section(&sec("cpu_usage", &[])), SectionKind::Module(ModuleRef { module: "cpu_usage".into(), instance: None }));
    assert_eq!(classify_section(&sec("hello", &[("full_text", "hi")])), SectionKind::Static);
}
```

- [ ] **Step 2–4:** FAIL → implement → PASS. Manual: start two `i3win-status -c <file with [s]\ncommand=echo %TIME%\nsignal=5>` processes, run `i3win-status --signal 5`; it prints `2` to stderr and both print a new line.
- [ ] **Step 5: Docs & live checks.** `docs/manual-checks.md` "Plan 7" items: put an `i3blocks.conf` with a `[weather]` curl block, a `[cpu_usage]` block and a `[clicker]` block (`command=echo %button%`) in `%USERPROFILE%\.config\i3win\`, reload i3win → blocks appear; clicking `clicker` with right button shows `3`; `i3win-status --signal N` refreshes a `signal=N` block; a block with `command=exit 2` shows the urgent `Error: Command 'exit 2' exited unexpectedly with code 2`; with only `i3status.conf`, `order` and `format` changes apply after reload; deleting both files makes i3win-status recreate `i3status.conf`. `scripts/live-checks.ps1`: run `target\debug\i3win-status.exe -c <temp i3blocks file>` for 2.5 s with `[t]\ncommand=echo hi\ninterval=1`, capture stdout, check it contains `{"version":1,"click_events":true}` and `"full_text":"hi"`.
- [ ] **Step 6: Commit** `feat(status): built-in modules from i3blocks.conf, signals; docs`.
