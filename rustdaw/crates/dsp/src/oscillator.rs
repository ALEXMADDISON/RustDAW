use std::f32::consts::TAU;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Waveform {
    Sine,
    Saw,
    Square,
    Triangle,
}

/// Band-limited (PolyBLEP) oscillator. `phase` and `inc` are normalized [0, 1).
#[derive(Clone, Copy, Debug)]
pub struct Oscillator {
    phase: f32,
    inc: f32,
    pub waveform: Waveform,
}

/// Polynomial band-limited step residual; removes most aliasing at edges.
#[inline]
fn poly_blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let x = t / dt;
        x + x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

impl Oscillator {
    pub const fn new(waveform: Waveform) -> Self {
        Self { phase: 0.0, inc: 0.0, waveform }
    }

    #[inline]
    pub fn set_frequency(&mut self, hz: f32, sample_rate: f32) {
        self.inc = hz / sample_rate;
    }

    pub fn reset_phase(&mut self) {
        self.phase = 0.0;
    }

    /// Generate one sample in [-1, 1].
    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        let t = self.phase;
        let dt = self.inc;
        let s = match self.waveform {
            Waveform::Sine => (t * TAU).sin(),
            Waveform::Saw => (2.0 * t - 1.0) - poly_blep(t, dt),
            Waveform::Square => {
                let naive = if t < 0.5 { 1.0 } else { -1.0 };
                naive + poly_blep(t, dt) - poly_blep((t + 0.5) % 1.0, dt)
            }
            Waveform::Triangle => 1.0 - 4.0 * (t - 0.5).abs(),
        };
        self.phase += dt;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        s
    }
}

/// MIDI note number -> frequency in Hz (A4 = 69 = 440 Hz).
#[inline]
pub fn midi_to_hz(note: u8) -> f32 {
    440.0 * 2.0f32.powf((note as f32 - 69.0) / 12.0)
}
