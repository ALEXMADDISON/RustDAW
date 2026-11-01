use serde::{Deserialize, Serialize};

/// Pulses (ticks) per quarter note: the project-wide timing resolution.
pub const PPQ: u32 = 960;

/// A MIDI note inside a MIDI clip. `start` is relative to the clip start (ticks).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub key: u8,
    pub velocity: u8,
    pub start: u32,
    pub len: u32,
}
