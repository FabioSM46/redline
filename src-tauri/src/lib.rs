mod actions;
mod autostart;
mod dependencies;
mod desktop;
mod gallery;
mod global_shortcuts;
mod installer;
mod recorder;
mod sampler;
mod settings;
mod sources;
#[cfg(test)]
mod test_support;
mod thumbnails;
mod tray;
mod tuning;

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
        // existing window to the front (D8), or, with an action such as `--save-replay`, has the
        // running instance perform it without showing the window (#32). The lock is a name on
        // the session bus: without a bus the plugin logs a warning and the app launches
        // normally, with no single instance, and performs its own action.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            actions::on_second_launch(app, &argv);
        }))
        // Backend only: no `autostart:*` permission reaches the frontend (D8).
        .plugin(autostart::plugin())
        .setup(|app| {
            settings::init(app.handle());
            sampler::start(app)?;
            tuning::init(app);
            installer::init(app);
            // A first launch with an action performs it once GPU Screen Recorder has been
            // detected and `replay_on_start` has run.
            let launch_action = actions::requested(std::env::args_os().skip(1));
            recorder::init(app.handle(), move |service| {
                if let Some(action) = launch_action {
                    actions::perform(service, action);
                }
            });
            tray::create(app.handle());
            // In the background: the portal may be missing (GNOME before 48), and its first
            // binding may wait on a desktop dialog.
            global_shortcuts::start(app.handle());
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
            settings::recording_defaults,
            autostart::get_autostart,
            autostart::set_autostart,
            tray::app_status,
            recorder::recorder_status,
            recorder::replay_start,
            recorder::replay_stop,
            recorder::replay_save,
            recorder::recording_toggle,
            sources::list_audio_sources,
            sources::list_capture_targets,
            gallery::list_recordings,
            gallery::open_recording,
            gallery::show_in_folder,
            thumbnails::recording_thumbnail,
            thumbnails::thumbnail_support,
            global_shortcuts::global_shortcuts_status,
            global_shortcuts::global_shortcuts_configure,
            tuning::tuning_status,
            tuning::set_power_limit,
            tuning::tuning_confirm,
            tuning::tuning_revert,
            tuning::profiles::list_profiles,
            tuning::profiles::create_profile,
            tuning::profiles::activate_profile,
            tuning::profiles::delete_profile,
            tuning::fan::set_fan,
            tuning::overdrive::prepare_enable_overdrive,
            tuning::overdrive::enable_overdrive,
            tuning::overdrive::prepare_disable_overdrive,
            tuning::overdrive::disable_overdrive,
            dependencies::dependency_status,
            installer::install_dependency,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app, event| {
            // Every way out (tray Quit, closing the window without a tray) ends here: the
            // replay process must not outlive the app, nor an unconfirmed tuning change.
            if let tauri::RunEvent::Exit = event {
                recorder::shutdown(app);
                // A tuning change still waiting for confirmation is reverted.
                tuning::shutdown(app);
            }
        });
}
