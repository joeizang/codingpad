#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use codingpad_kernel_core::{KernelEvent, RunResult, Supervisor};
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
async fn close_script(supervisor: State<'_, Supervisor>, script_id: String) -> Result<(), String> {
    supervisor.close(&script_id).await;
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .manage(Supervisor::default())
        .invoke_handler(tauri::generate_handler![run_script, close_script])
        .build(tauri::generate_context!())
        .expect("Could not start CodingPad")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                tauri::async_runtime::block_on(app.state::<Supervisor>().shutdown());
            }
        });
}
