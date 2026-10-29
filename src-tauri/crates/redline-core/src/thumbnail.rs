//! A small preview image of a saved video, without decoding anything in-process.
//!
//! Sources, in order:
//!
//! 1. The desktop's thumbnail cache (freedesktop Thumbnail Managing Standard): `large/` then
//!    `normal/` under `$XDG_CACHE_HOME/thumbnails`, named after the MD5 of the file URI and
//!    accepted only when its `Thumb::MTime` matches the file's modification time.
//! 2. Redline's own cache: one frame written earlier by an external tool, reused while the file
//!    keeps the same modification time and size (the cache name contains both).
//! 3. An external tool found on PATH, `ffmpegthumbnailer` then `ffmpeg`: it writes one JPEG
//!    frame into Redline's cache. At most one runs at a time, each killed after `timeout`, so
//!    one clip costs at most `tools × timeout`; a caller that cannot start within
//!    [`QUEUE_WAIT`] gets `None` and may ask again later. When every tool fails, a `.fail`
//!    marker naming the tools tried is cached: the same unchanged file is not tried again with
//!    the same tools, a different set of tools tries again, and a failure that was only a
//!    timeout is retried after [`TIMEOUT_RETRY`].
//!
//! No source is `None`, never an error: a missing tool is a disabled feature.

use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

/// The longest edge of a generated thumbnail, in pixels (the standard's `large` size).
pub const THUMBNAIL_SIZE: u32 = 256;
/// How long one tool may run on one file before it is killed.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a caller waits for its turn to run a tool before giving up (nothing is cached).
pub const QUEUE_WAIT: Duration = Duration::from_secs(60);
/// A failure caused only by timeouts (a loaded machine, not a bad file) is tried again after
/// this long.
pub const TIMEOUT_RETRY: Duration = Duration::from_secs(60 * 60);
/// Images larger than this are not read: a thumbnail is a few tens of kilobytes.
const MAX_IMAGE_BYTES: u64 = 4 * 1024 * 1024;
/// The desktop cache folders that are looked at, best first.
const DESKTOP_SIZES: [&str; 2] = ["large", "normal"];
/// What the UI says when no tool can generate a preview.
pub const INSTALL_HINT: &str = "Install ffmpeg (or ffmpegthumbnailer) to see previews of clips.";

/// Only one tool runs at a time, whatever the number of callers.
static GENERATING: Mutex<()> = Mutex::new(());

/// An external tool that can write one frame of a video as an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Ffmpegthumbnailer,
    Ffmpeg,
}

impl Tool {
    /// In the order they are tried.
    pub const ALL: [Tool; 2] = [Tool::Ffmpegthumbnailer, Tool::Ffmpeg];

    pub fn program(self) -> &'static str {
        match self {
            Tool::Ffmpegthumbnailer => "ffmpegthumbnailer",
            Tool::Ffmpeg => "ffmpeg",
        }
    }
}

/// An image ready for the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub mime: &'static str,
    pub bytes: Vec<u8>,
}

impl Image {
    /// `data:<mime>;base64,<bytes>`, usable as an `<img>` source.
    pub fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.mime, base64(&self.bytes))
    }
}

/// Where thumbnails are looked for and written, and which tools may be run.
#[derive(Debug, Clone)]
pub struct Thumbnailer {
    /// `$XDG_CACHE_HOME/thumbnails` (the desktop's cache, only read), if known.
    pub desktop_cache: Option<PathBuf>,
    /// Redline's own folder, the only one written to and pruned.
    pub own_cache: PathBuf,
    /// The PATH the tools are looked up in.
    pub search_path: OsString,
    pub timeout: Duration,
}

impl Thumbnailer {
    /// The tools found on the search path, in the order they are tried.
    pub fn tools(&self) -> Vec<(Tool, PathBuf)> {
        find_tools(&self.search_path)
    }

    /// The preview of the video at `video`, or `None` when no source has one. May run an
    /// external tool for up to `timeout`: call it off the UI thread.
    pub fn thumbnail(&self, video: &Path) -> Option<Image> {
        let video = std::path::absolute(video).ok()?;
        let stamp = Stamp::of(&video)?;
        let uri = file_uri(&video);
        if let Some(image) = self
            .desktop_cache
            .as_deref()
            .and_then(|cache| desktop_thumbnail(cache, &uri, stamp.mtime_s))
        {
            return Some(image);
        }
        let key = cache_key(&uri, stamp);
        let cached = self.own_cache.join(format!("{key}.jpg"));
        if let Some(image) = read_image(&cached) {
            return Some(image);
        }
        let tools = self.tools();
        if tools.is_empty() {
            // Nothing is cached: installing a tool later makes previews appear.
            return None;
        }
        let failed = self.own_cache.join(format!("{key}.fail"));
        let tried = tool_names(&tools);
        if failure_is_cached(&failed, &tried) {
            return None;
        }
        let _one_at_a_time = lock_with_deadline(&GENERATING, QUEUE_WAIT)?;
        // Another caller may have generated it, or failed, while this one waited.
        if let Some(image) = read_image(&cached) {
            return Some(image);
        }
        if failure_is_cached(&failed, &tried) {
            return None;
        }
        fs::create_dir_all(&self.own_cache).ok()?;
        let partial = self.own_cache.join(format!("{key}.part.jpg"));
        let seek_s = crate::media::duration_seconds(&video).map(|seconds| seconds / 10.0);
        let mut only_timeouts = true;
        for (tool, program) in tools {
            let _ = fs::remove_file(&partial);
            let mut command = generate_command(tool, &program, &video, &partial, seek_s);
            match run_with_timeout(&mut command, self.timeout) {
                Run::Succeeded => {
                    if let Some(image) = read_image(&partial)
                        && image.mime == "image/jpeg"
                        && fs::rename(&partial, &cached).is_ok()
                    {
                        return Some(image);
                    }
                    only_timeouts = false;
                }
                Run::Failed => only_timeouts = false,
                Run::TimedOut => {}
            }
        }
        let _ = fs::remove_file(&partial);
        let _ = fs::write(&failed, failure_marker(&tried, only_timeouts));
        None
    }

    /// Removes from Redline's cache every thumbnail and failure marker that does not belong to
    /// one of `videos` as they are now (deleted files, and files changed since). Never touches
    /// the desktop's cache.
    pub fn prune(&self, videos: &[PathBuf]) {
        let keep: Vec<String> = videos
            .iter()
            .filter_map(|video| {
                let video = std::path::absolute(video).ok()?;
                Some(cache_key(&file_uri(&video), Stamp::of(&video)?))
            })
            .collect();
        let Ok(listing) = fs::read_dir(&self.own_cache) else {
            return;
        };
        for entry in listing.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let key = name.split('.').next().unwrap_or_default();
            let ours = name.ends_with(".jpg") || name.ends_with(".fail");
            if ours && !keep.iter().any(|kept| kept == key) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// The names of `tools`, as a failure marker records them.
fn tool_names(tools: &[(Tool, PathBuf)]) -> String {
    tools
        .iter()
        .map(|(tool, _)| tool.program())
        .collect::<Vec<_>>()
        .join(",")
}

/// The content of a failure marker: the tools tried and whether they all only timed out.
fn failure_marker(tried: &str, only_timeouts: bool) -> String {
    format!("tools={tried}\ntimeout={}\n", u8::from(only_timeouts))
}

/// A failure marker at `failed` that still applies: written for the same tools, and either a
/// real failure or a timeout younger than [`TIMEOUT_RETRY`].
fn failure_is_cached(failed: &Path, tried: &str) -> bool {
    let Ok(marker) = fs::read_to_string(failed) else {
        return false;
    };
    if !marker.lines().any(|line| line == format!("tools={tried}")) {
        return false;
    }
    if !marker.lines().any(|line| line == "timeout=1") {
        return true;
    }
    fs::metadata(failed)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|written| written.elapsed().ok())
        .is_some_and(|age| age < TIMEOUT_RETRY)
}

/// `mutex`, waiting at most `wait` for it; `None` when the queue is too long.
fn lock_with_deadline(mutex: &Mutex<()>, wait: Duration) -> Option<std::sync::MutexGuard<'_, ()>> {
    let deadline = Instant::now() + wait;
    loop {
        match mutex.try_lock() {
            Ok(guard) => return Some(guard),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => return Some(poisoned.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(std::sync::TryLockError::WouldBlock) => return None,
        }
    }
}

/// What identifies one version of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    mtime_s: u64,
    mtime_ms: u64,
    size: u64,
}

impl Stamp {
    fn of(path: &Path) -> Option<Self> {
        let metadata = fs::metadata(path).ok()?;
        if !metadata.is_file() {
            return None;
        }
        let elapsed = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
        Some(Self {
            mtime_s: elapsed.as_secs(),
            mtime_ms: u64::try_from(elapsed.as_millis()).ok()?,
            size: metadata.len(),
        })
    }
}

/// The name of a file in Redline's cache: the MD5 of the URI, the modification time and the
/// size. A changed file gets a new name, and the old one is pruned.
fn cache_key(uri: &str, stamp: Stamp) -> String {
    format!(
        "{}-{}-{}",
        md5_hex(uri.as_bytes()),
        stamp.mtime_ms,
        stamp.size
    )
}

/// The desktop cache's thumbnail of `uri`, if one exists for the current modification time.
fn desktop_thumbnail(cache: &Path, uri: &str, mtime_s: u64) -> Option<Image> {
    let name = format!("{}.png", md5_hex(uri.as_bytes()));
    DESKTOP_SIZES.iter().find_map(|size| {
        let image = read_image(&cache.join(size).join(&name))?;
        let text = png_text(&image.bytes)?;
        let fresh = text.mtime.as_deref().and_then(parse_mtime) == Some(mtime_s)
            && text.uri.as_deref().is_none_or(|stored| stored == uri);
        fresh.then_some(image)
    })
}

/// `Thumb::MTime` is integer seconds; some writers add a fraction.
fn parse_mtime(text: &str) -> Option<u64> {
    text.split('.').next()?.trim().parse().ok()
}

/// A PNG or JPEG file no larger than [`MAX_IMAGE_BYTES`], recognised by its first bytes.
fn read_image(path: &Path) -> Option<Image> {
    let file = fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_IMAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return None;
    }
    let mime = if bytes.starts_with(PNG_SIGNATURE) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else {
        return None;
    };
    Some(Image { mime, bytes })
}

/// The command that writes one JPEG frame of `video` to `out`, scaled to [`THUMBNAIL_SIZE`].
/// `seek_s` is where to take it (a tenth of the length when known).
fn generate_command(
    tool: Tool,
    program: &Path,
    video: &Path,
    out: &Path,
    seek_s: Option<f64>,
) -> Command {
    let mut command = Command::new(program);
    match tool {
        Tool::Ffmpegthumbnailer => {
            command
                .arg("-i")
                .arg(video)
                .arg("-o")
                .arg(out)
                // No `-t`: the default seek is already 10% of the length.
                .args(["-s", &THUMBNAIL_SIZE.to_string(), "-c", "jpeg", "-q", "8"]);
        }
        Tool::Ffmpeg => {
            let seek = format!("{:.3}", seek_s.unwrap_or(0.0));
            command
                .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y"])
                .args(["-ss", &seek, "-i"])
                .arg(video)
                .args(["-an", "-frames:v", "1", "-f", "image2", "-c:v", "mjpeg"])
                .args(["-q:v", "5", "-vf"])
                .arg(format!(
                    "scale='min({THUMBNAIL_SIZE},iw)':'min({THUMBNAIL_SIZE},ih)':force_original_aspect_ratio=decrease"
                ))
                .arg(out);
        }
    }
    command
}

/// How a tool run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Run {
    Succeeded,
    /// It could not start, or exited with an error.
    Failed,
    /// Killed after the timeout.
    TimedOut,
}

/// Runs `command` with no input or output, for at most `timeout`; after that it is killed and
/// reaped.
pub fn run_with_timeout(command: &mut Command, timeout: Duration) -> Run {
    let Ok(mut child) = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Run::Failed;
    };
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Run::Succeeded,
            Ok(Some(_)) => return Run::Failed,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            outcome => {
                let _ = child.kill();
                let _ = child.wait();
                return if outcome.is_ok() {
                    Run::TimedOut
                } else {
                    Run::Failed
                };
            }
        }
    }
}

/// The tools found in the folders of `search_path`, in the order they are tried.
pub fn find_tools(search_path: &OsStr) -> Vec<(Tool, PathBuf)> {
    Tool::ALL
        .into_iter()
        .filter_map(|tool| find_program(tool.program(), search_path).map(|path| (tool, path)))
        .collect()
}

/// The first executable regular file called `program` in the folders of `search_path`.
pub fn find_program(program: &str, search_path: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(search_path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program))
        .find(|candidate| {
            fs::metadata(candidate)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

/// `file://` and the absolute path, escaped as GLib's `g_filename_to_uri` does: the thumbnail
/// cache names files after the MD5 of exactly this string.
pub fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"!$&'()*+,-./:=@_~".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    uri
}

const PNG_SIGNATURE: &[u8] = &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];

/// The thumbnail keys of a PNG's `tEXt` chunks.
#[derive(Debug, Default, PartialEq, Eq)]
struct PngText {
    uri: Option<String>,
    mtime: Option<String>,
}

/// Reads `Thumb::URI` and `Thumb::MTime` from the `tEXt` chunks; `None` if `png` is not a PNG.
fn png_text(png: &[u8]) -> Option<PngText> {
    let mut rest = png.strip_prefix(PNG_SIGNATURE)?;
    let mut text = PngText::default();
    while rest.len() >= 12 {
        let len = usize::try_from(u32::from_be_bytes(rest[..4].try_into().ok()?)).ok()?;
        let kind = &rest[4..8];
        let data = rest.get(8..8usize.checked_add(len)?)?;
        if kind == b"tEXt"
            && let Some(split) = data.iter().position(|&byte| byte == 0)
        {
            // tEXt is Latin-1; the two keys hold ASCII.
            let value = String::from_utf8_lossy(&data[split + 1..]).into_owned();
            match &data[..split] {
                b"Thumb::URI" => text.uri = Some(value),
                b"Thumb::MTime" => text.mtime = Some(value),
                _ => {}
            }
        }
        if kind == b"IEND" {
            break;
        }
        rest = rest.get(12 + len..)?;
    }
    Some(text)
}

/// Standard base64 with padding.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The MD5 of `data` as 32 lowercase hex digits (RFC 1321). Used only to compute the names the
/// thumbnail standard gives to files, never for security.
pub fn md5_hex(data: &[u8]) -> String {
    md5(data)
        .iter()
        .fold(String::with_capacity(32), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

fn md5(data: &[u8]) -> [u8; 16] {
    const SHIFTS: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76a_a478,
        0xe8c7_b756,
        0x2420_70db,
        0xc1bd_ceee,
        0xf57c_0faf,
        0x4787_c62a,
        0xa830_4613,
        0xfd46_9501,
        0x6980_98d8,
        0x8b44_f7af,
        0xffff_5bb1,
        0x895c_d7be,
        0x6b90_1122,
        0xfd98_7193,
        0xa679_438e,
        0x49b4_0821,
        0xf61e_2562,
        0xc040_b340,
        0x265e_5a51,
        0xe9b6_c7aa,
        0xd62f_105d,
        0x0244_1453,
        0xd8a1_e681,
        0xe7d3_fbc8,
        0x21e1_cde6,
        0xc337_07d6,
        0xf4d5_0d87,
        0x455a_14ed,
        0xa9e3_e905,
        0xfcef_a3f8,
        0x676f_02d9,
        0x8d2a_4c8a,
        0xfffa_3942,
        0x8771_f681,
        0x6d9d_6122,
        0xfde5_380c,
        0xa4be_ea44,
        0x4bde_cfa9,
        0xf6bb_4b60,
        0xbebf_bc70,
        0x289b_7ec6,
        0xeaa1_27fa,
        0xd4ef_3085,
        0x0488_1d05,
        0xd9d4_d039,
        0xe6db_99e5,
        0x1fa2_7cf8,
        0xc4ac_5665,
        0xf429_2244,
        0x432a_ff97,
        0xab94_23a7,
        0xfc93_a039,
        0x655b_59c3,
        0x8f0c_cc92,
        0xffef_f47d,
        0x8584_5dd1,
        0x6fa8_7e4f,
        0xfe2c_e6e0,
        0xa301_4314,
        0x4e08_11a1,
        0xf753_7e82,
        0xbd3a_f235,
        0x2ad7_d2bb,
        0xeb86_d391,
    ];
    let mut message = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_le_bytes());

    let mut state: [u32; 4] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];
    for block in message.chunks_exact(64) {
        let words: Vec<u32> = block
            .chunks_exact(4)
            .map(|word| u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
            .collect();
        let [mut a, mut b, mut c, mut d] = state;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let rotated = a
                .wrapping_add(f)
                .wrapping_add(K[i])
                .wrapping_add(words[g])
                .rotate_left(SHIFTS[i]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(rotated);
        }
        for (word, add) in state.iter_mut().zip([a, b, c, d]) {
            *word = word.wrapping_add(add);
        }
    }
    let mut digest = [0u8; 16];
    for (out, word) in digest.chunks_exact_mut(4).zip(state) {
        out.copy_from_slice(&word.to_le_bytes());
    }
    digest
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{File, FileTimes};
    use std::time::SystemTime;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("redline-thumbnail-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn set_mtime(path: &Path, seconds: u64) {
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(
                FileTimes::new()
                    .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)),
            )
            .unwrap();
    }

    fn video(dir: &Path, name: &str, seconds: u64) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, b"not really a video").unwrap();
        set_mtime(&path, seconds);
        path
    }

    /// A shell script called `name` in `bin`.
    fn tool(bin: &Path, name: &str, script: &str) {
        fs::create_dir_all(bin).unwrap();
        let path = bin.join(name);
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Fake ffmpegthumbnailer: writes the fake JPEG to the argument after `-o` and counts runs.
    fn fake_ffmpegthumbnailer(bin: &Path, log: &Path) {
        tool(
            bin,
            "ffmpegthumbnailer",
            &format!(
                "echo run >> '{}'\nwhile [ $# -gt 0 ]; do [ \"$1\" = -o ] && out=$2; shift; done\nprintf '\\377\\330\\377\\340' > \"$out\"",
                log.display()
            ),
        );
    }

    fn thumbnailer(root: &Path, bin: &Path) -> Thumbnailer {
        Thumbnailer {
            desktop_cache: Some(root.join("xdg-thumbnails")),
            own_cache: root.join("own"),
            search_path: OsString::from(format!("{}:relative/bin", bin.display())),
            timeout: Duration::from_secs(5),
        }
    }

    fn runs(log: &Path) -> usize {
        fs::read_to_string(log).map_or(0, |text| text.lines().count())
    }

    fn png_with_text(pairs: &[(&str, &str)]) -> Vec<u8> {
        let mut png = PNG_SIGNATURE.to_vec();
        let mut chunk = |kind: &[u8], data: &[u8]| {
            png.extend_from_slice(&(data.len() as u32).to_be_bytes());
            png.extend_from_slice(kind);
            png.extend_from_slice(data);
            png.extend_from_slice(&[0, 0, 0, 0]); // The CRC is not checked.
        };
        chunk(b"IHDR", &[0; 13]);
        for (key, value) in pairs {
            chunk(b"tEXt", format!("{key}\0{value}").as_bytes());
        }
        chunk(b"IDAT", &[0; 4]);
        chunk(b"IEND", &[]);
        png
    }

    #[test]
    fn md5_matches_the_rfc_vectors() {
        let vectors = [
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            (
                "abcdefghijklmnopqrstuvwxyz",
                "c3fcd3d76192e4007dfb496cca67e13b",
            ),
            (
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ];
        for (input, expected) in vectors {
            assert_eq!(md5_hex(input.as_bytes()), expected, "{input:?}");
        }
    }

    #[test]
    fn the_desktop_cache_name_is_the_md5_of_the_glib_uri() {
        // Expected values from Python's hashlib and GLib's g_filename_to_uri.
        assert_eq!(
            md5_hex(file_uri(Path::new("/clips/Replay_a.mkv")).as_bytes()),
            "eea8b8623e724237f148e7d59d43bf97"
        );
        assert_eq!(
            file_uri(Path::new("/clips/Replay 1;#?%é.mkv")),
            "file:///clips/Replay%201%3B%23%3F%25%C3%A9.mkv"
        );
        assert_eq!(
            file_uri(Path::new("/clips/a-b_c.d~(1)+@,=$&'!*:x.mkv")),
            "file:///clips/a-b_c.d~(1)+@,=$&'!*:x.mkv"
        );
    }

    #[test]
    fn base64_pads_like_the_standard() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), expected);
        }
        let image = Image {
            mime: "image/png",
            bytes: b"foo".to_vec(),
        };
        assert_eq!(image.data_url(), "data:image/png;base64,Zm9v");
    }

    #[test]
    fn png_text_reads_the_thumbnail_keys() {
        let png = png_with_text(&[
            ("Software", "test"),
            ("Thumb::URI", "file:///a.mkv"),
            ("Thumb::MTime", "1234"),
        ]);
        assert_eq!(
            png_text(&png),
            Some(PngText {
                uri: Some("file:///a.mkv".into()),
                mtime: Some("1234".into()),
            })
        );
        assert_eq!(png_text(b"not a png"), None);
        // A chunk claiming more bytes than the file has stops the reading.
        let mut truncated = png_with_text(&[]);
        truncated.truncate(20);
        assert_eq!(png_text(&truncated), None);
        assert_eq!(parse_mtime("1234.5"), Some(1234));
        assert_eq!(parse_mtime("soon"), None);
    }

    #[test]
    fn a_desktop_thumbnail_is_used_only_while_the_file_is_unchanged() {
        let root = temp_dir("desktop");
        let clip = video(&root, "Replay_a.mkv", 1_000);
        let uri = file_uri(&clip);
        let name = format!("{}.png", md5_hex(uri.as_bytes()));
        let large = root.join("xdg-thumbnails/large");
        fs::create_dir_all(&large).unwrap();
        fs::write(
            large.join(&name),
            png_with_text(&[("Thumb::URI", &uri), ("Thumb::MTime", "1000")]),
        )
        .unwrap();
        // No tool on the path: only the desktop cache can answer.
        let thumbs = thumbnailer(&root, &root.join("empty-bin"));

        let image = thumbs.thumbnail(&clip).unwrap();
        assert_eq!(image.mime, "image/png");

        // The file changed after the thumbnail was made: stale.
        set_mtime(&clip, 2_000);
        assert_eq!(thumbs.thumbnail(&clip), None);

        // A fresh one in `normal` is found when `large` is stale.
        let normal = root.join("xdg-thumbnails/normal");
        fs::create_dir_all(&normal).unwrap();
        fs::write(
            normal.join(&name),
            png_with_text(&[("Thumb::URI", &uri), ("Thumb::MTime", "2000")]),
        )
        .unwrap();
        assert!(thumbs.thumbnail(&clip).is_some());

        // A thumbnail for another URI under the same name is not trusted.
        fs::write(
            normal.join(&name),
            png_with_text(&[
                ("Thumb::URI", "file:///other.mkv"),
                ("Thumb::MTime", "2000"),
            ]),
        )
        .unwrap();
        assert_eq!(thumbs.thumbnail(&clip), None);
        // Nothing was written to Redline's cache without a tool.
        assert!(!root.join("own").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn tools_are_found_in_order_on_the_search_path() {
        let root = temp_dir("path");
        let first = root.join("first");
        let second = root.join("second");
        tool(&second, "ffmpeg", "exit 0");
        tool(&first, "ffmpegthumbnailer", "exit 0");
        // Not executable: skipped.
        fs::write(first.join("ffmpeg"), "").unwrap();
        let path = std::env::join_paths([&first, &second]).unwrap();
        assert_eq!(find_program("ffmpeg", &path), Some(second.join("ffmpeg")));
        let thumbs = Thumbnailer {
            search_path: path,
            ..thumbnailer(&root, &first)
        };
        assert_eq!(
            thumbs.tools(),
            [
                (Tool::Ffmpegthumbnailer, first.join("ffmpegthumbnailer")),
                (Tool::Ffmpeg, second.join("ffmpeg")),
            ]
        );
        assert_eq!(find_program("ffmpeg", OsStr::new("")), None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_generated_thumbnail_is_reused_until_the_file_changes() {
        let root = temp_dir("generate");
        let bin = root.join("bin");
        let log = root.join("runs.log");
        fake_ffmpegthumbnailer(&bin, &log);
        let thumbs = thumbnailer(&root, &bin);
        let clip = video(&root, "Replay_a.mkv", 1_000);

        let image = thumbs.thumbnail(&clip).unwrap();
        assert_eq!(image.mime, "image/jpeg");
        assert_eq!(runs(&log), 1);
        assert!(thumbs.thumbnail(&clip).is_some());
        assert_eq!(runs(&log), 1, "an unchanged file reuses the cache");

        set_mtime(&clip, 2_000);
        assert!(thumbs.thumbnail(&clip).is_some());
        assert_eq!(runs(&log), 2, "a changed file is generated again");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_next_tool_is_tried_and_a_failure_is_cached() {
        let root = temp_dir("failure");
        let bin = root.join("bin");
        let log = root.join("runs.log");
        // ffmpegthumbnailer fails; ffmpeg writes the image as its last argument.
        tool(
            &bin,
            "ffmpegthumbnailer",
            &format!("echo t >> '{}'; exit 1", log.display()),
        );
        tool(
            &bin,
            "ffmpeg",
            &format!(
                "echo f >> '{}'\nfor last; do :; done\nprintf '\\377\\330\\377\\340' > \"$last\"",
                log.display()
            ),
        );
        let thumbs = thumbnailer(&root, &bin);
        let clip = video(&root, "Replay_a.mkv", 1_000);
        assert_eq!(thumbs.thumbnail(&clip).unwrap().mime, "image/jpeg");
        assert_eq!(runs(&log), 2);

        // Both fail: one attempt, then the cached failure answers.
        tool(
            &bin,
            "ffmpeg",
            &format!("echo f >> '{}'; exit 1", log.display()),
        );
        let other = video(&root, "Replay_b.mkv", 1_000);
        assert_eq!(thumbs.thumbnail(&other), None);
        assert_eq!(runs(&log), 4);
        assert_eq!(thumbs.thumbnail(&other), None);
        assert_eq!(runs(&log), 4, "a cached failure is not retried");
        // Another set of tools tries again.
        fs::remove_file(bin.join("ffmpegthumbnailer")).unwrap();
        assert_eq!(thumbs.thumbnail(&other), None);
        assert_eq!(runs(&log), 5, "a failure with other tools does not apply");
        assert_eq!(thumbs.thumbnail(&other), None);
        assert_eq!(runs(&log), 5);
        // No partial file is left behind.
        let leftovers: Vec<_> = fs::read_dir(root.join("own"))
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().contains(".part"))
            .collect();
        assert!(leftovers.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_tool_that_hangs_is_killed_after_the_timeout() {
        let root = temp_dir("timeout");
        let bin = root.join("bin");
        let log = root.join("runs.log");
        tool(
            &bin,
            "ffmpegthumbnailer",
            &format!("echo h >> '{}'\nexec sleep 30", log.display()),
        );
        let thumbs = Thumbnailer {
            timeout: Duration::from_millis(200),
            ..thumbnailer(&root, &bin)
        };
        let clip = video(&root, "Replay_a.mkv", 1_000);
        let started = Instant::now();
        assert_eq!(thumbs.thumbnail(&clip), None);
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(runs(&log), 1);
        // A recent timeout is not retried on every call...
        assert_eq!(thumbs.thumbnail(&clip), None);
        assert_eq!(runs(&log), 1);
        // ...but an old one is: a timeout says more about the machine than about the file.
        let marker = fs::read_dir(root.join("own"))
            .unwrap()
            .flatten()
            .find(|entry| entry.file_name().to_string_lossy().ends_with(".fail"))
            .unwrap()
            .path();
        assert!(fs::read_to_string(&marker).unwrap().contains("timeout=1"));
        let old = SystemTime::now() - TIMEOUT_RETRY - Duration::from_secs(60);
        File::options()
            .write(true)
            .open(&marker)
            .unwrap()
            .set_times(FileTimes::new().set_modified(old))
            .unwrap();
        assert_eq!(thumbs.thumbnail(&clip), None);
        assert_eq!(runs(&log), 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_long_queue_gives_up_instead_of_waiting_forever() {
        let mutex = Mutex::new(());
        let held = mutex.lock().unwrap();
        let started = Instant::now();
        assert!(lock_with_deadline(&mutex, Duration::from_millis(150)).is_none());
        assert!(started.elapsed() >= Duration::from_millis(150));
        drop(held);
        assert!(lock_with_deadline(&mutex, Duration::from_millis(150)).is_some());
    }

    #[test]
    fn ffmpegthumbnailer_keeps_its_default_seek() {
        let command = generate_command(
            Tool::Ffmpegthumbnailer,
            Path::new("/bin/ffmpegthumbnailer"),
            Path::new("/clips/a.mkv"),
            Path::new("/cache/a.jpg"),
            Some(12.0),
        );
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect();
        assert_eq!(
            args,
            [
                "-i",
                "/clips/a.mkv",
                "-o",
                "/cache/a.jpg",
                "-s",
                "256",
                "-c",
                "jpeg",
                "-q",
                "8"
            ]
        );
    }

    #[test]
    fn without_a_tool_nothing_is_cached() {
        let root = temp_dir("no-tool");
        let thumbs = thumbnailer(&root, &root.join("empty-bin"));
        let clip = video(&root, "Replay_a.mkv", 1_000);
        assert!(thumbs.tools().is_empty());
        assert_eq!(thumbs.thumbnail(&clip), None);
        assert!(!root.join("own").exists());
        // Installing a tool afterwards makes the preview appear.
        fake_ffmpegthumbnailer(&root.join("empty-bin"), &root.join("runs.log"));
        assert!(thumbs.thumbnail(&clip).is_some());
        assert_eq!(thumbs.thumbnail(&root.join("missing.mkv")), None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn pruning_removes_the_thumbnails_of_deleted_and_changed_files() {
        let root = temp_dir("prune");
        let bin = root.join("bin");
        fake_ffmpegthumbnailer(&bin, &root.join("runs.log"));
        let thumbs = thumbnailer(&root, &bin);
        let kept = video(&root, "kept.mkv", 1_000);
        let changed = video(&root, "changed.mkv", 1_000);
        let deleted = video(&root, "deleted.mkv", 1_000);
        for clip in [&kept, &changed, &deleted] {
            assert!(thumbs.thumbnail(clip).is_some());
        }
        let desktop = root.join("xdg-thumbnails/large");
        fs::create_dir_all(&desktop).unwrap();
        fs::write(desktop.join("someone-else.png"), b"x").unwrap();
        fs::write(root.join("own/unrelated.txt"), b"x").unwrap();
        set_mtime(&changed, 2_000);
        fs::remove_file(&deleted).unwrap();

        thumbs.prune(&[kept.clone(), changed.clone()]);
        let mut left: Vec<String> = fs::read_dir(root.join("own"))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        let kept_key = cache_key(&file_uri(&kept), Stamp::of(&kept).unwrap());
        assert_eq!(
            left,
            [format!("{kept_key}.jpg"), "unrelated.txt".to_owned()]
        );
        // The desktop's cache is never touched.
        assert!(desktop.join("someone-else.png").exists());
        let _ = fs::remove_dir_all(root);
    }
}
