//! Fan control: automatic (the firmware's), a fixed speed or a temperature curve, plus the
//! firmware (PMFW) fan options of the GPUs whose fan is driven by the power management firmware.
//!
//! `set_fan(gpu_id, settings)` validates [`FanSettings`] against what the GPU can do, then sends
//! LACT's own `set_fan_control` request (never `set_gpu_config`), through [`Tuning::apply`]: the
//! change is pending until the user confirms it.
//!
//! What lactd 0.10.1 does with the request is documented on [`lact::FanOptions`]. The rules this
//! module enforces before lactd sees anything follow from it:
//!
//! - `static` and `curve` need LACT's `ConfigurableFanControl` flag. A GPU with a firmware fan
//!   (`HasPmfw`) gets it only with overdrive enabled: without it the sentence says so.
//! - On a firmware fan the curve is the firmware's: exactly as many points as
//!   `gpu_od/fan_ctrl/fan_curve` has (read from sysfs, never written), temperatures and speeds
//!   inside its `OD_RANGE`, whole percents (lactd truncates the fraction). A fixed speed is a flat
//!   firmware curve, so it follows the same speed range.
//! - Any other fan: 1 or more points, 0–[`MAX_CURVE_TEMPERATURE_C`] °C, 0–100 %.
//! - Every curve is checked as a whole: distinct temperatures and a speed that never drops as the
//!   temperature rises.
//! - The four firmware limits (`acoustic_limit`, `acoustic_target`, `target_temperature`,
//!   `minimum_pwm`) only take effect in `auto`, zero RPM (`zero_rpm`, `zero_rpm_threshold`) in
//!   `auto` and `curve`: lactd ignores them elsewhere, so they are refused there. Each needs the
//!   firmware to expose it with a range, and a value inside that range.
//! - lactd replaces its stored firmware options as a whole: Redline sends the stored ones
//!   (`get_gpu_config`) with the given ones on top, so an option the settings leave out keeps its
//!   value.

use super::{CurvePoint, FanStatus, Lact, PendingChange, TemperatureRange, Tuning, TuningChange};
use redline_core::lact::{
    DeviceFlag, DeviceInfo, DeviceStats, FanControlMode, FanCurveMap, FanInfo, FanOptions,
    PmfwInfo, PmfwOptions,
};
use redline_core::sensors::{AmdGpu, PmfwFanCurve};
use serde::{Deserialize, Serialize};
use tauri::State;

pub const NEEDS_OVERDRIVE: &str = "This GPU's fan is driven by its firmware, which accepts a \
    fixed speed, a curve or fan options only with overdrive enabled. Enable overdrive (the \
    `amdgpu.ppfeaturemask` kernel parameter) and reboot, then try again.";
pub const NOT_CONTROLLABLE: &str = "LACT cannot control this GPU's fan.";
pub const NO_FIRMWARE_OPTIONS: &str = "This GPU has no firmware fan options.";
pub const NO_FIRMWARE_CURVE: &str = "Redline cannot read this GPU's firmware fan curve, so it \
    cannot check a fixed speed or a curve against it.";
pub const FIRMWARE_CURVE_LOCKED: &str = "The firmware of this GPU does not allow changing its \
    fan curve.";
/// The highest temperature of a curve on a fan without a firmware curve. Redline's own bound:
/// LACT sets none, and no GPU runs this hot.
pub const MAX_CURVE_TEMPERATURE_C: i32 = 120;

/// What `set_fan` takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FanSetting {
    /// The firmware's automatic control: LACT stops driving the fan.
    Auto,
    Static,
    Curve,
}

/// The settings of `set_fan`. Fields that do not belong to `mode` are refused, not ignored.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FanSettings {
    pub mode: FanSetting,
    /// Percent, 0–100. Required in `static`, refused elsewhere.
    #[serde(default)]
    pub static_speed: Option<f64>,
    /// Required in `curve`, refused elsewhere. Any order; speeds in percent.
    #[serde(default)]
    pub curve: Option<Vec<CurvePoint>>,
    #[serde(default)]
    pub pmfw: PmfwSettings,
}

/// The firmware fan options to change; a missing one keeps the value LACT stores.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PmfwSettings {
    pub zero_rpm: Option<bool>,
    /// °C.
    pub zero_rpm_threshold: Option<u32>,
    /// °C.
    pub target_temperature: Option<u32>,
    /// RPM.
    pub acoustic_limit: Option<u32>,
    /// RPM.
    pub acoustic_target: Option<u32>,
    /// Percent.
    pub minimum_pwm: Option<u32>,
}

/// The fan speeds a fixed speed or a curve may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SpeedRange {
    pub min_pct: u32,
    pub max_pct: u32,
}

/// What a fixed speed and a curve must respect on this GPU: `FanStatus.curve_rules`, and exactly
/// what `set_fan` checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CurveRules {
    /// The exact number of points of a curve; `null` = any number from 1.
    pub points: Option<usize>,
    pub temperature: TemperatureRange,
    pub speed: SpeedRange,
    /// Speeds must be whole percents (a firmware curve).
    pub whole_percent: bool,
}

/// What the GPU can do with its fan, from LACT and from sysfs.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct FanCapabilities {
    /// LACT's `ConfigurableFanControl`.
    pub configurable: bool,
    /// LACT's `HasPmfw`: the fan is driven by the power management firmware.
    pub has_pmfw: bool,
    pub pmfw: PmfwInfo,
    /// `gpu_od/fan_ctrl/fan_curve`, read only for a firmware fan.
    pub firmware_curve: Option<PmfwFanCurve>,
}

impl FanCapabilities {
    /// The rules for a fixed speed or a curve, or why neither is possible.
    pub fn curve_rules(&self) -> Result<CurveRules, String> {
        if !self.configurable {
            return Err(if self.has_pmfw {
                NEEDS_OVERDRIVE
            } else {
                NOT_CONTROLLABLE
            }
            .to_owned());
        }
        if !self.has_pmfw {
            return Ok(CurveRules {
                points: None,
                temperature: TemperatureRange {
                    min_c: 0,
                    max_c: MAX_CURVE_TEMPERATURE_C,
                },
                speed: SpeedRange {
                    min_pct: 0,
                    max_pct: 100,
                },
                whole_percent: false,
            });
        }
        let curve = self.firmware_curve.ok_or(NO_FIRMWARE_CURVE)?;
        let (Some((min_c, max_c)), Some((min_pct, max_pct))) =
            (curve.temperature_range, curve.speed_range)
        else {
            return Err(FIRMWARE_CURVE_LOCKED.to_owned());
        };
        Ok(CurveRules {
            points: Some(curve.points),
            temperature: TemperatureRange { min_c, max_c },
            speed: SpeedRange { min_pct, max_pct },
            whole_percent: true,
        })
    }
}

fn mode_name(mode: FanSetting) -> &'static str {
    match mode {
        FanSetting::Auto => "automatic",
        FanSetting::Static => "static",
        FanSetting::Curve => "curve",
    }
}

/// `value` percent as a sentence fragment: `35 %`, `35.5 %`.
fn pct(value: f64) -> String {
    format!("{value} %")
}

/// A speed inside `rules`, or the sentence that says why not. `what` names it in the sentence.
fn check_speed(speed: f64, rules: &CurveRules, what: &str) -> Result<(), String> {
    let SpeedRange { min_pct, max_pct } = rules.speed;
    if !speed.is_finite() || speed < f64::from(min_pct) || speed > f64::from(max_pct) {
        return Err(format!(
            "{what} must be between {min_pct} % and {max_pct} %{}.",
            if speed.is_finite() {
                format!(", not {}", pct(speed))
            } else {
                String::new()
            }
        ));
    }
    if rules.whole_percent && speed.fract() != 0.0 {
        return Err(format!(
            "{what} must be a whole percent on this GPU (its firmware takes whole percents), \
             not {}.",
            pct(speed)
        ));
    }
    Ok(())
}

/// Percent → LACT's 0.0–1.0. For a firmware fan (`whole_percent`) lactd 0.10.1 turns the
/// fraction back into percent with `(ratio * 100.0) as u8`, which truncates: `0.53_f32 * 100.0`
/// is `52.99…` and would become 52 %. The fraction is then the smallest `f32` that comes back as
/// the same whole percent (at most a few ulps above `speed_pct / 100`, invisible at the one
/// decimal `tuning_status` reports).
fn fraction(speed_pct: f64, whole_percent: bool) -> f32 {
    let mut ratio = (speed_pct / 100.0) as f32;
    if whole_percent {
        while f64::from((ratio * 100.0).trunc()) < speed_pct && ratio < 1.0 {
            ratio = f32::from_bits(ratio.to_bits() + 1);
        }
    }
    ratio
}

/// The curve as LACT stores it, or the first problem found.
pub(super) fn check_curve(
    points: &[CurvePoint],
    rules: &CurveRules,
) -> Result<FanCurveMap, String> {
    if points.is_empty() {
        return Err("A fan curve needs at least one point.".to_owned());
    }
    if let Some(count) = rules.points
        && points.len() != count
    {
        return Err(format!(
            "This GPU's firmware fan curve has exactly {count} points; this curve has {}.",
            points.len()
        ));
    }
    let mut sorted = points.to_vec();
    sorted.sort_by_key(|point| point.temperature_c);
    let TemperatureRange { min_c, max_c } = rules.temperature;
    for point in &sorted {
        if point.temperature_c < min_c || point.temperature_c > max_c {
            return Err(format!(
                "Curve temperatures must be between {min_c} °C and {max_c} °C, not {} °C.",
                point.temperature_c
            ));
        }
        check_speed(
            point.speed_pct,
            rules,
            &format!("The speed at {} °C", point.temperature_c),
        )?;
    }
    for pair in sorted.windows(2) {
        let (low, high) = (pair[0], pair[1]);
        if low.temperature_c == high.temperature_c {
            return Err(format!(
                "The curve has two points at {} °C.",
                low.temperature_c
            ));
        }
        if high.speed_pct < low.speed_pct {
            return Err(format!(
                "The fan must not slow down as the temperature rises: {} at {} °C is below {} \
                 at {} °C.",
                pct(high.speed_pct),
                high.temperature_c,
                pct(low.speed_pct),
                low.temperature_c
            ));
        }
    }
    Ok(sorted
        .iter()
        .map(|point| {
            (
                point.temperature_c,
                fraction(point.speed_pct, rules.whole_percent),
            )
        })
        .collect())
}

/// Why the firmware option `name` is missing: overdrive is off (a firmware fan LACT cannot
/// drive), the GPU has no firmware fan at all, or this firmware does not expose that option.
fn missing_option(name: &str, caps: &FanCapabilities) -> String {
    match (caps.has_pmfw, caps.configurable) {
        (true, false) => NEEDS_OVERDRIVE.to_owned(),
        (false, _) => NO_FIRMWARE_OPTIONS.to_owned(),
        (true, true) => format!("The firmware of this GPU does not expose the {name}."),
    }
}

/// One firmware option inside the range the firmware reports.
fn check_pmfw_value(
    name: &str,
    unit: &str,
    value: u32,
    info: Option<FanInfo>,
    caps: &FanCapabilities,
) -> Result<(), String> {
    let Some(info) = info else {
        return Err(missing_option(name, caps));
    };
    let Some((min, max)) = info.allowed_range else {
        return Err(format!(
            "The firmware of this GPU does not allow changing the {name}."
        ));
    };
    if value < min || value > max {
        return Err(format!(
            "The {name} must be between {min} {unit} and {max} {unit}, not {value} {unit}."
        ));
    }
    Ok(())
}

/// Checks `pmfw` against the mode and the firmware, and lays it over the `stored` options.
fn check_pmfw(
    mode: FanSetting,
    pmfw: &PmfwSettings,
    caps: &FanCapabilities,
    stored: PmfwOptions,
) -> Result<PmfwOptions, String> {
    let limits = [
        (
            "acoustic limit",
            "RPM",
            pmfw.acoustic_limit,
            caps.pmfw.acoustic_limit,
        ),
        (
            "acoustic target",
            "RPM",
            pmfw.acoustic_target,
            caps.pmfw.acoustic_target,
        ),
        (
            "target temperature",
            "°C",
            pmfw.target_temperature,
            caps.pmfw.target_temp,
        ),
        (
            "minimum fan speed",
            "%",
            pmfw.minimum_pwm,
            caps.pmfw.minimum_pwm,
        ),
    ];
    for (name, unit, value, info) in limits {
        let Some(value) = value else { continue };
        if mode != FanSetting::Auto {
            return Err(format!(
                "The {name} applies only to the automatic fan control: LACT ignores it in {} \
                 mode.",
                mode_name(mode)
            ));
        }
        check_pmfw_value(name, unit, value, info, caps)?;
    }

    if (pmfw.zero_rpm.is_some() || pmfw.zero_rpm_threshold.is_some()) && mode == FanSetting::Static
    {
        return Err(
            "Zero RPM does not apply to a fixed speed: LACT turns it off in static mode."
                .to_owned(),
        );
    }
    if pmfw.zero_rpm.is_some() && caps.pmfw.zero_rpm_enable.is_none() {
        return Err(missing_option("zero RPM switch", caps));
    }
    if let Some(value) = pmfw.zero_rpm_threshold {
        check_pmfw_value(
            "zero RPM temperature",
            "°C",
            value,
            caps.pmfw.zero_rpm_temperature,
            caps,
        )?;
    }

    Ok(PmfwOptions {
        acoustic_limit: pmfw.acoustic_limit.or(stored.acoustic_limit),
        acoustic_target: pmfw.acoustic_target.or(stored.acoustic_target),
        minimum_pwm: pmfw.minimum_pwm.or(stored.minimum_pwm),
        target_temperature: pmfw.target_temperature.or(stored.target_temperature),
        zero_rpm: pmfw.zero_rpm.or(stored.zero_rpm),
        zero_rpm_threshold: pmfw.zero_rpm_threshold.or(stored.zero_rpm_threshold),
    })
}

/// The `set_fan_control` request for `settings`, or the first problem found. Nothing here talks
/// to lactd.
pub(super) fn fan_options(
    lact_id: &str,
    settings: &FanSettings,
    caps: &FanCapabilities,
    stored: PmfwOptions,
) -> Result<FanOptions, String> {
    let mode = settings.mode;
    match (
        mode,
        settings.static_speed.is_some(),
        settings.curve.is_some(),
    ) {
        (FanSetting::Static, false, _) => {
            return Err("Static mode needs a fan speed.".to_owned());
        }
        (FanSetting::Curve, _, false) => {
            return Err("Curve mode needs a fan curve.".to_owned());
        }
        (FanSetting::Auto | FanSetting::Curve, true, _) => {
            return Err(format!(
                "A fixed fan speed belongs to static mode, not to {} mode.",
                mode_name(mode)
            ));
        }
        (FanSetting::Auto | FanSetting::Static, _, true) => {
            return Err(format!(
                "A fan curve belongs to curve mode, not to {} mode.",
                mode_name(mode)
            ));
        }
        _ => {}
    }

    let (enabled, lact_mode, static_speed, curve) = match mode {
        FanSetting::Auto => (false, None, None, None),
        FanSetting::Static => {
            let rules = caps.curve_rules()?;
            let speed = settings.static_speed.unwrap_or_default();
            check_speed(speed, &rules, "The fan speed")?;
            (
                true,
                Some(FanControlMode::Static),
                Some(fraction(speed, rules.whole_percent)),
                None,
            )
        }
        FanSetting::Curve => {
            let rules = caps.curve_rules()?;
            let curve = check_curve(settings.curve.as_deref().unwrap_or_default(), &rules)?;
            (true, Some(FanControlMode::Curve), None, Some(curve))
        }
    };
    Ok(FanOptions {
        id: lact_id.to_owned(),
        enabled,
        mode: lact_mode,
        static_speed,
        curve,
        pmfw: check_pmfw(mode, &settings.pmfw, caps, stored)?,
    })
}

impl Lact {
    /// What the GPU `gpu_id` (LACT device `info`/`stats`) can do with its fan.
    pub(super) fn fan_capabilities(
        &self,
        gpu_id: &str,
        info: &DeviceInfo,
        stats: &DeviceStats,
    ) -> FanCapabilities {
        let has_pmfw = info.has(&DeviceFlag::HasPmfw);
        FanCapabilities {
            configurable: info.has(&DeviceFlag::ConfigurableFanControl),
            has_pmfw,
            pmfw: stats.fan.pmfw_info,
            firmware_curve: has_pmfw
                .then(|| AmdGpu::by_id_in(&self.drm, gpu_id).ok()?.pmfw_fan_curve())
                .flatten(),
        }
    }

    /// Fills the parts of `fan` that come from the capabilities.
    pub(super) fn add_fan_capabilities(
        &self,
        fan: &mut FanStatus,
        gpu_id: &str,
        info: &DeviceInfo,
        stats: &DeviceStats,
    ) {
        fan.curve_rules = self
            .fan_capabilities(gpu_id, info, stats)
            .curve_rules()
            .ok();
    }

    /// Checks `settings` against the GPU and sends `set_fan_control`.
    fn set_fan(&self, gpu_id: &str, lact_id: &str, settings: &FanSettings) -> Result<u64, String> {
        let reason = |error| self.reason(&error);
        let info = self.client.device_info(lact_id).map_err(reason)?;
        let stats = self.client.device_stats(lact_id).map_err(reason)?;
        let stored = self
            .client
            .get_gpu_config(lact_id)
            .map_err(reason)?
            .map(|config| config.pmfw_options)
            .unwrap_or_default();
        let caps = self.fan_capabilities(gpu_id, &info, &stats);
        let options = fan_options(lact_id, settings, &caps, stored)?;
        self.client
            .set_fan_control(&options)
            .map_err(|error| self.refusal(&error))
    }
}

impl Tuning {
    /// The fan of `gpu_id`, checked against what the GPU can do. Pending until confirmed.
    /// Blocking.
    pub fn set_fan(&self, gpu_id: &str, settings: &FanSettings) -> Result<PendingChange, String> {
        self.apply(gpu_id, TuningChange::Fan, |lact, lact_id| {
            lact.set_fan(gpu_id, lact_id, settings)
        })
    }
}

/// Sets the fan of `gpu_id`: `auto` gives it back to the firmware, `static` holds a speed,
/// `curve` follows a temperature curve; `pmfw` changes firmware fan options. The change is
/// pending: the user must confirm it with `tuning_confirm` before its deadline.
#[tauri::command]
pub async fn set_fan(
    gpu_id: String,
    settings: FanSettings,
    tuning: State<'_, Tuning>,
) -> Result<PendingChange, String> {
    super::blocking(&tuning, move |tuning| tuning.set_fan(&gpu_id, &settings)).await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A firmware fan with overdrive on, as lactd 0.10.1 and the kernel report an RDNA3/4 card.
    fn firmware_fan() -> FanCapabilities {
        let range = |current, min, max| {
            Some(FanInfo {
                current,
                allowed_range: Some((min, max)),
            })
        };
        FanCapabilities {
            configurable: true,
            has_pmfw: true,
            pmfw: PmfwInfo {
                acoustic_limit: range(2450, 500, 3200),
                acoustic_target: range(2000, 500, 3200),
                target_temp: range(95, 25, 105),
                minimum_pwm: range(15, 15, 100),
                zero_rpm_enable: Some(true),
                zero_rpm_temperature: range(50, 25, 100),
            },
            firmware_curve: Some(PmfwFanCurve {
                points: 5,
                temperature_range: Some((25, 100)),
                speed_range: Some((15, 100)),
            }),
        }
    }

    /// A fan without firmware curve that LACT drives itself (pre-RDNA3).
    fn plain_fan() -> FanCapabilities {
        FanCapabilities {
            configurable: true,
            ..FanCapabilities::default()
        }
    }

    fn settings(value: serde_json::Value) -> FanSettings {
        serde_json::from_value(value).unwrap()
    }

    fn check(value: serde_json::Value, caps: &FanCapabilities) -> Result<FanOptions, String> {
        fan_options("lact-id", &settings(value), caps, PmfwOptions::default())
    }

    fn curve(points: &[(i32, f64)]) -> serde_json::Value {
        json!({
            "mode": "curve",
            "curve": points
                .iter()
                .map(|(t, s)| json!({ "temperature_c": t, "speed_pct": s }))
                .collect::<Vec<_>>()
        })
    }

    const FIVE: [(i32, f64); 5] = [(40, 20.0), (50, 30.0), (60, 50.0), (70, 75.0), (85, 100.0)];

    #[test]
    fn auto_gives_the_fan_back_to_the_firmware() {
        let options = check(json!({ "mode": "auto" }), &plain_fan()).unwrap();
        assert_eq!(
            options,
            FanOptions {
                id: "lact-id".into(),
                enabled: false,
                mode: None,
                static_speed: None,
                curve: None,
                pmfw: PmfwOptions::default(),
            }
        );
        // Even where LACT could not drive the fan: giving it back is always possible.
        assert!(check(json!({ "mode": "auto" }), &FanCapabilities::default()).is_ok());
    }

    #[test]
    fn a_static_speed_is_a_fraction_for_lact() {
        let options = check(
            json!({ "mode": "static", "static_speed": 35 }),
            &plain_fan(),
        )
        .unwrap();
        assert!(options.enabled);
        assert_eq!(options.mode, Some(FanControlMode::Static));
        assert_eq!(options.static_speed, Some(0.35));
        assert_eq!(options.curve, None);
        // A fan LACT drives itself takes any percent, fractions included.
        assert!(check(json!({ "mode": "static", "static_speed": 0 }), &plain_fan()).is_ok());
        assert!(
            check(
                json!({ "mode": "static", "static_speed": 42.5 }),
                &plain_fan()
            )
            .is_ok()
        );
        for bad in [-1.0, 100.5] {
            let error = check(
                json!({ "mode": "static", "static_speed": bad }),
                &plain_fan(),
            )
            .unwrap_err();
            assert!(error.contains("between 0 % and 100 %"), "{bad}: {error}");
        }
    }

    #[test]
    fn a_static_speed_on_a_firmware_fan_follows_its_speed_range() {
        let caps = firmware_fan();
        assert!(check(json!({ "mode": "static", "static_speed": 15 }), &caps).is_ok());
        assert_eq!(
            check(json!({ "mode": "static", "static_speed": 10 }), &caps),
            Err("The fan speed must be between 15 % and 100 %, not 10 %.".to_owned())
        );
        let error = check(json!({ "mode": "static", "static_speed": 40.5 }), &caps).unwrap_err();
        assert!(error.contains("whole percent"), "{error}");
    }

    #[test]
    fn each_mode_takes_only_its_own_fields() {
        let caps = plain_fan();
        assert_eq!(
            check(json!({ "mode": "static" }), &caps),
            Err("Static mode needs a fan speed.".to_owned())
        );
        assert_eq!(
            check(json!({ "mode": "curve" }), &caps),
            Err("Curve mode needs a fan curve.".to_owned())
        );
        let error = check(json!({ "mode": "auto", "static_speed": 50 }), &caps).unwrap_err();
        assert!(error.contains("static mode"), "{error}");
        let mut with_speed = curve(&FIVE);
        with_speed["static_speed"] = json!(50);
        assert!(
            check(with_speed, &caps)
                .unwrap_err()
                .contains("static mode")
        );
        let mut with_curve = curve(&FIVE);
        with_curve["mode"] = json!("static");
        with_curve["static_speed"] = json!(50);
        assert!(check(with_curve, &caps).unwrap_err().contains("curve mode"));
        // Unknown fields and modes are refused when the settings are read.
        for bad in [
            json!({ "mode": "auto", "speed": 50 }),
            json!({ "mode": "manual" }),
            json!({ "mode": "auto", "pmfw": { "zero_rpm_temp": 50 } }),
            json!({ "mode": "curve", "curve": [{ "temperature_c": 40, "speed_pct": 30, "x": 1 }] }),
        ] {
            assert!(
                serde_json::from_value::<FanSettings>(bad.clone()).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_curve_on_a_firmware_fan_has_its_point_count_and_ranges() {
        let caps = firmware_fan();
        let options = check(curve(&FIVE), &caps).unwrap();
        assert_eq!(options.mode, Some(FanControlMode::Curve));
        assert!(options.enabled);
        assert_eq!(
            options.curve,
            Some([(40, 0.2), (50, 0.3), (60, 0.5), (70, 0.75), (85, 1.0)].into())
        );

        assert_eq!(
            check(curve(&FIVE[..3]), &caps),
            Err("This GPU's firmware fan curve has exactly 5 points; this curve has 3.".to_owned())
        );
        let mut hot = FIVE;
        hot[4] = (105, 100.0);
        assert_eq!(
            check(curve(&hot), &caps),
            Err("Curve temperatures must be between 25 °C and 100 °C, not 105 °C.".to_owned())
        );
        let mut slow = FIVE;
        slow[0] = (40, 10.0);
        assert_eq!(
            check(curve(&slow), &caps),
            Err("The speed at 40 °C must be between 15 % and 100 %, not 10 %.".to_owned())
        );
        let mut fraction = FIVE;
        fraction[1] = (50, 30.5);
        assert!(
            check(curve(&fraction), &caps)
                .unwrap_err()
                .contains("whole percent")
        );
    }

    #[test]
    fn a_curve_must_rise_with_the_temperature() {
        let caps = plain_fan();
        // Given in any order: sorted by temperature.
        let options = check(curve(&[(80, 100.0), (40, 30.0)]), &caps).unwrap();
        assert_eq!(options.curve, Some([(40, 0.3), (80, 1.0)].into()));
        // A flat stretch is allowed.
        assert!(check(curve(&[(40, 30.0), (60, 30.0), (80, 90.0)]), &caps).is_ok());
        assert_eq!(
            check(curve(&[(40, 30.0), (60, 50.0), (80, 45.0)]), &caps),
            Err(
                "The fan must not slow down as the temperature rises: 45 % at 80 °C is below \
                 50 % at 60 °C."
                    .to_owned()
            )
        );
        assert_eq!(
            check(curve(&[(40, 30.0), (40, 50.0)]), &caps),
            Err("The curve has two points at 40 °C.".to_owned())
        );
        assert_eq!(
            check(curve(&[]), &caps),
            Err("A fan curve needs at least one point.".to_owned())
        );
        let error = check(curve(&[(130, 100.0)]), &caps).unwrap_err();
        assert!(error.contains("between 0 °C and 120 °C"), "{error}");
        let error = check(curve(&[(40, 30.0), (60, 101.0)]), &caps).unwrap_err();
        assert!(error.contains("The speed at 60 °C"), "{error}");
    }

    #[test]
    fn without_the_capability_a_speed_or_a_curve_points_to_overdrive() {
        // A firmware fan with overdrive off: no ConfigurableFanControl, no firmware values.
        let off = FanCapabilities {
            has_pmfw: true,
            ..FanCapabilities::default()
        };
        assert_eq!(
            check(json!({ "mode": "static", "static_speed": 50 }), &off),
            Err(NEEDS_OVERDRIVE.to_owned())
        );
        assert_eq!(check(curve(&FIVE), &off), Err(NEEDS_OVERDRIVE.to_owned()));
        assert_eq!(
            check(
                json!({ "mode": "auto", "pmfw": { "zero_rpm": false } }),
                &off
            ),
            Err(NEEDS_OVERDRIVE.to_owned())
        );
        assert_eq!(
            check(
                json!({ "mode": "auto", "pmfw": { "acoustic_limit": 2000 } }),
                &off
            ),
            Err(NEEDS_OVERDRIVE.to_owned())
        );
        assert!(NEEDS_OVERDRIVE.contains("amdgpu.ppfeaturemask"));

        // Any other fan LACT cannot drive.
        let none = FanCapabilities::default();
        assert_eq!(check(curve(&FIVE), &none), Err(NOT_CONTROLLABLE.to_owned()));
        assert_eq!(
            check(
                json!({ "mode": "auto", "pmfw": { "zero_rpm": true } }),
                &none
            ),
            Err(NO_FIRMWARE_OPTIONS.to_owned())
        );
    }

    #[test]
    fn a_firmware_curve_that_cannot_be_read_or_changed_is_refused() {
        let mut caps = firmware_fan();
        caps.firmware_curve = None;
        assert_eq!(
            check(curve(&FIVE), &caps),
            Err(NO_FIRMWARE_CURVE.to_owned())
        );
        caps.firmware_curve = Some(PmfwFanCurve {
            points: 5,
            temperature_range: None,
            speed_range: None,
        });
        assert_eq!(
            check(json!({ "mode": "static", "static_speed": 50 }), &caps),
            Err(FIRMWARE_CURVE_LOCKED.to_owned())
        );
    }

    #[test]
    fn firmware_options_are_checked_against_their_ranges() {
        let caps = firmware_fan();
        let options = check(
            json!({ "mode": "auto", "pmfw": {
                "zero_rpm": false, "zero_rpm_threshold": 60, "target_temperature": 90,
                "acoustic_limit": 3000, "acoustic_target": 1500, "minimum_pwm": 20
            } }),
            &caps,
        )
        .unwrap();
        assert_eq!(
            options.pmfw,
            PmfwOptions {
                acoustic_limit: Some(3000),
                acoustic_target: Some(1500),
                minimum_pwm: Some(20),
                target_temperature: Some(90),
                zero_rpm: Some(false),
                zero_rpm_threshold: Some(60),
            }
        );
        for (pmfw, expected) in [
            (
                json!({ "acoustic_limit": 4000 }),
                "The acoustic limit must be between 500 RPM and 3200 RPM, not 4000 RPM.",
            ),
            (
                json!({ "acoustic_target": 100 }),
                "The acoustic target must be between 500 RPM and 3200 RPM, not 100 RPM.",
            ),
            (
                json!({ "target_temperature": 110 }),
                "The target temperature must be between 25 °C and 105 °C, not 110 °C.",
            ),
            (
                json!({ "minimum_pwm": 5 }),
                "The minimum fan speed must be between 15 % and 100 %, not 5 %.",
            ),
            (
                json!({ "zero_rpm_threshold": 10 }),
                "The zero RPM temperature must be between 25 °C and 100 °C, not 10 °C.",
            ),
        ] {
            assert_eq!(
                check(json!({ "mode": "auto", "pmfw": pmfw }), &caps),
                Err(expected.to_owned())
            );
        }

        // A value the firmware reports without a range cannot be changed.
        let mut fixed = firmware_fan();
        fixed.pmfw.minimum_pwm = Some(FanInfo {
            current: 15,
            allowed_range: None,
        });
        assert_eq!(
            check(
                json!({ "mode": "auto", "pmfw": { "minimum_pwm": 20 } }),
                &fixed
            ),
            Err(
                "The firmware of this GPU does not allow changing the minimum fan speed."
                    .to_owned()
            )
        );
    }

    #[test]
    fn an_option_missing_with_overdrive_on_does_not_blame_overdrive() {
        let mut caps = firmware_fan();
        caps.pmfw.acoustic_target = None;
        caps.pmfw.zero_rpm_enable = None;
        assert_eq!(
            check(
                json!({ "mode": "auto", "pmfw": { "acoustic_target": 1500 } }),
                &caps
            ),
            Err("The firmware of this GPU does not expose the acoustic target.".to_owned())
        );
        assert_eq!(
            check(
                json!({ "mode": "auto", "pmfw": { "zero_rpm": true } }),
                &caps
            ),
            Err("The firmware of this GPU does not expose the zero RPM switch.".to_owned())
        );
    }

    #[test]
    fn firmware_options_only_where_lact_applies_them() {
        let caps = firmware_fan();
        let mut curve_with_limit = curve(&FIVE);
        curve_with_limit["pmfw"] = json!({ "acoustic_limit": 2000 });
        assert_eq!(
            check(curve_with_limit, &caps),
            Err(
                "The acoustic limit applies only to the automatic fan control: LACT ignores it \
                 in curve mode."
                    .to_owned()
            )
        );
        // Zero RPM works with a curve, not with a fixed speed.
        let mut curve_with_zero_rpm = curve(&FIVE);
        curve_with_zero_rpm["pmfw"] = json!({ "zero_rpm": true, "zero_rpm_threshold": 50 });
        assert!(check(curve_with_zero_rpm, &caps).is_ok());
        let error = check(
            json!({ "mode": "static", "static_speed": 50, "pmfw": { "zero_rpm": true } }),
            &caps,
        )
        .unwrap_err();
        assert!(error.contains("static mode"), "{error}");
    }

    #[test]
    fn firmware_options_left_out_keep_their_stored_value() {
        let stored = PmfwOptions {
            acoustic_limit: Some(2800),
            zero_rpm: Some(true),
            ..PmfwOptions::default()
        };
        let options = fan_options(
            "lact-id",
            &settings(json!({ "mode": "auto", "pmfw": { "zero_rpm": false } })),
            &firmware_fan(),
            stored,
        )
        .unwrap();
        assert_eq!(
            options.pmfw,
            PmfwOptions {
                acoustic_limit: Some(2800),
                zero_rpm: Some(false),
                ..PmfwOptions::default()
            }
        );
    }

    #[test]
    fn whole_percents_survive_lacts_conversion() {
        // lactd 0.10.1 turns a curve speed into the firmware's percent with
        // `(ratio * 100.0) as u8` and a fixed speed with `(100.0 * speed) as u8`: both truncate,
        // so a whole percent must come back unchanged.
        for percent in 0..=100u8 {
            let ratio = fraction(f64::from(percent), true);
            assert!((0.0..=1.0).contains(&ratio), "{percent}");
            assert!((f64::from(ratio) * 100.0 - f64::from(percent)).abs() < 1e-4);
            assert_eq!((ratio * 100.0) as u8, percent, "curve {percent}");
            assert_eq!(
                (f32::from(100u8) * ratio) as u8,
                percent,
                "static {percent}"
            );
        }
    }

    #[test]
    fn the_rules_are_reported_as_the_setter_checks_them() {
        assert_eq!(
            firmware_fan().curve_rules(),
            Ok(CurveRules {
                points: Some(5),
                temperature: TemperatureRange {
                    min_c: 25,
                    max_c: 100
                },
                speed: SpeedRange {
                    min_pct: 15,
                    max_pct: 100
                },
                whole_percent: true,
            })
        );
        let plain = plain_fan().curve_rules().unwrap();
        assert_eq!(plain.points, None);
        assert_eq!(
            plain.speed,
            SpeedRange {
                min_pct: 0,
                max_pct: 100
            }
        );
        assert!(!plain.whole_percent);
    }
}
