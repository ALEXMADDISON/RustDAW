use dsp::{midi_to_hz, Adsr, Oscillator, Waveform};

pub const MAX_VOICES: usize = 16;

#[derive(Clone, Copy)]
struct Voice {
    osc: Oscillator,
    env: Adsr,
    note: u8,
    velocity: f32,
}

/// Fixed-size polyphonic oscillator synth. All memory is inline; `process`
/// never allocates, so it is safe on the audio thread.
pub struct SimpleSynth {
    voices: [Voice; MAX_VOICES],
    sample_rate: f32,
    /// Round-robin cursor used for voice stealing.
    steal: usize,
    /// Frequency factor of the pitch automation, 1.0 = none.
    bend: f32,
}

impl SimpleSynth {
    pub fn new(sample_rate: f32) -> Self {
        let mut env = Adsr::new();
        env.set_params(0.005, 0.15, 0.6, 0.2, sample_rate);
        let voice = Voice { osc: Oscillator::new(Waveform::Saw), env, note: 0, velocity: 0.0 };
        Self { voices: [voice; MAX_VOICES], sample_rate, steal: 0, bend: 1.0 }
    }

    pub fn set_waveform(&mut self, w: Waveform) {
        for v in &mut self.voices {
            v.osc.waveform = w;
        }
    }

    /// Shift all voices (also the sounding ones) by `semitones`.
    pub fn set_pitch(&mut self, semitones: f32) {
        self.bend = 2f32.powf(semitones / 12.0);
        let (bend, sr) = (self.bend, self.sample_rate);
        for v in self.voices.iter_mut().filter(|v| v.env.is_active()) {
            v.osc.set_frequency(midi_to_hz(v.note) * bend, sr);
        }
    }

    pub fn note_on(&mut self, note: u8, velocity: u8) {
        let idx = match self.voices.iter().position(|v| !v.env.is_active()) {
            Some(i) => i,
            None => {
                self.steal = (self.steal + 1) % MAX_VOICES;
                self.steal
            }
        };
        let (sr, bend) = (self.sample_rate, self.bend);
        let v = &mut self.voices[idx];
        v.note = note;
        v.velocity = velocity as f32 / 127.0;
        v.osc.set_frequency(midi_to_hz(note) * bend, sr);
        v.env.note_on();
    }

    pub fn note_off(&mut self, note: u8) {
        for v in self.voices.iter_mut().filter(|v| v.env.is_active() && v.note == note) {
            v.env.note_off();
        }
    }

    /// Additively render `left.len()` frames into the two buffers.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        // Idle synths (most of the 100 tracks) cost almost nothing.
        if !self.voices.iter().any(|v| v.env.is_active()) {
            return;
        }
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let mut acc = 0.0;
            for v in self.voices.iter_mut().filter(|v| v.env.is_active()) {
                acc += v.osc.next_sample() * v.env.next_sample() * v.velocity;
            }
            let s = acc * 0.25; // headroom for polyphony
            *l += s;
            *r += s;
        }
    }
}
