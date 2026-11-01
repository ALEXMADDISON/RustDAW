//! egui front-end (Logic-Pro-style dark UI): transport + LCD, Samples browser with watch folders,
//! Playlist (100 tracks, recording, per-clip pitch / BPM), Patterns window, Inspector (VST3 instrument/effect slots), Piano Roll and
//! Mixer in a docking workspace, plus audio export.
//! The GUI talks to the audio engine only through `EngineHandle` (lock-free rings).
pub mod audio_devices;
mod app;
mod dialogs;
mod libsearch;
mod panels;
mod project;
mod settings;
mod state;
mod theme;
mod toolbar;
mod widgets;

pub use app::{run, run_with};
pub use state::AudioInput;
