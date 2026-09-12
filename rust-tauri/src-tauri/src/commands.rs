use crate::app::{self, App, Revision};
#[cfg(feature = "desktop")]
use crate::app::{ExportFormat, ExportView, ExportViewKind};
use crate::config::{self, Config, PublicConfig};
use crate::note::{Note, NoteMeta};
use crate::sync::SyncResult;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
#[cfg(feature = "desktop")]
use tauri_plugin_dialog::{DialogExt, FilePath};

struct Inner {
    app: Option<Arc<App>>,
    cfg: Config,
    root: PathBuf,
}

struct AppState {
    inner: Mutex<Inner>,
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
    let app = App::new(app::Config {
        root: root.to_path_buf(),
        base_url: cfg.base_url.clone(),
        owner: cfg.owner.clone(),
        repo: cfg.repo.clone(),
        username: cfg.username.clone(),
        token: cfg.token.clone(),
        use_api: cfg.use_api,
        on_note_changed: Some(Box::new(move |id| {
            let _ = h.emit("note-body-changed", id.to_string());
        })),
    })
    .map_err(|e| e.to_string())?;
    Ok(Arc::new(app))
}

fn current_app(state: &State<'_, AppState>) -> Option<Arc<App>> {
    state.inner.lock().unwrap().app.clone()
}

#[cfg(feature = "desktop")]
fn path_string(f: FilePath) -> String {
    match f.into_path() {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(_) => String::new(),
    }
}

#[cfg(feature = "desktop")]
fn spawn_push_imported(handle: AppHandle, app: Arc<App>, ids: Vec<String>) {
    tauri::async_runtime::spawn(async move {
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
            let (app, interval, configured) = {
                let st = handle.state::<AppState>();
                let g = st.inner.lock().unwrap();
                (
                    g.app.clone(),
                    g.cfg.interval_seconds(),
                    !g.cfg.base_url.is_empty(),
                )
            };
            tokio::time::sleep(Duration::from_secs(interval.max(1) as u64)).await;
            if !configured {
                continue;
            }
            if let Some(a) = app {
                if let Ok(pulled) = a.pull().await {
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
        "all" => Ok(ExportView { kind: ExportViewKind::All, tag: String::new() }),
        "trash" => Ok(ExportView { kind: ExportViewKind::Trash, tag: String::new() }),
        "untagged" => Ok(ExportView { kind: ExportViewKind::Untagged, tag: String::new() }),
        v if v.starts_with("tag:") => {
            let name = &v["tag:".len()..];
            if name.is_empty() {
                return Err("empty tag name".to_string());
            }
            Ok(ExportView { kind: ExportViewKind::Tag, tag: name.to_string() })
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
) -> Result<(), String> {
    let root = state.inner.lock().unwrap().root.clone();
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
        config::load_config(&root).token
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
    };
    config::save_config(&root, &cfg).map_err(|e| e.to_string())?;
    let app = build_app(&app_handle, &cfg, &root)?;
    let mut g = state.inner.lock().unwrap();
    g.app = Some(app);
    g.cfg = cfg;
    Ok(())
}

#[tauri::command]
fn list_notes(state: State<'_, AppState>) -> Result<Vec<NoteMeta>, String> {
    let app = current_app(&state).ok_or("Not configured")?;
    app.list_notes().map_err(|e| e.to_string())
}

#[tauri::command]
fn all_tags(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let app = current_app(&state).ok_or("Not configured")?;
    app.all_tags().map_err(|e| e.to_string())
}

#[tauri::command]
fn notes_by_tag(state: State<'_, AppState>, tag: String) -> Result<Vec<NoteMeta>, String> {
    let app = current_app(&state).ok_or("Not configured")?;
    app.notes_by_tag(&tag).map_err(|e| e.to_string())
}

#[tauri::command]
fn search_notes(state: State<'_, AppState>, q: String) -> Result<Vec<NoteMeta>, String> {
    let app = current_app(&state).ok_or("Not configured")?;
    app.search_notes(&q).map_err(|e| e.to_string())
}

#[tauri::command]
fn load_note(state: State<'_, AppState>, id: String) -> Result<Note, String> {
    let app = current_app(&state).ok_or("Not configured")?;
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
    app.create_note(&title, tags, &body).map_err(|e| e.to_string())
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
    app.update_note(&id, &title, tags, &body).map_err(|e| e.to_string())
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
    let app = current_app(&state).ok_or("Not configured")?;
    app.push_note(&id).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn push_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let app = current_app(&state).ok_or("Not configured")?;
    app.push_delete(&id).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn revision_list(
    state: State<'_, AppState>,
    id: String,
    page: i64,
    per_page: i64,
) -> Result<Vec<Revision>, String> {
    let app = current_app(&state).ok_or("Not configured")?;
    app.revision_list(&id, page, per_page).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn revision_diff(state: State<'_, AppState>, id: String, sha: String) -> Result<String, String> {
    let app = current_app(&state).ok_or("Not configured")?;
    app.revision_diff(&id, &sha).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn revision_body(state: State<'_, AppState>, id: String, sha: String) -> Result<String, String> {
    let app = current_app(&state).ok_or("Not configured")?;
    app.revision_body(&id, &sha).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn restore_revision(state: State<'_, AppState>, id: String, sha: String) -> Result<(), String> {
    let app = current_app(&state).ok_or("Not configured")?;
    app.restore_revision(&id, &sha).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn validate_sync(state: State<'_, AppState>) -> Result<String, String> {
    let (root, app) = {
        let g = state.inner.lock().unwrap();
        (g.root.clone(), g.app.clone())
    };
    let cfg = config::load_config(&root);
    config::preflight(&cfg)?;
    let app = app.ok_or("Not configured")?;
    match app.validate().await {
        Ok(count) => Ok(format!("OK: aligned {count} notes")),
        Err(e) => Err(config::map_validate_error(&cfg.base_url, &e.to_string())),
    }
}

#[tauri::command]
async fn check_remote_mismatch(state: State<'_, AppState>) -> Result<MismatchInfo, String> {
    let (root, app) = {
        let g = state.inner.lock().unwrap();
        (g.root.clone(), g.app.clone())
    };
    let cfg = config::load_config(&root);
    let app = match app {
        Some(a) if !cfg.use_api && !cfg.repo_url.is_empty() => a,
        _ => return Ok(MismatchInfo { mismatch: false, server_url: String::new() }),
    };
    let server_url = app.get_server_remote().await.map_err(|e| e.to_string())?;
    if server_url.is_empty() {
        return Ok(MismatchInfo { mismatch: false, server_url: String::new() });
    }
    if config::normalize_repo_url(&server_url) == config::normalize_repo_url(&cfg.repo_url) {
        return Ok(MismatchInfo { mismatch: false, server_url });
    }
    if server_url == cfg.mismatch_ack_server_url {
        return Ok(MismatchInfo { mismatch: false, server_url });
    }
    Ok(MismatchInfo { mismatch: true, server_url })
}

#[tauri::command]
async fn reclone_server(state: State<'_, AppState>) -> Result<(), String> {
    let (root, app) = {
        let g = state.inner.lock().unwrap();
        (g.root.clone(), g.app.clone())
    };
    let app = app.ok_or("Not configured")?;
    let cfg = config::load_config(&root);
    if cfg.repo_url.is_empty() {
        return Err("Missing repo URL".to_string());
    }
    app.reclone_server(&cfg.repo_url).await.map_err(|e| e.to_string())?;
    if !cfg.mismatch_ack_server_url.is_empty() {
        let mut c = cfg;
        c.mismatch_ack_server_url = String::new();
        config::save_config(&root, &c).map_err(|e| e.to_string())?;
        state.inner.lock().unwrap().cfg = c;
    }
    Ok(())
}

#[tauri::command]
fn acknowledge_remote_mismatch(state: State<'_, AppState>, server_url: String) -> Result<(), String> {
    let root = state.inner.lock().unwrap().root.clone();
    let mut cfg = config::load_config(&root);
    cfg.mismatch_ack_server_url = server_url.trim().to_string();
    config::save_config(&root, &cfg).map_err(|e| e.to_string())?;
    state.inner.lock().unwrap().cfg = cfg;
    Ok(())
}

#[cfg(feature = "desktop")]
#[tauri::command]
async fn import_from_dir(state: State<'_, AppState>, app_handle: AppHandle) -> Result<usize, String> {
    let app = match current_app(&state) {
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
    spawn_push_imported(app_handle, app, ids);
    Ok(n)
}

#[cfg(feature = "desktop")]
#[tauri::command]
async fn import_from_files(state: State<'_, AppState>, app_handle: AppHandle) -> Result<usize, String> {
    let app = match current_app(&state) {
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
    spawn_push_imported(app_handle, app, ids);
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
                root,
            }),
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
        import_from_dir,
        import_from_files,
        export_view,
    ]);
    #[cfg(not(feature = "desktop"))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        get_config,
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
    ]);
    builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
