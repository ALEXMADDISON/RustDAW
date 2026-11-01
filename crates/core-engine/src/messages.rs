use crate::clips::ClipList;
use dsp::{SampleData, Waveform};
use plugin_host::PluginProcessor;
use std::sync::Arc;

pub enum InstrumentKind {
    None,
    /// Built-in sine/saw/square/triangle synth.
    Synth,
    /// Built-in one-shot sampler. Keep your own `Arc` alive: the audio thread must never
    /// drop the last reference.
    Sampler(Option<Arc<SampleData>>),
}

/// GUI -> audio. Heap data travels as owned `Box`es.
pub enum Command {
    Play,
    Stop,
    /// Move the playhead (ticks).
    Seek(u32),
    SetBpm(f32),
    /// Loop range in ticks (start, end); None = no looping.
    SetCycle(Option<(u32, u32)>),
    NoteOn { track: usize, key: u8, velocity: u8 },
    NoteOff { track: usize, key: u8 },
    SetTrackGain { track: usize, gain: f32 },
    SetTrackPan { track: usize, pan: f32 },
    SetTrackMute { track: usize, mute: bool },
    SetTrackSolo { track: usize, solo: bool },
    SetMasterGain(f32),
    SetInstrument { track: usize, kind: InstrumentKind },
    SetWaveform { track: usize, waveform: Waveform },
    /// Slot 0 = instrument, 1..=FX_SLOTS = insert effects. `None` clears the slot.
    SetPlugin { track: usize, slot: usize, plugin: Option<Box<PluginProcessor>> },
    SetPluginParam { track: usize, slot: usize, id: u32, value: f64 },
    /// Replace the whole arrangement.
    SetClips(Box<ClipList>),
    /// One-shot audition straight to the master output.
    Preview(Arc<SampleData>),
    /// Stop a running audition.
    StopPreview,
}

/// Heap objects the audio thread hands back to be freed on the GUI thread.
pub enum Garbage {
    Clips(Box<ClipList>),
    Plugin(Box<PluginProcessor>),
}
