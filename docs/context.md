# Context

## Why Redline exists

Anyone moving from Windows to Linux to play games on an AMD GPU loses **AMD Software: Adrenalin
Edition**: an overlay with FPS and temperatures, performance charts, fan and power tuning,
instant replays of the last minute and recording on a shortcut. AMD does not ship it for Linux.

On Linux every piece already exists as a separate project:

| Feature | Project |
|---|---|
| In-game overlay | MangoHud |
| Tuning (fans, power, clocks, undervolt) | LACT, CoreCtrl |
| Replay and recording with hardware encoding | GPU Screen Recorder, OBS |
| Monitoring | amdgpu_top, Mission Center |

The problem is that they are five programs, with five interfaces and five configurations.
Redline is **one app** that coordinates them, with an interface that someone coming from AMD's
Windows software recognises.

## What it takes from AMD's software and what it leaves out

It takes: a dashboard with the main metrics, charts over time, the overlay, tuning with profiles,
instant replay, recording, shortcuts.

It leaves out, because only a tiny minority uses them: the built-in browser and social features,
built-in streaming, online game statistics, driver and game notifications, per-game graphics
settings that on Linux belong to Mesa and Proton, and display settings that belong to the
desktop.

## Who it is for

Gamers with an AMD GPU on a recent Linux distribution, under Wayland or X11. The reference is
Ubuntu LTS with GNOME, but Redline does not depend on a distribution or a desktop.
