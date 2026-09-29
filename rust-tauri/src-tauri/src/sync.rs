use crate::conflict;
use crate::local::{validate_id, Store, StoreError};
use crate::note;
use crate::syncclient::{SyncClient, SyncError};
use sha1::{Digest, Sha1};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};

const NOTES_DIR: &str = "notes";
const HEAD_KEY: &str = "__head__";

#[derive(Debug)]
pub enum EngineError {
    Store(StoreError),
    Client(SyncError),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::Store(e) => write!(f, "{e}"),
            EngineError::Client(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<StoreError> for EngineError {
    fn from(e: StoreError) -> Self {
        EngineError::Store(e)
    }
}

impl From<SyncError> for EngineError {
    fn from(e: SyncError) -> Self {
        EngineError::Client(e)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SyncResult {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Conflicted")]
    pub conflicted: bool,
}

impl SyncResult {
    fn plain(id: &str) -> SyncResult {
        SyncResult {
            id: id.to_string(),
            conflicted: false,
        }
    }
}

type Callback = Box<dyn Fn(&str) + Send + Sync>;

pub struct Syncer {
    pub store: Store,
    pub client: SyncClient,
    encryption_key: RwLock<Option<[u8; 32]>>,
    pub on_note_changed: Option<Callback>,
    op_mu: tokio::sync::Mutex<()>,
    failed: Mutex<HashSet<String>>,
    pub(crate) on_sync_error: Option<Arc<dyn Fn(String) + Send + Sync>>,
    reported_decrypt_errors: Mutex<HashSet<String>>,
}

fn note_path(id: &str) -> String {
    format!("{NOTES_DIR}/{id}.json")
}

fn git_blob_sha(content: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(format!("blob {}\0", content.len()).as_bytes());
    h.update(content);
    hex::encode(h.finalize())
}

fn trim_json(name: &str) -> String {
    name.strip_suffix(".json").unwrap_or(name).to_string()
}

fn id_from_path(p: &str) -> String {
    let prefix = format!("{NOTES_DIR}/");
    let name = match p.strip_prefix(&prefix) {
        Some(n) => n,
        None => return String::new(),
    };
    let id = match name.strip_suffix(".json") {
        Some(i) => i,
        None => return String::new(),
    };
    if validate_id(id).is_err() {
        return String::new();
    }
    id.to_string()
}

fn is_not_found(e: &StoreError) -> bool {
    matches!(e, StoreError::Io(io) if io.kind() == std::io::ErrorKind::NotFound)
}

impl Syncer {
    pub fn new(store: Store, client: SyncClient) -> Syncer {
        Self::new_with_key(store, client, None)
    }

    pub fn new_with_key(
        store: Store,
        client: SyncClient,
        encryption_key: Option<[u8; 32]>,
    ) -> Syncer {
        Syncer {
            store,
            client,
            encryption_key: RwLock::new(encryption_key),
            on_note_changed: None,
            op_mu: tokio::sync::Mutex::new(()),
            failed: Mutex::new(HashSet::new()),
            on_sync_error: None,
            reported_decrypt_errors: Mutex::new(HashSet::new()),
        }
    }

    pub(crate) fn encryption_key(&self) -> Option<[u8; 32]> {
        self.encryption_key.read().unwrap().clone()
    }

    pub(crate) fn set_encryption_key(&self, key: Option<[u8; 32]>) {
        *self.encryption_key.write().unwrap() = key;
    }

    fn notify(&self, id: &str) {
        if let Some(cb) = &self.on_note_changed {
            cb(id);
        }
    }

    fn mark_failed(&self, id: &str) {
        self.failed.lock().unwrap().insert(id.to_string());
        let _ = self.store.set_failed(id);
    }

    pub(crate) fn report_sync_error(&self, error: &str) {
        if let Some(callback) = &self.on_sync_error {
            callback(error.to_string());
        }
    }

    fn report_decrypt_error(&self, id: &str, error: &str) {
        if !self
            .reported_decrypt_errors
            .lock()
            .unwrap()
            .insert(id.to_string())
        {
            return;
        }
        self.report_sync_error(error);
    }

    fn clear_decrypt_error(&self, id: &str) {
        self.reported_decrypt_errors.lock().unwrap().remove(id);
    }

    pub fn mark_unsynced(&self, id: &str) {
        self.mark_failed(id);
    }

    fn clear_failed(&self, id: &str) {
        self.failed.lock().unwrap().remove(id);
        let _ = self.store.clear_failed(id);
    }

    pub fn load_failed(&self) -> Result<(), EngineError> {
        let ids = self.store.load_failed()?;
        let mut set = self.failed.lock().unwrap();
        for id in ids {
            set.insert(id);
        }
        Ok(())
    }

    pub(crate) fn failed_ids(&self) -> HashSet<String> {
        self.failed.lock().unwrap().clone()
    }

    fn is_failed(&self, id: &str) -> bool {
        self.failed.lock().unwrap().contains(id)
    }

    async fn ensure(&self) -> Result<(), SyncError> {
        self.client.ensure_branch().await
    }

    fn local_blob_sha(&self, id: &str) -> Result<(Vec<u8>, String, bool), EngineError> {
        match self.store.load(id) {
            Ok(n) => {
                let enc = note::encode_remote(&n, self.encryption_key().as_ref()).map_err(|e| {
                    EngineError::Store(StoreError::Json(serde_json::Error::io(
                        std::io::Error::new(std::io::ErrorKind::InvalidData, e),
                    )))
                })?;
                let sha = git_blob_sha(&enc);
                Ok((enc, sha, true))
            }
            Err(e) if is_not_found(&e) => Ok((Vec::new(), String::new(), false)),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn push(&self, id: &str) -> Result<SyncResult, EngineError> {
        let _g = self.op_mu.lock().await;
        if let Err(e) = self.ensure().await {
            self.mark_failed(id);
            return Err(e.into());
        }
        self.reconcile(id, "").await
    }

    pub async fn push_delete(&self, id: &str) -> Result<(), EngineError> {
        let _g = self.op_mu.lock().await;
        self.ensure().await?;
        self.reconcile(id, "").await.map(|_| ())
    }

    pub async fn pull(&self) -> Result<Vec<String>, EngineError> {
        let res = self.sync_all().await?;
        Ok(res.into_iter().map(|r| r.id).collect())
    }

    pub async fn validate(&self) -> Result<(), EngineError> {
        self.sync_all().await?;
        if !self.failed_ids().is_empty() {
            return Err(EngineError::Store(StoreError::Io(std::io::Error::other(
                "full sync has failed notes",
            ))));
        }
        Ok(())
    }

    pub(crate) async fn recover_remote_pin(&self, candidate: &str) -> Result<bool, EngineError> {
        if candidate.is_empty() {
            return Ok(false);
        }
        let candidate_key = crate::config::encryption_key(candidate);
        for entry in self.client.list(NOTES_DIR).await? {
            let id = id_from_path(&entry.path);
            if id.is_empty() {
                continue;
            }
            let (content, _) = self.client.get(&note_path(&id)).await?;
            let value: serde_json::Value = match serde_json::from_str(&content) {
                Ok(value) => value,
                Err(_) => continue,
            };
            if value.get("body_encryption_version").is_some() {
                return Ok(note::decode_remote(content.as_bytes(), Some(&candidate_key)).is_ok());
            }
        }
        Ok(true)
    }

    pub async fn sync_all(&self) -> Result<Vec<SyncResult>, EngineError> {
        let _g = self.op_mu.lock().await;
        self.ensure().await?;
        let remote_head = self.client.head("").await?;
        let local_head = self.store.sha(HEAD_KEY)?.unwrap_or_default();

        let mut remote_hint: HashMap<String, String> = HashMap::new();
        let mut use_bootstrap = false;
        if local_head.is_empty() {
            use_bootstrap = true;
        } else if !remote_head.is_empty() && local_head != remote_head {
            match self.client.compare(&local_head, &remote_head).await {
                Err(_) => use_bootstrap = true,
                Ok(changes) => {
                    for c in changes {
                        let id = id_from_path(&c.path);
                        if id.is_empty() {
                            continue;
                        }
                        remote_hint.insert(id, c.status);
                    }
                }
            }
        }

        if use_bootstrap {
            let entries = self.client.list(NOTES_DIR).await?;
            for e in entries {
                if e.kind != "file" {
                    continue;
                }
                let id = trim_json(&e.name);
                if validate_id(&id).is_err() {
                    continue;
                }
                remote_hint.insert(id, "modified".to_string());
            }
            let metas = self.store.list_meta()?;
            for m in metas {
                self.mark_failed(&m.id);
            }
        }

        let mut ids: HashSet<String> = HashSet::new();
        for id in remote_hint.keys() {
            ids.insert(id.clone());
        }
        for id in self.failed_ids() {
            ids.insert(id);
        }

        let mut results = Vec::new();
        let mut any_failed = false;
        for id in ids {
            let hint = remote_hint.get(&id).cloned().unwrap_or_default();
            match self.reconcile(&id, &hint).await {
                Err(_) => any_failed = true,
                Ok(r) => results.push(r),
            }
        }

        if !any_failed && self.failed_ids().is_empty() && !remote_head.is_empty() {
            self.store.set_sha(HEAD_KEY, &remote_head)?;
        }
        Ok(results)
    }

    pub async fn rewrite_all(&self) -> Result<usize, EngineError> {
        let _guard = self.op_mu.lock().await;
        self.rewrite_all_locked().await
    }

    pub async fn change_encryption_and_rewrite(
        &self,
        encryption_key: Option<[u8; 32]>,
    ) -> Result<usize, EngineError> {
        let _guard = self.op_mu.lock().await;
        *self.encryption_key.write().unwrap() = encryption_key;
        self.rewrite_all_with_key(encryption_key.as_ref()).await
    }

    async fn rewrite_all_locked(&self) -> Result<usize, EngineError> {
        self.rewrite_all_with_key(self.encryption_key().as_ref())
            .await
    }

    async fn rewrite_all_with_key(
        &self,
        encryption_key: Option<&[u8; 32]>,
    ) -> Result<usize, EngineError> {
        self.ensure().await?;
        let notes = self.store.list()?;
        let total = notes.len();
        let remote_entries = self.client.list(NOTES_DIR).await?;
        let mut remote_shas = HashMap::new();
        for entry in remote_entries
            .into_iter()
            .filter(|entry| entry.kind == "file")
        {
            let id = trim_json(&entry.name);
            if validate_id(&id).is_ok() {
                remote_shas.insert(id.clone(), entry.sha);
            }
        }
        for n in notes {
            let encoded = note::encode_remote(&n, encryption_key).map_err(|e| {
                EngineError::Store(StoreError::Json(serde_json::Error::io(
                    std::io::Error::new(std::io::ErrorKind::InvalidData, e),
                )))
            })?;
            let remote_sha = match remote_shas.get(&n.id) {
                Some(sha) => sha.clone(),
                None => String::new(),
            };
            let mut sha = remote_sha;
            let mut put_result = self
                .client
                .put(
                    &note_path(&n.id),
                    &to_str(&encoded),
                    &sha,
                    &format!("rewrite {}", n.id),
                )
                .await;
            if matches!(&put_result, Err(SyncError::Conflict)) {
                let (_, fresh_sha) = self.client.get(&note_path(&n.id)).await?;
                sha = fresh_sha;
                put_result = self
                    .client
                    .put(
                        &note_path(&n.id),
                        &to_str(&encoded),
                        &sha,
                        &format!("rewrite {}", n.id),
                    )
                    .await;
            }
            let sha = put_result?;
            self.store.set_sha(&n.id, &sha)?;
            self.clear_failed(&n.id);
        }
        self.store.del_sha(HEAD_KEY)?;
        let head = self.client.head("").await?;
        if !head.is_empty() {
            self.store.set_sha(HEAD_KEY, &head)?;
        }
        Ok(total)
    }

    async fn reconcile(&self, id: &str, remote_hint: &str) -> Result<SyncResult, EngineError> {
        validate_id(id)?;
        let base_sha = self.store.sha(id)?.unwrap_or_default();
        let dirty = self.is_failed(id);
        let (encoded, local_sha, local_exists) = self.local_blob_sha(id)?;

        if !local_exists {
            return self
                .reconcile_local_missing(id, &base_sha, dirty, remote_hint)
                .await;
        }
        self.reconcile_local_present(id, &base_sha, dirty, remote_hint, &encoded, &local_sha)
            .await
    }

    async fn reconcile_local_missing(
        &self,
        id: &str,
        base_sha: &str,
        dirty: bool,
        remote_hint: &str,
    ) -> Result<SyncResult, EngineError> {
        if dirty {
            if base_sha.is_empty() || remote_hint == "removed" {
                self.clear_failed(id);
                let _ = self.store.del_sha(id);
                return Ok(SyncResult::plain(id));
            }
            if remote_hint == "added" || remote_hint == "modified" {
                return self.pull_down(id).await;
            }
            match self
                .client
                .delete(&note_path(id), base_sha, &format!("delete {id}"))
                .await
            {
                Ok(()) => {
                    self.clear_failed(id);
                    let _ = self.store.del_sha(id);
                    Ok(SyncResult::plain(id))
                }
                Err(SyncError::Conflict) => self.pull_down(id).await,
                Err(e) => {
                    self.mark_failed(id);
                    self.report_sync_error(&e.to_string());
                    Err(e.into())
                }
            }
        } else if remote_hint == "added" || remote_hint == "modified" {
            self.pull_down(id).await
        } else if remote_hint == "removed" {
            let _ = self.store.del_sha(id);
            Ok(SyncResult::plain(id))
        } else {
            Ok(SyncResult::plain(id))
        }
    }

    async fn reconcile_local_present(
        &self,
        id: &str,
        base_sha: &str,
        dirty: bool,
        remote_hint: &str,
        encoded: &[u8],
        local_sha: &str,
    ) -> Result<SyncResult, EngineError> {
        let effective_dirty = dirty || base_sha.is_empty() || local_sha != base_sha;

        if remote_hint == "removed" {
            if !effective_dirty {
                self.store.delete(id)?;
                self.notify(id);
                return Ok(SyncResult::plain(id));
            }
            let new_sha = match self
                .client
                .put(
                    &note_path(id),
                    &to_str(encoded),
                    "",
                    &format!("recreate {id}"),
                )
                .await
            {
                Ok(s) => s,
                Err(e) => {
                    self.mark_failed(id);
                    self.report_sync_error(&e.to_string());
                    return Err(e.into());
                }
            };
            self.store.set_sha(id, &new_sha)?;
            self.clear_failed(id);
            return Ok(SyncResult {
                id: id.to_string(),
                conflicted: conflict::has_markers(&self.load_body(id)),
            });
        }

        if remote_hint == "added" || remote_hint == "modified" {
            return self
                .merge_with_remote(id, base_sha, effective_dirty, encoded, local_sha)
                .await;
        }

        if !effective_dirty {
            if dirty {
                self.clear_failed(id);
            }
            return Ok(SyncResult::plain(id));
        }
        if conflict::has_markers(&self.load_body(id)) {
            return Ok(SyncResult {
                id: id.to_string(),
                conflicted: true,
            });
        }
        match self
            .client
            .put(
                &note_path(id),
                &to_str(encoded),
                base_sha,
                &format!("update {id}"),
            )
            .await
        {
            Ok(new_sha) => {
                self.store.set_sha(id, &new_sha)?;
                self.clear_failed(id);
                Ok(SyncResult::plain(id))
            }
            Err(SyncError::Conflict) => {
                self.merge_with_remote(id, base_sha, effective_dirty, encoded, local_sha)
                    .await
            }
            Err(e) => {
                self.mark_failed(id);
                Err(e.into())
            }
        }
    }

    async fn merge_with_remote(
        &self,
        id: &str,
        base_sha: &str,
        dirty: bool,
        encoded: &[u8],
        local_sha: &str,
    ) -> Result<SyncResult, EngineError> {
        let (remote_content, remote_sha) = match self.client.get(&note_path(id)).await {
            Ok(v) => v,
            Err(e) => {
                self.mark_failed(id);
                self.report_sync_error(&e.to_string());
                return Err(e.into());
            }
        };
        if remote_content.is_empty() && remote_sha.is_empty() {
            if !dirty {
                return Ok(SyncResult::plain(id));
            }
            let new_sha = match self
                .client
                .put(
                    &note_path(id),
                    &to_str(encoded),
                    "",
                    &format!("recreate {id}"),
                )
                .await
            {
                Ok(s) => s,
                Err(e) => {
                    self.mark_failed(id);
                    self.report_sync_error(&e.to_string());
                    return Err(e.into());
                }
            };
            self.store.set_sha(id, &new_sha)?;
            self.clear_failed(id);
            return Ok(SyncResult::plain(id));
        }
        let remote_blob = remote_sha;
        if remote_blob == local_sha {
            self.store.set_sha(id, &remote_blob)?;
            self.clear_failed(id);
            return Ok(SyncResult::plain(id));
        }
        if !base_sha.is_empty() && remote_blob == base_sha {
            if !dirty {
                return Ok(SyncResult::plain(id));
            }
            match self
                .client
                .put(
                    &note_path(id),
                    &to_str(encoded),
                    base_sha,
                    &format!("update {id}"),
                )
                .await
            {
                Ok(new_sha) => {
                    self.store.set_sha(id, &new_sha)?;
                    self.clear_failed(id);
                    return Ok(SyncResult::plain(id));
                }
                Err(SyncError::Conflict) => {}
                Err(e) => {
                    self.mark_failed(id);
                    self.report_sync_error(&e.to_string());
                    return Err(e.into());
                }
            }
        }
        if !dirty && (base_sha.is_empty() || local_sha == base_sha) {
            let n = match note::decode_remote(
                remote_content.as_bytes(),
                self.encryption_key().as_ref(),
            ) {
                Ok(n) => n,
                Err(e) => {
                    self.mark_failed(id);
                    self.report_decrypt_error(id, &e);
                    return Err(EngineError::Store(StoreError::Json(serde_json::Error::io(
                        std::io::Error::new(std::io::ErrorKind::InvalidData, e),
                    ))));
                }
            };
            if let Err(e) = self.store.put(&n) {
                self.mark_failed(id);
                return Err(e.into());
            }
            self.store.set_sha(id, &remote_blob)?;
            self.notify(id);
            return Ok(SyncResult::plain(id));
        }
        let local_note = match self.store.load(id) {
            Ok(n) => n,
            Err(e) => {
                self.mark_failed(id);
                self.report_sync_error(&e.to_string());
                return Err(e.into());
            }
        };
        let remote_body =
            match note::decode_remote(remote_content.as_bytes(), self.encryption_key().as_ref()) {
                Ok(n) => n.body,
                Err(e) => {
                    self.mark_failed(id);
                    self.report_decrypt_error(id, &e);
                    return Err(EngineError::Store(StoreError::Json(serde_json::Error::io(
                        std::io::Error::new(std::io::ErrorKind::InvalidData, e),
                    ))));
                }
            };
        let merged = conflict::merge(&local_note.body, &remote_body);
        if let Err(e) = self.store.update(id, None, None, &merged) {
            self.mark_failed(id);
            return Err(e.into());
        }
        self.store.set_sha(id, &remote_blob)?;
        self.notify(id);
        let merged_note = match self.store.load(id) {
            Ok(n) => n,
            Err(e) => {
                self.mark_failed(id);
                return Err(e.into());
            }
        };
        let merged_encoded = match note::encode_remote(&merged_note, self.encryption_key().as_ref())
        {
            Ok(b) => b,
            Err(e) => {
                self.mark_failed(id);
                self.report_sync_error(&e);
                return Err(EngineError::Store(StoreError::Json(serde_json::Error::io(
                    std::io::Error::new(std::io::ErrorKind::InvalidData, e),
                ))));
            }
        };
        match self
            .client
            .put(
                &note_path(id),
                &to_str(&merged_encoded),
                &remote_blob,
                &format!("conflict {id}"),
            )
            .await
        {
            Ok(new_sha) => {
                self.store.set_sha(id, &new_sha)?;
                if conflict::has_markers(&merged) {
                    return Ok(SyncResult {
                        id: id.to_string(),
                        conflicted: true,
                    });
                }
                self.clear_failed(id);
                self.clear_decrypt_error(id);
                Ok(SyncResult::plain(id))
            }
            Err(_) => {
                self.mark_failed(id);
                Ok(SyncResult {
                    id: id.to_string(),
                    conflicted: conflict::has_markers(&merged),
                })
            }
        }
    }

    async fn pull_down(&self, id: &str) -> Result<SyncResult, EngineError> {
        let (content, sha) = match self.client.get(&note_path(id)).await {
            Ok(v) => v,
            Err(e) => {
                self.mark_failed(id);
                return Err(e.into());
            }
        };
        if content.is_empty() && sha.is_empty() {
            self.clear_failed(id);
            let _ = self.store.del_sha(id);
            return Ok(SyncResult::plain(id));
        }
        let n = match note::decode_remote(content.as_bytes(), self.encryption_key().as_ref()) {
            Ok(n) => n,
            Err(e) => {
                self.mark_failed(id);
                self.report_decrypt_error(id, &e);
                return Err(EngineError::Store(StoreError::Json(serde_json::Error::io(
                    std::io::Error::new(std::io::ErrorKind::InvalidData, e),
                ))));
            }
        };
        if let Err(e) = self.store.put(&n) {
            self.mark_failed(id);
            return Err(e.into());
        }
        self.store.set_sha(id, &sha)?;
        self.clear_failed(id);
        self.clear_decrypt_error(id);
        self.notify(id);
        Ok(SyncResult::plain(id))
    }

    fn load_body(&self, id: &str) -> String {
        match self.store.load(id) {
            Ok(n) => n.body,
            Err(_) => String::new(),
        }
    }
}

fn to_str(b: &[u8]) -> String {
    String::from_utf8_lossy(b).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note::Note;
    use aes_gcm::aead::OsRng;
    use aes_gcm::{Aes256Gcm, KeyInit};
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    use chrono::Utc;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn test_key() -> [u8; 32] {
        Aes256Gcm::generate_key(&mut OsRng).into()
    }

    struct FakeState {
        files: HashMap<String, Vec<u8>>,
        sha: HashMap<String, String>,
        head: String,
        compares: HashMap<String, Vec<(String, String)>>,
        puts: i32,
        deletes: i32,
        gets: i32,
        put_sha_empty: bool,
        conflict_once: HashSet<String>,
    }

    impl FakeState {
        fn new() -> FakeState {
            FakeState {
                files: HashMap::new(),
                sha: HashMap::new(),
                head: "head-0".to_string(),
                compares: HashMap::new(),
                puts: 0,
                deletes: 0,
                gets: 0,
                put_sha_empty: false,
                conflict_once: HashSet::new(),
            }
        }
    }

    type Shared = Arc<Mutex<FakeState>>;

    async fn start(state: Shared) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let state = state.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 1024];
                    let (head_end, clen) = loop {
                        let n = match sock.read(&mut tmp).await {
                            Ok(0) => return,
                            Ok(n) => n,
                            Err(_) => return,
                        };
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&buf[..pos]).to_string();
                            let cl = head
                                .split("\r\n")
                                .find_map(|l| {
                                    let ll = l.to_ascii_lowercase();
                                    ll.strip_prefix("content-length: ")
                                        .and_then(|v| v.trim().parse::<usize>().ok())
                                })
                                .unwrap_or(0);
                            break (pos + 4, cl);
                        }
                    };
                    while buf.len() < head_end + clen {
                        let n = match sock.read(&mut tmp).await {
                            Ok(0) => break,
                            Ok(n) => n,
                            Err(_) => break,
                        };
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    let head = String::from_utf8_lossy(&buf[..head_end - 4]).to_string();
                    let body = buf[head_end..(head_end + clen).min(buf.len())].to_vec();
                    let first = head.split("\r\n").next().unwrap_or("");
                    let mut parts = first.split(' ');
                    let method = parts.next().unwrap_or("").to_string();
                    let target = parts.next().unwrap_or("").to_string();
                    let path = target.split('?').next().unwrap_or("").to_string();
                    let (code, resp) = route(&state, &method, &path, &body);
                    let out = format!(
                        "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        code,
                        resp.as_bytes().len(),
                        resp
                    );
                    let _ = sock.write_all(out.as_bytes()).await;
                    let _ = sock.flush().await;
                });
            }
        });
        format!("http://{addr}")
    }

    fn route(state: &Shared, method: &str, path: &str, body: &[u8]) -> (u16, String) {
        let mut st = state.lock().unwrap();
        if let Some(i) = path.find("/compare/") {
            let spec = path[i + "/compare/".len()..].trim_matches('/');
            let (base, head) = spec.split_once("...").unwrap_or(("", ""));
            let key = format!("{base}...{head}");
            match st.compares.get(&key) {
                None => return (404, String::new()),
                Some(changes) => {
                    let files: Vec<serde_json::Value> = changes
                        .iter()
                        .map(|(f, s)| serde_json::json!({"filename": f, "status": s}))
                        .collect();
                    return (200, serde_json::json!({ "files": files }).to_string());
                }
            }
        }
        if path.ends_with("/commits/draftnote") {
            return (200, serde_json::json!({ "sha": st.head }).to_string());
        }
        if path.ends_with("/contents/notes") {
            let entries: Vec<serde_json::Value> = st
                .sha
                .iter()
                .map(|(p, s)| {
                    let name = p.strip_prefix("notes/").unwrap_or(p);
                    serde_json::json!({"name": name, "path": p, "type": "file", "sha": s})
                })
                .collect();
            return (200, serde_json::Value::Array(entries).to_string());
        }
        if let Some(i) = path.find("/contents/notes/") {
            let file_path = path[i + "/contents/".len()..].to_string();
            match method {
                "GET" => {
                    st.gets += 1;
                    match st.files.get(&file_path) {
                        None => (404, String::new()),
                        Some(content) => {
                            let enc = B64.encode(content);
                            let sha = st.sha.get(&file_path).cloned().unwrap_or_default();
                            (
                                200,
                                serde_json::json!({"content": enc, "sha": sha}).to_string(),
                            )
                        }
                    }
                }
                "PUT" => {
                    st.puts += 1;
                    if st.conflict_once.remove(&file_path) {
                        return (409, String::new());
                    }
                    let pb: serde_json::Value =
                        serde_json::from_slice(body).unwrap_or(serde_json::json!({}));
                    let sent_sha = pb["sha"].as_str().unwrap_or("");
                    let expected = st.sha.get(&file_path).cloned().unwrap_or_default();
                    if (!expected.is_empty() && sent_sha != expected)
                        || (expected.is_empty() && !sent_sha.is_empty())
                    {
                        return (409, String::new());
                    }
                    let dec = B64
                        .decode(pb["content"].as_str().unwrap_or(""))
                        .unwrap_or_default();
                    st.files.insert(file_path.clone(), dec);
                    let new_sha = if st.put_sha_empty {
                        String::new()
                    } else {
                        format!("blob-{}", st.puts)
                    };
                    st.sha.insert(file_path.clone(), new_sha.clone());
                    st.head = format!("head-{}", st.puts);
                    (
                        200,
                        serde_json::json!({"content": {"sha": new_sha}}).to_string(),
                    )
                }
                "DELETE" => {
                    st.deletes += 1;
                    let db: serde_json::Value =
                        serde_json::from_slice(body).unwrap_or(serde_json::json!({}));
                    let sent_sha = db["sha"].as_str().unwrap_or("");
                    let expected = st.sha.get(&file_path).cloned().unwrap_or_default();
                    if !expected.is_empty() && sent_sha != expected {
                        return (409, String::new());
                    }
                    st.files.remove(&file_path);
                    st.sha.remove(&file_path);
                    st.head = format!("head-{}-del", st.deletes);
                    (200, String::new())
                }
                _ => (400, String::new()),
            }
        } else {
            (404, String::new())
        }
    }

    async fn make_syncer(base: &str) -> Syncer {
        let dir = tempfile::tempdir().unwrap();
        let leaked: &'static tempfile::TempDir = Box::leak(Box::new(dir));
        let store = Store::open(leaked.path()).unwrap();
        let client = SyncClient::new(base, "o", "r", "", "tok", false);
        Syncer::new(store, client)
    }

    fn encode_encrypted_note(id: &str, body: &str, pin: &str) -> Vec<u8> {
        let now = Utc::now();
        let note = Note::new(
            id.to_string(),
            "t".to_string(),
            vec![],
            now,
            now,
            body.to_string(),
        );
        let key = crate::config::encryption_key(pin);
        note::encode_remote(&note, Some(&key)).unwrap()
    }

    #[tokio::test]
    async fn recover_remote_pin_checks_target_key_without_changing_active_key() {
        let st = Arc::new(Mutex::new(FakeState::new()));
        let base = start(st.clone()).await;
        let syncer = make_syncer(&base).await;
        let encrypted = encode_encrypted_note("20260901-143023-abcd", "secret", "654321");
        let mut state = st.lock().unwrap();
        state
            .files
            .insert("notes/20260901-143023-abcd.json".to_string(), encrypted);
        state.sha.insert(
            "notes/20260901-143023-abcd.json".to_string(),
            "sha-target".to_string(),
        );
        drop(state);

        assert!(syncer.recover_remote_pin("654321").await.unwrap());
        assert_eq!(syncer.encryption_key(), None);
        assert!(!syncer.recover_remote_pin("111111").await.unwrap());
        assert_eq!(syncer.encryption_key(), None);
    }

    #[tokio::test]
    async fn recover_remote_pin_ignores_plaintext_notes_and_accepts_empty_remote() {
        let state = Arc::new(Mutex::new(FakeState::new()));
        let base = start(state.clone()).await;
        let syncer = make_syncer(&base).await;
        let plain = encode_note("20260901-143023-abcd", "plain");
        {
            let mut state = state.lock().unwrap();
            state
                .files
                .insert("notes/20260901-143023-abcd.json".to_string(), plain);
            state.sha.insert(
                "notes/20260901-143023-abcd.json".to_string(),
                "sha-plain".to_string(),
            );
        }

        assert!(syncer.recover_remote_pin("123456").await.unwrap());
        assert!(syncer.recover_remote_pin("123456").await.unwrap());
    }

    #[tokio::test]
    async fn full_validation_fails_if_remote_note_cannot_be_decrypted() {
        let state = Arc::new(Mutex::new(FakeState::new()));
        let base = start(state.clone()).await;
        let syncer = make_syncer(&base).await;
        let encrypted = encode_encrypted_note("20260901-143023-abcd", "secret", "654321");
        {
            let mut remote = state.lock().unwrap();
            remote
                .files
                .insert("notes/20260901-143023-abcd.json".to_string(), encrypted);
            remote.sha.insert(
                "notes/20260901-143023-abcd.json".to_string(),
                "sha-remote".to_string(),
            );
        }
        assert!(syncer.validate().await.is_err());
    }

    #[tokio::test]
    async fn full_validation_succeeds_after_remote_note_is_decrypted() {
        let state = Arc::new(Mutex::new(FakeState::new()));
        let base = start(state.clone()).await;
        let syncer = make_syncer(&base).await;
        let encrypted = encode_encrypted_note("20260901-143023-abcd", "secret", "654321");
        {
            let mut remote = state.lock().unwrap();
            remote
                .files
                .insert("notes/20260901-143023-abcd.json".to_string(), encrypted);
            remote.sha.insert(
                "notes/20260901-143023-abcd.json".to_string(),
                "sha-remote".to_string(),
            );
        }
        assert!(syncer.validate().await.is_err());
        syncer.set_encryption_key(Some(crate::config::encryption_key("654321")));
        assert!(syncer.validate().await.is_ok());
    }

    fn encode_note(id: &str, body: &str) -> Vec<u8> {
        let now = Utc::now();
        let n = Note::new(
            id.to_string(),
            "t".to_string(),
            vec![],
            now,
            now,
            body.to_string(),
        );
        note::encode_remote(&n, None).unwrap()
    }

    fn write_remote(st: &Shared, id: &str, body: &str, sha: &str) {
        let enc = encode_note(id, body);
        let mut s = st.lock().unwrap();
        s.files.insert(format!("notes/{id}.json"), enc);
        s.sha.insert(format!("notes/{id}.json"), sha.to_string());
    }

    #[tokio::test]
    async fn reconcile_nothing_changed() {
        let st = Arc::new(Mutex::new(FakeState::new()));
        let base = start(st.clone()).await;
        let s = make_syncer(&base).await;
        let n = s.store.create("t", vec![], "same").unwrap();
        let enc = note::encode_remote(&n, None).unwrap();
        let blob = git_blob_sha(&enc);
        write_remote(&st, &n.id, "same", &blob);
        s.store.set_sha(&n.id, &blob).unwrap();
        st.lock().unwrap().head = "H".to_string();
        s.store.set_sha(HEAD_KEY, "H").unwrap();

        let res = s.sync_all().await.unwrap();
        assert!(res.is_empty(), "expected no results: {res:?}");
        let g = st.lock().unwrap();
        assert_eq!(g.puts, 0);
        assert_eq!(g.gets, 0);
    }

    #[tokio::test]
    async fn only_local_changed_pushes() {
        let st = Arc::new(Mutex::new(FakeState::new()));
        let base = start(st.clone()).await;
        let s = make_syncer(&base).await;
        let n = s.store.create("t", vec![], "local edited").unwrap();
        s.store.set_sha(&n.id, "base-sha").unwrap();
        write_remote(&st, &n.id, "old remote", "base-sha");
        st.lock().unwrap().head = "H".to_string();
        s.store.set_sha(HEAD_KEY, "H").unwrap();
        s.mark_unsynced(&n.id);

        let res = s.push(&n.id).await.unwrap();
        assert!(!res.conflicted);
        assert_eq!(st.lock().unwrap().puts, 1);
        assert!(!s.is_failed(&n.id));
        let remote = st.lock().unwrap().files[&format!("notes/{}.json", n.id)].clone();
        assert!(String::from_utf8_lossy(&remote).contains("local edited"));
    }

    #[tokio::test]
    async fn only_remote_changed_pulls() {
        let st = Arc::new(Mutex::new(FakeState::new()));
        let base = start(st.clone()).await;
        let s = make_syncer(&base).await;
        let n = s.store.create("t", vec![], "base body").unwrap();
        let base_blob = git_blob_sha(&note::encode_remote(&n, None).unwrap());
        s.store.set_sha(&n.id, &base_blob).unwrap();
        write_remote(&st, &n.id, "remote updated", "remote-sha");
        {
            let mut g = st.lock().unwrap();
            g.head = "H1".to_string();
            g.compares.insert(
                "H0...H1".to_string(),
                vec![(format!("notes/{}.json", n.id), "modified".to_string())],
            );
        }
        s.store.set_sha(HEAD_KEY, "H0").unwrap();

        let res = s.sync_all().await.unwrap();
        assert_eq!(res.len(), 1);
        let loaded = s.store.load(&n.id).unwrap();
        assert_eq!(loaded.body, "remote updated");
        assert_eq!(s.store.sha(HEAD_KEY).unwrap().unwrap(), "H1");
    }

    #[tokio::test]
    async fn both_changed_merges() {
        let st = Arc::new(Mutex::new(FakeState::new()));
        let base = start(st.clone()).await;
        let s = make_syncer(&base).await;
        let n = s.store.create("t", vec![], "line1\nAAA\nline3\n").unwrap();
        s.store.set_sha(&n.id, "base-sha").unwrap();
        write_remote(&st, &n.id, "line1\nBBB\nline3\n", "remote-sha");
        {
            let mut g = st.lock().unwrap();
            g.head = "H1".to_string();
            g.compares.insert(
                "H0...H1".to_string(),
                vec![(format!("notes/{}.json", n.id), "modified".to_string())],
            );
        }
        s.store.set_sha(HEAD_KEY, "H0").unwrap();
        s.mark_unsynced(&n.id);

        let res = s.sync_all().await.unwrap();
        assert_eq!(res.len(), 1);
        assert!(res[0].conflicted);
        let loaded = s.store.load(&n.id).unwrap();
        assert!(conflict::has_markers(&loaded.body));
        assert!(loaded.body.contains("AAA") && loaded.body.contains("BBB"));
        assert!(st.lock().unwrap().puts >= 1);
        let remote = st.lock().unwrap().files[&format!("notes/{}.json", n.id)].clone();
        let remote_note = note::decode_remote(&remote, None).unwrap();
        assert!(conflict::has_markers(&remote_note.body));
        assert!(s.is_failed(&n.id));
        assert_ne!(s.store.sha(HEAD_KEY).unwrap().unwrap_or_default(), "H1");
    }

    #[tokio::test]
    async fn remote_deleted_local_clean_accepts_removal() {
        let st = Arc::new(Mutex::new(FakeState::new()));
        let base = start(st.clone()).await;
        let s = make_syncer(&base).await;
        let n = s.store.create("t", vec![], "body").unwrap();
        s.store
            .set_sha(
                &n.id,
                &git_blob_sha(&note::encode_remote(&n, None).unwrap()),
            )
            .unwrap();
        {
            let mut g = st.lock().unwrap();
            g.head = "H1".to_string();
            g.compares.insert(
                "H0...H1".to_string(),
                vec![(format!("notes/{}.json", n.id), "removed".to_string())],
            );
        }
        s.store.set_sha(HEAD_KEY, "H0").unwrap();
        s.sync_all().await.unwrap();
        assert!(s.store.load(&n.id).is_err());
    }

    #[tokio::test]
    async fn remote_deleted_local_dirty_recreates() {
        let st = Arc::new(Mutex::new(FakeState::new()));
        let base = start(st.clone()).await;
        let s = make_syncer(&base).await;
        let n = s.store.create("t", vec![], "local edited").unwrap();
        s.store.set_sha(&n.id, "base-sha").unwrap();
        {
            let mut g = st.lock().unwrap();
            g.head = "H1".to_string();
            g.compares.insert(
                "H0...H1".to_string(),
                vec![(format!("notes/{}.json", n.id), "removed".to_string())],
            );
        }
        s.store.set_sha(HEAD_KEY, "H0").unwrap();
        s.mark_unsynced(&n.id);
        s.sync_all().await.unwrap();
        assert!(s.store.load(&n.id).is_ok());
        assert!(st
            .lock()
            .unwrap()
            .files
            .contains_key(&format!("notes/{}.json", n.id)));
        assert!(!s.is_failed(&n.id));
    }

    #[tokio::test]
    async fn local_deleted_remote_clean_deletes_remote() {
        let st = Arc::new(Mutex::new(FakeState::new()));
        let base = start(st.clone()).await;
        let s = make_syncer(&base).await;
        let id = s.store.new_id().unwrap();
        write_remote(&st, &id, "body", "remote-sha");
        s.store.set_sha(&id, "remote-sha").unwrap();
        s.store.mark_deleted(&id).unwrap();
        s.mark_unsynced(&id);
        st.lock().unwrap().head = "H0".to_string();
        s.store.set_sha(HEAD_KEY, "H0").unwrap();
        s.sync_all().await.unwrap();
        assert!(!st
            .lock()
            .unwrap()
            .files
            .contains_key(&format!("notes/{id}.json")));
        assert_eq!(st.lock().unwrap().deletes, 1);
        assert!(!s.is_failed(&id));
    }

    #[tokio::test]
    async fn local_deleted_remote_changed_keeps_remote() {
        let st = Arc::new(Mutex::new(FakeState::new()));
        let base = start(st.clone()).await;
        let s = make_syncer(&base).await;
        let id = s.store.new_id().unwrap();
        write_remote(&st, &id, "remote new body", "remote-new-sha");
        s.store.set_sha(&id, "old-remote-sha").unwrap();
        s.store.mark_deleted(&id).unwrap();
        s.mark_unsynced(&id);
        {
            let mut g = st.lock().unwrap();
            g.head = "H1".to_string();
            g.compares.insert(
                "H0...H1".to_string(),
                vec![(format!("notes/{id}.json"), "modified".to_string())],
            );
        }
        s.store.set_sha(HEAD_KEY, "H0").unwrap();
        s.sync_all().await.unwrap();
        let loaded = s.store.load(&id).unwrap();
        assert_eq!(loaded.body, "remote new body");
        assert!(st
            .lock()
            .unwrap()
            .files
            .contains_key(&format!("notes/{id}.json")));
    }

    #[tokio::test]
    async fn head_advances_when_clean() {
        let st = Arc::new(Mutex::new(FakeState::new()));
        let base = start(st.clone()).await;
        let s = make_syncer(&base).await;
        let n = s.store.create("t", vec![], "local edit").unwrap();
        let enc = note::encode_remote(&n, None).unwrap();
        write_remote(&st, &n.id, "local edit", &git_blob_sha(&enc));
        s.store.set_sha(&n.id, &git_blob_sha(&enc)).unwrap();
        {
            let mut g = st.lock().unwrap();
            g.head = "H1".to_string();
            g.compares.insert("H0...H1".to_string(), vec![]);
        }
        s.store.set_sha(HEAD_KEY, "H0").unwrap();
        s.sync_all().await.unwrap();
        assert_eq!(s.store.sha(HEAD_KEY).unwrap().unwrap(), "H1");
    }

    #[tokio::test]
    async fn bootstrap_lists_when_local_head_empty() {
        let st = Arc::new(Mutex::new(FakeState::new()));
        let base = start(st.clone()).await;
        let s = make_syncer(&base).await;
        let id = s.store.new_id().unwrap();
        write_remote(&st, &id, "hello", "remote-sha");
        st.lock().unwrap().head = "H".to_string();
        s.sync_all().await.unwrap();
        let loaded = s.store.load(&id).unwrap();
        assert_eq!(loaded.body, "hello");
        assert_eq!(s.store.sha(HEAD_KEY).unwrap().unwrap(), "H");
    }

    #[tokio::test]
    async fn push_single_note_quick_path() {
        let st = Arc::new(Mutex::new(FakeState::new()));
        let base = start(st.clone()).await;
        let s = make_syncer(&base).await;
        let n = s.store.create("t", vec![], "body").unwrap();
        s.mark_unsynced(&n.id);
        let r = s.push(&n.id).await.unwrap();
        assert!(!r.conflicted);
        assert!(st
            .lock()
            .unwrap()
            .files
            .contains_key(&format!("notes/{}.json", n.id)));
        assert!(!s.is_failed(&n.id));
    }

    #[tokio::test]
    async fn encrypted_sync_keeps_local_plaintext_and_remote_ciphertext() {
        let st = Arc::new(Mutex::new(FakeState::new()));
        let base = start(st).await;
        let syncer = make_syncer(&base).await;
        let key = test_key();
        let syncer = syncer;
        *syncer.encryption_key.write().unwrap() = Some(key);
        let note = syncer
            .store
            .create("t", vec![], "private paragraph")
            .unwrap();
        syncer.push(&note.id).await.unwrap();
        let local = syncer.store.load(&note.id).unwrap();
        assert_eq!(local.body, "private paragraph");
        let remote = syncer.client.get(&note_path(&note.id)).await.unwrap().0;
        assert!(!remote.contains("private paragraph"));
        assert_eq!(
            note::decode_remote(remote.as_bytes(), Some(&key))
                .unwrap()
                .body,
            "private paragraph"
        );
    }

    #[tokio::test]
    async fn rewrite_all_reencodes_all_notes_for_new_mode() {
        let st = Arc::new(Mutex::new(FakeState::new()));
        let base = start(st.clone()).await;
        let plain = make_syncer(&base).await;
        let n1 = plain.store.create("one", vec![], "alpha\n\nbeta").unwrap();
        let n2 = plain.store.create("two", vec![], "gamma").unwrap();
        plain.push(&n1.id).await.unwrap();
        plain.push(&n2.id).await.unwrap();

        let key = test_key();
        *plain.encryption_key.write().unwrap() = Some(key);
        assert_eq!(plain.rewrite_all().await.unwrap(), 2);
        let remote = st.lock().unwrap();
        for n in [&n1, &n2] {
            let path = format!("notes/{}.json", n.id);
            let wire = String::from_utf8(remote.files[&path].clone()).unwrap();
            assert!(wire.contains("body_encryption_version"));
            assert!(!wire.contains(&n.body));
            assert_eq!(
                note::decode_remote(wire.as_bytes(), Some(&key))
                    .unwrap()
                    .body,
                n.body
            );
        }
    }

    #[tokio::test]
    async fn rewrite_all_recovers_when_remote_already_has_target_ciphertext_but_local_sha_is_stale()
    {
        let st = Arc::new(Mutex::new(FakeState::new()));
        let base = start(st.clone()).await;
        let plain = make_syncer(&base).await;
        let note = plain.store.create("one", vec![], "alpha").unwrap();
        plain.push(&note.id).await.unwrap();
        let key = test_key();
        *plain.encryption_key.write().unwrap() = Some(key);
        plain.rewrite_all().await.unwrap();
        plain.store.set_sha(&note.id, "stale-sha").unwrap();

        let result = plain.rewrite_all().await;

        assert!(
            result.is_ok(),
            "rewrite must recover from a stale SHA: {result:?}"
        );
        let remote = st.lock().unwrap();
        let wire =
            String::from_utf8(remote.files[&format!("notes/{}.json", note.id)].clone()).unwrap();
        assert_eq!(
            note::decode_remote(wire.as_bytes(), Some(&key))
                .unwrap()
                .body,
            "alpha"
        );
    }

    #[tokio::test]
    async fn rewrite_all_disable_encryption_overwrites_ciphertext_as_plaintext() {
        let st = Arc::new(Mutex::new(FakeState::new()));
        let base = start(st.clone()).await;
        let syncer = make_syncer(&base).await;
        let note = syncer.store.create("one", vec![], "private text").unwrap();
        let key = test_key();
        syncer
            .change_encryption_and_rewrite(Some(key))
            .await
            .unwrap();
        syncer.change_encryption_and_rewrite(None).await.unwrap();
        let remote = st.lock().unwrap();
        let wire =
            String::from_utf8(remote.files[&format!("notes/{}.json", note.id)].clone()).unwrap();
        assert!(wire.contains("private text"));
        assert!(!wire.contains("body_encryption_version"));
    }

    #[tokio::test]
    async fn rewrite_all_preserves_remote_notes_missing_locally() {
        let st = Arc::new(Mutex::new(FakeState::new()));
        let base = start(st.clone()).await;
        let syncer = make_syncer(&base).await;
        let note = syncer.store.create("one", vec![], "keep local").unwrap();
        syncer.push(&note.id).await.unwrap();
        write_remote(&st, "20250101-000000-dead", "stale remote", "old-sha");

        syncer.rewrite_all().await.unwrap();

        assert!(st
            .lock()
            .unwrap()
            .files
            .contains_key("notes/20250101-000000-dead.json"));
        assert!(st
            .lock()
            .unwrap()
            .files
            .contains_key(&format!("notes/{}.json", note.id)));
    }

    #[tokio::test]
    async fn rewrite_all_rewrites_when_remote_head_changes_during_bulk_operation() {
        let st = Arc::new(Mutex::new(FakeState::new()));
        let base = start(st.clone()).await;
        let syncer = make_syncer(&base).await;
        let first = syncer.store.create("one", vec![], "first").unwrap();
        let second = syncer.store.create("two", vec![], "second").unwrap();
        syncer.push(&first.id).await.unwrap();
        syncer.push(&second.id).await.unwrap();
        let start_puts = st.lock().unwrap().puts;

        let result = syncer.rewrite_all().await.unwrap();

        assert_eq!(result, 2);
        assert_eq!(st.lock().unwrap().puts, start_puts + 2);
        assert_ne!(syncer.store.sha(&first.id).unwrap().unwrap(), "");
        assert_ne!(syncer.store.sha(&second.id).unwrap().unwrap(), "");
    }

    #[tokio::test]
    async fn rewrite_all_retries_one_sha_conflict_with_fresh_remote_sha() {
        let st = Arc::new(Mutex::new(FakeState::new()));
        let base = start(st.clone()).await;
        let syncer = make_syncer(&base).await;
        let note = syncer.store.create("one", vec![], "body").unwrap();
        syncer.push(&note.id).await.unwrap();
        st.lock()
            .unwrap()
            .conflict_once
            .insert(format!("notes/{}.json", note.id));

        let key = test_key();
        syncer
            .change_encryption_and_rewrite(Some(key))
            .await
            .unwrap();

        let wire =
            String::from_utf8(st.lock().unwrap().files[&format!("notes/{}.json", note.id)].clone())
                .unwrap();
        assert_eq!(
            note::decode_remote(wire.as_bytes(), Some(&key))
                .unwrap()
                .body,
            "body"
        );
    }

    #[tokio::test]
    async fn on_note_changed_fires_on_pull() {
        let st = Arc::new(Mutex::new(FakeState::new()));
        let base = start(st.clone()).await;
        let mut s = make_syncer(&base).await;
        let got = Arc::new(Mutex::new(Vec::<String>::new()));
        let got2 = got.clone();
        s.on_note_changed = Some(Box::new(move |id| {
            got2.lock().unwrap().push(id.to_string())
        }));
        let id = s.store.new_id().unwrap();
        write_remote(&st, &id, "remote body", "rsha");
        st.lock().unwrap().head = "H1".to_string();
        s.sync_all().await.unwrap();
        let g = got.lock().unwrap();
        assert!(!g.is_empty() && g[0] == id);
    }

    #[tokio::test]
    async fn branch_reset_recreates_all_locals() {
        let st = Arc::new(Mutex::new(FakeState::new()));
        let base = start(st.clone()).await;
        let s = make_syncer(&base).await;
        let n1 = s.store.create("a", vec![], "alpha").unwrap();
        let n2 = s.store.create("b", vec![], "beta").unwrap();
        s.store
            .set_sha(
                &n1.id,
                &git_blob_sha(&note::encode_remote(&n1, None).unwrap()),
            )
            .unwrap();
        s.store
            .set_sha(
                &n2.id,
                &git_blob_sha(&note::encode_remote(&n2, None).unwrap()),
            )
            .unwrap();
        s.store.set_sha(HEAD_KEY, "OLD-HEAD-GONE").unwrap();
        {
            let mut g = st.lock().unwrap();
            g.head = "H-fresh".to_string();
            g.files.clear();
            g.sha.clear();
        }
        s.sync_all().await.unwrap();
        let g = st.lock().unwrap();
        assert!(g.files.contains_key(&format!("notes/{}.json", n1.id)));
        assert!(g.files.contains_key(&format!("notes/{}.json", n2.id)));
        drop(g);
        assert!(!s.is_failed(&n1.id) && !s.is_failed(&n2.id));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_pushes_serialize_without_interleaving() {
        let st = Arc::new(Mutex::new(FakeState::new()));
        let base = start(st.clone()).await;
        let s = Arc::new(make_syncer(&base).await);
        let mut ids = Vec::new();
        for i in 0..8 {
            let n = s.store.create("t", vec![], &format!("body {i}")).unwrap();
            s.mark_unsynced(&n.id);
            ids.push(n.id);
        }
        let mut handles = Vec::new();
        for id in ids.clone() {
            let s = s.clone();
            handles.push(tokio::spawn(async move { s.push(&id).await }));
        }
        for h in handles {
            let r = h.await.unwrap().unwrap();
            assert!(!r.conflicted);
        }
        let g = st.lock().unwrap();
        assert_eq!(
            g.puts, 8,
            "each note pushed exactly once under serial queue"
        );
        for id in &ids {
            assert!(g.files.contains_key(&format!("notes/{id}.json")));
        }
        drop(g);
        for id in &ids {
            assert!(!s.is_failed(id));
        }
    }
}
