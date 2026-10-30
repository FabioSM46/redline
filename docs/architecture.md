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
| `lact` | Client of the `lactd` socket (one connection and one JSON line per request, 5 s timeouts): typed, read-only subsets of LACT's answers for `system_info`, `list_devices`, `device_info`, `device_stats`, `get_gpu_config` and `list_profiles` (unknown fields ignored, missing ones `None`), plus `set_power_cap` (answers the seconds lactd waits before reverting) and `confirm_pending_config` (`confirm` / `revert`). Profiles: `list_profiles` also gives the sorted profile names (`ProfilesInfo.profiles`), `set_profile` (always `auto_switch: false`), `create_profile` (`ProfileBase::Default` or `Profile(name)`) and `delete_profile`. `LactError` tells apart a missing socket, a stale one (nobody listening), a denied one (not in lactd's admin group), an I/O failure or timeout, an answer it does not understand and a refusal from the daemon. `find_device` maps a PCI slot to the LACT id that ends with it. Verified against LACT 0.10.1 (`VERIFIED_VERSION`) | typed answers checked against lactd 0.10.1 on real hardware; parsing and socket errors tested with a fake socket |
| `recorder` | Starts GPU Screen Recorder in replay mode with its IPC socket, saves the replay over IPC, stops it with signals sent only to the process tree it started, one audio track per source. Lists audio devices, applications playing audio and monitors; holds the allowed `-k`, `-q`, `-f`, `-r` values (`CODECS`, `QUALITIES`, `FPS`, `REPLAY_SECONDS`). Verified against GPU Screen Recorder 6.1.6 (`VERIFIED_VERSION`) | replay verified on real hardware; IPC, process tree and stdout parsing tested with fakes |
| `media` | The length of a saved video from its container header, without decoding and without extra tools: Matroska/WebM (`Info → Duration`) and MP4/MOV (`mvhd`). Anything else, or a header without a usable duration (fragmented MP4, a file still being written), is unknown | tested with synthetic headers; checked against ffprobe on files written by ffmpeg |
| `thumbnail` | The preview of a saved video, never decoded in-process: the desktop's thumbnail cache (`large/`, `normal/`, named after the MD5 of the GLib file URI, valid only while `Thumb::MTime` matches), then Redline's own cache, then one JPEG frame (256 px) written by `ffmpegthumbnailer` or `ffmpeg` found on PATH, at most one at a time, each killed after 15 s (a caller that waits more than 60 s for its turn gets no preview and nothing is cached). A failure is cached with the tools that were tried until the file changes, other tools try again and a timeout-only failure is retried after an hour; with no tool nothing is cached. MD5, base64 and the PNG `tEXt` reader are in the module (no new dependency) | tested with fake tools on a fake PATH; checked with ffmpeg on generated mkv and mp4 files |

## Tauri app modules (`src-tauri/src/`)

| Module | Responsibility | Commands |
|---|---|---|
| `sampler` | Background thread started in `setup`: discovers the AMD GPUs and reads them once per second into one `history` buffer per GPU id (see "Statistics" below) | `gpu_snapshot`, `gpu_history` |
| `settings` | `Settings { theme: "system" \| "light" \| "dark", start_minimised, recording, overlay }` in `settings.json` inside `app_config_dir()`. `recording { replay_seconds, audio_tracks, replay_dir, recording_dir, capture, codec, quality, fps, replay_on_start }`; the folders default to `<video folder>/Redline/{Replays,Recordings}`, and `recording_defaults` returns those two defaults (`null` without a video folder) for "Reset to default". A missing file means the defaults; an unreadable or invalid file means the defaults plus a line on stderr, and a single bad field (also inside `recording`) falls back to its own default. Writes are atomic (temporary file + `rename`). `set_settings` without `recording` keeps the saved one; an invalid `recording` is refused with every reason. A new `capture` deletes the portal session token (`portal-session-token` in `app_data_dir()`), a recording change restarts a running replay. Other modules read the recording settings with `settings::recording_settings(app)`. `overlay { preset, position, metrics, toggle_key }` (see `overlay`) is saved only by `set_overlay`, field by field on load like `recording`; `set_settings` never changes it | `get_settings`, `set_settings`, `recording_defaults` |
| `autostart` | Starting with the system through `tauri-plugin-autostart` (an XDG autostart entry), used only by the backend: the frontend has no `autostart:*` permission. The entry launches the app with `--autostart`. The operating system is the only source of truth: the state is not stored in `settings.json` and is read back after every write | `get_autostart`, `set_autostart` |
| `recorder` | The instant replay service: at most one replay session for the whole app, in the app process (D8). `RecorderStatus { available, unavailable_reason, replay: "off" \| "starting" \| "on" \| "error", recording, replay_seconds, error, warning }`, sent again as the `recorder-state` event on every change; `file-saved { kind: "replay" \| "recording", file_name, saved_at_ms }` for every file GPU Screen Recorder saves (the name only, never the path). Its `ReplayConfig` comes from the recording settings; `replay_on_start` starts it with the app. `recording_toggle` starts or stops a regular recording, in the replay process or, with the replay off, in its own process (never both). Stopped on app exit, keeping a recording in progress | `recorder_status`, `replay_start`, `replay_stop`, `replay_save`, `recording_toggle` |
| `sources` | What the recording settings can choose from, as `{ available, unavailable_reason, items: [{ id, label }] }`: audio sources (`default_output`, `default_input`, `device:<name>`, then running applications as `app:<name>`) and capture targets (`portal` first, then monitors by name). Without GPU Screen Recorder: `available: false` and what to install | `list_audio_sources`, `list_capture_targets` |
| `gallery` | The saved clips of the replay and recording folders (read through `gallery::folders`, from the recording settings at every call): `Recording { id, kind: "replay" \| "recording", file_name, saved_at_ms, size_bytes, duration_s }`, newest first, only regular video files directly inside the folders. `id` is opaque: the backend maps it back to a path only by listing the folders again, so it never names a file outside them. Also the desktop notification ("Replay saved" / "Recording saved" with the file name) for every `file-saved` | `list_recordings`, `open_recording`, `show_in_folder` |
| `thumbnails` | Previews of saved clips by gallery id: `recording_thumbnail(id)` is a `data:image/png` or `data:image/jpeg` base64 URL, or `null`; `thumbnail_support()` is `{ generator: "ffmpegthumbnailer" \| "ffmpeg" \| null, hint }`, the hint saying what to install. Desktop cache in `cache_dir()/thumbnails` (read only), Redline's in `app_cache_dir()/thumbnails`, pruned by `list_recordings` | `recording_thumbnail`, `thumbnail_support` |
| `global_shortcuts` | The GlobalShortcuts portal (D10), through `ashpd`, in a task started in `setup`. `ShortcutsStatus` is `{ state: "checking" }`, `{ state: "unavailable" }` (no portal: the commands of D5 stay), `{ state: "ready", version, can_configure, bindings: [{ arg, trigger }] }` (one entry per `ACTION_ARGS` argument, `trigger` the desktop's description of the keys or `null`) or `{ state: "failed", error }`, sent again as the `global-shortcuts` event on every change. `Activated` calls `actions::dispatch`; `global_shortcuts_configure` opens the desktop's dialog (portal version 2) | `global_shortcuts_status`, `global_shortcuts_configure` |
| `tuning` | The tuning service (`Tuning`, Tauri state). `tuning_status(gpu_id)` detects LACT and reads one GPU on a blocking thread: `TuningStatus { available, unavailable_reason, warning, lact_version, overdrive_enabled, reboot_pending, power: { current_w, min_w, max_w, default_w }, fan: { configurable, control_enabled, mode: "static" \| "curve", static_speed, curve: [{ temperature_c, speed_pct }], temperature_range: { min_c, max_c }, pmfw: { acoustic_limit, acoustic_target, target_temperature, minimum_pwm, zero_rpm, zero_rpm_threshold } }, profile, pending }`. Each PMFW value is `{ current, min, max }`. Speeds are percent. `power` and `fan` are `null` when `available` is false; any value LACT does not report is `null`, never zero. Changes are pending until the user answers: `set_power_limit(gpu_id, watts \| null)` (checked against `min_w`/`max_w`) returns a `PendingChange { gpu_id, what: "power_limit", deadline_ms }`, also emitted as `tuning-pending`; `tuning_confirm(gpu_id)` / `tuning_revert(gpu_id)` answer `"confirmed" \| "reverted" \| "expired"`, and every change ends with one `tuning-settled { gpu_id, outcome }`. One pending change at a time across every GPU; quitting reverts it. Overdrive (`tuning::overdrive`, D11): `prepare_enable_overdrive()` / `prepare_disable_overdrive()` return `OverdrivePlan { action: "enable" \| "disable", steps: [sentence], reboot_required: true, token, expires_ms }` and change nothing; `enable_overdrive(token)` / `disable_overdrive(token)` return `{ reboot_required: true }` and refuse any token but the latest plan's for the same action, unexpired; `TuningStatus.reboot_pending`. Profiles (`tuning/profiles.rs`): `list_profiles()` and `create_profile(name)` / `delete_profile(name)` return `Profiles { profiles, active, auto_switch }`; `activate_profile(gpu_id, name \| null)` returns a `PendingChange` with `what: "profile"`; every change to the profiles emits `tuning-profile` with the new `Profiles` | `tuning_status`, `set_power_limit`, `tuning_confirm`, `tuning_revert`, `prepare_enable_overdrive`, `enable_overdrive`, `prepare_disable_overdrive`, `disable_overdrive`, `list_profiles`, `create_profile`, `activate_profile`, `delete_profile` |
| `dependencies` | The external tools and whether each one is there, on a blocking thread: `dependency_status()` is `[{ id: "lact" \| "gpu_screen_recorder" \| "ffmpeg", name, installed, version, verified_version, warning, hint, daemon, can_install }]`, always in that order (`can_install`: the packaged helper and `pkexec` are both present). LACT is detected through the tuning service's `Lact` (`lact` on PATH, `system_info` on the socket; `version` only while `lactd` answers; `daemon: { running, socket_accessible }`, `null` for the other tools); GPU Screen Recorder as the native binary on PATH, otherwise the Flatpak (`Version:` of `flatpak info`); ffmpeg on PATH (`ffmpeg -version`, no verified version). A missing tool is `installed: false` with a `hint` that says what to do, never an error; a version other than the verified one fills `warning`. Every tool runs in the C locale and is killed after 5 s | `dependency_status` |
| `installer` | The guided install (D4): `install_dependency(name)` with `name` one of the `dependency_status` ids runs `pkexec /usr/share/redline/install-dependency <lact \| gpu-screen-recorder \| ffmpeg>` under the polkit action `com.fabiosm46.redline.install`, which allows only that program; Redline itself never runs as root. The helper's `progress <step>` lines become `dependency-install` events `{ id, stage: "progress", message }`; the end is one event with `stage` `succeeded`, `failed` (the helper's `error` line, otherwise its last stderr line) or `cancelled` (pkexec 126: the password prompt was dismissed, not an error), and the command resolves with that same event. One install at a time; a missing helper or `pkexec` fails before anything runs. Redline cannot stop the root helper, so the helper bounds itself: every step is killed after 30 minutes and apt waits at most 5 minutes for the dpkg lock; an open password dialog lasts until the user answers it (dismissing it is `cancelled`). A successful GPU Screen Recorder install makes the recorder detect it again | `install_dependency` |
| `overlay` | MangoHud, configured by Redline (D12). `overlay_status()` is `OverlayStatus { available, unavailable_reason, config: "absent" \| "managed" \| "user_config", settings, launch_hint }`, with `settings` the `OverlaySettings { preset: "off" \| "minimal" \| "standard" \| "full", position: "top-left" \| "top-right" \| "bottom-left" \| "bottom-right", metrics: ["fps" \| "frametime" \| "gpu_temp" \| "gpu_power" \| "gpu_core_clock" \| "vram" \| "cpu_temp" \| "ram"], toggle_key }` and `launch_hint` the Steam launch option `mangohud %command%`. `set_overlay(settings)` and `take_over_overlay_config()` return the new status; without MangoHud both fail with the same sentence as `unavailable_reason` (see "Overlay" below). `overlay::mangohud_present()` is the only detection of MangoHud | `overlay_status`, `set_overlay`, `take_over_overlay_config` |
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

**Tuning** (LACT, verified with 0.10.1): `tuning_status(gpu_id)` asks `lactd`, over
`/run/lactd.sock`, for `system_info` (version and the `amdgpu` overdrive bit), `list_devices`,
then `device_info` (`include_api_info: false`, for the fan flags) and `device_stats` (power cap
and fan) of the LACT device whose id ends with the GPU's PCI slot (`VENDOR:DEVICE-SUBVENDOR:SUBDEVICE-PCI_SLOT`),
and finally `list_profiles` (`include_state: false`) for the active profile. Each failure becomes
`available: false` with a sentence: no socket and no `lact` on PATH → install LACT; no socket
(or a stale one) with `lact` installed → start `lactd`; permission denied → the user is not in
the `admin_group` of `/etc/lact/config.yaml`; anything else → the daemon's own message. A GPU
LACT does not list is "not tunable", not an error. Another LACT version fills `warning` and
blocks nothing. On RDNA3/4 with overdrive off LACT exposes no firmware fan options and no
`ConfigurableFanControl` flag, so `fan.configurable` is false and every `pmfw` value is `null`,
while the power cap stays available.

Changes: the frontend asks for a change (`set_power_limit`) → the backend sends the setter to
`lactd` → `lactd` applies it at once and answers how many seconds it waits for a confirmation
(`apply_settings_timer`, 5 by default, in `/etc/lact/config.yaml`) → the backend records the
pending change with its deadline and emits `tuning-pending` → the user confirms or reverts and
the backend sends `confirm_pending_config` → `tuning-settled` with `confirmed` or `reverted`.
Without an answer `lactd` restores the previous values by itself (verified in 0.10.1's
`wait_config_confirm`): at the deadline the backend sends nothing and emits `expired`. lactd
holds one pending change for the whole daemon and refuses any setter while it exists, so Redline
refuses a second change on any GPU with a sentence, and names a change left pending by another
LACT client. Nothing confirms on its own; quitting the app with a change pending sends `revert`.

Profiles (verified in 0.10.1's `set_profile`, `set_current_profile`, `create_profile` and
`delete_profile`): a profile holds a configuration for every GPU, and `null` is LACT's default
profile. `set_profile` is **not** a pending change in lactd: it applies the profile to every GPU
at once and saves it, with no timer. So Redline holds the switch itself:
`activate_profile(gpu_id, name)` sends `set_profile { name, auto_switch: false }`, records a
`PendingChange { what: "profile" }` on `gpu_id` with a 15-second deadline and emits
`tuning-pending` and `tuning-profile`. `tuning_confirm` keeps it without a request;
`tuning_revert`, the deadline (`expired`) and quitting the app send `set_profile` with the
previous profile. A switch is refused while any change is pending and every change is refused
while a switch is pending, because lactd's revert of a pending setter writes into whichever
profile is active by then. `create_profile(name)` copies the active configuration (`base` is the
active profile, or `default`) and applies nothing; `delete_profile(name)` refuses the active
profile, which lactd would replace with the default one without a confirmation. Both are refused
while a change is pending, since lactd saves its whole configuration, unconfirmed values
included. Names are trimmed, non-empty, at most 64 characters, without control characters, not
`Default`, and unique ignoring case. Redline never turns LACT's automatic switching on, and
refuses a manual switch while LACT has it on.

**Fan** (`tuning::fan`, verified against lactd 0.10.1's `set_fan_control` handler and
`apply_config`): `set_fan(gpu_id, settings)` with `settings = { mode: "auto" | "static" |
"curve", static_speed?, curve?: [{ temperature_c, speed_pct }], pmfw?: { zero_rpm,
zero_rpm_threshold, target_temperature, acoustic_limit, acoustic_target, minimum_pwm } }` returns
a `PendingChange` with `what: "fan"`, confirmed or reverted like any other change. `auto` gives
the fan back to the firmware, `static` holds a speed (percent), `curve` follows the curve.
Fields that do not belong to the mode, and unknown fields, are refused. Before lactd sees
anything the backend checks the settings against the GPU, from `device_info` (the
`ConfigurableFanControl` and `HasPmfw` flags), `device_stats` (the firmware options and their
ranges) and, for a firmware fan, `gpu_od/fan_ctrl/fan_curve` in sysfs (read only: the fixed number
of curve points and their `OD_RANGE`). `tuning_status` reports the same rules as
`fan.curve_rules = { points (exact count, or null = any from 1), temperature: { min_c, max_c },
speed: { min_pct, max_pct }, whole_percent }`, `null` when neither a speed nor a curve can be
set. A curve has distinct temperatures and never slows down as the temperature rises. The four
firmware limits apply only in `auto` and zero RPM only outside `static` (lactd ignores them
elsewhere), each inside the range the firmware reports. A firmware fan with overdrive off has
none of this, and the refusal says to enable overdrive. The request is LACT's own
`set_fan_control`, never `set_gpu_config`; lactd replaces its stored firmware options as a whole,
so the backend sends the stored ones (`get_gpu_config`) with the new ones on top.

Overdrive (D11) is not a pending change: it is a kernel parameter for the next boot. The frontend
calls `prepare_enable_overdrive` → the backend reads `system_info` (the live overdrive bit and
`amdgpu_params_configurator`) and answers the steps lactd will take, `reboot_required: true` and
a single-use token valid for five minutes, changing nothing → the UI shows them and, on the
user's confirmation, calls `enable_overdrive(token)` → the backend checks the token (latest plan,
same action, not expired; any call spends it) and only then sends `enable_overdrive` to lactd,
waiting up to ten minutes while lactd regenerates the initramfs → `{ reboot_required: true }`, or
lactd's refusal as a sentence. `disable_overdrive` works the same way. `overdrive_enabled` keeps
the running kernel's value (false until the reboot) and `reboot_pending` turns true for the rest
of the session; lactd allows one change per daemon start, and its "already toggled" refusal also
sets `reboot_pending`. Enabling when the bit is already on, a system where LACT cannot set
kernel parameters (no configurator, as on NixOS) and an unknown configurator are refused at
`prepare`, before any token exists. A request that times out may still run in lactd (it marks
overdrive as toggled only when done), so no other overdrive request is sent for ten minutes.

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
Where the GlobalShortcuts portal exists, the same actions are also registered as the desktop's
global shortcuts (D10), and a key press calls the same `actions::dispatch` a second launch does.
Settings → Shortcuts then shows the keys the portal reports and a button for the desktop's own
dialog, with the commands behind a disclosure; without the portal it shows the commands.

**Overlay** (D12; keys checked against `MangoHud.conf` and `overlay_params.cpp` of MangoHud
0.6.9-1, the version Ubuntu 24.04 ships, and 0.8.4, the latest release; `mangohud` has no version
flag, so nothing checks the installed one): MangoHud is detected without running anything, by a
`MangoHud*.json` layer manifest in `vulkan/implicit_layer.d` under `XDG_DATA_DIRS`, `/usr/share` or
`/etc`, or by `mangohud` on PATH. `set_overlay` saves the settings, then renders them into
`MangoHud/MangoHud.conf` in `config_dir()` (temporary file + `rename`), first line
`# Managed by Redline`. With `user_config` (no marker on the first line, or anything that cannot
be read as Redline's: a folder, a dangling link) the settings are saved and the file stays as it
is. `take_over_overlay_config` writes the new file aside, hard-links the user's file to
`MangoHud.conf.redline-backup` (which fails if a backup exists, and then nothing changes), then
renames the new file into place. A `toggle_key` is key names joined by `+`: only the syntax is
checked, MangoHud ignores a name it does not know.

The file: `legacy_layout=0`, `position`, `toggle_hud`, `no_display` for `off`, then one line per
element in a fixed order, because MangoHud draws in file order. `metrics`, when not empty, replaces
the figures of the preset: `minimal` is FPS and frametime; `standard` adds GPU temperature, power
and core clock and CPU temperature; `full` adds VRAM and RAM; `off` starts hidden and its key shows
the `minimal` figures. MangoHud always draws the GPU load with a GPU figure and the CPU load with
the CPU temperature, and frametime needs the FPS line. Per-game files
(`MangoHud/<app>.conf`) and `MANGOHUD_CONFIG` take precedence over this file, and are not touched.

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
