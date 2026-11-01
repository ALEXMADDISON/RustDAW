//! Tempo detection for loops and drum samples (GUI / worker threads only).

/// Estimate the tempo of `left`/`right` in BPM, or `None` when the clip is too short or has no
/// clear pulse. Tempo detection is a heuristic: results may be off by an octave (half / double).
pub fn detect_bpm(left: &[f32], right: &[f32], sr: f32) -> Option<f32> {
    let total = left.len().min(right.len());
    if total == 0 || sr < 1000.0 || (total as f32) / sr < 1.0 {
        return None;
    }
    let n = total.min((sr * 90.0) as usize); // analyse at most the first 90 s
    let hop = ((sr / 200.0) as usize).max(64); // ~5 ms
    let win = hop * 2;
    let fps = sr as f64 / hop as f64;
    let frames = n.saturating_sub(win) / hop;
    if frames < 64 {
        return None;
    }
    // Loudness envelope, compressed so quiet hits count too.
    let mut comp: Vec<f32> = Vec::with_capacity(frames);
    for f in 0..frames {
        let a = f * hop;
        let mut e = 0.0f32;
        for i in a..a + win {
            let m = (left[i] + right[i]) * 0.5;
            e += m * m;
        }
        comp.push((1.0 + 100.0 * (e / win as f32).sqrt()).ln());
    }
    // Onset strength: positive change, minus its local average (+/- 40 frames).
    let mut flux: Vec<f32> = Vec::with_capacity(frames);
    flux.push(0.0);
    for i in 1..frames {
        flux.push((comp[i] - comp[i - 1]).max(0.0));
    }
    let mut cum: Vec<f64> = Vec::with_capacity(frames + 1);
    cum.push(0.0);
    for v in &flux {
        let last = *cum.last().unwrap_or(&0.0);
        cum.push(last + *v as f64);
    }
    let raw = flux.clone();
    for i in 0..frames {
        let a = i.saturating_sub(40);
        let b = (i + 41).min(frames);
        let avg = ((cum[b] - cum[a]) / (b - a) as f64) as f32;
        flux[i] = (raw[i] - avg).max(0.0);
    }
    let len = flux.len();
    if flux.iter().all(|&v| v <= 0.0) {
        return None;
    }

    // Autocorrelation at a fractional lag (in frames).
    let ac = |lag: f64| -> f64 {
        let l0 = lag.floor() as usize;
        let fr = lag - l0 as f64;
        if l0 + 2 >= len {
            return 0.0;
        }
        let m = len - l0 - 1;
        let mut s = 0.0f64;
        for i in 0..m {
            let v = flux[i + l0] as f64 * (1.0 - fr) + flux[i + l0 + 1] as f64 * fr;
            s += flux[i] as f64 * v;
        }
        s / m as f64
    };
    // A tempo is supported by the pulse at one beat and at two / four beats.
    let score = |bpm: f64| -> f64 {
        let lag = 60.0 * fps / bpm;
        let mut s = 0.0f64;
        for (h, w) in [(1.0f64, 1.0f64), (2.0, 0.5), (4.0, 0.25)] {
            if lag * h < len as f64 / 2.0 {
                s += w * ac(lag * h);
            }
        }
        s
    };

    // Coarse scan with a mild preference for everyday tempos, then a fine scan without it.
    let mut best = 120.0f64;
    let mut best_s = f64::MIN;
    let mut b = 70.0f64;
    while b <= 180.0 {
        let prior = (-0.5 * ((b / 120.0).log2() / 0.7).powi(2)).exp();
        let s = score(b) * prior;
        if s > best_s {
            best_s = s;
            best = b;
        }
        b += 0.5;
    }
    if best_s <= 0.0 {
        return None;
    }
    let mut fine = best;
    let mut fine_s = f64::MIN;
    let mut b = best - 1.0;
    while b <= best + 1.0 {
        let s = score(b);
        if s > fine_s {
            fine_s = s;
            fine = b;
        }
        b += 0.05;
    }
    let mut bpm = fine;
    // Loops are usually a whole number of bars long: snap when that is within 2 %.
    let secs = total as f64 / sr as f64;
    let beats = (bpm * secs / 60.0).round();
    if beats >= 4.0 && beats % 4.0 == 0.0 {
        let snapped = 60.0 * beats / secs;
        if (snapped - bpm).abs() / bpm < 0.02 {
            bpm = snapped;
        }
    }
    Some(((bpm * 100.0).round() / 100.0) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drum_loop(bpm: f32, bars: usize, sr: f32) -> Vec<f32> {
        let beat = 60.0 / bpm;
        let n = (bars as f32 * 4.0 * beat * sr) as usize;
        let mut x = vec![0.0f32; n];
        let mut seed = 12345u32;
        let mut noise = move || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / 8_388_608.0 - 1.0
        };
        let mut hit = |x: &mut Vec<f32>, t: f32, kick: bool| {
            let i = (t * sr) as usize;
            let l = ((if kick { 0.15 } else { 0.05 }) * sr) as usize;
            for j in 0..l.min(n.saturating_sub(i)) {
                let tt = j as f32 / sr;
                x[i + j] += if kick { (2.0 * std::f32::consts::PI * (50.0 + 80.0 * (-tt * 40.0).exp()) * tt).sin() * (-tt * 18.0).exp() } else { noise() * (-tt * 60.0).exp() * 0.4 };
            }
        };
        for bar in 0..bars {
            for b in 0..4 {
                let t = (bar * 4 + b) as f32 * beat;
                hit(&mut x, t, b % 2 == 0);
                hit(&mut x, t + beat * 0.5, false);
            }
        }
        x
    }

    #[test]
    fn finds_common_tempos() {
        let sr = 44_100.0;
        for bpm in [90.0f32, 110.0, 128.0, 140.0] {
            let x = drum_loop(bpm, 4, sr);
            let got = detect_bpm(&x, &x, sr).expect("tempo");
            assert!((got - bpm).abs() < 1.5, "wanted {bpm}, got {got}");
        }
    }

    #[test]
    fn too_short_has_no_tempo() {
        let x = vec![0.0f32; 1000];
        assert!(detect_bpm(&x, &x, 44_100.0).is_none());
    }
}
