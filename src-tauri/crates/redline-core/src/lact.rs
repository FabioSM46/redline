//! Minimal client for the LACT daemon (`lactd`).
//!
//! Protocol: Unix socket, one JSON request per line, one JSON response per line.
//! Request:  `{"command": "<snake_case>", "args": {...}}`
//! Response: `{"status": "ok", "data": ...}` or `{"status": "error", "data": ...}`
//! Reference: lact-schema/src/request.rs in the LACT repository (v0.10.1).

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

pub const DEFAULT_SOCKET: &str = "/run/lactd.sock";

pub struct LactClient {
    socket: PathBuf,
}

impl Default for LactClient {
    fn default() -> Self {
        Self::new(DEFAULT_SOCKET)
    }
}

impl LactClient {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
        }
    }

    /// Sends a command and returns the `data` field of the response.
    pub fn request(&self, command: &str, args: Option<Value>) -> Result<Value> {
        let mut req = json!({ "command": command });
        if let Some(args) = args {
            req["args"] = args;
        }

        let mut stream = UnixStream::connect(&self.socket).with_context(|| {
            format!(
                "connecting to {} failed: is lactd running and are you in its admin group (sudo)?",
                self.socket.display()
            )
        })?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;

        let mut line = serde_json::to_vec(&req)?;
        line.push(b'\n');
        stream.write_all(&line)?;

        let mut buf = String::new();
        BufReader::new(stream).read_line(&mut buf)?;
        let resp: Value = serde_json::from_str(&buf).context("invalid response from lactd")?;

        match resp["status"].as_str() {
            Some("ok") => Ok(resp["data"].clone()),
            Some("error") => bail!("lactd: {}", resp["data"]),
            _ => bail!("unexpected response from lactd: {buf}"),
        }
    }

    pub fn ping(&self) -> Result<()> {
        self.request("ping", None).map(|_| ())
    }

    pub fn list_devices(&self) -> Result<Value> {
        self.request("list_devices", None)
    }

    pub fn device_info(&self, id: &str) -> Result<Value> {
        self.request("device_info", Some(json!({ "id": id })))
    }

    pub fn device_stats(&self, id: &str) -> Result<Value> {
        self.request("device_stats", Some(json!({ "id": id })))
    }

    /// Sets the power cap in watts (`None` = default).
    /// LACT applies the change pending confirmation: follow it with `confirm_pending`,
    /// otherwise after a few seconds the daemon reverts to the previous values.
    pub fn set_power_cap(&self, id: &str, watts: Option<f64>) -> Result<Value> {
        self.request("set_power_cap", Some(json!({ "id": id, "cap": watts })))
    }

    pub fn confirm_pending(&self, confirm: bool) -> Result<()> {
        let command = if confirm { "confirm" } else { "revert" };
        self.request(
            "confirm_pending_config",
            Some(json!({ "command": command })),
        )
        .map(|_| ())
    }
}
