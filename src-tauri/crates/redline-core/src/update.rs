//! Whether a newer Redline has been published (D14): the latest GitHub release, read through
//! `curl` as an external process, compared with the running version.
//!
//! Redline notifies and links, it never downloads or installs a release. The URLs are constants:
//! nothing read from the network is opened or run, only `tag_name` is read from the answer and
//! only as three numbers. `releases/latest` excludes drafts and prereleases, and a tag that is
//! not a plain `major.minor.patch` (`v0.3.0-rc1`) is an unreadable answer, never an update.
//!
//! Every way a check can fail is a [`CheckFailure`], not an error for the user: without `curl`
//! the feature is off (nothing is started), offline or rate limited it reports the outcome and
//! tries again at the next check. `curl` runs in the C locale, ignores the user's `curlrc`, has
//! a limit of its own and is killed after a limit of ours; its output is bounded.

use serde::Deserialize;
use std::ffi::OsStr;
use std::fmt;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::thumbnail::find_program;

/// The latest published release. Fixed: the host, the repository and the scheme never change.
pub const RELEASES_API: &str = "https://api.github.com/repos/FabioSM46/redline/releases/latest";
/// The page `open_latest_release` opens, instead of the `html_url` of the answer.
pub const RELEASE_PAGE: &str = "https://github.com/FabioSM46/redline/releases/latest";
/// How long `curl` is left running before it is killed. `curl` gives up first, by itself, after
/// `CURL_MAX_SECONDS`.
pub const KILL_AFTER: Duration = Duration::from_secs(25);

const CURL: &str = "curl";
const CURL_MAX_SECONDS: &str = "20";
/// `CURLE_HTTP_RETURNED_ERROR`: `--fail` met a status of 400 or above, as a rate limit answers.
const CURL_HTTP_ERROR: i32 = 22;
/// Far more than a release with long notes; past it the answer is not read.
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// A plain `major.minor.patch`. The order is the numeric one, field by field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl Version {
    /// Reads `0.2.1` or `v0.2.1`. Anything else is `None`: a suffix such as `-rc1`, a missing or
    /// extra number, a sign, a leading zero.
    pub fn parse(text: &str) -> Option<Self> {
        let mut numbers = text.strip_prefix('v').unwrap_or(text).split('.');
        let version = Self {
            major: number(numbers.next()?)?,
            minor: number(numbers.next()?)?,
            patch: number(numbers.next()?)?,
        };
        numbers.next().is_none().then_some(version)
    }
}

fn number(text: &str) -> Option<u64> {
    let digits = !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    let padded = text.len() > 1 && text.starts_with('0');
    if digits && !padded {
        text.parse().ok()
    } else {
        None
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Whether `latest` is an update for the running version `current`: strictly newer. An unreadable
/// `current` is never updated, so a build with a suffix reports nothing.
pub fn is_update(current: &str, latest: Version) -> bool {
    Version::parse(current).is_some_and(|current| latest > current)
}

/// Why a check found no version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckFailure {
    /// No `curl` on the search path: nothing was started.
    CurlMissing,
    /// No answer: offline, no name resolution, a TLS failure, a timeout.
    Unreachable,
    /// GitHub answered with an HTTP error, such as the unauthenticated rate limit.
    Refused,
    /// An answer that is too long, is not JSON, or has no readable `tag_name`.
    BadResponse,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
}

/// The latest published version. Blocking: up to `kill_after`.
pub fn latest_release(search_path: &OsStr, kill_after: Duration) -> Result<Version, CheckFailure> {
    let curl = find_program(CURL, search_path).ok_or(CheckFailure::CurlMissing)?;
    parse_release(&fetch(&curl, kill_after)?)
}

fn parse_release(body: &[u8]) -> Result<Version, CheckFailure> {
    serde_json::from_slice::<Release>(body)
        .ok()
        .and_then(|release| Version::parse(&release.tag_name))
        .ok_or(CheckFailure::BadResponse)
}

/// Runs `curl` as `run_outcome` in the app's `dependencies` runs a tool (C locale, reading while
/// waiting, killed and reaped after the limit), plus a bound on the output and the exit code,
/// which tells a refusal from no answer. A bound that closes the pipe makes `curl` fail, so the
/// length is judged before the exit status.
fn fetch(curl: &Path, kill_after: Duration) -> Result<Vec<u8>, CheckFailure> {
    let mut child = Command::new(curl)
        // `--disable` works only as the first argument. A `curlrc` must not change this request.
        .args(["--disable", "--silent", "--fail", "--proto", "=https"])
        .args(["--max-time", CURL_MAX_SECONDS])
        .args(["--header", "Accept: application/vnd.github+json"])
        .arg(RELEASES_API)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| CheckFailure::Unreachable)?;
    let stdout = child.stdout.take().ok_or(CheckFailure::Unreachable)?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut body = Vec::new();
        let read = stdout
            .take(MAX_RESPONSE_BYTES as u64 + 1)
            .read_to_end(&mut body);
        let _ = sender.send(read.ok().map(|_| body));
    });
    let deadline = Instant::now() + kill_after;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CheckFailure::Unreachable);
            }
        }
    };
    let left = deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(100);
    let body = receiver.recv_timeout(left).ok().flatten();
    if body
        .as_ref()
        .is_some_and(|body| body.len() > MAX_RESPONSE_BYTES)
    {
        return Err(CheckFailure::BadResponse);
    }
    match (status.success(), status.code(), body) {
        (true, _, Some(body)) => Ok(body),
        (false, Some(CURL_HTTP_ERROR), _) => Err(CheckFailure::Refused),
        _ => Err(CheckFailure::Unreachable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::ErrorKind;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    const LIMIT: Duration = Duration::from_secs(10);

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    struct Bin(PathBuf);

    impl Bin {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("redline-update-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Bin(dir)
        }

        /// A fake `curl` on this search path. `--probe` waits out ETXTBSY: a test thread that
        /// forks while the file is open for writing makes exec fail until that child execs.
        fn curl(&self, body: &str) -> &Self {
            let path = self.0.join(CURL);
            let script = format!("#!/bin/sh\n[ \"$1\" = --probe ] && exit 0\n{body}\n");
            fs::write(&path, script).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            let deadline = Instant::now() + LIMIT;
            loop {
                match Command::new(&path).arg("--probe").status() {
                    Err(error) if error.kind() == ErrorKind::ExecutableFileBusy => {
                        assert!(Instant::now() < deadline, "the fake curl stays busy");
                        thread::sleep(Duration::from_millis(10));
                    }
                    _ => break,
                }
            }
            self
        }

        fn check(&self, limit: Duration) -> Result<Version, CheckFailure> {
            latest_release(self.0.as_os_str(), limit)
        }
    }

    impl Drop for Bin {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn answering(name: &str, json: &str) -> Result<Version, CheckFailure> {
        let bin = Bin::new(name);
        bin.curl(&format!("cat <<'JSON'\n{json}\nJSON"));
        bin.check(LIMIT)
    }

    #[test]
    fn a_version_is_three_plain_numbers_with_an_optional_v() {
        assert_eq!(v("v0.2.1"), v("0.2.1"));
        assert_eq!(v("10.20.30").to_string(), "10.20.30");
        for bad in [
            "",
            "v",
            "0.2",
            "0.2.1.4",
            "v0.3.0-rc1",
            "0.3.0+build",
            "V0.2.1",
            "vv0.2.1",
            "+1.2.3",
            "1..3",
            "01.2.3",
            " 0.2.1",
            "0.2.1\n",
            "latest",
            "99999999999999999999.0.0",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn only_a_strictly_newer_version_is_an_update() {
        assert!(is_update("0.2.1", v("0.2.2")));
        assert!(is_update("0.2.9", v("0.3.0")));
        assert!(is_update("0.9.9", v("1.0.0")));
        // Numbers, not text.
        assert!(is_update("0.9.0", v("0.10.0")));
        assert!(!is_update("0.2.1", v("0.2.1")));
        assert!(!is_update("0.3.0", v("0.2.9")));
        assert!(!is_update("0.3.0-dev", v("9.0.0")));
        assert!(!is_update("", v("9.0.0")));
    }

    #[test]
    fn the_tag_of_the_answer_is_the_version() {
        let json = r#"{"tag_name": "v0.3.0", "html_url": "https://example.com/x", "assets": []}"#;
        assert_eq!(answering("tag", json), Ok(v("0.3.0")));
    }

    #[test]
    fn an_unreadable_answer_is_a_bad_response() {
        for (name, json) in [
            ("prerelease", r#"{"tag_name": "v0.3.0-rc1"}"#),
            ("garbage-tag", r#"{"tag_name": "nightly"}"#),
            ("number-tag", r#"{"tag_name": 3}"#),
            ("no-tag", r#"{"message": "Not Found"}"#),
            ("html", "<html>a captive portal</html>"),
            ("empty", ""),
        ] {
            assert_eq!(
                answering(name, json),
                Err(CheckFailure::BadResponse),
                "{name}"
            );
        }
    }

    #[test]
    fn no_curl_starts_nothing() {
        let bin = Bin::new("missing");
        assert_eq!(bin.check(LIMIT), Err(CheckFailure::CurlMissing));
    }

    #[test]
    fn an_http_error_is_a_refusal_and_other_failures_no_answer() {
        let bin = Bin::new("exit");
        bin.curl("exit 22");
        assert_eq!(bin.check(LIMIT), Err(CheckFailure::Refused));
        bin.curl("exit 6");
        assert_eq!(bin.check(LIMIT), Err(CheckFailure::Unreachable));
    }

    #[test]
    fn a_curl_that_hangs_is_killed_at_the_limit() {
        let bin = Bin::new("hang");
        bin.curl("exec sleep 30");
        let started = Instant::now();
        assert_eq!(
            bin.check(Duration::from_millis(100)),
            Err(CheckFailure::Unreachable)
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn an_oversized_answer_is_not_read_to_the_end() {
        let bin = Bin::new("large");
        bin.curl("printf '{\"tag_name\": \"v9.0.0\", \"x\": \"'\nexec yes");
        assert_eq!(bin.check(LIMIT), Err(CheckFailure::BadResponse));
    }

    #[test]
    fn the_request_is_fixed_https_with_a_time_limit_and_no_curlrc() {
        let bin = Bin::new("args");
        let log = bin.0.join("args");
        bin.curl(&format!(
            "printf '%s\\n' \"$LC_ALL\" \"$@\" > '{}'\necho '{{\"tag_name\":\"v1.0.0\"}}'",
            log.display()
        ));
        assert_eq!(bin.check(LIMIT), Ok(v("1.0.0")));
        let text = fs::read_to_string(log).unwrap();
        let args: Vec<_> = text.lines().collect();
        assert_eq!(args[0], "C");
        assert_eq!(args[1], "--disable");
        assert_eq!(args.last(), Some(&RELEASES_API));
        assert!(args.contains(&"--fail"));
        assert!(args.windows(2).any(|pair| pair == ["--proto", "=https"]));
        assert!(args.windows(2).any(|pair| pair[0] == "--max-time"));
    }
}
