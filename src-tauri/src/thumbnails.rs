//! Previews of saved clips, on top of the gallery's ids (the frontend never sees a path).
//!
//! - `recording_thumbnail(id)` → `string | null`: a `data:image/png;base64,…` or
//!   `data:image/jpeg;base64,…` URL, or `null` when no preview can be produced. An unknown id is
//!   an error, as for the other gallery commands.
//! - `thumbnail_support()` → `{ generator: "ffmpegthumbnailer" | "ffmpeg" | null, hint }`:
//!   which tool generates the missing previews, and when there is none, the sentence that says
//!   what to install. The desktop's thumbnail cache can still answer without a tool.
//!
//! The work is in `redline_core::thumbnail`; this module only supplies the folders: the desktop
//! cache (`<cache dir>/thumbnails`) and Redline's own (`<app cache dir>/thumbnails`).

use redline_core::thumbnail::{self, Thumbnailer, Tool};
use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime};

use crate::gallery;

/// Whether missing previews can be generated, as `thumbnail_support` returns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThumbnailSupport {
    /// The tool that generates previews, `null` when none is installed.
    pub generator: Option<&'static str>,
    /// With no generator: what to install. `null` otherwise.
    pub hint: Option<&'static str>,
}

/// The thumbnailer for this app's folders and the current PATH (looked up on every call, so a
/// tool installed while Redline runs is used). `None` without a cache folder.
pub fn thumbnailer<R: Runtime>(app: &AppHandle<R>) -> Option<Thumbnailer> {
    let paths = app.path();
    Some(Thumbnailer {
        desktop_cache: paths.cache_dir().ok().map(|dir| dir.join("thumbnails")),
        own_cache: paths.app_cache_dir().ok()?.join("thumbnails"),
        search_path: std::env::var_os("PATH").unwrap_or_default(),
        timeout: thumbnail::DEFAULT_TIMEOUT,
    })
}

fn support(tools: &[(Tool, std::path::PathBuf)]) -> ThumbnailSupport {
    match tools.first() {
        Some((tool, _)) => ThumbnailSupport {
            generator: Some(tool.program()),
            hint: None,
        },
        None => ThumbnailSupport {
            generator: None,
            hint: Some(thumbnail::INSTALL_HINT),
        },
    }
}

/// The preview of the clip `id` as a data URL, or `null`. Runs on the blocking pool: it may
/// wait for an external tool (one at a time, with a timeout).
#[tauri::command]
pub async fn recording_thumbnail(app: AppHandle, id: String) -> Result<Option<String>, String> {
    let thumbnailer = thumbnailer(&app);
    gallery::with_folders(app, move |folders| {
        let path = gallery::resolve(&folders, &id)?;
        Ok(thumbnailer
            .and_then(|thumbnailer| thumbnailer.thumbnail(&path))
            .map(|image| image.data_url()))
    })
    .await
}

/// Which tool generates missing previews, or what to install.
#[tauri::command]
pub fn thumbnail_support() -> ThumbnailSupport {
    support(&thumbnail::find_tools(
        &std::env::var_os("PATH").unwrap_or_default(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn support_names_the_first_tool_or_says_what_to_install() {
        let none = serde_json::to_value(support(&[])).unwrap();
        assert_eq!(
            none,
            serde_json::json!({ "generator": null, "hint": thumbnail::INSTALL_HINT })
        );
        let both = [
            (
                Tool::Ffmpegthumbnailer,
                PathBuf::from("/bin/ffmpegthumbnailer"),
            ),
            (Tool::Ffmpeg, PathBuf::from("/bin/ffmpeg")),
        ];
        assert_eq!(
            serde_json::to_value(support(&both)).unwrap(),
            serde_json::json!({ "generator": "ffmpegthumbnailer", "hint": null })
        );
        assert_eq!(support(&both[1..]).generator, Some("ffmpeg"));
    }
}
