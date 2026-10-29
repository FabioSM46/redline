mod autostart;
mod sampler;
mod settings;
mod tray;

use redline_core::sensors::{self, GpuInfo, GpuSnapshot};
use sampler::Sampler;
use tauri::State;

/// Every AMD GPU, sorted by PCI slot. No GPU is an empty list.
#[tauri::command]
fn list_gpus() -> Result<Vec<GpuInfo>, String> {
    sensors::list_gpus().map_err(|e| e.to_string())
}

/// The latest reading of the GPU with id `gpu_id` (its PCI slot), taken by the sampler;
/// a GPU the sampler has not recorded yet is read live.
#[tauri::command]
fn gpu_snapshot(gpu_id: String, sampler: State<'_, Sampler>) -> Result<GpuSnapshot, String> {
    sampler.latest(&gpu_id, sampler::read_live)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Registered first, as the plugin requires: a second launch exits and brings the
        // existing window to the front (D8).
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main_window(app);
        }))
        // Backend only: no `autostart:*` permission reaches the frontend (D8).
        .plugin(autostart::plugin())
        .setup(|app| {
            settings::init(app.handle());
            sampler::start(app)?;
            tray::create(app.handle());
            autostart::show_main_window_unless_hidden_start(app.handle());
            Ok(())
        })
        .on_window_event(tray::on_window_event)
        .invoke_handler(tauri::generate_handler![
            list_gpus,
            gpu_snapshot,
            sampler::gpu_history,
            settings::get_settings,
            settings::set_settings,
            autostart::get_autostart,
            autostart::set_autostart,
            tray::app_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
