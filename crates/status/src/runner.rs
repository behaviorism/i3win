//! Pure scheduling core: which slots run when, how command output becomes a
//! block, and when the writer may print. No I/O; times are seconds (`f64`)
//! from an arbitrary origin so tests can use a fake clock.

use crate::command_block::{
    apply_json_line, apply_raw_output, cap_text, exit_code_block, parse_interval, static_block,
    truncate_str, Interval, OutFormat,
};
use crate::i3blocks_conf::Section;
use crate::mods::Module;
use i3win_core::i3bar::{Block, Click};

/// Minimum time between two printed lines (writer coalescing).
pub const WRITE_GAP: f64 = 0.05;
/// Least time before retrying a command that could not be started.
pub const SPAWN_RETRY: f64 = 1.0;
/// Most output kept from one non-persist run; the rest is dropped.
pub const MAX_RUN_OUTPUT: usize = 1024 * 1024;

pub enum Slot {
    Module(Box<dyn Module>),
    Command(CommandSlot),
    Static(Block),
}

pub struct CommandSlot {
    pub section: Section,
    pub interval: Interval,
    pub format: OutFormat,
    pub running: bool,
    pub next_due: Option<f64>,
    pub signal: Option<u32>,
    /// Sequence number of the latest run; messages from older runs are stale.
    pub run: u64,
}

impl CommandSlot {
    /// A slot for a section with a `command`; due at once (i3blocks runs every
    /// block at start).
    pub fn from_section(section: Section) -> CommandSlot {
        let format = match section.get("format") {
            Some(f) if f.trim().eq_ignore_ascii_case("json") => OutFormat::Json,
            _ => OutFormat::Raw,
        };
        CommandSlot {
            interval: parse_interval(section.get("interval")),
            format,
            running: false,
            next_due: Some(0.0),
            signal: section.get("signal").and_then(|s| s.trim().parse().ok()),
            run: 0,
            section,
        }
    }

    /// The `command` value as written.
    pub fn command(&self) -> &str {
        self.section.get("command").unwrap_or("")
    }

    pub fn is_persist(&self) -> bool {
        self.interval == Interval::Persist
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickAction {
    Spawn,
    SendToStdin,
    Drop,
}

pub struct Schedule {
    pub slots: Vec<Slot>,
    /// `blocks[i]` is the last output of slot `i`.
    pub blocks: Vec<Block>,
}

impl Schedule {
    pub fn command(&self, i: usize) -> Option<&CommandSlot> {
        match self.slots.get(i) {
            Some(Slot::Command(c)) => Some(c),
            _ => None,
        }
    }

    fn command_mut(&mut self, i: usize) -> Option<&mut CommandSlot> {
        match self.slots.get_mut(i) {
            Some(Slot::Command(c)) => Some(c),
            _ => None,
        }
    }

    fn commands_where(&self, pred: impl Fn(&CommandSlot) -> bool) -> Vec<usize> {
        (0..self.slots.len())
            .filter(|&i| self.command(i).is_some_and(&pred))
            .collect()
    }

    /// Command slots whose `next_due <= now` and that are not running.
    pub fn due(&self, now: f64) -> Vec<usize> {
        self.commands_where(|c| !c.running && c.next_due.is_some_and(|t| t <= now))
    }

    /// Marks slot `i` running and returns the new run's sequence number.
    pub fn on_started(&mut self, i: usize) -> u64 {
        match self.command_mut(i) {
            Some(c) => {
                c.running = true;
                c.next_due = None;
                c.run += 1;
                c.run
            }
            None => 0,
        }
    }

    /// True when a message tagged `run` belongs to slot `i`'s running run
    /// (output arriving after that run finished is stale).
    pub fn is_current(&self, i: usize, run: u64) -> bool {
        self.command(i).is_some_and(|c| c.running && c.run == run)
    }

    pub fn on_finished(&mut self, i: usize, now: f64) {
        if let Some(c) = self.command_mut(i) {
            c.running = false;
            c.next_due = match c.interval {
                Interval::Secs(s) => Some(now + s),
                Interval::Repeat => Some(now),
                Interval::Once | Interval::None | Interval::Persist => None,
            };
        }
    }

    /// Like [`Schedule::on_finished`] for a run that never started, but the
    /// next attempt waits at least [`SPAWN_RETRY`] (no tight `repeat` loop).
    pub fn on_spawn_failed(&mut self, i: usize, now: f64) {
        self.on_finished(i, now);
        if let Some(c) = self.command_mut(i) {
            c.next_due = c.next_due.map(|t| t.max(now + SPAWN_RETRY));
        }
    }

    pub fn on_click(&mut self, i: usize) -> ClickAction {
        match self.command(i) {
            Some(c) if c.is_persist() => ClickAction::SendToStdin,
            Some(c) if c.running => ClickAction::Drop,
            Some(_) => ClickAction::Spawn,
            None => ClickAction::Drop,
        }
    }

    /// Non-persist, not running slots with `signal == sig`.
    pub fn on_signal(&mut self, sig: u32) -> Vec<usize> {
        self.commands_where(|c| !c.is_persist() && !c.running && c.signal == Some(sig))
    }

    /// Every non-persist, not running slot.
    pub fn refresh_all(&mut self) -> Vec<usize> {
        self.commands_where(|c| !c.is_persist() && !c.running)
    }

    /// Stores slot `i`'s block; true when it changed.
    pub fn set_block(&mut self, i: usize, b: Block) -> bool {
        match self.blocks.get_mut(i) {
            Some(old) if *old != b => {
                *old = b;
                true
            }
            _ => false,
        }
    }
}

/// First slot whose block's name and instance match the click's.
pub fn slot_for_click(s: &Schedule, name: Option<&str>, instance: Option<&str>) -> Option<usize> {
    s.blocks
        .iter()
        .position(|b| b.name.as_deref() == name && b.instance.as_deref() == instance)
}

/// The section's static block without the `label` prefix, so output can be
/// applied and the label added once afterwards.
fn base_block(section: &Section) -> Block {
    let mut b = static_block(section);
    b.full_text = section.get("full_text").unwrap_or("").to_string();
    b
}

fn add_label(b: &mut Block, section: &Section) {
    if let Some(label) = section.get("label") {
        b.full_text = cap_text(&format!("{label}{}", b.full_text));
    }
}

/// Block for one finished (non-persist) run: rebuilt from the section, then
/// the output (raw: whole output; json: each line), label, exit code.
pub fn run_block(section: &Section, format: OutFormat, output: &str, code: i32) -> Block {
    let mut b = base_block(section);
    match format {
        OutFormat::Raw => apply_raw_output(&mut b, output),
        OutFormat::Json => {
            for line in output.lines() {
                apply_json_line(&mut b, line);
            }
        }
    }
    add_label(&mut b, section);
    exit_code_block(&mut b, section.get("command").unwrap_or(""), code);
    b
}

/// Block for one output line of a persist command (rebuilt per line).
pub fn persist_line_block(section: &Section, format: OutFormat, line: &str) -> Block {
    let mut b = base_block(section);
    match format {
        OutFormat::Raw => apply_raw_output(&mut b, line),
        OutFormat::Json => {
            apply_json_line(&mut b, line);
        }
    }
    add_label(&mut b, section);
    b
}

/// Adds one line of a non-persist run's output, keeping the total at most
/// [`MAX_RUN_OUTPUT`] bytes (later lines are dropped). `size` is the running
/// total for this run.
pub fn push_capped(pending: &mut Vec<String>, size: &mut usize, line: String) {
    let cost = line.len() + 1;
    if *size + cost <= MAX_RUN_OUTPUT {
        *size += cost;
        pending.push(line);
    }
}

/// What a persist block's process gets on stdin for a click (i3blocks 1.5):
/// json format the click object, raw format the button number.
pub fn persist_click_line(format: OutFormat, click: &Click) -> String {
    match format {
        OutFormat::Json => {
            let obj = serde_json::to_string(click).unwrap_or_else(|_| "{}".into());
            format!("{obj}\n")
        }
        OutFormat::Raw => format!("{}\n", click.button),
    }
}

/// Seconds between renders of a module: its own `interval`, else `general`.
pub fn module_interval(m: &dyn Module, general: f64) -> f64 {
    m.interval()
        .filter(|s| s.is_finite() && *s > 0.0)
        .unwrap_or(general)
}

/// A red block reporting a config parse error.
pub fn config_error_block(line: usize) -> Block {
    let mut b = Block::new(format!("config error line {line}"));
    b.color = Some("#FF0000".to_string());
    b
}

/// Longest status line written; the bar drops lines over 64 KiB.
pub const MAX_STATUS_LINE: usize = 60 * 1024;

/// Serialized blocks, with the longest texts halved until the line fits in
/// [`MAX_STATUS_LINE`] bytes.
fn blocks_json(blocks: &[Block]) -> String {
    let ser = |b: &[Block]| serde_json::to_string(b).unwrap_or_else(|_| "[]".to_string());
    let json = ser(blocks);
    if json.len() <= MAX_STATUS_LINE {
        return json;
    }
    let mut blocks = blocks.to_vec();
    loop {
        let longest = blocks
            .iter_mut()
            .flat_map(|b| std::iter::once(&mut b.full_text).chain(b.short_text.as_mut()))
            .max_by_key(|t| t.len());
        match longest {
            Some(t) if !t.is_empty() => {
                let keep = truncate_str(t, t.len() / 2).len();
                t.truncate(keep);
            }
            _ => {
                // Non-text fields are capped per block; if the line is still
                // too long, drop blocks from the left until it fits.
                let mut json = ser(&blocks);
                while json.len() > MAX_STATUS_LINE && !blocks.is_empty() {
                    blocks.remove(0);
                    json = ser(&blocks);
                }
                return json;
            }
        }
        let json = ser(&blocks);
        if json.len() <= MAX_STATUS_LINE {
            return json;
        }
    }
}

/// One i3bar protocol line (`,` prefix after the first).
pub fn render_line(blocks: &[Block], first: bool) -> String {
    let json = blocks_json(blocks);
    if first {
        json
    } else {
        format!(",{json}")
    }
}

/// `output_format = "none"`: full texts joined by the separator.
pub fn plain_line(blocks: &[Block], separator: &str) -> String {
    blocks
        .iter()
        .map(|b| b.full_text.as_str())
        .collect::<Vec<_>>()
        .join(separator)
}

/// Writer coalescing: at most one line per [`WRITE_GAP`].
#[derive(Default)]
pub struct Coalescer {
    last: Option<f64>,
    pub dirty: bool,
}

impl Coalescer {
    /// True when a line should be printed now; then clears `dirty`.
    pub fn take(&mut self, now: f64) -> bool {
        if self.dirty && self.wait(now) == 0.0 {
            self.dirty = false;
            self.last = Some(now);
            return true;
        }
        false
    }

    /// Seconds until a line may be printed (0 when it may be printed now).
    pub fn wait(&self, now: f64) -> f64 {
        match self.last {
            Some(t) => (t + WRITE_GAP - now).max(0.0),
            None => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn blocks_json_never_exceeds_cap_even_with_huge_other_fields() {
        let blocks: Vec<Block> = (0..400)
            .map(|i| {
                let mut b = Block::new(format!("t{i}"));
                b.name = Some("x".repeat(2000));
                b
            })
            .collect();
        let out = blocks_json(&blocks);
        assert!(out.len() <= MAX_STATUS_LINE);
        assert!(serde_json::from_str::<serde_json::Value>(&out).is_ok());
    }

    use super::*;

    fn sec(name: &str, kv: &[(&str, &str)]) -> Section {
        Section {
            name: name.into(),
            props: kv
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn cmd_slot(interval: Interval) -> Slot {
        Slot::Command(CommandSlot {
            section: sec("x", &[]),
            interval,
            format: OutFormat::Raw,
            running: false,
            next_due: Some(0.0),
            signal: Some(3),
            run: 0,
        })
    }

    #[test]
    fn slow_block_does_not_block_others() {
        let mut s = Schedule {
            slots: vec![cmd_slot(Interval::Secs(1.0)), cmd_slot(Interval::Secs(1.0))],
            blocks: vec![Block::new(""), Block::new("")],
        };
        assert_eq!(s.due(0.0), vec![0, 1]);
        s.on_started(0);
        s.on_started(1);
        s.on_finished(1, 0.2);
        assert_eq!(s.due(1.3), vec![1]); // slot 0 still running: not due again, slot 1 unaffected
    }

    #[test]
    fn click_while_running_is_dropped() {
        let mut s = Schedule {
            slots: vec![cmd_slot(Interval::None), cmd_slot(Interval::Persist)],
            blocks: vec![Block::new(""); 2],
        };
        assert_eq!(s.on_click(0), ClickAction::Spawn);
        s.on_started(0);
        assert_eq!(s.on_click(0), ClickAction::Drop);
        assert_eq!(s.on_click(1), ClickAction::SendToStdin);
    }

    #[test]
    fn intervals_after_finish() {
        let mut s = Schedule {
            slots: vec![
                cmd_slot(Interval::Once),
                cmd_slot(Interval::Repeat),
                cmd_slot(Interval::Secs(5.0)),
            ],
            blocks: vec![Block::new(""); 3],
        };
        for i in 0..3 {
            s.on_started(i);
            s.on_finished(i, 10.0);
        }
        assert_eq!(s.due(10.0), vec![1]);
        assert_eq!(s.due(15.0), vec![1, 2]);
    }

    #[test]
    fn failed_spawn_is_not_retried_at_once() {
        let mut s = Schedule {
            slots: vec![
                cmd_slot(Interval::Repeat),
                cmd_slot(Interval::Secs(5.0)),
                cmd_slot(Interval::Once),
            ],
            blocks: vec![Block::new(""); 3],
        };
        for i in 0..3 {
            s.on_started(i);
            s.on_spawn_failed(i, 10.0);
        }
        assert!(s.due(10.0).is_empty());
        assert!(s.due(10.0 + SPAWN_RETRY - 0.01).is_empty());
        assert_eq!(s.due(10.0 + SPAWN_RETRY), vec![0]);
        assert_eq!(s.due(15.0), vec![0, 1]);
        assert!(!s.command(0).unwrap().running);
    }

    #[test]
    fn signals_and_refresh() {
        let mut s = Schedule {
            slots: vec![cmd_slot(Interval::None), cmd_slot(Interval::Persist)],
            blocks: vec![Block::new(""); 2],
        };
        assert_eq!(s.on_signal(3), vec![0]);
        assert_eq!(s.on_signal(4), Vec::<usize>::new());
        assert_eq!(s.refresh_all(), vec![0]);
        s.on_started(0);
        assert_eq!(s.refresh_all(), Vec::<usize>::new());
    }

    #[test]
    fn output_after_finish_is_stale() {
        let mut s = Schedule {
            slots: vec![cmd_slot(Interval::Secs(1.0))],
            blocks: vec![Block::new("")],
        };
        let r1 = s.on_started(0);
        assert!(s.is_current(0, r1));
        s.on_finished(0, 0.0);
        assert!(!s.is_current(0, r1)); // late output of a finished run
        let r2 = s.on_started(0);
        assert_ne!(r1, r2);
        assert!(!s.is_current(0, r1)); // old run's output during the new run
        assert!(s.is_current(0, r2));
        assert!(!s.is_current(5, r2));
    }

    #[test]
    fn run_output_is_capped() {
        let mut pending = Vec::new();
        let mut size = 0;
        let big = "x".repeat(MAX_RUN_OUTPUT / 2);
        push_capped(&mut pending, &mut size, big.clone());
        push_capped(&mut pending, &mut size, big.clone()); // would exceed: dropped
        push_capped(&mut pending, &mut size, "tail".into());
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[1], "tail");
        assert!(size <= MAX_RUN_OUTPUT);
    }

    #[test]
    fn static_and_module_slots_never_due_or_clickable() {
        let mut s = Schedule {
            slots: vec![Slot::Static(Block::new("s"))],
            blocks: vec![Block::new("s")],
        };
        assert!(s.due(100.0).is_empty());
        assert_eq!(s.on_click(0), ClickAction::Drop);
        assert!(s.refresh_all().is_empty());
    }

    #[test]
    fn slot_from_section_and_click_lookup() {
        let c = CommandSlot::from_section(sec(
            "w",
            &[
                ("command", "echo hi"),
                ("interval", "persist"),
                ("format", "json"),
                ("signal", "10"),
            ],
        ));
        assert_eq!(
            (c.interval.clone(), c.format, c.signal, c.next_due),
            (Interval::Persist, OutFormat::Json, Some(10), Some(0.0))
        );
        assert_eq!(c.command(), "echo hi");
        let mut a = Block::new("a");
        a.name = Some("w".into());
        let mut b = Block::new("b");
        b.name = Some("w".into());
        b.instance = Some("2".into());
        let s = Schedule {
            slots: vec![Slot::Static(a.clone()), Slot::Static(b.clone())],
            blocks: vec![a, b],
        };
        assert_eq!(slot_for_click(&s, Some("w"), Some("2")), Some(1));
        assert_eq!(slot_for_click(&s, Some("w"), None), Some(0));
        assert_eq!(slot_for_click(&s, Some("z"), None), None);
    }

    #[test]
    fn run_block_rebuilds_and_labels() {
        let s = sec(
            "w",
            &[("command", "x.bat"), ("label", "L: "), ("color", "#111")],
        );
        let b = run_block(&s, OutFormat::Raw, "one\nshort\n#222\n", 0);
        assert_eq!(b.full_text, "L: one");
        assert_eq!(b.color.as_deref(), Some("#222"));
        // Next run: fields absent from the output come back from the section.
        let b = run_block(&s, OutFormat::Raw, "two\n", 0);
        assert_eq!(
            (b.full_text.as_str(), b.color.as_deref()),
            ("L: two", Some("#111"))
        );
        assert_eq!(b.short_text, None);
        let b = run_block(&s, OutFormat::Raw, "", 2);
        assert_eq!(
            b.full_text,
            "Error: Command 'x.bat' exited unexpectedly with code 2"
        );
        assert!(b.urgent);
        let b = run_block(&s, OutFormat::Raw, "u\n", 33);
        assert!(b.urgent && b.full_text == "L: u");
    }

    #[test]
    fn json_and_persist_blocks() {
        let s = sec("w", &[("label", "L "), ("full_text", "static")]);
        let b = run_block(
            &s,
            OutFormat::Json,
            "{\"full_text\":\"a\"}\n{\"color\":\"#333\"}\n",
            0,
        );
        assert_eq!(
            (b.full_text.as_str(), b.color.as_deref()),
            ("L a", Some("#333"))
        );
        // A json line without full_text keeps the static text, labelled once.
        let b = persist_line_block(&s, OutFormat::Json, "{\"color\":\"#444\"}");
        assert_eq!(b.full_text, "L static");
        let b = persist_line_block(&s, OutFormat::Raw, "tick");
        assert_eq!(
            (b.full_text.as_str(), b.name.as_deref()),
            ("L tick", Some("w"))
        );
    }

    #[test]
    fn persist_click_payload() {
        let c = Click {
            name: Some("w".into()),
            button: 3,
            ..Click::default()
        };
        assert_eq!(persist_click_line(OutFormat::Raw, &c), "3\n");
        let line = persist_click_line(OutFormat::Json, &c);
        assert!(line.ends_with('\n'));
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["button"], 3);
        assert_eq!(v["name"], "w");
    }

    #[test]
    fn writer_coalesces_and_formats() {
        let mut w = Coalescer::default();
        assert!(!w.take(0.0)); // nothing changed
        w.dirty = true;
        assert!(w.take(0.0));
        w.dirty = true;
        assert!(!w.take(0.03));
        assert!((w.wait(0.03) - 0.02).abs() < 1e-9);
        assert!(w.take(0.05));
        let blocks = vec![Block::new("a"), config_error_block(4)];
        assert!(render_line(&blocks, true).starts_with("[{"));
        assert!(render_line(&blocks, false).starts_with(",[{"));
        assert_eq!(plain_line(&blocks, " | "), "a | config error line 4");
        assert_eq!(blocks[1].color.as_deref(), Some("#FF0000"));
        assert!(blocks[1].separator);
    }

    #[test]
    fn oversized_output_still_fits_the_bar_line() {
        let s = sec("w", &[("command", "x")]);
        let huge = "x".repeat(70_000);
        let b = run_block(&s, OutFormat::Raw, &format!("{huge}\n{huge}\n"), 0);
        let line = render_line(&[b], false);
        assert!(line.len() < 64 * 1024, "{}", line.len());
        let _: serde_json::Value = serde_json::from_str(&line[1..]).unwrap();
        // Many capped blocks of control characters (6x JSON escaping) still fit.
        let ctl = "\u{1}".repeat(70_000);
        let blocks: Vec<Block> = (0..20)
            .map(|_| run_block(&s, OutFormat::Raw, &format!("{ctl}\n{ctl}\n"), 0))
            .collect();
        let line = render_line(&blocks, true);
        assert!(line.len() <= MAX_STATUS_LINE, "{}", line.len());
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 20);
    }

    struct FakeModule(Option<f64>);
    impl Module for FakeModule {
        fn name(&self) -> &str {
            "fake"
        }
        fn instance(&self) -> Option<&str> {
            None
        }
        fn interval(&self) -> Option<f64> {
            self.0
        }
        fn render(&mut self) -> Block {
            Block::new("f")
        }
    }

    #[test]
    fn module_interval_override() {
        assert_eq!(module_interval(&FakeModule(None), 5.0), 5.0);
        assert_eq!(module_interval(&FakeModule(Some(2.0)), 5.0), 2.0);
        assert_eq!(module_interval(&FakeModule(Some(0.0)), 5.0), 5.0);
    }
}
