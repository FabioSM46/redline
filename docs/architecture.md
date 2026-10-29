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
| `recorder` | Starts GPU Screen Recorder in replay mode with its IPC socket, saves the replay over IPC, stops it with signals sent only to the process tree it started, one audio track per source. Lists audio devices, applications playing audio and monitors; holds the allowed `-k`, `-q`, `-f`, `-r` values (`CODECS`, `QUALITIES`, `FPS`, `REPLAY_SECONDS`). Verified against GPU Screen Recorder 6.1.6 (`VERIFIED_VERSION`) | replay verified on real hardware; IPC, process tree and stdout parsing tested with fakes |
| `media` | The length of a saved video from its container header, without decoding and without extra tools: Matroska/WebM (`Info → Duration`) and MP4/MOV (`mvhd`). Anything else, or a header without a usable duration (fragmented MP4, a file still being written), is unknown | tested with synthetic headers; checked against ffprobe on files written by ffmpeg |
| `thumbnail` | The preview of a saved video, never decoded in-process: the desktop's thumbnail cache (`large/`, `normal/`, named after the MD5 of the GLib file URI, valid only while `Thumb::MTime` matches), then Redline's own cache, then one JPEG frame (256 px) written by `ffmpegthumbnailer` or `ffmpeg` found on PATH, at most one at a time, each killed after 15 s (a caller that waits more than 60 s for its turn gets no preview and nothing is cached). A failure is cached with the tools that were tried until the file changes, other tools try again and a timeout-only failure is retried after an hour; with no tool nothing is cached. MD5, base64 and the PNG `tEXt` reader are in the module (no new dependency) | tested with fake tools on a fake PATH; checked with ffmpeg on generated mkv and mp4 files |

## Tauri app modules (`src-tauri/src/`)

| Module | Responsibility | Commands |
|---|---|---|
| `sampler` | Background thread started in `setup`: discovers the AMD GPUs and reads them once per second into one `history` buffer per GPU id (see "Statistics" below) | `gpu_snapshot`, `gpu_history` |
| `settings` | `Settings { theme: "system" \| "light" \| "dark", start_minimised, recording }` in `settings.json` inside `app_config_dir()`. `recording { replay_seconds, audio_tracks, replay_dir, recording_dir, capture, codec, quality, fps, replay_on_start }`; the folders default to `<video folder>/Redline/{Replays,Recordings}`. A missing file means the defaults; an unreadable or invalid file means the defaults plus a line on stderr, and a single bad field (also inside `recording`) falls back to its own default. Writes are atomic (temporary file + `rename`). `set_settings` without `recording` keeps the saved one; an invalid `recording` is refused with every reason. A new `capture` deletes the portal session token (`portal-session-token` in `app_data_dir()`), a recording change restarts a running replay. Other modules read the recording settings with `settings::recording_settings(app)` | `get_settings`, `set_settings` |
| `autostart` | Starting with the system through `tauri-plugin-autostart` (an XDG autostart entry), used only by the backend: the frontend has no `autostart:*` permission. The entry launches the app with `--autostart`. The operating system is the only source of truth: the state is not stored in `settings.json` and is read back after every write | `get_autostart`, `set_autostart` |
| `recorder` | The instant replay service: at most one replay session for the whole app, in the app process (D8). `RecorderStatus { available, unavailable_reason, replay: "off" \| "starting" \| "on" \| "error", recording, replay_seconds, error, warning }`, sent again as the `recorder-state` event on every change; `file-saved { kind: "replay" \| "recording", file_name, saved_at_ms }` for every file GPU Screen Recorder saves (the name only, never the path). Its `ReplayConfig` comes from the recording settings; `replay_on_start` starts it with the app. `recording_toggle` starts or stops a regular recording, in the replay process or, with the replay off, in its own process (never both). Stopped on app exit, keeping a recording in progress | `recorder_status`, `replay_start`, `replay_stop`, `replay_save`, `recording_toggle` |
| `sources` | What the recording settings can choose from, as `{ available, unavailable_reason, items: [{ id, label }] }`: audio sources (`default_output`, `default_input`, `device:<name>`, then running applications as `app:<name>`) and capture targets (`portal` first, then monitors by name). Without GPU Screen Recorder: `available: false` and what to install | `list_audio_sources`, `list_capture_targets` |
| `gallery` | The saved clips of the replay and recording folders (read through `gallery::folders`, from the recording settings at every call): `Recording { id, kind: "replay" \| "recording", file_name, saved_at_ms, size_bytes, duration_s }`, newest first, only regular video files directly inside the folders. `id` is opaque: the backend maps it back to a path only by listing the folders again, so it never names a file outside them. Also the desktop notification ("Replay saved" / "Recording saved" with the file name) for every `file-saved` | `list_recordings`, `open_recording`, `show_in_folder` |
| `thumbnails` | Previews of saved clips by gallery id: `recording_thumbnail(id)` is a `data:image/png` or `data:image/jpeg` base64 URL, or `null`; `thumbnail_support()` is `{ generator: "ffmpegthumbnailer" \| "ffmpeg" \| null, hint }`, the hint saying what to install. Desktop cache in `cache_dir()/thumbnails` (read only), Redline's in `app_cache_dir()/thumbnails`, pruned by `list_recordings` | `recording_thumbnail`, `thumbnail_support` |
| `desktop` | Two session bus calls through GIO (`extern "C"`, as `tray`): `org.freedesktop.Notifications.Notify` and `org.freedesktop.FileManager1.ShowItems`. A missing service is `false`, never an error | — |

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
and `-ipc <socket>`, everything else from the recording settings. With portal capture the
session is restored from Redline's own token file (`-portal-session-token-filepath`), and changing
`capture` deletes it so the desktop asks again. The socket is in `$XDG_RUNTIME_DIR/redline/` (shared with the Flatpak sandbox
through `--filesystem`, as is the token's folder). The Flatpak has a private `/tmp`, so an output
folder under `/tmp` is refused with its name. The replay is `starting` until GPU Screen Recorder answers on the socket,
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

**Recording**: `recording_toggle` never starts a second GPU Screen Recorder process. With the
replay `on`, it sends the IPC requests `start-replay-recording` / `stop-replay-recording` to the
replay process, which records into `recording_dir` (`-ro`); while the replay is `starting` it is
refused. With the replay off, a separate process records one file,
`<recording_dir>/Video_<date>_<time>.mkv`, with the same settings and no buffer, and is stopped like
the replay (SIGINT); the replay cannot start while it runs. Stopping with SIGINT finishes and keeps a
recording in progress, so turning the replay off, a settings change that restarts it, and quitting
Redline all keep the file. GPU Screen Recorder prints the path of every finished recording, which
becomes `file-saved` with `kind: "recording"`.

**Command-line actions** (D5): `redline --save-replay`, `redline --toggle-replay` and
`redline --toggle-recording`, to bind in the desktop's custom shortcuts. The list is
`ACTION_ARGS` in `src-tauri/src/actions.rs`, the only place that names them. A second launch
forwards its arguments to the running instance through the single-instance plugin: with an
action the instance performs it without showing or focusing the window, without one it brings
the window to the front. A first launch with an action starts hidden in the tray (when there is
one) and performs the action once GPU Screen Recorder has been detected and `replay_on_start`
has run. The actions call the same `Recorder` methods as the commands (`--toggle-replay` stops a
replay that is `starting` or `on` and starts it otherwise); a failure is a desktop notification
with the same sentence. Unknown arguments are ignored, and one launch performs one action.

**Saved clips**: every `file-saved` also sends a desktop notification over D-Bus with the file
name only; without a notification server nothing is shown and nothing fails. `list_recordings`
lists both folders (missing folders are empty); `open_recording(id)` runs `xdg-open` on the file
and `show_in_folder(id)` asks the file manager to select it (`FileManager1.ShowItems`), falling
back to `xdg-open` on the folder. The frontend only ever holds ids, never paths.
`recording_thumbnail(id)` resolves the id the same way and answers with a data URL from the
desktop's thumbnail cache, Redline's cache or a frame written by `ffmpegthumbnailer`/`ffmpeg`
(on the blocking pool, one tool at a time, with a timeout), or `null`. Without either tool
`thumbnail_support` carries the hint to install ffmpeg. Every `list_recordings` removes from
Redline's cache the previews of clips that no longer exist or have changed.
