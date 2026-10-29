//! The saved clips: the listing of the replay and recording folders, opening a clip or its
//! folder, and the desktop notification for every saved file.
//!
//! The frontend never sees a path. Every clip has an opaque `id`; the backend turns it back
//! into a path only by listing the two folders again and taking the entry with that id, so an id
//! can never name a file outside them, nor a file the listing would not show.
//!
//! - `list_recordings` → `[{ id, kind: "replay" | "recording", file_name, saved_at_ms,
//!   size_bytes, duration_s }]`, newest first. `saved_at_ms` is the file's modification time
//!   (wall-clock milliseconds), `duration_s` is `null` when the container header does not say.
//!   Only regular files with a video extension, directly inside the folders; hidden files,
//!   folders, symbolic links and folders that do not exist are skipped.
//! - `open_recording(id)`: `xdg-open` on the file. `show_in_folder(id)`: the file manager's
//!   `FileManager1.ShowItems` with the file selected, otherwise `xdg-open` on its folder. An
//!   unknown id is an error.
//! - Previews are in `thumbnails` (`recording_thumbnail(id)`); listing prunes their cache.
//! - [`notify_saved`]: "Replay saved" / "Recording saved" with the file name, hooked on the
//!   recorder's `file-saved`. No notification server means no notification, silently.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::UNIX_EPOCH;

use redline_core::media;
use redline_core::recorder::ReplayConfig;
use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime};

use crate::recorder::{self, FileKind, FileSaved};
use crate::{desktop, thumbnails};

/// The extensions of the containers GPU Screen Recorder can write.
const VIDEO_EXTENSIONS: [&str; 5] = ["mkv", "mp4", "webm", "flv", "mov"];
const APP_NAME: &str = "Redline";
const NO_FOLDER: &str = "No video folder to look for saved clips in.";
const UNKNOWN_ID: &str = "This clip is no longer in the replay or recording folder.";

/// One saved clip, as `list_recordings` returns it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Recording {
    /// Opaque to the frontend: pass it back to `open_recording` and `show_in_folder`.
    pub id: String,
    pub kind: FileKind,
    pub file_name: String,
    pub saved_at_ms: u64,
    pub size_bytes: u64,
    pub duration_s: Option<f64>,
}

/// The two folders the gallery shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folders {
    pub replay: PathBuf,
    pub recording: PathBuf,
}

impl From<&ReplayConfig> for Folders {
    fn from(config: &ReplayConfig) -> Self {
        Self {
            replay: config.replay_dir.clone(),
            recording: config.recording_dir.clone(),
        }
    }
}

/// The replay and recording folders, from the same source the recorder uses. The only place
/// the gallery reads them: when the recording settings land (#29), this is the line to change.
pub fn folders<R: Runtime>(app: &AppHandle<R>) -> Result<Folders, String> {
    let paths = app.path();
    let video_dir = paths
        .video_dir()
        .or_else(|_| paths.home_dir().map(|home| home.join("Videos")))
        .map_err(|_| NO_FOLDER.to_owned())?;
    Ok(Folders::from(&recorder::default_config(&video_dir)))
}

/// A clip found in a folder, before its duration is read.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    id: String,
    kind: FileKind,
    file_name: String,
    path: PathBuf,
    saved_at_ms: u64,
    size_bytes: u64,
}

fn kind_name(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Replay => "replay",
        FileKind::Recording => "recording",
    }
}

/// Every clip in the two folders, newest first (ties by file name, so the order is stable).
fn entries(folders: &Folders) -> Vec<Entry> {
    let mut found = Vec::new();
    if folders.replay == folders.recording {
        // One folder for both: GPU Screen Recorder names recordings `Video_…`.
        scan(&folders.replay, &mut found, |name| {
            if name.starts_with("Video_") {
                FileKind::Recording
            } else {
                FileKind::Replay
            }
        });
    } else {
        scan(&folders.replay, &mut found, |_| FileKind::Replay);
        scan(&folders.recording, &mut found, |_| FileKind::Recording);
    }
    found.sort_by(|a, b| {
        b.saved_at_ms
            .cmp(&a.saved_at_ms)
            .then_with(|| b.file_name.cmp(&a.file_name))
            .then_with(|| a.id.cmp(&b.id))
    });
    found
}

/// Adds the clips directly inside `dir`; a folder that cannot be read adds nothing.
fn scan(dir: &Path, found: &mut Vec<Entry>, kind_of: impl Fn(&str) -> FileKind) {
    let Ok(listing) = fs::read_dir(dir) else {
        return;
    };
    for entry in listing.flatten() {
        // `file_type` does not follow links: a link could point outside the folder.
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let Ok(file_name) = entry.file_name().into_string() else {
            continue;
        };
        if !is_clip_name(&file_name) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let saved_at_ms = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            });
        let kind = kind_of(&file_name);
        found.push(Entry {
            id: format!("{}:{file_name}", kind_name(kind)),
            kind,
            path: entry.path(),
            file_name,
            saved_at_ms,
            size_bytes: metadata.len(),
        });
    }
}

/// A visible file with the extension of a container GPU Screen Recorder writes.
fn is_clip_name(file_name: &str) -> bool {
    if file_name.starts_with('.') {
        return false;
    }
    Path::new(file_name)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            VIDEO_EXTENSIONS
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

/// The clips of both folders, newest first, with their duration when the header has it.
pub fn list(folders: &Folders) -> Vec<Recording> {
    entries(folders)
        .into_iter()
        .map(|entry| Recording {
            duration_s: media::duration_seconds(&entry.path),
            id: entry.id,
            kind: entry.kind,
            file_name: entry.file_name,
            saved_at_ms: entry.saved_at_ms,
            size_bytes: entry.size_bytes,
        })
        .collect()
}

/// The paths of the clips of both folders, in listing order, without reading their headers.
pub fn paths(folders: &Folders) -> Vec<PathBuf> {
    entries(folders)
        .into_iter()
        .map(|entry| entry.path)
        .collect()
}

/// The path of the clip with `id`, found by listing the folders again: an id the listing does
/// not produce right now is unknown.
pub fn resolve(folders: &Folders, id: &str) -> Result<PathBuf, String> {
    entries(folders)
        .into_iter()
        .find(|entry| entry.id == id)
        .map(|entry| entry.path)
        .ok_or_else(|| UNKNOWN_ID.to_owned())
}

/// Starts `xdg-open` on `target` and reaps it in the background.
fn xdg_open(target: &Path) -> Result<(), String> {
    let mut child = Command::new("xdg-open")
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Could not run xdg-open ({error}). Install xdg-utils."))?;
    // xdg-open returns once the application is started; waiting keeps it from staying a zombie.
    let reaped = thread::Builder::new()
        .name("redline-xdg-open".into())
        .spawn(move || {
            let _ = child.wait();
        });
    if let Err(error) = reaped {
        eprintln!("redline: could not wait for xdg-open ({error})");
    }
    Ok(())
}

/// The notification for a saved file: its summary and its body (the file name, escaped for the
/// servers that read the body as markup). No folder, no path.
pub fn notification_text(file: &FileSaved) -> (&'static str, String) {
    let summary = match file.kind {
        FileKind::Replay => "Replay saved",
        FileKind::Recording => "Recording saved",
    };
    let body = file
        .file_name
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    (summary, body)
}

/// Shows "Replay saved" / "Recording saved" for `file`, on its own thread: the caller is the
/// thread reading GPU Screen Recorder's output, and the D-Bus call may wait. Without a
/// notification server nothing happens.
pub fn notify_saved(file: &FileSaved) {
    let (summary, body) = notification_text(file);
    let spawned = thread::Builder::new()
        .name("redline-notify".into())
        .spawn(move || {
            desktop::notify(APP_NAME, summary, &body);
        });
    if let Err(error) = spawned {
        eprintln!("redline: the saved-file notification did not start ({error})");
    }
}

/// Runs `job` with the folders on the blocking pool: listing and opening touch the disk.
pub(crate) async fn with_folders<R: Runtime, T: Send + 'static>(
    app: AppHandle<R>,
    job: impl FnOnce(Folders) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || job(folders(&app)?))
        .await
        .map_err(|error| error.to_string())?
}

/// The saved clips of both folders, newest first. Also drops the cached previews of clips
/// that are gone or changed.
#[tauri::command]
pub async fn list_recordings(app: AppHandle) -> Result<Vec<Recording>, String> {
    let thumbnailer = thumbnails::thumbnailer(&app);
    with_folders(app, move |folders| {
        if let Some(thumbnailer) = thumbnailer {
            thumbnailer.prune(&paths(&folders));
        }
        Ok(list(&folders))
    })
    .await
}

/// Opens the clip `id` in the default video player.
#[tauri::command]
pub async fn open_recording(app: AppHandle, id: String) -> Result<(), String> {
    with_folders(app, move |folders| xdg_open(&resolve(&folders, &id)?)).await
}

/// Shows the clip `id` in the file manager: selected when the file manager supports it,
/// otherwise its folder.
#[tauri::command]
pub async fn show_in_folder(app: AppHandle, id: String) -> Result<(), String> {
    with_folders(app, move |folders| {
        let path = resolve(&folders, &id)?;
        if desktop::show_items(&path) {
            return Ok(());
        }
        let folder = path.parent().ok_or_else(|| UNKNOWN_ID.to_owned())?;
        xdg_open(folder)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{File, FileTimes};
    use std::time::{Duration, SystemTime};

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("redline-gallery-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A file of `size` bytes modified `seconds` after the epoch.
    fn clip(dir: &Path, name: &str, seconds: u64, size: usize) {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, vec![0u8; size]).unwrap();
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(time))
            .unwrap();
    }

    fn folders(root: &Path) -> Folders {
        Folders {
            replay: root.join("Replays"),
            recording: root.join("Recordings"),
        }
    }

    fn names(list: &[Recording]) -> Vec<&str> {
        list.iter().map(|clip| clip.file_name.as_str()).collect()
    }

    #[test]
    fn both_folders_are_listed_newest_first() {
        let root = temp_dir("order");
        let folders = folders(&root);
        clip(&folders.replay, "Replay_a.mkv", 1_000, 10);
        clip(&folders.recording, "Video_b.mp4", 3_000, 20);
        clip(&folders.replay, "Replay_c.mkv", 2_000, 30);

        let list = list(&folders);
        assert_eq!(
            names(&list),
            ["Video_b.mp4", "Replay_c.mkv", "Replay_a.mkv"]
        );
        assert_eq!(list[0].kind, FileKind::Recording);
        assert_eq!(list[1].kind, FileKind::Replay);
        assert_eq!(list[0].saved_at_ms, 3_000_000);
        assert_eq!(list[0].size_bytes, 20);
        // Zero bytes are not a video header: unknown, not zero.
        assert_eq!(list[0].duration_s, None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn only_visible_video_files_are_listed() {
        let root = temp_dir("filter");
        let folders = folders(&root);
        clip(&folders.replay, "Replay_a.MKV", 1, 1);
        clip(&folders.replay, "Replay_b.webm", 2, 1);
        clip(&folders.replay, "notes.txt", 3, 1);
        clip(&folders.replay, "Replay_c", 4, 1);
        clip(&folders.replay, ".Replay_d.mkv", 5, 1);
        clip(&folders.replay.join("nested.mkv"), "Replay_e.mkv", 6, 1);
        std::os::unix::fs::symlink(
            folders.replay.join("Replay_a.MKV"),
            folders.replay.join("link.mkv"),
        )
        .unwrap();
        // The recording folder does not exist: nothing from it, and no error.
        assert_eq!(names(&list(&folders)), ["Replay_b.webm", "Replay_a.MKV"]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_folders_are_an_empty_list() {
        let root = temp_dir("missing");
        assert!(list(&folders(&root.join("nowhere"))).is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ids_resolve_only_to_listed_files() {
        let root = temp_dir("ids");
        let folders = folders(&root);
        clip(&folders.replay, "Replay_a.mkv", 1, 1);
        clip(&folders.recording, "Replay_a.mkv", 2, 1);
        clip(&root, "outside.mkv", 3, 1);

        let list = list(&folders);
        assert_eq!(list.len(), 2);
        // The same name in both folders gives two different ids.
        assert_ne!(list[0].id, list[1].id);
        for clip in &list {
            let path = resolve(&folders, &clip.id).unwrap();
            assert_eq!(path.file_name().unwrap(), "Replay_a.mkv");
        }
        assert_eq!(
            resolve(&folders, &list[0].id).unwrap(),
            folders.recording.join("Replay_a.mkv")
        );

        for unknown in [
            "",
            "replay:missing.mkv",
            "replay:../outside.mkv",
            "recording:/etc/passwd",
            "Replay_a.mkv",
        ] {
            assert_eq!(
                resolve(&folders, unknown),
                Err(UNKNOWN_ID.to_owned()),
                "{unknown}"
            );
        }
        // A clip deleted after the listing is unknown too.
        fs::remove_file(folders.replay.join("Replay_a.mkv")).unwrap();
        assert_eq!(
            resolve(&folders, "replay:Replay_a.mkv"),
            Err(UNKNOWN_ID.to_owned())
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn one_folder_for_both_kinds_lists_each_file_once() {
        let root = temp_dir("same");
        let same = Folders {
            replay: root.clone(),
            recording: root.clone(),
        };
        clip(&root, "Replay_a.mkv", 1, 1);
        clip(&root, "Video_b.mkv", 2, 1);
        let list = list(&same);
        assert_eq!(names(&list), ["Video_b.mkv", "Replay_a.mkv"]);
        assert_eq!(list[0].kind, FileKind::Recording);
        assert_eq!(list[1].kind, FileKind::Replay);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_listing_uses_the_documented_shape() {
        let value = serde_json::to_value(Recording {
            id: "replay:Replay_a.mkv".into(),
            kind: FileKind::Replay,
            file_name: "Replay_a.mkv".into(),
            saved_at_ms: 1_700_000_000_000,
            size_bytes: 42,
            duration_s: None,
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "id": "replay:Replay_a.mkv",
                "kind": "replay",
                "file_name": "Replay_a.mkv",
                "saved_at_ms": 1_700_000_000_000u64,
                "size_bytes": 42,
                "duration_s": null,
            })
        );
    }

    #[test]
    fn notifications_name_the_kind_and_the_file_only() {
        let saved = |kind, name: &str| FileSaved {
            kind,
            file_name: name.into(),
            saved_at_ms: 0,
        };
        assert_eq!(
            notification_text(&saved(FileKind::Replay, "Replay_2026.mkv")),
            ("Replay saved", "Replay_2026.mkv".to_owned())
        );
        assert_eq!(
            notification_text(&saved(FileKind::Recording, "Video <1> & 2.mp4")),
            ("Recording saved", "Video &lt;1&gt; &amp; 2.mp4".to_owned())
        );
    }

    #[test]
    fn the_folders_come_from_the_recorder_config() {
        let config = recorder::default_config(Path::new("/redline-test/Videos"));
        assert_eq!(
            Folders::from(&config),
            Folders {
                replay: PathBuf::from("/redline-test/Videos/Redline/Replays"),
                recording: PathBuf::from("/redline-test/Videos/Redline/Recordings"),
            }
        );
    }
}
