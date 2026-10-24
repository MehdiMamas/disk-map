//! The undo journal: before any tweak writes, the old value of everything it touches is
//! recorded here (a small JSON file in the app data folder). Undo and "Restore all" replay it.
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Something a tweak can change.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub enum Target {
    Reg { hive: String, path: String, name: String },
    /// A Windows service's start type.
    Service { name: String },
    /// The active power plan (value is the plan's GUID).
    Power,
}

/// A value as we saw it. `Other` is a value of a type we do not handle; we never overwrite it.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub enum Val {
    Absent,
    Dword(u32),
    Str(String),
    Other,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct Change {
    pub target: Target,
    pub old: Val,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    pub seq: u64,
    pub tweak: String,
    /// Unix seconds.
    pub at: u64,
    pub changes: Vec<Change>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct LastProfile {
    pub id: String,
    pub name: String,
    pub at: u64,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, Default)]
pub struct Journal {
    #[serde(default)]
    pub entries: Vec<Entry>,
    #[serde(default)]
    pub last_profile: Option<LastProfile>,
    #[serde(default)]
    pub next_seq: u64,
}

impl Journal {
    pub fn push(&mut self, tweak: &str, at: u64, changes: Vec<Change>) -> u64 {
        self.next_seq += 1;
        let seq = self.next_seq;
        self.entries.push(Entry { seq, tweak: tweak.to_string(), at, changes });
        seq
    }

    pub fn remove(&mut self, seq: u64) {
        self.entries.retain(|e| e.seq != seq);
    }

    pub fn has(&self, tweak: &str) -> bool {
        self.entries.iter().any(|e| e.tweak == tweak)
    }

    /// Entries of one tweak, newest first.
    pub fn for_tweak(&self, tweak: &str) -> Vec<Entry> {
        let mut v: Vec<Entry> = self.entries.iter().filter(|e| e.tweak == tweak).cloned().collect();
        v.sort_by(|a, b| b.seq.cmp(&a.seq));
        v
    }

    /// All entries, newest first (the order "Restore all" replays them).
    pub fn newest_first(&self) -> Vec<Entry> {
        let mut v = self.entries.clone();
        v.sort_by(|a, b| b.seq.cmp(&a.seq));
        v
    }

    /// A missing file is an empty journal. A file we cannot read is an error, so we never
    /// overwrite a journal we failed to understand.
    pub fn load(path: &Path) -> Result<Journal, String> {
        match std::fs::read_to_string(path) {
            Ok(s) if s.trim().is_empty() => Ok(Journal::default()),
            Ok(s) => serde_json::from_str(&s).map_err(|e| format!("The undo journal could not be read: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Journal::default()),
            Err(e) => Err(format!("The undo journal could not be read: {e}")),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(n: &str, old: Val) -> Change {
        Change { target: Target::Reg { hive: "HKCU".into(), path: "Software\\X".into(), name: n.into() }, old }
    }

    #[test]
    fn round_trip_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join("j.json");
        assert_eq!(Journal::load(&p).unwrap(), Journal::default());
        let mut j = Journal::default();
        j.push("a", 10, vec![ch("one", Val::Absent), ch("two", Val::Dword(1))]);
        j.push("b", 11, vec![Change { target: Target::Power, old: Val::Str("guid".into()) }]);
        j.last_profile = Some(LastProfile { id: "gaming".into(), name: "Gaming".into(), at: 12 });
        j.save(&p).unwrap();
        assert_eq!(Journal::load(&p).unwrap(), j);
    }

    #[test]
    fn newest_first_and_per_tweak() {
        let mut j = Journal::default();
        j.push("a", 1, vec![]);
        j.push("b", 2, vec![]);
        j.push("a", 3, vec![]);
        let all: Vec<u64> = j.newest_first().iter().map(|e| e.seq).collect();
        assert_eq!(all, vec![3, 2, 1]);
        let a: Vec<u64> = j.for_tweak("a").iter().map(|e| e.seq).collect();
        assert_eq!(a, vec![3, 1]);
        j.remove(3);
        assert!(j.has("a") && j.has("b"));
        j.remove(1);
        assert!(!j.has("a"));
    }

    #[test]
    fn broken_file_is_an_error_not_an_empty_journal() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("j.json");
        std::fs::write(&p, "{ not json").unwrap();
        assert!(Journal::load(&p).is_err());
    }
}
