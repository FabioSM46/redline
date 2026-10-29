//! Client minimale per il demone LACT (`lactd`).
//!
//! Protocollo: socket Unix, una richiesta JSON per riga, una risposta JSON per riga.
//! Richiesta: `{"command": "<snake_case>", "args": {...}}`
//! Risposta:  `{"status": "ok", "data": ...}` oppure `{"status": "error", "data": ...}`
//! Riferimento: lact-schema/src/request.rs nel repo di LACT (v0.10.1).

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

    /// Invia un comando e restituisce il campo `data` della risposta.
    pub fn request(&self, command: &str, args: Option<Value>) -> Result<Value> {
        let mut req = json!({ "command": command });
        if let Some(args) = args {
            req["args"] = args;
        }

        let mut stream = UnixStream::connect(&self.socket).with_context(|| {
            format!(
                "connessione a {} fallita: lactd è attivo e sei nel gruppo admin (sudo)?",
                self.socket.display()
            )
        })?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;

        let mut line = serde_json::to_vec(&req)?;
        line.push(b'\n');
        stream.write_all(&line)?;

        let mut buf = String::new();
        BufReader::new(stream).read_line(&mut buf)?;
        let resp: Value = serde_json::from_str(&buf).context("risposta di lactd non valida")?;

        match resp["status"].as_str() {
            Some("ok") => Ok(resp["data"].clone()),
            Some("error") => bail!("lactd: {}", resp["data"]),
            _ => bail!("risposta inattesa da lactd: {buf}"),
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

    /// Imposta il limite di potenza in watt (`None` = default).
    /// LACT applica la modifica in attesa di conferma: va seguita da `confirm_pending`,
    /// altrimenti dopo qualche secondo il demone torna ai valori precedenti.
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
