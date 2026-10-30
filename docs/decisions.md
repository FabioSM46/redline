# Decisions

Every decision has a number (D1, D2, …) that code and issues can cite. A decision that changes is
updated here, with the date and the reason.

## D1 — Stack: Rust + Tauri 2, React + TypeScript frontend (2026-10-29)

The same stack as wow-don: a Rust backend with Tauri 2, a React + TypeScript frontend with Vite
and pnpm. The development pipeline (skills, CI, review) is ported from wow-don.

## D2 — Coordinate existing tools, do not rewrite them (2026-10-29)

- **Sensors**: read directly from sysfs (`/sys/class/drm/cardN/device` and its `hwmon`), with no
  permissions.
- **Tuning**: the LACT daemon (`lactd`), through its Unix socket (`/run/lactd.sock`, one JSON
  request per line). LACT is MIT and runs as root on our behalf.
- **Replay and recording**: GPU Screen Recorder as an external process, driven through options,
  signals and IPC. It is GPL-3.0: no code copied.
- **Overlay**: MangoHud, configured by Redline.

Rewriting a Vulkan overlay, video capture or a privileged daemon would take years and produce
worse copies of mature projects.

## D3 — Linux only (2026-10-29)

Redline reads the kernel's `amdgpu` driver and uses Linux-only tools. On Windows AMD's own
software already exists. CI and packages are Linux only.

## D4 — Distribution: `.deb`, guided setup, then an apt repository (2026-10-29)

The main package is a `.deb` built by Tauri. LACT and GPU Screen Recorder are not in Ubuntu's
repositories, so the `.deb` cannot install them through `Depends`.

1. **Now**: on startup the app checks its dependencies and offers to install the missing ones,
   asking for permissions with `pkexec`. Features without their dependency stay disabled.
2. **Later**: an apt repository with `redline`, `lact` and `gpu-screen-recorder`, so that
   `apt install redline` installs everything and updates arrive with `apt upgrade`.

Rejected:
- `.deb` install scripts that download packages: they hang without network and confuse apt;
- binaries bundled in our `.deb`: GPU Screen Recorder brings GPL obligations and needs FFmpeg 8,
  and LACT risks clashing with an existing installation;
- Redline as a Flatpak: the sandbox gets in the way of sysfs and of the `lactd` socket.

## D5 — Global shortcuts (2026-10-29)

Under Wayland global shortcuts go through the GlobalShortcuts portal. GNOME implements it from
version 48; Ubuntu 24.04 ships GNOME 46. Until the portal is there, Redline will offer commands
(for example "save replay") to bind to the desktop's custom shortcuts. Where the portal exists,
Redline registers the same actions through it (D10); the commands stay as the fallback.

## D6 — MIT license (2026-10-29)

Redline is MIT. GPU Screen Recorder (GPL-3.0) stays a separate process, so it does not bind
Redline's license.

## D7 — English only in the repository (2026-10-29)

Everything in the repository is written in English: code, comments, user-facing strings,
documentation, skills, issue templates, issues, PRs and commit messages.

## D8 — UI layout, tray and autostart (2026-10-29)

Four sections in a tab bar at the top, as in AMD's Windows software, plus a status bar that is
always visible. Four entries do not justify a sidebar, and people coming from Windows find their
way at once.

```
┌────────────────────────────────────────────────────────────────────┐
│ ● Redline   Home   Performance   Record   Settings                 │
├────────────────────────────────────────────────────────────────────┤
│                                                                    │
│   (section content)                                                │
│                                                                    │
├────────────────────────────────────────────────────────────────────┤
│ <GPU name> · 41 °C · 32 W · 772 MHz            ⏺ Replay 60 s · ON  │
└────────────────────────────────────────────────────────────────────┘
```

- **Home**: the GPU state in a few large figures (temperature, power, clock, usage, VRAM), the
  replay switch and the latest saved clips. It answers one question: "is everything fine?".
- **Performance**, in two parts:
  - **Metrics**: charts over time (1, 5 or 15 minute window) and the MangoHud overlay choice;
  - **Tuning**: fan, power limit and profiles, with LACT's mandatory confirmation. Undervolt and
    clocks sit under "Advanced".
- **Record**: replay (length, audio tracks, folder), recording, and below them the gallery of
  saved files.
- **Settings**:
  - **General**: theme, start with the system, start minimised to the tray;
  - **Capture**: which screen to record (monitor or portal), codec, quality, frame rate;
  - **Shortcuts**: every action (save replay, start/stop recording, toggle replay, ...) has a
    customisable key binding;
  - **Dependencies**: state of LACT and GPU Screen Recorder, with "Install" (D4);
  - **About**.
- **Status bar**: GPU temperature and power, plus the replay indicator, which is also a
  switch. It is the single place that shows the replay state (Home's replay switch controls the
  same state), so nobody has to open Home to know whether the replay is recording. The tab bar
  and the status bar are fixed; only the section content between them scrolls.
- **Several GPUs**: a selector in the top bar, shown only when there is more than one.

**Tray and lifetime.** Redline has a tray icon. Closing the window hides it to the tray: the app
keeps running, and with it the replay. Quit is an explicit action in the tray menu.

If no tray host can be registered at startup (for example GNOME without an AppIndicator
extension), close-to-tray is off for that session: closing the window quits the app, and "start
minimised to the tray" is ignored, so an autostarted Redline shows its window. The app never
runs with no window and no tray icon.

Redline runs as a **single instance**. A second launch (a menu click, or autostart racing a
manual start) does not start a second process, a second replay buffer or a second set of
shortcuts: it brings the existing window to the front. This answers
the earlier open question of where the replay lives while the window is closed: in the app's own
process, not a separate service. Starting with the system is an option in Settings → General
(off by default), and so is starting minimised to the tray.

**Technical consequences**, for whoever implements it:

- The tray needs Tauri's `tray-icon` feature, and the `.deb` must depend on
  `libayatana-appindicator3-1`. On GNOME the icon shows only with an AppIndicator extension;
  Ubuntu enables one by default, other distributions may not, and Settings says so when the
  tray is unavailable.
- Autostart needs `tauri-plugin-autostart` (an XDG autostart entry on Linux). The entry
  launches the app with the `--autostart` argument, which is how the app tells a login start
  from a manual one: "start minimised to the tray" applies only to a launch with `--autostart`.
  Whether the entry exists is read from the operating system every time, never stored in the
  settings file. The entry is `com.fabiosm46.redline.desktop`, the bundle identifier like the
  installed desktop file (D10); an update replaces one an older build wrote as `redline.desktop`.
- Single-instance handling needs `tauri-plugin-single-instance`.
- **Shortcuts under Wayland** depend on the GlobalShortcuts portal (D5). Where it exists,
  Redline registers the bindings through it; where it does not (GNOME before 48), the Shortcuts
  section shows each action's command to bind in the desktop's custom shortcuts. X11 can grab
  keys directly.
- **Choosing the screen**: with portal capture, changing the screen means discarding the saved
  portal session so the desktop asks again; recording a named monitor directly (`-w <monitor>`)
  goes through GPU Screen Recorder's KMS helper and may ask for a password. Settings offers both,
  portal first.

These three crates (the `tray-icon` feature of `tauri`, `tauri-plugin-autostart`,
`tauri-plugin-single-instance`) and the system package are approved by this decision; nothing
else is.

Left out compared to AMD's software: the "Gaming" section with a game library (Steam and Lutris
own that on Linux), the built-in browser, streaming, driver notifications and display settings.

## D9 — Charts with uPlot (2026-10-29)

The charts over time in Performance → Metrics use **uPlot** (`uplot`, MIT, no dependencies of
its own), pinned to an exact version. It draws time series on a canvas, which keeps a 15-minute
window of four charts updated every second cheap, and it is small enough not to weigh on the
bundle.

How it is used:

- A thin React wrapper (`src/charts/TimeChart.tsx`) owns the chart's lifetime: one instance per
  chart, fed with `setData`, resized with its container, destroyed on unmount, rebuilt only when
  the series layout or the theme changes.
- The conversion from `gpu_history` to uPlot columns is a pure, tested function
  (`src/metrics.ts`). A missing sensor is `null` and draws a gap, never a zero; a hole of more
  than a few seconds in the sampling also breaks the line.
- A canvas cannot read CSS variables: the colours are the tokens in `src/App.css`, read with
  `getComputedStyle` and read again when the colour scheme or the root's attributes change.

Rejected: a general-purpose chart or component library (heavier, SVG-based charts slow down with
hundreds of points per series, and AGENTS.md asks for no component library without a decision);
hand-written canvas or SVG charts (axes, cursor and scaling are exactly what uPlot already does
well).

`uplot` is the only package this decision approves; no other chart or component library.

## D10 — Global shortcuts through the portal with `ashpd` (2026-10-29)

Where the `org.freedesktop.portal.GlobalShortcuts` portal exists (GNOME 48 and later, KDE
Plasma), Redline registers its actions through it (#65). The client is **`ashpd`** (MIT), the
Rust wrapper of the XDG desktop portals, pinned to an exact version (`=0.13.13`) with
`default-features = false` and only the `global_shortcuts` and `async-io` features. The user
approved this dependency explicitly for #65. It builds on `zbus`, already in `Cargo.lock` through
`tauri-plugin-single-instance` with its `async-io` backend: `ashpd` uses the same backend, so the
lockfile gains one package (`ashpd` itself) and the single-instance plugin's `zbus` keeps the
backend it was tested with. async-io runs its own reactor, so the futures run on Tauri's tokio
runtime unchanged.

How the portal is used (`src-tauri/src/global_shortcuts.rs`):

- A task on Tauri's async runtime, started in `setup`, so startup never waits on the portal. A
  host app first tells the portal its id (`org.freedesktop.host.portal.Registry`, the bundle
  identifier), best effort: the registry is missing on older portals, and an app the portal
  already identified from its `.desktop` file is refused harmlessly. The id is the same either
  way: the `.deb` installs `/usr/share/applications/com.fabiosm46.redline.desktop`. Tauri's
  bundler names that file after `productName`, which also gives the Debian package name, so
  `scripts/deb-desktop-id.sh` renames it in the built package and the package stays `redline`.
- Availability is read at runtime: the interface's `version` property. No interface, no portal
  service or no session bus means "unavailable", and Settings → Shortcuts keeps the commands of
  D5. That is not an error and nothing is shown as one.
- One session for the app's whole life, kept in managed state and independent of the window:
  hiding it in the tray changes nothing. `BindShortcuts` registers one shortcut per entry of
  `ACTION_ARGS`, with the argument without its dashes as the id (`save-replay`): the desktop
  stores the user's keys under that id, so it never changes. Redline proposes no keys; the
  desktop's dialog picks them.
- `Activated` performs the action with the same code as a second launch
  (`actions::dispatch`), without showing the window. `ShortcutsChanged` updates the bindings
  Settings → Shortcuts shows (the `global-shortcuts` event).
- `ConfigureShortcuts` (portal version 2) opens the desktop's own dialog to change the keys;
  with version 1 the keys are changed in the desktop's settings.

Rejected: talking to the portal through GIO (`extern "C"`, as `desktop.rs`): sessions, request
objects, `Response` signals and signal subscriptions over raw GVariant pointers are much more
unsafe code than two one-shot calls; X11 key grabs (out of scope for #65).

`ashpd` is the only crate this decision approves.

## D11 — Overdrive through LACT, on explicit confirmation (2026-10-29)

Overdrive is the `0x4000` bit of the `amdgpu.ppfeaturemask` kernel parameter: RDNA3/4 need it
for the firmware fan curve, older GPUs for clocks and voltages. Until now the rule was "the app
explains it, it does not set it". Decided with the user when M3 was specified (overdrive option
B, #56): **Redline may ask `lactd` to enable or disable it, after an explicit confirmation**.

- lactd, which already runs as root, does the work with its own requests `enable_overdrive` /
  `disable_overdrive` (verified against `lact-daemon/src/system.rs` at `v0.10.1`): it writes or
  removes `/etc/modprobe.d/99-amdgpu-overdrive.conf` and regenerates the initramfs
  (`update-initramfs`, `mkinitcpio` or `dracut`, by distribution), or uses `rpm-ostree kargs` on
  Fedora Atomic. `system_info.amdgpu_params_configurator` says which; without one (NixOS) LACT
  cannot do it and Redline says how to set the parameter by hand.
- Redline never writes system files, never edits kernel parameters itself, never runs as root and
  never reboots the machine.
- The confirmation is a handshake in the backend, not only a dialog: `prepare_enable_overdrive`
  (or `prepare_disable_overdrive`) returns the steps lactd will take, that a reboot is needed and
  a single-use token valid for five minutes; `enable_overdrive(token)` (or
  `disable_overdrive(token)`) refuses any call without the token of the latest plan for the same
  action. The UI shows the plan and sends the token only when the user confirms.
- It is not a pending change: lactd has nothing to confirm or revert, and the running kernel keeps
  its value until the reboot. `tuning_status.overdrive_enabled` keeps reading the live value;
  `reboot_pending` says it was changed in this session. lactd allows one change per daemon start.

Rejected: writing the modprobe file or the boot arguments from Redline (it would need root);
keeping overdrive manual-only (the fan curve, the main M3 feature on RDNA3/4, would need the user
to edit boot files by hand).

## Open

- How the overlay arrives: a MangoHud configuration managed by Redline, or an overlay of its own.
