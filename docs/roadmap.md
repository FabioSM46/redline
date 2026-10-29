# Roadmap

Milestones are `Iteration N` on GitHub; this is the direction, not a calendar.

## M0 — Foundations (done)

- [x] Rust workspace with `redline-core` and the `redline-probe` CLI
- [x] Sensor reading from sysfs
- [x] `lactd` client, verified against a running `lactd`
- [x] GPU Screen Recorder control: replay verified on real hardware, with desktop audio and
  microphone on two separate tracks
- [x] Tests of `sensors` on fake sysfs trees (the `tests` module of
  `src-tauri/crates/redline-core/src/sensors.rs`)
- [x] Minimal Tauri app with live metrics
- [x] Development pipeline ported from wow-don
- [x] Repository in English

## M1 — Performance (done)

- [x] App shell (D8): the four tabs, the status bar, the tray icon with close-to-tray and its
  fallback, single instance, and Settings → General (theme, start with the system, start
  minimised to the tray)
- [x] Home and Performance → Metrics: dashboard with charts over time (temperature, clocks,
  power, usage) over a 1, 5 or 15 minute window, drawn with uPlot (D9)
- [x] Several GPUs on the same machine, with a selector shown only when there is more than one
- [x] Checked by hand on real hardware (RDNA4, GNOME on Wayland)

## M2 — Recording

- Instant replay from the app, with a choice of audio tracks
- Recording started and stopped from the app
- Settings → Shortcuts: customisable key bindings, through the GlobalShortcuts portal where
  available and desktop commands elsewhere (D5, D8)
- Settings → Capture: choice of the screen to record, codec and quality
- Notification with the saved file

## M3 — Tuning

- Fan curve, power limit, profiles
- Confirmation of changes in the UI
- Guided dependency setup (D4)

## M4 — Overlay and release

- In-game overlay through MangoHud
- First published `.deb`
