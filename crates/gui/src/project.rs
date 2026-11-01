//! Project files: tempo, patterns and the whole playlist, including every audio clip's pitch / BPM /
//! trim / fade / gain / pan / reverse settings.
//!
//! A project is `<name>.rdaw` (JSON) plus a `<name>_audio` folder that holds the untouched source of
//! each audio clip as 32-bit float WAV, so the saved settings can always be re-applied.
//! Automation clips (target parameter + every point) are saved with the playlist.
//! Track names, mixer settings (volume, pan, mute, solo) and built-in instruments (synth + waveform,
//! sampler by sample name) are saved too, and so are VST3 instruments / effects: which plug-in sits
//! in which slot, its full VST3 state (sounds, presets, sample banks: whatever the plug-in stores,
//! kept in `<name>_plugins/*.bin`) and, as a fallback, every parameter that differs from its default.
use crate::state::*;
use core_engine::{AutoPoint, AutoTarget, Command, Note, Shape, FX_SLOTS};
use dsp::SampleData;
use project_io::{write_audio, ExportFormat};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    version: u32,
    bpm: f32,
    patterns: Vec<PatternFile>,
    clips: Vec<ClipFile>,
    #[serde(default)]
    tracks: Vec<TrackFile>,
}

#[derive(Serialize, Deserialize)]
struct TrackFile {
    index: usize,
    name: String,
    gain: f32,
    pan: f32,
    mute: bool,
    solo: bool,
    /// "none", "basic", "sampler:<library sample name>" or "vst" (not restored).
    #[serde(default)]
    inst: String,
    /// 0 = sine, 1 = saw, 2 = square, 3 = triangle
    #[serde(default)]
    wave: u8,
    /// VST3 instrument (slot 0) and effects (slots 1..) on this track.
    #[serde(default)]
    plugins: Vec<PluginFile>,
}

#[derive(Serialize, Deserialize)]
struct PluginFile {
    slot: usize,
    name: String,
    /// Hex VST3 class id (empty when the plug-in was not in the scanned list when saving).
    #[serde(default)]
    class_id: String,
    /// (parameter id, normalized value) for every parameter that is not at its default.
    #[serde(default)]
    params: Vec<(u32, f64)>,
    /// Full plug-in state, relative to the project file (empty when the plug-in gave none).
    #[serde(default)]
    state_file: String,
}

fn hex_id(id: &[u8; 16]) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}

fn inst_to_string(i: &InstUi) -> String {
    match i {
        InstUi::None => "none".to_string(),
        InstUi::Basic => "basic".to_string(),
        InstUi::Sampler(Some(n)) => format!("sampler:{n}"),
        InstUi::Sampler(None) => "sampler:".to_string(),
        InstUi::Vst(_) => "vst".to_string(),
    }
}

fn wave_to_u8(w: dsp::Waveform) -> u8 {
    match w {
        dsp::Waveform::Sine => 0,
        dsp::Waveform::Saw => 1,
        dsp::Waveform::Square => 2,
        dsp::Waveform::Triangle => 3,
    }
}

fn wave_from_u8(v: u8) -> dsp::Waveform {
    match v {
        1 => dsp::Waveform::Saw,
        2 => dsp::Waveform::Square,
        3 => dsp::Waveform::Triangle,
        _ => dsp::Waveform::Sine,
    }
}

#[derive(Serialize, Deserialize)]
struct NoteFile {
    key: u8,
    vel: u8,
    start: u32,
    len: u32,
}

#[derive(Serialize, Deserialize)]
struct PatternFile {
    id: u64,
    name: String,
    len: u32,
    notes: Vec<NoteFile>,
}

#[derive(Serialize, Deserialize, Clone)]
struct FxFile {
    semitones: f32,
    tape: bool,
    sample_bpm: Option<f32>,
    show_wave: bool,
    gain_db: f32,
    pan: f32,
    fade_in_ms: f32,
    fade_out_ms: f32,
    reverse: bool,
    /// 0 = Fast, 1 = Normal, 2 = Smooth
    quality: u8,
}

/// What an automation clip controls. `kind` is "gain", "pan", "pitch", "master", "clip" or "plugin".
#[derive(Serialize, Deserialize, Clone)]
struct AutoTargetFile {
    kind: String,
    #[serde(default)]
    track: u16,
    #[serde(default)]
    slot: u8,
    #[serde(default)]
    id: u32,
    /// For "clip": position of the audio clip in the saved clip list (clip ids change between runs).
    #[serde(default)]
    clip: i64,
}

fn target_to_file(t: AutoTarget, clips: &[UiClip]) -> AutoTargetFile {
    let mut f = AutoTargetFile { kind: String::new(), track: 0, slot: 0, id: 0, clip: -1 };
    match t {
        AutoTarget::TrackGain(i) => {
            f.kind = "gain".into();
            f.track = i;
        }
        AutoTarget::TrackPan(i) => {
            f.kind = "pan".into();
            f.track = i;
        }
        AutoTarget::TrackPitch(i) => {
            f.kind = "pitch".into();
            f.track = i;
        }
        AutoTarget::MasterGain => f.kind = "master".into(),
        AutoTarget::ClipGain(cid) => {
            f.kind = "clip".into();
            f.clip = clips.iter().position(|c| c.id == cid).map(|p| p as i64).unwrap_or(-1);
        }
        AutoTarget::Plugin { track, slot, id } => {
            f.kind = "plugin".into();
            f.track = track;
            f.slot = slot;
            f.id = id;
        }
    }
    f
}

/// For "clip" targets the result holds the clip's *index* for now; it is replaced by the new clip id
/// once all clips exist.
fn target_from_file(f: &AutoTargetFile) -> Option<AutoTarget> {
    Some(match f.kind.as_str() {
        "gain" => AutoTarget::TrackGain(f.track),
        "pan" => AutoTarget::TrackPan(f.track),
        "pitch" => AutoTarget::TrackPitch(f.track),
        "master" => AutoTarget::MasterGain,
        "clip" => AutoTarget::ClipGain(if f.clip < 0 { u64::MAX } else { f.clip as u64 }),
        "plugin" => AutoTarget::Plugin { track: f.track, slot: f.slot, id: f.id },
        _ => return None,
    })
}

#[derive(Serialize, Deserialize)]
enum ClipKindFile {
    Automation {
        target: AutoTargetFile,
        /// (tick, normalized value, curve shape of the segment after the point)
        points: Vec<(u32, f32, u8)>,
        len: u32,
    },
    Pattern {
        pat: u64,
        len: u32,
    },
    Audio {
        /// Source WAV, relative to the project file.
        file: String,
        /// Start offset and length as fractions of the (processed) sample.
        off_frac: f64,
        frames_frac: f64,
        fx: FxFile,
    },
}

#[derive(Serialize, Deserialize)]
struct ClipFile {
    track: usize,
    start: u32,
    name: String,
    kind: ClipKindFile,
}

fn fx_to_file(fx: &AudioFx) -> FxFile {
    FxFile {
        semitones: fx.semitones,
        tape: fx.tape,
        sample_bpm: fx.sample_bpm,
        show_wave: fx.show_wave,
        gain_db: fx.gain_db,
        pan: fx.pan,
        fade_in_ms: fx.fade_in_ms,
        fade_out_ms: fx.fade_out_ms,
        reverse: fx.reverse,
        quality: match fx.quality {
            dsp::StretchQuality::Fast => 0,
            dsp::StretchQuality::Normal => 1,
            dsp::StretchQuality::Smooth => 2,
        },
    }
}

fn fx_from_file(f: &FxFile) -> AudioFx {
    AudioFx {
        semitones: f.semitones.clamp(-24.0, 24.0),
        tape: f.tape,
        sample_bpm: f.sample_bpm.map(|b| b.clamp(40.0, 300.0)),
        show_wave: f.show_wave,
        gain_db: f.gain_db.clamp(-60.0, 12.0),
        pan: f.pan.clamp(-1.0, 1.0),
        fade_in_ms: f.fade_in_ms.max(0.0),
        fade_out_ms: f.fade_out_ms.max(0.0),
        reverse: f.reverse,
        quality: match f.quality {
            0 => dsp::StretchQuality::Fast,
            2 => dsp::StretchQuality::Smooth,
            _ => dsp::StretchQuality::Normal,
        },
    }
}

impl State {
    fn plugins_of_track(&self, t: usize, plugin_dir: &Path, plugin_dir_name: &str) -> Vec<PluginFile> {
        let mut out: Vec<PluginFile> = Vec::new();
        for slot in 0..=FX_SLOTS {
            let name: Option<String> = if slot == 0 {
                match &self.tracks[t].inst {
                    InstUi::Vst(n) => Some(n.clone()),
                    _ => None,
                }
            } else {
                self.tracks[t].fx[slot - 1].clone()
            };
            let Some(name) = name else { continue };
            let class_id = self.plugins.iter().find(|p| p.name == name).map(|p| hex_id(&p.class_id)).unwrap_or_default();
            let params: Vec<(u32, f64)> = match self.ctrls.get(&(t, slot)) {
                Some(c) => c
                    .params
                    .iter()
                    .filter_map(|d| {
                        let v = c.get(d.id);
                        if (v - d.default).abs() > 1e-6 {
                            Some((d.id, v))
                        } else {
                            None
                        }
                    })
                    .collect(),
                None => Vec::new(),
            };
            let mut state_file = String::new();
            if let Some(bytes) = self.ctrls.get(&(t, slot)).and_then(|c| c.save_state()) {
                let file_name = format!("t{t:03}_s{slot}.bin");
                if std::fs::create_dir_all(plugin_dir).is_ok() && std::fs::write(plugin_dir.join(&file_name), bytes).is_ok() {
                    state_file = format!("{plugin_dir_name}/{file_name}");
                }
            }
            out.push(PluginFile { slot, name, class_id, params, state_file });
        }
        out
    }

    fn apply_plugin_params(&mut self, t: usize, slot: usize, params: &[(u32, f64)]) {
        if let Some(c) = self.ctrls.get(&(t, slot)) {
            for &(id, v) in params {
                c.set(id, v);
            }
        }
        for &(id, value) in params {
            self.handle.send(Command::SetPluginParam { track: t, slot, id, value });
        }
    }

    pub fn save_project(&self, path: &Path) -> Result<(), String> {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("project").to_string();
        let audio_dir_name = format!("{stem}_audio");
        let audio_dir = path.with_file_name(&audio_dir_name);
        let plugin_dir_name = format!("{stem}_plugins");
        let plugin_dir = path.with_file_name(&plugin_dir_name);
        let mut files: HashMap<usize, String> = HashMap::new();
        let mut clips: Vec<ClipFile> = Vec::new();
        for c in &self.clips {
            let kind = match &c.body {
                ClipBody::Pattern { pat, len } => ClipKindFile::Pattern { pat: *pat, len: *len },
                ClipBody::Automation { auto } => ClipKindFile::Automation { target: target_to_file(auto.target, &self.clips), points: auto.points.iter().map(|p| (p.tick, p.value, p.shape.to_u8())).collect(), len: auto.len },
                ClipBody::Audio { orig, sample, offset, frames, fx, .. } => {
                    let key = Arc::as_ptr(orig) as usize;
                    let rel = match files.get(&key) {
                        Some(r) => r.clone(),
                        None => {
                            std::fs::create_dir_all(&audio_dir).map_err(|e| e.to_string())?;
                            let n = files.len() + 1;
                            let mut pcm: Vec<f32> = Vec::with_capacity(orig.len() * 2);
                            for i in 0..orig.len() {
                                pcm.push(orig.left[i]);
                                pcm.push(orig.right[i]);
                            }
                            write_audio(&audio_dir.join(format!("{n:03}.wav")), &pcm, orig.sample_rate as u32, ExportFormat::WavFloat32).map_err(|e| e.to_string())?;
                            let rel = format!("{audio_dir_name}/{n:03}.wav");
                            files.insert(key, rel.clone());
                            rel
                        }
                    };
                    let n = sample.len().max(1) as f64;
                    ClipKindFile::Audio { file: rel, off_frac: *offset as f64 / n, frames_frac: *frames as f64 / n, fx: fx_to_file(fx) }
                }
            };
            clips.push(ClipFile { track: c.track, start: c.start, name: c.name.clone(), kind });
        }
        let patterns: Vec<PatternFile> = self
            .patterns
            .iter()
            .map(|p| PatternFile { id: p.id, name: p.name.clone(), len: p.len, notes: p.notes.iter().map(|n| NoteFile { key: n.key, vel: n.velocity, start: n.start, len: n.len }).collect() })
            .collect();
        let tracks: Vec<TrackFile> = self.tracks.iter().enumerate().map(|(i, t)| TrackFile { index: i, name: t.name.clone(), gain: t.gain, pan: t.pan, mute: t.mute, solo: t.solo, inst: inst_to_string(&t.inst), wave: wave_to_u8(t.wave), plugins: self.plugins_of_track(i, &plugin_dir, &plugin_dir_name) }).collect();
        let pf = ProjectFile { version: 2, bpm: self.bpm, patterns, clips, tracks };
        let json = serde_json::to_string_pretty(&pf).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }

    pub fn load_project(&mut self, path: &Path) -> Result<(), String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let pf: ProjectFile = serde_json::from_str(&text).map_err(|e| format!("not a valid project file: {e}"))?;
        let base = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();

        // Decode all audio first: if something is missing the current project stays untouched.
        let mut audio: HashMap<String, (Arc<SampleData>, Arc<Vec<(f32, f32)>>)> = HashMap::new();
        for c in &pf.clips {
            if let ClipKindFile::Audio { file, .. } = &c.kind {
                if !audio.contains_key(file) {
                    let sd = project_io::load_sample(&base.join(file), self.sample_rate).map_err(|e| format!("{file}: {e}"))?;
                    let sd = Arc::new(sd);
                    let peaks = Arc::new(dense_peaks(&sd));
                    audio.insert(file.clone(), (sd, peaks));
                }
            }
        }

        self.close_editor(true);
        self.auto_ed = None;
        self.bpm = pf.bpm.clamp(40.0, 300.0);
        self.handle.send(Command::SetBpm(self.bpm));
        self.patterns = pf
            .patterns
            .iter()
            .map(|p| Pattern {
                id: p.id,
                name: p.name.clone(),
                len: p.len.max(1),
                notes: p.notes.iter().map(|n| Note { key: n.key, velocity: n.vel, start: n.start, len: n.len }).collect(),
            })
            .collect();
        let max_pat = self.patterns.iter().map(|p| p.id).max().unwrap_or(0);
        self.next_id = self.next_id.max(max_pat + 1);
        self.pat_counter = self.patterns.len() as u32;
        self.selected_pattern = self.patterns.first().map(|p| p.id);

        let pf_tracks = pf.tracks;
        let has_plugin_list = pf.version >= 2;
        self.clips.clear();
        let mut to_render: Vec<u64> = Vec::new();
        let mut new_ids: Vec<Option<u64>> = Vec::new();
        for c in pf.clips {
            let body = match c.kind {
                ClipKindFile::Pattern { pat, len } => {
                    if !self.patterns.iter().any(|p| p.id == pat) {
                        new_ids.push(None);
                        continue;
                    }
                    ClipBody::Pattern { pat, len: len.max(1) }
                }
                ClipKindFile::Automation { target, points, len } => {
                    let Some(target) = target_from_file(&target) else {
                        new_ids.push(None);
                        continue;
                    };
                    let mut pts: Vec<AutoPoint> = points.iter().map(|&(tick, value, shape)| AutoPoint { tick, value: value.clamp(0.0, 1.0), shape: Shape::from_u8(shape) }).collect();
                    pts.sort_by_key(|p| p.tick);
                    if pts.is_empty() {
                        pts.push(AutoPoint { tick: 0, value: 0.5, shape: Shape::Linear });
                    }
                    ClipBody::Automation { auto: AutoData { target, points: pts, len: len.max(1) } }
                }
                ClipKindFile::Audio { file, off_frac, frames_frac, fx } => {
                    let Some((sd, peaks)) = audio.get(&file) else {
                        new_ids.push(None);
                        continue;
                    };
                    let total = sd.len() as u32;
                    let n = total.max(1) as f64;
                    let offset = ((off_frac * n) as u32).min(total.saturating_sub(1));
                    let frames = ((frames_frac * n) as u32).max(1).min((total - offset).max(1));
                    ClipBody::Audio { sample: sd.clone(), orig: sd.clone(), peaks: peaks.clone(), offset, frames, fx: fx_from_file(&fx), serial: 0, pending: false }
                }
            };
            let id = self.alloc_id();
            if matches!(&body, ClipBody::Audio { fx, .. } if !fx.is_neutral()) {
                to_render.push(id);
            }
            self.clips.push(UiClip { id, track: c.track.min(TRACKS - 1), start: c.start, name: c.name, body });
            new_ids.push(Some(id));
        }
        // clip-volume automation points at audio clips by their saved position: use the new ids
        for c in &mut self.clips {
            if let ClipBody::Automation { auto } = &mut c.body {
                if let AutoTarget::ClipGain(i) = auto.target {
                    let nid = new_ids.get(i as usize).copied().flatten().unwrap_or(0);
                    auto.target = AutoTarget::ClipGain(nid);
                }
            }
        }
        for tf in &pf_tracks {
            if tf.index >= self.tracks.len() {
                continue;
            }
            let t = tf.index;
            let (gain, pan) = (tf.gain.clamp(0.0, 1.5), tf.pan.clamp(-1.0, 1.0));
            self.tracks[t].name = tf.name.clone();
            // only send what actually differs, so loading does not flood the engine
            if self.tracks[t].gain != gain {
                self.tracks[t].gain = gain;
                self.handle.send(Command::SetTrackGain { track: t, gain });
            }
            if self.tracks[t].pan != pan {
                self.tracks[t].pan = pan;
                self.handle.send(Command::SetTrackPan { track: t, pan });
            }
            if self.tracks[t].mute != tf.mute {
                self.tracks[t].mute = tf.mute;
                self.handle.send(Command::SetTrackMute { track: t, mute: tf.mute });
            }
            if self.tracks[t].solo != tf.solo {
                self.tracks[t].solo = tf.solo;
                self.handle.send(Command::SetTrackSolo { track: t, solo: tf.solo });
            }
            let wave = wave_from_u8(tf.wave);
            if self.tracks[t].wave != wave {
                self.tracks[t].wave = wave;
                self.handle.send(Command::SetWaveform { track: t, waveform: wave });
            }
            // built-in instruments (VST instruments are left as they are)
            if tf.inst == "none" {
                if self.tracks[t].inst != InstUi::None {
                    self.set_no_instrument(t);
                }
            } else if tf.inst == "basic" {
                if self.tracks[t].inst != InstUi::Basic {
                    self.set_basic_synth(t);
                }
            } else if let Some(name) = tf.inst.strip_prefix("sampler:") {
                if !name.is_empty() && self.tracks[t].inst != InstUi::Sampler(Some(name.to_string())) {
                    let found = self.library.iter().find(|e| e.name == name).map(|e| e.id);
                    match found {
                        Some(id) => self.set_sampler(t, id),
                        None => self.toast(format!("Sampler sample \"{name}\" is not in the library (track {})", t + 1), true),
                    }
                }
            }
            // VST3 plug-ins (projects saved by older versions have no plug-in list: leave those alone)
            for slot in 0..=FX_SLOTS {
                match tf.plugins.iter().find(|p| p.slot == slot) {
                    Some(pfile) => {
                        let idx = self.plugins.iter().position(|p| if pfile.class_id.is_empty() { p.name == pfile.name } else { hex_id(&p.class_id) == pfile.class_id });
                        match idx {
                            Some(i) => {
                                let state: Option<Vec<u8>> = if pfile.state_file.is_empty() { None } else { std::fs::read(base.join(&pfile.state_file)).ok() };
                                let applied = self.load_plugin_with_state(t, slot, i, state.as_deref());
                                if !applied {
                                    // no full state available: restore at least the changed parameters
                                    self.apply_plugin_params(t, slot, &pfile.params);
                                }
                            }
                            None => self.toast(format!("Plug-in \"{}\" was not found (has the VST3 scan finished?)", pfile.name), true),
                        }
                    }
                    None if has_plugin_list => {
                        let loaded = if slot == 0 { matches!(self.tracks[t].inst, InstUi::Vst(_)) } else { self.tracks[t].fx[slot - 1].is_some() };
                        if loaded {
                            self.remove_plugin(t, slot);
                        }
                    }
                    None => {}
                }
            }
        }
        self.sel_clips.clear();
        self.editing_clip = None;
        self.editing_pattern = None;
        self.roll_notes.clear();
        self.roll_sel.clear();
        self.roll_drag = None;
        self.undo.clear();
        self.redo.clear();
        self.last_ckpt = None;
        for id in to_render {
            self.request_fx(id);
        }
        self.push_clips();
        self.project_path = Some(path.to_path_buf());
        Ok(())
    }
}
