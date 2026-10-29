mod autostart;
mod sampler;
mod settings;

use redline_core::sensors::{self, GpuInfo, GpuSnapshot};
use sampler::Sampler;
use tauri::State;

/// Every AMD GPU, sorted by PCI slot. No GPU is an empty list.
#[tauri::command]
fn list_gpus() -> Result<Vec<GpuInfo>, String> {
    sensors::list_gpus().map_err(|e| e.to_string())
}

/// The latest reading of the GPU with id `gpu_id` (its PCI slot), taken by the sampler.
#[tauri::command]
fn gpu_snapshot(gpu_id: String, sampler: State<'_, Sampler>) -> Result<GpuSnapshot, String> {
    sampler.latest(&gpu_id)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Backend only: no `autostart:*` permission reaches the frontend (D8).
        .plugin(autostart::plugin())
        .setup(|app| {
            settings::init(app.handle());
            sampler::start(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_gpus,
            gpu_snapshot,
            sampler::gpu_history,
            settings::get_settings,
            settings::set_settings,
            autostart::get_autostart,
            autostart::set_autostart,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
