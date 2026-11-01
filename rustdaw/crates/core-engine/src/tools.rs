//! Piano-roll editing tools. Pure functions on note lists (GUI thread only).
use crate::model::Note;

/// Snap note starts and lengths to a grid (in ticks).
pub fn quantize(notes: &mut [Note], grid: u32) {
    let grid = grid.max(1);
    for n in notes {
        n.start = ((n.start + grid / 2) / grid) * grid;
        n.len = ((n.len + grid / 2) / grid).max(1) * grid;
    }
}

/// Split every note into `parts` equal pieces. Pieces are gated to 75% so
/// adjacent same-pitch notes retrigger cleanly.
pub fn chop(notes: &mut Vec<Note>, parts: u32) {
    let parts = parts.max(1);
    let mut out = Vec::with_capacity(notes.len() * parts as usize);
    for n in notes.iter() {
        let piece = (n.len / parts).max(1);
        for i in 0..parts {
            out.push(Note { start: n.start + i * piece, len: (piece * 3 / 4).max(1), ..*n });
        }
    }
    *notes = out;
}

#[cfg(test)]
mod tests {
    use super::*;
    fn n(start: u32, len: u32) -> Note {
        Note { key: 60, velocity: 100, start, len }
    }
    #[test]
    fn quantize_snaps() {
        let mut v = vec![n(130, 200)];
        quantize(&mut v, 240);
        assert_eq!((v[0].start, v[0].len), (240, 240));
    }
    #[test]
    fn chop_splits() {
        let mut v = vec![n(0, 480)];
        chop(&mut v, 2);
        assert_eq!(v.len(), 2);
        assert_eq!((v[1].start, v[1].len), (240, 180));
    }
}
