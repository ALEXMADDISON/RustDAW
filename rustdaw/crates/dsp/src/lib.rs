//! Pure DSP building blocks. No allocation, no locks, no I/O in the
//! per-sample paths: everything here is safe to call from the audio thread.
pub mod bpm;
pub mod envelope;
pub mod oscillator;
pub mod sample;
pub mod stretch;
pub use bpm::detect_bpm;
pub use envelope::Adsr;
pub use oscillator::{midi_to_hz, Oscillator, Waveform};
pub use sample::SampleData;
pub use stretch::{process_clip, resample, time_stretch, time_stretch_with, StretchQuality};
