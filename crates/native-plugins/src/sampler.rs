use dsp::SampleData;
use std::sync::Arc;

pub const SAMPLER_VOICES: usize = 8;
/// Key at which the sample plays back at its original pitch.
pub const ROOT_KEY: u8 = 60;

#[derive(Clone, Copy)]
struct Voice {
    pos: f64,
    ratio: f64,
    gain: f32,
    active: bool,
    age: u32,
}

/// One-shot sampler: pitch-shifts by resampling (varispeed), 8 voices.
/// The sample is shared as `Arc<SampleData>`; the GUI keeps its own clone for
/// the whole session so the audio thread never performs the final free.
pub struct Sampler {
    sample: Option<Arc<SampleData>>,
    voices: [Voice; SAMPLER_VOICES],
    clock: u32,
    /// Playback-speed factor applied to every voice (pitch automation), 1.0 = none.
    bend: f64,
}

impl Sampler {
    pub fn new() -> Self {
        let v = Voice { pos: 0.0, ratio: 1.0, gain: 0.0, active: false, age: 0 };
        Self { sample: None, voices: [v; SAMPLER_VOICES], clock: 0, bend: 1.0 }
    }

    pub fn set_sample(&mut self, s: Option<Arc<SampleData>>) {
        for v in &mut self.voices {
            v.active = false;
        }
        self.sample = s;
    }

    /// Shift every voice (also the ones already playing) by `semitones`.
    pub fn set_pitch(&mut self, semitones: f32) {
        self.bend = 2f64.powf(semitones as f64 / 12.0);
    }

    pub fn has_sample(&self) -> bool {
        self.sample.is_some()
    }

    pub fn note_on(&mut self, key: u8, velocity: u8) {
        if self.sample.is_none() {
            return;
        }
        self.clock = self.clock.wrapping_add(1);
        let idx = self
            .voices
            .iter()
            .position(|v| !v.active)
            .unwrap_or_else(|| self.voices.iter().enumerate().min_by_key(|(_, v)| v.age).map(|(i, _)| i).unwrap_or(0));
        self.voices[idx] = Voice {
            pos: 0.0,
            ratio: 2f64.powf((key as f64 - ROOT_KEY as f64) / 12.0),
            gain: velocity as f32 / 127.0 * 0.8,
            active: true,
            age: self.clock,
        };
    }

    /// Silence every voice immediately (used to stop library previews).
    pub fn stop_all(&mut self) {
        for v in &mut self.voices {
            v.active = false;
        }
    }

    /// One-shot: note-off does not cut the sample.
    pub fn note_off(&mut self, _key: u8) {}

    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let Some(s) = &self.sample else { return };
        let n = s.len();
        if n < 2 {
            return;
        }
        let bend = self.bend;
        for v in self.voices.iter_mut().filter(|v| v.active) {
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                let i = v.pos as usize;
                if i + 1 >= n {
                    v.active = false;
                    break;
                }
                let f = (v.pos - i as f64) as f32;
                *l += (s.left[i] + (s.left[i + 1] - s.left[i]) * f) * v.gain;
                *r += (s.right[i] + (s.right[i + 1] - s.right[i]) * f) * v.gain;
                v.pos += v.ratio * bend;
            }
        }
    }
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}
