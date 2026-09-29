use crate::app::{self, App, Revision};
#[cfg(feature = "desktop")]
use crate::app::{ExportFormat, ExportView, ExportViewKind};
use crate::config::{self, Config, PublicConfig};
use crate::note::{Note, NoteMeta};
use crate::sync::SyncResult;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
#[cfg(feature = "desktop")]
use tauri_plugin_dialog::{DialogExt, FilePath};

struct Inner {
    app: Option<Arc<App>>,
    cfg: Config,
    root: PathBuf,
    encryption_unlocked: bool,
    encryption_busy: bool,
    encryption_failures: u8,
    encryption_locked_until: i64,
}

struct AppState {
    inner: Mutex<Inner>,
    sync_gate: SyncGate,
}

fn sync_error_event(root: &Path, error: String) -> SyncErrorEvent {
    SyncErrorEvent {
        message: error,
        lock_until: config::load_config(root).encryption_locked_until,
    }
}

fn autosync_ready(inner: &Inner) -> bool {
    !inner.cfg.base_url.is_empty()
        && !inner.cfg.encryption_transition
        && !inner.encryption_busy
        && inner.encryption_unlocked
        && inner.encryption_locked_until <= chrono::Utc::now().timestamp()
}

#[derive(Clone, serde::Serialize)]
struct SyncErrorEvent {
    message: String,
    lock_until: i64,
}

struct SyncGate {
    paused: AtomicBool,
    operation: tokio::sync::Mutex<()>,
}

fn settings_transition_config(old: &Config, mut cfg: Config, target_pin: &str) -> Config {
    let changed = old.encryption_pin != target_pin;
    if changed && !old.encryption_transition {
        cfg.encryption_failures = 0;
        cfg.encryption_locked_until = 0;
        cfg.encryption_transition = true;
        cfg.encryption_transition_pin = target_pin.to_string();
    } else {
        cfg.encryption_transition = old.encryption_transition;
        cfg.encryption_transition_pin = if old.encryption_transition {
            old.encryption_transition_pin.clone()
        } else {
            String::new()
        };
        cfg.encryption_failures = old.encryption_failures;
        cfg.encryption_locked_until = old.encryption_locked_until;
    }
    cfg
}

fn settings_target_pin(old: &Config, submitted: &str) -> String {
    if submitted == "••••••" {
        if old.encryption_transition {
            old.encryption_transition_pin.clone()
        } else {
            old.encryption_pin.clone()
        }
    } else {
        submitted.to_string()
    }
}

fn settings_pin_valid(pin: &str, configured: bool) -> bool {
    config::valid_encryption_pin(pin) || (pin == "••••••" && configured)
}

impl SyncGate {
    fn new() -> Self {
        Self {
            paused: AtomicBool::new(false),
            operation: tokio::sync::Mutex::new(()),
        }
    }

    fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    async fn begin_settings_validation(&self) {
        self.paused.store(true, Ordering::Release);
        let guard = self.operation.lock().await;
        drop(guard);
    }

    fn settings_closed(&self) {
        self.paused.store(false, Ordering::Release);
    }

    async fn lock_remote(&self) -> Result<tokio::sync::MutexGuard<'_, ()>, String> {
        let guard = self.operation.lock().await;
        if self.is_paused() {
            return Err("Sync paused while Settings is open".to_string());
        }
        Ok(guard)
    }
}

#[derive(serde::Serialize)]
struct MismatchInfo {
    mismatch: bool,
    server_url: String,
}

#[cfg(feature = "desktop")]
fn default_root() -> PathBuf {
    let home = if cfg!(windows) {
        std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME"))
    } else {
        std::env::var("HOME")
    }
    .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".draftnote")
}

fn build_app(handle: &AppHandle, cfg: &Config, root: &Path) -> Result<Arc<App>, String> {
    let h = handle.clone();
    let mut app = App::new(app::Config {
        root: root.to_path_buf(),
        base_url: cfg.base_url.clone(),
        owner: cfg.owner.clone(),
        repo: cfg.repo.clone(),
        username: cfg.username.clone(),
        token: cfg.token.clone(),
        use_api: cfg.use_api,
        encryption_pin: cfg.encryption_pin.clone(),
        on_note_changed: Some(Box::new(move |id| {
            let _ = h.emit("note-body-changed", id.to_string());
        })),
    })
    .map_err(|e| e.to_string())?;
    let error_handle = handle.clone();
    let error_root = root.to_path_buf();
    app.set_sync_error_callback(Arc::new(move |error| {
        let _ = error_handle.emit("sync-error", sync_error_event(&error_root, error));
    }));
    Ok(Arc::new(app))
}

fn current_app(state: &State<'_, AppState>) -> Option<Arc<App>> {
    let g = state.inner.lock().unwrap();
    if !g.encryption_unlocked
        || g.encryption_busy
        || g.cfg.encryption_transition
        || g.encryption_locked_until > chrono::Utc::now().timestamp()
    {
        return None;
    }
    g.app.clone()
}

fn local_app(state: &State<'_, AppState>) -> Option<Arc<App>> {
    state.inner.lock().unwrap().app.clone()
}

fn remote_app(state: &State<'_, AppState>) -> Result<Arc<App>, String> {
    let inner = state.inner.lock().unwrap();
    if inner.encryption_locked_until > chrono::Utc::now().timestamp() {
        return Err(format!(
            "PIN verification locked until {}",
            inner.encryption_locked_until
        ));
    }
    if inner.encryption_busy || inner.cfg.encryption_transition {
        return Err("Encryption change is incomplete; validate settings to resume".to_string());
    }
    if !inner.encryption_unlocked {
        return Err("Encryption is locked".to_string());
    }
    inner
        .app
        .clone()
        .ok_or_else(|| "Not configured".to_string())
}

#[cfg(feature = "desktop")]
fn path_string(f: FilePath) -> String {
    match f.into_path() {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(_) => String::new(),
    }
}

#[cfg(feature = "desktop")]
fn spawn_push_imported(handle: AppHandle, ids: Vec<String>) {
    tauri::async_runtime::spawn(async move {
        let state = handle.state::<AppState>();
        let _sync_guard = match state.sync_gate.lock_remote().await {
            Ok(guard) => guard,
            Err(_) => return,
        };
        let app = match remote_app(&state) {
            Ok(app) => app,
            Err(_) => return,
        };
        for id in &ids {
            let _ = app.push_note(id).await;
        }
        if !ids.is_empty() {
            let _ = handle.emit("notes-changed", ());
        }
    });
}

fn spawn_autosync(handle: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            let interval = {
                let st = handle.state::<AppState>();
                let g = st.inner.lock().unwrap();
                g.cfg.interval_seconds()
            };
            tokio::time::sleep(Duration::from_secs(interval.max(1) as u64)).await;
            let state = handle.state::<AppState>();
            let _sync_guard = match state.sync_gate.lock_remote().await {
                Ok(guard) => guard,
                Err(_) => continue,
            };
            let app = {
                let g = state.inner.lock().unwrap();
                if !autosync_ready(&g) {
                    continue;
                }
                g.app.clone()
            };
            if let Some(app) = app {
                if let Ok(pulled) = app.pull().await {
                    if !pulled.is_empty() {
                        let _ = handle.emit("notes-changed", ());
                    }
                }
            }
        }
    });
}

#[cfg(feature = "desktop")]
fn parse_export_view(view: &str) -> Result<ExportView, String> {
    match view {
        "all" => Ok(ExportView {
            kind: ExportViewKind::All,
            tag: String::new(),
        }),
        "trash" => Ok(ExportView {
            kind: ExportViewKind::Trash,
            tag: String::new(),
        }),
        "untagged" => Ok(ExportView {
            kind: ExportViewKind::Untagged,
            tag: String::new(),
        }),
        v if v.starts_with("tag:") => {
            let name = &v["tag:".len()..];
            if name.is_empty() {
                return Err("empty tag name".to_string());
            }
            Ok(ExportView {
                kind: ExportViewKind::Tag,
                tag: name.to_string(),
            })
        }
        _ => Err(format!("unknown view: {view:?}")),
    }
}

#[cfg(feature = "desktop")]
fn parse_export_format(format: &str) -> Result<ExportFormat, String> {
    match format {
        "md" => Ok(ExportFormat::Md),
        "txt" => Ok(ExportFormat::Txt),
        _ => Err(format!("unknown format: {format:?}")),
    }
}

#[tauri::command]
fn get_config(state: State<'_, AppState>) -> PublicConfig {
    let root = state.inner.lock().unwrap().root.clone();
    config::load_config(&root).public()
}

#[tauri::command]
async fn begin_settings_validation(state: State<'_, AppState>) -> Result<(), String> {
    state.sync_gate.begin_settings_validation().await;
    Ok(())
}

#[tauri::command]
fn settings_closed(state: State<'_, AppState>) {
    state.sync_gate.settings_closed();
}

#[tauri::command]
async fn configure(
    state: State<'_, AppState>,
    app_handle: AppHandle,
    repo_url: String,
    server_url: String,
    username: String,
    token: String,
    use_api: bool,
    sync_interval_seconds: i64,
    keep_token: bool,
    encryption_pin: String,
) -> Result<(), String> {
    let _sync_guard = state.sync_gate.lock_remote().await?;
    let root = state.inner.lock().unwrap().root.clone();
    let old_cfg = config::load_config(&root);
    if !settings_pin_valid(
        &encryption_pin,
        !old_cfg.encryption_pin.is_empty() || old_cfg.encryption_transition,
    ) {
        return Err("PIN must be empty or exactly 6 digits".to_string());
    }
    let target_pin = settings_target_pin(&old_cfg, &encryption_pin);
    let encryption_changed = old_cfg.encryption_pin != target_pin;
    if old_cfg.encryption_locked_until > chrono::Utc::now().timestamp() {
        return Err(format!(
            "PIN verification locked until {}",
            old_cfg.encryption_locked_until
        ));
    }
    let interval = if sync_interval_seconds <= 0 {
        config::DEFAULT_SYNC_INTERVAL_SECONDS
    } else {
        sync_interval_seconds
    };
    let (mut base, owner, repo) =
        config::parse_repo_url(&repo_url, use_api).map_err(|e| format!("parse repo URL: {e}"))?;
    let mut server = server_url.trim().to_string();
    if !server.is_empty() {
        server = server.trim_end_matches('/').to_string();
        base = server.clone();
    }
    let token = if keep_token {
        old_cfg.token.clone()
    } else {
        token
    };
    let cfg = Config {
        repo_url: repo_url.trim().to_string(),
        server_url: server,
        username: username.trim().to_string(),
        token,
        token_secret: String::new(),
        use_api,
        base_url: base,
        owner,
        repo,
        sync_interval_seconds: interval,
        mismatch_ack_server_url: String::new(),
        encryption_pin: old_cfg.encryption_pin.clone(),
        encryption_transition_pin: if old_cfg.encryption_transition {
            old_cfg.encryption_transition_pin.clone()
        } else {
            String::new()
        },
        encryption_transition_pin_secret: String::new(),
        encryption_pin_secret: String::new(),
        encryption_failures: old_cfg.encryption_failures,
        encryption_locked_until: old_cfg.encryption_locked_until,
        encryption_transition: old_cfg.encryption_transition,
    };
    let cfg = settings_transition_config(&old_cfg, cfg, &target_pin);
    if old_cfg.encryption_transition && target_pin != old_cfg.encryption_transition_pin {
        if !config::check_transition_pin(&root, &target_pin)? {
            let updated = config::load_config(&root);
            let mut inner = state.inner.lock().unwrap();
            inner.encryption_failures = updated.encryption_failures;
            inner.encryption_locked_until = updated.encryption_locked_until;
            return Err(
                if updated.encryption_locked_until > chrono::Utc::now().timestamp() {
                    format!(
                        "Incorrect Encryption PIN. PIN locked until {}",
                        updated.encryption_locked_until
                    )
                } else {
                    format!(
                        "Incorrect Encryption PIN ({}/3 failed attempts)",
                        updated.encryption_failures
                    )
                },
            );
        }
    }
    config::save_config(&root, &cfg).map_err(|e| e.to_string())?;
    if cfg.encryption_transition {
        let target_pin = cfg.encryption_transition_pin.clone();
        let (app, current_pin_sync_succeeded) = if !old_cfg.encryption_transition
            && encryption_changed
            && !old_cfg.encryption_pin.is_empty()
        {
            let current = Config {
                encryption_pin: old_cfg.encryption_pin.clone(),
                ..cfg.clone()
            };
            let app = build_app(&app_handle, &current, &root)?;
            let result = app.validate().await.map_err(|error| {
                format!("Cannot change Encryption PIN before current sync succeeds: {error}")
            });
            (app, Some(result))
        } else {
            let app_cfg = Config {
                encryption_pin: old_cfg.encryption_pin.clone(),
                ..cfg.clone()
            };
            (build_app(&app_handle, &app_cfg, &root)?, None)
        };
        {
            let mut inner = state.inner.lock().unwrap();
            inner.app = Some(app.clone());
            inner.cfg = cfg.clone();
            inner.encryption_busy = true;
        }
        if !old_cfg.encryption_transition
            && old_cfg.encryption_pin.is_empty()
            && encryption_changed
            && !app
                .recover_remote_pin(&target_pin)
                .await
                .map_err(|error| error.to_string())?
        {
            return Err("Incorrect Encryption PIN".to_string());
        }
        if let Some(result) = current_pin_sync_succeeded {
            result?;
        }
        config::clear_encryption_failures(&root)?;
        app.set_encryption_pin(&target_pin);
        {
            let mut inner = state.inner.lock().unwrap();
            inner.encryption_busy = true;
        }
        if let Err(error) = app.rewrite_all(&target_pin).await {
            return Err(error.to_string());
        }
        let mut completed = cfg.clone();
        completed.encryption_pin = target_pin;
        completed.encryption_failures = 0;
        completed.encryption_locked_until = 0;
        completed.encryption_transition = false;
        completed.encryption_transition_pin.clear();
        config::save_config(&root, &completed).map_err(|e| e.to_string())?;
        let app = build_app(&app_handle, &completed, &root)?;
        let mut inner = state.inner.lock().unwrap();
        inner.app = Some(app);
        inner.cfg = completed;
        inner.encryption_unlocked = true;
        inner.encryption_busy = false;
        inner.encryption_failures = 0;
        inner.encryption_locked_until = 0;
    } else {
        let app = build_app(&app_handle, &cfg, &root)?;
        let mut inner = state.inner.lock().unwrap();
        inner.app = Some(app);
        inner.cfg = cfg;
    }
    Ok(())
}

#[tauri::command]
fn list_notes(state: State<'_, AppState>) -> Result<Vec<NoteMeta>, String> {
    let app = local_app(&state).ok_or("Not configured")?;
    app.list_notes().map_err(|e| e.to_string())
}

#[tauri::command]
fn all_tags(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let app = local_app(&state).ok_or("Not configured")?;
    app.all_tags().map_err(|e| e.to_string())
}

#[tauri::command]
fn notes_by_tag(state: State<'_, AppState>, tag: String) -> Result<Vec<NoteMeta>, String> {
    let app = local_app(&state).ok_or("Not configured")?;
    app.notes_by_tag(&tag).map_err(|e| e.to_string())
}

#[tauri::command]
fn search_notes(state: State<'_, AppState>, q: String) -> Result<Vec<NoteMeta>, String> {
    let app = local_app(&state).ok_or("Not configured")?;
    app.search_notes(&q).map_err(|e| e.to_string())
}

#[tauri::command]
fn load_note(state: State<'_, AppState>, id: String) -> Result<Note, String> {
    let app = local_app(&state).ok_or("Not configured")?;
    app.load_note(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn create_note(
    state: State<'_, AppState>,
    title: String,
    tags: Vec<String>,
    body: String,
) -> Result<Note, String> {
    let app = current_app(&state).ok_or("Not configured")?;
    app.create_note(&title, tags, &body)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn update_note(
    state: State<'_, AppState>,
    id: String,
    title: String,
    tags: Vec<String>,
    body: String,
) -> Result<Note, String> {
    let app = current_app(&state).ok_or("Not configured")?;
    app.update_note(&id, &title, tags, &body)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_note(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let app = current_app(&state).ok_or("Not configured")?;
    app.delete_note(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn duplicate_note(state: State<'_, AppState>, id: String) -> Result<Note, String> {
    let app = current_app(&state).ok_or("Not configured")?;
    app.duplicate_note(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_pinned(state: State<'_, AppState>, id: String, pinned: bool) -> Result<(), String> {
    let app = current_app(&state).ok_or("Not configured")?;
    app.set_pinned(&id, pinned).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_note_file_type(state: State<'_, AppState>, id: String, ft: String) -> Result<(), String> {
    let app = current_app(&state).ok_or("Not configured")?;
    app.set_note_file_type(&id, &ft).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_trashed(state: State<'_, AppState>, id: String, trashed: bool) -> Result<(), String> {
    let app = current_app(&state).ok_or("Not configured")?;
    app.set_trashed(&id, trashed).map_err(|e| e.to_string())
}

#[tauri::command]
fn purge_expired_trash(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let app = current_app(&state).ok_or("Not configured")?;
    app.purge_expired_trash().map_err(|e| e.to_string())
}

#[tauri::command]
async fn push_note(state: State<'_, AppState>, id: String) -> Result<SyncResult, String> {
    let _sync_guard = state.sync_gate.lock_remote().await?;
    let app = remote_app(&state)?;
    app.push_note(&id).await.map_err(|error| error.to_string())
}

#[tauri::command]
async fn push_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let _sync_guard = state.sync_gate.lock_remote().await?;
    let app = remote_app(&state)?;
    app.push_delete(&id).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn revision_list(
    state: State<'_, AppState>,
    id: String,
    page: i64,
    per_page: i64,
) -> Result<Vec<Revision>, String> {
    let _sync_guard = state.sync_gate.lock_remote().await?;
    let app = remote_app(&state)?;
    app.revision_list(&id, page, per_page)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn revision_diff(
    state: State<'_, AppState>,
    id: String,
    sha: String,
) -> Result<String, String> {
    let _sync_guard = state.sync_gate.lock_remote().await?;
    let app = remote_app(&state)?;
    app.revision_diff(&id, &sha)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn revision_body(
    state: State<'_, AppState>,
    id: String,
    sha: String,
) -> Result<String, String> {
    let _sync_guard = state.sync_gate.lock_remote().await?;
    let app = remote_app(&state)?;
    app.revision_body(&id, &sha)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn restore_revision(
    state: State<'_, AppState>,
    id: String,
    sha: String,
) -> Result<(), String> {
    let _sync_guard = state.sync_gate.lock_remote().await?;
    let app = remote_app(&state)?;
    app.restore_revision(&id, &sha)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn validate_sync(state: State<'_, AppState>) -> Result<String, String> {
    let _sync_guard = state.sync_gate.lock_remote().await?;
    let (root, app) = {
        let g = state.inner.lock().unwrap();
        if g.cfg.encryption_transition || g.encryption_busy {
            return Err(
                "Encryption change is incomplete; enter the target PIN and validate to resume"
                    .to_string(),
            );
        }
        if g.encryption_locked_until > chrono::Utc::now().timestamp() {
            return Err(format!(
                "PIN verification locked until {}",
                g.encryption_locked_until
            ));
        }
        (g.root.clone(), g.app.clone())
    };
    let cfg = config::load_config(&root);
    config::preflight(&cfg)?;
    let app = app.ok_or("Not configured")?;
    match app.validate().await {
        Ok(count) => Ok(format!("OK: aligned {count} notes")),
        Err(e) => {
            let error = e.to_string();
            let lower = error.to_ascii_lowercase();
            if lower.contains("incorrect pin") || lower.contains("no pin is configured") {
                let updated = config::load_config(&root);
                return Err(
                    if updated.encryption_locked_until > chrono::Utc::now().timestamp() {
                        format!(
                            "Incorrect Encryption PIN. PIN locked until {}",
                            updated.encryption_locked_until
                        )
                    } else {
                        format!(
                            "Incorrect Encryption PIN ({}/3 failed attempts)",
                            updated.encryption_failures
                        )
                    },
                );
            }
            Err(config::map_validate_error(&cfg.base_url, &error))
        }
    }
}

#[tauri::command]
async fn check_remote_mismatch(state: State<'_, AppState>) -> Result<MismatchInfo, String> {
    let _sync_guard = state.sync_gate.lock_remote().await?;
    let (root, app) = {
        let g = state.inner.lock().unwrap();
        if g.encryption_busy || g.cfg.encryption_transition || !g.encryption_unlocked {
            return Err("Remote operations are unavailable during encryption change".to_string());
        }
        (g.root.clone(), g.app.clone())
    };
    let cfg = config::load_config(&root);
    let app = match app {
        Some(a) if !cfg.use_api && !cfg.repo_url.is_empty() => a,
        _ => {
            return Ok(MismatchInfo {
                mismatch: false,
                server_url: String::new(),
            })
        }
    };
    let server_url = app.get_server_remote().await.map_err(|e| e.to_string())?;
    if server_url.is_empty() {
        return Ok(MismatchInfo {
            mismatch: false,
            server_url: String::new(),
        });
    }
    if config::normalize_repo_url(&server_url) == config::normalize_repo_url(&cfg.repo_url) {
        return Ok(MismatchInfo {
            mismatch: false,
            server_url,
        });
    }
    if server_url == cfg.mismatch_ack_server_url {
        return Ok(MismatchInfo {
            mismatch: false,
            server_url,
        });
    }
    Ok(MismatchInfo {
        mismatch: true,
        server_url,
    })
}

#[tauri::command]
async fn reclone_server(state: State<'_, AppState>) -> Result<(), String> {
    let _sync_guard = state.sync_gate.lock_remote().await?;
    let _ = remote_app(&state)?;
    let (root, app) = {
        let g = state.inner.lock().unwrap();
        (g.root.clone(), g.app.clone())
    };
    let app = app.ok_or("Not configured")?;
    let cfg = config::load_config(&root);
    if cfg.repo_url.is_empty() {
        return Err("Missing repo URL".to_string());
    }
    app.reclone_server(&cfg.repo_url)
        .await
        .map_err(|e| e.to_string())?;
    if !cfg.mismatch_ack_server_url.is_empty() {
        let mut c = cfg;
        c.mismatch_ack_server_url = String::new();
        config::save_config(&root, &c).map_err(|e| e.to_string())?;
        state.inner.lock().unwrap().cfg = c;
    }
    Ok(())
}

#[tauri::command]
fn acknowledge_remote_mismatch(
    state: State<'_, AppState>,
    server_url: String,
) -> Result<(), String> {
    let root = state.inner.lock().unwrap().root.clone();
    let mut cfg = config::load_config(&root);
    cfg.mismatch_ack_server_url = server_url.trim().to_string();
    config::save_config(&root, &cfg).map_err(|e| e.to_string())?;
    state.inner.lock().unwrap().cfg = cfg;
    Ok(())
}

#[tauri::command]
async fn verify_encryption_pin(state: State<'_, AppState>, pin: String) -> Result<(), String> {
    let root = state.inner.lock().unwrap().root.clone();
    let cfg = config::load_config(&root);
    if cfg.encryption_locked_until > chrono::Utc::now().timestamp() {
        return Err(format!(
            "PIN verification locked until {}",
            cfg.encryption_locked_until
        ));
    }
    if cfg.encryption_transition || !cfg.encryption_pin.is_empty() {
        if !config::valid_encryption_pin(&pin) || pin.is_empty() {
            return Err("PIN must be exactly 6 digits".to_string());
        }
        let valid = if cfg.encryption_transition {
            config::verify_transition_candidate(&root, &pin)?
        } else {
            config::check_candidate_against_pin(&root, &pin, &cfg.encryption_pin)?
        };
        let updated = config::load_config(&root);
        let mut inner = state.inner.lock().unwrap();
        inner.encryption_failures = updated.encryption_failures;
        inner.encryption_locked_until = updated.encryption_locked_until;
        if !valid {
            return Err(
                if updated.encryption_locked_until > chrono::Utc::now().timestamp() {
                    format!(
                        "Incorrect Encryption PIN. PIN locked until {}",
                        updated.encryption_locked_until
                    )
                } else {
                    format!(
                        "Incorrect Encryption PIN ({}/3 failed attempts)",
                        updated.encryption_failures
                    )
                },
            );
        }
        if cfg.encryption_transition {
            drop(inner);
            return Ok(());
        }
        config::clear_encryption_failures(&root)?;
        let updated = config::load_config(&root);
        inner.cfg = updated;
        inner.encryption_failures = 0;
        inner.encryption_locked_until = 0;
        inner.encryption_unlocked = true;
        inner.encryption_busy = false;
        let app = inner.app.clone();
        drop(inner);
        if let Some(app) = app {
            app.set_encryption_pin(&pin);
        }
        return Ok(());
    }
    if !config::valid_encryption_pin(&pin) || pin.is_empty() {
        return Err("PIN must be exactly 6 digits".to_string());
    }
    let app = local_app(&state).ok_or("Not configured")?;
    let _sync_guard = state.sync_gate.operation.lock().await;
    let previous_pin = cfg.encryption_pin.clone();
    app.set_encryption_pin(&pin);
    let candidate_valid = match app.recover_remote_pin(&pin).await {
        Ok(valid) => valid,
        Err(error) => {
            app.set_encryption_pin(&previous_pin);
            return Err(error.to_string());
        }
    };
    if !candidate_valid {
        app.set_encryption_pin(&previous_pin);
        let failures = config::record_candidate_failure(&root)?;
        let failed = config::load_config(&root);
        let mut inner = state.inner.lock().unwrap();
        inner.encryption_failures = failed.encryption_failures;
        inner.encryption_locked_until = failed.encryption_locked_until;
        return Err(
            if failed.encryption_locked_until > chrono::Utc::now().timestamp() {
                format!(
                    "Incorrect Encryption PIN. PIN locked until {}",
                    failed.encryption_locked_until
                )
            } else {
                format!("Incorrect Encryption PIN ({failures}/3 failed attempts)")
            },
        );
    }
    config::clear_encryption_failures(&root)?;
    let mut updated = config::load_config(&root);
    updated.encryption_pin = pin.clone();
    updated.encryption_failures = 0;
    updated.encryption_locked_until = 0;
    config::save_config(&root, &updated).map_err(|error| error.to_string())?;
    app.set_encryption_pin(&pin);
    let mut inner = state.inner.lock().unwrap();
    inner.cfg = updated;
    inner.encryption_failures = 0;
    inner.encryption_locked_until = 0;
    inner.encryption_unlocked = true;
    inner.encryption_busy = false;
    Ok(())
}

#[cfg(feature = "desktop")]
#[tauri::command]
async fn import_from_dir(
    state: State<'_, AppState>,
    app_handle: AppHandle,
) -> Result<usize, String> {
    let app = match local_app(&state) {
        Some(a) => a,
        None => return Ok(0),
    };
    let folder = app_handle.dialog().file().blocking_pick_folder();
    let dir = match folder {
        Some(f) => path_string(f),
        None => return Ok(0),
    };
    let ids = app.import_dir(&dir).map_err(|e| e.to_string())?;
    let n = ids.len();
    spawn_push_imported(app_handle, ids);
    Ok(n)
}

#[cfg(feature = "desktop")]
#[tauri::command]
async fn import_from_files(
    state: State<'_, AppState>,
    app_handle: AppHandle,
) -> Result<usize, String> {
    let app = match local_app(&state) {
        Some(a) => a,
        None => return Ok(0),
    };
    let picked = app_handle
        .dialog()
        .file()
        .add_filter("Notes", &["md", "txt", "sh", "json"])
        .blocking_pick_files();
    let paths: Vec<String> = match picked {
        Some(list) => list.into_iter().map(path_string).collect(),
        None => return Ok(0),
    };
    if paths.is_empty() {
        return Ok(0);
    }
    let ids = app.import_files(paths).map_err(|e| e.to_string())?;
    let n = ids.len();
    spawn_push_imported(app_handle, ids);
    Ok(n)
}

#[cfg(feature = "desktop")]
#[tauri::command]
async fn export_view(
    state: State<'_, AppState>,
    app_handle: AppHandle,
    view: String,
    format: String,
) -> Result<usize, String> {
    let app = match current_app(&state) {
        Some(a) => a,
        None => return Ok(0),
    };
    let dest = match app_handle.dialog().file().blocking_pick_folder() {
        Some(f) => path_string(f),
        None => return Ok(0),
    };
    let v = parse_export_view(&view)?;
    let f = parse_export_format(&format)?;
    app.export_view(&dest, &v, f).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(feature = "desktop")]
    let builder = builder.plugin(tauri_plugin_dialog::init());
    let builder = builder.setup(|app| {
        let handle = app.handle().clone();
        #[cfg(feature = "desktop")]
        let root = default_root();
        #[cfg(not(feature = "desktop"))]
        let root = app
            .path()
            .app_data_dir()
            .expect("app_data_dir")
            .join("draftnote");
        let cfg = config::load_config(&root);
        let built = build_app(&handle, &cfg, &root).ok();
        app.manage(AppState {
            inner: Mutex::new(Inner {
                app: built,
                cfg: cfg.clone(),
                root: root.clone(),
                encryption_unlocked: true,
                encryption_busy: cfg.encryption_transition,
                encryption_failures: cfg.encryption_failures,
                encryption_locked_until: cfg.encryption_locked_until,
            }),
            sync_gate: SyncGate::new(),
        });
        if let Some(a) = current_app(&app.state::<AppState>()) {
            let _ = a.purge_expired_trash();
        }
        spawn_autosync(handle);
        Ok(())
    });
    #[cfg(feature = "desktop")]
    let builder = builder.invoke_handler(tauri::generate_handler![
        get_config,
        begin_settings_validation,
        settings_closed,
        configure,
        list_notes,
        all_tags,
        notes_by_tag,
        search_notes,
        load_note,
        create_note,
        update_note,
        delete_note,
        duplicate_note,
        set_pinned,
        set_note_file_type,
        set_trashed,
        purge_expired_trash,
        push_note,
        push_delete,
        revision_list,
        revision_diff,
        revision_body,
        restore_revision,
        validate_sync,
        check_remote_mismatch,
        reclone_server,
        acknowledge_remote_mismatch,
        verify_encryption_pin,
        import_from_dir,
        import_from_files,
        export_view,
    ]);
    #[cfg(not(feature = "desktop"))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        get_config,
        begin_settings_validation,
        settings_closed,
        configure,
        list_notes,
        all_tags,
        notes_by_tag,
        search_notes,
        load_note,
        create_note,
        update_note,
        delete_note,
        duplicate_note,
        set_pinned,
        set_note_file_type,
        set_trashed,
        purge_expired_trash,
        push_note,
        push_delete,
        revision_list,
        revision_diff,
        revision_body,
        restore_revision,
        validate_sync,
        check_remote_mismatch,
        reclone_server,
        acknowledge_remote_mismatch,
        verify_encryption_pin,
    ]);
    builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::{
        settings_pin_valid, settings_target_pin, settings_transition_config, sync_error_event,
        AppState, Inner, SyncGate,
    };
    use crate::app;
    use crate::config::{self, Config};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn test_state() -> AppState {
        let root = tempfile::tempdir().unwrap().keep();
        let app = app::App::new(app::Config {
            root: root.clone(),
            base_url: String::new(),
            owner: String::new(),
            repo: String::new(),
            username: String::new(),
            token: String::new(),
            use_api: true,
            encryption_pin: String::new(),
            on_note_changed: None,
        })
        .unwrap();
        AppState {
            inner: Mutex::new(Inner {
                app: Some(Arc::new(app)),
                cfg: Config {
                    encryption_transition: true,
                    ..Config::default()
                },
                root: PathBuf::from(root),
                encryption_unlocked: true,
                encryption_busy: true,
                encryption_failures: 0,
                encryption_locked_until: 0,
            }),
            sync_gate: SyncGate::new(),
        }
    }

    #[tokio::test]
    async fn settings_validation_waits_for_active_operation_and_gates_new_operations() {
        let gate = std::sync::Arc::new(SyncGate::new());
        let operation = gate.operation.lock().await;
        let validating_gate = gate.clone();
        let validation = tokio::spawn(async move {
            validating_gate.begin_settings_validation().await;
        });

        tokio::time::timeout(Duration::from_secs(1), async {
            while !gate.is_paused() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("settings validation did not pause new operations");
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!validation.is_finished());

        drop(operation);
        tokio::time::timeout(Duration::from_secs(1), validation)
            .await
            .expect("settings validation did not drain the active operation")
            .expect("settings validation task panicked");
        assert!(gate.lock_remote().await.is_err());

        gate.settings_closed();
        assert!(gate.lock_remote().await.is_ok());
    }

    #[tokio::test]
    async fn settings_open_skips_autosync_and_validate_unlocks_shared_operation() {
        let gate = SyncGate::new();
        gate.begin_settings_validation().await;
        assert!(gate.lock_remote().await.is_err());
        gate.settings_closed();
        let validation = gate.lock_remote().await.unwrap();
        assert!(gate.operation.try_lock().is_err());
        drop(validation);
        assert!(gate.lock_remote().await.is_ok());
    }

    #[tokio::test]
    async fn manual_full_sync_waits_for_existing_auto_sync_operation() {
        let gate = Arc::new(SyncGate::new());
        let automatic = gate.lock_remote().await.unwrap();
        let validation_gate = gate.clone();
        let validation = tokio::spawn(async move {
            let _guard = validation_gate.lock_remote().await.unwrap();
        });
        tokio::task::yield_now().await;
        assert!(!validation.is_finished());
        drop(automatic);
        tokio::time::timeout(Duration::from_secs(1), validation)
            .await
            .expect("full sync did not run after automatic sync")
            .expect("full sync task panicked");
    }

    #[tokio::test]
    async fn remote_entry_points_share_one_gate() {
        let gate = SyncGate::new();
        gate.begin_settings_validation().await;
        assert!(gate.lock_remote().await.is_err());
        assert!(gate.lock_remote().await.is_err());
        assert!(gate.lock_remote().await.is_err());
        gate.settings_closed();
        assert!(gate.lock_remote().await.is_ok());
    }

    #[tokio::test]
    async fn local_notes_remain_available_during_interrupted_encryption_transition() {
        let state = test_state();
        let result = state
            .inner
            .lock()
            .unwrap()
            .app
            .as_ref()
            .unwrap()
            .list_notes();
        assert!(result.is_ok());
        assert!(!state.sync_gate.is_paused());
        assert!(state.inner.lock().unwrap().encryption_busy);
        state
            .sync_gate
            .paused
            .store(true, std::sync::atomic::Ordering::Release);
        assert!(state.sync_gate.lock_remote().await.is_err());
    }

    #[tokio::test]
    async fn unlocked_settings_state_clears_saved_failure_counter() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config {
            encryption_pin: "123456".into(),
            encryption_failures: 2,
            ..Config::default()
        };
        config::save_config(dir.path(), &cfg).unwrap();
        assert!(config::check_encryption_pin(dir.path(), "123456").unwrap());
        cfg = config::load_config(dir.path());
        assert_eq!(cfg.encryption_failures, 0);
    }

    #[test]
    fn interrupted_migration_state_keeps_local_commands_usable() {
        let state = test_state();
        let app = state.inner.lock().unwrap().app.clone().unwrap();
        let note = app.create_note("local", Vec::new(), "body").unwrap();
        assert_eq!(app.load_note(&note.id).unwrap().body, "body");
        assert!(state.inner.lock().unwrap().cfg.encryption_transition);
        assert!(state.inner.lock().unwrap().encryption_busy);
    }

    #[test]
    fn local_app_remains_available_while_migration_state_is_busy() {
        let state = test_state();
        let app = {
            let inner = state.inner.lock().unwrap();
            assert!(inner.encryption_busy);
            assert!(inner.cfg.encryption_transition);
            inner.app.clone()
        };
        assert!(app.is_some());
    }

    #[test]
    fn settings_recovery_fields_keep_the_encryption_transition_atomic() {
        let previous = Config {
            encryption_pin: "111111".into(),
            encryption_transition_pin: "222222".into(),
            encryption_transition: true,
            encryption_failures: 2,
            ..Config::default()
        };
        let replacement = Config {
            encryption_pin: previous.encryption_pin.clone(),
            encryption_transition_pin: previous.encryption_transition_pin.clone(),
            encryption_transition: true,
            encryption_failures: previous.encryption_failures,
            ..Config::default()
        };
        assert_eq!(replacement.encryption_pin, "111111");
        assert_eq!(replacement.encryption_transition_pin, "222222");
        assert_eq!(replacement.encryption_failures, 2);
    }

    #[test]
    fn settings_pin_transition_retains_and_replaces_pending_target() {
        let previous = Config {
            encryption_pin: "111111".into(),
            encryption_transition_pin: "222222".into(),
            encryption_transition: true,
            ..Config::default()
        };
        assert_eq!(settings_target_pin(&previous, "••••••"), "222222");
        assert_eq!(settings_target_pin(&previous, "333333"), "333333");
        let next = settings_transition_config(&previous, previous.clone(), "222222");
        assert!(next.encryption_transition);
        assert_eq!(next.encryption_transition_pin, "222222");
    }

    #[test]
    fn settings_pin_accepts_mask_only_when_a_pin_exists() {
        assert!(settings_pin_valid("", false));
        assert!(settings_pin_valid("123456", false));
        assert!(!settings_pin_valid("••••••", false));
        assert!(settings_pin_valid("••••••", true));
    }

    #[test]
    fn repeated_background_errors_leave_pin_attempts_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let cfg = Config {
            encryption_pin: "123456".into(),
            encryption_failures: 2,
            ..Config::default()
        };
        config::save_config(root.path(), &cfg).unwrap();
        for _ in 0..3 {
            let event = sync_error_event(root.path(), "incorrect PIN".to_string());
            assert_eq!(event.message, "incorrect PIN");
            assert_eq!(event.lock_until, 0);
        }
        let persisted = config::load_config(root.path());
        assert_eq!(persisted.encryption_failures, 2);
        assert_eq!(persisted.encryption_locked_until, 0);
    }

    #[test]
    fn beginning_and_resuming_pin_transition_preserves_old_key_until_rewrite() {
        let old = Config {
            encryption_pin: "111111".into(),
            encryption_failures: 2,
            ..Config::default()
        };
        let mut pending = Config {
            encryption_pin: old.encryption_pin.clone(),
            encryption_failures: old.encryption_failures,
            ..Config::default()
        };
        pending = settings_transition_config(&old, pending, "222222");
        assert!(pending.encryption_transition);
        assert_eq!(pending.encryption_pin, "111111");
        assert_eq!(pending.encryption_transition_pin, "222222");
        assert_eq!(pending.encryption_failures, 0);

        let resumed = settings_transition_config(&pending, pending.clone(), "222222");
        assert!(resumed.encryption_transition);
        assert_eq!(resumed.encryption_pin, "111111");
        assert_eq!(resumed.encryption_transition_pin, "222222");
    }

    #[test]
    fn settings_transition_rejects_a_different_target_without_mutating_state() {
        let previous = Config {
            encryption_pin: "111111".into(),
            encryption_transition_pin: "222222".into(),
            encryption_transition: true,
            encryption_failures: 1,
            ..Config::default()
        };
        let target = settings_target_pin(&previous, "333333");
        let mut next = settings_transition_config(&previous, previous.clone(), &target);
        next.encryption_transition_pin = previous.encryption_transition_pin.clone();
        assert_eq!(next.encryption_pin, "111111");
        assert_eq!(next.encryption_transition_pin, "222222");
        assert_eq!(next.encryption_failures, 1);
    }
}
