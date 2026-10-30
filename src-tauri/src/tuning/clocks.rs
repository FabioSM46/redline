//! Clock and voltage controls: what the firmware of a GPU lets LACT change, and the two setters
//! that change it (D13).
//!
//! `tuning_status` asks lactd for `device_clocks_info` and turns the overdrive table into
//! [`ClocksStatus`]. The controls come from the table, never from a list of GPU names, because
//! they differ by family (verified against `lact-schema` and `gpu_controller/amd.rs` at LACT
//! `v0.10.1` and `gpu_handle/overdrive/rdna.rs` of `amdgpu-sysfs` 0.21.0):
//!
//! - RDNA1–3 have a maximum core clock, RDNA4 a core clock offset instead.
//! - Discrete GPUs have a maximum VRAM clock, integrated ones do not.
//! - RDNA2 has a voltage offset but gives no range for it, and `set_voltage_offset` then checks
//!   nothing: the range stays absent, and whoever sets it must keep the value at or below 0.
//!   RDNA3 and later give a range; what is offered from it never goes above 0.
//!
//! `clocks` is `null` when the feature does not apply or the answer says nothing:
//!
//! | lactd's answer | Overdrive | `clocks` |
//! |---|---|---|
//! | no answer, or one this client cannot read | any | `null`: the rest of the status stands |
//! | a GCN table | any | [`NOT_SUPPORTED`], no control |
//! | a table of another kind | any | `null` |
//! | a refusal, or no table | off | [`NEEDS_OVERDRIVE`], no control |
//! | an RDNA table | off | [`NEEDS_OVERDRIVE`], no control |
//! | a refusal, or no table | on or unknown | `null` |
//! | an RDNA table | on or unknown | the controls it exposes; `null` if none |
//!
//! Any refusal counts, not just lactd 0.10.1's "Clocks table not available": the sentence does
//! not depend on its wording, and with overdrive off it is true whatever lactd said.
//!
//! # Setting them
//!
//! [`Tuning::set_clocks`] sends every value in one `batch_set_clocks_value`, so they apply and
//! revert together, and [`Tuning::reset_clocks`] sends LACT's `reset` alone. Both are pending
//! changes on the path of [`Tuning::apply`]. What the backend checks, because lactd does not
//! always (`apply_clocks_config_to_table` in `gpu_controller/amd.rs` at `v0.10.1`):
//!
//! - A core clock offset goes into the table with no range check.
//! - A voltage offset without a range goes in with no check at all, which is why a value above 0
//!   is refused here on every GPU.
//! - A value lactd cannot apply fails the whole batch, with nothing to say which one.
//!
//! lactd handles the performance level itself, `REQUIRE_MANUAL_DEVICE_IDS` included, in
//! `apply_config`: Redline never sets it. A confirmed value comes back every time lactd starts
//! (`apply_current_config`): the reset is the way out, so it does not depend on the state of the
//! table the way `set_clocks` does. It needs no table and works with overdrive off.

use super::{Lact, PendingChange, Tuning, TuningChange};
use redline_core::lact::{
    ClockCommand, ClockRange, ClockSetting, ClocksInfo, ClocksTable, LactError, LactResult,
    RdnaClocksTable,
};
use serde::Serialize;
use serde_json::{Map, Value};
use tauri::State;

pub const NEEDS_OVERDRIVE: &str = "Clock and voltage controls need overdrive. Enable overdrive \
    (the `amdgpu.ppfeaturemask` kernel parameter) and reboot.";
pub const NOT_SUPPORTED: &str = "Redline does not support clock and voltage controls on this GPU \
    family.";
pub const NO_CONTROLS: &str = "LACT did not report the clock and voltage controls of this GPU, \
    so Redline cannot check the values.";
pub const NOTHING_TO_APPLY: &str = "No clock or voltage value was given.";
pub const UNDERVOLT_ONLY: &str = "Redline only undervolts: the voltage offset must be 0 mV or \
    lower.";

/// One control: its value now and the range the firmware allows. `min` and `max` are both set or
/// both `null`, and `null` means the firmware gives no range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ClockControl {
    pub current: i32,
    pub min: Option<i32>,
    pub max: Option<i32>,
}

/// `TuningStatus.clocks`. A control the GPU does not expose is `null`, never zero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClocksStatus {
    /// Why no control is offered, as a sentence; `null` when the controls below are read.
    pub unavailable_reason: Option<String>,
    /// MHz.
    pub max_core_clock: Option<ClockControl>,
    /// MHz, relative to the firmware's clock.
    pub core_clock_offset: Option<ClockControl>,
    /// MHz.
    pub max_vram_clock: Option<ClockControl>,
    /// mV. Its `max` is never above 0.
    pub voltage_offset: Option<ClockControl>,
}

impl ClocksStatus {
    fn unavailable(reason: &str) -> Self {
        ClocksStatus {
            unavailable_reason: Some(reason.to_owned()),
            max_core_clock: None,
            core_clock_offset: None,
            max_vram_clock: None,
            voltage_offset: None,
        }
    }
}

/// `TuningStatus.clocks`, from lactd's answer and the overdrive bit. See the table above.
pub(super) fn clocks_status(
    answer: LactResult<ClocksInfo>,
    overdrive_enabled: Option<bool>,
) -> Option<ClocksStatus> {
    let table = match answer {
        Ok(info) => info.table,
        Err(LactError::Daemon(_)) => None,
        Err(_) => return None,
    };
    match (table, overdrive_enabled) {
        (Some(ClocksTable::Gcn), _) => Some(ClocksStatus::unavailable(NOT_SUPPORTED)),
        (Some(ClocksTable::Other), _) => None,
        (_, Some(false)) => Some(ClocksStatus::unavailable(NEEDS_OVERDRIVE)),
        (Some(ClocksTable::Rdna(table)), _) => controls(&table),
        (None, _) => None,
    }
}

fn control(current: Option<i32>, range: Option<(i32, i32)>) -> Option<ClockControl> {
    current.map(|current| ClockControl {
        current,
        min: range.map(|(min, _)| min),
        max: range.map(|(_, max)| max),
    })
}

/// The range offered for the voltage offset: the firmware's, never above 0. A range entirely
/// above 0 leaves nothing to offer.
fn voltage_offset_range(range: Option<ClockRange>) -> Option<(i32, i32)> {
    let (min, max) = range?.full()?;
    (min <= 0).then_some((min, max.min(0)))
}

/// The controls `table` exposes, or `None` when it exposes none.
fn controls(table: &RdnaClocksTable) -> Option<ClocksStatus> {
    let od = &table.od_range;
    // The range lactd checks a maximum core clock against (`get_max_sclk_range`).
    let core_range = od.curve_sclk_points.last().copied().or(od.sclk);
    let status = ClocksStatus {
        unavailable_reason: None,
        max_core_clock: control(
            table.current_sclk_range.max,
            core_range.and_then(ClockRange::full),
        ),
        core_clock_offset: control(table.sclk_offset, od.sclk_offset.and_then(ClockRange::full)),
        max_vram_clock: control(
            table.current_mclk_range.max,
            od.mclk.and_then(ClockRange::full),
        ),
        voltage_offset: control(
            table.voltage_offset,
            voltage_offset_range(od.voltage_offset),
        ),
    };
    let none = status.max_core_clock.is_none()
        && status.core_clock_offset.is_none()
        && status.max_vram_clock.is_none()
        && status.voltage_offset.is_none();
    (!none).then_some(status)
}

/// A control `set_clocks` accepts: its key in the request, its name in a sentence, its unit, what
/// lactd calls it, and where `ClocksStatus` reads it.
struct Field {
    key: &'static str,
    name: &'static str,
    unit: &'static str,
    setting: ClockSetting,
    control: fn(&ClocksStatus) -> Option<ClockControl>,
}

/// In the order of the request lactd receives.
const FIELDS: [Field; 4] = [
    Field {
        key: "max_core_clock",
        name: "maximum core clock",
        unit: "MHz",
        setting: ClockSetting::MaxCoreClock,
        control: |status| status.max_core_clock,
    },
    Field {
        key: "core_clock_offset",
        name: "core clock offset",
        unit: "MHz",
        // lactd reads only power state 0 of an RDNA table.
        setting: ClockSetting::GpuClockOffset(0),
        control: |status| status.core_clock_offset,
    },
    Field {
        key: "max_vram_clock",
        name: "maximum VRAM clock",
        unit: "MHz",
        setting: ClockSetting::MaxMemoryClock,
        control: |status| status.max_vram_clock,
    },
    Field {
        key: "voltage_offset",
        name: "voltage offset",
        unit: "mV",
        setting: ClockSetting::VoltageOffset,
        control: |status| status.voltage_offset,
    },
];

/// One value of a `set_clocks` request; `None` clears the control, which leaves it at the
/// firmware's value.
type Requested = (&'static Field, Option<i32>);

/// The values of a `set_clocks` request, in [`FIELDS`] order. Read by hand and not with
/// `deny_unknown_fields`, whose refusal is serde's text instead of a sentence for the user.
fn requested(values: &Map<String, Value>) -> Result<Vec<Requested>, String> {
    if let Some(key) = values
        .keys()
        .find(|key| !FIELDS.iter().any(|field| field.key == *key))
    {
        return Err(format!(
            "`{key}` is not a clock or voltage control Redline can set."
        ));
    }
    let requested = FIELDS
        .iter()
        .filter_map(|field| values.get(field.key).map(|value| (field, value)))
        .map(|(field, value)| Ok((field, number(field, value)?)))
        .collect::<Result<Vec<_>, String>>()?;
    if requested.is_empty() {
        return Err(NOTHING_TO_APPLY.to_owned());
    }
    Ok(requested)
}

fn number(field: &Field, value: &Value) -> Result<Option<i32>, String> {
    if value.is_null() {
        return Ok(None);
    }
    match value.as_i64().and_then(|value| i32::try_from(value).ok()) {
        Some(value) => Ok(Some(value)),
        None => Err(format!(
            "The {} must be a whole number of {}.",
            field.name, field.unit
        )),
    }
}

/// `value` is allowed for `field` on a GPU whose control is `control`, or the sentence that says
/// why not. Clearing needs no range: nothing is written.
fn check(field: &Field, value: Option<i32>, control: Option<ClockControl>) -> Result<(), String> {
    let Some(control) = control else {
        return Err(format!("This GPU does not expose a {}.", field.name));
    };
    let Some(value) = value else { return Ok(()) };
    let voltage = field.setting == ClockSetting::VoltageOffset;
    if voltage && value > 0 {
        return Err(UNDERVOLT_ONLY.to_owned());
    }
    match (control.min, control.max) {
        (Some(min), Some(max)) if value < min || value > max => Err(format!(
            "The {name} must be between {min} {unit} and {max} {unit}.",
            name = field.name,
            unit = field.unit,
        )),
        (Some(_), Some(_)) => Ok(()),
        // RDNA2: no range for the voltage offset, and 0 or below is all that was asked of it.
        _ if voltage => Ok(()),
        _ => Err(format!(
            "This GPU does not report the range of its {}, so Redline cannot check the value.",
            field.name
        )),
    }
}

/// The commands for `requested`, each checked against the table `status` was read from.
fn commands(requested: &[Requested], status: &ClocksStatus) -> Result<Vec<ClockCommand>, String> {
    if let Some(reason) = &status.unavailable_reason {
        return Err(reason.clone());
    }
    requested
        .iter()
        .map(|&(field, value)| {
            check(field, value, (field.control)(status))?;
            Ok(ClockCommand {
                setting: field.setting,
                value,
            })
        })
        .collect()
}

impl Lact {
    /// The controls of the LACT device `lact_id`, as `tuning_status` reports them. Unlike the
    /// status, an answer that says nothing is a refusal: there is no range to check against.
    fn clock_controls(&self, lact_id: &str) -> Result<ClocksStatus, String> {
        let system = self
            .client
            .system_info()
            .map_err(|error| self.reason(&error))?;
        let answer = self.client.device_clocks_info(lact_id);
        if let Err(error) = &answer
            && !matches!(error, LactError::Daemon(_))
        {
            return Err(self.reason(error));
        }
        clocks_status(answer, system.amdgpu_overdrive_enabled).ok_or_else(|| NO_CONTROLS.to_owned())
    }
}

impl Tuning {
    /// Applies `values`, by key in [`FIELDS`], as one change: a number sets a control, `null`
    /// clears it, a key left out stays as it is. Everything is checked before lactd is asked to
    /// change anything. Pending until confirmed. Blocking.
    pub fn set_clocks(
        &self,
        gpu_id: &str,
        values: &Map<String, Value>,
    ) -> Result<PendingChange, String> {
        let requested = requested(values)?;
        self.apply(gpu_id, TuningChange::Clocks, |lact, lact_id| {
            let commands = commands(&requested, &lact.clock_controls(lact_id)?)?;
            lact.client
                .batch_set_clocks(lact_id, &commands)
                .map_err(|error| lact.refusal(&error))
        })
    }

    /// Returns `gpu_id` to the firmware's clocks and voltages and clears them from lactd's
    /// configuration. Pending until confirmed. Blocking.
    pub fn reset_clocks(&self, gpu_id: &str) -> Result<PendingChange, String> {
        self.apply(gpu_id, TuningChange::Clocks, |lact, lact_id| {
            lact.client
                .reset_clocks(lact_id)
                .map_err(|error| lact.refusal(&error))
        })
    }
}

/// Applies clock and voltage values to `gpu_id`: `{ max_core_clock, core_clock_offset,
/// max_vram_clock, voltage_offset }`, each optional, in MHz and mV, `null` clearing it. The change
/// is pending: the user must confirm it with `tuning_confirm` before its deadline.
#[tauri::command]
pub async fn set_clocks(
    gpu_id: String,
    values: Map<String, Value>,
    tuning: State<'_, Tuning>,
) -> Result<PendingChange, String> {
    super::blocking(&tuning, move |tuning| tuning.set_clocks(&gpu_id, &values)).await?
}

/// Returns `gpu_id` to the firmware's clocks and voltages. The change is pending like any other.
#[tauri::command]
pub async fn reset_clocks(
    gpu_id: String,
    tuning: State<'_, Tuning>,
) -> Result<PendingChange, String> {
    super::blocking(&tuning, move |tuning| tuning.reset_clocks(&gpu_id)).await?
}

#[cfg(test)]
mod tests {
    use super::super::tests::{Collected, FakeLactd, ManualClock, Setup};
    use super::super::{
        NOT_MANAGED, NOTHING_PENDING, Outcome, PENDING_ELSEWHERE, PENDING_EVENT, PENDING_HERE,
        PENDING_OUTSIDE, SETTLED_EVENT,
    };
    use super::*;
    use redline_core::lact::VERIFIED_VERSION;
    use serde_json::{Value, json};
    use std::io;
    use std::sync::{Arc, Mutex};

    const GPU: &str = "1002:7550-1043:0643-0000:0d:00.0";
    const SLOT: &str = "0000:0d:00.0";

    /// lact-schema 0.10.1's two tags around a table of `amdgpu-sysfs` 0.21.0.
    fn wire(kind: &str, data: Value) -> Value {
        json!({ "table": { "type": "amd", "value": { "kind": kind, "data": data } } })
    }

    /// The tables below are the `parse_*_full` snapshots of `amdgpu-sysfs` 0.21.0.
    fn rdna2_rx6800() -> Value {
        wire(
            "rdna",
            json!({
                "current_sclk_range": { "min": 500, "max": 2314 }, "sclk_offset": null,
                "current_mclk_range": { "min": 97, "max": 1000 }, "vddc_curve": [],
                "voltage_offset": 0,
                "od_range": {
                    "sclk": { "min": 500, "max": 2600 }, "sclk_offset": null,
                    "mclk": { "min": 674, "max": 1075 }, "curve_sclk_points": [],
                    "curve_voltage_points": [], "voltage_offset": null
                }
            }),
        )
    }

    fn rdna3_rx7900xt(voltage_offset: Value) -> Value {
        wire(
            "rdna",
            json!({
                "current_sclk_range": { "min": 500, "max": 2735 }, "sclk_offset": null,
                "current_mclk_range": { "min": 97, "max": 1250 }, "vddc_curve": [],
                "voltage_offset": 0,
                "od_range": {
                    "sclk": { "min": 500, "max": 5000 }, "sclk_offset": null,
                    "mclk": { "min": 97, "max": 1500 }, "curve_sclk_points": [],
                    "curve_voltage_points": [], "voltage_offset": voltage_offset
                }
            }),
        )
    }

    fn rdna4_rx9070xt() -> Value {
        wire(
            "rdna",
            json!({
                "current_sclk_range": { "min": null, "max": null }, "sclk_offset": 0,
                "current_mclk_range": { "min": 97, "max": 1259 }, "vddc_curve": [],
                "voltage_offset": 0,
                "od_range": {
                    "sclk": null, "sclk_offset": { "min": -500, "max": 1000 },
                    "mclk": { "min": 97, "max": 1500 }, "curve_sclk_points": [],
                    "curve_voltage_points": [], "voltage_offset": { "min": -200, "max": 0 }
                }
            }),
        )
    }

    /// An integrated GPU: no VRAM clock and no voltage offset.
    fn rdna_phoenix() -> Value {
        wire(
            "rdna",
            json!({
                "current_sclk_range": { "min": 800, "max": 2700 }, "sclk_offset": null,
                "current_mclk_range": { "min": null, "max": null }, "vddc_curve": [],
                "voltage_offset": null,
                "od_range": {
                    "sclk": { "min": 800, "max": 2700 }, "sclk_offset": null, "mclk": null,
                    "curve_sclk_points": [], "curve_voltage_points": [], "voltage_offset": null
                }
            }),
        )
    }

    fn gcn_vega56() -> Value {
        wire(
            "gcn",
            json!({
                "sclk_levels": [{ "clockspeed": 852, "voltage": 800 }],
                "mclk_levels": [{ "clockspeed": 167, "voltage": 800 }],
                "od_range": {
                    "sclk": { "min": 852, "max": 2400 }, "mclk": { "min": 167, "max": 1500 },
                    "vddc": { "min": 800, "max": 1200 }
                }
            }),
        )
    }

    fn derive(info: Value, overdrive: Option<bool>) -> Option<ClocksStatus> {
        clocks_status(Ok(serde_json::from_value(info).unwrap()), overdrive)
    }

    fn range(current: i32, min: i32, max: i32) -> Option<ClockControl> {
        Some(ClockControl {
            current,
            min: Some(min),
            max: Some(max),
        })
    }

    #[test]
    fn rdna2_has_a_voltage_offset_with_no_range() {
        let clocks = derive(rdna2_rx6800(), Some(true)).unwrap();
        assert_eq!(clocks.unavailable_reason, None);
        assert_eq!(clocks.max_core_clock, range(2314, 500, 2600));
        assert_eq!(clocks.core_clock_offset, None);
        assert_eq!(clocks.max_vram_clock, range(1000, 674, 1075));
        assert_eq!(
            clocks.voltage_offset,
            Some(ClockControl {
                current: 0,
                min: None,
                max: None
            })
        );
    }

    #[test]
    fn rdna3_has_a_maximum_core_clock_and_a_voltage_range() {
        let clocks = derive(rdna3_rx7900xt(json!({ "min": -450, "max": 0 })), Some(true)).unwrap();
        assert_eq!(clocks.max_core_clock, range(2735, 500, 5000));
        assert_eq!(clocks.core_clock_offset, None);
        assert_eq!(clocks.max_vram_clock, range(1250, 97, 1500));
        assert_eq!(clocks.voltage_offset, range(0, -450, 0));
    }

    #[test]
    fn rdna4_has_a_core_clock_offset_and_no_maximum() {
        let clocks = derive(rdna4_rx9070xt(), Some(true)).unwrap();
        assert_eq!(clocks.max_core_clock, None);
        assert_eq!(clocks.core_clock_offset, range(0, -500, 1000));
        assert_eq!(clocks.max_vram_clock, range(1259, 97, 1500));
        assert_eq!(clocks.voltage_offset, range(0, -200, 0));
    }

    #[test]
    fn an_integrated_gpu_exposes_only_what_its_table_has() {
        let clocks = derive(rdna_phoenix(), Some(true)).unwrap();
        assert_eq!(clocks.max_core_clock, range(2700, 800, 2700));
        assert_eq!(clocks.max_vram_clock, None);
        assert_eq!(clocks.voltage_offset, None);
        // Absent is `null` on the wire, not zero.
        let json = serde_json::to_value(&clocks).unwrap();
        assert_eq!(json["max_vram_clock"], Value::Null);
        assert_eq!(json["voltage_offset"], Value::Null);
    }

    #[test]
    fn the_voltage_offset_range_never_goes_above_zero() {
        let voltage = |firmware: Value| {
            derive(rdna3_rx7900xt(firmware), Some(true))
                .unwrap()
                .voltage_offset
        };
        assert_eq!(
            voltage(json!({ "min": -450, "max": 100 })),
            range(0, -450, 0)
        );
        assert_eq!(voltage(json!({ "min": 0, "max": 100 })), range(0, 0, 0));
        // Nothing at or below 0, or half a range: no range is offered, none is invented.
        let no_range = Some(ClockControl {
            current: 0,
            min: None,
            max: None,
        });
        assert_eq!(voltage(json!({ "min": 50, "max": 100 })), no_range);
        assert_eq!(voltage(json!({ "min": -450, "max": null })), no_range);
    }

    #[test]
    fn the_last_curve_point_is_the_range_of_the_maximum_core_clock() {
        let mut info = rdna2_rx6800();
        info["table"]["value"]["data"]["od_range"]["curve_sclk_points"] =
            json!([{ "min": 500, "max": 900 }, { "min": 800, "max": 2500 }]);
        let clocks = derive(info, Some(true)).unwrap();
        assert_eq!(clocks.max_core_clock, range(2314, 800, 2500));
    }

    #[test]
    fn a_current_value_without_a_range_is_not_given_one() {
        let mut info = rdna2_rx6800();
        info["table"]["value"]["data"]["od_range"]["sclk"] = json!({ "min": 500, "max": null });
        let clock = derive(info, Some(true)).unwrap().max_core_clock.unwrap();
        assert_eq!((clock.current, clock.min, clock.max), (2314, None, None));
    }

    #[test]
    fn a_table_that_exposes_nothing_is_not_a_block() {
        assert_eq!(derive(wire("rdna", json!({})), Some(true)), None);
    }

    #[test]
    fn overdrive_off_offers_no_control_and_says_why() {
        let needed = Some(ClocksStatus::unavailable(NEEDS_OVERDRIVE));
        let refused = || {
            Err(LactError::Daemon(
                "Clocks table not available: No such file or directory".into(),
            ))
        };
        assert_eq!(clocks_status(refused(), Some(false)), needed);
        assert_eq!(derive(json!({ "table": null }), Some(false)), needed);
        // A table readable with overdrive off still offers nothing.
        assert_eq!(derive(rdna4_rx9070xt(), Some(false)), needed);
        // A table missing for another reason is not an overdrive problem.
        assert_eq!(clocks_status(refused(), Some(true)), None);
        assert_eq!(clocks_status(refused(), None), None);
    }

    #[test]
    fn a_gcn_table_says_the_family_is_not_supported_whatever_overdrive_says() {
        let unsupported = Some(ClocksStatus::unavailable(NOT_SUPPORTED));
        assert_eq!(derive(gcn_vega56(), Some(true)), unsupported);
        assert_eq!(derive(gcn_vega56(), Some(false)), unsupported);
        assert_eq!(derive(gcn_vega56(), None), unsupported);
    }

    #[test]
    fn a_table_of_another_kind_is_not_a_block() {
        let nvidia = json!({ "table": { "type": "nvidia", "value": {} } });
        assert_eq!(derive(nvidia.clone(), Some(true)), None);
        assert_eq!(derive(nvidia, Some(false)), None);
    }

    #[test]
    fn a_lactd_that_does_not_answer_is_null_even_with_overdrive_off() {
        let silent = LactError::Io(io::Error::from(io::ErrorKind::TimedOut));
        assert_eq!(clocks_status(Err(silent), Some(false)), None);
        assert_eq!(
            clocks_status(Err(LactError::SocketMissing), Some(true)),
            None
        );
        let garbled = LactError::Protocol("device_clocks_info: invalid".into());
        assert_eq!(clocks_status(Err(garbled), Some(false)), None);
    }

    /// A lactd with one GPU, the overdrive bit `overdrive` and `clocks` for `device_clocks_info`.
    fn lactd(setup: &Setup, overdrive: Option<bool>, clocks: Value) -> FakeLactd {
        FakeLactd::start(
            &setup.socket(),
            Box::new(move |command, args| match command {
                "system_info" => json!({
                    "version": VERIFIED_VERSION, "amdgpu_overdrive_enabled": overdrive
                }),
                "list_devices" => json!([{ "id": GPU }]),
                "device_info" => json!({ "flags": ["HasPmfw"] }),
                "device_stats" => json!({ "fan": {}, "power": { "cap_current": 317.0 } }),
                "device_clocks_info" => {
                    assert_eq!(args["id"], GPU);
                    clocks.clone()
                }
                "list_profiles" => json!({ "current_profile": "Quiet", "auto_switch": false }),
                _ => json!({ "error": "unexpected" }),
            }),
        )
    }

    #[test]
    fn tuning_status_carries_the_controls_of_the_gpu() {
        let setup = Setup::new("clocks-status");
        let _lactd = lactd(&setup, Some(true), rdna4_rx9070xt());
        let status = setup.lact(true).status(SLOT);
        assert!(status.available);
        let clocks = status.clocks.unwrap();
        assert_eq!(clocks.core_clock_offset, range(0, -500, 1000));
    }

    #[test]
    fn with_overdrive_off_tuning_status_says_so_and_the_rest_stands() {
        let setup = Setup::new("clocks-off");
        let refusal = json!({ "error": "Clocks table not available" });
        let _lactd = lactd(&setup, Some(false), refusal);
        let status = setup.lact(true).status(SLOT);
        assert!(status.available);
        assert_eq!(
            status.clocks,
            Some(ClocksStatus::unavailable(NEEDS_OVERDRIVE))
        );
    }

    #[test]
    fn a_failed_clocks_request_leaves_the_rest_of_tuning_status_working() {
        let setup = Setup::new("clocks-failed");
        // Neither a table nor a refusal: an answer this client cannot read.
        let _lactd = lactd(&setup, Some(true), json!("garbled"));
        let status = setup.lact(true).status(SLOT);
        assert!(status.available);
        assert_eq!(status.clocks, None);
        assert_eq!(status.power.unwrap().current_w, Some(317.0));
        assert_eq!(status.profile.as_deref(), Some("Quiet"));
    }

    #[test]
    fn unavailable_tuning_has_no_clocks() {
        let setup = Setup::new("clocks-unavailable");
        let status = setup.lact(false).status(SLOT);
        assert!(!status.available);
        assert_eq!(status.clocks, None);
    }

    #[test]
    fn the_serialized_shape_is_what_the_frontend_reads() {
        let clocks = derive(rdna4_rx9070xt(), Some(true)).unwrap();
        assert_eq!(
            serde_json::to_value(&clocks).unwrap(),
            json!({
                "unavailable_reason": null,
                "max_core_clock": null,
                "core_clock_offset": { "current": 0, "min": -500, "max": 1000 },
                "max_vram_clock": { "current": 1259, "min": 97, "max": 1500 },
                "voltage_offset": { "current": 0, "min": -200, "max": 0 }
            })
        );
    }

    // ---- Setting values ----

    const OTHER_GPU: &str = "1002:164e-1043:8877-0000:0c:00.0";
    const OTHER_SLOT: &str = "0000:0c:00.0";

    /// What the fake lactd holds: the answer to `device_clocks_info`, the overdrive bit, whether
    /// a change is pending, and every request that is not a read.
    struct Daemon {
        clocks: Value,
        overdrive: Option<bool>,
        pending: bool,
        requests: Vec<Value>,
    }

    /// lactd 0.10.1's rules: one pending change, refused setters while it exists, and an error
    /// for anything Redline must never send (`set_gpu_config` among them).
    fn pending_lactd(daemon: Arc<Mutex<Daemon>>) -> Box<crate::tuning::tests::Handler> {
        Box::new(move |command, args| {
            let mut daemon = daemon.lock().unwrap();
            match command {
                "system_info" => json!({
                    "version": VERIFIED_VERSION, "amdgpu_overdrive_enabled": daemon.overdrive
                }),
                "list_devices" => json!([{ "id": GPU }, { "id": OTHER_GPU }]),
                "device_clocks_info" => daemon.clocks.clone(),
                "device_info" => json!({}),
                "device_stats" => json!({ "power": { "cap_min": 200.0, "cap_max": 300.0 } }),
                "list_profiles" => json!({ "auto_switch": false }),
                _ => {
                    daemon
                        .requests
                        .push(json!({ "command": command, "args": args }));
                    match command {
                        "batch_set_clocks_value" | "set_clocks_value" | "set_power_cap"
                            if daemon.pending =>
                        {
                            json!({ "error": "There is an unconfirmed configuration change pending" })
                        }
                        "batch_set_clocks_value" | "set_clocks_value" | "set_power_cap" => {
                            daemon.pending = true;
                            json!(5)
                        }
                        "confirm_pending_config" => {
                            daemon.pending = false;
                            Value::Null
                        }
                        _ => json!({ "error": "unexpected" }),
                    }
                }
            }
        })
    }

    struct Bench {
        daemon: Arc<Mutex<Daemon>>,
        clock: Arc<ManualClock>,
        events: Arc<Collected>,
        tuning: Tuning,
        _lactd: FakeLactd,
        _setup: Setup,
    }

    impl Bench {
        fn start(name: &str, clocks: Value, overdrive: Option<bool>) -> Self {
            let setup = Setup::new(name);
            let daemon = Arc::new(Mutex::new(Daemon {
                clocks,
                overdrive,
                pending: false,
                requests: Vec::new(),
            }));
            let lactd = FakeLactd::start(&setup.socket(), pending_lactd(daemon.clone()));
            let clock = Arc::new(ManualClock::default());
            let events = Arc::new(Collected::default());
            let tuning = Tuning::new(setup.lact(true), clock.clone(), events.clone());
            Bench {
                daemon,
                clock,
                events,
                tuning,
                _lactd: lactd,
                _setup: setup,
            }
        }

        fn set(&self, values: Value) -> Result<PendingChange, String> {
            self.tuning
                .set_clocks(SLOT, values.as_object().expect("an object"))
        }

        fn requests(&self) -> Vec<Value> {
            std::mem::take(&mut self.daemon.lock().unwrap().requests)
        }

        /// `values` is refused with `sentence` and lactd is asked to change nothing.
        fn refuses(&self, values: Value, sentence: &str) {
            assert_eq!(
                self.set(values.clone()),
                Err(sentence.to_owned()),
                "{values}"
            );
            assert_eq!(self.requests(), Vec::<Value>::new(), "{values}");
            assert_eq!(self.events.take(), Vec::<Value>::new(), "{values}");
            assert_eq!(self.tuning.status(SLOT).pending, None);
        }

        /// `values` reaches lactd; the change is reverted so the next one can follow.
        fn accepts(&self, values: Value) {
            assert!(self.set(values.clone()).is_ok(), "{values}");
            self.tuning.settle(SLOT, false).unwrap();
            self.requests();
            self.events.take();
        }
    }

    fn rdna3() -> Value {
        rdna3_rx7900xt(json!({ "min": -450, "max": 0 }))
    }

    fn batch(commands: Value) -> Value {
        json!({
            "command": "batch_set_clocks_value",
            "args": { "id": GPU, "commands": commands }
        })
    }

    fn reset_request() -> Value {
        json!({
            "command": "set_clocks_value",
            "args": { "id": GPU, "command": { "type": "reset", "value": null } }
        })
    }

    fn confirm_request(answer: &str) -> Value {
        json!({ "command": "confirm_pending_config", "args": { "command": answer } })
    }

    #[test]
    fn every_value_goes_in_one_batch_and_the_change_is_pending() {
        let bench = Bench::start("batch", rdna3(), Some(true));
        bench.clock.advance(1_000);
        let change = bench
            .set(json!({
                "voltage_offset": -60, "max_vram_clock": 1300, "max_core_clock": 2600
            }))
            .unwrap();
        assert_eq!(
            change,
            PendingChange {
                gpu_id: SLOT.into(),
                what: TuningChange::Clocks,
                deadline_ms: 6_000,
            }
        );
        assert_eq!(
            bench.requests(),
            vec![batch(json!([
                { "type": "max_core_clock", "value": 2600 },
                { "type": "max_memory_clock", "value": 1300 },
                { "type": "voltage_offset", "value": -60 }
            ]))]
        );
        assert_eq!(
            bench.events.take(),
            vec![json!({
                "event": PENDING_EVENT, "gpu_id": SLOT, "what": "clocks", "deadline_ms": 6_000
            })]
        );
        assert_eq!(bench.tuning.status(SLOT).pending, Some(change));
    }

    #[test]
    fn the_core_clock_offset_is_the_offset_of_power_state_zero() {
        let bench = Bench::start("offset", rdna4_rx9070xt(), Some(true));
        bench.set(json!({ "core_clock_offset": -100 })).unwrap();
        assert_eq!(
            bench.requests(),
            vec![batch(
                json!([{ "type": { "gpu_clock_offset": 0 }, "value": -100 }])
            )]
        );
    }

    #[test]
    fn null_clears_a_control_and_a_key_left_out_stays() {
        let bench = Bench::start("clear", rdna3(), Some(true));
        bench
            .set(json!({ "max_core_clock": null, "voltage_offset": -10 }))
            .unwrap();
        assert_eq!(
            bench.requests(),
            vec![batch(json!([
                { "type": "max_core_clock", "value": null },
                { "type": "voltage_offset", "value": -10 }
            ]))]
        );
    }

    #[test]
    fn each_end_of_every_range_is_allowed_and_one_past_it_is_not() {
        let bench = Bench::start("ranges", rdna3(), Some(true));
        for (key, name, unit, min, max) in [
            ("max_core_clock", "maximum core clock", "MHz", 500, 5000),
            ("max_vram_clock", "maximum VRAM clock", "MHz", 97, 1500),
            ("voltage_offset", "voltage offset", "mV", -450, 0),
        ] {
            bench.accepts(json!({ key: min }));
            bench.accepts(json!({ key: max }));
            let sentence = format!("The {name} must be between {min} {unit} and {max} {unit}.");
            bench.refuses(json!({ key: min - 1 }), &sentence);
            if key != "voltage_offset" {
                bench.refuses(json!({ key: max + 1 }), &sentence);
            }
        }

        let bench = Bench::start("ranges-offset", rdna4_rx9070xt(), Some(true));
        bench.accepts(json!({ "core_clock_offset": -500 }));
        bench.accepts(json!({ "core_clock_offset": 1000 }));
        let sentence = "The core clock offset must be between -500 MHz and 1000 MHz.";
        bench.refuses(json!({ "core_clock_offset": -501 }), sentence);
        bench.refuses(json!({ "core_clock_offset": 1001 }), sentence);
    }

    #[test]
    fn one_bad_value_refuses_the_whole_request() {
        let bench = Bench::start("all-or-nothing", rdna3(), Some(true));
        bench.refuses(
            json!({ "max_core_clock": 2600, "voltage_offset": -451 }),
            "The voltage offset must be between -450 mV and 0 mV.",
        );
    }

    #[test]
    fn a_positive_voltage_offset_is_refused_with_or_without_a_range() {
        let bench = Bench::start("positive", rdna3(), Some(true));
        bench.refuses(json!({ "voltage_offset": 1 }), UNDERVOLT_ONLY);

        // RDNA2 gives no range and lactd would check nothing.
        let bench = Bench::start("positive-rdna2", rdna2_rx6800(), Some(true));
        bench.refuses(json!({ "voltage_offset": 1 }), UNDERVOLT_ONLY);
        bench.refuses(json!({ "voltage_offset": 50 }), UNDERVOLT_ONLY);
        bench.accepts(json!({ "voltage_offset": 0 }));
        bench.accepts(json!({ "voltage_offset": -50 }));
    }

    #[test]
    fn a_firmware_range_above_zero_does_not_let_a_positive_value_through() {
        let firmware = json!({ "min": -450, "max": 100 });
        let bench = Bench::start("positive-range", rdna3_rx7900xt(firmware), Some(true));
        bench.refuses(json!({ "voltage_offset": 50 }), UNDERVOLT_ONLY);
    }

    #[test]
    fn unknown_fields_are_refused_with_a_sentence() {
        let bench = Bench::start("unknown", rdna3(), Some(true));
        bench.refuses(
            json!({ "min_core_clock": 800, "max_core_clock": 2600 }),
            "`min_core_clock` is not a clock or voltage control Redline can set.",
        );
    }

    #[test]
    fn a_value_that_is_not_a_whole_number_is_refused() {
        let bench = Bench::start("numbers", rdna3(), Some(true));
        let sentence = "The maximum core clock must be a whole number of MHz.";
        for value in [
            json!(2600.5),
            json!("2600"),
            json!(true),
            json!(2_147_483_648_i64),
        ] {
            bench.refuses(json!({ "max_core_clock": value }), sentence);
        }
    }

    #[test]
    fn an_empty_request_changes_nothing() {
        let bench = Bench::start("empty", rdna3(), Some(true));
        bench.refuses(json!({}), NOTHING_TO_APPLY);
    }

    #[test]
    fn a_control_the_gpu_does_not_expose_is_refused() {
        let bench = Bench::start("absent", rdna4_rx9070xt(), Some(true));
        bench.refuses(
            json!({ "max_core_clock": 2600 }),
            "This GPU does not expose a maximum core clock.",
        );
        bench.refuses(
            json!({ "max_core_clock": null }),
            "This GPU does not expose a maximum core clock.",
        );

        let bench = Bench::start("absent-rdna3", rdna3(), Some(true));
        bench.refuses(
            json!({ "core_clock_offset": 100 }),
            "This GPU does not expose a core clock offset.",
        );

        let bench = Bench::start("absent-igpu", rdna_phoenix(), Some(true));
        bench.refuses(
            json!({ "max_vram_clock": 1000 }),
            "This GPU does not expose a maximum VRAM clock.",
        );
        bench.refuses(
            json!({ "voltage_offset": -10 }),
            "This GPU does not expose a voltage offset.",
        );
    }

    #[test]
    fn a_clock_without_a_firmware_range_is_refused_but_can_be_cleared() {
        let mut info = rdna2_rx6800();
        info["table"]["value"]["data"]["od_range"]["sclk"] = json!({ "min": 500, "max": null });
        let bench = Bench::start("no-range", info, Some(true));
        bench.refuses(
            json!({ "max_core_clock": 2000 }),
            "This GPU does not report the range of its maximum core clock, so Redline cannot \
             check the value.",
        );
        bench.accepts(json!({ "max_core_clock": null }));
    }

    #[test]
    fn what_tuning_status_says_blocks_the_setter_with_the_same_sentence() {
        let bench = Bench::start("overdrive-off", rdna3(), Some(false));
        bench.refuses(json!({ "max_core_clock": 2600 }), NEEDS_OVERDRIVE);

        let refusal = json!({ "error": "Clocks table not available" });
        let bench = Bench::start("no-table", refusal.clone(), Some(false));
        bench.refuses(json!({ "voltage_offset": -10 }), NEEDS_OVERDRIVE);

        let bench = Bench::start("gcn", gcn_vega56(), Some(true));
        bench.refuses(json!({ "max_core_clock": 1500 }), NOT_SUPPORTED);

        let bench = Bench::start("table-gone", refusal, Some(true));
        bench.refuses(json!({ "max_core_clock": 2600 }), NO_CONTROLS);

        let nvidia = json!({ "table": { "type": "nvidia", "value": {} } });
        let bench = Bench::start("nvidia", nvidia, Some(true));
        bench.refuses(json!({ "max_core_clock": 2600 }), NO_CONTROLS);
    }

    #[test]
    fn a_gpu_lact_does_not_manage_is_not_tunable() {
        let bench = Bench::start("unmanaged", rdna3(), Some(true));
        assert_eq!(
            bench.tuning.set_clocks(
                "0000:03:00.0",
                json!({ "voltage_offset": 0 }).as_object().unwrap()
            ),
            Err(NOT_MANAGED.to_owned())
        );
        assert_eq!(
            bench.tuning.reset_clocks("0000:03:00.0"),
            Err(NOT_MANAGED.to_owned())
        );
        assert_eq!(bench.requests(), Vec::<Value>::new());
    }

    #[test]
    fn a_confirmed_change_settles_once() {
        let bench = Bench::start("confirm", rdna3(), Some(true));
        bench.set(json!({ "voltage_offset": -60 })).unwrap();
        bench.requests();
        bench.events.take();
        assert_eq!(bench.tuning.settle(SLOT, true), Ok(Outcome::Confirmed));
        assert_eq!(bench.requests(), vec![confirm_request("confirm")]);
        assert_eq!(
            bench.events.take(),
            vec![json!({ "event": SETTLED_EVENT, "gpu_id": SLOT, "outcome": "confirmed" })]
        );
        assert_eq!(
            bench.tuning.settle(SLOT, true),
            Err(NOTHING_PENDING.to_owned())
        );
    }

    #[test]
    fn a_reverted_change_settles_once() {
        let bench = Bench::start("revert", rdna3(), Some(true));
        bench.set(json!({ "voltage_offset": -60 })).unwrap();
        bench.requests();
        bench.events.take();
        assert_eq!(bench.tuning.settle(SLOT, false), Ok(Outcome::Reverted));
        assert_eq!(bench.requests(), vec![confirm_request("revert")]);
        assert_eq!(
            bench.events.take(),
            vec![json!({ "event": SETTLED_EVENT, "gpu_id": SLOT, "outcome": "reverted" })]
        );
    }

    #[test]
    fn at_the_deadline_redline_sends_nothing() {
        let bench = Bench::start("expire", rdna3(), Some(true));
        bench.set(json!({ "voltage_offset": -60 })).unwrap();
        bench.requests();
        bench.events.take();
        bench.clock.advance(5_000);
        assert_eq!(
            bench.events.take(),
            vec![json!({ "event": SETTLED_EVENT, "gpu_id": SLOT, "outcome": "expired" })]
        );
        assert_eq!(bench.requests(), Vec::<Value>::new());
    }

    #[test]
    fn a_reset_is_sent_alone_and_is_pending() {
        let bench = Bench::start("reset", rdna3(), Some(true));
        bench.clock.advance(1_000);
        let change = bench.tuning.reset_clocks(SLOT).unwrap();
        assert_eq!(change.what, TuningChange::Clocks);
        assert_eq!(change.deadline_ms, 6_000);
        assert_eq!(bench.requests(), vec![reset_request()]);
        assert_eq!(bench.tuning.settle(SLOT, true), Ok(Outcome::Confirmed));

        bench.requests();
        bench.tuning.reset_clocks(SLOT).unwrap();
        assert_eq!(bench.tuning.settle(SLOT, false), Ok(Outcome::Reverted));
        assert_eq!(
            bench.requests(),
            vec![reset_request(), confirm_request("revert")]
        );
    }

    #[test]
    fn a_reset_is_the_way_out_whatever_the_table_says() {
        // After overdrive is turned off, or with a table that cannot be read, a stored value can
        // still come back at every start of lactd.
        for (name, clocks, overdrive) in [
            ("reset-off", rdna3(), Some(false)),
            ("reset-gone", json!({ "error": "no table" }), Some(true)),
        ] {
            let bench = Bench::start(name, clocks, overdrive);
            bench.tuning.reset_clocks(SLOT).unwrap();
            assert_eq!(bench.requests(), vec![reset_request()]);
        }
    }

    #[test]
    fn a_change_while_another_is_pending_is_refused_before_lactd() {
        let bench = Bench::start("pending", rdna3(), Some(true));
        bench.tuning.set_power_limit(SLOT, Some(250.0)).unwrap();
        bench.requests();
        bench.events.take();

        assert_eq!(
            bench.set(json!({ "voltage_offset": -60 })),
            Err(PENDING_HERE.to_owned())
        );
        assert_eq!(
            bench.tuning.reset_clocks(SLOT),
            Err(PENDING_HERE.to_owned())
        );
        assert_eq!(
            bench.tuning.set_clocks(
                OTHER_SLOT,
                json!({ "voltage_offset": -60 }).as_object().unwrap()
            ),
            Err(PENDING_ELSEWHERE.to_owned())
        );
        assert_eq!(
            bench.tuning.reset_clocks(OTHER_SLOT),
            Err(PENDING_ELSEWHERE.to_owned())
        );
        assert_eq!(bench.requests(), Vec::<Value>::new());
        assert_eq!(bench.events.take(), Vec::<Value>::new());
    }

    #[test]
    fn a_change_pending_in_another_lact_client_is_named() {
        let bench = Bench::start("outside", rdna3(), Some(true));
        bench.daemon.lock().unwrap().pending = true;
        assert_eq!(
            bench.set(json!({ "voltage_offset": -60 })),
            Err(PENDING_OUTSIDE.to_owned())
        );
        assert_eq!(
            bench.tuning.reset_clocks(SLOT),
            Err(PENDING_OUTSIDE.to_owned())
        );
        assert_eq!(bench.tuning.status(SLOT).pending, None);
        assert_eq!(bench.events.take(), Vec::<Value>::new());
    }

    #[test]
    fn a_daemon_that_stops_answering_says_what_to_do() {
        let bench = Bench::start("stopped", rdna3(), Some(true));
        let Bench { tuning, _lactd, .. } = bench;
        drop(_lactd);
        let error = tuning
            .set_clocks(SLOT, json!({ "voltage_offset": -60 }).as_object().unwrap())
            .unwrap_err();
        assert_eq!(error, crate::tuning::NOT_RUNNING);
    }
}
