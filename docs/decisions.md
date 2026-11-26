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

Update (2026-10-31): packages are published on GitHub Releases only. Next to the `.deb`, each
release has an Arch package and a Fedora package, built in containers of those systems because a
package has to link the libraries its system ships. The apt repository and the AUR are dropped:
nothing is published anywhere else. The guided install takes LACT from the distribution on Arch
and from LACT's official package, with a pinned checksum, on Fedora.

Update (2026-11-16): why Redline is in no repository, asked again after the public launch. For the
distribution repositories the answer is "not yet"; for Flathub it is "no", unless one of the two
things named below changes.

- **Official distribution repositories** are not the author's to enter: a distribution's
  maintainers choose and package what they trust, and a project a few weeks old is not picked up.
  Adoption by a maintainer is the realistic way in.
- **The AUR, COPR, a PPA or an apt repository of our own** stay dropped. Each is a second place
  to keep in sync and to check, kept by one person, and none of them is a review: anyone can
  publish there, so they would not give the assurance people ask a repository for.
- **Flathub** is not feasible today, for more than the sandbox. Its requirements for app authors,
  read on this date, exclude Redline as it is now on three counts:
  - *Generative AI policy*. Two separate rules, quoted from
    [Flathub's requirements](https://docs.flathub.org/docs/for-app-authors/requirements)
    ("Generative AI policy"); the rest of this entry is our reading of them.
    The packaging: "Flathub manifests must not contain AI-generated or AI-assisted content", and
    AI tools or agents must not open or automate a submission. This one has an answer: the
    maintainer writes the manifest and the submission by hand.
    The application: generated code, documentation and packaging must be disclosed, "Disclosure
    does not create a presumption of acceptance", and reviewers may reject on the extent or role
    of the generated material. This one has no answer today: Redline's code is written with AI
    assistance, as `AGENTS.md` describes, and a hand-written manifest does not change that.
  - *Host-dependent applications*: an application that relies on host components for its core
    function, or that is a thin wrapper around other tools, is not accepted, exceptions aside.
    Tuning, replay and the overlay all rest on `lactd`, GPU Screen Recorder and MangoHud
    installed outside the package (D2).
  - *Maturity*: a meaningful history of development and evidence of real-world use are required,
    and an application that has existed only for a very short time is generally not accepted.
- **What "the sandbox gets in the way" means.** A Flatpak sees its own files and the few services
  it was granted, not the host's programs or sockets, and Redline does almost only things that
  are outside: it talks to `/run/lactd.sock`, starts GPU Screen Recorder as a host process, reads
  and writes MangoHud's configuration and looks for its layer in the system folders, and asks for
  a password with `pkexec` to install a missing tool. Each can be granted one by one, as LACT's
  own Flathub package shows (the `lactd` socket, systemd, and `org.freedesktop.Flatpak`, which
  runs commands outside the sandbox). Added up, the package asks to leave its sandbox: more work,
  the guided install lost (`pkexec` does not work from inside), and an isolation that exists only
  on paper. Not verified: whether sysfs is readable from the sandbox without an extra permission,
  and whether the tray icon works for a Tauri application in a Flatpak.

In their place: the source is public and MIT, Redline never runs as root, the packages are built
by public CI from the tagged commit and installed on clean systems before a release, and every
release since 0.4.0 has `SHA256SUMS` (integrity, not a signature).

What would change the answer: for the distribution repositories, a history of releases and users,
which only time gives. For Flathub, age settles the maturity rule alone; the rule on generated
application code stands until Flathub changes it or Redline's code is no longer written with AI
assistance, and the host-dependency rule stands as long as D2 does.

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
│ <GPU name> · 41 °C · 32 W · 772 MHz    ⏺ Replay 60 s · ON  v0.3.0  │
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
  - **Dependencies**: state of LACT, GPU Screen Recorder, ffmpeg and MangoHud, with "Install" (D4);
  - **About**.
- **Status bar**: GPU temperature and power, plus the replay indicator, which is also a
  switch. It is the single place that shows the replay state (Home's replay switch controls the
  same state), so nobody has to open Home to know whether the replay is recording. The last item
  on the right is the installed version (`v0.3.0`), muted and in tabular figures: plain text, not
  focusable, and absent while the update status is loading or unreadable. When a newer release
  that is not dismissed exists (D14) the same place becomes a button that names both versions
  (`v0.3.0 → 0.4.0`) and opens the release page; there is no separate update notice and no
  dismiss control in the bar. The tab bar and the status bar are fixed; only the section content
  between them scrolls.

  Update (2026-11-16): the version item replaces the "Update … available" button and its `×`,
  which appeared only when an update existed. The installed version is now always in view, and
  dismissing a version is done in Settings → General only.
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
- **One switch, and a guard on the way off** (#119). Overdrive is never disabled while something
  needs it, because the fans would stop being controlled. `prepare_disable_overdrive` lists what
  depends on it on every `amdgpu` GPU, read from what lactd stores (`get_gpu_config`, which is the
  active profile's copy when one is active): clock or voltage values, and on a firmware fan
  (`HasPmfw`) a curve or fixed speed in use or firmware fan options. A plan with a list has no token,
  and `disable_overdrive` reads the list again, refusing if it is not empty, if it cannot be read
  or while a change is pending. A saved profile that is not active does not block, and the plan
  says so.
- **Default values are never restored silently.** Each item has a "Back to default" action:
  `reset_clocks`, or `reset_fan`, which is `set_fan_control` with the fan automatic and the firmware
  options cleared (`set_fan` keeps the stored ones). Each is a pending change the user keeps or
  reverts, and under an active profile it changes that profile's copy, which the action says. A
  reset inside the disable request would confirm on the user's behalf.
- **From the disable until the reboot, nothing that needs overdrive can be set.** The running
  kernel keeps it, so `set_fan` on a firmware fan (other than back to automatic), `set_clocks` and `activate_profile`
  would be accepted and left stored for a boot without it. Only an answer that proves nothing
  changed lifts the stop: lactd unreachable, or its refusals that come before it touches anything,
  matched whole. A timeout, "already toggled", an initramfs failure or an unreadable answer all
  keep it; an accepted enable and a new boot end it.
- **The stop outlives a restart of Redline** (#127), because the risk lasts until the reboot. It
  is stored, not derived: `overdrive-stop` in `app_data_dir()` holds the kernel's boot id
  (`/proc/sys/kernel/random/boot_id`), written before the request leaves; if it cannot be written
  the disable is refused. At start another boot's id removes the file, and a file or boot id that
  cannot be read keeps the stop. The running bit against LACT's modprobe file cannot do this: a
  missing file also describes overdrive set by hand, and `rpm-ostree` has no such file.
  `reboot_pending` stays a session value: after a restart lactd's "already toggled" sets it again.
- Known limits: lactd holds its one pending change in memory with no request to read it, so one
  left by the LACT app is invisible here; and LACT's automatic profile switching can activate a
  saved profile later, which the plan says.

Rejected: writing the modprobe file or the boot arguments from Redline (it would need root);
keeping overdrive manual-only (the fan curve, the main M3 feature on RDNA3/4, would need the user
to edit boot files by hand); resetting the dependent settings inside the disable request (see
above).

## D12 — The overlay is MangoHud, configured through its user config file (2026-10-30)

This closes the open point on how the overlay arrives (#85). Redline has no overlay of its own:
it renders its settings into MangoHud's user config file, `MangoHud/MangoHud.conf` in
`$XDG_CONFIG_HOME` (otherwise `~/.config`).

- **The player enables MangoHud per game.** MangoHud's implicit layer loads only with `MANGOHUD=1`
  in the game's environment, so the file changes nothing by itself. Redline shows the Steam launch
  option `mangohud %command%` and does not set `MANGOHUD=1` globally: no `environment.d`, no
  `/etc/environment`, no edits to Steam's configuration. A global switch would inject a layer into
  every Vulkan application of the session.
- **The file has an owner.** Redline's file starts with the line `# Managed by Redline`. A file
  without it is the user's and is never overwritten silently: `set_overlay` saves the settings and
  leaves the file alone until the user agrees to a take-over, which keeps the file as
  `MangoHud.conf.redline-backup` and never replaces an existing backup.
- **The file lists everything, with `legacy_layout=0`.** Without that key MangoHud draws its
  defaults (GPU, CPU, FPS and the frame graph) whatever else is written, so a preset could only
  add figures. `gpu_stats`, `cpu_stats` and `legacy_layout` are therefore written although no
  setting exposes them: `gpu_temp`, `gpu_power` and `gpu_core_clock` are drawn inside `gpu_stats`
  and `cpu_temp` inside `cpu_stats`.

Rejected: an overlay of Redline's own (a Vulkan layer is a project of its own, and MangoHud
already does it); `MANGOHUD_CONFIG` or a global `MANGOHUD=1` (they reach every application, and a
variable set by the player would be overridden without warning).

## D13 — Clocks and voltages: RDNA only, undervolt only, overdrive required (2026-10-30)

Clocks and voltages moved out of M3 when M3 was specified; they arrive before the first published
`.deb` (#102, #103). Redline offers what a player uses and leaves the rest to LACT.

- **Four controls, from the overdrive table.** Maximum core clock (RDNA1–3), core clock offset
  (RDNA4), maximum VRAM clock (discrete GPUs) and voltage offset. The table decides which ones a
  GPU has, never a list of GPU names. Minimum clocks, V/F curve points, power states, the
  performance level and power profile modes stay in LACT.
- **RDNA only.** A GCN table has per-level clocks and voltages, a different and riskier interface
  for GPUs Redline cannot test: `clocks` says the family is not supported.
- **Undervolt only.** The backend refuses a voltage offset above 0 on every GPU, whatever range
  the firmware reports. Raising the voltage is the one setting here that can damage a GPU over
  time, and RDNA2 gives no range, so lactd would check nothing. Without a range Redline also
  refuses anything below -450 mV, the widest firmware range known (RX 7900 XT): a value that
  hangs the GPU cannot be answered in the confirmation, so a typo must not reach it. This floor
  is a sanity limit, not a safe value, and `clocks` keeps reporting no range.
- **Overdrive required.** Without it the kernel takes no clock or voltage value: the status and
  the setter give the same sentence, which points to overdrive (D11).
- **Checked before lactd.** Every value is checked against the current table, because lactd checks
  no core clock offset and no voltage offset without a range, and a refused value fails the whole
  batch without saying which one. A clock with no firmware range is refused: there is nothing to
  check it against.
- **One change, always pending.** The values go in one `batch_set_clocks_value`, so they apply and
  revert together, and the confirmation is the usual one. `set_gpu_config` is never sent: it replaces
  the whole GPU configuration, fan and power limit included, with a copy that can be out of date. Redline does not set the
  performance level either: lactd's `apply_config` handles it, including the GPUs that need
  `manual`.
- **A confirmed value persists.** lactd stores it and applies it at every start
  (`apply_current_config`), so an undervolt that passes the confirmation and fails later comes back
  after every reboot. Keeping a value only until the next reboot would mean changing how lactd
  stores its configuration, which Redline does not do. The way out is the reset: `reset_clocks`
  checks no table and works with overdrive off, and it clears the stored values. If the machine
  crashes before the app can be reached, stop lactd from a console
  (`sudo systemctl stop lactd`, or `systemd.mask=lactd.service` on the kernel command line for one
  boot), or remove the clock values of the GPU from `/etc/lact/config.yaml`, then restart it.
- **A reset is sent alone**, through `set_clocks_value`: at LACT 0.10.1 only that request calls
  `reset_clocks()`. It runs before lactd checks for a pending change, so while another program
  (the LACT app) holds one, lactd resets the table and then refuses. Redline reports the refusal;
  the stored values take over again when that change settles or at the next lactd start.

Rejected: offering the V/F curve or minimum clocks (Adrenalin's depth, not what Redline is for,
D8); a positive voltage offset behind a second warning (a confirmation does not undo wear); a
per-GPU list of supported models (the table already says it, and a list goes stale); applying
values on Redline's start instead of confirming them in lactd (Redline would have to run for the
tuning to hold, and it would be a second store of the same values).

## D14 — Updates: Redline notifies and links, it does not install (2026-11-01)

Redline finds out that a newer release is published and shows the release page. Installing stays
the user's act, through their package manager.

The packages install as root, Redline never runs as root, `install-dependency` accepts only fixed
tool names (never a URL, a path or a command), and the packages are unsigned: an in-app install
would run an unverified download as root.

- **The check is `curl` as an external process**, following D2 and the rule that a missing tool is
  a disabled feature. It is the app's first outbound HTTP call, and no HTTP or TLS library is
  compiled in: `curl` costs a package dependency, a crate would cost a TLS stack. `curl` is a
  declared dependency of the `.deb`, Arch and Fedora packages.
- **One fixed request**: `https://api.github.com/repos/FabioSM46/redline/releases/latest`, which
  excludes drafts and prereleases, shortly after startup and every 24 hours while the app runs.
- **GitHub sees the user's IP address about once a day**, and nothing else is sent. It can be
  twice on a day with a retry: `unreachable` means no usable answer reached Redline, not that
  nothing reached GitHub, and a timeout or a broken connection can come after the request left.
  `check_for_updates` (on by default) turns it off: no request is made, no retry waits, and what
  the checks found is forgotten, so the status bar goes back to the plain installed version and
  nothing about an update outlives the opt-out. A check still running at that
  moment is dropped. The README says so.
- **Only a strictly newer stable `major.minor.patch` is an update.** An equal or older version, a
  build newer than the release or an unreadable tag or answer reports nothing.
- **Nothing read from the network is opened or run.** `open_latest_release` opens a constant, not
  the answer's `html_url`; only `tag_name` is read, and only as three numbers.
- **A failed check is an outcome, never a dialog or a retry loop**: offline, rate limited (60
  unauthenticated requests per hour per IP) or a bad answer shows in `update_status`, and the
  next try is the next round. **One exception, changed on purpose after the first version of this
  decision:** a check that ends as `unreachable` gets one more check about five minutes later, so
  an autostarted app that checks before the network is up does not stay silent for 24 hours of
  running time. It is a single retry, not a loop: a failed retry gets no other, and the 24-hour
  rounds do not move. `refused`, `bad_response` and `curl_missing` are never retried early: a rate
  limit must be left alone, and the others will not change in five minutes.
- **A dismissal holds one version**: a later version reports again.
- **Where it shows** (changed 2026-11-16): the status bar's version item (D8) becomes a button
  that names the installed and the newer version and opens the release page; a dismissed version
  leaves it as plain text. Settings → General has the row with the outcome of the last check and
  the only control that dismisses a version. A failed check shows in General alone.

Rejected: the Tauri updater plugin and any in-app install (see above); a crate for HTTP or for the
comparison (`reqwest` and its TLS stack, `semver` for three numbers); following prereleases or a
channel selector; notifying from the tray or the desktop; backoff, several retries or waiting on
a network-online signal.

## D15 — Game launch options: Redline composes, the player pastes (2026-11-16)

Redline does one thing for a game: it composes a launch option and shows it. Starting the game
with it stays the player's act, in the launcher.

- **One string, composed from the player's choices.** The choices are the overlay (D12), frame
  generation, the upscaler upgrade and the upscaler watermark. Redline turns them into a single
  launch option (variables first, then the `mangohud` wrapper, then `%command%`) and shows it
  with a Copy button, to be pasted into the launcher. With only the overlay chosen it is the
  `mangohud %command%` of D12.
- **Redline never edits the launcher's configuration and never sets a variable globally.** No
  write to Steam's or Lutris's files, no `environment.d`, no `/etc/environment`. The reasoning is
  D12's: a global variable reaches every application of the session, and a value the player set
  would be overridden without warning. A pasted line is visible where it acts and is removed the
  same way.
- **The choices are global; Redline keeps no per-game state.** There is no list of games, no
  per-game profile and no detection of the running game. A game that needs a different line gets
  it from the player: change the choices, copy, paste.
- **The FPS limit is not part of the string.** It is a MangoHud key (`fps_limit`) in the global
  `MangoHud.conf` that Redline manages (D12), so it applies to every game started with MangoHud
  and to none started without it.
- **A missing tool is a disabled choice, not an error**: a choice whose tool is not installed
  composes nothing and says what to install.

- **The upscaler upgrade is a fourth choice (#170).** The watermark alone shows nothing unless
  Proton's upscaler upgrade is on, and until this choice existed the player had to add that
  variable by hand. The switch is off by default and, like the other three, only composes text
  the player pastes: `PROTON_FSR4_UPGRADE=1`, written before the watermark's variable. It changes
  how a game renders, so it stays a choice of its own instead of following the watermark.
  Redline still keeps no per-game state and sets nothing globally, does not detect whether a
  game or a GPU supports the upgrade, and pins no upscaler version. Which Proton builds read the
  variable is in `docs/architecture.md` ("Launch options").

The code shipped with version 0.4.0 (#153 to #156, and #170 for the upscaler upgrade).

Rejected / not possible:

- **Upscaling through Gamescope.** It would be a fifth external tool, after LACT, GPU Screen
  Recorder, ffmpeg and MangoHud, and one that the reference distribution does not reliably
  package; the overlay must then be started differently
  (`--mangoapp` instead of the `mangohud` wrapper); and what it offers is a spatial upscaler,
  below what games already ship. Deferred on 2026-11-16 and refused on 2026-11-26: nobody asked
  for it again, and the upscaler upgrade (#170) already serves the games that ship an upscaler.
- **An anti-lag switch.** `VK_AMD_anti_lag` is a Vulkan extension the game itself must call:
  there is no global switch to offer. Up to Mesa 26.2 it comes from an implicit Mesa layer
  (`VK_LAYER_MESA_anti_lag`); from Mesa 26.3 RADV implements it natively and the layer is gone
  (checked on 2026-11-16 against Mesa 26.2.6 and 26.3).
- **A status-bar badge for the running game's upscaler.** Reading the memory maps of the
  player's own processes only proves that a library is loaded, not that the upscaler is active.
  Proton's own watermark answers the question reliably, which is why the watermark is a choice.
  A probe spike was left open on 2026-11-16 and dropped on 2026-11-26: a badge that is always in
  view would be read as "active", which is the one thing it cannot know.
- **The upscaler version in the overlay.** MangoHud's `fsr` key works only inside Gamescope and
  reports Gamescope's own upscaler, not the game's.
- **Detecting a game's frame generation.** Nothing exposes it: no sysfs file, no driver
  statistic, no overlay key.
- **HDR colour accuracy and saturation.** Display colour belongs to the Wayland compositor, as
  display settings belong to the desktop (`docs/context.md`).
