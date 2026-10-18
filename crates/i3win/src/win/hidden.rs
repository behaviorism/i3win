//! Tracking of hides we caused ourselves, and persistence of hidden windows so
//! they can be restored after a crash.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use i3win_core::tree::WindowId;

use super::{cloak, window};

const EXPECT_TTL: Duration = Duration::from_secs(2);

/// Process-global copy of the hidden set, readable from panic hooks and
/// console-control handlers where the `App` is unreachable.
static HIDDEN_MIRROR: Mutex<BTreeSet<isize>> = Mutex::new(BTreeSet::new());

fn lock_mirror() -> std::sync::MutexGuard<'static, BTreeSet<isize>> {
    HIDDEN_MIRROR.lock().unwrap_or_else(|e| e.into_inner())
}

/// Remembers hides we initiated so the resulting EVENT_OBJECT_HIDE can be ignored.
#[derive(Default)]
pub struct HideTracker {
    pending: HashMap<WindowId, Vec<Instant>>,
}

impl HideTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn expect(&mut self, id: WindowId, now: Instant) {
        self.pending.entry(id).or_default().push(now);
    }

    /// Drops expired expectations, then removes one if present.
    pub fn consume(&mut self, id: WindowId, now: Instant) -> bool {
        let Some(v) = self.pending.get_mut(&id) else {
            return false;
        };
        v.retain(|t| now.saturating_duration_since(*t) <= EXPECT_TTL);
        let found = !v.is_empty();
        if found {
            v.remove(0);
        }
        if v.is_empty() {
            self.pending.remove(&id);
        }
        found
    }

    pub fn forget(&mut self, id: WindowId) {
        self.pending.remove(&id);
    }
}

/// Windows we have hidden, persisted one `hwnd pid` (decimal) line per window.
/// The pid guards against HWND reuse when the file outlives the windows.
pub struct HiddenStore {
    path: PathBuf,
    set: BTreeMap<isize, u32>,
    dirty: bool,
}

impl HiddenStore {
    pub fn open(path: PathBuf) -> HiddenStore {
        HiddenStore {
            path,
            set: BTreeMap::new(),
            dirty: false,
        }
    }

    /// Records the window (with its current owner pid). Persisted by `persist_if_dirty`.
    pub fn add(&mut self, id: WindowId) {
        self.add_with_pid(id, window::window_pid(id));
    }

    pub fn add_with_pid(&mut self, id: WindowId, pid: u32) {
        if self.set.insert(id.0, pid) != Some(pid) {
            self.dirty = true;
        }
    }

    pub fn remove(&mut self, id: WindowId) {
        if self.set.remove(&id.0).is_some() {
            self.dirty = true;
        }
    }

    pub fn contains(&self, id: WindowId) -> bool {
        self.set.contains_key(&id.0)
    }

    #[allow(dead_code)] // inspection helper, unused until IPC (Plan 4)
    pub fn ids(&self) -> Vec<WindowId> {
        self.set.keys().map(|&i| WindowId(i)).collect()
    }

    pub fn clear_file(&mut self) {
        self.set.clear();
        self.dirty = false;
        *lock_mirror() = BTreeSet::new();
        clear_store_file(&self.path);
    }

    /// Writes the file (and the crash mirror) once if anything changed.
    pub fn persist_if_dirty(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        *lock_mirror() = self.set.keys().copied().collect();
        let text: String = self
            .set
            .iter()
            .map(|(i, pid)| {
                format!(
                    "{i} {pid}
"
                )
            })
            .collect();
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        if let Err(e) = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, &self.path))
        {
            log::warn!("hidden store: write {}: {e}", self.path.display());
        }
    }
}

/// Deletes the store file (used when the session ends and everything was shown).
pub fn clear_store_file(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            log::warn!("hidden store: remove {}: {e}", path.display());
        }
    }
}

/// Parses `hwnd pid` lines, skipping anything else (including old single-number lines).
fn parse_store(text: &str) -> Vec<(WindowId, u32)> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let id = it.next()?.parse::<isize>().ok()?;
            let pid = it.next()?.parse::<u32>().ok()?;
            if it.next().is_some() {
                return None;
            }
            Some((WindowId(id), pid))
        })
        .collect()
}

/// Reads `(window, pid)` pairs left by a previous run and deletes the file.
/// On a read error other than "not found" the file is kept and an error logged.
pub fn take_previous(path: &Path) -> Vec<(WindowId, u32)> {
    match std::fs::read_to_string(path) {
        Ok(s) => {
            let _ = std::fs::remove_file(path);
            parse_store(&s)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            log::error!("hidden store: read {}: {e}", path.display());
            Vec::new()
        }
    }
}

/// Shows every window in the mirror. Safe for panic hooks and console handlers.
pub fn restore_all_hidden() {
    let ids: Vec<isize> = {
        let mut m = lock_mirror();
        let v = m.iter().copied().collect();
        m.clear();
        v
    };
    for i in ids {
        let id = WindowId(i);
        if window::is_window(id) {
            // Both are harmless if the window was not hidden that way.
            cloak::uncloak_fresh(id);
            window::show(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn tracker_consumes_expected_hides_and_expires() {
        let t0 = Instant::now();
        let mut t = HideTracker::new();
        let w = WindowId(5);
        t.expect(w, t0);
        assert!(t.consume(w, t0 + Duration::from_millis(10)));
        assert!(!t.consume(w, t0 + Duration::from_millis(20))); // only one expected
        t.expect(w, t0);
        assert!(!t.consume(w, t0 + Duration::from_secs(3))); // expired
    }

    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("i3win_test_{name}_{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn store_roundtrip_and_take_previous() {
        let p = temp("hidden_rt");
        let mut s = HiddenStore::open(p.clone());
        s.add_with_pid(WindowId(10), 100);
        s.add_with_pid(WindowId(20), 200);
        s.remove(WindowId(10));
        assert!(!p.exists()); // nothing is written until persisted
        s.persist_if_dirty();
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "20 200
"
        );
        assert_eq!(take_previous(&p), vec![(WindowId(20), 200)]);
        assert!(!p.exists());
    }

    #[test]
    fn malformed_and_old_format_lines_are_skipped() {
        let p = temp("hidden_bad");
        std::fs::write(
            &p,
            "30
garbage
5 x
7 8 9
 40  41 
",
        )
        .unwrap();
        assert_eq!(take_previous(&p), vec![(WindowId(40), 41)]);
    }

    #[test]
    fn persist_only_when_dirty() {
        let p = temp("hidden_dirty");
        let mut s = HiddenStore::open(p.clone());
        s.persist_if_dirty();
        assert!(!p.exists());
        s.add_with_pid(WindowId(1), 2);
        s.persist_if_dirty();
        std::fs::remove_file(&p).unwrap();
        s.add_with_pid(WindowId(1), 2); // unchanged
        s.persist_if_dirty();
        assert!(!p.exists());
        assert!(s.contains(WindowId(1)));
        clear_store_file(&p);
    }
}
