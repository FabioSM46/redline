# Release checklist

Every step here is **human-only**: merging into `main`, tagging and publishing are not done by an
AI or by a workflow. `.github/workflows/release.yml` only builds the `.deb` for a tag already on
`main` and attaches it to a draft release. The example is `0.1.0`; replace it with the version
being released.

## 1. Prepare `develop`

- [ ] The iteration's issues are closed and `develop` holds their PRs.
- [ ] The version is the same in `package.json`, `src-tauri/tauri.conf.json` and
      `src-tauri/Cargo.toml`.
- [ ] `CHANGELOG.md` has an entry for this version.
- [ ] The privacy checks of `AGENTS.md` pass over the whole history, and a secret scanner has
      read it too.

## 2. Manual checks before tagging

CI has no AMD GPU, no desktop and no real package install, so these are checked by hand. Build the
candidate from `develop` with the same commands as `release.yml`:

```bash
pnpm install --frozen-lockfile
python3 scripts/third-party-licenses.py --target x86_64-unknown-linux-gnu \
  --output src-tauri/target/THIRD-PARTY-LICENSES
pnpm tauri build --bundles deb --config src-tauri/tauri.release.linux.conf.json
bash scripts/deb-desktop-id.sh
```

The package is `src-tauri/target/release/bundle/deb/redline_<version>_amd64.deb`. Use a clean
virtual machine for what needs a clean system, and a machine with an AMD GPU for what needs one.
Quit from the tray menu after each check: closing the window leaves the app running.

### The package

- [ ] `dpkg-deb -c <file>.deb` lists `usr/share/applications/com.fabiosm46.redline.desktop` and no
      `redline.desktop`. This has only run on test packages since `deb-desktop-id.sh` last
      changed, never on a real build with `tauri.release.linux.conf.json`.
- [ ] It also lists `usr/share/doc/redline/THIRD-PARTY-LICENSES`,
      `usr/share/redline/install-dependency` and the polkit action.

### Clean virtual machine (Ubuntu LTS)

- [ ] `sudo apt install ./<file>.deb` succeeds, and `dpkg -V redline` prints nothing.
- [ ] Redline appears in the menu and starts from it.
- [ ] With nothing else installed, Settings → Dependencies shows the four tools as missing and the
      features that need them say what to do.
- [ ] Guided install of **LACT**, of **GPU Screen Recorder**, of **ffmpeg** and of **MangoHud**,
      each with its real `pkexec` prompt. No guided MangoHud install has run through a real
      `pkexec` yet.
- [ ] After each one, the row turns to installed without restarting the app.
- [ ] Cancelling the password prompt changes nothing and the page says so.
- [ ] On a clean install of each of Ubuntu 24.04, Ubuntu 26.04 and Debian 13: the package installs,
      starts, and the guided install of all four tools succeeds.

### Tray, autostart and themes

- [ ] Tray icon: closing the window keeps the app running, the icon brings it back, and Quit ends
      it.
- [ ] Autostart: the switch in Settings → General creates
      `~/.config/autostart/com.fabiosm46.redline.desktop` with `Name=Redline`, the desktop's
      startup applications list it as Redline, and after a new login the app starts, minimised to
      the tray when that option is on.
- [ ] Old autostart entry: with a Redline-written `~/.config/autostart/redline.desktop` in place
      and no new entry, start the app. Only `com.fabiosm46.redline.desktop` remains, and the next
      login starts the app once. Only unit tests have exercised this, not a real autostart folder.
- [ ] Light and dark: every tab reads correctly in both.

### Shortcuts (GNOME 48 or later, or KDE)

- [ ] Launch from the menu, bind the shortcuts in Settings → Shortcuts, quit, launch from a
      terminal with `redline`: the keys still work, with no second request from the desktop.
- [ ] The desktop's shortcut store holds them once, under `com.fabiosm46.redline` and not under
      `redline`. The store differs by desktop: on KDE look in `~/.config/kglobalshortcutsrc`, on
      GNOME in its global-shortcuts settings in dconf. This has not been checked yet on a desktop
      with the portal.

### Overlay (AMD GPU, a real game)

- [ ] The Copy button of Performance → Metrics → Overlay puts exactly `mangohud %command%` on the
      clipboard; paste it into the game's Launch options in Steam.
- [ ] A game started from Lutris shows the overlay with Configure → System options →
      **FPS counter (MangoHud)** on, and again with that off and `mangohud` as the Command prefix
      (under Advanced). Under Wine, use a game running on Vulkan, normally through DXVK.
- [ ] The rendered config works in the game: each preset (Off, Minimal, Standard, Full) shows what
      the page describes, and position and toggle key take effect.
- [ ] Take-over: with a `MangoHud.conf` of your own, **Let Redline manage it** keeps the file as
      `MangoHud.conf.redline-backup` and writes Redline's.
- [ ] Take-over refused: with a `MangoHud.conf` of your own and a `MangoHud.conf.redline-backup`
      already there, the page shows the error and neither file changes.

### Functions on real hardware

- [ ] Home and Performance show live figures; a change of power limit asks for confirmation and
      reverts without it.
- [ ] An instant replay saves a clip with its audio tracks.

### Overdrive switch (AMD GPU, overdrive on, enabled through LACT)

Run these in light and dark.

- [ ] With a fan curve and a stored undervolt, turn the switch off: the plan lists both, names the
      GPU and, if there is one, the active profile, and the confirmation stays disabled.
- [ ] Back to default on the fan and on the clocks: each asks to keep or revert. Revert one and it
      stays listed; keep both and the list empties, which enables the confirmation.
- [ ] Confirm: the section says a reboot is needed, and a fan curve, a clock value and a profile
      switch are refused until then.
- [ ] Reboot: overdrive is off and the fans still respond to temperature under the firmware.
- [ ] Turn the switch on, confirm and reboot: overdrive is back.

### Clocks and voltage (AMD GPU, overdrive on)

Use small, safe values, and keep a console or SSH session at hand: an unstable value can freeze
the machine. Run these in light and dark.

- [ ] Performance → Tuning → Advanced → Clocks and voltage shows only the controls of this GPU,
      with units and ranges, and warns before the first apply.
- [ ] Apply a small undervolt and let the countdown run out: the value goes back and the page says
      so. LACT's own view of the GPU agrees.
- [ ] Apply it and revert: the previous value is back.
- [ ] Apply it and confirm: the value stays, and it is still there after a reboot.
- [ ] Reset to factory values, confirm: the firmware's values are back and stay after a reboot.
- [ ] Reset and revert: the previous value is back.
- [ ] A value outside the range, and a positive voltage offset, are refused inline and nothing
      changes.
- [ ] With overdrive off the block points to the Overdrive section, and with `lactd` stopped it
      says what to do. No clock or voltage change has run on real hardware yet.
- [ ] Recovery as D13 describes it: with a confirmed value stored, turn overdrive off, reboot and
      reset from the app; then the console route, stopping `lactd` and restarting it after removing
      the values from `/etc/lact/config.yaml`.

Anything that fails goes to an issue and a PR on `develop`, and the checks start again.

## 3. Merge and tag

- [ ] Open a PR from `develop` to `main`, wait for a green `ci-gate` (on `main` every job runs) and
      merge it.
- [ ] Tag the merge commit on `main` and push the tag:

```bash
git fetch origin main
git tag v0.1.0 origin/main
git push origin v0.1.0
```

`release.yml` refuses a tag that is not on `main`.

## 4. Review the draft release

- [ ] The `Release` run is green and the draft `Redline v0.1.0` exists on GitHub Releases.
- [ ] Download `redline_0.1.0_amd64.deb` from the draft and repeat the `dpkg-deb -c` checks of
      "The package".
- [ ] Install it on the clean virtual machine; it starts from the menu.
- [ ] Choose the release date: today, as `YYYY-MM-DD`. This is the only step that chooses it, and
      it stands even if publishing happens on a later day.
- [ ] Replace the draft's notes with the `CHANGELOG.md` entry, with `Unreleased` replaced by that
      date.

If the package is wrong, do not publish: fix it through `develop` and `main`, then move the tag to
the new commit. The workflow replaces the package of a draft, and refuses to touch a published
release.

## 5. Publish

- [ ] Publish the draft.

## 6. After publishing

Through a PR to `develop`:

- [ ] `CHANGELOG.md`: replace `Unreleased` with the date chosen in step 4, the one in the published
      notes.
- [ ] `docs/roadmap.md`: check the release item and add `(done)` to the milestone. M4 is marked done
      only now, not when its issues close.
- [ ] `README.md`: update the Status section, which still points to building from source.
