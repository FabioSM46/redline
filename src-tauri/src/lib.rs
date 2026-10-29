mod autostart;
mod recorder;
mod sampler;
mod settings;
mod sources;
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
        // existing window to the front (D8). The lock is a name on the session bus: without a
        // bus the plugin logs a warning and the app launches normally, with no single instance.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main_window(app);
        }))
        // Backend only: no `autostart:*` permission reaches the frontend (D8).
        .plugin(autostart::plugin())
        .setup(|app| {
            settings::init(app.handle());
            sampler::start(app)?;
            recorder::init(app.handle());
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
            recorder::recorder_status,
            recorder::replay_start,
            recorder::replay_stop,
            recorder::replay_save,
            sources::list_audio_sources,
            sources::list_capture_targets,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app, event| {
            // Every way out (tray Quit, closing the window without a tray) ends here: the
            // replay process must not outlive the app.
            if let tauri::RunEvent::Exit = event {
                recorder::shutdown(app);
            }
        });
}
