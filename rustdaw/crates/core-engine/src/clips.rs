//! Real-time view of the arrangement. The GUI builds a complete `ClipList` and sends it
//! to the audio thread; the old list comes back through the garbage ring so it is freed
//! on the GUI thread.
use crate::automation::RtAuto;
use crate::model::Note;
use dsp::SampleData;
use std::sync::Arc;

pub const MAX_TRACKS: usize = 100;

/// Per-clip playback settings that are applied while the clip plays (no re-rendering needed).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioParams {
    /// Linear gain per channel (clip volume combined with pan).
    pub gain_l: f32,
    pub gain_r: f32,
    /// Fade lengths in frames, counted from the start / end of the clip.
    pub fade_in: u32,
    pub fade_out: u32,
    pub reverse: bool,
}

impl Default for AudioParams {
    fn default() -> Self {
        Self { gain_l: 1.0, gain_r: 1.0, fade_in: 0, fade_out: 0, reverse: false }
    }
}

pub enum RtClipKind {
    /// `offset` / `frames` are in sample frames at the engine sample rate.
    Audio { sample: Arc<SampleData>, offset: u32, frames: u32, params: AudioParams },
    /// Notes are clip-relative and sorted by start; `len` is the clip length in ticks.
    Midi { notes: Vec<Note>, len: u32 },
}

pub struct RtClip {
    pub id: u64,
    pub track: u16,
    /// Start position in ticks (PPQ).
    pub start: u32,
    pub kind: RtClipKind,
    /// Volume factor set by `ClipGain` automation (engine bookkeeping, 1.0 = unchanged).
    pub gain_mul: f32,
}

#[derive(Default)]
pub struct ClipList {
    pub clips: Vec<RtClip>,
    /// Automation clips; the engine owns their bookkeeping fields while the list is active.
    pub automation: Vec<RtAuto>,
}
