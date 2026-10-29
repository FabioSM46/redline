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

## Open

- How the overlay arrives: a MangoHud configuration managed by Redline, or an overlay of its own.
- Where the replay lives while the app is closed: a systemd user service or a process of the app.
