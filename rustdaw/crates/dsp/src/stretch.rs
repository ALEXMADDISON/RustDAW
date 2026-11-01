//! Offline pitch shifting and time stretching for audio clips.
//! GUI / worker threads only: everything here allocates.
use crate::sample::SampleData;

/// Analysis window of the WSOLA time stretcher (frames). The synthesis hop is half of it.
const WIN: usize = 2048;

/// Time-stretch algorithm preset: a longer analysis window sounds smoother on sustained material,
/// a shorter one keeps transients (drums) tighter and renders faster.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StretchQuality {
    /// 1024-frame window: tight transients, fastest.
    Fast,
    /// 2048-frame window: good all-rounder.
    #[default]
    Normal,
    /// 4096-frame window: smooth pads and vocals.
    Smooth,
}

impl StretchQuality {
    pub fn window(self) -> usize {
        match self {
            StretchQuality::Fast => 1024,
            StretchQuality::Normal => WIN,
            StretchQuality::Smooth => 4096,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            StretchQuality::Fast => "Fast (drums)",
            StretchQuality::Normal => "Normal",
            StretchQuality::Smooth => "Smooth (pads / vocals)",
        }
    }
    pub const ALL: [StretchQuality; 3] = [StretchQuality::Fast, StretchQuality::Normal, StretchQuality::Smooth];
}

/// Resample by `rate` with cubic interpolation. `rate > 1` plays faster (higher pitch, shorter);
/// the output is `src.len() / rate` frames long.
pub fn resample(src: &[f32], rate: f64) -> Vec<f32> {
    if src.is_empty() || rate <= 0.0 {
        return Vec::new();
    }
    let out_len = ((src.len() as f64 / rate).round() as usize).max(1);
    let last = (src.len() - 1) as isize;
    let at = |i: isize| -> f32 { src[i.clamp(0, last) as usize] };
    (0..out_len)
        .map(|o| {
            let p = o as f64 * rate;
            let k = p.floor() as isize;
            let t = (p - k as f64) as f32;
            let (a, b, c, d) = (at(k - 1), at(k), at(k + 1), at(k + 2));
            // Catmull-Rom spline
            0.5 * (2.0 * b + (c - a) * t + (2.0 * a - 5.0 * b + 4.0 * c - d) * t * t + (3.0 * b - a - 3.0 * c + d) * t * t * t)
        })
        .collect()
}

/// Change the length of a stereo signal to `ratio` times the original **without** changing its
/// pitch (WSOLA: waveform-similarity overlap-add). `ratio > 1` = slower / longer.
pub fn time_stretch(left: &[f32], right: &[f32], ratio: f64) -> (Vec<f32>, Vec<f32>) {
    time_stretch_with(left, right, ratio, StretchQuality::Normal)
}

/// Same as [`time_stretch`] with a choice of analysis window (see [`StretchQuality`]).
pub fn time_stretch_with(left: &[f32], right: &[f32], ratio: f64, quality: StretchQuality) -> (Vec<f32>, Vec<f32>) {
    let win_n = quality.window();
    let n_in = left.len().min(right.len());
    if n_in == 0 {
        return (Vec::new(), Vec::new());
    }
    if (ratio - 1.0).abs() < 1e-4 {
        return (left[..n_in].to_vec(), right[..n_in].to_vec());
    }
    let ratio = ratio.clamp(0.05, 20.0);
    if n_in < win_n * 4 {
        // Too short for overlap-add: plain resampling (pitch moves a little, only for tiny clips).
        let rate = 1.0 / ratio;
        return (resample(&left[..n_in], rate), resample(&right[..n_in], rate));
    }
    let out_len = ((n_in as f64 * ratio).round() as usize).max(1);
    let hs = win_n / 2; // synthesis hop
    let ha = hs as f64 / ratio; // nominal analysis hop
    let delta = win_n / 4; // search tolerance (+/- frames)
    let pad = win_n * 3;

    let mut l: Vec<f32> = Vec::with_capacity(n_in + pad);
    l.extend_from_slice(&left[..n_in]);
    l.resize(n_in + pad, 0.0);
    let mut r: Vec<f32> = Vec::with_capacity(n_in + pad);
    r.extend_from_slice(&right[..n_in]);
    r.resize(n_in + pad, 0.0);
    let mono: Vec<f32> = l.iter().zip(r.iter()).map(|(a, b)| (a + b) * 0.5).collect();
    // Mono signal decimated by 4 for the coarse similarity search.
    let dec: Vec<f32> = (0..mono.len() / 4).map(|i| (mono[i * 4] + mono[i * 4 + 1] + mono[i * 4 + 2] + mono[i * 4 + 3]) * 0.25).collect();
    let window: Vec<f32> = (0..win_n).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / win_n as f32).cos()).collect();

    let mut ol = vec![0.0f32; out_len + win_n];
    let mut or = vec![0.0f32; out_len + win_n];
    let m = hs / 4;
    let mut prev: usize = 0; // analysis position chosen for the previous frame
    let mut k: usize = 0;
    loop {
        let out_pos = k * hs;
        if out_pos >= out_len {
            break;
        }
        let chosen: usize = if k == 0 {
            0
        } else {
            let nominal = ((k as f64 * ha).round() as usize).min(n_in - 1);
            let lo = nominal.saturating_sub(delta);
            let hi = (nominal + delta).min(n_in - 1);
            // Coarse search on the decimated signal: which segment best continues the previous one?
            let ti = (prev + hs) / 4;
            let tmpl = &dec[ti..ti + m];
            let mut best = lo;
            let mut best_s = f32::MIN;
            let mut c = lo;
            while c <= hi {
                let ci = c / 4;
                let cand = &dec[ci..ci + m];
                let mut dot = 0.0f32;
                let mut e = 1e-9f32;
                for i in 0..m {
                    dot += tmpl[i] * cand[i];
                    e += cand[i] * cand[i];
                }
                let sc = dot / e.sqrt();
                if sc > best_s {
                    best_s = sc;
                    best = c;
                }
                c += 4;
            }
            // Refine at full resolution.
            let t2 = &mono[prev + hs..prev + hs + hs];
            let rlo = best.saturating_sub(3).max(lo);
            let rhi = (best + 3).min(hi);
            let mut best2 = best;
            let mut best2_s = f32::MIN;
            for c in rlo..=rhi {
                let cand = &mono[c..c + hs];
                let mut dot = 0.0f32;
                let mut e = 1e-9f32;
                for i in 0..hs {
                    dot += t2[i] * cand[i];
                    e += cand[i] * cand[i];
                }
                let sc = dot / e.sqrt();
                if sc > best2_s {
                    best2_s = sc;
                    best2 = c;
                }
            }
            best2
        };
        for i in 0..win_n {
            // The very first frame keeps its full-level attack (no fade-in).
            let w = if k == 0 && i < hs { 1.0 } else { window[i] };
            ol[out_pos + i] += w * l[chosen + i];
            or[out_pos + i] += w * r[chosen + i];
        }
        prev = chosen;
        k += 1;
    }
    ol.truncate(out_len);
    or.truncate(out_len);
    (ol, or)
}

/// Process a whole sample: shift the pitch by `semitones` and play it at `tempo` times the
/// original speed (`tempo > 1` = shorter).
///
/// * `tape == false` ("Stretch"): pitch and length are independent (resample + time stretch).
/// * `tape == true` ("Resample"): pitch and speed are linked like on a turntable, so a faster
///   tempo also raises the pitch.
pub fn process_clip(src: &SampleData, semitones: f32, tape: bool, tempo: f64, quality: StretchQuality) -> SampleData {
    let p = 2f64.powf(semitones as f64 / 12.0);
    let tempo = tempo.clamp(0.1, 10.0);
    let pitched = (p - 1.0).abs() >= 1e-4;
    if !pitched && (tempo - 1.0).abs() < 1e-4 {
        return src.clone();
    }
    if tape {
        let rate = p * tempo;
        return SampleData { name: src.name.clone(), sample_rate: src.sample_rate, left: resample(&src.left, rate), right: resample(&src.right, rate) };
    }
    let mut ratio = 1.0 / tempo;
    let (mut l, mut r) = if pitched {
        ratio *= p;
        (resample(&src.left, p), resample(&src.right, p))
    } else {
        (src.left.clone(), src.right.clone())
    };
    if (ratio - 1.0).abs() > 1e-3 {
        let (a, b) = time_stretch_with(&l, &r, ratio, quality);
        l = a;
        r = b;
    }
    SampleData { name: src.name.clone(), sample_rate: src.sample_rate, left: l, right: r }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, secs: f32, sr: f32) -> Vec<f32> {
        (0..(secs * sr) as usize).map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin() * 0.8).collect()
    }

    /// Dominant frequency by counting rising zero crossings.
    fn freq_of(x: &[f32], sr: f32) -> f32 {
        let mut n = 0;
        for w in x.windows(2) {
            if w[0] < 0.0 && w[1] >= 0.0 {
                n += 1;
            }
        }
        n as f32 * sr / x.len() as f32
    }

    #[test]
    fn stretch_changes_length_but_not_pitch() {
        let sr = 44_100.0;
        let x = sine(440.0, 3.0, sr);
        for ratio in [0.6f64, 1.5, 2.0] {
            let (a, b) = time_stretch(&x, &x, ratio);
            assert_eq!(a.len(), b.len());
            assert!((a.len() as f64 - x.len() as f64 * ratio).abs() < 2.0);
            let mid = &a[a.len() / 4..a.len() * 3 / 4];
            let f = freq_of(mid, sr);
            assert!((f - 440.0).abs() < 8.0, "ratio {ratio}: {f} Hz");
        }
    }

    #[test]
    fn pitch_shift_keeps_length() {
        let sr = 44_100.0;
        let s = SampleData { name: "t".into(), sample_rate: sr, left: sine(440.0, 3.0, sr), right: sine(440.0, 3.0, sr) };
        let out = process_clip(&s, 12.0, false, 1.0, StretchQuality::Normal);
        assert!((out.len() as f64 - s.len() as f64).abs() < 4.0);
        let mid = &out.left[out.len() / 4..out.len() * 3 / 4];
        assert!((freq_of(mid, sr) - 880.0).abs() < 15.0);
        // tape mode: an octave up halves the length
        let tape = process_clip(&s, 12.0, true, 1.0, StretchQuality::Normal);
        assert!((tape.len() as f64 - s.len() as f64 / 2.0).abs() < 4.0);
    }

    #[test]
    fn identity_is_a_plain_copy() {
        let s = SampleData { name: "t".into(), sample_rate: 48_000.0, left: vec![0.1; 100], right: vec![0.2; 100] };
        let out = process_clip(&s, 0.0, false, 1.0, StretchQuality::Normal);
        assert_eq!(out.left, s.left);
    }
}
