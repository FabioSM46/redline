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
| `recorder` | Starts GPU Screen Recorder in replay mode with its IPC socket, saves the replay over IPC, stops it with signals sent only to the process tree it started, one audio track per source. Verified against GPU Screen Recorder 6.1.6 (`VERIFIED_VERSION`) | replay verified on real hardware; IPC, process tree and stdout parsing tested with fakes |

## Tauri app modules (`src-tauri/src/`)

| Module | Responsibility | Commands |
|---|---|---|
| `sampler` | Background thread started in `setup`: discovers the AMD GPUs and reads them once per second into one `history` buffer per GPU id (see "Statistics" below) | `gpu_snapshot`, `gpu_history` |
| `settings` | `Settings { theme: "system" \| "light" \| "dark", start_minimised }` in `settings.json` inside `app_config_dir()`. A missing file means the defaults (`system`, `false`); an unreadable or invalid file means the defaults plus a line on stderr, and a single bad field falls back to its own default. Writes are atomic (temporary file + `rename`) | `get_settings`, `set_settings` |
| `autostart` | Starting with the system through `tauri-plugin-autostart` (an XDG autostart entry), used only by the backend: the frontend has no `autostart:*` permission. The entry launches the app with `--autostart`. The operating system is the only source of truth: the state is not stored in `settings.json` and is read back after every write | `get_autostart`, `set_autostart` |
| `recorder` | The instant replay service: at most one replay session for the whole app, in the app process (D8). `RecorderStatus { available, unavailable_reason, replay: "off" \| "starting" \| "on" \| "error", recording, replay_seconds, error, warning }`, sent again as the `recorder-state` event on every change; `file-saved { kind: "replay" \| "recording", file_name, saved_at_ms }` for every file GPU Screen Recorder saves (the name only, never the path). Stopped on app exit | `recorder_status`, `replay_start`, `replay_stop`, `replay_save` |

## Flows

**Statistics**: a sampler thread in the app (`src-tauri/src/sampler.rs`, started in `setup`)
discovers the AMD GPUs and reads sysfs once per second, then pushes each reading into one
`history` buffer per GPU id, without holding the lock while it reads. It keeps running while the
window is hidden in the tray (D8). The frontend calls `list_gpus` once (every AMD GPU, sorted by
PCI slot; the id is the PCI slot, the same one LACT uses), then `gpu_snapshot(gpu_id)`, which
returns the sampler's latest reading, and `gpu_history(gpu_id, window_s)` for the charts
(`window_s` is 60, 300 or 900; samples are `{ timestamp_ms, snapshot }`, oldest first, with
wall-clock milliseconds and missing sensors as `null`). A GPU the sampler has not recorded yet
(it appeared since the last round) is read live, so the two commands never disagree with
`list_gpus`; its history is empty until the next round. An unknown id is an error; a GPU that
disappears stops being sampled, keeps its history and has no current reading. A failing discovery
is logged on stderr once when it starts and once when it ends. No permissions, no
daemon, nothing written to disk.

**Tuning**: the frontend asks for a change → the backend sends it to `lactd` → `lactd` applies it
pending confirmation → the user confirms or cancels → the backend sends `confirm_pending_config`.
Without confirmation `lactd` reverts to the previous values by itself.

**Replay**: the `recorder` app module starts GPU Screen Recorder (the native binary on PATH,
otherwise the Flatpak) with the buffer in RAM, one audio track per chosen source (`-a` repeated)
and `-ipc <socket>`, the socket in `$XDG_RUNTIME_DIR/redline/` (shared with the Flatpak sandbox
through `--filesystem`). The replay is `starting` until GPU Screen Recorder answers on the socket,
which it does only once capture runs (with the portal: after the user has picked a screen), then
`on`. Saving is the IPC `save-replay` request; GPU Screen Recorder writes the path of the saved
file to stdout and the backend sends `file-saved` with the file name. Stopping is SIGINT to the
GPU Screen Recorder process found among the descendants of the process Redline started (with the
Flatpak, `flatpak run` becomes `bwrap`, which does not forward signals), then SIGKILL to that tree
after five seconds: no other process of the user is ever touched. An exit nobody asked for turns
the replay to `error` with GPU Screen Recorder's last error line. Quitting Redline stops the
replay. Without GPU Screen Recorder `available` is false, `unavailable_reason` says what to
install and every command fails with that sentence. Options, signals and the IPC protocol are
verified against GPU Screen Recorder 6.1.6; another version is reported in `warning`.
