//! Shared GUI state and the operations that talk to the audio engine.
use core_engine::{AudioParams, ClipList, Command, EngineHandle, InstrumentKind, Meters, Note, RtClip, RtClipKind, FX_SLOTS, MAX_BLOCK, MAX_TRACKS, PPQ};
use core_engine::{eval_points, owned_value, AutoPoint, AutoTarget, RtAuto, Shape};
use dsp::{SampleData, Waveform};
use eframe::egui::{Color32, Pos2, Rect};
use plugin_host::{PluginController, PluginEditor, PluginInfo};
use project_io::{Config, ExportFormat, FolderWatcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

pub(crate) const TRACKS: usize = MAX_TRACKS;
pub(crate) const BAR_TICKS: u32 = PPQ * 4;
pub(crate) const SNAPS: [(&str, u32); 6] = [("Bar", BAR_TICKS), ("1/2", BAR_TICKS / 2), ("Beat", PPQ), ("1/8", PPQ / 2), ("1/16", PPQ / 4), ("Off", 1)];
pub(crate) const TRACK_COLORS: [Color32; 8] = [
    Color32::from_rgb(99, 140, 255),
    Color32::from_rgb(52, 211, 153),
    Color32::from_rgb(251, 146, 60),
    Color32::from_rgb(192, 132, 252),
    Color32::from_rgb(248, 113, 113),
    Color32::from_rgb(250, 204, 21),
    Color32::from_rgb(34, 211, 238),
    Color32::from_rgb(244, 114, 182),
];

pub(crate) fn track_color(t: usize) -> Color32 {
    TRACK_COLORS[t % TRACK_COLORS.len()]
}

/// Live microphone / line-in stream, fed by the cpal input callback.
pub struct AudioInput {
    pub rx: rtrb::Consumer<[f32; 2]>,
    pub sample_rate: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scale {
    Chromatic,
    Major,
    Minor,
}

impl Scale {
    pub fn contains(self, key: u8, root: u8) -> bool {
        let d = (key + 12 - root) % 12;
        match self {
            Scale::Chromatic => true,
            Scale::Major => [0, 2, 4, 5, 7, 9, 11].contains(&d),
            Scale::Minor => [0, 2, 3, 5, 7, 8, 10].contains(&d),
        }
    }
}

// ---- library ----------------------------------------------------------------

pub(crate) struct Loaded {
    pub sample: Arc<SampleData>,
    pub peaks: Vec<(f32, f32)>,
    pub dense: Arc<Vec<(f32, f32)>>,
}

/// Result of the background analysis of one library file.
pub(crate) struct WaveInfo {
    pub peaks: Vec<(f32, f32)>,
    pub secs: f32,
    pub key: Option<crate::libsearch::Key>,
}

pub(crate) struct LibEntry {
    pub id: u64,
    pub name: String,
    /// Lower-case name, cached for searching.
    pub lname: String,
    pub path: Option<PathBuf>,
    /// Watch folder this file came from (None = imported by hand).
    pub folder: Option<PathBuf>,
    pub data: Option<Loaded>,
    pub failed: bool,
    /// Musical key found in the file name.
    pub key: Option<crate::libsearch::Key>,
    /// Length in seconds (known after decoding, or after the background probe).
    pub duration: Option<f32>,
    /// Small min/max envelope for the list view, produced by the background analysis.
    pub wave: Option<Vec<(f32, f32)>>,
}

impl LibEntry {
    fn new(id: u64, name: String, path: Option<PathBuf>, folder: Option<PathBuf>, data: Option<Loaded>) -> Self {
        let key = crate::libsearch::key_from_name(&name);
        let duration = data.as_ref().map(|d| d.sample.duration_secs());
        Self { id, lname: name.to_lowercase(), name, path, folder, data, failed: false, key, duration, wave: None }
    }
}

// ---- tracks / clips ---------------------------------------------------------------

#[derive(Clone, PartialEq)]
pub(crate) enum InstUi {
    None,
    Basic,
    Sampler(Option<String>),
    Vst(String),
}

impl InstUi {
    pub fn label(&self) -> String {
        match self {
            InstUi::None => "No instrument".into(),
            InstUi::Basic => "Basic Synth".into(),
            InstUi::Sampler(Some(n)) => format!("Sampler: {n}"),
            InstUi::Sampler(None) => "Sampler".into(),
            InstUi::Vst(n) => n.clone(),
        }
    }
}

pub(crate) struct TrackUi {
    pub name: String,
    pub inst: InstUi,
    pub fx: [Option<String>; FX_SLOTS],
    pub gain: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    pub arm: bool,
    pub wave: Waveform,
}

/// Per-clip pitch / tempo settings of an audio clip (the "sample menu" in the playlist).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct AudioFx {
    /// Pitch shift in semitones.
    pub semitones: f32,
    /// false = pitch changes independently of the length (time-stretch); true = tape / varispeed.
    pub tape: bool,
    /// Tempo of the source material. `Some` = the clip is stretched to follow the project tempo.
    pub sample_bpm: Option<f32>,
    /// Draw the waveform inside the clip.
    pub show_wave: bool,
    /// Clip volume in dB and pan (-1 = left, 1 = right); applied while playing.
    pub gain_db: f32,
    pub pan: f32,
    /// Fades at the start / end of the clip in milliseconds; applied while playing.
    pub fade_in_ms: f32,
    pub fade_out_ms: f32,
    pub reverse: bool,
    /// Time-stretch algorithm preset.
    pub quality: dsp::StretchQuality,
}

impl Default for AudioFx {
    fn default() -> Self {
        Self { semitones: 0.0, tape: false, sample_bpm: None, show_wave: true, gain_db: 0.0, pan: 0.0, fade_in_ms: 0.0, fade_out_ms: 0.0, reverse: false, quality: dsp::StretchQuality::Normal }
    }
}

impl AudioFx {
    /// Playback speed relative to the original at project tempo `bpm` (> 1 = faster / shorter).
    pub fn tempo(&self, bpm: f32) -> f64 {
        match self.sample_bpm {
            Some(b) if b > 1.0 => bpm as f64 / b as f64,
            _ => 1.0,
        }
    }

    pub fn is_neutral(&self) -> bool {
        self.semitones.abs() < 1e-3 && self.sample_bpm.is_none()
    }

    /// Playback settings for the engine at sample rate `sr`.
    pub fn params(&self, sr: f32) -> AudioParams {
        let gain = 10f32.powf(self.gain_db / 20.0);
        let pan = self.pan.clamp(-1.0, 1.0);
        let (pl, pr) = (1.0 - pan.max(0.0), 1.0 - (-pan).max(0.0));
        AudioParams {
            gain_l: gain * pl,
            gain_r: gain * pr,
            fade_in: (self.fade_in_ms * sr / 1000.0).max(0.0) as u32,
            fade_out: (self.fade_out_ms * sr / 1000.0).max(0.0) as u32,
            reverse: self.reverse,
        }
    }
}

/// A named group of notes (FL-style pattern). Playlist clips point at a pattern, so editing the
/// pattern changes every clip that uses it.
#[derive(Clone)]
pub(crate) struct Pattern {
    pub id: u64,
    pub name: String,
    pub notes: Vec<Note>,
    pub len: u32,
}

impl UiClip {
    pub fn dup(&self) -> UiClip {
        UiClip { id: self.id, track: self.track, start: self.start, name: self.name.clone(), body: self.body.dup() }
    }
}

pub(crate) enum ClipBody {
    /// `sample` is the processed (pitched / stretched) audio that is played; `orig` the untouched
    /// source. `offset` / `frames` index into `sample`.
    Audio { sample: Arc<SampleData>, orig: Arc<SampleData>, peaks: Arc<Vec<(f32, f32)>>, offset: u32, frames: u32, fx: AudioFx, serial: u64, pending: bool },
    /// Instance of pattern `pat`; `len` is the clip length in ticks (the pattern repeats to fill it).
    Pattern { pat: u64, len: u32 },
    /// Automation clip: drives one parameter (see `core_engine::automation` for the rules).
    Automation { auto: AutoData },
}

/// The data of an automation clip. Point ticks are relative to the clip start.
#[derive(Clone)]
pub(crate) struct AutoData {
    pub target: AutoTarget,
    /// Sorted by tick, at least one point.
    pub points: Vec<AutoPoint>,
    /// Clip length in ticks.
    pub len: u32,
}

/// Part of the Automation Editor window that survives between frames.
pub(crate) struct AutoEditor {
    pub id: u64,
    /// Pixels per tick and the tick at the left edge of the canvas.
    pub ppt: f32,
    pub scroll: f32,
    /// Visible value range (0..1 = everything).
    pub v_lo: f32,
    pub v_hi: f32,
    /// Selected point indices.
    pub sel: Vec<usize>,
    pub drag: Option<AutoDrag>,
    pub snap_h: bool,
    pub snap_v: bool,
    /// Horizontal snap step in ticks, vertical snap = this many steps over 0..1.
    pub grid_ticks: u32,
    pub grid_v: u32,
    /// Fit the whole clip into the window on the next frame.
    pub fit: bool,
    /// Where Ctrl+V pastes (the tick last clicked / hovered).
    pub cursor_tick: u32,
    /// The engine gets point edits at most every 60 ms while a point is being dragged.
    pub last_push: Instant,
    pub pending_push: bool,
}

pub(crate) enum AutoDrag {
    /// Moving the selected points: (index, original tick, original value) of each, and where the grab started.
    Points { orig: Vec<(usize, u32, f32)>, grab_tick: f32, grab_val: f32 },
    /// Rubber band selection in (tick, value) coordinates.
    Band { t0: f32, v0: f32 },
}

impl AutoEditor {
    pub fn new(id: u64) -> Self {
        Self { id, ppt: 0.1, scroll: 0.0, v_lo: 0.0, v_hi: 1.0, sel: Vec::new(), drag: None, snap_h: true, snap_v: false, grid_ticks: PPQ / 4, grid_v: 16, fit: true, cursor_tick: 0, last_push: Instant::now(), pending_push: false }
    }
}

/// Cut a point list at `at` (clip-relative ticks): (points of the first part, points of the second
/// part with ticks relative to the cut). Both parts get a point at the cut so the curve is unchanged.
pub(crate) fn split_points(points: &[AutoPoint], at: u32) -> (Vec<AutoPoint>, Vec<AutoPoint>) {
    let v = eval_points(points, at as f32);
    let carry = points.iter().rev().find(|p| p.tick <= at).map(|p| p.shape).unwrap_or(Shape::Linear);
    let mut head: Vec<AutoPoint> = points.iter().filter(|p| p.tick < at).copied().collect();
    head.push(AutoPoint { tick: at, value: v, shape: Shape::Linear });
    let mut tail = vec![AutoPoint { tick: 0, value: v, shape: carry }];
    tail.extend(points.iter().filter(|p| p.tick > at).map(|p| AutoPoint { tick: p.tick - at, ..*p }));
    (head, tail)
}

impl ClipBody {
    pub fn audio(sample: Arc<SampleData>, peaks: Arc<Vec<(f32, f32)>>, offset: u32, frames: u32) -> Self {
        ClipBody::Audio { orig: sample.clone(), sample, peaks, offset, frames, fx: AudioFx::default(), serial: 0, pending: false }
    }

    pub fn dup(&self) -> ClipBody {
        match self {
            ClipBody::Audio { sample, orig, peaks, offset, frames, fx, serial, pending } => {
                ClipBody::Audio { sample: sample.clone(), orig: orig.clone(), peaks: peaks.clone(), offset: *offset, frames: *frames, fx: *fx, serial: *serial, pending: *pending }
            }
            ClipBody::Pattern { pat, len } => ClipBody::Pattern { pat: *pat, len: *len },
            ClipBody::Automation { auto } => ClipBody::Automation { auto: auto.clone() },
        }
    }
}

/// Re-render request for the background pitch / tempo worker.
pub(crate) struct FxJob {
    pub clip: u64,
    pub serial: u64,
    pub orig: Arc<SampleData>,
    pub semitones: f32,
    pub tape: bool,
    pub tempo: f64,
    pub quality: dsp::StretchQuality,
}

pub(crate) struct FxDone {
    pub clip: u64,
    pub serial: u64,
    pub sample: Arc<SampleData>,
    pub peaks: Arc<Vec<(f32, f32)>>,
}

/// Transport state to restore after the Sample Editor's preview.
pub(crate) struct EditorPreview {
    pub cycle_on: bool,
    pub cycle: (u32, u32),
    pub was_playing: bool,
}

/// The Sample Editor window (opened by double-clicking an audio clip). Edits are applied live so
/// they can be heard; `backup` lets Cancel put the clip back exactly as it was.
pub(crate) struct ClipEditor {
    pub id: u64,
    pub backup: UiClip,
    pub undo_len: usize,
    pub preview: Option<EditorPreview>,
}

/// Rubber-band selection being dragged in the playlist. Positions are kept in song coordinates
/// (ticks / track rows) so the rectangle stays correct while the view scrolls.
pub(crate) struct Marquee {
    pub tick0: f32,
    pub row0: f32,
    /// Clips that were already selected when the drag began (kept when Shift / Ctrl is held).
    pub base: Vec<u64>,
}

/// Everything Undo / Redo restores.
pub(crate) struct Snapshot {
    clips: Vec<UiClip>,
    patterns: Vec<Pattern>,
    selected_pattern: Option<u64>,
}

/// Repeat a pattern's notes to fill a clip of `clip_len` ticks (notes are cut at the clip end).
pub(crate) fn tile_notes(notes: &[Note], pat_len: u32, clip_len: u32) -> Vec<Note> {
    let plen = pat_len.max(1);
    let mut out: Vec<Note> = Vec::new();
    let mut base: u32 = 0;
    while base < clip_len {
        for n in notes {
            if n.start >= plen {
                continue;
            }
            let st = base + n.start;
            if st >= clip_len {
                continue;
            }
            let len = n.len.min(plen - n.start).min(clip_len - st);
            if len > 0 {
                out.push(Note { start: st, len, ..*n });
            }
        }
        base += plen;
    }
    out.sort_by_key(|n| n.start);
    out
}

pub(crate) struct UiClip {
    pub id: u64,
    pub track: usize,
    pub start: u32,
    pub name: String,
    pub body: ClipBody,
}

impl UiClip {
    pub fn len_ticks(&self, bpm: f32, sr: f32) -> u32 {
        match &self.body {
            ClipBody::Audio { frames, .. } => frames_to_ticks(*frames, bpm, sr).max(1),
            ClipBody::Pattern { len, .. } => *len,
            ClipBody::Automation { auto } => auto.len,
        }
    }
}

pub(crate) fn frames_to_ticks(frames: u32, bpm: f32, sr: f32) -> u32 {
    (frames as f64 / sr as f64 * bpm as f64 * PPQ as f64 / 60.0) as u32
}

pub(crate) fn ticks_to_frames(ticks: u32, bpm: f32, sr: f32) -> u32 {
    (ticks as f64 * 60.0 / (bpm as f64 * PPQ as f64) * sr as f64) as u32
}

pub(crate) enum ClipDrag {
    Move { anchor_tick: i64, anchor_row: i32, orig: Vec<(u64, u32, usize)> },
    Resize,
    /// Bottom-left handle of an audio clip: trims its start. (start tick, offset, frames) at drag start.
    TrimStart { start: u32, offset: u32, frames: u32 },
    /// Top handle of an audio clip: time-stretch to the pointer.
    Stretch,
    /// Right edge of a pattern clip: continue the pattern (or repeat it with Ctrl). Holds the
    /// pattern's length at drag start so dragging back and forth is reversible within one drag.
    Extend { orig_plen: u32 },
}

pub(crate) enum RollDrag {
    /// orig: (index, start, key, len)
    Move { anchor_tick: u32, anchor_key: i32, orig: Vec<(usize, u32, u8, u32)> },
    Resize { anchor_tick: u32, orig: Vec<(usize, u32, u8, u32)> },
    Marquee { from: Pos2, to: Pos2 },
}

pub(crate) struct ExportUi {
    pub open: bool,
    pub format: ExportFormat,
    pub whole_project: bool,
    pub path: String,
    pub tail_secs: f32,
    pub running: bool,
    pub progress: Arc<AtomicU32>,
    pub cancel: Arc<AtomicBool>,
    pub result: Arc<Mutex<Option<Result<String, String>>>>,
}

pub(crate) struct State {
    pub handle: EngineHandle,
    pub meters: Arc<Meters>,
    pub sample_rate: f32,
    pub cfg: Config,
    // --- tracks
    pub tracks: Vec<TrackUi>,
    pub selected_track: usize,
    pub master_gain: f32,
    // --- plugins (VST3)
    pub plugins: Vec<PluginInfo>,
    pub scan_rx: Option<mpsc::Receiver<(Vec<PluginInfo>, Vec<(PathBuf, String)>)>>,
    pub plugin_errors: Vec<(PathBuf, String)>,
    // NOTE: `editors` must stay declared before `ctrls` so editor windows are destroyed first.
    pub editors: HashMap<(usize, usize), PluginEditor>,
    pub ctrls: HashMap<(usize, usize), PluginController>,
    pub open_params: Vec<(usize, usize)>,
    pub vst_dir_input: String,
    // --- piano roll (edits the clip `editing_clip`)
    pub editing_clip: Option<u64>,
    pub editing_pattern: Option<u64>,
    pub roll_notes: Vec<Note>,
    pub roll_len: u32,
    /// Clip id -> length (ticks) the clip will have once its background time-stretch render finishes.
    pub stretch_preview: HashMap<u64, u32>,
    pub roll_sel: Vec<usize>,
    pub roll_drag: Option<RollDrag>,
    pub last_len: u32,
    pub snap: u32,
    pub scale: Scale,
    pub root: u8,
    pub zoom: f32,
    pub roll_init: bool,
    pub request_roll_tab: bool,
    pub audition: Option<(usize, u8, Instant)>,
    // --- transport
    pub bpm: f32,
    pub playing: bool,
    pub playhead: u32,
    pub origin: u32,
    pub cycle_on: bool,
    pub cycle: (u32, u32),
    // --- library / drag & drop
    pub library: Vec<LibEntry>,
    pub next_lib_id: u64,
    pub lib_filter: String,
    pub lib_key: String,
    pub lib_len: String,
    /// Library folders the user has opened (all start closed).
    pub lib_open: std::collections::HashSet<PathBuf>,
    pub wave_tx: mpsc::Sender<PathBuf>,
    pub wave_rx: mpsc::Receiver<(PathBuf, Option<WaveInfo>)>,
    pub wave_req: std::collections::HashSet<PathBuf>,
    /// Watch folders whose first scan result has arrived (used by the splash screen).
    pub lib_scanned: std::collections::HashSet<PathBuf>,
    /// Sample being auditioned: (library id, start time, length in seconds).
    pub preview: Option<(u64, Instant, f32)>,
    pub durations: HashMap<PathBuf, f32>,
    pub probing: std::collections::HashSet<PathBuf>,
    pub dur_rx: mpsc::Receiver<(PathBuf, f32)>,
    pub dur_tx: mpsc::Sender<(PathBuf, f32)>,
    pub watcher: FolderWatcher,
    pub folder_input: String,
    pub retained: Vec<Arc<SampleData>>,
    pub drag: Option<u64>,
    pub toast: Option<(String, bool, Instant)>,
    // --- patterns
    pub patterns: Vec<Pattern>,
    pub pat_counter: u32,
    pub selected_pattern: Option<u64>,
    pub pat_drag: Option<u64>,
    pub pat_rename: Option<u64>,
    pub pat_rename_buf: String,
    pub pat_confirm_delete: Option<u64>,
    // --- audio clip pitch / tempo
    pub fx_tx: mpsc::Sender<FxJob>,
    pub fx_rx: mpsc::Receiver<FxDone>,
    pub fx_counter: u64,
    pub bpm_tx: mpsc::Sender<(u64, Option<f32>)>,
    pub bpm_rx: mpsc::Receiver<(u64, Option<f32>)>,
    pub editor: Option<ClipEditor>,
    pub project_path: Option<PathBuf>,
    pub undo: Vec<Snapshot>,
    pub redo: Vec<Snapshot>,
    pub last_ckpt: Option<(&'static str, Instant)>,
    pub clipboard: Vec<UiClip>,
    pub pl_snap_on: bool,
    pub pl_row_h: f32,
    pub drag_hint: Option<(Pos2, String)>,
    pub marquee: Option<Marquee>,
    // --- automation
    /// Copy of the engine's automation list: used to show automated values in the GUI.
    pub auto_cache: Vec<RtAuto>,
    pub auto_dirty: bool,
    pub auto_synced_pos: u32,
    pub auto_ed: Option<AutoEditor>,
    pub auto_clipboard: Vec<AutoPoint>,
    pub auto_shape: Shape,
    pub fx_drag_start: Option<f32>,
    // --- playlist
    pub clips: Vec<UiClip>,
    pub next_id: u64,
    pub sel_clips: Vec<u64>,
    pub pl_ppb: f32,
    pub pl_snap: u32,
    pub pl_scroll_x: f32,
    pub pl_lanes: Vec<(usize, Rect)>,
    pub pl_heads: Vec<(usize, Rect)>,
    pub pl_origin_x: f32,
    pub pl_ppt: f32,
    pub pl_row0_y: f32,
    /// Top of every track row relative to the first row (+ total height), and the automation layers of every track.
    pub pl_row_y: Vec<f32>,
    pub pl_auto_lanes: Vec<Vec<AutoTarget>>,
    pub clip_drag: Option<ClipDrag>,
    pub pl_drag_ptr: Option<Pos2>,
    pub focus_playlist: bool,
    // --- recording
    pub input: Option<AudioInput>,
    pub in_peak: f32,
    pub recording: bool,
    pub rec_l: Vec<f32>,
    pub rec_r: Vec<f32>,
    pub rec_start: u32,
    pub rec_tracks: Vec<usize>,
    pub take_counter: u32,
    // --- export
    pub export: ExportUi,
}

impl State {
    pub fn new(mut handle: EngineHandle, sample_rate: f32, input: Option<AudioInput>) -> Self {
        let cfg = Config::load();
        let (dur_tx, dur_rx) = mpsc::channel();
        // Waveform + key analysis worker: decodes one file at a time, keeps only the small result.
        let (wave_tx, job_rx) = mpsc::channel::<PathBuf>();
        let (res_tx, wave_rx) = mpsc::channel::<(PathBuf, Option<WaveInfo>)>();
        std::thread::spawn(move || {
            while let Ok(p) = job_rx.recv() {
                let info = project_io::load_sample(&p, 44_100.0).ok().map(|sd| WaveInfo { peaks: sd.peaks(480), secs: sd.duration_secs(), key: crate::libsearch::estimate_key(&sd.left, &sd.right, sd.sample_rate) });
                if res_tx.send((p, info)).is_err() {
                    return;
                }
            }
        });
        // Pitch / tempo worker: renders the newest request per clip, older ones are skipped.
        let (fx_tx, fx_job_rx) = mpsc::channel::<FxJob>();
        let (fx_done_tx, fx_rx) = mpsc::channel::<FxDone>();
        std::thread::spawn(move || {
            while let Ok(first) = fx_job_rx.recv() {
                let mut jobs: Vec<FxJob> = vec![first];
                while let Ok(j) = fx_job_rx.try_recv() {
                    jobs.retain(|x| x.clip != j.clip);
                    jobs.push(j);
                }
                for j in jobs {
                    let out = dsp::process_clip(&j.orig, j.semitones, j.tape, j.tempo, j.quality);
                    let peaks = Arc::new(dense_peaks(&out));
                    if fx_done_tx.send(FxDone { clip: j.clip, serial: j.serial, sample: Arc::new(out), peaks }).is_err() {
                        return;
                    }
                }
            }
        });
        let (bpm_tx, bpm_rx) = mpsc::channel::<(u64, Option<f32>)>();
        let watcher = FolderWatcher::new(Duration::from_secs(2));
        watcher.set_folders(cfg.watch_folders.clone());
        let tracks = (0..TRACKS)
            .map(|t| TrackUi { name: format!("Track {}", t + 1), inst: InstUi::None, fx: std::array::from_fn(|_| None), gain: 0.9, pan: 0.0, mute: false, solo: false, arm: false, wave: Waveform::Saw })
            .collect();
        let meters = handle.meters.clone();
        let export_dir = cfg.export_dir.clone().or_else(dirs_music).unwrap_or_else(|| PathBuf::from("."));
        let mut s = Self {
            meters,
            sample_rate,
            tracks,
            selected_track: 0,
            master_gain: 0.9,
            plugins: Vec::new(),
            scan_rx: None,
            plugin_errors: Vec::new(),
            editors: HashMap::new(),
            ctrls: HashMap::new(),
            open_params: Vec::new(),
            vst_dir_input: String::new(),
            editing_clip: None,
            editing_pattern: None,
            roll_notes: Vec::new(),
            roll_len: BAR_TICKS,
            stretch_preview: HashMap::new(),
            roll_sel: Vec::new(),
            roll_drag: None,
            last_len: PPQ / 4,
            snap: PPQ / 4,
            scale: Scale::Chromatic,
            root: 0,
            zoom: 0.22,
            roll_init: false,
            request_roll_tab: false,
            audition: None,
            bpm: 120.0,
            playing: false,
            playhead: 0,
            origin: 0,
            cycle_on: false,
            cycle: (0, BAR_TICKS * 4),
            library: Vec::new(),
            next_lib_id: 1,
            lib_filter: String::new(),
            lib_key: String::new(),
            lib_len: String::new(),
            lib_open: Default::default(),
            wave_tx,
            wave_rx,
            wave_req: Default::default(),
            lib_scanned: Default::default(),
            preview: None,
            durations: HashMap::new(),
            probing: Default::default(),
            dur_rx,
            dur_tx,
            watcher,
            folder_input: String::new(),
            retained: Vec::new(),
            drag: None,
            toast: None,
            patterns: Vec::new(),
            pat_counter: 0,
            selected_pattern: None,
            pat_drag: None,
            pat_rename: None,
            pat_rename_buf: String::new(),
            pat_confirm_delete: None,
            fx_tx,
            fx_rx,
            fx_counter: 0,
            bpm_tx,
            bpm_rx,
            editor: None,
            project_path: None,
            undo: Vec::new(),
            redo: Vec::new(),
            last_ckpt: None,
            clipboard: Vec::new(),
            pl_snap_on: true,
            pl_row_h: 54.0,
            drag_hint: None,
            marquee: None,
            auto_cache: Vec::new(),
            auto_dirty: true,
            auto_synced_pos: u32::MAX,
            auto_ed: None,
            auto_clipboard: Vec::new(),
            auto_shape: Shape::Linear,
            fx_drag_start: None,
            clips: Vec::new(),
            next_id: 1,
            sel_clips: Vec::new(),
            pl_ppb: 36.0,
            pl_snap: PPQ,
            pl_scroll_x: 0.0,
            pl_lanes: Vec::new(),
            pl_heads: Vec::new(),
            pl_origin_x: 0.0,
            pl_ppt: 36.0 / PPQ as f32,
            pl_row0_y: 0.0,
            pl_row_y: Vec::new(),
            pl_auto_lanes: Vec::new(),
            clip_drag: None,
            pl_drag_ptr: None,
            focus_playlist: false,
            input,
            in_peak: 0.0,
            recording: false,
            rec_l: Vec::new(),
            rec_r: Vec::new(),
            rec_start: 0,
            rec_tracks: Vec::new(),
            take_counter: 0,
            export: ExportUi {
                open: false,
                format: ExportFormat::Wav24,
                whole_project: true,
                path: export_dir.join("rust-daw-mix.wav").to_string_lossy().into_owned(),
                tail_secs: 2.0,
                running: false,
                progress: Arc::new(AtomicU32::new(0)),
                cancel: Arc::new(AtomicBool::new(false)),
                result: Arc::new(Mutex::new(None)),
            },
            cfg,
            handle: {
                handle.send(Command::SetBpm(120.0));
                handle
            },
        };
        // Every track starts with the built-in synth so MIDI clips make sound out of the box.
        for t in 0..TRACKS {
            s.tracks[t].inst = InstUi::Basic;
            s.handle.send(Command::SetInstrument { track: t, kind: InstrumentKind::Synth });
        }
        s.rescan_plugins();
        // Like FL Studio: a first empty pattern is always there to paint into the playlist.
        s.add_pattern();
        s.undo.clear();
        s
    }

    pub fn toast(&mut self, msg: impl Into<String>, is_error: bool) {
        self.toast = Some((msg.into(), is_error, Instant::now()));
    }

    /// Snap step of the playlist in ticks (1 = free when snapping is switched off).
    pub fn eff_snap(&self) -> u32 {
        if self.pl_snap_on {
            self.pl_snap.max(1)
        } else {
            1
        }
    }

    pub fn snap_round(&self, ticks: f32, snap: u32) -> u32 {
        let s = snap.max(1) as f32;
        ((ticks / s).round().max(0.0) * s) as u32
    }

    pub fn track_active(&self, t: usize) -> bool {
        let tr = &self.tracks[t];
        tr.arm || t == self.selected_track || tr.inst != InstUi::Basic || tr.fx.iter().any(|f| f.is_some()) || self.clips.iter().any(|c| c.track == t)
    }

    // ---- engine feedback ---------------------------------------------------------

    pub fn poll_engine(&mut self) {
        self.handle.collect_garbage();
        if self.playing {
            self.playhead = self.meters.position();
        }
        // input: always drained (meter), stored only while recording
        let mut peak = self.in_peak * 0.9;
        let cap = (self.input.as_ref().map(|i| i.sample_rate).unwrap_or(48_000.0) * 600.0) as usize;
        if let Some(inp) = self.input.as_mut() {
            while let Ok([l, r]) = inp.rx.pop() {
                peak = peak.max(l.abs()).max(r.abs());
                if self.recording && self.rec_l.len() < cap {
                    self.rec_l.push(l);
                    self.rec_r.push(r);
                }
            }
        }
        self.in_peak = peak;
        if let Some((t, k, at)) = self.audition {
            if at.elapsed() > Duration::from_millis(250) {
                self.handle.send(Command::NoteOff { track: t, key: k });
                self.audition = None;
            }
        }
        // VST3 scan results
        if let Some(rx) = &self.scan_rx {
            if let Ok((list, errors)) = rx.try_recv() {
                self.plugins = list;
                self.plugin_errors = errors;
                self.scan_rx = None;
            }
        }
        // library preview finished?
        if let Some((_, at, len)) = self.preview {
            if at.elapsed().as_secs_f32() > len + 0.1 {
                self.preview = None;
            }
        }
        // background waveform / key analysis
        while let Ok((path, info)) = self.wave_rx.try_recv() {
            if let Some(e) = self.library.iter_mut().find(|e| e.path.as_ref() == Some(&path)) {
                match info {
                    Some(w) => {
                        e.wave = Some(w.peaks);
                        e.duration.get_or_insert(w.secs);
                        if e.key.is_none() {
                            e.key = w.key;
                        }
                    }
                    None => e.failed = true,
                }
            }
        }
        // background length probes
        while let Ok((path, secs)) = self.dur_rx.try_recv() {
            self.probing.remove(&path);
            self.durations.insert(path.clone(), secs);
            if let Some(e) = self.library.iter_mut().find(|e| e.path.as_ref() == Some(&path)) {
                e.duration = Some(secs);
            }
        }
        // pitch / tempo renders and tempo detection finished
        let mut fx_changed = false;
        while let Ok(d) = self.fx_rx.try_recv() {
            if self.apply_fx_done(d) {
                fx_changed = true;
            }
        }
        if fx_changed {
            self.push_clips();
        }
        while let Ok((id, found)) = self.bpm_rx.try_recv() {
            self.apply_detected_bpm(id, found);
        }
        // watch-folder updates
        while let Ok(r) = self.watcher.rx.try_recv() {
            self.apply_scan(r);
        }
        // export result
        if self.export.running {
            let res = self.export.result.lock().unwrap().take();
            if let Some(res) = res {
                self.export.running = false;
                match res {
                    Ok(p) => self.toast(format!("Exported {p}"), false),
                    Err(e) => self.toast(format!("Export failed: {e}"), true),
                }
            }
        }
    }

    pub fn start_audition(&mut self, track: usize, key: u8) {
        self.stop_audition();
        self.handle.send(Command::NoteOn { track, key, velocity: 100 });
        self.audition = Some((track, key, Instant::now()));
    }

    pub fn stop_audition(&mut self) {
        if let Some((track, key, _)) = self.audition.take() {
            self.handle.send(Command::NoteOff { track, key });
        }
    }

    // ---- transport ---------------------------------------------------------------

    pub fn seek(&mut self, t: u32) {
        self.handle.send(Command::Seek(t));
        self.playhead = t;
        if !self.playing {
            self.origin = t;
        }
    }

    pub fn toggle_play(&mut self) {
        if self.playing {
            self.stop_all();
        } else {
            self.origin = self.playhead;
            self.playing = true;
            self.handle.send(Command::Play);
        }
    }

    pub fn stop_all(&mut self) {
        if self.recording {
            self.finish_recording();
        }
        self.playing = false;
        self.handle.send(Command::Stop);
        self.handle.send(Command::Seek(self.origin));
        self.playhead = self.origin;
    }

    pub fn apply_cycle(&mut self) {
        let c = if self.cycle_on && self.cycle.1 > self.cycle.0 { Some(self.cycle) } else { None };
        self.handle.send(Command::SetCycle(c));
    }

    pub fn clips_end_tick(&self) -> u32 {
        self.clips.iter().map(|c| c.start + c.len_ticks(self.bpm, self.sample_rate)).max().unwrap_or(0)
    }

    // ---- recording ---------------------------------------------------------------

    pub fn start_record(&mut self) {
        if self.recording {
            return;
        }
        let armed: Vec<usize> = (0..TRACKS).filter(|&t| self.tracks[t].arm).collect();
        if armed.is_empty() {
            self.toast("Arm a track first (press R on a track header)", true);
            return;
        }
        if self.input.is_none() {
            self.toast("No audio input device found", true);
            return;
        }
        self.rec_tracks = armed;
        self.rec_l.clear();
        self.rec_r.clear();
        self.rec_start = self.playhead;
        if let Some(inp) = self.input.as_mut() {
            while inp.rx.pop().is_ok() {}
        }
        self.recording = true;
        if !self.playing {
            self.origin = self.playhead;
            self.playing = true;
            self.handle.send(Command::Play);
        }
    }

    pub fn finish_recording(&mut self) {
        self.recording = false;
        let Some(rate) = self.input.as_ref().map(|i| i.sample_rate) else { return };
        let skip = ((self.cfg.rec_offset_ms / 1000.0) * rate) as usize;
        if self.rec_l.len() <= skip + 256 {
            self.rec_l.clear();
            self.rec_r.clear();
            self.toast("Recording was too short", true);
            return;
        }
        let left = self.rec_l.split_off(skip);
        let right = self.rec_r.split_off(skip);
        self.rec_l.clear();
        self.rec_r.clear();
        self.take_counter += 1;
        let name = format!("Take {}", self.take_counter);
        let sample = SampleData { name: name.clone(), sample_rate: rate, left, right }.resampled(self.sample_rate);
        let dense = Arc::new(dense_peaks(&sample));
        let sample = Arc::new(sample);
        let frames = sample.len() as u32;
        let start = self.rec_start;
        let tracks = std::mem::take(&mut self.rec_tracks);
        for &t in &tracks {
            let id = self.alloc_id();
            self.clips.push(UiClip { id, track: t, start, name: name.clone(), body: ClipBody::audio(sample.clone(), dense.clone(), 0, frames) });
        }
        self.push_clips();
        self.toast(format!("Recorded {} ({:.1} s) on {} track(s)", name, sample.duration_secs(), tracks.len()), false);
    }

    // ---- sample library ----------------------------------------------------------

    pub fn lib_pos(&self, id: u64) -> Option<usize> {
        self.library.iter().position(|e| e.id == id)
    }

    fn make_loaded(sd: SampleData) -> Loaded {
        let peaks = sd.peaks(160);
        let dense = Arc::new(dense_peaks(&sd));
        Loaded { sample: Arc::new(sd), peaks, dense }
    }

    /// Decode a library entry on demand (watch-folder files are not decoded until used).
    pub fn ensure_loaded(&mut self, id: u64) -> bool {
        let Some(i) = self.lib_pos(id) else { return false };
        if self.library[i].data.is_some() {
            return true;
        }
        if self.library[i].failed {
            return false;
        }
        let Some(path) = self.library[i].path.clone() else { return false };
        match project_io::load_sample(&path, self.sample_rate) {
            Ok(sd) => {
                let l = Self::make_loaded(sd);
                self.retained.push(l.sample.clone());
                self.library[i].duration = Some(l.sample.duration_secs());
                self.library[i].data = Some(l);
                true
            }
            Err(e) => {
                self.library[i].failed = true;
                let n = self.library[i].name.clone();
                self.toast(format!("Couldn't read {n}: {e}"), true);
                false
            }
        }
    }

    pub fn import_path(&mut self, path: &Path) -> Option<u64> {
        match project_io::load_sample(path, self.sample_rate) {
            Ok(sd) => {
                let msg = format!("Imported {} ({:.2} s)", sd.name, sd.duration_secs());
                let name = sd.name.clone();
                let l = Self::make_loaded(sd);
                self.retained.push(l.sample.clone());
                let id = self.next_lib_id;
                self.next_lib_id += 1;
                self.library.push(LibEntry::new(id, name, Some(path.to_path_buf()), None, Some(l)));
                self.lib_open.insert(PathBuf::from("<imported>")); // show hand-imported files right away
                self.toast(msg, false);
                Some(id)
            }
            Err(e) => {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string();
                self.toast(format!("Couldn't import {name}: {e}"), true);
                None
            }
        }
    }

    /// Merge a watch-folder scan result into the library.
    pub fn apply_scan(&mut self, r: project_io::ScanResult) {
        if !self.cfg.watch_folders.contains(&r.folder) {
            return;
        }
        self.lib_scanned.insert(r.folder.clone());
        let mut old: HashMap<PathBuf, LibEntry> = HashMap::new();
        let mut rest = Vec::new();
        for e in self.library.drain(..) {
            if e.folder.as_ref() == Some(&r.folder) {
                if let Some(p) = e.path.clone() {
                    old.insert(p, e);
                }
            } else {
                rest.push(e);
            }
        }
        let before = old.len();
        self.library = rest;
        let mut added = 0;
        let mut to_probe: Vec<PathBuf> = Vec::new();
        for f in r.files {
            if let Some(e) = old.remove(&f) {
                self.library.push(e);
            } else {
                let name = f.file_stem().and_then(|n| n.to_str()).unwrap_or("sample").to_string();
                let id = self.next_lib_id;
                self.next_lib_id += 1;
                let mut e = LibEntry::new(id, name, Some(f.clone()), Some(r.folder.clone()), None);
                e.duration = self.durations.get(&f).copied();
                if e.duration.is_none() && self.probing.insert(f.clone()) {
                    to_probe.push(f);
                }
                self.library.push(e);
                added += 1;
            }
        }
        if before > 0 && added > 0 {
            self.toast(format!("{added} new sample(s) in {}", r.folder.file_name().and_then(|n| n.to_str()).unwrap_or("watch folder")), false);
        }
        // Hand-imported files first, then each watch folder; inside a folder, sub-folders before files.
        self.library.sort_by_cached_key(|e| {
            let comps: Vec<(bool, String)> = match (&e.folder, &e.path) {
                (Some(f), Some(p)) => {
                    let rel: Vec<String> = p.strip_prefix(f).unwrap_or(p).components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect();
                    let last = rel.len().saturating_sub(1);
                    rel.into_iter().enumerate().map(|(i, c)| (i == last, c)).collect()
                }
                _ => vec![(true, e.lname.clone())],
            };
            (e.folder.is_some(), e.folder.clone(), comps)
        });
        if !to_probe.is_empty() {
            let tx = self.dur_tx.clone();
            std::thread::spawn(move || {
                for p in to_probe {
                    if let Some(d) = project_io::probe_duration(&p) {
                        if tx.send((p, d)).is_err() {
                            return;
                        }
                    }
                }
            });
        }
    }

    /// Ask the background worker for a file's waveform and key (once per file).
    pub fn request_wave(&mut self, path: PathBuf) {
        if self.wave_req.insert(path.clone()) {
            let _ = self.wave_tx.send(path);
        }
    }

    /// Click on a library row: play it, or stop it if it is already playing.
    pub fn toggle_preview(&mut self, id: u64) {
        if matches!(self.preview, Some((pid, _, _)) if pid == id) {
            self.handle.send(Command::StopPreview);
            self.preview = None;
            return;
        }
        if !self.ensure_loaded(id) {
            return;
        }
        let Some(i) = self.lib_pos(id) else { return };
        let Some(d) = &self.library[i].data else { return };
        let (sample, secs) = (d.sample.clone(), d.sample.duration_secs());
        self.library[i].duration = Some(secs);
        self.handle.send(Command::Preview(sample));
        self.preview = Some((id, Instant::now(), secs));
    }

    pub fn add_watch_folder(&mut self, dir: PathBuf) {
        if !dir.is_dir() {
            self.toast(format!("Not a folder: {}", dir.display()), true);
            return;
        }
        if !self.cfg.watch_folders.contains(&dir) {
            self.cfg.watch_folders.push(dir);
            self.cfg.save();
            self.watcher.set_folders(self.cfg.watch_folders.clone());
        }
    }

    pub fn remove_watch_folder(&mut self, dir: &Path) {
        self.cfg.watch_folders.retain(|d| d != dir);
        self.library.retain(|e| e.folder.as_deref() != Some(dir));
        self.cfg.save();
        self.watcher.set_folders(self.cfg.watch_folders.clone());
    }

    // ---- instruments / plugins ----------------------------------------------------------

    pub fn set_basic_synth(&mut self, t: usize) {
        self.drop_plugin_ui(t, 0);
        self.handle.send(Command::SetInstrument { track: t, kind: InstrumentKind::Synth });
        self.handle.send(Command::SetWaveform { track: t, waveform: self.tracks[t].wave });
        self.tracks[t].inst = InstUi::Basic;
    }

    pub fn set_no_instrument(&mut self, t: usize) {
        self.drop_plugin_ui(t, 0);
        self.handle.send(Command::SetInstrument { track: t, kind: InstrumentKind::None });
        self.tracks[t].inst = InstUi::None;
    }

    /// Use a library sample as the track's sampler instrument (MIDI notes pitch it, C4 = original).
    pub fn set_sampler(&mut self, t: usize, lib_id: u64) {
        if !self.ensure_loaded(lib_id) {
            return;
        }
        let Some(i) = self.lib_pos(lib_id) else { return };
        let (sample, name) = match &self.library[i].data {
            Some(d) => (d.sample.clone(), self.library[i].name.clone()),
            None => return,
        };
        self.drop_plugin_ui(t, 0);
        self.handle.send(Command::SetInstrument { track: t, kind: InstrumentKind::Sampler(Some(sample)) });
        self.tracks[t].inst = InstUi::Sampler(Some(name.clone()));
        self.toast(format!("{name} is now the instrument of {}", self.tracks[t].name), false);
    }

    fn drop_plugin_ui(&mut self, t: usize, slot: usize) {
        self.editors.remove(&(t, slot)); // close the plugin window before its controller goes
        self.ctrls.remove(&(t, slot));
        self.open_params.retain(|&k| k != (t, slot));
    }

    pub fn rescan_plugins(&mut self) {
        let mut dirs = plugin_host::default_vst3_dirs();
        dirs.extend(self.cfg.vst_dirs.clone());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(plugin_host::scan_with_errors(&dirs));
        });
        self.scan_rx = Some(rx);
    }

    pub fn add_vst_dir(&mut self, dir: PathBuf) {
        if dir.is_dir() && !self.cfg.vst_dirs.contains(&dir) {
            self.cfg.vst_dirs.push(dir);
            self.cfg.save();
            self.rescan_plugins();
        }
    }

    /// Load plugin `idx` (from `self.plugins`) into slot 0 (instrument) or 1..=FX_SLOTS (effect).
    pub fn load_plugin(&mut self, t: usize, slot: usize, idx: usize) {
        self.load_plugin_with_state(t, slot, idx, None);
    }

    /// Like `load_plugin`, but restores a state saved with `PluginController::save_state` (sounds,
    /// presets) before the plug-in starts. Returns true when the plug-in accepted that state.
    pub fn load_plugin_with_state(&mut self, t: usize, slot: usize, idx: usize, state: Option<&[u8]>) -> bool {
        let Some(info) = self.plugins.get(idx).cloned() else { return false };
        let mut applied = false;
        match plugin_host::load_with_state(&info, self.sample_rate as f64, MAX_BLOCK, state) {
            Ok((proc, ctrl)) => {
                self.drop_plugin_ui(t, slot);
                self.handle.send(Command::SetPlugin { track: t, slot, plugin: Some(Box::new(proc)) });
                if let Some(c) = ctrl {
                    applied = c.state_applied;
                    self.ctrls.insert((t, slot), c);
                }
                if slot == 0 {
                    self.tracks[t].inst = InstUi::Vst(info.name.clone());
                } else {
                    self.tracks[t].fx[slot - 1] = Some(info.name.clone());
                }
                self.toast(format!("Loaded {} on {}", info.name, self.tracks[t].name), false);
            }
            Err(e) => self.toast(format!("Couldn't load {}: {e}", info.name), true),
        }
        applied
    }

    /// Open the plugin's own window; plugins without one (or platforms that cannot host it)
    /// get the generic parameter list instead.
    pub fn open_plugin_editor(&mut self, t: usize, slot: usize) {
        if let Some(e) = self.editors.get(&(t, slot)) {
            if !e.is_closed() {
                e.bring_to_front();
                return;
            }
            self.editors.remove(&(t, slot));
        }
        let Some(ctrl) = self.ctrls.get(&(t, slot)) else {
            self.toast("This plugin has no editor or parameters to show", false);
            return;
        };
        let name = if slot == 0 { self.tracks[t].inst.label() } else { self.tracks[t].fx[slot - 1].clone().unwrap_or_default() };
        match ctrl.create_editor(&format!("{} \u{b7} {}", self.tracks[t].name, name)) {
            Ok(e) => {
                self.editors.insert((t, slot), e);
            }
            Err(msg) => {
                self.open_generic_params(t, slot);
                self.toast(format!("{msg}; showing the generic parameter list"), false);
            }
        }
    }

    pub fn open_generic_params(&mut self, t: usize, slot: usize) {
        if self.ctrls.contains_key(&(t, slot)) {
            if !self.open_params.contains(&(t, slot)) {
                self.open_params.push((t, slot));
            }
        } else {
            self.toast("This plugin exposes no editable parameters", false);
        }
    }

    /// Per frame: drop closed editor windows and forward edits made inside them to the engine.
    pub fn service_plugin_editors(&mut self) {
        self.editors.retain(|_, e| !e.is_closed());
        let mut batch = Vec::new();
        for (&(track, slot), c) in &self.ctrls {
            for (id, value) in c.take_edits() {
                batch.push(Command::SetPluginParam { track, slot, id, value });
            }
        }
        for cmd in batch {
            self.handle.send(cmd);
        }
    }

    pub fn remove_plugin(&mut self, t: usize, slot: usize) {
        self.drop_plugin_ui(t, slot);
        self.handle.send(Command::SetPlugin { track: t, slot, plugin: None });
        if slot == 0 {
            self.tracks[t].inst = InstUi::None;
        } else {
            self.tracks[t].fx[slot - 1] = None;
        }
    }

    // ---- playlist ---------------------------------------------------------------------

    pub fn alloc_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Send the complete arrangement to the audio thread.
    pub fn push_clips(&mut self) {
        let mut list = ClipList::default();
        for c in &self.clips {
            let kind = match &c.body {
                ClipBody::Automation { auto } => {
                    list.automation.push(RtAuto::new(c.id, auto.target, c.start, auto.len, auto.points.clone()));
                    continue;
                }
                ClipBody::Audio { sample, offset, frames, fx, .. } => RtClipKind::Audio { sample: sample.clone(), offset: *offset, frames: *frames, params: fx.params(self.sample_rate) },
                ClipBody::Pattern { pat, len } => {
                    let notes = match self.patterns.iter().find(|p| p.id == *pat) {
                        Some(p) => tile_notes(&p.notes, p.len, *len),
                        None => Vec::new(),
                    };
                    RtClipKind::Midi { notes, len: *len }
                }
            };
            list.clips.push(RtClip { id: c.id, track: c.track as u16, start: c.start, kind, gain_mul: 1.0 });
        }
        // the GUI keeps its own copy to show automated values (knobs / sliders follow the automation)
        self.auto_cache = list.automation.iter().map(|a| RtAuto::new(a.id, a.target, a.start, a.len, a.points.clone())).collect();
        self.auto_dirty = true;
        self.handle.send(Command::SetClips(Box::new(list)));
    }

    pub fn clip_index(&self, id: u64) -> Option<usize> {
        self.clips.iter().position(|c| c.id == id)
    }

    pub fn add_audio_clip(&mut self, track: usize, start: u32, lib_id: u64) -> Option<u64> {
        if !self.ensure_loaded(lib_id) {
            return None;
        }
        let i = self.lib_pos(lib_id)?;
        let d = self.library[i].data.as_ref()?;
        let (sample, peaks) = (d.sample.clone(), d.dense.clone());
        let name = self.library[i].name.clone();
        let frames = sample.len() as u32;
        let id = self.alloc_id();
        self.checkpoint();
        self.clips.push(UiClip { id, track, start, name, body: ClipBody::audio(sample, peaks, 0, frames) });
        self.sel_clips = vec![id];
        self.selected_track = track;
        self.push_clips();
        Some(id)
    }

    // ---- undo / redo -----------------------------------------------------------------------

    fn snapshot(&self) -> Snapshot {
        Snapshot { clips: self.clips.iter().map(|c| c.dup()).collect(), patterns: self.patterns.clone(), selected_pattern: self.selected_pattern }
    }

    /// Remember the current arrangement before an edit (Ctrl+Z goes back to it).
    pub fn checkpoint(&mut self) {
        let snap = self.snapshot();
        self.undo.push(snap);
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.last_ckpt = None;
    }

    /// Like `checkpoint`, but repeated calls with the same key within a moment share one step.
    pub fn checkpoint_coalesced(&mut self, key: &'static str) {
        if let Some((k, t)) = self.last_ckpt {
            if k == key && t.elapsed() < Duration::from_millis(900) {
                self.last_ckpt = Some((key, Instant::now()));
                return;
            }
        }
        self.checkpoint();
        self.last_ckpt = Some((key, Instant::now()));
    }

    pub fn undo(&mut self) {
        let Some(prev) = self.undo.pop() else {
            self.toast("Nothing to undo", false);
            return;
        };
        let cur = self.snapshot();
        self.redo.push(cur);
        self.restore(prev);
    }

    pub fn redo(&mut self) {
        let Some(next) = self.redo.pop() else {
            self.toast("Nothing to redo", false);
            return;
        };
        let cur = self.snapshot();
        self.undo.push(cur);
        self.restore(next);
    }

    fn restore(&mut self, snap: Snapshot) {
        self.clips = snap.clips;
        self.patterns = snap.patterns;
        self.selected_pattern = snap.selected_pattern.filter(|&p| self.patterns.iter().any(|x| x.id == p));
        let alive: Vec<u64> = self.clips.iter().map(|c| c.id).collect();
        self.sel_clips.retain(|x| alive.contains(x));
        if self.editing_clip.map_or(false, |c| !alive.contains(&c)) {
            self.editing_clip = None;
        }
        match self.editing_pattern.and_then(|p| self.pat_index(p)) {
            Some(pi) => {
                self.roll_notes = self.patterns[pi].notes.clone();
                self.roll_len = self.patterns[pi].len;
                self.roll_sel.clear();
                self.roll_drag = None;
            }
            None => {
                self.editing_pattern = None;
                self.roll_notes.clear();
                self.roll_sel.clear();
            }
        }
        if self.editor.as_ref().map_or(false, |e| !alive.contains(&e.id)) {
            self.stop_clip_preview();
            self.editor = None;
        }
        let pending: Vec<u64> = self.clips.iter().filter(|c| matches!(&c.body, ClipBody::Audio { pending: true, .. })).map(|c| c.id).collect();
        for id in pending {
            self.request_fx(id);
        }
        self.last_ckpt = None;
        self.push_clips();
    }

    // ---- clipboard -----------------------------------------------------------------------------

    pub fn copy_selected(&mut self) -> usize {
        let sel = self.sel_clips.clone();
        self.clipboard = self.clips.iter().filter(|c| sel.contains(&c.id)).map(|c| c.dup()).collect();
        self.clipboard.len()
    }

    pub fn cut_selected(&mut self) {
        if self.copy_selected() > 0 {
            self.delete_selected_clips();
        }
    }

    /// Paste the copied clips so the earliest one starts at the playhead (same tracks).
    pub fn paste_at_playhead(&mut self) {
        if self.clipboard.is_empty() {
            self.toast("Clipboard is empty", false);
            return;
        }
        let base = self.clipboard.iter().map(|c| c.start).min().unwrap_or(0);
        // The topmost copied clip lands on the selected track, the others keep their distance to it.
        let first_track = self.clipboard.iter().map(|c| c.track).min().unwrap_or(0);
        let target = self.selected_track.min(TRACKS - 1);
        let at = self.playhead;
        let items: Vec<UiClip> = self.clipboard.iter().map(|c| c.dup()).collect();
        self.checkpoint();
        let mut ids = Vec::new();
        for c in items {
            if let ClipBody::Pattern { pat, .. } = &c.body {
                if self.pat_index(*pat).is_none() {
                    continue; // its pattern was deleted
                }
            }
            let id = self.alloc_id();
            ids.push(id);
            self.clips.push(UiClip { id, track: (target + (c.track - first_track)).min(TRACKS - 1), start: at + (c.start - base), name: c.name, body: c.body });
            self.request_fx_if_pending(id);
        }
        if !ids.is_empty() {
            let n = ids.len();
            self.sel_clips = ids;
            self.push_clips();
            self.toast(format!("Pasted {n} clip(s) on track {}", target + 1), false);
        }
    }

    // ---- Sample Editor ---------------------------------------------------------------------------

    pub fn open_editor(&mut self, id: u64) {
        let Some(i) = self.clip_index(id) else { return };
        if !matches!(self.clips[i].body, ClipBody::Audio { .. }) {
            return;
        }
        if self.editor.as_ref().map_or(false, |e| e.id == id) {
            return;
        }
        self.close_editor(true);
        self.checkpoint();
        let backup = self.clips[i].dup();
        self.editor = Some(ClipEditor { id, backup, undo_len: self.undo.len(), preview: None });
        self.sel_clips = vec![id];
    }

    /// `keep = true` applies the edits, `false` puts the clip back as it was when the editor opened.
    pub fn close_editor(&mut self, keep: bool) {
        self.stop_clip_preview();
        let Some(ed) = self.editor.take() else { return };
        if keep {
            return;
        }
        if let Some(i) = self.clip_index(ed.id) {
            self.clips[i] = ed.backup;
            self.request_fx_if_pending(ed.id);
        }
        if self.undo.len() == ed.undo_len {
            self.undo.pop();
        }
        self.push_clips();
    }

    /// Loop the edited clip so the result can be heard (the transport is restored afterwards).
    pub fn start_clip_preview(&mut self) {
        let Some(id) = self.editor.as_ref().map(|e| e.id) else { return };
        let Some(i) = self.clip_index(id) else { return };
        let st = self.clips[i].start;
        let en = st + self.clips[i].len_ticks(self.bpm, self.sample_rate);
        let prev = EditorPreview { cycle_on: self.cycle_on, cycle: self.cycle, was_playing: self.playing };
        if let Some(ed) = &mut self.editor {
            if ed.preview.is_none() {
                ed.preview = Some(prev);
            }
        }
        self.cycle = (st, en);
        self.cycle_on = true;
        self.apply_cycle();
        self.seek(st);
        if !self.playing {
            self.toggle_play();
        }
    }

    pub fn stop_clip_preview(&mut self) {
        let prev = self.editor.as_mut().and_then(|e| e.preview.take());
        if let Some(p) = prev {
            self.cycle_on = p.cycle_on;
            self.cycle = p.cycle;
            self.apply_cycle();
            if !p.was_playing && self.playing {
                self.toggle_play();
            }
        }
    }

    /// Trim an audio clip without changing its speed. `new_off` = new start offset into the sample
    /// (the clip's left edge moves so the remaining audio stays where it is), `new_end_trim` =
    /// frames removed from the end. Both in frames of the (processed) sample.
    pub fn set_clip_trim(&mut self, id: u64, new_off: Option<u32>, new_end_trim: Option<u32>) {
        let (bpm, sr) = (self.bpm, self.sample_rate);
        let Some(i) = self.clip_index(id) else { return };
        let mut shift: i64 = 0;
        if let ClipBody::Audio { sample, offset, frames, .. } = &mut self.clips[i].body {
            let len = sample.len() as u32;
            let mut off = *offset;
            let mut end = (*offset + *frames).min(len);
            if let Some(o) = new_off {
                off = o.min(end.saturating_sub(1));
            }
            if let Some(t) = new_end_trim {
                end = len.saturating_sub(t).max(off + 1).min(len);
            }
            shift = off as i64 - *offset as i64;
            *offset = off;
            *frames = (end - off).max(1);
        }
        if shift != 0 {
            let dt = frames_to_ticks(shift.unsigned_abs() as u32, bpm, sr) as i64 * shift.signum();
            self.clips[i].start = (self.clips[i].start as i64 + dt).max(0) as u32;
        }
        self.push_clips();
    }

    /// Playlist trim handle (bottom-left): move the clip's left edge to `tick`. Pass the clip's
    /// values from the start of the drag so the result does not drift.
    pub fn trim_clip_start_to(&mut self, id: u64, tick: u32, orig_start: u32, orig_off: u32, orig_frames: u32) {
        let (bpm, sr) = (self.bpm, self.sample_rate);
        let Some(i) = self.clip_index(id) else { return };
        let delta = tick as i64 - orig_start as i64;
        let df: i64 = if delta >= 0 { ticks_to_frames(delta as u32, bpm, sr) as i64 } else { -(ticks_to_frames((-delta) as u32, bpm, sr) as i64) };
        let new_off = (orig_off as i64 + df).clamp(0, (orig_off as i64 + orig_frames as i64 - 1).max(0));
        let real_df = new_off - orig_off as i64;
        let new_start: i64 = if real_df == df {
            tick as i64
        } else {
            let t = frames_to_ticks(real_df.unsigned_abs() as u32, bpm, sr) as i64;
            orig_start as i64 + if real_df >= 0 { t } else { -t }
        };
        if let ClipBody::Audio { offset, frames, .. } = &mut self.clips[i].body {
            *offset = new_off as u32;
            *frames = (orig_frames as i64 - real_df).max(1) as u32;
        }
        self.clips[i].start = new_start.max(0) as u32;
    }

    // ---- automation ----------------------------------------------------------------------

    /// Display name of an automation target: "Track 3 \u{b7} Volume", "Synth \u{b7} FX 1 \u{b7} Cutoff", ...
    pub fn target_name(&self, t: AutoTarget) -> String {
        let track_name = |i: u16| self.tracks.get(i as usize).map(|x| x.name.clone()).unwrap_or_else(|| format!("Track {}", i + 1));
        match t {
            AutoTarget::TrackGain(i) => format!("{} \u{b7} Volume", track_name(i)),
            AutoTarget::TrackPan(i) => format!("{} \u{b7} Pan", track_name(i)),
            AutoTarget::TrackPitch(i) => format!("{} \u{b7} Pitch", track_name(i)),
            AutoTarget::MasterGain => "Master \u{b7} Volume".to_string(),
            AutoTarget::ClipGain(id) => match self.clip_index(id) {
                Some(i) => format!("Clip volume \u{b7} {}", self.clips[i].name),
                None => "Clip volume (clip deleted)".to_string(),
            },
            AutoTarget::Plugin { track, slot, id } => {
                let plug = self.tracks.get(track as usize).map(|x| if slot == 0 { x.inst.label() } else { x.fx.get(slot as usize - 1).cloned().flatten().unwrap_or_default() }).unwrap_or_default();
                let param = self.ctrls.get(&(track as usize, slot as usize)).and_then(|c| c.params.iter().find(|p| p.id == id).map(|p| p.name.clone())).unwrap_or_else(|| format!("param {id}"));
                format!("{} \u{b7} {} \u{b7} {}", track_name(track), plug, param)
            }
        }
    }

    /// Where the parameter is right now, normalized 0..1 (the starting value of new automation).
    pub fn target_current_norm(&self, t: AutoTarget) -> f32 {
        match t {
            AutoTarget::TrackGain(i) => self.tracks.get(i as usize).map(|x| t.from_param(x.gain)).unwrap_or(0.6),
            AutoTarget::TrackPan(i) => self.tracks.get(i as usize).map(|x| t.from_param(x.pan)).unwrap_or(0.5),
            AutoTarget::TrackPitch(_) | AutoTarget::ClipGain(_) => 0.5,
            AutoTarget::MasterGain => t.from_param(self.master_gain),
            AutoTarget::Plugin { track, slot, id } => self.ctrls.get(&(track as usize, slot as usize)).map(|c| c.get(id) as f32).unwrap_or(0.5),
        }
    }

    /// New automation clip for `target` on the selected track at the playhead (4 bars, flat at the
    /// parameter's current value, so nothing changes until you edit the points). Opens the editor.
    pub fn create_automation(&mut self, target: AutoTarget) -> Option<u64> {
        let mut len = BAR_TICKS * 4;
        let v = self.target_current_norm(target);
        let name = self.target_name(target);
        let (mut track, mut start) = (self.selected_track, self.playhead);
        // clip volume: start with the clip, in the automation lane of the clip's own track, as long as the clip
        if let AutoTarget::ClipGain(cid) = target {
            if let Some(i) = self.clip_index(cid) {
                start = self.clips[i].start;
                track = self.clips[i].track;
                len = ((self.clips[i].len_ticks(self.bpm, self.sample_rate) + PPQ - 1) / PPQ).max(1) * PPQ;
            }
        }
        self.checkpoint();
        let id = self.alloc_id();
        let points = vec![AutoPoint { tick: 0, value: v, shape: Shape::Linear }, AutoPoint { tick: len, value: v, shape: Shape::Linear }];
        self.clips.push(UiClip { id, track, start, name, body: ClipBody::Automation { auto: AutoData { target, points, len } } });
        self.sel_clips = vec![id];
        self.push_clips();
        self.open_auto_editor(id);
        Some(id)
    }

    pub fn open_auto_editor(&mut self, id: u64) {
        let Some(i) = self.clip_index(id) else { return };
        if !matches!(self.clips[i].body, ClipBody::Automation { .. }) {
            return;
        }
        self.sel_clips = vec![id];
        self.auto_ed = Some(AutoEditor::new(id));
    }

    /// Automation clips that control `target` (to offer "edit" next to "create").
    pub fn automation_clips_of(&self, target: AutoTarget) -> Vec<u64> {
        self.clips.iter().filter(|c| matches!(&c.body, ClipBody::Automation { auto } if auto.target == target)).map(|c| c.id).collect()
    }

    /// Make the GUI controls show what the automation does at the playhead (the audio thread
    /// already applies it by itself). Cheap: only runs when the arrangement or the position changed.
    pub fn sync_automation_display(&mut self) {
        if !(self.playing || self.auto_dirty || self.playhead != self.auto_synced_pos) {
            return;
        }
        self.auto_dirty = false;
        self.auto_synced_pos = self.playhead;
        let pos = self.playhead as f64;
        for i in 0..self.auto_cache.len() {
            let Some(v) = owned_value(&self.auto_cache, i, pos) else { continue };
            if self.auto_cache[i].applied == v {
                continue;
            }
            self.auto_cache[i].applied = v;
            let target = self.auto_cache[i].target;
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
                AutoTarget::MasterGain => self.master_gain = p,
                AutoTarget::Plugin { track, slot, id } => {
                    if let Some(c) = self.ctrls.get(&(track as usize, slot as usize)) {
                        c.set(id, v as f64);
                    }
                }
                AutoTarget::TrackPitch(_) | AutoTarget::ClipGain(_) => {}
            }
        }
    }

    // ---- patterns -----------------------------------------------------------------------

    pub fn pat_index(&self, id: u64) -> Option<usize> {
        self.patterns.iter().position(|p| p.id == id)
    }

    /// Create an empty one-bar pattern ("Pattern N") and select it.
    pub fn add_pattern(&mut self) -> u64 {
        self.checkpoint();
        self.pat_counter += 1;
        let id = self.alloc_id();
        self.patterns.push(Pattern { id, name: format!("Pattern {}", self.pat_counter), notes: Vec::new(), len: BAR_TICKS });
        self.selected_pattern = Some(id);
        id
    }

    /// The pattern new clips are made from (a new one when none exists).
    pub fn current_pattern(&mut self) -> u64 {
        match self.selected_pattern.filter(|&p| self.pat_index(p).is_some()) {
            Some(p) => p,
            None => match self.patterns.first().map(|p| p.id) {
                Some(p) => {
                    self.selected_pattern = Some(p);
                    p
                }
                None => self.add_pattern(),
            },
        }
    }

    pub fn clone_pattern(&mut self, id: u64) -> Option<u64> {
        let i = self.pat_index(id)?;
        let (notes, len, base) = (self.patterns[i].notes.clone(), self.patterns[i].len, self.patterns[i].name.clone());
        self.checkpoint();
        let nid = self.alloc_id();
        self.patterns.insert(i + 1, Pattern { id: nid, name: format!("{base} (copy)"), notes, len });
        self.selected_pattern = Some(nid);
        Some(nid)
    }

    pub fn rename_pattern(&mut self, id: u64, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let Some(i) = self.pat_index(id) else { return };
        self.checkpoint();
        self.patterns[i].name = name.to_string();
        for c in &mut self.clips {
            if matches!(&c.body, ClipBody::Pattern { pat, .. } if *pat == id) {
                c.name = name.to_string();
            }
        }
    }

    pub fn pattern_clip_count(&self, id: u64) -> usize {
        self.clips.iter().filter(|c| matches!(&c.body, ClipBody::Pattern { pat, .. } if *pat == id)).count()
    }

    /// Delete a pattern together with every playlist clip that uses it.
    pub fn delete_pattern(&mut self, id: u64) {
        let Some(i) = self.pat_index(id) else { return };
        self.checkpoint();
        let name = self.patterns[i].name.clone();
        let used = self.pattern_clip_count(id);
        self.clips.retain(|c| !matches!(&c.body, ClipBody::Pattern { pat, .. } if *pat == id));
        let alive: Vec<u64> = self.clips.iter().map(|c| c.id).collect();
        self.sel_clips.retain(|x| alive.contains(x));
        self.patterns.remove(i);
        if self.editing_pattern == Some(id) {
            self.editing_pattern = None;
            self.editing_clip = None;
            self.roll_notes.clear();
            self.roll_sel.clear();
            self.roll_drag = None;
        }
        if self.editing_clip.map_or(false, |c| !alive.contains(&c)) {
            self.editing_clip = None;
        }
        if self.selected_pattern == Some(id) {
            self.selected_pattern = self.patterns.first().map(|p| p.id);
        }
        if self.pat_rename == Some(id) {
            self.pat_rename = None;
        }
        self.pat_confirm_delete = None;
        self.push_clips();
        if used > 0 {
            self.toast(format!("Deleted {name} and its {used} clip(s)"), false);
        } else {
            self.toast(format!("Deleted {name}"), false);
        }
    }

    /// Put an instance of pattern `pat` into the playlist.
    /// Developer aid (`RUSTDAW_DEMO=1`): fills the playlist so the editing UI can be inspected quickly.
    pub fn demo_scenario(&mut self) {
        let Some(lib) = self.library.last().map(|e| e.id) else { return };
        // track 1: a whole clip; track 2: the same sample split in two; track 3: a pattern with notes
        let whole = self.add_audio_clip(0, BAR_TICKS, lib);
        let _ = whole;
        self.add_audio_clip(1, BAR_TICKS, lib);
        self.playhead = BAR_TICKS + 2 * PPQ;
        self.sel_clips.clear();
        self.selected_track = 1;
        if let Some(id) = self.clips.iter().find(|c| c.track == 1).map(|c| c.id) {
            self.sel_clips = vec![id];
        }
        self.split_at_playhead();
        let pat = self.current_pattern();
        if let Some(pi) = self.pat_index(pat) {
            for (i, k) in [60u8, 64, 67, 72].iter().enumerate() {
                self.patterns[pi].notes.push(Note { key: *k, velocity: 100, start: i as u32 * PPQ, len: PPQ / 2 });
            }
        }
        self.add_pattern_clip(2, BAR_TICKS, pat);
        self.sel_clips.clear();
        self.playhead = 0;
    }

    pub fn add_pattern_clip(&mut self, track: usize, start: u32, pat: u64) -> Option<u64> {
        let pi = self.pat_index(pat)?;
        let (name, len) = (self.patterns[pi].name.clone(), self.patterns[pi].len);
        self.checkpoint();
        let id = self.alloc_id();
        self.clips.push(UiClip { id, track, start, name, body: ClipBody::Pattern { pat, len } });
        self.sel_clips = vec![id];
        self.selected_track = track;
        self.selected_pattern = Some(pat);
        self.push_clips();
        Some(id)
    }

    /// Give a pattern clip its own copy of the pattern (FL's "make unique").
    pub fn make_clip_pattern_unique(&mut self, id: u64) {
        let Some(i) = self.clip_index(id) else { return };
        let pat = match &self.clips[i].body {
            ClipBody::Pattern { pat, .. } => *pat,
            _ => return,
        };
        let Some(newp) = self.clone_pattern(pat) else { return };
        let name = self.pat_index(newp).map(|pi| self.patterns[pi].name.clone()).unwrap_or_default();
        if let ClipBody::Pattern { pat, .. } = &mut self.clips[i].body {
            *pat = newp;
        }
        self.clips[i].name = name.clone();
        self.push_clips();
        self.toast(format!("This clip now has its own pattern: {name}"), false);
    }

    /// Pattern dragged from the Patterns window and dropped at `p`: onto a lane = new clip.
    pub fn drop_pattern_at(&mut self, pat: u64, p: Pos2) -> bool {
        if let Some((track, tick)) = self.lane_at(p) {
            self.add_pattern_clip(track, tick, pat);
            return true;
        }
        false
    }

    // ---- audio clip pitch / tempo ----------------------------------------------------------

    /// Queue a re-render of an audio clip after its pitch / tempo settings changed.
    pub fn request_fx(&mut self, id: u64) {
        let bpm = self.bpm;
        let Some(i) = self.clip_index(id) else { return };
        let ClipBody::Audio { orig, fx, serial, pending, .. } = &mut self.clips[i].body else { return };
        self.fx_counter += 1;
        *serial = self.fx_counter;
        *pending = true;
        let job = FxJob { clip: id, serial: self.fx_counter, orig: orig.clone(), semitones: fx.semitones, tape: fx.tape, tempo: fx.tempo(bpm), quality: fx.quality };
        let _ = self.fx_tx.send(job);
    }

    /// Change an audio clip's settings; re-renders its sound when pitch / tempo changed.
    pub fn set_clip_fx(&mut self, id: u64, f: impl FnOnce(&mut AudioFx)) {
        let Some(i) = self.clip_index(id) else { return };
        let mut rerender = false;
        let mut changed = false;
        let mut old_fx: Option<AudioFx> = None;
        if let ClipBody::Audio { fx, .. } = &mut self.clips[i].body {
            let before = *fx;
            f(&mut *fx);
            fx.semitones = fx.semitones.clamp(-24.0, 24.0);
            if let Some(b) = fx.sample_bpm {
                fx.sample_bpm = Some(b.clamp(40.0, 300.0));
            }
            fx.gain_db = fx.gain_db.clamp(-60.0, 12.0);
            fx.pan = fx.pan.clamp(-1.0, 1.0);
            fx.fade_in_ms = fx.fade_in_ms.clamp(0.0, 20_000.0);
            fx.fade_out_ms = fx.fade_out_ms.clamp(0.0, 20_000.0);
            rerender = before.semitones != fx.semitones || before.tape != fx.tape || before.sample_bpm != fx.sample_bpm || before.quality != fx.quality;
            changed = before != *fx;
            if changed {
                old_fx = Some(before);
            }
        }
        if changed {
            if self.editor.is_none() {
                // The Sample Editor takes one undo step when it opens; other edits coalesce.
                let after = self.clip_fx(id).map(|x| x.0);
                if let (Some(old), Some(new)) = (old_fx, after) {
                    // checkpoint must hold the state *before* this change
                    self.checkpoint_with_fx(id, old);
                    let _ = new;
                }
            }
            if rerender {
                self.request_fx(id);
            }
            self.push_clips();
        }
    }

    /// Undo checkpoint taken just after an fx change: the snapshot gets the clip's previous fx back.
    fn checkpoint_with_fx(&mut self, id: u64, old: AudioFx) {
        if let Some((k, t)) = self.last_ckpt {
            if k == "fx" && t.elapsed() < Duration::from_millis(900) {
                self.last_ckpt = Some(("fx", Instant::now()));
                return;
            }
        }
        let mut snap = self.snapshot();
        for c in &mut snap.clips {
            if c.id == id {
                if let ClipBody::Audio { fx, .. } = &mut c.body {
                    *fx = old;
                }
            }
        }
        self.undo.push(snap);
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.last_ckpt = Some(("fx", Instant::now()));
    }

    pub fn clip_fx(&self, id: u64) -> Option<(AudioFx, bool)> {
        let i = self.clip_index(id)?;
        match &self.clips[i].body {
            ClipBody::Audio { fx, pending, .. } => Some((*fx, *pending)),
            _ => None,
        }
    }

    fn apply_fx_done(&mut self, d: FxDone) -> bool {
        let Some(i) = self.clip_index(d.clip) else { return false };
        let ClipBody::Audio { sample, peaks, offset, frames, serial, pending, .. } = &mut self.clips[i].body else { return false };
        if *serial != d.serial {
            return false; // a newer request is already on its way
        }
        let old_len = sample.len().max(1) as f64;
        let new_len = d.sample.len() as u32;
        let ratio = d.sample.len() as f64 / old_len;
        let off = ((*offset as f64 * ratio).round() as u32).min(new_len.saturating_sub(1));
        let fr = ((*frames as f64 * ratio).round() as u32).max(1).min(new_len.saturating_sub(off).max(1));
        *offset = off;
        *frames = fr;
        *sample = d.sample;
        *peaks = d.peaks;
        *pending = false;
        true
    }

    /// The project tempo changed: clips that follow it are re-stretched.
    pub fn on_bpm_changed(&mut self) {
        let ids: Vec<u64> = self.clips.iter().filter(|c| matches!(&c.body, ClipBody::Audio { fx, .. } if fx.sample_bpm.is_some())).map(|c| c.id).collect();
        for id in ids {
            self.request_fx(id);
        }
    }

    /// "Detect BPM": a tempo in the file name wins, otherwise the audio is analysed in the background.
    pub fn detect_clip_bpm(&mut self, id: u64) {
        let Some(i) = self.clip_index(id) else { return };
        let name = self.clips[i].name.clone();
        let orig = match &self.clips[i].body {
            ClipBody::Audio { orig, .. } => orig.clone(),
            _ => return,
        };
        if let Some(b) = crate::libsearch::bpm_from_name(&name) {
            self.apply_detected_bpm(id, Some(b));
            return;
        }
        self.toast("Detecting tempo\u{2026}", false);
        let tx = self.bpm_tx.clone();
        std::thread::spawn(move || {
            let found = dsp::detect_bpm(&orig.left, &orig.right, orig.sample_rate);
            let _ = tx.send((id, found));
        });
    }

    fn apply_detected_bpm(&mut self, id: u64, found: Option<f32>) {
        match found {
            Some(b) => {
                self.set_clip_fx(id, |fx| fx.sample_bpm = Some(b));
                self.toast(format!("Detected {b:.1} BPM: the clip now follows the project tempo (use \u{d7}2 / \u{f7}2 if it is an octave off)"), false);
            }
            None => self.toast("Couldn't find a steady tempo in this sample", true),
        }
    }

    pub fn lane_at(&self, p: Pos2) -> Option<(usize, u32)> {
        let (track, _) = self.pl_lanes.iter().find(|(_, r)| r.contains(p))?;
        let tick = self.snap_round((p.x - self.pl_origin_x) / self.pl_ppt, self.eff_snap());
        Some((*track, tick))
    }

    pub fn head_at(&self, p: Pos2) -> Option<usize> {
        self.pl_heads.iter().find(|(_, r)| r.contains(p)).map(|(t, _)| *t)
    }

    /// Library sample dropped at `p`: onto a lane = audio clip; onto a header = sampler instrument.
    pub fn drop_sample_at(&mut self, lib_id: u64, p: Pos2) -> bool {
        if let Some((track, tick)) = self.lane_at(p) {
            self.add_audio_clip(track, tick, lib_id);
            return true;
        }
        if let Some(t) = self.head_at(p) {
            self.selected_track = t;
            self.set_sampler(t, lib_id);
            return true;
        }
        false
    }

    pub fn delete_selected_clips(&mut self) {
        if self.sel_clips.is_empty() {
            return;
        }
        self.checkpoint();
        let sel = std::mem::take(&mut self.sel_clips);
        self.clips.retain(|c| !sel.contains(&c.id));
        if self.editing_clip.map_or(false, |e| sel.contains(&e)) {
            self.editing_clip = None;
        }
        self.push_clips();
    }

    pub fn delete_clip(&mut self, id: u64) {
        self.checkpoint();
        self.clips.retain(|c| c.id != id);
        self.sel_clips.retain(|&x| x != id);
        if self.editing_clip == Some(id) {
            self.editing_clip = None;
        }
        self.push_clips();
    }

    pub fn duplicate_clip(&mut self, id: u64) {
        let Some(i) = self.clip_index(id) else { return };
        let (bpm, sr) = (self.bpm, self.sample_rate);
        let c = &self.clips[i];
        let len = c.len_ticks(bpm, sr);
        let body = c.body.dup();
        let (track, start, name) = (c.track, c.start + len, c.name.clone());
        let nid = self.alloc_id();
        self.checkpoint_coalesced("dup");
        self.clips.push(UiClip { id: nid, track, start, name, body });
        self.sel_clips = vec![nid];
        self.request_fx_if_pending(nid);
        self.push_clips();
    }

    /// A copy made while its source is still being rendered needs its own render request.
    fn request_fx_if_pending(&mut self, id: u64) {
        if matches!(self.clip_fx(id), Some((_, true))) {
            self.request_fx(id);
        }
    }

    /// Copies of the given clips at exactly the same place (Shift + drag: the copies are then dragged
    /// away). Returns the ids of the copies; the caller decides about the undo checkpoint.
    pub fn clone_clips_in_place(&mut self, ids: &[u64]) -> Vec<u64> {
        let mut out = Vec::new();
        for &id in ids {
            let Some(i) = self.clip_index(id) else { continue };
            let (track, start, name, body) = {
                let c = &self.clips[i];
                (c.track, c.start, c.name.clone(), c.body.dup())
            };
            let nid = self.alloc_id();
            self.clips.push(UiClip { id: nid, track, start, name, body });
            self.request_fx_if_pending(nid);
            out.push(nid);
        }
        out
    }

    pub fn duplicate_selected(&mut self) {
        for id in self.sel_clips.clone() {
            self.duplicate_clip(id);
        }
    }

    /// Split the clip at the playhead (selected clips first, otherwise any clip on the selected track).
    pub fn split_at_playhead(&mut self) {
        let t = self.playhead;
        let (bpm, sr) = (self.bpm, self.sample_rate);
        let targets: Vec<u64> = if self.sel_clips.is_empty() {
            self.clips.iter().filter(|c| c.track == self.selected_track).map(|c| c.id).collect()
        } else {
            self.sel_clips.clone()
        };
        let mut did = false;
        let mut rerender: Vec<u64> = Vec::new();
        for id in targets {
            let Some(i) = self.clip_index(id) else { continue };
            let (cstart, ctrack, cname, len) = {
                let c = &self.clips[i];
                (c.start, c.track, c.name.clone(), c.len_ticks(bpm, sr))
            };
            if t <= cstart || t >= cstart + len {
                continue;
            }
            let first = t - cstart;
            let nid = self.alloc_id();
            let second = match &mut self.clips[i].body {
                ClipBody::Audio { sample, orig, peaks, offset, frames, fx, serial, pending } => {
                    let f1 = ticks_to_frames(first, bpm, sr).clamp(1, frames.saturating_sub(1).max(1));
                    let b = ClipBody::Audio { sample: sample.clone(), orig: orig.clone(), peaks: peaks.clone(), offset: *offset + f1, frames: *frames - f1, fx: *fx, serial: *serial, pending: *pending };
                    if *pending {
                        rerender.push(nid);
                    }
                    *frames = f1;
                    b
                }
                ClipBody::Automation { auto } => {
                    let (head, tail) = split_points(&auto.points, first);
                    let rest = auto.len - first;
                    auto.points = head;
                    auto.len = first;
                    ClipBody::Automation { auto: AutoData { target: auto.target, points: tail, len: rest } }
                }
                ClipBody::Pattern { pat, len } => {
                    // Both halves play the same pattern; the second half restarts it from the beginning.
                    let rest = *len - first;
                    let pat = *pat;
                    *len = first;
                    ClipBody::Pattern { pat, len: rest }
                }
            };
            self.checkpoint_coalesced("split");
            self.clips.push(UiClip { id: nid, track: ctrack, start: t, name: cname, body: second });
            did = true;
        }
        for id in rerender {
            self.request_fx(id);
        }
        if did {
            self.push_clips();
        } else {
            self.toast("Place the playhead inside a clip to split it", false);
        }
    }

    // ---- piano roll ---------------------------------------------------------------------

    /// Open a pattern clip's pattern in the piano roll.
    pub fn open_clip(&mut self, id: u64) {
        let Some(i) = self.clip_index(id) else { return };
        let pat = match &self.clips[i].body {
            ClipBody::Pattern { pat, .. } => *pat,
            ClipBody::Automation { .. } => {
                self.open_auto_editor(id);
                return;
            }
            _ => return,
        };
        self.open_pattern(pat, Some(id));
    }

    /// Open pattern `pat` in the piano roll. `clip` = the playlist clip it was opened from (it
    /// decides which track plays the notes and where the playhead is drawn).
    pub fn open_pattern(&mut self, pat: u64, clip: Option<u64>) {
        let Some(pi) = self.pat_index(pat) else { return };
        self.roll_notes = self.patterns[pi].notes.clone();
        self.roll_len = self.patterns[pi].len;
        self.editing_pattern = Some(pat);
        self.editing_clip = clip;
        self.selected_pattern = Some(pat);
        self.roll_sel.clear();
        self.roll_drag = None;
        self.roll_init = false;
        self.request_roll_tab = true;
        if let Some(ci) = clip.and_then(|c| self.clip_index(c)) {
            self.selected_track = self.clips[ci].track;
        }
    }

    /// Write the roll's working copy back into the pattern and send the arrangement to the engine.
    pub fn sync_roll(&mut self) {
        let Some(pid) = self.editing_pattern else { return };
        let Some(pi) = self.pat_index(pid) else { return };
        self.checkpoint_coalesced("roll");
        let old_len = self.patterns[pi].len;
        self.patterns[pi].notes = self.roll_notes.clone();
        self.patterns[pi].len = self.roll_len;
        if old_len != self.roll_len {
            let new_len = self.roll_len;
            for c in &mut self.clips {
                if let ClipBody::Pattern { pat, len } = &mut c.body {
                    if *pat == pid && *len == old_len {
                        *len = new_len;
                    }
                }
            }
        }
        self.push_clips();
    }

    pub fn remove_note(&mut self, i: usize) {
        if i < self.roll_notes.len() {
            self.roll_notes.remove(i);
            self.roll_sel.retain(|&j| j != i);
            for j in &mut self.roll_sel {
                if *j > i {
                    *j -= 1;
                }
            }
        }
    }

    pub fn delete_selected_notes(&mut self) {
        if self.roll_sel.is_empty() {
            return;
        }
        let mut sel = std::mem::take(&mut self.roll_sel);
        sel.sort_unstable();
        for &i in sel.iter().rev() {
            self.roll_notes.remove(i);
        }
        self.sync_roll();
    }

    pub fn delete_selection(&mut self) {
        if self.focus_playlist {
            self.delete_selected_clips();
        } else {
            self.delete_selected_notes();
        }
    }

    pub fn select_all(&mut self) {
        if self.focus_playlist {
            self.sel_clips = self.clips.iter().map(|c| c.id).collect();
        } else {
            self.roll_sel = (0..self.roll_notes.len()).collect();
        }
    }

    // ---- export --------------------------------------------------------------------------

    pub fn start_export(&mut self) {
        if self.export.running {
            return;
        }
        let (start, end) = if self.export.whole_project || !self.cycle_on {
            (0, self.clips_end_tick())
        } else {
            self.cycle
        };
        if end <= start {
            self.toast("Nothing to export: add some clips first", true);
            return;
        }
        let Some(shared) = self.handle.shared.clone() else {
            self.toast("Export is not available in this build", true);
            return;
        };
        let path = PathBuf::from(self.export.path.trim());
        if path.as_os_str().is_empty() {
            self.toast("Choose an output file", true);
            return;
        }
        let path = if path.extension().is_none() { path.with_extension(self.export.format.extension()) } else { path };
        if let Some(dir) = path.parent() {
            self.cfg.export_dir = Some(dir.to_path_buf());
            self.cfg.save();
        }
        self.export.running = true;
        self.export.progress.store(0, Ordering::Relaxed);
        self.export.cancel.store(false, Ordering::Relaxed);
        *self.export.result.lock().unwrap() = None;
        let (progress, cancel, result) = (self.export.progress.clone(), self.export.cancel.clone(), self.export.result.clone());
        let (format, tail, sr) = (self.export.format, self.export.tail_secs, self.sample_rate as u32);
        std::thread::spawn(move || {
            // The audio callback uses try_lock, so it outputs silence while we render.
            let pcm = {
                let mut e = shared.lock().unwrap();
                e.export_render(start, end, tail, &progress, &cancel)
            };
            let res = if cancel.load(Ordering::Relaxed) {
                Err("cancelled".to_string())
            } else {
                project_io::write_audio(&path, &pcm, sr, format).map(|_| path.display().to_string()).map_err(|e| e.to_string())
            };
            *result.lock().unwrap() = Some(res);
        });
    }
}

fn dirs_music() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(|h| PathBuf::from(h).join("Music"))
}

/// Peak envelope with a resolution that scales with the clip length (for the playlist).
pub(crate) fn dense_peaks(s: &SampleData) -> Vec<(f32, f32)> {
    s.peaks((s.len() / 512).clamp(64, 4000))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with_input() -> (State, rtrb::Producer<[f32; 2]>) {
        let (engine, mut handle) = core_engine::create_engine(48_000.0);
        core_engine::share_engine(engine, &mut handle);
        let (tx, rx) = rtrb::RingBuffer::<[f32; 2]>::new(48_000);
        (State::new(handle, 48_000.0, Some(AudioInput { rx, sample_rate: 48_000.0 })), tx)
    }

    #[test]
    fn recording_creates_a_clip_on_every_armed_track() {
        let (mut s, mut tx) = state_with_input();
        s.tracks[5].arm = true;
        s.tracks[7].arm = true;
        s.seek(3840);
        s.start_record();
        assert!(s.recording && s.playing);
        for i in 0..24_000 {
            tx.push([(i as f32 * 0.05).sin() * 0.5; 2]).unwrap();
        }
        s.poll_engine();
        s.stop_all();
        assert!(!s.recording && !s.playing);
        assert_eq!(s.playhead, 3840, "stop returns to the start position");
        let mut tracks: Vec<usize> = s.clips.iter().map(|c| c.track).collect();
        tracks.sort();
        assert_eq!(tracks, vec![5, 7]);
        for c in &s.clips {
            assert_eq!(c.start, 3840);
            assert!(matches!(&c.body, ClipBody::Audio { frames, .. } if *frames == 24_000));
        }
    }

    #[test]
    fn record_without_armed_track_is_refused() {
        let (mut s, _tx) = state_with_input();
        s.start_record();
        assert!(!s.recording && s.toast.is_some());
    }

    #[test]
    fn pattern_clips_share_one_pattern_and_split_by_length() {
        let (mut s, _tx) = state_with_input();
        let pid = s.current_pattern();
        let a = s.add_pattern_clip(0, 0, pid).unwrap();
        let b = s.add_pattern_clip(1, 3840, pid).unwrap();
        s.open_clip(a);
        s.roll_notes = vec![Note { key: 60, velocity: 100, start: 0, len: 480 }, Note { key: 62, velocity: 100, start: 2400, len: 480 }];
        s.sync_roll();
        assert_eq!(s.patterns.len(), 1);
        // lengthening the pattern lengthens the clips that followed it
        s.roll_len = BAR_TICKS * 2;
        s.sync_roll();
        for id in [a, b] {
            let i = s.clip_index(id).unwrap();
            assert_eq!(s.clips[i].len_ticks(s.bpm, s.sample_rate), BAR_TICKS * 2);
        }
        // split clip a in the middle
        s.sel_clips = vec![a];
        s.playhead = 1920;
        s.split_at_playhead();
        assert_eq!(s.clips.len(), 3);
        let second = s.clips.iter().find(|c| c.start == 1920).unwrap();
        assert_eq!(second.len_ticks(s.bpm, s.sample_rate), BAR_TICKS * 2 - 1920);
        s.sel_clips = s.clips.iter().map(|c| c.id).collect();
        s.delete_selected_clips();
        assert!(s.clips.is_empty());
        assert_eq!(s.patterns.len(), 1, "deleting clips keeps the pattern");
    }

    #[test]
    fn pattern_add_clone_rename_delete() {
        let (mut s, _tx) = state_with_input();
        assert_eq!(s.patterns.len(), 1, "a first pattern exists at startup");
        let first = s.patterns[0].id;
        let second = s.add_pattern();
        assert_eq!(s.patterns[1].name, "Pattern 2");
        s.add_pattern_clip(0, 0, second).unwrap();
        s.add_pattern_clip(2, 960, second).unwrap();
        let copy = s.clone_pattern(second).unwrap();
        assert_eq!(s.patterns.len(), 3);
        s.rename_pattern(second, "  Hats  ");
        assert_eq!(s.patterns[s.pat_index(second).unwrap()].name, "Hats");
        assert!(s.clips.iter().all(|c| c.name == "Hats"));
        s.delete_pattern(second);
        assert!(s.clips.is_empty(), "deleting a pattern removes its clips");
        assert!(s.pat_index(second).is_none());
        assert!(s.pat_index(first).is_some() && s.pat_index(copy).is_some());
        assert!(s.selected_pattern.map_or(false, |p| s.pat_index(p).is_some()));
    }

    #[test]
    fn short_patterns_repeat_inside_longer_clips() {
        let notes = vec![Note { key: 60, velocity: 100, start: 0, len: 240 }, Note { key: 64, velocity: 100, start: 480, len: 960 }];
        let t = tile_notes(&notes, 960, 2400);
        // three passes (the last one cut at 2400); the long note is trimmed to the pattern end
        assert_eq!(t.iter().filter(|n| n.key == 60).count(), 3);
        assert_eq!(t.iter().filter(|n| n.key == 64).count(), 2, "the third pass starts after the clip end");
        assert!(t.iter().all(|n| n.start + n.len <= 2400));
        assert_eq!(t.iter().find(|n| n.key == 64).unwrap().len, 480);
    }

    #[test]
    fn audio_fx_tempo_follows_project_bpm() {
        let fx = AudioFx { sample_bpm: Some(100.0), ..AudioFx::default() };
        assert!((fx.tempo(120.0) - 1.2).abs() < 1e-9);
        assert_eq!(AudioFx::default().tempo(120.0), 1.0);
        assert!(AudioFx::default().is_neutral());
    }

    #[test]
    fn paste_lands_on_the_selected_track_and_keeps_the_layout() {
        let (mut s, _tx) = state_with_input();
        let pid = s.current_pattern();
        let a = s.add_pattern_clip(1, 0, pid).unwrap();
        let b = s.add_pattern_clip(2, 960, pid).unwrap();
        s.sel_clips = vec![a, b];
        assert_eq!(s.copy_selected(), 2);
        s.selected_track = 5;
        s.playhead = 3840;
        let before = s.clips.len();
        s.paste_at_playhead();
        assert_eq!(s.clips.len(), before + 2);
        let new: Vec<&UiClip> = s.clips.iter().filter(|c| c.id != a && c.id != b).collect();
        let mut tracks: Vec<usize> = new.iter().map(|c| c.track).collect();
        tracks.sort();
        assert_eq!(tracks, vec![5, 6], "the upper clip goes to the selected track, the other keeps its distance");
        assert_eq!(new.iter().find(|c| c.track == 5).unwrap().start, 3840);
        assert_eq!(new.iter().find(|c| c.track == 6).unwrap().start, 3840 + 960);
    }

    fn ramp(s: &mut State, id: u64) -> u32 {
        let i = s.clip_index(id).unwrap();
        match &mut s.clips[i].body {
            ClipBody::Automation { auto } => {
                auto.points = vec![AutoPoint { tick: 0, value: 0.0, shape: Shape::Linear }, AutoPoint { tick: auto.len, value: 1.0, shape: Shape::Linear }];
                auto.len
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn splitting_an_automation_clip_keeps_the_curve_and_pasting_keeps_it_automation() {
        let (mut s, _tx) = state_with_input();
        let id = s.create_automation(AutoTarget::TrackGain(0)).unwrap();
        let len = ramp(&mut s, id);
        s.sel_clips = vec![id];
        s.playhead = len / 2;
        s.split_at_playhead();
        let autos: Vec<(u32, Vec<AutoPoint>, u32)> = s.clips.iter().filter_map(|c| match &c.body {
            ClipBody::Automation { auto } => Some((c.start, auto.points.clone(), auto.len)),
            _ => None,
        }).collect();
        assert_eq!(autos.len(), 2);
        let (first, second) = if autos[0].0 < autos[1].0 { (&autos[0], &autos[1]) } else { (&autos[1], &autos[0]) };
        assert!((first.1.last().unwrap().value - 0.5).abs() < 1e-3, "first half ends where the curve was cut");
        assert!((second.1.first().unwrap().value - 0.5).abs() < 1e-3, "second half starts there");
        assert!((second.1.last().unwrap().value - 1.0).abs() < 1e-3);
        assert_eq!(first.2 + second.2, len);
        // copy + paste keeps clips automation clips
        s.sel_clips = s.clips.iter().map(|c| c.id).collect();
        s.copy_selected();
        s.playhead = len * 2;
        s.paste_at_playhead();
        assert_eq!(s.clips.iter().filter(|c| matches!(c.body, ClipBody::Automation { .. })).count(), 4);
    }

    #[test]
    fn automation_survives_a_project_round_trip_including_clip_volume_targets() {
        let (mut s, _tx) = state_with_input();
        let a = test_audio_clip(&mut s, 48_000);
        s.create_automation(AutoTarget::TrackGain(2)).unwrap();
        s.create_automation(AutoTarget::ClipGain(a)).unwrap();
        let dir = std::env::temp_dir().join(format!("rdaw_auto_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.rdaw");
        s.save_project(&path).unwrap();
        s.load_project(&path).unwrap();
        let audio_id = s.clips.iter().find(|c| matches!(c.body, ClipBody::Audio { .. })).map(|c| c.id).unwrap();
        let targets: Vec<AutoTarget> = s.clips.iter().filter_map(|c| match &c.body {
            ClipBody::Automation { auto } => Some(auto.target),
            _ => None,
        }).collect();
        assert_eq!(targets.len(), 2);
        assert!(targets.contains(&AutoTarget::TrackGain(2)));
        assert!(targets.contains(&AutoTarget::ClipGain(audio_id)), "clip volume automation follows the clip's new id");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn test_audio_clip(s: &mut State, frames: usize) -> u64 {
        let sd = Arc::new(SampleData { name: "t".into(), sample_rate: s.sample_rate, left: vec![0.5; frames], right: vec![0.5; frames] });
        let peaks = Arc::new(dense_peaks(&sd));
        let id = s.alloc_id();
        s.clips.push(UiClip { id, track: 0, start: 0, name: "t".into(), body: ClipBody::audio(sd, peaks, 0, frames as u32) });
        id
    }

    #[test]
    fn undo_and_redo_bring_back_deleted_clips() {
        let (mut s, _tx) = state_with_input();
        let id = test_audio_clip(&mut s, 48_000);
        s.sel_clips = vec![id];
        s.delete_selected_clips();
        assert!(s.clips.is_empty());
        s.undo();
        assert_eq!(s.clips.len(), 1);
        assert_eq!(s.clips[0].id, id);
        s.redo();
        assert!(s.clips.is_empty());
    }

    #[test]
    fn trimming_never_changes_the_speed_and_can_be_undone_by_dragging_back() {
        let (mut s, _tx) = state_with_input();
        let (bpm, sr) = (s.bpm, s.sample_rate);
        let frames = ticks_to_frames(PPQ * 2, bpm, sr);
        let id = test_audio_clip(&mut s, frames as usize);
        s.trim_clip_start_to(id, PPQ / 2, 0, 0, frames);
        let i = s.clip_index(id).unwrap();
        assert_eq!(s.clips[i].start, PPQ / 2);
        let (off, fr, fx) = match &s.clips[i].body {
            ClipBody::Audio { offset, frames, fx, .. } => (*offset, *frames, *fx),
            _ => unreachable!(),
        };
        assert!(off > 0 && fr < frames && off + fr == frames);
        assert!(fx.is_neutral(), "trimming must not touch pitch or BPM");
        // restore the whole sample
        s.set_clip_trim(id, Some(0), Some(0));
        let i = s.clip_index(id).unwrap();
        assert!(s.clips[i].start <= 1);
        assert!(matches!(&s.clips[i].body, ClipBody::Audio { offset: 0, frames: f, .. } if *f == frames));
    }

    #[test]
    fn clip_settings_become_engine_params() {
        let fx = AudioFx { gain_db: -6.0, pan: 1.0, fade_in_ms: 10.0, reverse: true, ..AudioFx::default() };
        let p = fx.params(48_000.0);
        assert!((p.gain_r - 0.501).abs() < 0.01 && p.gain_l.abs() < 1e-6);
        assert_eq!(p.fade_in, 480);
        assert!(p.reverse);
    }

    #[test]
    fn watch_folder_entries_load_lazily_and_follow_changes() {
        let (mut s, _tx) = state_with_input();
        let dir = std::env::temp_dir().join(format!("rustdaw_gui_watch_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.wav"), b"not really audio").unwrap();
        s.cfg.watch_folders = vec![dir.clone()];
        s.apply_scan(project_io::ScanResult { folder: dir.clone(), files: project_io::library_scan(&dir) });
        assert_eq!(s.library.len(), 1);
        assert!(s.library[0].data.is_none(), "must not decode before use");
        let id = s.library[0].id;
        assert!(!s.ensure_loaded(id), "garbage file must fail gracefully");
        assert!(s.library[0].failed);
        std::fs::write(dir.join("b.wav"), b"x").unwrap();
        s.apply_scan(project_io::ScanResult { folder: dir.clone(), files: project_io::library_scan(&dir) });
        assert_eq!(s.library.len(), 2);
        assert!(s.library.iter().any(|e| e.id == id && e.failed), "existing entries keep their state");
        s.remove_watch_folder(&dir);
        assert!(s.library.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- playlist regression tests: tails after stretch / BPM change / split ----

    fn tone_wav(name: &str, secs: f32) -> std::path::PathBuf {
        let pcm: Vec<f32> = (0..(secs * 48_000.0) as usize).flat_map(|i| {
            let t = i as f32 / 48_000.0;
            let v = (t * 220.0 * std::f32::consts::TAU).sin() * 0.5 * (1.0 - (t / secs));
            [v, v]
        }).collect();
        let p = std::env::temp_dir().join(format!("rustdaw_{}_{}.wav", name, std::process::id()));
        project_io::write_audio(&p, &pcm, 48_000, project_io::ExportFormat::WavFloat32).unwrap();
        p
    }

    fn clip_with_audio(s: &mut State, name: &str, secs: f32) -> u64 {
        let p = tone_wav(name, secs);
        let lib = s.import_path(&p).expect("import");
        s.add_audio_clip(0, 0, lib).expect("clip")
    }

    fn wait_fx(s: &mut State, id: u64) {
        for _ in 0..400 {
            s.poll_engine();
            if s.clip_fx(id).map_or(true, |(_, pending)| !pending) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("fx render never finished");
    }

    fn audio_parts(s: &State, id: u64) -> (u32, u32, u32) {
        let i = s.clip_index(id).unwrap();
        match &s.clips[i].body {
            ClipBody::Audio { sample, offset, frames, .. } => (*offset, *frames, sample.len() as u32),
            _ => panic!(),
        }
    }

    #[test]
    fn bpm_change_on_an_untrimmed_clip_leaves_no_tail() {
        let (mut s, _tx) = state_with_input();
        let id = clip_with_audio(&mut s, "bpm", 4.0);
        for bpm in [100.0, 87.5, 140.0, 95.0] {
            s.set_clip_fx(id, |f| f.sample_bpm = Some(bpm));
        }
        wait_fx(&mut s, id);
        let (off, frames, len) = audio_parts(&s, id);
        assert_eq!(off, 0);
        assert_eq!(off + frames, len, "clip must cover the whole stretched audio (frames {frames}, len {len})");
    }

    #[test]
    fn stretch_handle_gives_the_requested_length_and_no_tail() {
        let (mut s, _tx) = state_with_input();
        let id = clip_with_audio(&mut s, "stretch", 4.0);
        let want = BAR_TICKS * 2; // two bars
        let (ticks, _) = crate::panels::playlist::stretch_clip(&mut s, id, want).unwrap();
        assert_eq!(ticks, want);
        wait_fx(&mut s, id);
        let i = s.clip_index(id).unwrap();
        let got = s.clips[i].len_ticks(s.bpm, s.sample_rate);
        assert!((got as f32 - want as f32).abs() / (want as f32) < 0.01, "clip is {got} ticks, wanted {want}");
        let (off, frames, len) = audio_parts(&s, id);
        assert_eq!(off + frames, len, "no unplayed tail after stretching");
    }

    #[test]
    fn stretching_a_trimmed_clip_keeps_the_trim_and_the_requested_length() {
        let (mut s, _tx) = state_with_input();
        let id = clip_with_audio(&mut s, "stretch_trim", 4.0);
        // trim: remove 1 s from the start and 1 s from the end
        s.set_clip_trim(id, Some(48_000), Some(48_000));
        let (_, f0, l0) = audio_parts(&s, id);
        assert_eq!((f0, l0), (96_000, 192_000));
        let start = s.clips[s.clip_index(id).unwrap()].start;
        let want = BAR_TICKS; // 2 s of audio -> one bar (2 s at 120 BPM) -> ratio 1, use something else
        let want = want + PPQ; // 5 beats
        crate::panels::playlist::stretch_clip(&mut s, id, start + want).unwrap();
        wait_fx(&mut s, id);
        let i = s.clip_index(id).unwrap();
        let got = s.clips[i].len_ticks(s.bpm, s.sample_rate);
        assert!((got as f32 - want as f32).abs() / (want as f32) < 0.02, "clip is {got} ticks, wanted {want}");
        let (off, frames, len) = audio_parts(&s, id);
        // still trimmed by about a quarter on each side of the stretched audio
        let (a, b) = (off as f32 / len as f32, (len - off - frames) as f32 / len as f32);
        assert!((a - 0.25).abs() < 0.01 && (b - 0.25).abs() < 0.01, "trim fractions drifted: {a} / {b}");
    }

    #[test]
    fn splitting_leaves_two_contiguous_halves() {
        let (mut s, _tx) = state_with_input();
        let id = clip_with_audio(&mut s, "split", 4.0);
        s.playhead = PPQ * 4; // 2 s into the clip
        s.sel_clips = vec![id];
        s.split_at_playhead();
        assert_eq!(s.clips.len(), 2);
        let mut halves: Vec<(u32, u32, u32, u32)> = s.clips.iter().map(|c| match &c.body {
            ClipBody::Audio { offset, frames, sample, .. } => (c.start, *offset, *frames, sample.len() as u32),
            _ => panic!(),
        }).collect();
        halves.sort();
        let (a, b) = (halves[0], halves[1]);
        assert_eq!(a.1, 0);
        assert_eq!(a.1 + a.2, b.1, "second half must start exactly where the first ends");
        assert_eq!(b.1 + b.2, b.3, "second half must run to the end of the audio");
    }

    // ---- pattern clips: the right edge CONTINUES the pattern instead of copying it ----

    fn pattern_with_notes(s: &mut State) -> u64 {
        let pat = s.current_pattern();
        let pi = s.pat_index(pat).unwrap();
        s.patterns[pi].notes = vec![Note { key: 60, velocity: 100, start: 0, len: PPQ / 2 }, Note { key: 64, velocity: 100, start: PPQ, len: PPQ / 2 }];
        s.patterns[pi].len = BAR_TICKS;
        pat
    }

    fn clip_len(s: &State, id: u64) -> u32 {
        match &s.clips[s.clip_index(id).unwrap()].body {
            ClipBody::Pattern { len, .. } => *len,
            _ => panic!(),
        }
    }

    #[test]
    fn dragging_a_pattern_clip_edge_continues_the_pattern_without_copying_notes() {
        use crate::panels::playlist::extend_pattern_clip;
        let (mut s, _tx) = state_with_input();
        let pat = pattern_with_notes(&mut s);
        let a = s.add_pattern_clip(0, 0, pat).unwrap();
        let b = s.add_pattern_clip(1, 0, pat).unwrap(); // second instance, one run of the pattern
        let before = s.patterns[s.pat_index(pat).unwrap()].notes.clone();

        let (len, continued) = extend_pattern_clip(&mut s, a, BAR_TICKS * 3, false, BAR_TICKS).unwrap();
        assert_eq!((len, continued), (BAR_TICKS * 3, true));
        let pi = s.pat_index(pat).unwrap();
        assert_eq!(s.patterns[pi].len, BAR_TICKS * 3, "the pattern itself became longer");
        assert_eq!(s.patterns[pi].notes, before, "no notes were copied");
        assert_eq!(clip_len(&s, b), BAR_TICKS * 3, "the other single-run instance follows the pattern length");

        // the engine must not receive repeated notes: the clip is exactly one (longer) run of the pattern
        let tiled = crate::state::tile_notes(&s.patterns[pi].notes, s.patterns[pi].len, clip_len(&s, a));
        assert_eq!(tiled.len(), before.len());

        // dragging back inside the SAME drag restores everything
        let (len, continued) = extend_pattern_clip(&mut s, a, BAR_TICKS, false, BAR_TICKS).unwrap();
        assert_eq!((len, continued), (BAR_TICKS, false));
        assert_eq!(s.patterns[s.pat_index(pat).unwrap()].len, BAR_TICKS);
        assert_eq!(clip_len(&s, b), BAR_TICKS);
    }

    #[test]
    fn ctrl_drag_repeats_the_pattern_and_shortening_only_trims_the_clip() {
        use crate::panels::playlist::extend_pattern_clip;
        let (mut s, _tx) = state_with_input();
        let pat = pattern_with_notes(&mut s);
        let a = s.add_pattern_clip(0, 0, pat).unwrap();
        let (len, continued) = extend_pattern_clip(&mut s, a, BAR_TICKS * 3, true, BAR_TICKS).unwrap();
        assert_eq!((len, continued), (BAR_TICKS * 3, false));
        let pi = s.pat_index(pat).unwrap();
        assert_eq!(s.patterns[pi].len, BAR_TICKS, "Ctrl leaves the pattern alone");
        let tiled = crate::state::tile_notes(&s.patterns[pi].notes, s.patterns[pi].len, clip_len(&s, a));
        assert_eq!(tiled.len(), 6, "three repeats of two notes");
        // shorten below the pattern length: only the clip changes
        let (len, _) = extend_pattern_clip(&mut s, a, PPQ * 2, false, BAR_TICKS).unwrap();
        assert_eq!(len, PPQ * 2);
        assert_eq!(s.patterns[pi].len, BAR_TICKS);
    }

    #[test]
    fn stretch_shows_the_target_length_immediately() {
        let (mut s, _tx) = state_with_input();
        let id = clip_with_audio(&mut s, "preview", 4.0);
        let want = BAR_TICKS * 3;
        crate::panels::playlist::stretch_clip(&mut s, id, want).unwrap();
        assert_eq!(s.stretch_preview.get(&id), Some(&want), "the playlist draws this length while the render is pending");
    }
}
