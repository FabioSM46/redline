//! What the recording settings can choose from: audio sources and capture targets, as GPU Screen
//! Recorder lists them (`--list-audio-devices`, `--list-application-audio`, `--list-monitors`).
//!
//! Without GPU Screen Recorder both commands answer `available: false` with the sentence that
//! says what to install: a missing tool is a disabled feature, not an error.

use redline_core::recorder::{self, Launcher};
use serde::Serialize;
use tauri::State;

use crate::recorder::Recorder;

/// One choice: `id` is the value the recording settings store, `label` what the user reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Choice {
    pub id: String,
    pub label: String,
}

/// A list of choices, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Choices {
    pub available: bool,
    pub unavailable_reason: Option<String>,
    pub items: Vec<Choice>,
}

impl Choices {
    fn missing(reason: String) -> Self {
        Self {
            available: false,
            unavailable_reason: Some(reason),
            items: Vec::new(),
        }
    }

    fn ready(items: Vec<Choice>) -> Self {
        Self {
            available: true,
            unavailable_reason: None,
            items,
        }
    }
}

/// Audio sources for `audio_tracks`: the devices (the default output and input first, as
/// GPU Screen Recorder lists them), then the applications playing audio now as `app:<name>`.
pub fn audio_choices(devices: Vec<(String, String)>, apps: Vec<String>) -> Vec<Choice> {
    let devices = devices.into_iter().map(|(name, description)| {
        let id = if matches!(name.as_str(), "default_output" | "default_input") {
            name.clone()
        } else {
            format!("device:{name}")
        };
        let label = if description.is_empty() {
            name
        } else {
            description
        };
        Choice { id, label }
    });
    let apps = apps.into_iter().map(|app| Choice {
        id: format!("app:{app}"),
        label: app,
    });
    devices.chain(apps).collect()
}

/// Capture targets for `capture`: the portal first (D8), then each monitor by name.
pub fn capture_choices(monitors: Vec<(String, String)>) -> Vec<Choice> {
    let portal = Choice {
        id: recorder::PORTAL.to_owned(),
        label: "Choose with the desktop (portal)".to_owned(),
    };
    let monitors = monitors.into_iter().map(|(name, resolution)| Choice {
        label: if resolution.is_empty() {
            name.clone()
        } else {
            format!("{name} ({resolution})")
        },
        id: name,
    });
    std::iter::once(portal).chain(monitors).collect()
}

/// Runs `list` with GPU Screen Recorder on the blocking pool: listing starts a process.
async fn listing(
    service: &Recorder,
    list: impl FnOnce(&Launcher) -> Result<Vec<Choice>, String> + Send + 'static,
) -> Result<Choices, String> {
    let service = service.clone();
    tauri::async_runtime::spawn_blocking(move || match service.launcher() {
        Ok(launcher) => list(&launcher).map(Choices::ready),
        Err(reason) => Ok(Choices::missing(reason)),
    })
    .await
    .map_err(|error| format!("The recorder did not answer: {error}"))?
}

#[tauri::command]
pub async fn list_audio_sources(service: State<'_, Recorder>) -> Result<Choices, String> {
    listing(&service, |launcher| {
        let devices = recorder::list_audio_devices(launcher)
            .map_err(|error| format!("Could not list the audio devices: {error:#}"))?;
        let apps = recorder::list_application_audio(launcher)
            .map_err(|error| format!("Could not list the applications: {error:#}"))?;
        Ok(audio_choices(devices, apps))
    })
    .await
}

#[tauri::command]
pub async fn list_capture_targets(service: State<'_, Recorder>) -> Result<Choices, String> {
    listing(&service, |launcher| {
        recorder::list_monitors(launcher)
            .map(capture_choices)
            .map_err(|error| format!("Could not list the monitors: {error:#}"))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(id: &str, label: &str) -> Choice {
        Choice {
            id: id.into(),
            label: label.into(),
        }
    }

    #[test]
    fn audio_sources_are_devices_then_applications() {
        // From the fixed sample text, through the same parsers the commands use.
        let devices = recorder::parse_audio_devices(
            "default_output|Default output\n\
             default_input|Default input\n\
             alsa_input.usb-mic.analog-stereo|USB Microphone\n\
             alsa_output.hdmi.monitor|\n",
        );
        let apps = recorder::parse_application_audio("Firefox\nDiscord\n");
        assert_eq!(
            audio_choices(devices, apps),
            [
                choice("default_output", "Default output"),
                choice("default_input", "Default input"),
                choice("device:alsa_input.usb-mic.analog-stereo", "USB Microphone"),
                choice(
                    "device:alsa_output.hdmi.monitor",
                    "alsa_output.hdmi.monitor"
                ),
                choice("app:Firefox", "Firefox"),
                choice("app:Discord", "Discord"),
            ]
        );
    }

    #[test]
    fn capture_targets_put_the_portal_first() {
        let monitors = recorder::parse_monitors("DP-2|3440x1440\nHDMI-A-1|\n");
        assert_eq!(
            capture_choices(monitors),
            [
                choice("portal", "Choose with the desktop (portal)"),
                choice("DP-2", "DP-2 (3440x1440)"),
                choice("HDMI-A-1", "HDMI-A-1"),
            ]
        );
        assert_eq!(capture_choices(Vec::new()).len(), 1);
    }

    #[test]
    fn every_listed_id_is_a_valid_setting() {
        use crate::settings::RecordingSettings;
        let mut settings = RecordingSettings::defaults(std::path::Path::new("/videos"));
        let audio = audio_choices(
            vec![("alsa_input.usb-mic".into(), "Mic".into())],
            vec!["Firefox".into()],
        );
        settings.audio_tracks = vec![audio.into_iter().map(|choice| choice.id).collect()];
        for target in capture_choices(vec![("DP-2".into(), "3440x1440".into())]) {
            settings.capture = target.id;
            assert!(settings.problems().is_empty(), "{settings:?}");
        }
    }

    #[test]
    fn a_missing_tool_is_a_disabled_list() {
        let value = serde_json::to_value(Choices::missing("Install it.".into())).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"available": false, "unavailable_reason": "Install it.", "items": []})
        );
    }
}
