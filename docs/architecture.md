# Architecture

```
┌──────────────────────── Redline (user, never root) ─────────────────────────┐
│  React frontend (src/)  ──invoke()──▶  Tauri app (src-tauri/src/)           │
│                                              │                              │
│                                       redline-core                          │
│                    ┌───────────────┬─────────┴──────────┬────────────────┐  │
│                    │ sensors       │ lact               │ recorder       │  │
└────────────────────┼───────────────┼────────────────────┼────────────────┼──┘
                     ▼               ▼                    ▼                ▼
               amdgpu sysfs   /run/lactd.sock     gpu-screen-recorder   MangoHud
               (read only)    (lactd, root)       (external process)    (config)
```

## `redline-core` modules

| Module | Responsibility | Status |
|---|---|---|
| `sensors` | Finds AMD GPUs in `/sys/class/drm`, reads temperatures, power, fan, clocks, voltage, usage and VRAM | prototype, tested on RDNA4 |
| `history` | Ring buffer of timestamped `GpuSnapshot`s: 900 samples (15 minutes at 1 Hz), `push`, `latest` and `window(seconds)` in chronological order. Pure: no clock, no sysfs | tested with synthetic samples |
| `lact` | Client of the `lactd` socket: devices, statistics, power cap, confirming or reverting changes | prototype, verified on real hardware |
| `recorder` | Starts GPU Screen Recorder in replay mode, saves the replay, starts and stops recording, one audio track per source | prototype, replay verified on real hardware |

## Flows

**Statistics**: a sampler thread in the app (`src-tauri/src/sampler.rs`, started in `setup`)
discovers the AMD GPUs and reads sysfs once per second, then pushes each reading into one
`history` buffer per GPU id, without holding the lock while it reads. It keeps running while the
window is hidden in the tray (D8). The frontend calls `list_gpus` once (every AMD GPU, sorted by
PCI slot; the id is the PCI slot, the same one LACT uses), then `gpu_snapshot(gpu_id)`, which
returns the sampler's latest reading, and `gpu_history(gpu_id, window_s)` for the charts
(`window_s` is 60, 300 or 900; samples are `{ timestamp_ms, snapshot }`, oldest first, with
wall-clock milliseconds and missing sensors as `null`). An unknown id is an error; a GPU that
disappears stops being sampled, keeps its history and has no current reading. No permissions, no
daemon, nothing written to disk.

**Tuning**: the frontend asks for a change → the backend sends it to `lactd` → `lactd` applies it
pending confirmation → the user confirms or cancels → the backend sends `confirm_pending_config`.
Without confirmation `lactd` reverts to the previous values by itself.

**Replay**: the backend starts GPU Screen Recorder with the buffer in RAM and one audio track per
chosen source (`-a` repeated). Saving is a signal (or an IPC command); GPU Screen Recorder writes
the path of the saved file to stdout and the backend forwards it to the UI.
