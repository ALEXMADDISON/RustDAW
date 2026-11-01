/// Decoded stereo audio held in memory. Immutable once created, so it can be
/// shared with the audio thread through an `Arc` without locks.
#[derive(Clone, Debug)]
pub struct SampleData {
    pub name: String,
    pub sample_rate: f32,
    pub left: Vec<f32>,
    pub right: Vec<f32>,
}

impl SampleData {
    pub fn len(&self) -> usize {
        self.left.len()
    }

    pub fn is_empty(&self) -> bool {
        self.left.is_empty()
    }

    pub fn duration_secs(&self) -> f32 {
        self.len() as f32 / self.sample_rate.max(1.0)
    }

    /// Linear-interpolation resample (GUI/IO thread only; allocates).
    pub fn resampled(&self, target_rate: f32) -> SampleData {
        if (self.sample_rate - target_rate).abs() < 0.5 || self.is_empty() {
            return SampleData { sample_rate: target_rate, ..self.clone() };
        }
        let ratio = self.sample_rate as f64 / target_rate as f64;
        let out_len = (self.len() as f64 / ratio) as usize;
        let interp = |src: &[f32]| -> Vec<f32> {
            (0..out_len)
                .map(|i| {
                    let p = i as f64 * ratio;
                    let k = p as usize;
                    let f = (p - k as f64) as f32;
                    let a = src[k.min(src.len() - 1)];
                    let b = src[(k + 1).min(src.len() - 1)];
                    a + (b - a) * f
                })
                .collect()
        };
        SampleData { name: self.name.clone(), sample_rate: target_rate, left: interp(&self.left), right: interp(&self.right) }
    }

    /// Min/max envelope for drawing waveforms.
    pub fn peaks(&self, buckets: usize) -> Vec<(f32, f32)> {
        let n = self.len();
        if n == 0 || buckets == 0 {
            return Vec::new();
        }
        (0..buckets)
            .map(|b| {
                let a = b * n / buckets;
                let z = (((b + 1) * n) / buckets).max(a + 1).min(n);
                let mut lo = 0.0f32;
                let mut hi = 0.0f32;
                for i in a..z {
                    let m = (self.left[i] + self.right[i]) * 0.5;
                    lo = lo.min(m);
                    hi = hi.max(m);
                }
                (lo, hi)
            })
            .collect()
    }
}
