pub mod cleanup;
pub mod duplicates;
pub mod index;
mod metrics;
pub mod model;
mod native;
pub mod providers;
pub mod rules;
pub mod scanner;

use model::*;
use providers::{ProviderReport, ProviderSettings, ProviderStore};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tauri::{Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

pub struct AppState {
    db_path: PathBuf,
    busy: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    plans: Mutex<cleanup::PlanStore>,
    provider: Arc<ProviderStore>,
    chosen_root: Mutex<Option<(String, PathBuf)>>,
    metrics: Arc<metrics::Metrics>,
}

struct OperationGuard(Arc<AtomicBool>);
impl Drop for OperationGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl AppState {
    fn begin(&self) -> Result<OperationGuard, String> {
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                "Another operation is running. Cancel it or wait for it to finish.".to_string()
            })?;
        self.cancel.store(false, Ordering::Release);
        Ok(OperationGuard(self.busy.clone()))
    }
}

async fn query<T: Send + 'static>(
    db: PathBuf,
    action: impl FnOnce(index::Index) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || action(index::Index::open(&db)?))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn choose_root(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let path = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("Choose a folder to analyze")
            .blocking_pick_folder()
            .map(|p| p.into_path().map_err(|e| e.to_string()))
            .transpose()
    })
    .await
    .map_err(|e| e.to_string())??;
    let Some(path) = path else { return Ok(None) };
    let token = uuid::Uuid::new_v4().to_string();
    *state
        .chosen_root
        .lock()
        .map_err(|_| "Folder picker unavailable")? = Some((token.clone(), path));
    Ok(Some(token))
}

#[tauri::command]
async fn start_scan(
    root: String,
    scan_id: Option<i64>,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<i64, String> {
    let guard = state.begin()?;
    let chosen = state
        .chosen_root
        .lock()
        .map_err(|_| "Folder picker unavailable")?
        .clone();
    let db = state.db_path.clone();
    let scan_id = query(db.clone(), move |index| {
        let path = if let Some(id) = scan_id {
            index.root_path(id)?
        } else if let Some((_, path)) = chosen.filter(|(token, _)| token == &root) {
            path
        } else {
            return Err("Choose a folder before starting its first scan.".into());
        };
        index.begin_scan(&path)
    })
    .await?;
    let cancel = state.cancel.clone();
    let metrics = state.metrics.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        match scanner::scan(&db, scan_id, cancel, |summary| {
            metrics.event(&summary);
            let _ = app.emit("scan-progress", summary);
        }) {
            Ok(summary) => {
                let _ = app.emit("scan-finished", summary);
            }
            Err(message) => {
                let _ = app.emit("operation-error", message);
            }
        }
        let _ = metrics.save(&db);
    });
    Ok(scan_id)
}

#[tauri::command]
fn cancel_operation(state: State<'_, AppState>) {
    state.cancel.store(true, Ordering::Release);
}

#[tauri::command]
async fn list_scans(state: State<'_, AppState>) -> Result<Vec<ScanSummary>, String> {
    query(state.db_path.clone(), |index| index.list_scans()).await
}
#[tauri::command]
async fn get_scan(scan_id: i64, state: State<'_, AppState>) -> Result<ScanSummary, String> {
    query(state.db_path.clone(), move |index| index.get_scan(scan_id)).await
}
#[tauri::command]
async fn remove_scan(scan_id: i64, state: State<'_, AppState>) -> Result<(), String> {
    let _guard = state.begin()?;
    state
        .plans
        .lock()
        .map_err(|_| "Plan storage unavailable")?
        .clear();
    query(state.db_path.clone(), move |index| {
        index.remove_scan(scan_id)
    })
    .await
}
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn get_entries(
    scan_id: i64,
    parent_id: Option<i64>,
    offset: u64,
    limit: u64,
    sort: String,
    search: String,
    candidates_only: bool,
    state: State<'_, AppState>,
) -> Result<EntryPage, String> {
    let metrics = state.metrics.clone();
    query(state.db_path.clone(), move |index| {
        let page = index.get_entries(
            scan_id,
            parent_id,
            offset,
            limit,
            &sort,
            &search,
            candidates_only,
        )?;
        metrics.page(&page);
        let _ = index.conn.busy_timeout(std::time::Duration::ZERO);
        let _ = metrics.save_connection(&index.conn);
        Ok(page)
    })
    .await
}
#[tauri::command]
async fn get_entry(
    scan_id: i64,
    entry_id: i64,
    state: State<'_, AppState>,
) -> Result<EntryRow, String> {
    query(state.db_path.clone(), move |index| {
        index.get_entry(scan_id, entry_id)
    })
    .await
}
#[tauri::command]
async fn get_breadcrumbs(
    scan_id: i64,
    entry_id: i64,
    state: State<'_, AppState>,
) -> Result<Vec<EntryRow>, String> {
    query(state.db_path.clone(), move |index| {
        index.breadcrumbs(scan_id, entry_id)
    })
    .await
}

#[tauri::command]
async fn find_duplicates(
    scan_id: i64,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<DuplicateGroup>, String> {
    let guard = state.begin()?;
    let db = state.db_path.clone();
    let cancel = state.cancel.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        duplicates::find(&db, scan_id, cancel, |processed| {
            let _ = app.emit("operation-progress", serde_json::json!({"kind":"duplicates","processed":processed,"message":"Verifying matching content"}));
        })
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn create_plan(
    scan_id: i64,
    entry_ids: Vec<i64>,
    mode: String,
    state: State<'_, AppState>,
) -> Result<CleanupPlan, String> {
    let _guard = state.begin()?;
    let plan = query(state.db_path.clone(), move |index| {
        cleanup::prepare(&index, scan_id, &entry_ids, &mode)
    })
    .await?;
    let visible = plan.visible.clone();
    state
        .plans
        .lock()
        .map_err(|_| "Plan storage unavailable")?
        .insert(plan);
    Ok(visible)
}

#[tauri::command]
async fn execute_plan(
    token: String,
    confirmation: String,
    state: State<'_, AppState>,
) -> Result<CleanupResult, String> {
    let guard = state.begin()?;
    let plan = state
        .plans
        .lock()
        .map_err(|_| "Plan storage unavailable")?
        .take(&token)?;
    let db = state.db_path.clone();
    let cancel = state.cancel.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        cleanup::execute(&index::Index::open(&db)?, plan, &confirmation, cancel)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn get_provider_settings(state: State<'_, AppState>) -> Result<ProviderSettings, String> {
    let provider = state.provider.clone();
    tauri::async_runtime::spawn_blocking(move || provider.get_settings())
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn save_provider_settings(
    settings: ProviderSettings,
    key: Option<String>,
    session_only: bool,
    state: State<'_, AppState>,
) -> Result<ProviderSettings, String> {
    let _guard = state.begin()?;
    let provider = state.provider.clone();
    tauri::async_runtime::spawn_blocking(move || {
        provider.save_settings(settings, key, session_only)
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn remove_provider_key(
    mode: String,
    state: State<'_, AppState>,
) -> Result<ProviderSettings, String> {
    let _guard = state.begin()?;
    let provider = state.provider.clone();
    tauri::async_runtime::spawn_blocking(move || provider.remove_key(&mode))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn test_provider(state: State<'_, AppState>) -> Result<ProviderReport, String> {
    let _guard = state.begin()?;
    Ok(state.provider.test(state.cancel.clone()).await)
}
#[tauri::command]
async fn analyze_candidates(
    scan_id: i64,
    entry_ids: Vec<i64>,
    state: State<'_, AppState>,
) -> Result<ProviderReport, String> {
    let _guard = state.begin()?;
    if entry_ids.is_empty() || entry_ids.len() > 20 {
        return Err("Select between 1 and 20 cleanup candidates.".into());
    }
    let entries = query(state.db_path.clone(), move |index| {
        let mut entries = Vec::new();
        for id in entry_ids {
            let record = index.get_record(scan_id, id)?;
            let metadata = std::fs::symlink_metadata(&record.path)
                .map_err(|_| "Candidate changed. Scan again.")?;
            if record.row.disposition == "keep"
                || !record.row.complete
                || scanner::path_fingerprint(&record.path, &metadata)? != record.fingerprint
            {
                return Err(
                    "Only unchanged, completely inspected candidates can be sent for analysis."
                        .into(),
                );
            }
            entries.push(record.row);
        }
        Ok(entries)
    })
    .await?;
    Ok(state.provider.analyze(entries, state.cancel.clone()).await)
}

#[tauri::command]
async fn record_frontend_ready(state: State<'_, AppState>) -> Result<(), String> {
    state.metrics.ready();
    let metrics = state.metrics.clone();
    let db = state.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || metrics.save(&db))
        .await
        .map_err(|e| e.to_string())?
}

pub fn run() {
    let metrics = Arc::new(metrics::Metrics::new());
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let directory = app.path().app_local_data_dir()?;
            std::fs::create_dir_all(&directory)?;
            let db_path = directory.join("index.sqlite");
            let index = index::Index::open(&db_path).map_err(std::io::Error::other)?;
            // A stopped process must not leave a scan claiming it is still running.
            index.conn.execute(
                "UPDATE scans SET status='interrupted' WHERE status IN ('queued','scanning')",
                [],
            )?;
            app.manage(AppState {
                provider: Arc::new(
                    ProviderStore::new(db_path.clone()).map_err(std::io::Error::other)?,
                ),
                db_path,
                busy: Arc::new(AtomicBool::new(false)),
                cancel: Arc::new(AtomicBool::new(false)),
                plans: Mutex::new(cleanup::PlanStore::default()),
                chosen_root: Mutex::new(None),
                metrics: metrics.clone(),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            choose_root,
            start_scan,
            cancel_operation,
            list_scans,
            get_scan,
            remove_scan,
            get_entries,
            get_entry,
            get_breadcrumbs,
            find_duplicates,
            create_plan,
            execute_plan,
            get_provider_settings,
            save_provider_settings,
            remove_provider_key,
            test_provider,
            analyze_candidates,
            record_frontend_ready
        ])
        .run(tauri::generate_context!())
        .expect("TrapClean could not start");
}
