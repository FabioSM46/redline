# Changelog

What each published version of Redline contains. The plan is in the
[roadmap](docs/roadmap.md), the release steps in [docs/release.md](docs/release.md).

## 0.2.1 — 2026-10-31

### Packages

- Every release now has Arch and Fedora packages next to the `.deb`.
- The guided install covers LACT on Arch and its derivatives, from the distribution, and on
  Fedora 43 and 44, from LACT's official package.
- Every release package is installed on clean systems before a draft is created: Ubuntu 24.04,
  Ubuntu 26.04, Debian 13, Arch and Fedora 44.

### Documentation

- The README shows the app and describes its state for users.

## 0.2.0 — 2026-10-30

### Overdrive

- One switch in Performance → Tuning → Overdrive turns overdrive on and off, each way with a
  confirmation that shows what will change and that a reboot is needed.
- Overdrive cannot be turned off while something depends on it: a stored clock or voltage value,
  or a fan curve or firmware fan setting, including one from the active LACT profile. The page
  lists each one, on every GPU, with a "Back to default" action that is confirmed like any change.

### Fixes

- The release build installs the appindicator headers it needs.

## 0.1.0 — 2026-10-30

The first release: a control panel for AMD GPUs on Linux, packaged as a `.deb`.

### Foundations (M0)

- Sensor reading from sysfs, a client for the LACT daemon and control of GPU Screen Recorder, in a
  core library with no dependency on Tauri, plus the `redline-probe` command-line tool.

### Performance (M1)

- App shell: Home, Performance, Record and Settings tabs, a status bar, and a tray icon that keeps
  the app running when the window is closed.
- Live figures and charts over a 1, 5 or 15 minute window, for every AMD GPU on the machine.
- A single instance, start with the system and start minimised to the tray.
- Light and dark themes.

### Recording (M2)

- Instant replay with a choice of audio tracks, on separate tracks in the saved file.
- Recording started and stopped from the app, with or without the replay.
- Gallery of saved clips with thumbnails, and a desktop notification for each saved file.
- Global shortcuts through the GlobalShortcuts portal where the desktop has it, desktop commands
  elsewhere.
- Settings for the screen to record, codec, quality and frame rate.

### Tuning (M3)

- Through LACT: fan control with a visual curve editor, power limit and profiles.
- Every change is confirmed in the app, or LACT reverts it.
- Overdrive enabled through LACT after an explicit confirmation; it takes effect after a reboot.
- Settings → Dependencies: status of the external tools and a guided install with one password
  prompt.

### Overlay and release (M4)

- In-game overlay through MangoHud: presets, position, toggle key and figures, with the
  `mangohud %command%` launch option to enable it per game. A `MangoHud.conf` of the user's is
  changed only after they agree, and is kept as a backup.
- Clocks and undervolt through LACT on RDNA GPUs with overdrive on: maximum core clock or core
  clock offset, maximum VRAM clock and a voltage offset at or below 0, checked against the GPU's
  ranges, confirmed like every change, with a reset to factory values. A confirmed value comes
  back after a reboot.
- MangoHud in Settings → Dependencies, with its guided install.
- The `.deb` installs `com.fabiosm46.redline.desktop`, so the desktop entry, the autostart entry
  and the portal app id are the same however Redline is started. An older autostart entry is
  replaced at startup.
- The package carries the licenses of the crates and npm packages compiled into it.
