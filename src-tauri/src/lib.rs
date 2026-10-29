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
        .invoke_handler(tauri::generate_handler![list_gpus, gpu_snapshot])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
