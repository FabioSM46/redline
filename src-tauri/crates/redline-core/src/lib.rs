//! Redline core: GPU sensors, the LACT daemon client and GPU Screen Recorder control.
//! No dependency on Tauri: the app backend uses this crate as it is.

pub mod lact;
pub mod recorder;
pub mod sensors;
