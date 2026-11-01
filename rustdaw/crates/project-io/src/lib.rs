//! File I/O: audio import/export, sample-library folder scanning + watching, app config.
pub mod audio;
pub mod config;
pub mod export;
pub mod library;

pub use audio::{load_sample, probe_duration};
pub use config::Config;
pub use export::{write_audio, ExportFormat};
pub use library::{scan_folder as library_scan, FolderWatcher, ScanResult};
