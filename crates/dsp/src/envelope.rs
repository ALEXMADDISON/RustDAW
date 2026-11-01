#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// Linear ADSR envelope, advanced one sample at a time.
#[derive(Clone, Copy, Debug)]
pub struct Adsr {
    stage: Stage,
    level: f32,
    attack_rate: f32,
    decay_rate: f32,
    sustain: f32,
    release_rate: f32,
}

impl Adsr {
    pub const fn new() -> Self {
        Self { stage: Stage::Idle, level: 0.0, attack_rate: 0.01, decay_rate: 0.001, sustain: 0.7, release_rate: 0.001 }
    }

    /// Times in seconds, sustain in [0, 1].
    pub fn set_params(&mut self, attack: f32, decay: f32, sustain: f32, release: f32, sr: f32) {
        let rate = |secs: f32| 1.0 / (secs.max(0.0005) * sr);
        self.attack_rate = rate(attack);
        self.decay_rate = rate(decay);
        self.sustain = sustain.clamp(0.0, 1.0);
        self.release_rate = rate(release);
    }

    pub fn note_on(&mut self) {
        self.stage = Stage::Attack;
    }

    pub fn note_off(&mut self) {
        if self.stage != Stage::Idle {
            self.stage = Stage::Release;
        }
    }

    pub fn is_active(&self) -> bool {
        self.stage != Stage::Idle
    }

    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        match self.stage {
            Stage::Idle | Stage::Sustain => {}
            Stage::Attack => {
                self.level += self.attack_rate;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.level -= self.decay_rate;
                if self.level <= self.sustain {
                    self.level = self.sustain;
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Release => {
                self.level -= self.release_rate;
                if self.level <= 0.0 {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
        }
        self.level
    }
}

impl Default for Adsr {
    fn default() -> Self {
        Self::new()
    }
}
