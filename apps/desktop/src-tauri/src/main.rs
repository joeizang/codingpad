#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use codingpad_kernel_core::{
    attached_data_context,
    crash::{self, CrashDetails, CrashEnvironment},
    databases::{Connection, ConnectionStore, SqlManager, SqlRunResult},
    export::{export_table, ExportFormat, ExportSummary},
    lsp::{LanguageServers, LspEvent, LspStartResult},
    samples::{self, Sample},
    secrets::{SecretEntry, SecretOrigin, SecretStore},
    store::{self, FolderEntry, LibrarySettings, ScriptFile},
    toolchains::Toolchains,
    updates::{self, UpdateNotice},
    ExpandResult, KernelEvent, Language, RunResult, Supervisor,
};
use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{ipc::Channel, Manager, State};

/// Record when the interface is ready for `just bench`. A no-op in normal use.
#[tauri::command]
fn ui_ready(frontend: serde_json::Value) {
    bench_phase("editor_interactive", frontend);
    let Some(path) = std::env::var_os("CODINGPAD_BENCH_MARKER") else {
        return;
    };
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let _ = fs::write(path, nanos.to_string());
}

/// Sidecar JSON lines preserve startup phases without changing the ready marker.
fn bench_phase(phase: &str, detail: serde_json::Value) {
    let Some(path) = std::env::var_os("CODINGPAD_BENCH_MARKER") else {
        return;
    };
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(format!("{}.trace", PathBuf::from(path).display()))
    {
        let _ = writeln!(
            file,
            "{}",
            serde_json::json!({"phase": phase, "unix_ns": nanos as u64, "detail": detail})
        );
    }
}

#[tauri::command]
fn list_connections(connections: State<'_, Arc<ConnectionStore>>) -> Vec<Connection> {
    connections.list()
}

#[tauri::command]
fn list_samples() -> Vec<Sample> {
    samples::all()
}

#[tauri::command]
async fn save_connection(
    connections: State<'_, Arc<ConnectionStore>>,
    secrets: State<'_, Arc<SecretStore>>,
    connection: Connection,
) -> Result<Connection, String> {
    let connections = connections.inner().clone();
    let secrets = secrets.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let previous = connections
            .list()
            .into_iter()
            .find(|item| item.id == connection.id);
        let previous_secret = previous.and_then(|item| item.secret);
        let saved = connections.save(connection)?;
        if let Some(previous) = previous_secret {
            if saved.secret.as_deref() != Some(previous.as_str()) {
                secrets.delete(&previous, false).map_err(|error| {
                    format!(
                        "The connection was saved, but its old password was not removed: {error}"
                    )
                })?;
            }
        }
        Ok(saved)
    })
    .await
    .map_err(|error| format!("The connection save failed: {error}"))?
}

#[tauri::command]
async fn delete_connection(
    connections: State<'_, Arc<ConnectionStore>>,
    secrets: State<'_, Arc<SecretStore>>,
    id: String,
) -> Result<(), String> {
    let connections = connections.inner().clone();
    let secrets = secrets.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let removed = connections.delete(&id)?;
        if let Some(secret) = removed.secret {
            secrets.delete(&secret, false).map_err(|error| {
                format!("The connection was removed, but its password was not: {error}")
            })?;
        }
        Ok(())
    })
    .await
    .map_err(|error| format!("The connection delete failed: {error}"))?
}

#[tauri::command]
async fn connection_schema(
    sql: State<'_, Arc<SqlManager>>,
    name: String,
) -> Result<codingpad_kernel_core::databases::schema::Schema, String> {
    sql.schema(&name).await
}

#[tauri::command]
async fn test_connection(sql: State<'_, Arc<SqlManager>>, id: String) -> Result<String, String> {
    sql.test(&id).await
}

#[tauri::command]
fn begin_sql_run(sql: State<'_, Arc<SqlManager>>, script_id: String) {
    sql.begin_run(&script_id);
}

#[tauri::command]
async fn run_sql(
    sql: State<'_, Arc<SqlManager>>,
    script_id: String,
    source: String,
    path: Option<String>,
) -> Result<SqlRunResult, String> {
    sql.run(&script_id, &source, path.as_deref()).await
}

#[tauri::command]
fn expand_sql(
    sql: State<'_, Arc<SqlManager>>,
    script_id: String,
    handle: String,
    cursor: Option<String>,
    limit: Option<u32>,
) -> Result<ExpandResult, String> {
    sql.expand(&script_id, &handle, cursor.as_deref(), limit)
        .map(|node| ExpandResult { node })
}

#[tauri::command]
async fn save_secret(
    secrets: State<'_, Arc<SecretStore>>,
    name: String,
    value: String,
    session_only: Option<bool>,
) -> Result<SecretOrigin, String> {
    let store = secrets.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        if session_only.unwrap_or(false) {
            store
                .set_session(&name, &value)
                .map(|()| SecretOrigin::Session)
        } else {
            store.set(&name, &value)
        }
    })
    .await
    .map_err(|error| format!("The secret operation failed: {error}"))?
}

#[tauri::command]
async fn delete_secret(
    secrets: State<'_, Arc<SecretStore>>,
    name: String,
    session_only: Option<bool>,
) -> Result<(), String> {
    let store = secrets.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.delete(&name, session_only.unwrap_or(false)))
        .await
        .map_err(|error| format!("The secret operation failed: {error}"))?
}

#[tauri::command]
fn list_secrets(secrets: State<'_, Arc<SecretStore>>) -> Vec<SecretEntry> {
    secrets.entries()
}

#[tauri::command]
fn vault_available(secrets: State<'_, Arc<SecretStore>>) -> Option<bool> {
    secrets.clear_failures();
    secrets.vault_available()
}

/// The only request CodingPad makes on its own (D29, D31). `None` covers a
/// current build, an offline machine, and every other failed check.
#[tauri::command]
async fn check_for_update(app: tauri::AppHandle) -> Option<UpdateNotice> {
    updates::check(&app.package_info().version.to_string()).await
}

fn current_crash_environment(app: &tauri::AppHandle) -> CrashEnvironment {
    crash::environment(&app.package_info().version.to_string())
}

/// The version and platform shown in the crash screen and the issue.
#[tauri::command]
fn crash_environment(app: tauri::AppHandle) -> CrashEnvironment {
    current_crash_environment(&app)
}

/// The pre-filled issue. Building it sends nothing (D31); the crash screen
/// opens it in the browser only after the user clicks Report on GitHub.
#[tauri::command]
fn crash_report_url(app: tauri::AppHandle, details: CrashDetails) -> String {
    crash::report(&details, &current_crash_environment(&app)).url
}

/// A crash left by an earlier panic, offered once on the next launch.
#[tauri::command]
fn take_pending_crash(app: tauri::AppHandle) -> Option<CrashDetails> {
    app.path()
        .app_data_dir()
        .ok()
        .and_then(|directory| crash::take_pending(&directory))
}

#[tauri::command]
async fn install_uv(
    tools: State<'_, Toolchains>,
    on_progress: Channel<String>,
) -> Result<(), String> {
    tools
        .install_uv(|message| {
            let _ = on_progress.send(message.to_owned());
        })
        .await?;
    Ok(())
}

#[tauri::command]
async fn install_dotnet(
    tools: State<'_, Toolchains>,
    on_progress: Channel<String>,
) -> Result<(), String> {
    tools
        .install_dotnet(|message| {
            let _ = on_progress.send(message.to_owned());
        })
        .await?;
    Ok(())
}

#[tauri::command]
async fn install_bun(
    tools: State<'_, Toolchains>,
    on_progress: Channel<String>,
) -> Result<(), String> {
    tools
        .install_bun(|message| {
            let _ = on_progress.send(message.to_owned());
        })
        .await?;
    Ok(())
}

#[tauri::command]
async fn run_script(
    supervisor: State<'_, Supervisor>,
    sql: State<'_, Arc<SqlManager>>,
    script_id: String,
    source: String,
    language: Option<Language>,
    path: Option<String>,
    on_event: Channel<KernelEvent>,
) -> Result<RunResult, String> {
    let language = language.unwrap_or_default();
    let (data, data_error) = attached_data_context(sql.inner().as_ref(), &source, language).await;
    if let Some(error) = data_error {
        let _ = on_event.send(KernelEvent {
            method: "stream".into(),
            params: serde_json::json!({
                "runId": 0,
                "stream": "stderr",
                "text": format!("{error}\n"),
            }),
        });
    }
    supervisor
        .run_language_at_with_data(
            &script_id,
            &source,
            language,
            path.as_deref(),
            data,
            |event| {
                let _ = on_event.send(event);
            },
        )
        .await
}

#[tauri::command]
async fn expand_node(
    supervisor: State<'_, Supervisor>,
    script_id: String,
    handle: String,
    cursor: Option<String>,
    limit: Option<u32>,
) -> Result<ExpandResult, String> {
    supervisor
        .expand(&script_id, &handle, cursor.as_deref(), limit)
        .await
}

/// Cancellation flags for running exports, by export id.
#[derive(Default)]
struct Exports(Mutex<HashMap<String, Arc<AtomicBool>>>);

/// Export every row of a table dump to CSV or Excel. `sql` selects a SQL
/// script's stored result instead of a kernel's.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn export_table_dump(
    supervisor: State<'_, Supervisor>,
    sql_manager: State<'_, Arc<SqlManager>>,
    exports: State<'_, Exports>,
    export_id: String,
    script_id: String,
    handle: String,
    sql: bool,
    format: ExportFormat,
    path: String,
    // `{rows, total}`; total is null while a deferred sequence is unfinished.
    on_progress: Channel<serde_json::Value>,
) -> Result<ExportSummary, String> {
    // A cancel can arrive before this command starts; it leaves its flag set.
    let cancel = exports
        .0
        .lock()
        .unwrap()
        .entry(export_id.clone())
        .or_insert_with(|| Arc::new(AtomicBool::new(false)))
        .clone();
    let supervisor = supervisor.inner();
    let sql_manager = sql_manager.inner().clone();
    let result = export_table(
        std::path::Path::new(&path),
        format,
        |cursor, limit| {
            let sql_manager = sql_manager.clone();
            let (script_id, handle) = (script_id.clone(), handle.clone());
            async move {
                if sql {
                    sql_manager.export_page(&script_id, &handle, cursor.as_deref(), limit)
                } else {
                    supervisor
                        .export_page(&script_id, &handle, cursor.as_deref(), limit)
                        .await
                }
            }
        },
        &cancel,
        |rows, total| {
            let _ = on_progress.send(serde_json::json!({ "rows": rows, "total": total }));
        },
    )
    .await;
    exports.0.lock().unwrap().remove(&export_id);
    result
}

#[tauri::command]
fn cancel_export(exports: State<'_, Exports>, export_id: String) {
    exports
        .0
        .lock()
        .unwrap()
        .entry(export_id)
        .or_insert_with(|| Arc::new(AtomicBool::new(false)))
        .store(true, std::sync::atomic::Ordering::SeqCst);
}

#[tauri::command]
async fn stop_script(
    supervisor: State<'_, Supervisor>,
    sql: State<'_, Arc<SqlManager>>,
    script_id: String,
) -> Result<(), String> {
    supervisor.stop(&script_id).await;
    sql.stop(&script_id);
    Ok(())
}

#[tauri::command]
async fn reset_script(
    supervisor: State<'_, Supervisor>,
    sql: State<'_, Arc<SqlManager>>,
    script_id: String,
) -> Result<(), String> {
    sql.reset(&script_id);
    supervisor.reset(&script_id).await
}

#[tauri::command]
async fn begin_language_server_session(
    language_servers: State<'_, LanguageServers>,
    session: String,
) -> Result<(), String> {
    language_servers.begin_session(&session).await;
    Ok(())
}

#[tauri::command]
async fn prepare_typescript_project(
    language_servers: State<'_, LanguageServers>,
    script_id: String,
    source: String,
    session: String,
) -> Result<Option<LspStartResult>, String> {
    language_servers
        .prepare_typescript(&script_id, &source, &session)
        .await
}

#[tauri::command]
async fn start_language_server(
    language_servers: State<'_, LanguageServers>,
    script_id: String,
    source: String,
    language: String,
    path: Option<String>,
    session: String,
    on_event: Channel<LspEvent>,
) -> Result<LspStartResult, String> {
    match language.as_str() {
        "python" => {
            language_servers
                .start_python(&script_id, &source, &session, move |event| {
                    let _ = on_event.send(event);
                })
                .await
        }
        "csharp" => {
            language_servers
                .start_csharp(
                    &script_id,
                    &source,
                    path.as_deref().map(std::path::Path::new),
                    &session,
                    move |event| {
                        let _ = on_event.send(event);
                    },
                )
                .await
        }
        "typescript" => {
            language_servers
                .start_typescript(&script_id, &source, &session, move |event| {
                    let _ = on_event.send(event);
                })
                .await
        }
        "rust" => {
            language_servers
                .start_rust(
                    &script_id,
                    &source,
                    path.as_deref().map(std::path::Path::new),
                    &session,
                    move |event| {
                        let _ = on_event.send(event);
                    },
                )
                .await
        }
        _ => Err("Code intelligence is not available for this language".into()),
    }
}

#[tauri::command]
async fn change_language_server(
    language_servers: State<'_, LanguageServers>,
    script_id: String,
    source: String,
) -> Result<(), String> {
    language_servers.did_change(&script_id, &source).await
}

#[tauri::command]
async fn request_language_server(
    language_servers: State<'_, LanguageServers>,
    script_id: String,
    method: String,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    language_servers.request(&script_id, &method, params).await
}

#[tauri::command]
async fn rust_build_cache_size(supervisor: State<'_, Supervisor>) -> Result<u64, String> {
    supervisor.rust_build_cache_size().await
}

#[tauri::command]
async fn clear_rust_build_cache(supervisor: State<'_, Supervisor>) -> Result<(), String> {
    supervisor.clear_rust_build_cache().await
}

#[tauri::command]
async fn close_script(
    supervisor: State<'_, Supervisor>,
    language_servers: State<'_, LanguageServers>,
    sql: State<'_, Arc<SqlManager>>,
    script_id: String,
) -> Result<(), String> {
    language_servers.close(&script_id).await;
    supervisor.close(&script_id).await;
    sql.reset(&script_id);
    Ok(())
}

fn settings_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join("settings.json"))
        .map_err(|error| format!("Could not find the settings folder: {error}"))
}

fn default_library(app: &tauri::AppHandle) -> PathBuf {
    let documents = app.path().document_dir().ok();
    store::default_library_path(documents.as_deref())
}

#[tauri::command]
fn load_library_settings(app: tauri::AppHandle) -> Result<LibrarySettings, String> {
    let settings = store::load_settings(&settings_path(&app)?, &default_library(&app));
    // The sidebar should always be usable, even on a first launch.
    let _ = fs::create_dir_all(&settings.library_path);
    Ok(settings)
}

#[tauri::command]
fn save_library_settings(app: tauri::AppHandle, settings: LibrarySettings) -> Result<(), String> {
    fs::create_dir_all(&settings.library_path).map_err(|error| {
        format!(
            "Could not create the library folder {}: {error}",
            settings.library_path
        )
    })?;
    store::save_settings(&settings_path(&app)?, &settings)
}

#[tauri::command]
fn list_folder(path: String) -> Result<Vec<FolderEntry>, String> {
    store::list_folder(PathBuf::from(path).as_path())
}

#[tauri::command]
fn read_script(path: String) -> Result<ScriptFile, String> {
    store::read_script(PathBuf::from(path).as_path())
}

#[tauri::command]
fn write_script(path: String, source: String) -> Result<ScriptFile, String> {
    store::write_script(PathBuf::from(path).as_path(), &source)
}

fn main() {
    bench_phase("process_entry", serde_json::Value::Null);
    let context = tauri::generate_context!();
    // Install the hook before the builder so a panic in a plugin or in the
    // builder is recorded too. On desktop this is the folder Tauri later hands
    // out as app_data_dir.
    if let Some(data_dir) = crash::default_data_dir(&context.config().identifier) {
        crash::install_panic_hook(data_dir);
    }
    // Acceptance aid for D31: crash on purpose, then relaunch to see the Report
    // on GitHub button. Debug builds only; a release ignores it.
    #[cfg(debug_assertions)]
    {
        if std::env::var_os("CODINGPAD_FORCE_CRASH").is_some() {
            panic!("CodingPad was asked to crash (CODINGPAD_FORCE_CRASH).");
        }
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .on_page_load(|_, payload| {
            bench_phase(
                match payload.event() {
                    tauri::webview::PageLoadEvent::Started => "webview_load_started",
                    tauri::webview::PageLoadEvent::Finished => "webview_load_finished",
                },
                serde_json::Value::Null,
            );
        })
        .setup(|app| {
            bench_phase("setup_started", serde_json::Value::Null);
            let data_dir = app.path().app_data_dir()?;
            let secrets = Arc::new(SecretStore::new(&data_dir));
            app.manage(secrets.clone());
            let connections = Arc::new(ConnectionStore::new(&data_dir));
            if let Err(error) = samples::ensure_database(&data_dir) {
                eprintln!("CodingPad could not prepare the sample database: {error}");
            }
            app.manage(connections.clone());
            let sql = Arc::new(SqlManager::new(connections.clone(), secrets.clone()));
            app.manage(sql.clone());
            let supervisor = Supervisor::with_data_dir_and_secrets(data_dir.clone(), secrets)
                .with_session_rust_cache()
                .with_connections(connections);
            let shadow_projects = supervisor.shadow_projects();
            app.manage(supervisor);
            app.manage(Toolchains::new(data_dir.clone()));
            app.manage(Exports::default());
            app.manage(
                LanguageServers::with_data_dir_and_databases(data_dir, sql)
                    .with_shadow_projects(shadow_projects),
            );
            bench_phase("setup_finished", serde_json::Value::Null);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ui_ready,
            list_connections,
            list_samples,
            save_connection,
            delete_connection,
            test_connection,
            connection_schema,
            begin_sql_run,
            run_sql,
            expand_sql,
            save_secret,
            delete_secret,
            list_secrets,
            vault_available,
            check_for_update,
            crash_environment,
            crash_report_url,
            take_pending_crash,
            install_uv,
            install_dotnet,
            install_bun,
            run_script,
            stop_script,
            reset_script,
            rust_build_cache_size,
            clear_rust_build_cache,
            expand_node,
            export_table_dump,
            cancel_export,
            close_script,
            begin_language_server_session,
            prepare_typescript_project,
            start_language_server,
            change_language_server,
            request_language_server,
            load_library_settings,
            save_library_settings,
            list_folder,
            read_script,
            write_script
        ])
        .build(context)
        .expect("Could not start CodingPad")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                // A clean exit means nothing is left for the next launch to
                // report, even if a worker panic was recorded earlier.
                if let Ok(data_dir) = app.path().app_data_dir() {
                    crash::clear_pending(&data_dir);
                }
                app.state::<Arc<SqlManager>>().shutdown();
                tauri::async_runtime::block_on(async {
                    app.state::<Supervisor>().shutdown().await;
                    app.state::<LanguageServers>().shutdown().await;
                });
            }
        });
}
