//! VST3 hosting: scan folders, load plugins, process audio + MIDI on the audio
//! thread, and edit parameters from the GUI thread.
//!
//! Threading model (follows the VST3 spec):
//!   * [`load`] runs on the GUI thread and returns a [`PluginProcessor`] (moved to the audio
//!     thread, `Send`) and a [`PluginController`] (stays on the GUI thread).
//!   * Parameter edits travel GUI -> audio as (id, normalized value) pairs, applied through
//!     `IParameterChanges` on the next block. Nothing allocates inside `process_*`.
mod editor;
mod host;
mod module;
mod processor;

pub use editor::PluginEditor;
pub use host::PluginController;
pub use module::{default_vst3_dirs, scan, scan_with_errors, PluginInfo};
pub use processor::{EventKind, MidiEvent, PluginProcessor, Transport};

/// Load a plugin found by [`scan`]. GUI thread only.
pub fn load(info: &PluginInfo, sample_rate: f64, max_block: usize) -> Result<(PluginProcessor, Option<PluginController>), String> {
    processor::load(info, sample_rate, max_block, None)
}

/// Like [`load`], but first hands a state produced by [`PluginController::save_state`] to the
/// plug-in (before it starts processing), so its sounds and presets come back. GUI thread only.
pub fn load_with_state(info: &PluginInfo, sample_rate: f64, max_block: usize, state: Option<&[u8]>) -> Result<(PluginProcessor, Option<PluginController>), String> {
    processor::load(info, sample_rate, max_block, state)
}
