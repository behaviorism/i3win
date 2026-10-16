//! Tracking of hides we caused ourselves, and persistence of hidden windows so
//! they can be restored after a crash.
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use i3win_core::tree::WindowId;

use super::window;

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

/// Windows we have hidden, persisted one decimal id per line.
pub struct HiddenStore {
    path: PathBuf,
    set: BTreeSet<isize>,
}

impl HiddenStore {
    pub fn open(path: PathBuf) -> HiddenStore {
        HiddenStore {
            path,
            set: BTreeSet::new(),
        }
    }

    pub fn add(&mut self, id: WindowId) {
        self.set.insert(id.0);
        self.persist();
    }

    pub fn remove(&mut self, id: WindowId) {
        self.set.remove(&id.0);
        self.persist();
    }

    pub fn contains(&self, id: WindowId) -> bool {
        self.set.contains(&id.0)
    }

    pub fn ids(&self) -> Vec<WindowId> {
        self.set.iter().map(|&i| WindowId(i)).collect()
    }

    pub fn clear_file(&mut self) {
        self.set.clear();
        self.persist();
        if let Err(e) = std::fs::remove_file(&self.path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("hidden store: remove {}: {e}", self.path.display());
            }
        }
    }

    fn persist(&self) {
        *lock_mirror() = self.set.clone();
        let text: String = self.set.iter().map(|i| format!("{i}\n")).collect();
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        if let Err(e) = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, &self.path))
        {
            log::warn!("hidden store: write {}: {e}", self.path.display());
        }
    }
}

/// Reads ids left by a previous run (skipping bad lines) and deletes the file.
pub fn take_previous(path: &Path) -> Vec<WindowId> {
    let ids = std::fs::read_to_string(path)
        .map(|s| {
            s.lines()
                .filter_map(|l| l.trim().parse::<isize>().ok())
                .map(WindowId)
                .collect()
        })
        .unwrap_or_default();
    let _ = std::fs::remove_file(path);
    ids
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

    #[test]
    fn store_roundtrip_and_take_previous() {
        let p = std::env::temp_dir().join(format!("i3win_test_hidden_{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let mut s = HiddenStore::open(p.clone());
        s.add(WindowId(10));
        s.add(WindowId(20));
        s.remove(WindowId(10));
        std::fs::write(
            &p,
            format!(
                "{}\ngarbage\n30\n",
                std::fs::read_to_string(&p).unwrap().trim()
            ),
        )
        .unwrap();
        assert_eq!(take_previous(&p), vec![WindowId(20), WindowId(30)]);
        assert!(!p.exists());
    }
}
