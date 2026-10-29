//! Lettura diretta dei sensori amdgpu da sysfs. Non servono permessi né LACT.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

const AMD_VENDOR: &str = "0x1002";

#[derive(Debug, Clone)]
pub struct AmdGpu {
    /// es. /sys/class/drm/card1/device
    pub device: PathBuf,
    pub hwmon: PathBuf,
    /// es. 0000:0d:00.0, lo stesso id che usa LACT
    pub pci_slot: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct GpuSnapshot {
    pub temp_edge_c: Option<f64>,
    pub temp_junction_c: Option<f64>,
    pub temp_mem_c: Option<f64>,
    pub power_w: Option<f64>,
    pub power_cap_w: Option<f64>,
    pub fan_rpm: Option<u64>,
    pub sclk_mhz: Option<u64>,
    pub mclk_mhz: Option<u64>,
    pub vddgfx_mv: Option<u64>,
    pub gpu_busy_pct: Option<u64>,
    pub mem_busy_pct: Option<u64>,
    pub vram_used_mib: Option<u64>,
    pub vram_total_mib: Option<u64>,
}

impl AmdGpu {
    /// Restituisce tutte le GPU AMD presenti (card0, card1, ...).
    pub fn discover() -> Result<Vec<AmdGpu>> {
        Self::discover_in(Path::new("/sys/class/drm"))
    }

    /// Come `discover`, ma su una cartella data: i test passano un albero sysfs finto.
    pub fn discover_in(drm: &Path) -> Result<Vec<AmdGpu>> {
        let mut gpus = Vec::new();
        let entries = fs::read_dir(drm).with_context(|| format!("lettura {}", drm.display()))?;
        for entry in entries {
            let path = entry?.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            // solo "cardN", non i connettori tipo "card1-DP-1"
            if !name.starts_with("card") || name.contains('-') {
                continue;
            }
            let device = path.join("device");
            if read_trim(&device.join("vendor")).as_deref() != Some(AMD_VENDOR) {
                continue;
            }
            let Some(hwmon) = first_hwmon(&device) else {
                continue;
            };
            let pci_slot = fs::canonicalize(&device)?
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            gpus.push(AmdGpu {
                device,
                hwmon,
                pci_slot,
            });
        }
        Ok(gpus)
    }

    pub fn first() -> Result<AmdGpu> {
        match Self::discover()?.into_iter().next() {
            Some(gpu) => Ok(gpu),
            None => bail!("nessuna GPU AMD trovata in /sys/class/drm"),
        }
    }

    pub fn snapshot(&self) -> GpuSnapshot {
        let h = &self.hwmon;
        let d = &self.device;
        GpuSnapshot {
            temp_edge_c: self.labeled(h, "temp", "edge").map(|v| v as f64 / 1000.0),
            temp_junction_c: self
                .labeled(h, "temp", "junction")
                .map(|v| v as f64 / 1000.0),
            temp_mem_c: self.labeled(h, "temp", "mem").map(|v| v as f64 / 1000.0),
            // alcune GPU espongono power1_average, altre power1_input
            power_w: read_u64(&h.join("power1_average"))
                .or_else(|| read_u64(&h.join("power1_input")))
                .map(|uw| uw as f64 / 1e6),
            power_cap_w: read_u64(&h.join("power1_cap")).map(|uw| uw as f64 / 1e6),
            fan_rpm: read_u64(&h.join("fan1_input")),
            sclk_mhz: self.labeled(h, "freq", "sclk").map(|hz| hz / 1_000_000),
            mclk_mhz: self.labeled(h, "freq", "mclk").map(|hz| hz / 1_000_000),
            vddgfx_mv: self
                .labeled(h, "in", "vddgfx")
                .or_else(|| read_u64(&h.join("in0_input"))),
            gpu_busy_pct: read_u64(&d.join("gpu_busy_percent")),
            mem_busy_pct: read_u64(&d.join("mem_busy_percent")),
            vram_used_mib: read_u64(&d.join("mem_info_vram_used")).map(|b| b >> 20),
            vram_total_mib: read_u64(&d.join("mem_info_vram_total")).map(|b| b >> 20),
        }
    }

    /// Cerca il sensore `<prefix>N_input` la cui etichetta `<prefix>N_label` vale `label`.
    fn labeled(&self, hwmon: &Path, prefix: &str, label: &str) -> Option<u64> {
        let start = if prefix == "in" { 0 } else { 1 };
        (start..start + 8).find_map(|n| {
            let l = read_trim(&hwmon.join(format!("{prefix}{n}_label")))?;
            (l == label).then(|| read_u64(&hwmon.join(format!("{prefix}{n}_input"))))?
        })
    }
}

fn first_hwmon(device: &Path) -> Option<PathBuf> {
    fs::read_dir(device.join("hwmon"))
        .ok()?
        .flatten()
        .map(|e| e.path())
        .next()
}

fn read_trim(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_owned())
}

fn read_u64(path: &Path) -> Option<u64> {
    read_trim(path)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    /// Albero sysfs finto: `<root>/drm/cardN -> <root>/devices/<slot>` come nel kernel.
    fn fake_card(root: &Path, card: &str, slot: &str, vendor: &str, files: &[(&str, &str)]) {
        let device = root.join("devices").join(slot);
        let hwmon = device.join("hwmon/hwmon3");
        fs::create_dir_all(&hwmon).unwrap();
        fs::write(device.join("vendor"), format!("{vendor}\n")).unwrap();
        for (name, value) in files {
            let path = if name.starts_with("hwmon/") {
                hwmon.join(name.trim_start_matches("hwmon/"))
            } else {
                device.join(name)
            };
            fs::write(path, format!("{value}\n")).unwrap();
        }
        let card_dir = root.join("drm").join(card);
        fs::create_dir_all(&card_dir).unwrap();
        symlink(&device, card_dir.join("device")).unwrap();
    }

    fn temp_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("redline-sensors-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("drm")).unwrap();
        root
    }

    #[test]
    fn finds_only_amd_cards_and_skips_connectors() {
        let root = temp_root("discover");
        fake_card(&root, "card0", "0000:00:02.0", "0x8086", &[]);
        fake_card(&root, "card1", "0000:0d:00.0", AMD_VENDOR, &[]);
        fs::create_dir_all(root.join("drm/card1-DP-1")).unwrap();

        let gpus = AmdGpu::discover_in(&root.join("drm")).unwrap();
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].pci_slot, "0000:0d:00.0");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn reads_labeled_sensors_and_converts_units() {
        let root = temp_root("snapshot");
        fake_card(
            &root,
            "card1",
            "0000:0d:00.0",
            AMD_VENDOR,
            &[
                ("hwmon/temp1_label", "edge"),
                ("hwmon/temp1_input", "41000"),
                ("hwmon/temp2_label", "junction"),
                ("hwmon/temp2_input", "43500"),
                ("hwmon/power1_average", "32000000"),
                ("hwmon/power1_cap", "317000000"),
                ("hwmon/freq1_label", "sclk"),
                ("hwmon/freq1_input", "2460000000"),
                ("hwmon/freq2_label", "mclk"),
                ("hwmon/freq2_input", "772000000"),
                ("hwmon/in0_label", "vddgfx"),
                ("hwmon/in0_input", "700"),
                ("gpu_busy_percent", "7"),
                ("mem_info_vram_used", "1134559232"),
                ("mem_info_vram_total", "17095983104"),
            ],
        );

        let gpu = AmdGpu::discover_in(&root.join("drm")).unwrap().remove(0);
        let s = gpu.snapshot();
        assert_eq!(s.temp_edge_c, Some(41.0));
        assert_eq!(s.temp_junction_c, Some(43.5));
        assert_eq!(
            s.temp_mem_c, None,
            "un sensore assente resta None, non zero"
        );
        assert_eq!(s.power_w, Some(32.0));
        assert_eq!(s.power_cap_w, Some(317.0));
        assert_eq!(s.sclk_mhz, Some(2460));
        assert_eq!(s.mclk_mhz, Some(772));
        assert_eq!(s.vddgfx_mv, Some(700));
        assert_eq!(s.gpu_busy_pct, Some(7));
        assert_eq!(s.fan_rpm, None);
        assert_eq!(s.vram_used_mib, Some(1082));
        assert_eq!(s.vram_total_mib, Some(16304));
        fs::remove_dir_all(&root).unwrap();
    }
}
