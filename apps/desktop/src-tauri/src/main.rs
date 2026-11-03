#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use codingpad_kernel_core::{
    lsp::{LanguageServers, LspEvent, LspStartResult},
    store::{self, FolderEntry, LibrarySettings, ScriptFile},
    toolchains::Toolchains,
    ExpandResult, KernelEvent, Language, RunResult, Supervisor,
};
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{ipc::Channel, Manager, State};

/// Record when the interface is ready for `just bench`. A no-op in normal use.
#[tauri::command]
fn ui_ready() {
    let Some(path) = std::env::var_os("CODINGPAD_BENCH_MARKER") else {
        return;
    };
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let _ = fs::write(path, nanos.to_string());
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
async fn run_script(
    supervisor: State<'_, Supervisor>,
    script_id: String,
    source: String,
    language: Option<Language>,
    path: Option<String>,
    on_event: Channel<KernelEvent>,
) -> Result<RunResult, String> {
    supervisor
        .run_language_at(
            &script_id,
            &source,
            language.unwrap_or_default(),
            path.as_deref(),
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

#[tauri::command]
async fn stop_script(supervisor: State<'_, Supervisor>, script_id: String) -> Result<(), String> {
    supervisor.stop(&script_id).await;
    Ok(())
}

#[tauri::command]
async fn reset_script(supervisor: State<'_, Supervisor>, script_id: String) -> Result<(), String> {
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
async fn close_script(
    supervisor: State<'_, Supervisor>,
    language_servers: State<'_, LanguageServers>,
    script_id: String,
) -> Result<(), String> {
    supervisor.close(&script_id).await;
    language_servers.close(&script_id).await;
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
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            app.manage(Supervisor::with_data_dir(data_dir.clone()));
            app.manage(Toolchains::new(data_dir.clone()));
            app.manage(LanguageServers::with_data_dir(data_dir));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ui_ready,
            install_uv,
            run_script,
            stop_script,
            reset_script,
            expand_node,
            close_script,
            begin_language_server_session,
            start_language_server,
            change_language_server,
            request_language_server,
            load_library_settings,
            save_library_settings,
            list_folder,
            read_script,
            write_script
        ])
        .build(tauri::generate_context!())
        .expect("Could not start CodingPad")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                tauri::async_runtime::block_on(async {
                    app.state::<Supervisor>().shutdown().await;
                    app.state::<LanguageServers>().shutdown().await;
                });
            }
        });
}
