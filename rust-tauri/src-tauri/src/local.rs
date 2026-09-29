use crate::note::{self, Note, NoteMeta};
use chrono::{DateTime, Utc};
use rand::RngCore;
use regex::Regex;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const NOTES_DIR: &str = "notes";
const STATE_FILE: &str = "sync-state.json";
const FAILED_PREFIX: &str = "__failed__/";

pub const TRASH_RETENTION: chrono::Duration = chrono::Duration::days(90);

#[derive(Debug)]
pub enum StoreError {
    InvalidId,
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::InvalidId => write!(f, "invalid note id"),
            StoreError::Io(e) => write!(f, "io: {e}"),
            StoreError::Json(e) => write!(f, "json: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Io(e)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        StoreError::Json(e)
    }
}

fn id_pattern() -> &'static Regex {
    static PAT: OnceLock<Regex> = OnceLock::new();
    PAT.get_or_init(|| Regex::new(r"^[0-9]{8}-[0-9]{6}-[0-9a-f]{4}$").unwrap())
}

pub fn validate_id(id: &str) -> Result<(), StoreError> {
    if id_pattern().is_match(id) {
        Ok(())
    } else {
        Err(StoreError::InvalidId)
    }
}

type Clock = Box<dyn Fn() -> DateTime<Utc> + Send + Sync>;

pub struct Store {
    root: PathBuf,
    now: Clock,
}

impl Store {
    pub fn open<P: AsRef<Path>>(root: P) -> Result<Store, StoreError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join(NOTES_DIR))?;
        Ok(Store {
            root,
            now: Box::new(Utc::now),
        })
    }

    pub fn set_clock(&mut self, clock: Clock) {
        self.now = clock;
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn note_path(&self, id: &str) -> PathBuf {
        self.root.join(NOTES_DIR).join(format!("{id}.json"))
    }

    pub fn new_id(&self) -> Result<String, StoreError> {
        let mut b = [0u8; 2];
        rand::thread_rng().fill_bytes(&mut b);
        let stamp = (self.now)().format("%Y%m%d-%H%M%S");
        Ok(format!("{stamp}-{:02x}{:02x}", b[0], b[1]))
    }

    pub fn create(&self, title: &str, tags: Vec<String>, body: &str) -> Result<Note, StoreError> {
        let id = self.new_id()?;
        let now = (self.now)();
        let n = Note::new(id, title.to_string(), tags, now, now, body.to_string());
        self.write(&n)?;
        Ok(n)
    }

    pub fn load(&self, id: &str) -> Result<Note, StoreError> {
        validate_id(id)?;
        let b = fs::read(self.note_path(id))?;
        Ok(note::decode(&b)?)
    }

    pub fn update(
        &self,
        id: &str,
        title: Option<&str>,
        tags: Option<Vec<String>>,
        body: &str,
    ) -> Result<Note, StoreError> {
        validate_id(id)?;
        let mut n = self.load(id)?;
        if let Some(t) = title {
            n.title = t.to_string();
        }
        if let Some(tg) = tags {
            n.tags = tg;
        }
        n.body = body.to_string();
        n.updated = (self.now)();
        self.write(&n)?;
        Ok(n)
    }

    pub fn delete(&self, id: &str) -> Result<(), StoreError> {
        validate_id(id)?;
        remove_if_exists(&self.note_path(id))?;
        let mut state = self.load_state()?;
        let mut changed = false;
        if state.remove(id).is_some() {
            changed = true;
        }
        if state.remove(&format!("{FAILED_PREFIX}{id}")).is_some() {
            changed = true;
        }
        if changed {
            self.save_state(&state)?;
        }
        Ok(())
    }

    pub fn mark_deleted(&self, id: &str) -> Result<(), StoreError> {
        validate_id(id)?;
        remove_if_exists(&self.note_path(id))?;
        let mut state = self.load_state()?;
        state.insert(format!("{FAILED_PREFIX}{id}"), "1".to_string());
        self.save_state(&state)
    }

    pub fn list(&self) -> Result<Vec<Note>, StoreError> {
        let mut out = self.read_all_notes()?;
        sort_notes(&mut out);
        Ok(out)
    }

    pub fn list_meta(&self) -> Result<Vec<NoteMeta>, StoreError> {
        let notes = self.list()?;
        Ok(notes.iter().map(|n| n.meta()).collect())
    }

    pub fn search_meta(&self, query: &str) -> Result<Vec<NoteMeta>, StoreError> {
        let q = query.trim();
        if q.is_empty() {
            return self.list_meta();
        }
        let needle = q.to_lowercase();
        let mut out: Vec<Note> = Vec::new();
        for n in self.read_all_notes()? {
            let mut title = note::first_line_title(&n.body);
            if title.is_empty() {
                title = n.title.clone();
            }
            if title.to_lowercase().contains(&needle) || n.body.to_lowercase().contains(&needle) {
                out.push(n);
            }
        }
        sort_notes(&mut out);
        Ok(out.iter().map(|n| n.meta()).collect())
    }

    pub fn duplicate(&self, id: &str) -> Result<Note, StoreError> {
        validate_id(id)?;
        let src = self.load(id)?;
        self.create(&src.title, src.tags.clone(), &src.body)
    }

    pub fn set_pinned(&self, id: &str, pinned: bool) -> Result<(), StoreError> {
        validate_id(id)?;
        let mut n = self.load(id)?;
        n.pinned = pinned;
        self.write(&n)
    }

    pub fn set_file_type(&self, id: &str, ft: &str) -> Result<(), StoreError> {
        validate_id(id)?;
        let mut n = self.load(id)?;
        n.filetype = ft.to_string();
        self.write(&n)
    }

    pub fn set_trashed(&self, id: &str, trashed: bool) -> Result<(), StoreError> {
        validate_id(id)?;
        let mut n = self.load(id)?;
        n.trashed = trashed;
        n.trashed_at = if trashed { Some((self.now)()) } else { None };
        self.write(&n)
    }

    pub fn purge_expired_trash(&self) -> Result<Vec<String>, StoreError> {
        let notes = self.list()?;
        let cutoff = (self.now)() - TRASH_RETENTION;
        let mut purged = Vec::new();
        for n in &notes {
            let ts = match n.trashed_at {
                Some(ts) if n.trashed => ts,
                _ => continue,
            };
            if ts < cutoff {
                self.delete(&n.id)?;
                purged.push(n.id.clone());
            }
        }
        Ok(purged)
    }

    pub fn put(&self, n: &Note) -> Result<(), StoreError> {
        validate_id(&n.id)?;
        self.write(n)
    }

    fn read_all_notes(&self) -> Result<Vec<Note>, StoreError> {
        let dir = self.root.join(NOTES_DIR);
        let mut out = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if entry.file_type()?.is_dir() || !name.ends_with(".json") {
                continue;
            }
            let b = fs::read(entry.path())?;
            out.push(note::decode(&b)?);
        }
        Ok(out)
    }

    fn write(&self, n: &Note) -> Result<(), StoreError> {
        let b = n.encode()?;
        let path = self.note_path(&n.id);
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, &b)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    fn load_state(&self) -> Result<BTreeMap<String, String>, StoreError> {
        let path = self.root.join(STATE_FILE);
        match fs::read(&path) {
            Ok(b) => Ok(serde_json::from_slice(&b)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(e.into()),
        }
    }

    fn save_state(&self, m: &BTreeMap<String, String>) -> Result<(), StoreError> {
        let b = serde_json::to_vec_pretty(m)?;
        let path = self.root.join(STATE_FILE);
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, &b)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn sha(&self, id: &str) -> Result<Option<String>, StoreError> {
        Ok(self.load_state()?.get(id).cloned())
    }

    pub fn set_sha(&self, id: &str, sha: &str) -> Result<(), StoreError> {
        let mut state = self.load_state()?;
        state.insert(id.to_string(), sha.to_string());
        self.save_state(&state)
    }

    pub fn del_sha(&self, id: &str) -> Result<(), StoreError> {
        let mut state = self.load_state()?;
        if state.remove(id).is_none() {
            return Ok(());
        }
        self.save_state(&state)
    }

    pub fn set_failed(&self, id: &str) -> Result<(), StoreError> {
        self.set_sha(&format!("{FAILED_PREFIX}{id}"), "1")
    }

    pub fn clear_failed(&self, id: &str) -> Result<(), StoreError> {
        self.del_sha(&format!("{FAILED_PREFIX}{id}"))
    }

    pub fn load_failed(&self) -> Result<Vec<String>, StoreError> {
        let state = self.load_state()?;
        Ok(state
            .keys()
            .filter_map(|k| k.strip_prefix(FAILED_PREFIX).map(|s| s.to_string()))
            .collect())
    }
}

fn remove_if_exists(path: &Path) -> Result<(), StoreError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn sort_notes(out: &mut [Note]) {
    out.sort_by(|a, b| {
        if a.pinned != b.pinned {
            return b.pinned.cmp(&a.pinned);
        }
        b.updated.cmp(&a.updated)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::sync::Mutex;

    fn test_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(dir.path()).unwrap();
        let tick = Mutex::new(Utc.with_ymd_and_hms(2026, 9, 1, 14, 30, 22).unwrap());
        s.set_clock(Box::new(move || {
            let mut t = tick.lock().unwrap();
            *t += chrono::Duration::seconds(1);
            *t
        }));
        (dir, s)
    }

    #[test]
    fn create_load_round_trip() {
        let (_d, s) = test_store();
        let n = s
            .create("标题", vec!["医案".into()], "# 正文\n内容")
            .unwrap();
        assert!(!n.id.is_empty());
        let got = s.load(&n.id).unwrap();
        assert_eq!(got.body, "# 正文\n内容");
        assert_eq!(got.title, "标题");
        assert!(got.has_tag("医案"));
    }

    #[test]
    fn new_id_stable_format_and_unique() {
        let (_d, s) = test_store();
        let id1 = s.new_id().unwrap();
        let id2 = s.new_id().unwrap();
        assert_ne!(id1, id2);
        assert_eq!(id1.len(), "20260901-143023-abcd".len());
        assert!(validate_id(&id1).is_ok());
    }

    #[test]
    fn title_change_keeps_filename() {
        let (_d, s) = test_store();
        let n = s.create("旧标题", vec![], "body").unwrap();
        let id = n.id.clone();
        s.update(&id, Some("新标题"), None, "body2").unwrap();
        assert!(s.note_path(&id).exists());
        let got = s.load(&id).unwrap();
        assert_eq!(got.title, "新标题");
        assert_eq!(got.body, "body2");
    }

    #[test]
    fn update_bumps_updated_not_created() {
        let (_d, s) = test_store();
        let n = s.create("t", vec![], "a").unwrap();
        let created = n.created;
        let got = s.update(&n.id, None, None, "b").unwrap();
        assert_eq!(got.created, created);
        assert!(got.updated > created);
    }

    #[test]
    fn delete_idempotent_and_clears_sha() {
        let (_d, s) = test_store();
        let n = s.create("t", vec![], "a").unwrap();
        s.set_sha(&n.id, "abc").unwrap();
        s.delete(&n.id).unwrap();
        assert!(s.sha(&n.id).unwrap().is_none());
        s.delete(&n.id).unwrap();
        assert!(s.load(&n.id).is_err());
    }

    #[test]
    fn list_sorted_by_updated_desc() {
        let (_d, s) = test_store();
        let a = s.create("A", vec![], "a").unwrap();
        let b = s.create("B", vec![], "b").unwrap();
        let list = s.list().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, b.id);
        assert_eq!(list[1].id, a.id);
    }

    #[test]
    fn purge_expired_trash() {
        let (_d, s) = test_store();
        let n1 = s.create("keep", vec![], "a").unwrap();
        let n2 = s.create("expire", vec![], "b").unwrap();
        s.set_trashed(&n2.id, true).unwrap();
        let mut s = s;
        let fixed = Utc::now() + TRASH_RETENTION + chrono::Duration::days(2);
        s.set_clock(Box::new(move || fixed));
        let purged = s.purge_expired_trash().unwrap();
        assert_eq!(purged, vec![n2.id.clone()]);
        assert!(s.load(&n2.id).is_err());
        assert!(s.load(&n1.id).is_ok());
    }

    #[test]
    fn purge_skips_recent_trash() {
        let (_d, s) = test_store();
        let n = s.create("recent", vec![], "x").unwrap();
        s.set_trashed(&n.id, true).unwrap();
        let purged = s.purge_expired_trash().unwrap();
        assert!(purged.is_empty());
        assert!(s.load(&n.id).is_ok());
    }

    #[test]
    fn set_trashed_records_timestamp() {
        let (_d, s) = test_store();
        let n = s.create("t", vec![], "x").unwrap();
        s.set_trashed(&n.id, true).unwrap();
        let got = s.load(&n.id).unwrap();
        assert!(got.trashed && got.trashed_at.is_some());
        s.set_trashed(&n.id, false).unwrap();
        let got = s.load(&n.id).unwrap();
        assert!(!got.trashed && got.trashed_at.is_none());
    }

    #[test]
    fn rejects_path_traversal_id() {
        let (_d, s) = test_store();
        let outside = s.root().join("outside.json");
        fs::write(&outside, b"{}").unwrap();
        let legit = s.create("t", vec![], "body").unwrap();

        let bad = [
            "../outside",
            "..",
            ".",
            "",
            "a/b",
            "a\\b",
            "a\x00b",
            "a b",
            "-flag",
            "a.json",
            "日本語",
            "20260901-143022-a3f2/../evil",
        ];
        for id in bad {
            assert!(s.load(id).is_err(), "Load({id:?}) should reject");
            assert!(s.delete(id).is_err(), "Delete({id:?}) should reject");
            assert!(s.update(id, None, None, "x").is_err(), "Update({id:?})");
            assert!(s.set_pinned(id, true).is_err(), "SetPinned({id:?})");
            assert!(s.set_trashed(id, true).is_err(), "SetTrashed({id:?})");
            assert!(s.duplicate(id).is_err(), "Duplicate({id:?})");
        }
        assert!(outside.exists(), "outside file affected by traversal");
        assert!(s.load(&legit.id).is_ok(), "legit id broke after guard");
    }

    #[test]
    fn sha_round_trip() {
        let (_d, s) = test_store();
        assert!(s.sha("missing").unwrap().is_none());
        s.set_sha("id1", "sha1").unwrap();
        assert_eq!(s.sha("id1").unwrap(), Some("sha1".to_string()));
        assert!(s.root().join(STATE_FILE).exists());
    }
}
