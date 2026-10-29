//! Core di Redline: sensori GPU, client del demone LACT e controllo di GPU Screen Recorder.
//! Nessuna dipendenza da Tauri: il backend dell'app userà questo crate così com'è.

pub mod lact;
pub mod recorder;
pub mod sensors;
