//! Clock and voltage controls: what the firmware of a GPU lets LACT change, read only.
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

use redline_core::lact::{
    ClockRange, ClocksInfo, ClocksTable, LactError, LactResult, RdnaClocksTable,
};
use serde::Serialize;

pub const NEEDS_OVERDRIVE: &str = "Clock and voltage controls need overdrive. Enable overdrive \
    (the `amdgpu.ppfeaturemask` kernel parameter) and reboot.";
pub const NOT_SUPPORTED: &str = "Redline does not support clock and voltage controls on this GPU \
    family.";

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

#[cfg(test)]
mod tests {
    use super::super::tests::{FakeLactd, Setup};
    use super::*;
    use redline_core::lact::VERIFIED_VERSION;
    use serde_json::{Value, json};
    use std::io;

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
}
