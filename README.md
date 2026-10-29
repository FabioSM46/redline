# Redline

A control panel for **AMD GPUs on Linux**. One app to watch temperatures, clocks and power draw,
tune fans and power limits, and save a replay of the last minute of gameplay with a shortcut.
The inspiration is the software AMD ships only for Windows; Redline keeps the features people
actually use and leaves the rest out.

> Independent project, not affiliated with or endorsed by AMD.

## Status

In development: **M0 — Foundations** is done, **M1 — Performance** is next (see the
[roadmap](docs/roadmap.md)). Today there is:

- GPU sensor reading, tested on an RX 9070 (RDNA4);
- a client for the LACT daemon, verified on real hardware;
- control of GPU Screen Recorder, verified on real hardware: an instant replay saved with desktop
  audio and microphone on two separate tracks;
- a minimal Tauri app that shows live metrics.

There is no installable package yet.

## Planned features

| Feature | How |
|---|---|
| Live statistics and charts | read directly from sysfs (`/sys/class/drm/cardN/device`), no permissions needed |
| In-game overlay (FPS, frametime, temperatures) | [MangoHud](https://github.com/flightlessmango/MangoHud), configured by Redline |
| Tuning: fans, power limit, clocks, undervolt, profiles | the [LACT](https://github.com/ilya-zlobintsev/LACT) daemon (`lactd`) through its socket |
| Instant replay and recording, with audio on separate tracks | [GPU Screen Recorder](https://git.dec05eba.com/gpu-screen-recorder/about/) as an external process, with hardware encoding |

Redline does not rewrite these tools: it coordinates them behind a single interface. The why is
in [docs/context.md](docs/context.md), the choices in [docs/decisions.md](docs/decisions.md).

## Requirements

- Linux with an AMD GPU on the `amdgpu` driver (every recent Radeon)
- Wayland or X11
- For tuning: LACT with the `lactd` daemon running
- For replay and recording: GPU Screen Recorder

On Ubuntu 24.04, `scripts/install-deps.sh` installs LACT (from the official `.deb`) and GPU
Screen Recorder (from Flathub):

```bash
sudo bash scripts/install-deps.sh
```

Undervolting and overclocking also need the kernel parameter `amdgpu.ppfeaturemask=0xffffffff`.

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
│   └── roadmap.md          # milestones
├── scripts/                # install-deps.sh, pipeline helpers and automation tests
├── .github/                # CI, DeepSeek review, labeler, issue templates
├── .claude/skills/         # agent skills (/dev-issue, /process-pr, /develop-iteration, /scrum-master)
├── AGENTS.md               # rules for AI agents and the pipeline
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
  libxdo-dev libssl-dev librsvg2-dev
```

### Commands

```bash
pnpm install          # frontend dependencies
pnpm tauri dev        # run the app in development with hot reload
pnpm tauri build      # release build and .deb package
```

The `.deb` ends up in `src-tauri/target/release/bundle/deb/`.

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
cargo run -p redline-probe -- lact stats          # statistics from lactd
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
  implements from version 48. On older versions Redline will rely on the desktop's custom
  shortcuts.
- **Screen capture under Wayland**: the first time, the desktop asks which screen to share;
  the choice is remembered afterwards.
- **Tuning**: every change must be confirmed, otherwise LACT reverts to the previous values
  after a few seconds. That is a safeguard, not a bug.

## Release

A `v<version>` tag on a commit of `main` starts `.github/workflows/release.yml`, which builds
the `.deb` and attaches it to a draft GitHub release. The release is published by hand after
checking it. The package carries `THIRD-PARTY-LICENSES` with the licenses of the crates and npm
packages compiled into the app, in `/usr/share/doc/redline/` next to `copyright`.

## Documentation

- [Context](docs/context.md)
- [Decisions](docs/decisions.md)
- [Architecture](docs/architecture.md)
- [Roadmap](docs/roadmap.md)

## License

[MIT](LICENSE). LACT, GPU Screen Recorder and MangoHud are separate projects with their own
licenses; GPU Screen Recorder (GPL-3.0) is only started as an external program, and none of its
code is part of Redline.
