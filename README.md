# Redline

Redline is for people who play on Linux with an **AMD GPU** and want one control panel instead of
several separate tools: one app to watch temperatures, clocks and power draw, tune fans and power
limits, and save a replay of the last minute of gameplay with a shortcut. If you already run LACT,
MangoHud and GPU Screen Recorder side by side and are happy with them, you do not need Redline:
it coordinates those same tools and adds no capability beyond what they already provide.

The inspiration is the software AMD ships only for Windows; Redline keeps the features people
actually use and leaves the rest out.

> Independent project, not affiliated with or endorsed by AMD.

![Redline's home page with live GPU figures and instant replay](docs/screenshots/home.png)

![Redline's performance charts](docs/screenshots/performance.png)

## Status

Everything listed below works in the current release. Download it from
[GitHub Releases](https://github.com/FabioSM46/redline/releases); the
[changelog](CHANGELOG.md) says what each version added and the [roadmap](docs/roadmap.md) what
comes next.

## Features

| Feature | How |
|---|---|
| Live statistics and charts | read directly from sysfs (`/sys/class/drm/cardN/device`), no permissions needed |
| In-game overlay (FPS, frametime, temperatures) | [MangoHud](https://github.com/flightlessmango/MangoHud), configured by Redline |
| Tuning: fans, power limit, profiles, clocks and undervolt | the [LACT](https://github.com/ilya-zlobintsev/LACT) daemon (`lactd`) through its socket |
| Instant replay and recording, with audio on separate tracks | [GPU Screen Recorder](https://git.dec05eba.com/gpu-screen-recorder/about/) as an external process, with hardware encoding |
| FPS limit and a composed launch option for games (overlay, frame generation, upscaler upgrade, upscaler watermark) | MangoHud's limiter and one line you paste into the launcher; Redline changes no launcher setting |
| Notice of a newer release | `curl` asks GitHub for the latest release about once a day; Redline links to it and installs nothing |

Redline does not rewrite these tools: it coordinates them behind a single interface. The why is in
[docs/context.md](docs/context.md), the choices in [docs/decisions.md](docs/decisions.md).

## Install

Download the package for your system from
[GitHub Releases](https://github.com/FabioSM46/redline/releases), then install it from the folder
it is in:

```bash
sudo apt install ./redline_<version>_amd64.deb            # Ubuntu, Debian, Linux Mint
sudo pacman -U ./redline-<version>-1-x86_64.pkg.tar.zst   # Arch, CachyOS
sudo dnf install ./redline-<version>-1.fc44.x86_64.rpm    # Fedora
```

To check the download first, save `SHA256SUMS` from the same release into that folder and run:

```bash
sha256sum -c --ignore-missing SHA256SUMS
```

It prints `OK` next to the package when the file is the one on the release page, and `FAILED`
when it is not: download it again and do not install it. The packages and `SHA256SUMS` are not
signed, so the checksum proves that the download is intact and matches the release page, which the
public CI wrote; it does not prove who built the package.

Open Redline and go to **Settings →
Dependencies**: it shows whether LACT, GPU Screen Recorder, ffmpeg and MangoHud are installed and
offers to install what is missing, with one password prompt (`pkexec`). A missing tool only
disables its feature, and the app says what to do.

**What a package sets up**: the `redline` program, the menu entry
`/usr/share/applications/com.fabiosm46.redline.desktop`, and the helper and polkit action behind
the guided install.

**What it does not**: it installs none of the four tools by itself, enables nothing at login
(starting with the system is off until you turn it on in Settings → General; the entry is then
`~/.config/autostart/com.fabiosm46.redline.desktop`), changes no kernel parameter, and adds no
package repository, so a new version is a new `.deb` installed the same way. The package is not
signed.

### Supported systems

| System | State |
|---|---|
| Ubuntu 24.04 and 26.04, Debian 13 | Supported: package and guided install of all four tools |
| Other releases based on Ubuntu 24.04 (Linux Mint, Pop!_OS, ...) | The guided install takes the Ubuntu 24.04 LACT package. The package and LACT were installed in a Linux Mint 22.3 container; not tried on a desktop |
| Arch and derivatives (CachyOS, ...) | Package and guided install of all four tools; LACT comes from the distribution |
| Fedora 44 | Package and guided install of all four tools |
| Windows, macOS | Not supported: Redline is Linux only |

**Redline itself is not available as a Flatpak** or in any repository: the reasons are in D4 of
[docs/decisions.md](docs/decisions.md). GPU Screen Recorder is a separate program: its guided install takes the Flatpak
from Flathub, and first installs Flatpak and adds the Flathub remote if they are missing.

### Requirements

- x86-64 Linux with an AMD GPU on the `amdgpu` driver (every recent Radeon)
- Wayland or X11
- For tuning: LACT with the `lactd` daemon running (verified with 0.10.1, the version the guided
  install takes; another version works with a warning)
- For replay and recording: GPU Screen Recorder (verified with 6.1.6; another version works
  with a warning)
- For previews of saved clips: ffmpeg
- For the in-game overlay: MangoHud
- For the update check: `curl`, which the packages depend on

From a source checkout, `scripts/install-deps.sh` runs the same helper as the guided install:
LACT (the official package of the pinned version above), GPU
Screen Recorder (from Flathub), and ffmpeg and MangoHud (apt, dnf or pacman). LACT is the official
package on Ubuntu, Debian and Fedora, and the distribution's on Arch and its derivatives:

```bash
sudo bash scripts/install-deps.sh
```

### Updates

Redline looks for a newer release shortly after it starts and every 24 hours while it runs, with
one request to `api.github.com`, so GitHub sees your IP address about once a day. If a check gets
no answer, Redline tries once more about five minutes later; a check can reach GitHub and still
get no answer, for example when the connection breaks halfway, so on such a day GitHub can see
your IP address twice. Nothing else is sent: no identifier, no statistics. A new version is announced with a link to its release page; download
and install it as before, Redline does not install it for you. The check can be turned off in
settings (`check_for_updates`), and then no request is made.

### Overdrive

The fan curve on RDNA3/4 GPUs needs overdrive, the `0x4000` bit of the `amdgpu.ppfeaturemask`
kernel parameter. In **Performance → Tuning → Overdrive** Redline shows what LACT will change (it
sets the parameter for the next boot and updates the boot files) and asks it to go ahead only
after you confirm. The same switch turns it off, and refuses while a fan curve, a firmware fan
setting or a stored clock value still needs it, on any GPU and including the active profile: each
one has a **Back to default** action that you keep or revert. Redline never edits those files
itself and never reboots: the change takes effect after you reboot.

### Overlay

The overlay is MangoHud, and **Performance → Metrics → Overlay** chooses what it shows. It appears
only in the games you start with MangoHud. In Steam, paste `mangohud %command%` into the game's
Launch options (the page has a Copy button). In Lutris, open the game's Configure → System options
and turn on **FPS counter (MangoHud)**, or enter `mangohud`, without `%command%`, as the Command
prefix, under Advanced; under Wine the overlay is reliable only on Vulkan, which normally means
DXVK. A `MangoHud.conf` of your own stays untouched until you agree, in the same page, to let
Redline manage it.

The same page has an **FPS limit**, which applies to every game started with MangoHud, and four
switches that compose the launch option: the overlay, frame generation, the upscaler upgrade and
the upscaler watermark.
Frame generation is the [afmf-linux](https://github.com/serialexperimentslainnnn/afmf-linux) layer
(checked with its release 1.3.1): install it from that project, and the switch turns on by itself.
The upgrade is `PROTON_FSR4_UPGRADE=1`, read by GE-Proton 10-9 or later and proton-cachyos
10.0-20250623 or later, not by Valve's Proton: it needs a game with FSR 3.1 or later and changes
how the game renders (proton-cachyos 11.0-20261001 or later upgrades by itself). The watermark is
`PROTON_FSR4_INDICATOR=1`, read by GE-Proton 10-26 or later and proton-cachyos 10.0-20250905 or
later, and it shows something only while the upgrade is active. Redline only writes the line for
you to paste.
In Lutris the line goes in parts, under the game's Configure → System options: each `NAME=1` pair
becomes an entry in Environment variables (key `NAME`, value `1`), `mangohud` is the overlay as
above, and `%command%` is left out; the page shows the same steps under **In Lutris**.

## Stack

- **Backend**: Rust (edition 2024), [Tauri 2](https://tauri.app)
- **Frontend**: React 19 + TypeScript, Vite, pnpm
- **System logic**: the `redline-core` crate, with no dependency on Tauri

## Repository layout

```
redline/
├── src/                    # React + TypeScript frontend
├── src-tauri/              # Rust workspace
│   ├── src/                # Tauri app: commands exposed to the frontend
│   ├── crates/
│   │   ├── redline-core/   # sensors, lactd client, GPU Screen Recorder control
│   │   └── redline-probe/  # CLI to try redline-core pieces without the UI
│   └── tauri.conf.json
├── docs/
│   ├── context.md          # why it exists, what it takes and what it leaves out
│   ├── decisions.md        # decision log
│   ├── architecture.md     # modules and flows
│   ├── roadmap.md          # milestones
│   └── release.md          # release checklist
├── packaging/              # Arch and Fedora package definitions
├── scripts/                # install-deps.sh, pipeline helpers and automation tests
├── .github/                # CI, DeepSeek review, labeler, issue templates
├── .claude/skills/         # agent skills (/dev-issue, /process-pr, /develop-iteration, /scrum-master)
├── AGENTS.md               # rules for AI agents and the pipeline
├── CHANGELOG.md            # what each release contains
└── package.json
```

## Development

### Toolchain

| Tool | Version | Used for |
| ----- | ------------------------------- | ------------------ |
| Rust  | pinned in `src-tauri/rust-toolchain.toml` (rustfmt + clippy) | backend |
| Node  | 22.18 or later                  | frontend |
| pnpm  | 10                              | frontend |
| gh    | 2.18.0 or later                 | pipeline scripts |
| jq    | any recent                      | pipeline scripts, `scripts/test/` |
| python3 | 3.12 recommended              | DeepSeek reviewer tests |

System libraries for Tauri, on Ubuntu/Debian:

```bash
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libayatana-appindicator3-dev libxdo-dev libssl-dev librsvg2-dev
```

### Commands

```bash
pnpm install          # frontend dependencies
pnpm tauri dev        # run the app in development with hot reload
pnpm tauri build      # release build and .deb package
```

The `.deb` ends up in `src-tauri/target/release/bundle/deb/`. A plain `pnpm tauri build` still
produces `redline.desktop`: run `bash scripts/deb-desktop-id.sh` afterwards, as the release does.
It renames the file inside the package to the bundle identifier,
`com.fabiosm46.redline.desktop`, which Tauri's bundler cannot do by itself.

Quick checks:

```bash
pnpm build                                        # frontend type-check and build
pnpm test                                         # frontend tests (node --test)
cargo test --manifest-path src-tauri/Cargo.toml   # Rust workspace tests
```

### Trying the pieces without the UI

`redline-probe` uses `redline-core` directly, from the `src-tauri/` directory:

```bash
cargo run -p redline-probe -- gpus               # AMD GPUs with their id and name
cargo run -p redline-probe -- sensors --watch     # sensors, one line per second (--gpu <id>)
cargo run -p redline-probe -- lact stats          # statistics from lactd (also: system, devices, info, config, profiles)
cargo run -p redline-probe -- audio               # audio sources for GPU Screen Recorder
# 60 s replay, game audio and microphone on two separate tracks
cargo run -p redline-probe -- replay --track default_output --track default_input
cargo run -p redline-probe -- tracks <file>.mkv   # tracks of a saved file
```

In the replay: `s` + Enter saves the last seconds, `r` + Enter starts or stops a recording,
`q` + Enter quits. Files go to `~/Videos/Redline/`. The replay needs an interactive terminal:
when stdin closes, the probe stops the recorder and exits.

### Workflow

The integration branch is `develop` (the default branch); `main` only receives releases. Work
goes issue → PR to `develop` → CI and automated DeepSeek review → merge. The rules for AI agents
and the pipeline details are in [AGENTS.md](AGENTS.md).

## Known limits

- **Global shortcuts under Wayland** go through the GlobalShortcuts portal, which GNOME
  implements from version 48. Where it exists Redline registers its shortcuts through it; on
  older versions Redline relies on the desktop's custom shortcuts.
- **Screen capture under Wayland**: the first time, the desktop asks which screen to share;
  the choice is remembered afterwards.
- **Tuning**: every change must be confirmed, otherwise LACT reverts to the previous values
  after a few seconds. That is a safeguard, not a bug.

## What Redline does not do

The reasons in full are in D15 of [docs/decisions.md](docs/decisions.md).

- **Edit your launcher or set anything for the whole session.** Redline shows a launch option and
  you paste it into the game: a global variable would reach every application.
- **Per-game profiles.** The choices are global and Redline keeps no list of games; the FPS limit
  lives in MangoHud's configuration, so it applies to every game started with MangoHud.
- **Upscaling through Gamescope.** It is one more tool to install, not reliably packaged on
  Ubuntu, and a spatial upscaler below what games already ship, while the upscaler upgrade above
  already covers the games that ship one.
- **An anti-lag switch.** `VK_AMD_anti_lag` is an extension the game itself must call: there is
  no global switch to offer.
- **Show which upscaler, or which version of it, a running game uses.** A loaded library does not
  prove the upscaler is active, and MangoHud's `fsr` key only reports Gamescope's own upscaler;
  Proton's own watermark is the reliable answer.
- **Detect a game's frame generation.** Nothing exposes it.
- **HDR colour accuracy and saturation.** Display colour belongs to the Wayland compositor.
- **Publish packages anywhere but [GitHub Releases](https://github.com/FabioSM46/redline/releases).**
  No apt repository, no AUR, no Flatpak: one place to download from and to check.
- **Run as root.** Redline never runs as root and never writes to sysfs: tuning goes through
  the `lactd` daemon, and installing a missing tool asks for your password once (`pkexec`).

## Release

A `v<version>` tag on a commit of `main` starts `.github/workflows/release.yml`, which builds
the `.deb`, Arch and Fedora packages and attaches them to a draft GitHub release. The release is published by hand after
checking it, following [docs/release.md](docs/release.md). The package carries `THIRD-PARTY-LICENSES` with the licenses of the crates and npm
packages compiled into the app, in `/usr/share/doc/redline/` next to `copyright`.

## Documentation

- [Context](docs/context.md)
- [Decisions](docs/decisions.md)
- [Architecture](docs/architecture.md)
- [Roadmap](docs/roadmap.md)
- [Release checklist](docs/release.md)
- [Changelog](CHANGELOG.md)

## Support

Redline is free and stays free. If it is useful to you, you can support its development through
[GitHub Sponsors](https://github.com/sponsors/FabioSM46) or [Ko-fi](https://ko-fi.com/fabiosm46);
the same links are in Settings → About.

## License

[MIT](LICENSE). LACT, GPU Screen Recorder and MangoHud are separate projects with their own
licenses; GPU Screen Recorder (GPL-3.0) is only started as an external program, and none of its
code is part of Redline.
