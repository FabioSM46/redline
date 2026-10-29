# Roadmap

Milestones are `Iteration N` on GitHub; this is the direction, not a calendar.

## M0 — Foundations (in progress)

- [x] Rust workspace with `redline-core` and the `redline-probe` CLI
- [x] Sensor reading from sysfs
- [x] `lactd` client and GPU Screen Recorder control (replay still to verify on real hardware)
- [x] Minimal Tauri app with live metrics
- [x] Development pipeline ported from wow-don
- [x] Repository in English

## M1 — Performance

- Dashboard with charts over time (temperature, clocks, power, usage)
- Several GPUs on the same machine
- Tests of `sensors` on fake sysfs trees

## M2 — Recording

- Instant replay from the app, with a choice of audio tracks
- Recording started and stopped from the app
- Commands for desktop shortcuts (D5)
- Notification with the saved file

## M3 — Tuning

- Fan curve, power limit, profiles
- Confirmation of changes in the UI
- Guided dependency setup (D4)

## M4 — Overlay and release

- In-game overlay through MangoHud
- First published `.deb`
