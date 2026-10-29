mod autostart;
mod settings;

use redline_core::sensors::{self, AmdGpu, GpuInfo, GpuSnapshot};

/// Every AMD GPU, sorted by PCI slot. No GPU is an empty list.
#[tauri::command]
fn list_gpus() -> Result<Vec<GpuInfo>, String> {
    sensors::list_gpus().map_err(|e| e.to_string())
}

/// One sensor reading of the GPU with id `gpu_id` (its PCI slot).
#[tauri::command]
fn gpu_snapshot(gpu_id: String) -> Result<GpuSnapshot, String> {
    AmdGpu::by_id(&gpu_id)
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
            list_gpus,
            gpu_snapshot,
            settings::get_settings,
            settings::set_settings,
            autostart::get_autostart,
            autostart::set_autostart,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
