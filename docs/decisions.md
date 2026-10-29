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
(for example "save replay") to bind to the desktop's custom shortcuts.

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
│ ● Redline   Home   Performance   Record   Settings       ⏺ Replay  │
├────────────────────────────────────────────────────────────────────┤
│                                                                    │
│   (section content)                                                │
│                                                                    │
├────────────────────────────────────────────────────────────────────┤
│ <GPU name> · 41 °C · 32 W · 772 MHz              Replay 60 s · ON  │
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
- **Status bar**: GPU temperature and power, plus a replay indicator. It is the only fixed
  element, so nobody has to open Home to know whether the replay is recording.
- **Several GPUs**: a selector in the top bar, shown only when there is more than one.

**Tray and lifetime.** Redline has a tray icon. Closing the window hides it to the tray: the app
keeps running, and with it the replay. Quit is an explicit action in the tray menu. This answers
the earlier open question of where the replay lives while the window is closed: in the app's own
process, not a separate service. Starting with the system is an option in Settings → General
(off by default), and so is starting minimised to the tray.

**Technical consequences**, for whoever implements it:

- The tray needs Tauri's `tray-icon` feature, and the `.deb` must depend on
  `libayatana-appindicator3-1`. On GNOME the icon shows only with an AppIndicator extension;
  Ubuntu enables one by default, other distributions may not, and Settings says so when the
  tray is unavailable.
- Autostart needs `tauri-plugin-autostart` (an XDG autostart entry on Linux).
- **Shortcuts under Wayland** depend on the GlobalShortcuts portal (D5). Where it exists,
  Redline registers the bindings through it; where it does not (GNOME before 48), the Shortcuts
  section shows each action's command to bind in the desktop's custom shortcuts. X11 can grab
  keys directly.
- **Choosing the screen**: with portal capture, changing the screen means discarding the saved
  portal session so the desktop asks again; recording a named monitor directly (`-w <monitor>`)
  goes through GPU Screen Recorder's KMS helper and may ask for a password. Settings offers both,
  portal first.

These two crates and the system package are approved by this decision; nothing else is.

Left out compared to AMD's software: the "Gaming" section with a game library (Steam and Lutris
own that on Linux), the built-in browser, streaming, driver notifications and display settings.

## Open

- How the overlay arrives: a MangoHud configuration managed by Redline, or an overlay of its own.
