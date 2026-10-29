use redline_core::sensors::{AmdGpu, GpuSnapshot};

/// Una lettura dei sensori della prima GPU AMD.
#[tauri::command]
fn gpu_snapshot() -> Result<GpuSnapshot, String> {
    AmdGpu::first()
        .map(|gpu| gpu.snapshot())
        .map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![gpu_snapshot])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
