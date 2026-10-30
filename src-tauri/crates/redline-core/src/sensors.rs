//! Direct reading of amdgpu sensors from sysfs. Needs neither permissions nor LACT.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

const AMD_VENDOR: &str = "0x1002";
/// Where the kernel lists the DRM cards.
pub const DRM_ROOT: &str = "/sys/class/drm";

/// What the UI needs to list a GPU and ask for it again by id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GpuInfo {
    /// The PCI slot: stable across reboots and the same id LACT uses.
    pub id: String,
    /// `device/product_name` when the driver exposes it, otherwise `GPU <pci_slot>`.
    pub name: String,
    /// e.g. 0000:0d:00.0
    pub pci_slot: String,
}

/// Every AMD GPU present, sorted by PCI slot. No GPU is an empty list, not an error.
pub fn list_gpus() -> Result<Vec<GpuInfo>> {
    list_gpus_in(Path::new(DRM_ROOT))
}

/// Like `list_gpus`, but on a given directory: tests pass a fake sysfs tree.
pub fn list_gpus_in(drm: &Path) -> Result<Vec<GpuInfo>> {
    Ok(AmdGpu::discover_in(drm)?.iter().map(AmdGpu::info).collect())
}

#[derive(Debug, Clone)]
pub struct AmdGpu {
    /// e.g. /sys/class/drm/card1/device
    pub device: PathBuf,
    pub hwmon: PathBuf,
    /// e.g. 0000:0d:00.0, the same id LACT uses
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
    /// Returns every AMD GPU present, sorted by PCI slot (`read_dir` order is not stable).
    pub fn discover() -> Result<Vec<AmdGpu>> {
        Self::discover_in(Path::new(DRM_ROOT))
    }

    /// Like `discover`, but on a given directory: tests pass a fake sysfs tree.
    /// A missing directory means no GPU, not an error.
    pub fn discover_in(drm: &Path) -> Result<Vec<AmdGpu>> {
        let mut gpus = Vec::new();
        let entries = match fs::read_dir(drm) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(gpus),
            Err(e) => return Err(e).with_context(|| format!("reading {}", drm.display())),
        };
        for entry in entries {
            let path = entry?.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            // only "cardN", not connectors such as "card1-DP-1"
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
        gpus.sort_by(|a, b| a.pci_slot.cmp(&b.pci_slot));
        Ok(gpus)
    }

    /// The first GPU by PCI slot. The app asks by id instead; this is for the probe CLI.
    pub fn first() -> Result<AmdGpu> {
        match Self::discover()?.into_iter().next() {
            Some(gpu) => Ok(gpu),
            None => bail!("no AMD GPU found in {DRM_ROOT}"),
        }
    }

    /// The GPU whose id (PCI slot) is `id`.
    pub fn by_id(id: &str) -> Result<AmdGpu> {
        Self::by_id_in(Path::new(DRM_ROOT), id)
    }

    /// Like `by_id`, but on a given directory: tests pass a fake sysfs tree.
    pub fn by_id_in(drm: &Path, id: &str) -> Result<AmdGpu> {
        match Self::discover_in(drm)?
            .into_iter()
            .find(|gpu| gpu.pci_slot == id)
        {
            Some(gpu) => Ok(gpu),
            None => bail!("no AMD GPU with id {id}"),
        }
    }

    /// Id and display name. Redline adds no brand name of its own: the name is either
    /// the driver's `product_name` or a neutral fallback.
    pub fn info(&self) -> GpuInfo {
        let name = read_trim(&self.device.join("product_name"))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("GPU {}", self.pci_slot));
        GpuInfo {
            id: self.pci_slot.clone(),
            name,
            pci_slot: self.pci_slot.clone(),
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
            // some GPUs expose power1_average, others power1_input
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

    /// The firmware (PMFW) fan curve, `gpu_od/fan_ctrl/fan_curve`: only on GPUs whose fan is
    /// driven by the power management firmware, and there only with overdrive enabled. `None`
    /// when the file is missing or unreadable.
    pub fn pmfw_fan_curve(&self) -> Option<PmfwFanCurve> {
        parse_pmfw_fan_curve(
            &fs::read_to_string(self.device.join("gpu_od/fan_ctrl/fan_curve")).ok()?,
        )
    }

    /// Finds the `<prefix>N_input` sensor whose `<prefix>N_label` equals `label`.
    fn labeled(&self, hwmon: &Path, prefix: &str, label: &str) -> Option<u64> {
        let start = if prefix == "in" { 0 } else { 1 };
        (start..start + 8).find_map(|n| {
            let l = read_trim(&hwmon.join(format!("{prefix}{n}_label")))?;
            (l == label).then(|| read_u64(&hwmon.join(format!("{prefix}{n}_input"))))?
        })
    }
}

/// The shape of a firmware fan curve. Its number of points is fixed by the firmware: a new curve
/// must have exactly as many. The ranges are `None` when the firmware does not allow changing
/// the curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PmfwFanCurve {
    pub points: usize,
    /// °C, inclusive.
    pub temperature_range: Option<(i32, i32)>,
    /// Percent, inclusive.
    pub speed_range: Option<(u32, u32)>,
}

/// Parses `fan_curve` as the kernel writes it (the format `amdgpu-sysfs` 0.21, LACT 0.10.1's
/// reader, expects):
///
/// ```text
/// OD_FAN_CURVE:
/// 0: 0C 0%
/// ...
/// OD_RANGE:
/// FAN_CURVE(hotspot temp): 25C 100C
/// FAN_CURVE(fan speed): 20% 100%
/// ```
pub fn parse_pmfw_fan_curve(text: &str) -> Option<PmfwFanCurve> {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    if lines.next()? != "OD_FAN_CURVE:" {
        return None;
    }
    let mut points = 0;
    let mut temperature_range = None;
    let mut speed_range = None;
    let mut in_range = false;
    for line in lines {
        if line == "OD_RANGE:" {
            in_range = true;
            continue;
        }
        let (name, value) = line.split_once(": ")?;
        if !in_range {
            // `<index>: <temp>C <speed>%`
            let (temperature, speed) = value.split_once(' ')?;
            temperature.strip_suffix('C')?.parse::<i32>().ok()?;
            speed.strip_suffix('%')?.parse::<u32>().ok()?;
            points += 1;
            continue;
        }
        let (min, max) = value.split_once(' ')?;
        match name {
            "FAN_CURVE(hotspot temp)" => {
                let parse = |v: &str| v.strip_suffix('C')?.parse::<i32>().ok();
                temperature_range = Some((parse(min)?, parse(max)?));
            }
            "FAN_CURVE(fan speed)" => {
                let parse = |v: &str| v.strip_suffix('%')?.parse::<u32>().ok();
                speed_range = Some((parse(min)?, parse(max)?));
            }
            _ => {}
        }
    }
    // Like LACT: both ranges, or the curve cannot be changed.
    let (temperature_range, speed_range) = match (temperature_range, speed_range) {
        (Some(t), Some(s)) => (Some(t), Some(s)),
        _ => (None, None),
    };
    (points > 0).then_some(PmfwFanCurve {
        points,
        temperature_range,
        speed_range,
    })
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

    /// Fake sysfs tree: `<root>/drm/cardN -> <root>/devices/<slot>`, as in the kernel.
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

    /// Two AMD cards (the higher slot created first) and one non-AMD card.
    fn two_amd_one_other(root: &Path) {
        fake_card(
            root,
            "card0",
            "0000:0d:00.0",
            AMD_VENDOR,
            &[("product_name", "Discrete Test Card")],
        );
        fake_card(root, "card1", "0000:00:02.0", "0x8086", &[]);
        fake_card(
            root,
            "card2",
            "0000:06:00.0",
            AMD_VENDOR,
            &[("product_name", "")],
        );
    }

    #[test]
    fn lists_amd_gpus_sorted_by_pci_slot_with_names() {
        let root = temp_root("list");
        two_amd_one_other(&root);

        let gpus = list_gpus_in(&root.join("drm")).unwrap();
        assert_eq!(
            gpus,
            vec![
                GpuInfo {
                    id: "0000:06:00.0".into(),
                    name: "GPU 0000:06:00.0".into(),
                    pci_slot: "0000:06:00.0".into(),
                },
                GpuInfo {
                    id: "0000:0d:00.0".into(),
                    name: "Discrete Test Card".into(),
                    pci_slot: "0000:0d:00.0".into(),
                },
            ]
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn name_falls_back_when_product_name_is_missing() {
        let root = temp_root("noname");
        fake_card(&root, "card1", "0000:0d:00.0", AMD_VENDOR, &[]);

        let gpus = list_gpus_in(&root.join("drm")).unwrap();
        assert_eq!(gpus[0].name, "GPU 0000:0d:00.0");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn no_gpu_is_an_empty_list() {
        let root = temp_root("empty");
        fake_card(&root, "card0", "0000:00:02.0", "0x8086", &[]);
        assert!(list_gpus_in(&root.join("drm")).unwrap().is_empty());
        assert!(list_gpus_in(&root.join("missing")).unwrap().is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn looks_up_a_gpu_by_id() {
        let root = temp_root("byid");
        two_amd_one_other(&root);

        let gpu = AmdGpu::by_id_in(&root.join("drm"), "0000:0d:00.0").unwrap();
        assert_eq!(gpu.pci_slot, "0000:0d:00.0");
        assert_eq!(gpu.info().name, "Discrete Test Card");

        let err = AmdGpu::by_id_in(&root.join("drm"), "0000:00:02.0").unwrap_err();
        assert!(err.to_string().contains("0000:00:02.0"), "{err}");
        let err = AmdGpu::by_id_in(&root.join("drm"), "0000:ff:00.0").unwrap_err();
        assert!(err.to_string().contains("0000:ff:00.0"), "{err}");
        fs::remove_dir_all(&root).unwrap();
    }

    const FAN_CURVE: &str = "OD_FAN_CURVE:\n0: 0C 0%\n1: 0C 0%\n2: 0C 0%\n3: 0C 0%\n4: 0C 0%\nOD_RANGE:\nFAN_CURVE(hotspot temp): 25C 100C\nFAN_CURVE(fan speed): 20% 100%";

    #[test]
    fn parses_the_firmware_fan_curve() {
        assert_eq!(
            parse_pmfw_fan_curve(FAN_CURVE),
            Some(PmfwFanCurve {
                points: 5,
                temperature_range: Some((25, 100)),
                speed_range: Some((20, 100)),
            })
        );
        // Without OD_RANGE the curve cannot be changed.
        let fixed = "OD_FAN_CURVE:\n0: 30C 20%\n1: 50C 40%\n";
        assert_eq!(
            parse_pmfw_fan_curve(fixed),
            Some(PmfwFanCurve {
                points: 2,
                temperature_range: None,
                speed_range: None,
            })
        );
        for broken in [
            "",
            "OD_SCLK:\n0: 500Mhz",
            "OD_FAN_CURVE:\n0: 30 20%",
            "OD_FAN_CURVE:\n",
        ] {
            assert_eq!(parse_pmfw_fan_curve(broken), None, "{broken:?}");
        }
    }

    #[test]
    fn reads_the_firmware_fan_curve_only_where_the_gpu_has_one() {
        let root = temp_root("fancurve");
        fake_card(&root, "card1", "0000:0d:00.0", AMD_VENDOR, &[]);
        let gpu = AmdGpu::by_id_in(&root.join("drm"), "0000:0d:00.0").unwrap();
        // No `gpu_od` folder: an older GPU, or overdrive off.
        assert_eq!(gpu.pmfw_fan_curve(), None);
        let dir = root.join("devices/0000:0d:00.0/gpu_od/fan_ctrl");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("fan_curve"), FAN_CURVE).unwrap();
        assert_eq!(gpu.pmfw_fan_curve().map(|curve| curve.points), Some(5));
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
        assert_eq!(s.temp_mem_c, None, "a missing sensor stays None, not zero");
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
