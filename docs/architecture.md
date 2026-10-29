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
| `lact` | Client of the `lactd` socket: devices, statistics, power cap, confirming or reverting changes | prototype, verified on real hardware |
| `recorder` | Starts GPU Screen Recorder in replay mode, saves the replay, starts and stops recording, one audio track per source | prototype, replay verified on real hardware |

## Flows

**Statistics**: the frontend calls `gpu_snapshot` every second; the backend reads sysfs. No
permissions, no daemon.

**Tuning**: the frontend asks for a change → the backend sends it to `lactd` → `lactd` applies it
pending confirmation → the user confirms or cancels → the backend sends `confirm_pending_config`.
Without confirmation `lactd` reverts to the previous values by itself.

**Replay**: the backend starts GPU Screen Recorder with the buffer in RAM and one audio track per
chosen source (`-a` repeated). Saving is a signal (or an IPC command); GPU Screen Recorder writes
the path of the saved file to stdout and the backend forwards it to the UI.
