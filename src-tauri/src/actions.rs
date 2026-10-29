//! Command-line actions (D5, D8): `redline --save-replay`, `--toggle-replay` and
//! `--toggle-recording`, to bind in the desktop's custom shortcuts until the GlobalShortcuts
//! portal is available.
//!
//! [`ACTION_ARGS`] is the whole list, and the contract Settings → Shortcuts (#37) shows: an
//! argument is accepted here only if it is in that list.
//!
//! - **Second launch** (an instance is running): the single-instance plugin hands the running
//!   instance the new launch's arguments ([`on_second_launch`]). With an action it performs it
//!   without showing or focusing the window; without one it brings the window to the front, as
//!   before.
//! - **First launch** with an action: the app starts hidden in the tray when there is one
//!   (`autostart::start_hidden`) and performs the action once GPU Screen Recorder has been
//!   detected and `replay_on_start` has run (`recorder::init`).
//!
//! The actions call the same [`Recorder`] methods as the interface's commands. A failure (GPU
//! Screen Recorder missing, the replay off or still starting) becomes a desktop notification:
//! the window may be hidden and the user is in a game. Unknown arguments are ignored.

use std::ffi::OsStr;
use std::thread;

use tauri::{AppHandle, Manager, Runtime};

use crate::desktop;
use crate::recorder::Recorder;
use crate::tray;

/// What a launch argument asks the running instance to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    SaveReplay,
    ToggleReplay,
    ToggleRecording,
}

/// An accepted argument and the action it performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionArg {
    pub arg: &'static str,
    pub action: Action,
}

/// Every accepted action argument, in the order Settings → Shortcuts shows them (#37).
pub const ACTION_ARGS: [ActionArg; 3] = [
    ActionArg {
        arg: "--save-replay",
        action: Action::SaveReplay,
    },
    ActionArg {
        arg: "--toggle-replay",
        action: Action::ToggleReplay,
    },
    ActionArg {
        arg: "--toggle-recording",
        action: Action::ToggleRecording,
    },
];

/// Shown before the error when the recorder is not there at all.
const NOT_READY: &str = "Redline is still starting: try again in a moment.";

impl Action {
    /// The notification summary when the action fails.
    fn failure_summary(self) -> &'static str {
        match self {
            Action::SaveReplay => "Replay not saved",
            Action::ToggleReplay => "Replay not started or stopped",
            Action::ToggleRecording => "Recording not started or stopped",
        }
    }
}

/// The action `args` (without the program name) ask for: the first argument that is exactly one
/// of [`ACTION_ARGS`]. Every other argument, `--autostart` included, is ignored here; one launch
/// performs one action.
pub fn requested<I, S>(args: I) -> Option<Action>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter().find_map(|arg| {
        ACTION_ARGS
            .iter()
            .find(|accepted| arg.as_ref() == OsStr::new(accepted.arg))
            .map(|accepted| accepted.action)
    })
}

/// What the running instance does for a second launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecondLaunch {
    /// No action: bring the window to the front (D8).
    Show,
    /// Perform the action, leaving the window as it is.
    Act(Action),
}

/// Decides a second launch from its `argv` as the single-instance plugin forwards it, program
/// name first.
pub fn second_launch(argv: &[String]) -> SecondLaunch {
    match requested(argv.iter().skip(1)) {
        Some(action) => SecondLaunch::Act(action),
        None => SecondLaunch::Show,
    }
}

/// The single-instance callback. It runs while the second launch waits for the answer on the
/// session bus, so the action runs on its own thread.
pub fn on_second_launch<R: Runtime>(app: &AppHandle<R>, argv: &[String]) {
    let action = match second_launch(argv) {
        SecondLaunch::Show => {
            tray::show_main_window(app);
            return;
        }
        SecondLaunch::Act(action) => action,
    };
    let service = app
        .try_state::<Recorder>()
        .map(|service| service.inner().clone());
    let spawned =
        thread::Builder::new()
            .name("redline-action".into())
            .spawn(move || match service {
                Some(service) => perform(&service, action),
                None => notify_failure(action, NOT_READY),
            });
    if let Err(error) = spawned {
        eprintln!("redline: the launch action did not start ({error})");
    }
}

/// Performs `action` and reports a failure with a desktop notification. Blocking: saving waits
/// for the file, stopping waits for the process.
pub fn perform(service: &Recorder, action: Action) {
    perform_with(service, action, notify_failure);
}

fn perform_with(service: &Recorder, action: Action, on_failure: impl FnOnce(Action, &str)) {
    if let Err(error) = run(service, action) {
        on_failure(action, &error);
    }
}

/// The same [`Recorder`] methods the interface's commands call.
fn run(service: &Recorder, action: Action) -> Result<(), String> {
    match action {
        Action::SaveReplay => service.save(),
        Action::ToggleReplay => service.toggle_replay().map(|_| ()),
        Action::ToggleRecording => service.toggle_recording().map(|_| ()),
    }
}

fn notify_failure(action: Action, error: &str) {
    // Also on stderr: without a notification server the notification shows nothing.
    eprintln!("redline: {}: {error}", action.failure_summary());
    desktop::notify(
        desktop::APP_NAME,
        action.failure_summary(),
        &desktop::escape_markup(error),
    );
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::recorder::{Availability, Environment, Events, FileSaved, MISSING, RecorderStatus};

    struct NoEvents;

    impl Events for NoEvents {
        fn state(&self, _: &RecorderStatus) {}
        fn file_saved(&self, _: &FileSaved) {}
    }

    fn argv(args: &[&str]) -> Vec<String> {
        std::iter::once("redline")
            .chain(args.iter().copied())
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn each_accepted_argument_names_its_action() {
        assert_eq!(requested(["--save-replay"]), Some(Action::SaveReplay));
        assert_eq!(requested(["--toggle-replay"]), Some(Action::ToggleReplay));
        assert_eq!(
            requested(["--toggle-recording"]),
            Some(Action::ToggleRecording)
        );
        for accepted in ACTION_ARGS {
            assert_eq!(requested([accepted.arg]), Some(accepted.action));
            assert!(accepted.arg.starts_with("--"));
        }
    }

    #[test]
    fn the_list_has_one_argument_per_action() {
        for (i, a) in ACTION_ARGS.iter().enumerate() {
            for b in &ACTION_ARGS[i + 1..] {
                assert_ne!(a.arg, b.arg);
                assert_ne!(a.action, b.action);
            }
        }
    }

    #[test]
    fn unknown_arguments_are_ignored() {
        let none: [&str; 0] = [];
        assert_eq!(requested(none), None);
        for unknown in [
            "--autostart",
            "--save-replay=1",
            "--SAVE-REPLAY",
            "save-replay",
            "-save-replay",
            "--toggle",
            "",
            "--",
        ] {
            assert_eq!(requested([unknown]), None, "{unknown:?}");
        }
        assert_eq!(
            requested(["--unknown", "--save-replay", "extra"]),
            Some(Action::SaveReplay)
        );
    }

    #[test]
    fn non_utf8_arguments_are_ignored_not_a_crash() {
        use std::os::unix::ffi::OsStrExt;
        let invalid = OsStr::from_bytes(b"--save-\xff");
        assert_eq!(requested([invalid]), None);
        assert_eq!(
            requested([invalid, OsStr::new("--toggle-replay")]),
            Some(Action::ToggleReplay)
        );
    }

    #[test]
    fn one_launch_performs_the_first_action_only() {
        assert_eq!(
            requested(["--toggle-recording", "--save-replay"]),
            Some(Action::ToggleRecording)
        );
    }

    #[test]
    fn autostart_together_with_an_action_still_performs_it() {
        assert_eq!(
            requested(["--autostart", "--save-replay"]),
            Some(Action::SaveReplay)
        );
        assert_eq!(
            requested(["--toggle-replay", "--autostart"]),
            Some(Action::ToggleReplay)
        );
    }

    #[test]
    fn a_second_launch_without_an_action_shows_the_window() {
        assert_eq!(second_launch(&argv(&[])), SecondLaunch::Show);
        assert_eq!(second_launch(&argv(&["--autostart"])), SecondLaunch::Show);
        assert_eq!(second_launch(&argv(&["--whatever"])), SecondLaunch::Show);
        // An empty argv (no program name) is not a crash either.
        assert_eq!(second_launch(&[]), SecondLaunch::Show);
    }

    #[test]
    fn a_second_launch_with_an_action_acts() {
        assert_eq!(
            second_launch(&argv(&["--save-replay"])),
            SecondLaunch::Act(Action::SaveReplay)
        );
        assert_eq!(
            second_launch(&argv(&["--autostart", "--toggle-recording"])),
            SecondLaunch::Act(Action::ToggleRecording)
        );
    }

    #[test]
    fn the_program_name_is_never_read_as_an_action() {
        let argv = vec!["--save-replay".to_owned()];
        assert_eq!(second_launch(&argv), SecondLaunch::Show);
    }

    #[test]
    fn a_failed_action_is_reported_with_the_recorder_sentence() {
        // No process is started: GPU Screen Recorder is missing.
        let service = Recorder::new(
            Environment {
                detect: Box::new(|| Availability::Missing(MISSING.to_owned())),
                config: Box::new(|| Err("no settings in this test".to_owned())),
                socket: Ok(PathBuf::from("/nowhere/gsr.sock")),
            },
            NoEvents,
        );
        // Detection has run by the time an action arrives (see `recorder::init`).
        assert!(!service.status().available);
        for accepted in ACTION_ARGS {
            let mut reported = None;
            perform_with(&service, accepted.action, |action, error| {
                reported = Some((action, error.to_owned()));
            });
            assert_eq!(reported, Some((accepted.action, MISSING.to_owned())));
        }
    }

    #[test]
    fn every_action_has_its_own_failure_summary() {
        let summaries: Vec<_> = ACTION_ARGS
            .iter()
            .map(|accepted| accepted.action.failure_summary())
            .collect();
        for (i, summary) in summaries.iter().enumerate() {
            assert!(!summaries[i + 1..].contains(summary));
        }
    }
}
