use crate::automation::{owned_value, AutoTarget};
use crate::clips::{ClipList, RtClipKind, MAX_TRACKS};
use crate::messages::{Command, Garbage, InstrumentKind};
use crate::model::PPQ;
use native_plugins::{Sampler, SimpleSynth};
use plugin_host::{EventKind as PEvent, MidiEvent, PluginProcessor, Transport};
use rtrb::{Consumer, Producer, RingBuffer};
use std::f32::consts::{FRAC_PI_4, SQRT_2};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

pub const MAX_BLOCK: usize = 512;
pub const FX_SLOTS: usize = 4;
const MAX_EVENTS: usize = 2048;
const MAX_AUDIO_VOICES: usize = 256;
const MAX_LIVE: usize = 128;

/// Lock-free meters and transport position shared with the GUI.
pub struct Meters {
    peaks: Box<[AtomicU32]>,
    position: AtomicU32,
}

impl Meters {
    fn new() -> Self {
        Self { peaks: (0..(MAX_TRACKS + 1) * 2).map(|_| AtomicU32::new(0)).collect(), position: AtomicU32::new(0) }
    }
    fn store(&self, idx: usize, l: f32, r: f32) {
        self.peaks[idx * 2].store(l.to_bits(), Ordering::Relaxed);
        self.peaks[idx * 2 + 1].store(r.to_bits(), Ordering::Relaxed);
    }
    /// Peak (left, right) since the last audio block; index `MAX_TRACKS` is the master.
    pub fn peak(&self, idx: usize) -> (f32, f32) {
        (f32::from_bits(self.peaks[idx * 2].load(Ordering::Relaxed)), f32::from_bits(self.peaks[idx * 2 + 1].load(Ordering::Relaxed)))
    }
    pub fn master(&self) -> (f32, f32) {
        self.peak(MAX_TRACKS)
    }
    /// Playhead in ticks.
    pub fn position(&self) -> u32 {
        self.position.load(Ordering::Relaxed)
    }
}

enum Instrument {
    None,
    Synth(SimpleSynth),
    Sampler(Sampler),
    Vst(Box<PluginProcessor>),
}

struct Track {
    instrument: Instrument,
    fx: [Option<Box<PluginProcessor>>; FX_SLOTS],
    gain: f32,
    pan: f32,
    mute: bool,
    solo: bool,
    l: Box<[f32]>,
    r: Box<[f32]>,
    has_audio: bool,
    /// Gains used at the end of the previous block: the next block ramps from them (no zipper noise
    /// when gain / pan / mute are automated or changed by hand).
    smooth_l: f32,
    smooth_r: f32,
    smooth_init: bool,
}

impl Track {
    fn new() -> Self {
        Self {
            instrument: Instrument::None,
            fx: std::array::from_fn(|_| None),
            gain: 0.9,
            pan: 0.0,
            mute: false,
            solo: false,
            l: vec![0.0; MAX_BLOCK].into_boxed_slice(),
            r: vec![0.0; MAX_BLOCK].into_boxed_slice(),
            has_audio: false,
            smooth_l: 0.0,
            smooth_r: 0.0,
            smooth_init: false,
        }
    }
    fn needs_processing(&self) -> bool {
        self.has_audio || !matches!(self.instrument, Instrument::None) || self.fx.iter().any(|f| f.is_some())
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Ev {
    On(u8, u8),
    Off(u8),
    AllOff,
}

#[derive(Clone, Copy)]
struct Sched {
    track: u16,
    frame: u32,
    ev: Ev,
}

const NO_EVENT: Sched = Sched { track: 0, frame: 0, ev: Ev::AllOff };

/// A playing audio clip. It refers to its clip by id/index (never clones the `Arc`),
/// so the audio thread can never perform the final free.
#[derive(Clone, Copy)]
struct AudioVoice {
    active: bool,
    id: u64,
    idx: usize,
    track: u16,
    pos: usize,
    remaining: u32,
    delay: u32,
}

const IDLE_VOICE: AudioVoice = AudioVoice { active: false, id: 0, idx: 0, track: 0, pos: 0, remaining: 0, delay: 0 };

/// Lives on the audio thread.
pub struct Engine {
    sample_rate: f32,
    playing: bool,
    bpm: f32,
    pos_ticks: f64,
    cycle: Option<(u32, u32)>,
    export_limit: Option<f64>,
    tracks: Vec<Track>,
    master_gain: f32,
    master_l: Box<[f32]>,
    master_r: Box<[f32]>,
    preview: Sampler,
    clips: Option<Box<ClipList>>,
    voices: [AudioVoice; MAX_AUDIO_VOICES],
    any_solo: bool,
    events: Box<[Sched]>,
    n_events: usize,
    live: Vec<Sched>,
    midi_tmp: Vec<MidiEvent>,
    cmd_rx: Consumer<Command>,
    garbage_tx: Producer<Garbage>,
    meters: Arc<Meters>,
}

/// Lives on the GUI thread.
pub struct EngineHandle {
    pub cmd_tx: Producer<Command>,
    garbage_rx: Consumer<Garbage>,
    pub meters: Arc<Meters>,
    /// Set by [`share_engine`]; used by the GUI to run offline exports.
    pub shared: Option<Arc<Mutex<Engine>>>,
}

impl EngineHandle {
    /// Returns `false` if the ring is full (command dropped).
    pub fn send(&mut self, cmd: Command) -> bool {
        self.cmd_tx.push(cmd).is_ok()
    }

    /// Free objects the audio thread has retired. Call this every UI frame.
    pub fn collect_garbage(&mut self) {
        while let Ok(g) = self.garbage_rx.pop() {
            drop(g);
        }
    }
}

/// Build the engine on the main thread (allocates freely here), then move it
/// into the audio callback closure.
pub fn create_engine(sample_rate: f32) -> (Engine, EngineHandle) {
    let (cmd_tx, cmd_rx) = RingBuffer::new(8192);
    let (garbage_tx, garbage_rx) = RingBuffer::new(1024);
    let meters = Arc::new(Meters::new());
    let engine = Engine {
        sample_rate,
        playing: false,
        bpm: 120.0,
        pos_ticks: 0.0,
        cycle: None,
        export_limit: None,
        tracks: (0..MAX_TRACKS).map(|_| Track::new()).collect(),
        master_gain: 0.9,
        master_l: vec![0.0; MAX_BLOCK].into_boxed_slice(),
        master_r: vec![0.0; MAX_BLOCK].into_boxed_slice(),
        preview: Sampler::new(),
        clips: Some(Box::new(ClipList::default())),
        voices: [IDLE_VOICE; MAX_AUDIO_VOICES],
        any_solo: false,
        events: vec![NO_EVENT; MAX_EVENTS].into_boxed_slice(),
        n_events: 0,
        live: Vec::with_capacity(MAX_LIVE),
        midi_tmp: Vec::with_capacity(MAX_EVENTS),
        cmd_rx,
        garbage_tx,
        meters: meters.clone(),
    };
    (engine, EngineHandle { cmd_tx, garbage_rx, meters, shared: None })
}

/// Wrap the engine for sharing between the audio callback and the exporter. The audio
/// callback must use `try_lock` (never block); the exporter holds the lock while rendering.
pub fn share_engine(engine: Engine, handle: &mut EngineHandle) -> Arc<Mutex<Engine>> {
    let shared = Arc::new(Mutex::new(engine));
    handle.shared = Some(shared.clone());
    shared
}

impl Engine {
    #[inline]
    fn tpf(&self) -> f64 {
        self.bpm as f64 * PPQ as f64 / 60.0 / self.sample_rate as f64
    }

    /// The audio callback body. `out` is interleaved stereo f32.
    /// REAL-TIME SAFE: no allocation, no locks, no I/O.
    pub fn process(&mut self, out: &mut [f32]) {
        self.drain_commands();
        for chunk in out.chunks_mut(MAX_BLOCK * 2) {
            self.process_block(chunk);
        }
        self.meters.position.store(self.pos_ticks as u32, Ordering::Relaxed);
    }

    fn retire(&mut self, g: Garbage) {
        // If the garbage ring is full we must NOT drop here (that would free on the
        // audio thread): leak instead.
        if let Err(rtrb::PushError::Full(g)) = self.garbage_tx.push(g) {
            std::mem::forget(g);
        }
    }

    fn drain_commands(&mut self) {
        while let Ok(cmd) = self.cmd_rx.pop() {
            match cmd {
                Command::Play => {
                    if !self.playing {
                        self.playing = true;
                        self.start_voices_at(self.pos_ticks, 0);
                    }
                }
                Command::Stop => {
                    self.playing = false;
                    self.stop_voices();
                    self.all_notes_off();
                }
                Command::Seek(t) => {
                    self.pos_ticks = t as f64;
                    self.apply_automation(t as f64);
                    if self.playing {
                        self.stop_voices();
                        self.all_notes_off();
                        self.start_voices_at(t as f64, 0);
                    }
                }
                Command::SetBpm(b) => self.bpm = b.clamp(10.0, 999.0),
                Command::SetCycle(c) => self.cycle = c.filter(|(s, e)| e > s),
                Command::NoteOn { track, key, velocity } => self.push_live(track, Ev::On(key, velocity)),
                Command::NoteOff { track, key } => self.push_live(track, Ev::Off(key)),
                Command::SetTrackGain { track, gain } => {
                    if let Some(t) = self.tracks.get_mut(track) { t.gain = gain; }
                }
                Command::SetTrackPan { track, pan } => {
                    if let Some(t) = self.tracks.get_mut(track) { t.pan = pan; }
                }
                Command::SetTrackMute { track, mute } => {
                    if let Some(t) = self.tracks.get_mut(track) { t.mute = mute; }
                }
                Command::SetTrackSolo { track, solo } => {
                    if let Some(t) = self.tracks.get_mut(track) { t.solo = solo; }
                    self.any_solo = self.tracks.iter().any(|t| t.solo);
                }
                Command::SetMasterGain(g) => self.master_gain = g,
                Command::SetInstrument { track, kind } => {
                    self.invalidate_automation();
                    let sr = self.sample_rate;
                    let old = match self.tracks.get_mut(track) {
                        Some(t) => {
                            let new = match kind {
                                InstrumentKind::None => Instrument::None,
                                InstrumentKind::Synth => Instrument::Synth(SimpleSynth::new(sr)),
                                InstrumentKind::Sampler(s) => {
                                    let mut sm = Sampler::new();
                                    sm.set_sample(s);
                                    Instrument::Sampler(sm)
                                }
                            };
                            std::mem::replace(&mut t.instrument, new)
                        }
                        None => continue,
                    };
                    if let Instrument::Vst(p) = old {
                        self.retire(Garbage::Plugin(p));
                    }
                }
                Command::SetWaveform { track, waveform } => {
                    if let Some(Track { instrument: Instrument::Synth(s), .. }) = self.tracks.get_mut(track) {
                        s.set_waveform(waveform);
                    }
                }
                Command::SetPlugin { track, slot, plugin } => {
                    self.invalidate_automation();
                    let old = match self.tracks.get_mut(track) {
                        Some(t) if slot == 0 => {
                            let new = match plugin {
                                Some(p) => Instrument::Vst(p),
                                None => Instrument::None,
                            };
                            match std::mem::replace(&mut t.instrument, new) {
                                Instrument::Vst(p) => Some(p),
                                _ => None,
                            }
                        }
                        Some(t) if slot <= FX_SLOTS => std::mem::replace(&mut t.fx[slot - 1], plugin),
                        _ => {
                            if let Some(p) = plugin {
                                self.retire(Garbage::Plugin(p));
                            }
                            continue;
                        }
                    };
                    if let Some(p) = old {
                        self.retire(Garbage::Plugin(p));
                    }
                }
                Command::SetPluginParam { track, slot, id, value } => {
                    if let Some(t) = self.tracks.get_mut(track) {
                        if slot == 0 {
                            if let Instrument::Vst(p) = &mut t.instrument {
                                p.set_param(id, value);
                            }
                        } else if let Some(Some(p)) = t.fx.get_mut(slot.wrapping_sub(1)) {
                            p.set_param(id, value);
                        }
                    }
                }
                Command::SetClips(list) => {
                    if let Some(old) = self.clips.replace(list) {
                        self.retire(Garbage::Clips(old));
                    }
                    self.apply_automation(self.pos_ticks);
                    // Re-bind running voices to the new list (by clip id).
                    if let Some(list) = &self.clips {
                        for v in self.voices.iter_mut().filter(|v| v.active) {
                            match list.clips.iter().position(|c| c.id == v.id) {
                                Some(i) => v.idx = i,
                                None => v.active = false,
                            }
                        }
                    }
                }
                Command::Preview(sample) => {
                    self.preview.set_sample(Some(sample));
                    self.preview.note_on(60, 110);
                }
                Command::StopPreview => self.preview.stop_all(),
            }
        }
    }

    /// Make the next `apply_automation` send every value again (a plug-in / instrument was replaced).
    fn invalidate_automation(&mut self) {
        if let Some(list) = self.clips.as_mut() {
            for a in list.automation.iter_mut() {
                a.applied = f32::NAN;
            }
        }
    }

    /// Set every automated parameter to the value its automation clips give at song position `pos`
    /// (ticks). Only parameters that have automation clips are touched. Allocation free.
    fn apply_automation(&mut self, pos: f64) {
        let Some(list) = self.clips.as_mut() else { return };
        for i in 0..list.automation.len() {
            let Some(v) = owned_value(&list.automation, i, pos) else { continue };
            let target = list.automation[i].target;
            let unchanged = list.automation[i].applied == v;
            list.automation[i].current = v;
            if unchanged {
                continue;
            }
            list.automation[i].applied = v;
            let p = target.to_param(v);
            match target {
                AutoTarget::TrackGain(t) => {
                    if let Some(tr) = self.tracks.get_mut(t as usize) {
                        tr.gain = p;
                    }
                }
                AutoTarget::TrackPan(t) => {
                    if let Some(tr) = self.tracks.get_mut(t as usize) {
                        tr.pan = p;
                    }
                }
                AutoTarget::TrackPitch(t) => match self.tracks.get_mut(t as usize) {
                    Some(Track { instrument: Instrument::Synth(s), .. }) => s.set_pitch(p),
                    Some(Track { instrument: Instrument::Sampler(s), .. }) => s.set_pitch(p),
                    _ => {}
                },
                AutoTarget::MasterGain => self.master_gain = p,
                AutoTarget::ClipGain(id) => {
                    if let Some(c) = list.clips.iter_mut().find(|c| c.id == id) {
                        c.gain_mul = p;
                    }
                }
                AutoTarget::Plugin { track, slot, id } => {
                    if let Some(t) = self.tracks.get_mut(track as usize) {
                        let slot = slot as usize;
                        if slot == 0 {
                            if let Instrument::Vst(pl) = &mut t.instrument {
                                pl.set_param(id, p as f64);
                            }
                        } else if let Some(Some(pl)) = t.fx.get_mut(slot - 1) {
                            pl.set_param(id, p as f64);
                        }
                    }
                }
            }
        }
    }

    fn push_live(&mut self, track: usize, ev: Ev) {
        if track < MAX_TRACKS && self.live.len() < MAX_LIVE {
            self.live.push(Sched { track: track as u16, frame: 0, ev });
        }
    }

    fn stop_voices(&mut self) {
        for v in &mut self.voices {
            v.active = false;
        }
    }

    /// Silence every instrument (called on stop / seek / loop).
    fn all_notes_off(&mut self) {
        for t in &mut self.tracks {
            match &mut t.instrument {
                Instrument::Synth(s) => (0..128u8).for_each(|k| s.note_off(k)),
                Instrument::Sampler(_) | Instrument::None => {}
                Instrument::Vst(p) => p.all_notes_off(),
            }
        }
    }

    // ---- song scheduling -------------------------------------------------

    fn push_event(&mut self, track: u16, frame: u32, ev: Ev) {
        if self.n_events < MAX_EVENTS {
            self.events[self.n_events] = Sched { track, frame, ev };
            self.n_events += 1;
        }
    }

    fn start_voice(&mut self, idx: usize, id: u64, track: u16, pos: usize, remaining: u32, delay: u32) {
        let slot = self.voices.iter().position(|v| !v.active).unwrap_or(0);
        self.voices[slot] = AudioVoice { active: true, id, idx, track, pos, remaining, delay };
    }

    /// Start voices for audio clips already under the playhead at `tick` (`delay` frames into the block).
    fn start_voices_at(&mut self, tick: f64, delay: u32) {
        let tpf = self.tpf();
        let Some(list) = self.clips.take() else { return };
        for (i, c) in list.clips.iter().enumerate() {
            if let RtClipKind::Audio { offset, frames, .. } = &c.kind {
                let s = c.start as f64;
                if s < tick {
                    let elapsed = ((tick - s) / tpf) as u32;
                    if elapsed < *frames {
                        self.start_voice(i, c.id, c.track, (*offset + elapsed) as usize, *frames - elapsed, delay);
                    }
                }
            }
        }
        self.clips = Some(list);
    }

    /// Schedule everything that starts or ends in absolute ticks [a, b); `frame0` is the
    /// block frame where tick `a` lands.
    fn schedule_song(&mut self, a: f64, mut b: f64, frame0: f64, tpf: f64) {
        if let Some(limit) = self.export_limit {
            if a >= limit {
                return;
            }
            b = b.min(limit);
        }
        let Some(list) = self.clips.take() else { return };
        for (i, c) in list.clips.iter().enumerate() {
            let s = c.start as f64;
            match &c.kind {
                RtClipKind::Audio { offset, frames, .. } => {
                    if s >= a && s < b {
                        let delay = (frame0 + (s - a) / tpf) as u32;
                        self.start_voice(i, c.id, c.track, *offset as usize, *frames, delay);
                    }
                }
                RtClipKind::Midi { notes, len } => {
                    let e = s + *len as f64;
                    if s >= b || e < a {
                        continue;
                    }
                    for n in notes {
                        let on = s + n.start as f64;
                        let off = s + (n.start + n.len).min(*len) as f64;
                        if on >= a && on < b && on < e {
                            self.push_event(c.track, (frame0 + (on - a) / tpf) as u32, Ev::On(n.key, n.velocity));
                        }
                        if off >= a && off < b {
                            self.push_event(c.track, (frame0 + (off - a) / tpf) as u32, Ev::Off(n.key));
                        }
                    }
                }
            }
        }
        self.clips = Some(list);
    }

    // ---- rendering ---------------------------------------------------------

    fn render_voices(&mut self, frames: usize) {
        let Some(list) = self.clips.take() else { return };
        for v in self.voices.iter_mut().filter(|v| v.active) {
            let Some(clip) = list.clips.get(v.idx) else {
                v.active = false;
                continue;
            };
            let RtClipKind::Audio { sample, offset, frames: total, params } = &clip.kind else {
                v.active = false;
                continue;
            };
            let track = &mut self.tracks[v.track as usize];
            track.has_audio = true;
            let n = sample.len();
            let clip_mul = clip.gain_mul;
            let start = (v.delay as usize).min(frames);
            for f in start..frames {
                if v.remaining == 0 {
                    v.active = false;
                    break;
                }
                // position inside the clip, counted in frames from its (trimmed) start
                let elapsed = total.saturating_sub(v.remaining);
                let idx = if params.reverse { (*offset + *total).saturating_sub(1 + elapsed) as usize } else { (*offset + elapsed) as usize };
                if idx >= n {
                    v.active = false;
                    break;
                }
                let mut g = 1.0f32;
                if params.fade_in > 0 && elapsed < params.fade_in {
                    g *= elapsed as f32 / params.fade_in as f32;
                }
                if params.fade_out > 0 && v.remaining < params.fade_out {
                    g *= v.remaining as f32 / params.fade_out as f32;
                }
                track.l[f] += sample.left[idx] * g * params.gain_l * clip_mul;
                track.r[f] += sample.right[idx] * g * params.gain_r * clip_mul;
                v.pos = idx;
                v.remaining -= 1;
            }
            v.delay = 0;
        }
        self.clips = Some(list);
    }

    fn process_block(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        for t in &mut self.tracks {
            t.l[..frames].fill(0.0);
            t.r[..frames].fill(0.0);
            t.has_audio = false;
        }
        self.n_events = 0;
        let tpf = self.tpf();
        if self.playing {
            self.apply_automation(self.pos_ticks);
        }

        // live (audition) events go first
        for i in 0..self.live.len() {
            let l = self.live[i];
            self.push_event(l.track, 0, l.ev);
        }
        self.live.clear();

        // ---- scheduling, with cycle (loop) handling ----
        if self.playing {
            let mut pos = self.pos_ticks;
            let mut done = 0usize;
            while done < frames {
                let remaining = frames - done;
                let wrap_at = match self.cycle {
                    Some((_, le)) if self.export_limit.is_none() => {
                        if pos >= le as f64 {
                            Some(0) // already at / past the loop end: wrap immediately
                        } else {
                            let need = ((le as f64 - pos) / tpf - 1e-6).ceil() as usize;
                            (need <= remaining).then_some(need.max(1))
                        }
                    }
                    _ => None,
                };
                match wrap_at {
                    Some(span) => {
                        let le = self.cycle.map(|c| c.1).unwrap_or(0) as f64;
                        let ls = self.cycle.map(|c| c.0).unwrap_or(0) as f64;
                        if span > 0 {
                            self.schedule_song(pos, le, done as f64, tpf);
                        }
                        let wrap_frame = (done + span) as u32;
                        // clip voices at the loop end, release notes, restart from the loop start
                        for v in self.voices.iter_mut().filter(|v| v.active) {
                            v.remaining = v.remaining.min(wrap_frame.saturating_sub(v.delay));
                        }
                        for t in 0..MAX_TRACKS {
                            if !matches!(self.tracks[t].instrument, Instrument::None) {
                                self.push_event(t as u16, wrap_frame.min(frames as u32 - 1), Ev::AllOff);
                            }
                        }
                        pos = ls;
                        self.start_voices_at(ls, wrap_frame);
                        done += span;
                    }
                    None => {
                        let end = ((pos + remaining as f64 * tpf) * 1e6).round() / 1e6;
                        self.schedule_song(pos, end, done as f64, tpf);
                        pos = end;
                        done = frames;
                    }
                }
            }
            self.pos_ticks = pos;
        }
        // (track, frame, note-off before note-on): in-place, allocation-free
        self.events[..self.n_events].sort_unstable_by_key(|e| (e.track, e.frame, matches!(e.ev, Ev::On(..))));

        self.render_voices(frames);

        // ---- per track: instrument -> inserts -> gain/pan -> master ----
        self.master_l[..frames].fill(0.0);
        self.master_r[..frames].fill(0.0);
        let transport = Transport { playing: self.playing, tempo: self.bpm as f64, quarters: self.pos_ticks / PPQ as f64 };
        let mut ei = 0usize;
        for ti in 0..MAX_TRACKS {
            let start = ei;
            while ei < self.n_events && self.events[ei].track as usize == ti {
                ei += 1;
            }
            let evs = &self.events[start..ei];
            let track = &mut self.tracks[ti];
            if !track.needs_processing() {
                self.meters.store(ti, 0.0, 0.0);
                continue;
            }
            render_instrument(track, evs, &mut self.midi_tmp, frames, &transport);
            for fx in track.fx.iter_mut().flatten() {
                fx.process_effect(&mut track.l[..frames], &mut track.r[..frames], &transport);
            }
            let audible = !track.mute && (!self.any_solo || track.solo);
            let g = if audible { track.gain } else { 0.0 };
            let angle = (track.pan.clamp(-1.0, 1.0) + 1.0) * FRAC_PI_4;
            let (tgl, tgr) = (g * angle.cos() * SQRT_2, g * angle.sin() * SQRT_2);
            if !track.smooth_init {
                track.smooth_l = tgl;
                track.smooth_r = tgr;
                track.smooth_init = true;
            }
            let (from_l, from_r) = (track.smooth_l, track.smooth_r);
            let inv = 1.0 / frames as f32;
            let (mut pl, mut pr) = (0.0f32, 0.0f32);
            for n in 0..frames {
                // linear ramp from last block's gain to this block's gain
                let k = (n + 1) as f32 * inv;
                let gl = from_l + (tgl - from_l) * k;
                let gr = from_r + (tgr - from_r) * k;
                let (l, r) = (track.l[n] * gl, track.r[n] * gr);
                pl = pl.max(l.abs());
                pr = pr.max(r.abs());
                self.master_l[n] += l;
                self.master_r[n] += r;
            }
            track.smooth_l = tgl;
            track.smooth_r = tgr;
            self.meters.store(ti, pl, pr);
        }

        // ---- master ----
        self.preview.process(&mut self.master_l[..frames], &mut self.master_r[..frames]);
        let (mut ml, mut mr) = (0.0f32, 0.0f32);
        for n in 0..frames {
            // tanh soft clip protects the DAC / speakers from overs.
            let l = (self.master_l[n] * self.master_gain).tanh();
            let r = (self.master_r[n] * self.master_gain).tanh();
            ml = ml.max(l.abs());
            mr = mr.max(r.abs());
            out[2 * n] = l;
            out[2 * n + 1] = r;
        }
        self.meters.store(MAX_TRACKS, ml, mr);
    }

    // ---- offline export ------------------------------------------------------

    /// Render `[start, end)` ticks (plus `tail_secs` for reverb/delay tails) faster than real
    /// time. Call with the engine locked (the audio callback then outputs silence).
    /// Returns interleaved stereo f32. `progress` runs 0..=1000.
    pub fn export_render(&mut self, start: u32, end: u32, tail_secs: f32, progress: &AtomicU32, cancel: &AtomicBool) -> Vec<f32> {
        self.drain_commands();
        let saved = (self.playing, self.pos_ticks, self.cycle);
        self.cycle = None;
        self.export_limit = Some(end as f64);
        self.stop_voices();
        self.all_notes_off();
        self.pos_ticks = start as f64;
        self.playing = true;
        self.start_voices_at(start as f64, 0);

        let main_frames = (((end.saturating_sub(start)) as f64 / self.tpf()).ceil() as usize).max(1);
        let total = main_frames + (tail_secs.max(0.0) * self.sample_rate) as usize;
        let mut pcm: Vec<f32> = Vec::with_capacity(total * 2);
        let mut buf = vec![0.0f32; MAX_BLOCK * 2];
        let mut done = 0usize;
        while done < total {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let n = (total - done).min(MAX_BLOCK);
            self.process_block(&mut buf[..n * 2]);
            pcm.extend_from_slice(&buf[..n * 2]);
            done += n;
            progress.store((done as u64 * 1000 / total as u64) as u32, Ordering::Relaxed);
        }
        self.stop_voices();
        self.all_notes_off();
        self.export_limit = None;
        self.playing = false;
        self.pos_ticks = saved.1;
        self.cycle = saved.2;
        let _ = saved.0;
        pcm
    }
}

fn render_instrument(track: &mut Track, evs: &[Sched], tmp: &mut Vec<MidiEvent>, frames: usize, tr: &Transport) {
    let Track { instrument, l, r, .. } = track;
    match instrument {
        Instrument::None => {}
        Instrument::Synth(s) => {
            let mut cursor = 0usize;
            for e in evs {
                let f = (e.frame as usize).min(frames);
                s.process(&mut l[cursor..f], &mut r[cursor..f]);
                cursor = f;
                match e.ev {
                    Ev::On(k, v) => s.note_on(k, v),
                    Ev::Off(k) => s.note_off(k),
                    Ev::AllOff => (0..128u8).for_each(|k| s.note_off(k)),
                }
            }
            s.process(&mut l[cursor..frames], &mut r[cursor..frames]);
        }
        Instrument::Sampler(s) => {
            let mut cursor = 0usize;
            for e in evs {
                let f = (e.frame as usize).min(frames);
                s.process(&mut l[cursor..f], &mut r[cursor..f]);
                cursor = f;
                if let Ev::On(k, v) = e.ev {
                    s.note_on(k, v);
                }
            }
            s.process(&mut l[cursor..frames], &mut r[cursor..frames]);
        }
        Instrument::Vst(p) => {
            tmp.clear();
            for e in evs {
                let kind = match e.ev {
                    Ev::On(key, velocity) => PEvent::NoteOn { key, velocity },
                    Ev::Off(key) => PEvent::NoteOff { key },
                    Ev::AllOff => {
                        p.all_notes_off();
                        continue;
                    }
                };
                if tmp.len() < tmp.capacity() {
                    tmp.push(MidiEvent { frame: e.frame, kind });
                }
            }
            p.process_instrument(tmp, &mut l[..frames], &mut r[..frames], tr);
        }
    }
}
