//! Library search helpers: musical key found in file names ("Pad_Am_120", "Lead F# min") and
//! length queries ("1-3", "<2", ">5", "2.5").

/// (root pitch class 0..=11 with C = 0, Some(true) = minor / Some(false) = major / None = unknown mode)
pub type Key = (u8, Option<bool>);

pub const KEY_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

pub fn key_label(k: Key) -> String {
    let r = KEY_NAMES[(k.0 % 12) as usize];
    match k.1 {
        Some(true) => format!("{r}m"),
        Some(false) => format!("{r} maj"),
        None => r.to_string(),
    }
}

fn note_from(letter: char, acc: Option<char>) -> Option<u8> {
    let base = match letter.to_ascii_lowercase() {
        'c' => 0i32,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let d = match acc {
        Some('#') => 1,
        Some('b') | Some('B') => -1,
        _ => 0,
    };
    Some((base + d).rem_euclid(12) as u8)
}

fn mode_from(s: &str) -> Option<Option<bool>> {
    match s.to_ascii_lowercase().as_str() {
        "" => Some(None),
        "m" | "min" | "minor" => Some(Some(true)),
        "maj" | "major" => Some(Some(false)),
        _ => None,
    }
}

/// Parse one token such as "Am", "F#min", "Bb", "Cmaj". Returns the key and whether a mode was given.
fn parse_token(tok: &str, strict: bool) -> Option<Key> {
    let mut chars = tok.chars();
    let letter = chars.next()?;
    if !('a'..='g').contains(&letter.to_ascii_lowercase()) {
        return None;
    }
    let rest: String = chars.collect();
    let (acc, tail) = match rest.chars().next() {
        Some('#') => (Some('#'), &rest[1..]),
        // a 'b' directly after the letter is a flat unless it starts "bm"/... which cannot follow a note anyway
        Some('b') if letter.is_ascii_uppercase() || !strict => (Some('b'), &rest[1..]),
        _ => (None, &rest[..]),
    };
    let mode = mode_from(tail)?;
    // In file names a bare lowercase letter ("a", "e") is usually a word: require upper case or a mode.
    if strict && mode.is_none() && !letter.is_ascii_uppercase() {
        return None;
    }
    Some((note_from(letter, acc)?, mode))
}

/// Find a key in a file name.
pub fn key_from_name(name: &str) -> Option<Key> {
    let toks: Vec<&str> = name.split(|c: char| !(c.is_ascii_alphanumeric() || c == '#')).filter(|t| !t.is_empty()).collect();
    for (i, t) in toks.iter().enumerate() {
        if let Some(mut k) = parse_token(t, true) {
            // "F#" + "min"
            if k.1.is_none() {
                if let Some(next) = toks.get(i + 1) {
                    if let Some(m) = mode_from(next) {
                        if m.is_some() {
                            k.1 = m;
                        }
                    }
                }
            }
            return Some(k);
        }
    }
    None
}

/// Parse what the user typed in the key box ("a", "Am", "f# minor", "Bb maj").
pub fn parse_key_query(q: &str) -> Option<Key> {
    let t: Vec<&str> = q.split_whitespace().collect();
    match t.as_slice() {
        [one] => parse_token(one, false),
        [a, b] => {
            let (k, _) = parse_token(a, false).map(|k| (k, ()))?;
            let m = mode_from(b)?;
            Some((k.0, if m.is_some() { m } else { k.1 }))
        }
        _ => None,
    }
}

pub fn key_matches(entry: Option<Key>, query: Key) -> bool {
    match entry {
        None => false,
        Some((root, mode)) => root == query.0 && (query.1.is_none() || mode.is_none() || mode == query.1),
    }
}

/// Parse a length query into an inclusive (min, max) range in seconds.
pub fn parse_len_query(q: &str) -> Option<(f32, f32)> {
    let q = q.trim().trim_end_matches(|c| c == 's' || c == 'S').trim();
    if q.is_empty() {
        return None;
    }
    let num = |s: &str| s.trim().trim_end_matches(|c| c == 's' || c == 'S').trim().parse::<f32>().ok();
    if let Some(r) = q.strip_prefix('<') {
        return Some((0.0, num(r)?));
    }
    if let Some(r) = q.strip_prefix('>') {
        return Some((num(r)?, f32::INFINITY));
    }
    if let Some((a, b)) = q.split_once('-') {
        let (a, b) = (num(a)?, num(b)?);
        return Some((a.min(b), a.max(b)));
    }
    let v = num(q)?;
    Some(((v * 0.9 - 0.05).max(0.0), v * 1.1 + 0.05))
}

/// Estimate the key of a sample from its audio (pitch-class profile + Krumhansl-Schmuckler).
/// Returns a root-only key for single-note samples (bass hits, one-shots), root + mode for
/// chords / melodies, and None for drums, noise and anything not clearly tonal.
pub fn estimate_key(left: &[f32], right: &[f32], sr: f32) -> Option<Key> {
    const N: usize = 16384;
    let len = left.len().min(right.len());
    if len < 2048 || sr < 8000.0 {
        return None;
    }
    let win = N.min(len);
    let hann: Vec<f32> = (0..win).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / win as f32).cos()).collect();
    let mut chroma = [0.0f64; 12];
    let mut buf = vec![0.0f32; win];
    let windows = (len / win).clamp(1, 8);
    let mut energy = 0.0f64;
    for w in 0..windows {
        let start = w * win;
        for i in 0..win {
            let j = start + i;
            buf[i] = if j < len { (left[j] + right[j]) * 0.5 * hann[i] } else { 0.0 };
        }
        energy += buf.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>();
        for midi in 36..=84u32 {
            let f = 440.0 * 2f32.powf((midi as f32 - 69.0) / 12.0);
            if f > sr * 0.45 {
                break;
            }
            let coeff = 2.0 * (std::f32::consts::TAU * f / sr).cos();
            let (mut s1, mut s2) = (0.0f32, 0.0f32);
            for &x in &buf {
                let s0 = x + coeff * s1 - s2;
                s2 = s1;
                s1 = s0;
            }
            let p = (s1 * s1 + s2 * s2 - coeff * s1 * s2).max(0.0) as f64;
            chroma[(midi % 12) as usize] += p.sqrt();
        }
    }
    if energy < 1e-6 {
        return None;
    }
    let total: f64 = chroma.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let (top, top_v) = chroma.iter().enumerate().fold((0, 0.0), |a, (i, &v)| if v > a.1 { (i, v) } else { a });
    if top_v / total >= 0.4 {
        return Some((top as u8, None));
    }
    // Flat chroma = noise / drums: not tonal.
    if top_v / total < 0.14 {
        return None;
    }
    const MAJ: [f64; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
    const MIN: [f64; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];
    let mean_c = total / 12.0;
    let corr = |profile: &[f64; 12], shift: usize| -> f64 {
        let mean_p = profile.iter().sum::<f64>() / 12.0;
        let (mut num, mut dc, mut dp) = (0.0, 0.0, 0.0);
        for i in 0..12 {
            let c = chroma[(i + shift) % 12] - mean_c;
            let p = profile[i] - mean_p;
            num += c * p;
            dc += c * c;
            dp += p * p;
        }
        num / (dc.sqrt() * dp.sqrt()).max(1e-12)
    };
    let mut best: (f64, Key) = (-1.0, (0, None));
    for root in 0..12usize {
        let a = corr(&MAJ, root);
        if a > best.0 {
            best = (a, (root as u8, Some(false)));
        }
        let b = corr(&MIN, root);
        if b > best.0 {
            best = (b, (root as u8, Some(true)));
        }
    }
    (best.0 >= 0.72).then_some(best.1)
}

/// Tempo written into a file name next to "bpm": "Loop_128bpm", "bpm 90 drums", "Pad 120 BPM".
pub fn bpm_from_name(name: &str) -> Option<f32> {
    let lower = name.to_lowercase();
    let b = lower.as_bytes();
    let mut from = 0;
    while let Some(off) = lower[from..].find("bpm") {
        let at = from + off;
        // number right before "bpm" (optionally separated by spaces / _ / -)
        let mut end = at;
        while end > 0 && matches!(b[end - 1], b' ' | b'_' | b'-') {
            end -= 1;
        }
        let mut start = end;
        while start > 0 && (b[start - 1].is_ascii_digit() || b[start - 1] == b'.') {
            start -= 1;
        }
        let mut cands: Vec<&str> = Vec::new();
        if start < end {
            cands.push(&lower[start..end]);
        }
        // ... or right after it
        let mut s2 = at + 3;
        while s2 < b.len() && matches!(b[s2], b' ' | b'_' | b'-') {
            s2 += 1;
        }
        let mut e2 = s2;
        while e2 < b.len() && (b[e2].is_ascii_digit() || b[e2] == b'.') {
            e2 += 1;
        }
        if s2 < e2 {
            cands.push(&lower[s2..e2]);
        }
        for c in cands {
            if let Ok(v) = c.trim_matches('.').parse::<f32>() {
                if (40.0..=300.0).contains(&v) {
                    return Some(v);
                }
            }
        }
        from = at + 3;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_bpm_in_names() {
        assert_eq!(bpm_from_name("Pad_Am_120bpm"), Some(120.0));
        assert_eq!(bpm_from_name("Drums 90 BPM"), Some(90.0));
        assert_eq!(bpm_from_name("bpm_174_break"), Some(174.0));
        assert_eq!(bpm_from_name("Kick_01"), None);
        assert_eq!(bpm_from_name("Loop 7bpm"), None);
    }

    #[test]
    fn finds_keys_in_names() {
        assert_eq!(key_from_name("Pad_Am_120bpm"), Some((9, Some(true))));
        assert_eq!(key_from_name("Lead - F# min"), Some((6, Some(true))));
        assert_eq!(key_from_name("Bass Bb"), Some((10, None)));
        assert_eq!(key_from_name("Strings Cmaj"), Some((0, Some(false))));
        assert_eq!(key_from_name("kick a one"), None);
        assert_eq!(key_from_name("Kick_01"), None);
    }

    fn tone(freqs: &[f32], secs: f32, sr: f32) -> Vec<f32> {
        (0..(secs * sr) as usize).map(|i| freqs.iter().map(|f| (std::f32::consts::TAU * f * i as f32 / sr).sin()).sum::<f32>() * 0.2).collect()
    }

    #[test]
    fn estimates_keys_from_audio() {
        let sr = 44_100.0;
        // single note A2 -> root only
        let a = tone(&[110.0], 1.0, sr);
        assert_eq!(estimate_key(&a, &a, sr), Some((9, None)));
        // F minor chord progression-ish: F, Ab, C, Eb, G (F natural minor notes)
        let f = [174.61, 207.65, 261.63, 311.13, 392.0, 349.23, 233.08];
        let m = tone(&f, 3.0, sr);
        let k = estimate_key(&m, &m, sr).expect("tonal");
        assert!(k == (5, Some(true)) || k == (8, Some(false)), "got {k:?}");
        // noise and silence -> nothing
        let mut seed = 12345u32;
        let noise: Vec<f32> = (0..88200)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed >> 8) as f32 / 8388608.0 - 1.0
            })
            .collect();
        assert_eq!(estimate_key(&noise, &noise, sr), None);
        assert_eq!(estimate_key(&vec![0.0; 44100], &vec![0.0; 44100], sr), None);
    }

    #[test]
    fn key_queries() {
        let am = key_from_name("loop_Am").unwrap();
        assert!(key_matches(Some(am), parse_key_query("a").unwrap()));
        assert!(key_matches(Some(am), parse_key_query("Am").unwrap()));
        assert!(!key_matches(Some(am), parse_key_query("A maj").unwrap()));
        assert!(!key_matches(Some(am), parse_key_query("C").unwrap()));
        assert_eq!(parse_key_query("zzz"), None);
    }

    #[test]
    fn length_queries() {
        assert_eq!(parse_len_query("1-3"), Some((1.0, 3.0)));
        assert_eq!(parse_len_query("<2"), Some((0.0, 2.0)));
        assert_eq!(parse_len_query(">5 s"), Some((5.0, f32::INFINITY)));
        assert!(parse_len_query("abc").is_none());
        let (lo, hi) = parse_len_query("2").unwrap();
        assert!(lo < 2.0 && hi > 2.0);
    }
}
