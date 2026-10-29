//! Redline core: GPU sensors and their history, the LACT daemon client, GPU Screen Recorder
//! control, the length of saved videos and their thumbnails.
//! No dependency on Tauri: the app backend uses this crate as it is.

pub mod history;
pub mod lact;
pub mod media;
pub mod recorder;
pub mod sensors;
pub mod thumbnail;
