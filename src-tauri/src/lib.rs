mod autostart;
mod settings;

use redline_core::sensors::{AmdGpu, GpuSnapshot};

/// One sensor reading of the first AMD GPU.
#[tauri::command]
fn gpu_snapshot() -> Result<GpuSnapshot, String> {
    AmdGpu::first()
        .map(|gpu| gpu.snapshot())
        .map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Backend only: no `autostart:*` permission reaches the frontend (D8).
        .plugin(autostart::plugin())
        .setup(|app| {
            settings::init(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            gpu_snapshot,
            settings::get_settings,
            settings::set_settings,
            autostart::get_autostart,
            autostart::set_autostart,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
