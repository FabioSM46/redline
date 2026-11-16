//! The game launch option: Redline composes it from the player's choices, the player pastes it
//! into the launcher (D15).
//!
//! - **The composer** ([`compose`], pure): variables first, then the `mangohud` wrapper, then
//!   `%command%`. A choice whose tool is missing composes nothing.
//! - **Detection** ([`frame_generation_installed`]): the frame generation layer's manifest in a
//!   `vulkan/implicit_layer.d` folder. Nothing is run, and missing it is a disabled choice with
//!   [`frame_generation_missing`], not an error. The overlay is detected by `overlay`.
//! - **The choices** are three switches stored by `settings` and saved through
//!   [`settings::save_launch_options`]. They are global: there is no per-game state.
//!
//! Redline sets none of these variables anywhere and edits no launcher configuration: the line
//! acts only where the player pastes it.
//!
//! What was verified against the tools' source, and when, is in `docs/architecture.md`
//! ("Launch options").

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime, State};

use crate::overlay;
use crate::settings::{self, LaunchChoices, SettingsState};

/// The release of the frame generation layer its manifest name, install folders and enable
/// variable were read from (tag `v1.3.1` of its repository). Nothing checks the installed one:
/// the layer is detected by its manifest, which carries no release number.
pub const FRAME_GENERATION_VERIFIED_VERSION: &str = "1.3.1";
/// The layer's manifest, as its build and its packages install it.
const FRAME_GENERATION_MANIFEST: &str = "afmf-linux.json";
/// The layer's documented switch (`enable_environment` in its manifest): without it the layer
/// stays out of the game.
const FRAME_GENERATION_VARIABLE: &str = "AFMF_ENABLE=1";
/// Read by the Proton builds named in [`UPSCALER_INDICATOR_NOTE`], ignored by the others.
const UPSCALER_INDICATOR_VARIABLE: &str = "PROTON_FSR4_INDICATOR=1";
/// MangoHud's launcher, which starts the game with the overlay (D12).
const OVERLAY_WRAPPER: &str = "mangohud";
/// What the launcher replaces with the game's own command.
const GAME_COMMAND: &str = "%command%";

/// What to do when the frame generation layer is missing. It names no product: the project is
/// named in `docs/architecture.md`.
pub fn frame_generation_missing() -> String {
    format!(
        "The frame generation layer is not installed. Install its release \
         {FRAME_GENERATION_VERIFIED_VERSION} for your distribution (Redline's documentation \
         names the project), then open this page again."
    )
}

/// Which games show the watermark.
pub const UPSCALER_INDICATOR_NOTE: &str = "The watermark appears only in games run with a Proton \
    build that supports it: GE-Proton 10-26 or later, or proton-cachyos 10.0-20250905 or later. \
    Valve's Proton does not.";

/// Which of the tools behind the choices are installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tools {
    /// MangoHud, as `overlay` detects it.
    pub overlay: bool,
    /// The frame generation layer.
    pub frame_generation: bool,
}

/// Whether a choice can be used, and what to install when it cannot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Availability {
    pub available: bool,
    /// What to install; `null` when `available`.
    pub unavailable_reason: Option<String>,
}

impl Availability {
    fn of(available: bool, missing: &str) -> Self {
        Self {
            available,
            unavailable_reason: (!available).then(|| missing.to_owned()),
        }
    }
}

/// The launch option, as `launch_options_status` and `set_launch_options` return it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LaunchOptionsStatus {
    /// The saved choices, also the ones whose tool is missing.
    pub choices: LaunchChoices,
    pub overlay: Availability,
    pub frame_generation: Availability,
    /// Which games show the upscaler watermark.
    pub upscaler_indicator_note: &'static str,
    /// The line to paste into the launcher.
    pub command: String,
}

/// The launch option for `choices`. Pure.
///
/// Variables first, then the wrapper, then `%command%`: the launcher puts the game's command in
/// place of the last word, and everything before a wrapper is the environment it starts in. A
/// choice whose tool is missing composes nothing (D15): the line never names a program that is
/// not there, and a stored "on" comes back by itself once the tool is installed. The watermark
/// has no tool to detect: it is a variable that a Proton build reads or ignores.
pub fn compose(choices: LaunchChoices, tools: Tools) -> String {
    let mut words: Vec<&str> = Vec::new();
    if choices.frame_generation && tools.frame_generation {
        words.push(FRAME_GENERATION_VARIABLE);
    }
    if choices.upscaler_indicator {
        words.push(UPSCALER_INDICATOR_VARIABLE);
    }
    if choices.overlay && tools.overlay {
        words.push(OVERLAY_WRAPPER);
    }
    words.push(GAME_COMMAND);
    words.join(" ")
}

/// The folders that can hold the frame generation layer's manifest: the user's data folder, where
/// the layer's own installer puts it, then the folders searched for MangoHud's.
pub fn frame_generation_dirs(
    data_home: Option<&Path>,
    xdg_data_dirs: Option<&OsStr>,
) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = data_home
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(overlay::LAYER_FOLDER))
        .into_iter()
        .collect();
    for dir in overlay::layer_dirs(xdg_data_dirs) {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// Whether one of `layer_dirs` holds the frame generation layer's manifest. Reads only the file
/// system.
pub fn frame_generation_installed(layer_dirs: &[PathBuf]) -> bool {
    layer_dirs
        .iter()
        .any(|dir| overlay::has_manifest(dir, |name| name == FRAME_GENERATION_MANIFEST))
}

fn status_of(choices: LaunchChoices, tools: Tools) -> LaunchOptionsStatus {
    LaunchOptionsStatus {
        choices,
        overlay: Availability::of(tools.overlay, overlay::MISSING),
        frame_generation: Availability::of(tools.frame_generation, &frame_generation_missing()),
        upscaler_indicator_note: UPSCALER_INDICATOR_NOTE,
        command: compose(choices, tools),
    }
}

/// The tools on this system, for this process's PATH and `XDG_DATA_DIRS` and the user's data
/// folder as Tauri resolves it.
fn detect<R: Runtime>(app: &AppHandle<R>) -> Tools {
    let data_home = app.path().data_dir().ok();
    let data_dirs: Option<OsString> = std::env::var_os("XDG_DATA_DIRS");
    let layer_dirs = frame_generation_dirs(data_home.as_deref(), data_dirs.as_deref());
    Tools {
        overlay: overlay::mangohud_present(),
        frame_generation: frame_generation_installed(&layer_dirs),
    }
}

/// The saved choices, which of their tools are installed, and the launch option they compose.
/// Changes nothing.
#[tauri::command]
pub fn launch_options_status<R: Runtime>(
    app: AppHandle<R>,
    settings: State<'_, SettingsState>,
) -> LaunchOptionsStatus {
    status_of(settings.current().launch_options, detect(&app))
}

/// Saves the choices and returns the new status. A choice whose tool is missing is saved as it
/// is sent: it composes nothing until the tool is installed.
///
/// `choices` replaces the saved ones as a whole, and all three are required.
#[tauri::command]
pub fn set_launch_options<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, SettingsState>,
    choices: LaunchChoices,
) -> Result<LaunchOptionsStatus, String> {
    let saved = settings::save_launch_options(&state, choices)?;
    Ok(status_of(saved, detect(&app)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use serde_json::json;
    use std::fs;

    const ALL: Tools = Tools {
        overlay: true,
        frame_generation: true,
    };
    const NONE: Tools = Tools {
        overlay: false,
        frame_generation: false,
    };

    fn choices(overlay: bool, frame_generation: bool, upscaler_indicator: bool) -> LaunchChoices {
        LaunchChoices {
            overlay,
            frame_generation,
            upscaler_indicator,
        }
    }

    /// A fake data dir with `name` in its layer folder; returns that folder.
    fn fake_manifest(dir: &Path, name: &str) -> PathBuf {
        let layers = dir.join("data").join(overlay::LAYER_FOLDER);
        fs::create_dir_all(&layers).unwrap();
        fs::write(layers.join(name), "{}").unwrap();
        layers
    }

    #[test]
    fn every_combination_composes_its_exact_line() {
        let cases = [
            ((false, false, false), "%command%"),
            ((true, false, false), "mangohud %command%"),
            ((false, true, false), "AFMF_ENABLE=1 %command%"),
            ((false, false, true), "PROTON_FSR4_INDICATOR=1 %command%"),
            ((true, true, false), "AFMF_ENABLE=1 mangohud %command%"),
            (
                (true, false, true),
                "PROTON_FSR4_INDICATOR=1 mangohud %command%",
            ),
            (
                (false, true, true),
                "AFMF_ENABLE=1 PROTON_FSR4_INDICATOR=1 %command%",
            ),
            (
                (true, true, true),
                "AFMF_ENABLE=1 PROTON_FSR4_INDICATOR=1 mangohud %command%",
            ),
        ];
        for ((overlay, frame_generation, upscaler_indicator), line) in cases {
            let chosen = choices(overlay, frame_generation, upscaler_indicator);
            assert_eq!(compose(chosen, ALL), line, "{chosen:?}");
        }
    }

    #[test]
    fn the_default_choices_compose_the_launch_option_of_the_overlay() {
        assert_eq!(compose(LaunchChoices::default(), ALL), overlay::LAUNCH_HINT);
    }

    #[test]
    fn a_choice_whose_tool_is_missing_composes_nothing() {
        let all = choices(true, true, true);
        assert_eq!(
            compose(
                all,
                Tools {
                    frame_generation: false,
                    ..ALL
                }
            ),
            "PROTON_FSR4_INDICATOR=1 mangohud %command%"
        );
        assert_eq!(
            compose(
                all,
                Tools {
                    overlay: false,
                    ..ALL
                }
            ),
            "AFMF_ENABLE=1 PROTON_FSR4_INDICATOR=1 %command%"
        );
        // The watermark needs no tool on this system.
        assert_eq!(compose(all, NONE), "PROTON_FSR4_INDICATOR=1 %command%");
        assert_eq!(compose(choices(true, true, false), NONE), "%command%");
    }

    #[test]
    fn the_line_always_ends_with_the_game_command_after_the_variables() {
        for bits in 0..8u8 {
            let chosen = choices(bits & 1 != 0, bits & 2 != 0, bits & 4 != 0);
            for tools in [ALL, NONE] {
                let line = compose(chosen, tools);
                let words: Vec<&str> = line.split(' ').collect();
                assert_eq!(words.last(), Some(&"%command%"), "{line}");
                let first_program = words.iter().position(|word| !word.contains('=')).unwrap();
                assert!(
                    words[first_program..].iter().all(|w| !w.contains('=')),
                    "{line}"
                );
            }
        }
    }

    #[test]
    fn the_manifest_in_a_layer_folder_is_the_layer() {
        let dir = TempDir::new("launch", "manifest");
        let layers = fake_manifest(&dir, FRAME_GENERATION_MANIFEST);
        assert_eq!(FRAME_GENERATION_MANIFEST, "afmf-linux.json");
        assert!(frame_generation_installed(&[
            dir.join("missing"),
            layers.clone()
        ]));
        fs::remove_file(layers.join(FRAME_GENERATION_MANIFEST)).unwrap();
        assert!(!frame_generation_installed(&[layers]));
    }

    #[test]
    fn other_manifests_and_no_folders_are_not_the_layer() {
        let dir = TempDir::new("launch", "absent");
        let layers = fake_manifest(&dir, "MangoHud.x86_64.json");
        fake_manifest(&dir, "afmf-linux.json.bak");
        fake_manifest(&dir, "not-afmf-linux.json");
        fake_manifest(&dir, "AFMF-LINUX.JSON");
        assert!(!frame_generation_installed(&[layers, dir.join("missing")]));
        assert!(!frame_generation_installed(&[]));
    }

    #[test]
    fn a_folder_named_like_the_manifest_is_not_one() {
        let dir = TempDir::new("launch", "manifest-folder");
        let layers = fake_manifest(&dir, "other.json");
        fs::create_dir(layers.join(FRAME_GENERATION_MANIFEST)).unwrap();
        assert!(!frame_generation_installed(&[layers]));
    }

    #[test]
    fn the_users_data_folder_is_searched_before_the_folders_of_the_overlay() {
        let data_dirs = OsStr::new("/data/one:/usr/share");
        assert_eq!(
            frame_generation_dirs(Some(Path::new("/user/data")), Some(data_dirs)),
            [
                "/user/data/vulkan/implicit_layer.d",
                "/data/one/vulkan/implicit_layer.d",
                "/usr/share/vulkan/implicit_layer.d",
                "/etc/vulkan/implicit_layer.d",
            ]
            .map(PathBuf::from)
        );
        // Without a data folder, a relative one, or one already listed: the overlay's folders.
        let shared = overlay::layer_dirs(Some(data_dirs));
        assert_eq!(frame_generation_dirs(None, Some(data_dirs)), shared);
        assert_eq!(
            frame_generation_dirs(Some(Path::new("relative")), Some(data_dirs)),
            shared
        );
        assert_eq!(
            frame_generation_dirs(Some(Path::new("/data/one")), Some(data_dirs)),
            shared
        );
    }

    #[test]
    fn a_missing_layer_is_a_sentence_and_a_stored_on_composes_nothing() {
        let stored = choices(true, true, false);
        let status = status_of(
            stored,
            Tools {
                frame_generation: false,
                ..ALL
            },
        );
        assert_eq!(status.choices, stored);
        assert!(!status.frame_generation.available);
        assert_eq!(
            status.frame_generation.unavailable_reason.as_deref(),
            Some(frame_generation_missing().as_str())
        );
        assert_eq!(status.command, "mangohud %command%");

        let status = status_of(stored, ALL);
        assert_eq!(status.frame_generation.unavailable_reason, None);
        assert_eq!(status.command, "AFMF_ENABLE=1 mangohud %command%");
    }

    #[test]
    fn a_missing_overlay_carries_the_sentence_of_the_overlay() {
        let status = status_of(LaunchChoices::default(), NONE);
        assert_eq!(
            status.overlay.unavailable_reason.as_deref(),
            Some(overlay::MISSING)
        );
        assert_eq!(status.command, "%command%");
    }

    #[test]
    fn the_sentences_say_frame_generation_and_never_the_name_of_the_layer() {
        let missing = frame_generation_missing();
        for sentence in [missing.as_str(), UPSCALER_INDICATOR_NOTE] {
            let lower = sentence.to_lowercase();
            for word in ["afmf", "fsr", "amd", "radeon", "adrenalin"] {
                assert!(!lower.contains(word), "{sentence}");
            }
        }
        assert!(missing.contains("frame generation"));
        assert!(missing.contains("release 1.3.1 "));
        assert!(UPSCALER_INDICATOR_NOTE.contains("Proton"));
    }

    #[test]
    fn serializes_the_contract() {
        let status = status_of(
            choices(true, true, true),
            Tools {
                frame_generation: false,
                ..ALL
            },
        );
        assert_eq!(
            serde_json::to_value(status).unwrap(),
            json!({
                "choices": {
                    "overlay": true,
                    "frame_generation": true,
                    "upscaler_indicator": true,
                },
                "overlay": { "available": true, "unavailable_reason": null },
                "frame_generation": {
                    "available": false,
                    "unavailable_reason": frame_generation_missing(),
                },
                "upscaler_indicator_note": UPSCALER_INDICATOR_NOTE,
                "command": "PROTON_FSR4_INDICATOR=1 mangohud %command%",
            })
        );
        // What `set_launch_options` receives: the three switches, all of them.
        let sent =
            json!({ "overlay": false, "frame_generation": true, "upscaler_indicator": false });
        let read: LaunchChoices = serde_json::from_value(sent).unwrap();
        assert_eq!(read, choices(false, true, false));
        let partial = json!({ "overlay": false });
        assert!(serde_json::from_value::<LaunchChoices>(partial).is_err());
    }
}
