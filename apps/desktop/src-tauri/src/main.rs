#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use codingpad_kernel_core::{
    store::{self, FolderEntry, LibrarySettings, ScriptFile},
    toolchains::Toolchains,
    ExpandResult, KernelEvent, RunResult, Supervisor,
};
use std::{fs, path::PathBuf};
use tauri::{ipc::Channel, Manager, State};

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
    on_event: Channel<KernelEvent>,
) -> Result<RunResult, String> {
    supervisor
        .run(&script_id, &source, |event| {
            let _ = on_event.send(event);
        })
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
async fn close_script(supervisor: State<'_, Supervisor>, script_id: String) -> Result<(), String> {
    supervisor.close(&script_id).await;
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
            app.manage(Toolchains::new(data_dir));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            install_uv,
            run_script,
            stop_script,
            reset_script,
            expand_node,
            close_script,
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
                tauri::async_runtime::block_on(app.state::<Supervisor>().shutdown());
            }
        });
}
