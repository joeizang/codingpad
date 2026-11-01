#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use codingpad_kernel_core::{
    lsp::{LanguageServers, LspEvent, LspStartResult},
    ExpandResult, KernelEvent, RunResult, Supervisor,
};
use tauri::{ipc::Channel, Manager, State};

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
    session: String,
    on_event: Channel<LspEvent>,
) -> Result<LspStartResult, String> {
    language_servers
        .start_python(&script_id, &source, &session, move |event| {
            let _ = on_event.send(event);
        })
        .await
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
async fn close_script(
    supervisor: State<'_, Supervisor>,
    language_servers: State<'_, LanguageServers>,
    script_id: String,
) -> Result<(), String> {
    supervisor.close(&script_id).await;
    language_servers.close(&script_id).await;
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .manage(Supervisor::default())
        .manage(LanguageServers::default())
        .invoke_handler(tauri::generate_handler![
            run_script,
            stop_script,
            reset_script,
            expand_node,
            close_script,
            begin_language_server_session,
            start_language_server,
            change_language_server,
            request_language_server
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
