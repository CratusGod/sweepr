//! A record of every run, so anything sweepr moved can be put back.

use anyhow::{Context, Result};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub rule: String,
    pub original: PathBuf,
    pub quarantined: PathBuf,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub created: Timestamp,
    pub expires: Timestamp,
    /// False while a run is moving files. A crash leaves it false, and
    /// restore then only touches items that actually reached quarantine.
    pub complete: bool,
    pub items: Vec<Item>,
    /// Per-run quarantine directories, deleted when the run is purged.
    pub run_dirs: Vec<PathBuf>,
}

impl Manifest {
    pub fn total_size(&self) -> u64 {
        self.items.iter().map(|i| i.size).sum()
    }

    pub fn path_in(&self, runs_dir: &Path) -> PathBuf {
        runs_dir.join(format!("{}.json", self.id))
    }

    pub fn details_path_in(&self, runs_dir: &Path) -> PathBuf {
        runs_dir.join(format!("{}.txt", self.id))
    }

    /// Write atomically and durably: the record reaches the disk before any
    /// file is moved, so a power cut never leaves moved files without one.
    pub fn save(&self, runs_dir: &Path) -> Result<()> {
        fs::create_dir_all(runs_dir).with_context(|| format!("creating {}", runs_dir.display()))?;
        let final_path = self.path_in(runs_dir);
        let tmp = final_path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(self)?;
        let mut file = File::create(&tmp).with_context(|| format!("writing {}", tmp.display()))?;
        file.write_all(&json)?;
        file.sync_all()
            .with_context(|| format!("syncing {}", tmp.display()))?;
        fs::rename(&tmp, &final_path)
            .with_context(|| format!("saving {}", final_path.display()))?;
        File::open(runs_dir)
            .and_then(|d| d.sync_all())
            .with_context(|| format!("syncing {}", runs_dir.display()))?;
        Ok(())
    }

    /// Forget the run. Already gone counts as success.
    pub fn remove(&self, runs_dir: &Path) -> Result<()> {
        for path in [self.path_in(runs_dir), self.details_path_in(runs_dir)] {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| format!("removing {}", path.display())),
            }
        }
        Ok(())
    }
}

/// Readable run records (oldest first) plus a message for each unreadable one.
/// One damaged record must never block restoring or purging the others.
pub struct Loaded {
    pub runs: Vec<Manifest>,
    pub broken: Vec<String>,
}

impl Loaded {
    /// Print each broken record to stderr (the journal, under systemd).
    pub fn warn(self) -> Vec<Manifest> {
        for msg in &self.broken {
            eprintln!("sweepr: {msg}");
        }
        self.runs
    }
}

pub fn load_all(runs_dir: &Path) -> Result<Loaded> {
    let entries = match fs::read_dir(runs_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Loaded {
                runs: Vec::new(),
                broken: Vec::new(),
            });
        }
        Err(e) => return Err(e).with_context(|| format!("reading {}", runs_dir.display())),
    };
    let mut runs = Vec::new();
    let mut broken = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        match read_one(&path) {
            Ok(m) => runs.push(m),
            Err(e) => broken.push(format!("{e:#}; files from that run stay in quarantine")),
        }
    }
    runs.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(Loaded { runs, broken })
}

/// Load a single run by id, without reading any other record.
pub fn load_one(runs_dir: &Path, id: &str) -> Result<Option<Manifest>> {
    if id.contains('/') || id.starts_with('.') {
        anyhow::bail!("`{id}` is not a run id");
    }
    let path = runs_dir.join(format!("{id}.json"));
    if !path.exists() {
        return Ok(None);
    }
    read_one(&path).map(Some)
}

fn read_one(path: &Path) -> Result<Manifest> {
    let text = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice(&text).with_context(|| format!("damaged run record {}", path.display()))
}

/// A sortable, filename-safe run id such as `20261023T151200Z`.
pub fn new_id(now: Timestamp, runs_dir: &Path) -> String {
    let base = now.strftime("%Y%m%dT%H%M%SZ").to_string();
    let mut id = base.clone();
    let mut n = 2;
    while runs_dir.join(format!("{id}.json")).exists() {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str) -> Manifest {
        let now = Timestamp::from_second(1_800_000_000).unwrap();
        Manifest {
            id: id.into(),
            created: now,
            expires: now,
            complete: true,
            items: vec![Item {
                rule: "pip".into(),
                original: "/a".into(),
                quarantined: "/q/a".into(),
                size: 5,
            }],
            run_dirs: vec!["/q".into()],
        }
    }

    #[test]
    fn save_load_round_trip_sorted() {
        let dir = tempfile::tempdir().unwrap();
        sample("20260102T000000Z").save(dir.path()).unwrap();
        sample("20260101T000000Z").save(dir.path()).unwrap();
        let all = load_all(dir.path()).unwrap().runs;
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, "20260101T000000Z");
        assert_eq!(all[0], sample("20260101T000000Z"));
        assert_eq!(all[0].total_size(), 5);
    }

    #[test]
    fn damaged_record_is_reported_and_does_not_hide_the_others() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("bad.json"), "").unwrap();
        sample("20260101T000000Z").save(dir.path()).unwrap();
        let loaded = load_all(dir.path()).unwrap();
        assert_eq!(loaded.runs.len(), 1);
        assert_eq!(loaded.broken.len(), 1);
        assert!(load_one(dir.path(), "20260101T000000Z").unwrap().is_some());
        assert!(load_one(dir.path(), "missing").unwrap().is_none());
        assert!(load_one(dir.path(), "../x").is_err());
    }

    #[test]
    fn remove_twice_is_fine() {
        let dir = tempfile::tempdir().unwrap();
        let m = sample("20260101T000000Z");
        m.save(dir.path()).unwrap();
        m.remove(dir.path()).unwrap();
        m.remove(dir.path()).unwrap();
    }

    #[test]
    fn ids_do_not_collide() {
        let dir = tempfile::tempdir().unwrap();
        let now = Timestamp::from_second(1_800_000_000).unwrap();
        let first = new_id(now, dir.path());
        sample(&first).save(dir.path()).unwrap();
        let second = new_id(now, dir.path());
        assert_ne!(first, second);
        assert!(second.starts_with(&first));
    }
}
