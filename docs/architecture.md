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

## Tauri app modules (`src-tauri/src/`)

| Module | Responsibility | Commands |
|---|---|---|
| `settings` | `Settings { theme: "system" \| "light" \| "dark", start_minimised }` in `settings.json` inside `app_config_dir()`. A missing file means the defaults (`system`, `false`); an unreadable or invalid file means the defaults plus a line on stderr, and a single bad field falls back to its own default. Writes are atomic (temporary file + `rename`) | `get_settings`, `set_settings` |
| `autostart` | Starting with the system through `tauri-plugin-autostart` (an XDG autostart entry), used only by the backend: the frontend has no `autostart:*` permission. The entry launches the app with `--autostart`. The operating system is the only source of truth: the state is not stored in `settings.json` and is read back after every write | `get_autostart`, `set_autostart` |

## Flows

**Statistics**: the frontend calls `list_gpus` once (every AMD GPU, sorted by PCI slot; the id
is the PCI slot, the same one LACT uses), then `gpu_snapshot(gpu_id)` every second; the backend
reads sysfs. No permissions, no daemon.

**Tuning**: the frontend asks for a change → the backend sends it to `lactd` → `lactd` applies it
pending confirmation → the user confirms or cancels → the backend sends `confirm_pending_config`.
Without confirmation `lactd` reverts to the previous values by itself.

**Replay**: the backend starts GPU Screen Recorder with the buffer in RAM and one audio track per
chosen source (`-a` repeated). Saving is a signal (or an IPC command); GPU Screen Recorder writes
the path of the saved file to stdout and the backend forwards it to the UI.
