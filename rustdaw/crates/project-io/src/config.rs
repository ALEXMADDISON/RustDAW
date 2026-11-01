//! Persistent app settings (JSON in the user's config directory).
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Sample-library "watch folders".
    pub watch_folders: Vec<PathBuf>,
    /// Extra folders to scan for VST3 plugins (in addition to the system locations).
    pub vst_dirs: Vec<PathBuf>,
    /// Latency compensation applied to recorded takes.
    pub rec_offset_ms: f32,
    /// Last export folder.
    pub export_dir: Option<PathBuf>,
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("rust-daw").join("config.json"))
    }

    pub fn load() -> Config {
        Self::path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(p) = Self::path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(s) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(p, s);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trips_and_tolerates_missing_fields() {
        let c = Config { watch_folders: vec!["/a".into()], vst_dirs: vec![], rec_offset_ms: 12.5, export_dir: None };
        let s = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Config>(&s).unwrap(), c);
        assert_eq!(serde_json::from_str::<Config>("{}").unwrap(), Config::default());
    }
}
