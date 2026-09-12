mod portio;

use crate::local::{Store, StoreError};
use crate::note::{self, Note, NoteMeta};
use crate::sync::{EngineError, SyncResult, Syncer};
use crate::syncclient::SyncClient;

pub use portio::{ExportFormat, ExportView, ExportViewKind};

pub struct Config {
    pub root: std::path::PathBuf,
    pub base_url: String,
    pub owner: String,
    pub repo: String,
    pub username: String,
    pub token: String,
    pub use_api: bool,
    pub on_note_changed: Option<Box<dyn Fn(&str) + Send + Sync>>,
}

pub struct App {
    syncer: Syncer,
    configured: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Revision {
    pub sha: String,
    pub author: String,
    pub date: String,
    pub subject: String,
}

fn note_path(id: &str) -> String {
    format!("notes/{id}.json")
}

impl App {
    pub fn new(cfg: Config) -> Result<App, EngineError> {
        let store = Store::open(&cfg.root)?;
        let client = SyncClient::new(
            &cfg.base_url,
            &cfg.owner,
            &cfg.repo,
            &cfg.username,
            &cfg.token,
            cfg.use_api,
        );
        let mut syncer = Syncer::new(store, client);
        syncer.on_note_changed = cfg.on_note_changed;
        let _ = syncer.load_failed();
        Ok(App {
            syncer,
            configured: !cfg.base_url.is_empty(),
        })
    }

    pub(crate) fn store(&self) -> &Store {
        &self.syncer.store
    }

    pub fn list_notes(&self) -> Result<Vec<NoteMeta>, EngineError> {
        let mut metas = self.store().list_meta()?;
        self.apply_sync_failed(&mut metas);
        Ok(metas)
    }

    fn apply_sync_failed(&self, metas: &mut [NoteMeta]) {
        if !self.configured {
            return;
        }
        let failed = self.syncer.failed_ids();
        if failed.is_empty() {
            return;
        }
        for m in metas.iter_mut() {
            if failed.contains(&m.id) {
                m.sync_failed = true;
            }
        }
    }

    pub fn all_tags(&self) -> Result<Vec<String>, EngineError> {
        let metas = self.store().list_meta()?;
        Ok(note::all_meta_tags(&metas))
    }

    pub fn notes_by_tag(&self, tag: &str) -> Result<Vec<NoteMeta>, EngineError> {
        let metas = self.store().list_meta()?;
        let mut filtered: Vec<NoteMeta> =
            note::filter_meta(&metas, tag).into_iter().cloned().collect();
        self.apply_sync_failed(&mut filtered);
        Ok(filtered)
    }

    pub fn search_notes(&self, query: &str) -> Result<Vec<NoteMeta>, EngineError> {
        let mut metas = self.store().search_meta(query)?;
        self.apply_sync_failed(&mut metas);
        Ok(metas)
    }

    pub fn load_note(&self, id: &str) -> Result<Note, EngineError> {
        Ok(self.store().load(id)?)
    }

    pub fn create_note(
        &self,
        title: &str,
        tags: Vec<String>,
        body: &str,
    ) -> Result<Note, EngineError> {
        let n = self.store().create(title, tags, body)?;
        self.syncer.mark_unsynced(&n.id);
        Ok(n)
    }

    pub fn update_note(
        &self,
        id: &str,
        title: &str,
        tags: Vec<String>,
        body: &str,
    ) -> Result<Note, EngineError> {
        let n = self.store().update(id, Some(title), Some(tags), body)?;
        self.syncer.mark_unsynced(id);
        Ok(n)
    }

    pub fn delete_note(&self, id: &str) -> Result<(), EngineError> {
        self.store().mark_deleted(id)?;
        self.syncer.mark_unsynced(id);
        Ok(())
    }

    pub fn duplicate_note(&self, id: &str) -> Result<Note, EngineError> {
        Ok(self.store().duplicate(id)?)
    }

    pub fn set_pinned(&self, id: &str, pinned: bool) -> Result<(), EngineError> {
        Ok(self.store().set_pinned(id, pinned)?)
    }

    pub fn set_note_file_type(&self, id: &str, ft: &str) -> Result<(), EngineError> {
        match ft {
            "" | "md" | "sh" | "json" => {}
            _ => return Ok(()),
        }
        self.store().set_file_type(id, ft)?;
        self.syncer.mark_unsynced(id);
        Ok(())
    }

    pub fn set_trashed(&self, id: &str, trashed: bool) -> Result<(), EngineError> {
        Ok(self.store().set_trashed(id, trashed)?)
    }

    pub fn purge_expired_trash(&self) -> Result<Vec<String>, EngineError> {
        Ok(self.store().purge_expired_trash()?)
    }

    pub async fn push_note(&self, id: &str) -> Result<SyncResult, EngineError> {
        self.syncer.push(id).await
    }

    pub async fn push_delete(&self, id: &str) -> Result<(), EngineError> {
        self.syncer.push_delete(id).await
    }

    pub async fn pull(&self) -> Result<Vec<String>, EngineError> {
        self.syncer.pull().await
    }

    pub async fn validate(&self) -> Result<usize, EngineError> {
        self.syncer.validate().await?;
        let metas = self.store().list_meta()?;
        Ok(metas.len())
    }

    pub async fn get_server_remote(&self) -> Result<String, EngineError> {
        Ok(self.syncer.client.get_remote().await?)
    }

    pub async fn reclone_server(&self, new_url: &str) -> Result<(), EngineError> {
        Ok(self.syncer.client.post_reclone(new_url).await?)
    }

    pub async fn revision_list(
        &self,
        id: &str,
        page: i64,
        per_page: i64,
    ) -> Result<Vec<Revision>, EngineError> {
        let page = if page <= 0 { 1 } else { page };
        let per_page = if per_page <= 0 { 30 } else { per_page };
        let commits = self
            .syncer
            .client
            .file_history(&note_path(id), page, per_page)
            .await?;
        Ok(commits
            .into_iter()
            .map(|c| Revision {
                sha: c.sha,
                author: c.author_name,
                date: c.date,
                subject: c.subject,
            })
            .collect())
    }

    pub async fn revision_diff(&self, id: &str, sha: &str) -> Result<String, EngineError> {
        Ok(self.syncer.client.commit_patch(sha, &note_path(id)).await?)
    }

    pub async fn revision_body(&self, id: &str, sha: &str) -> Result<String, EngineError> {
        let (content, _) = self.syncer.client.get_at_ref(&note_path(id), sha).await?;
        if content.is_empty() {
            return Ok(String::new());
        }
        match note::decode(content.as_bytes()) {
            Ok(n) => Ok(n.body),
            Err(_) => Ok(content),
        }
    }

    pub async fn restore_revision(&self, id: &str, sha: &str) -> Result<(), EngineError> {
        let (content, _) = self.syncer.client.get_at_ref(&note_path(id), sha).await?;
        let n = note::decode(content.as_bytes()).map_err(StoreError::from)?;
        self.store().update(id, Some(&n.title), Some(n.tags), &n.body)?;
        self.syncer.mark_unsynced(id);
        self.syncer.push(id).await.map(|_| ())
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;

    pub fn test_app() -> App {
        App::new(Config {
            root: tempfile::tempdir().unwrap().keep(),
            base_url: "http://example.invalid".to_string(),
            owner: "o".to_string(),
            repo: "r".to_string(),
            username: String::new(),
            token: "tok".to_string(),
            use_api: true,
            on_note_changed: None,
        })
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::test_app;
    use super::*;

    #[test]
    fn create_list_load() {
        let a = test_app();
        let n = a.create_note("标题", vec!["医案".into()], "body").unwrap();
        let list = a.list_notes().unwrap();
        assert_eq!(list.len(), 1);
        let got = a.load_note(&n.id).unwrap();
        assert_eq!(got.body, "body");
    }

    #[test]
    fn tags_and_filter() {
        let a = test_app();
        a.create_note("A", vec!["医案".into(), "太阳病".into()], "a").unwrap();
        a.create_note("B", vec!["笔记".into()], "b").unwrap();
        assert_eq!(a.all_tags().unwrap().len(), 3);
        assert_eq!(a.notes_by_tag("医案").unwrap().len(), 1);
    }

    #[test]
    fn update_and_delete() {
        let a = test_app();
        let n = a.create_note("t", vec![], "a").unwrap();
        let up = a.update_note(&n.id, "t2", vec!["x".into()], "b").unwrap();
        assert_eq!(up.title, "t2");
        assert_eq!(up.body, "b");
        a.delete_note(&n.id).unwrap();
        assert!(a.load_note(&n.id).is_err());
    }

    #[tokio::test]
    async fn list_notes_marks_sync_failed() {
        let a = test_app();
        let n = a.create_note("t", vec![], "body").unwrap();
        assert!(a.push_note(&n.id).await.is_err());
        let metas = a.list_notes().unwrap();
        let m = metas.iter().find(|m| m.id == n.id).expect("note in list");
        assert!(m.sync_failed);
    }

    #[tokio::test]
    async fn sync_failed_hidden_when_unconfigured() {
        let a = App::new(Config {
            root: tempfile::tempdir().unwrap().keep(),
            base_url: String::new(),
            owner: String::new(),
            repo: String::new(),
            username: String::new(),
            token: String::new(),
            use_api: false,
            on_note_changed: None,
        })
        .unwrap();
        let n = a.create_note("t", vec![], "body").unwrap();
        let _ = a.push_note(&n.id).await;
        let metas = a.list_notes().unwrap();
        for m in metas {
            if m.id == n.id {
                assert!(!m.sync_failed);
            }
        }
    }
}
