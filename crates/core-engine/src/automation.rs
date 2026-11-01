//! Automation shared by the GUI and the audio thread: what can be automated, the curve between
//! two points, and the rules that decide the value of a parameter at a song position.
//!
//! Values are stored normalized (0..1) and mapped to the real parameter by [`AutoTarget::to_param`].
//!
//! Rules (the same code runs in the engine and in the GUI, so what you see is what you hear):
//!  * inside an automation clip the parameter follows its curve;
//!  * when several clips cover the same parameter at the same time, the one that **starts latest**
//!    wins (ties: the one created later). Clips never add up or fight;
//!  * between clips the parameter holds the last value of the clip that ended most recently;
//!  * before the first clip it holds that clip's first value;
//!  * parameters without automation clips are never touched.

/// Something an automation clip can control.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AutoTarget {
    /// Mixer channel / instrument volume (0..1.5).
    TrackGain(u16),
    /// Pan (-1..1).
    TrackPan(u16),
    /// Pitch of the built-in synth / sampler on a track (-12..+12 semitones).
    TrackPitch(u16),
    /// Master volume (0..1.5).
    MasterGain,
    /// Volume of one audio clip (x0..x2, unity at the middle).
    ClipGain(u64),
    /// Any parameter of a VST3 plug-in: slot 0 = instrument, 1.. = insert effects.
    Plugin { track: u16, slot: u8, id: u32 },
}

impl AutoTarget {
    /// Value in the parameter's own unit for a normalized `v` (0..1).
    pub fn to_param(self, v: f32) -> f32 {
        let v = v.clamp(0.0, 1.0);
        match self {
            AutoTarget::TrackGain(_) | AutoTarget::MasterGain => v * 1.5,
            AutoTarget::TrackPan(_) => v * 2.0 - 1.0,
            AutoTarget::TrackPitch(_) => (v - 0.5) * 24.0,
            AutoTarget::ClipGain(_) => v * 2.0,
            AutoTarget::Plugin { .. } => v,
        }
    }

    /// Inverse of [`to_param`](Self::to_param).
    pub fn from_param(self, p: f32) -> f32 {
        let v = match self {
            AutoTarget::TrackGain(_) | AutoTarget::MasterGain => p / 1.5,
            AutoTarget::TrackPan(_) => (p + 1.0) * 0.5,
            AutoTarget::TrackPitch(_) => p / 24.0 + 0.5,
            AutoTarget::ClipGain(_) => p / 2.0,
            AutoTarget::Plugin { .. } => p,
        };
        v.clamp(0.0, 1.0)
    }

    /// Human readable value for the editor ("-3.0 dB", "L 40%", "+2.0 st", "0.37").
    pub fn format(self, v: f32) -> String {
        let p = self.to_param(v);
        match self {
            AutoTarget::TrackGain(_) | AutoTarget::MasterGain => format!("{:.0}%", p / 0.9 * 100.0),
            AutoTarget::ClipGain(_) => {
                if p < 0.001 {
                    "-inf dB".to_string()
                } else {
                    format!("{:+.1} dB", 20.0 * p.log10())
                }
            }
            AutoTarget::TrackPan(_) => {
                if p.abs() < 0.005 {
                    "C".to_string()
                } else if p < 0.0 {
                    format!("L {:.0}%", -p * 100.0)
                } else {
                    format!("R {:.0}%", p * 100.0)
                }
            }
            AutoTarget::TrackPitch(_) => format!("{p:+.2} st"),
            AutoTarget::Plugin { .. } => format!("{p:.3}"),
        }
    }
}

/// How the value travels from one point to the next.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Shape {
    #[default]
    Linear,
    /// Holds the value and jumps at the next point.
    Step,
    /// S-curve (smoothstep): gentle start and end.
    Smooth,
    /// Slow start, fast end.
    EaseIn,
    /// Fast start, slow end.
    EaseOut,
}

impl Shape {
    pub const ALL: [Shape; 5] = [Shape::Linear, Shape::Step, Shape::Smooth, Shape::EaseIn, Shape::EaseOut];

    pub fn label(self) -> &'static str {
        match self {
            Shape::Linear => "Linear",
            Shape::Step => "Step",
            Shape::Smooth => "Smooth (S-curve)",
            Shape::EaseIn => "Ease in",
            Shape::EaseOut => "Ease out",
        }
    }

    /// 0..1 -> 0..1
    pub fn curve(self, u: f32) -> f32 {
        let u = u.clamp(0.0, 1.0);
        match self {
            Shape::Linear => u,
            Shape::Step => 0.0,
            Shape::Smooth => u * u * (3.0 - 2.0 * u),
            Shape::EaseIn => u * u,
            Shape::EaseOut => 1.0 - (1.0 - u) * (1.0 - u),
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            Shape::Linear => 0,
            Shape::Step => 1,
            Shape::Smooth => 2,
            Shape::EaseIn => 3,
            Shape::EaseOut => 4,
        }
    }

    pub fn from_u8(v: u8) -> Shape {
        match v {
            1 => Shape::Step,
            2 => Shape::Smooth,
            3 => Shape::EaseIn,
            4 => Shape::EaseOut,
            _ => Shape::Linear,
        }
    }
}

/// One breakpoint. `tick` is relative to the start of the clip; `shape` describes the way to the
/// next point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoPoint {
    pub tick: u32,
    pub value: f32,
    pub shape: Shape,
}

/// Value of a point list (sorted by tick) at clip-relative position `t` (ticks).
pub fn eval_points(points: &[AutoPoint], t: f32) -> f32 {
    let Some(first) = points.first() else { return 0.5 };
    if t <= first.tick as f32 {
        return first.value;
    }
    let last = points[points.len() - 1];
    if t >= last.tick as f32 {
        return last.value;
    }
    // first point strictly after t
    let i = points.partition_point(|p| (p.tick as f32) <= t);
    let (a, b) = (points[i - 1], points[i]);
    let span = (b.tick - a.tick).max(1) as f32;
    let u = (t - a.tick as f32) / span;
    a.value + (b.value - a.value) * a.shape.curve(u)
}

/// An automation clip as the audio thread sees it.
pub struct RtAuto {
    /// Id of the playlist clip (used by the GUI for bookkeeping only).
    pub id: u64,
    pub target: AutoTarget,
    pub start: u32,
    pub len: u32,
    pub points: Vec<AutoPoint>,
    /// Engine bookkeeping: last value that was applied (NaN = nothing yet).
    pub applied: f32,
    /// Engine bookkeeping: current normalized value (read by audio voices for `ClipGain`).
    pub current: f32,
}

impl RtAuto {
    pub fn new(id: u64, target: AutoTarget, start: u32, len: u32, points: Vec<AutoPoint>) -> Self {
        Self { id, target, start, len, points, applied: f32::NAN, current: 0.5 }
    }

    fn end(&self) -> u32 {
        self.start.saturating_add(self.len)
    }
}

/// Normalized value of `autos[i].target` at song position `pos` (ticks), but only when `autos[i]`
/// is the clip that owns that parameter at `pos`; otherwise `None` (another clip decides).
/// See the module docs for the rules.
pub fn owned_value(autos: &[RtAuto], i: usize, pos: f64) -> Option<f32> {
    let me = &autos[i];
    let mut winner: Option<usize> = None;
    // 1. clips that cover `pos`: the latest start wins
    for (j, a) in autos.iter().enumerate() {
        if a.target == me.target && (a.start as f64) <= pos && pos < a.end() as f64 {
            winner = match winner {
                Some(w) if autos[w].start > a.start => Some(w),
                _ => Some(j), // later start, or equal start but created later
            };
        }
    }
    let (w, rel) = match winner {
        Some(w) => (w, (pos - autos[w].start as f64) as f32),
        None => {
            // 2. hold the value of the clip that ended last before `pos`
            let mut prev: Option<usize> = None;
            for (j, a) in autos.iter().enumerate() {
                if a.target == me.target && (a.end() as f64) <= pos {
                    prev = match prev {
                        Some(p) if autos[p].end() > a.end() => Some(p),
                        _ => Some(j),
                    };
                }
            }
            match prev {
                Some(p) => (p, autos[p].len as f32),
                None => {
                    // 3. before every clip: the first value of the earliest clip
                    let mut next: Option<usize> = None;
                    for (j, a) in autos.iter().enumerate() {
                        if a.target == me.target {
                            next = match next {
                                Some(n) if autos[n].start <= a.start => Some(n),
                                _ => Some(j),
                            };
                        }
                    }
                    (next?, 0.0)
                }
            }
        }
    };
    if w != i {
        return None;
    }
    Some(eval_points(&autos[w].points, rel))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(tick: u32, value: f32, shape: Shape) -> AutoPoint {
        AutoPoint { tick, value, shape }
    }

    #[test]
    fn shapes_connect_the_points() {
        let pts = [p(0, 0.0, Shape::Linear), p(100, 1.0, Shape::Step), p(200, 0.0, Shape::Smooth), p(300, 1.0, Shape::Linear)];
        assert_eq!(eval_points(&pts, -5.0), 0.0);
        assert!((eval_points(&pts, 50.0) - 0.5).abs() < 1e-6, "linear");
        assert_eq!(eval_points(&pts, 150.0), 1.0, "step holds");
        assert!((eval_points(&pts, 250.0) - 0.5).abs() < 1e-6, "smooth midpoint");
        assert_eq!(eval_points(&pts, 999.0), 1.0, "after the last point");
        assert_eq!(eval_points(&[], 3.0), 0.5);
    }

    #[test]
    fn parameter_mapping_round_trips() {
        let targets = [AutoTarget::TrackGain(0), AutoTarget::TrackPan(1), AutoTarget::TrackPitch(2), AutoTarget::MasterGain, AutoTarget::ClipGain(7), AutoTarget::Plugin { track: 0, slot: 1, id: 5 }];
        for t in targets {
            for v in [0.0f32, 0.25, 0.5, 1.0] {
                assert!((t.from_param(t.to_param(v)) - v).abs() < 1e-5);
            }
        }
        assert_eq!(AutoTarget::TrackPan(0).to_param(0.5), 0.0);
        assert_eq!(AutoTarget::ClipGain(1).to_param(0.5), 1.0);
    }

    fn auto(id: u64, start: u32, len: u32, a: f32, b: f32) -> RtAuto {
        RtAuto::new(id, AutoTarget::TrackGain(0), start, len, vec![p(0, a, Shape::Linear), p(len, b, Shape::Linear)])
    }

    /// Value that actually wins at `pos` (exactly one clip may own the parameter).
    fn at(autos: &[RtAuto], pos: f64) -> Option<f32> {
        let owners: Vec<f32> = (0..autos.len()).filter_map(|i| owned_value(autos, i, pos)).collect();
        assert!(owners.len() <= 1, "two clips drive one parameter at {pos}");
        owners.first().copied()
    }

    #[test]
    fn clips_hold_before_between_and_after() {
        let autos = [auto(1, 1000, 1000, 0.2, 0.8), auto(2, 3000, 1000, 0.5, 0.1)];
        assert!((at(&autos, 0.0).unwrap() - 0.2).abs() < 1e-6, "before the first clip: its first value");
        assert!((at(&autos, 1500.0).unwrap() - 0.5).abs() < 1e-6, "inside");
        assert!((at(&autos, 2500.0).unwrap() - 0.8).abs() < 1e-6, "between: last value of the earlier clip");
        assert!((at(&autos, 3500.0).unwrap() - 0.3).abs() < 1e-6, "inside the second clip");
        assert!((at(&autos, 9000.0).unwrap() - 0.1).abs() < 1e-6, "after everything");
    }

    #[test]
    fn overlapping_clips_never_fight_the_later_start_wins() {
        let autos = [auto(1, 0, 4000, 0.0, 1.0), auto(2, 1000, 1000, 0.9, 0.9)];
        assert!((at(&autos, 500.0).unwrap() - 0.125).abs() < 1e-6);
        assert!((at(&autos, 1500.0).unwrap() - 0.9).abs() < 1e-6, "the clip that starts later takes over");
        assert!((at(&autos, 2500.0).unwrap() - 0.625).abs() < 1e-6, "the first clip continues where its own curve is");
        // same start: the one created later wins
        let same = [auto(1, 0, 1000, 0.1, 0.1), auto(2, 0, 1000, 0.7, 0.7)];
        assert!((at(&same, 100.0).unwrap() - 0.7).abs() < 1e-6);
    }

    #[test]
    fn different_parameters_are_independent() {
        let mut a = auto(1, 0, 1000, 0.2, 0.2);
        let mut b = auto(2, 0, 1000, 0.8, 0.8);
        a.target = AutoTarget::TrackGain(0);
        b.target = AutoTarget::TrackGain(1);
        let autos = [a, b];
        assert!((owned_value(&autos, 0, 10.0).unwrap() - 0.2).abs() < 1e-6);
        assert!((owned_value(&autos, 1, 10.0).unwrap() - 0.8).abs() < 1e-6);
    }
}
