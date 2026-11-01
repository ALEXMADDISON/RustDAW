use core_engine::*;
use dsp::SampleData;
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::Arc;

fn burst(n: usize) -> Arc<SampleData> {
    let v: Vec<f32> = (0..n).map(|i| (i as f32 * 0.3).sin() * 0.8).collect();
    Arc::new(SampleData { name: "b".into(), sample_rate: 48_000.0, left: v.clone(), right: v })
}

fn peak(buf: &[f32]) -> f32 {
    buf.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

fn run(engine: &mut Engine, frames: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; 2 * frames];
    for chunk in out.chunks_mut(2 * 480) {
        engine.process(chunk);
    }
    out
}

fn midi(id: u64, track: u16, start: u32, len: u32, notes: &[(u8, u32, u32)]) -> RtClip {
    RtClip {
        id,
        track,
        start,
        gain_mul: 1.0,
        kind: RtClipKind::Midi { notes: notes.iter().map(|&(k, s, l)| Note { key: k, velocity: 100, start: s, len: l }).collect(), len },
    }
}

fn audio(id: u64, track: u16, start: u32, sample: Arc<SampleData>, offset: u32, frames: u32) -> RtClip {
    RtClip { id, track, start, gain_mul: 1.0, kind: RtClipKind::Audio { sample, offset, frames, params: core_engine::AudioParams::default() } }
}

#[test]
fn midi_clip_plays_through_the_built_in_synth_at_the_right_time() {
    let (mut e, mut h) = create_engine(48_000.0);
    h.send(Command::SetInstrument { track: 2, kind: InstrumentKind::Synth });
    // clip at beat 1 (0.5 s at 120 BPM) with a note at its start, one beat long
    h.send(Command::SetClips(Box::new(ClipList { automation: Vec::new(), clips: vec![midi(1, 2, 960, 3840, &[(69, 0, 960)])] })));
    h.send(Command::Play);
    let out = run(&mut e, 48_000 * 2);
    assert!(peak(&out[..2 * 23_000]) < 1e-4, "sound before the clip");
    assert!(peak(&out[2 * 24_200..2 * 30_000]) > 0.05, "note not audible");
    // note-off at 1.0 s, release 0.2 s => silent by 1.4 s
    assert!(peak(&out[2 * 72_000..]) < 1e-3, "note kept sounding after note-off");
}

#[test]
fn audio_clip_start_trim_and_stop_position() {
    let (mut e, mut h) = create_engine(48_000.0);
    h.send(Command::SetClips(Box::new(ClipList { automation: Vec::new(), clips: vec![audio(1, 3, 960, burst(4800), 0, 4800)] })));
    h.send(Command::Play);
    let out = run(&mut e, 48_000);
    assert!(peak(&out[..2 * 23_000]) < 1e-4);
    assert!(peak(&out[2 * 24_100..2 * 28_000]) > 0.05);
    assert!(peak(&out[2 * 29_500..]) < 1e-4);
    h.send(Command::Stop);
    let out = run(&mut e, 4800);
    assert!(peak(&out) < 1e-4, "audio after Stop");
    // Stop pauses in place; returning to the start position is a Seek issued by the GUI.
    let paused = h.meters.position();
    assert!(paused > 0);
    h.send(Command::Seek(0));
    run(&mut e, 480);
    assert_eq!(h.meters.position(), 0);
}

#[test]
fn mute_solo_and_seek_into_clip() {
    let (mut e, mut h) = create_engine(48_000.0);
    h.send(Command::SetClips(Box::new(ClipList { automation: Vec::new(), clips: vec![audio(1, 0, 0, burst(96_000), 0, 96_000), audio(2, 1, 0, burst(96_000), 0, 96_000)] })));
    h.send(Command::Seek(960));
    h.send(Command::Play);
    assert!(peak(&run(&mut e, 4800)) > 0.05, "seeking into a clip should play it");
    h.send(Command::SetTrackMute { track: 0, mute: true });
    h.send(Command::SetTrackMute { track: 1, mute: true });
    assert!(peak(&run(&mut e, 4800)) < 1e-4, "muted tracks still audible");
    h.send(Command::SetTrackMute { track: 1, mute: false });
    h.send(Command::SetTrackSolo { track: 0, solo: true }); // solo on a muted track: nothing else may play
    assert!(peak(&run(&mut e, 4800)) < 1e-4, "solo must silence other tracks");
    h.send(Command::SetTrackMute { track: 0, mute: false });
    assert!(peak(&run(&mut e, 4800)) > 0.05);
}

#[test]
fn cycle_loops_notes_and_audio() {
    let (mut e, mut h) = create_engine(48_000.0);
    h.send(Command::SetInstrument { track: 0, kind: InstrumentKind::Synth });
    h.send(Command::SetClips(Box::new(ClipList { automation: Vec::new(), clips: vec![midi(1, 0, 0, 3840, &[(69, 0, 240)]), audio(2, 1, 0, burst(2400), 0, 2400)] })));
    h.send(Command::SetCycle(Some((0, 960)))); // one beat = 0.5 s
    h.send(Command::Play);
    let out = run(&mut e, 48_000 * 2); // four loops
    for k in 0..4usize {
        let a = 2 * (k * 24_000 + 200);
        assert!(peak(&out[a..a + 2 * 2000]) > 0.05, "loop {k} did not retrigger");
    }
    assert!(h.meters.position() < 960, "playhead escaped the cycle: {}", h.meters.position());
}

#[test]
fn export_is_faster_than_realtime_bounded_and_restores_state() {
    let (mut e, mut h) = create_engine(48_000.0);
    h.send(Command::SetInstrument { track: 0, kind: InstrumentKind::Synth });
    h.send(Command::SetClips(Box::new(ClipList {
        automation: Vec::new(),
        clips: vec![midi(1, 0, 0, 3840, &[(69, 0, 960)]), audio(2, 1, 3840, burst(48_000), 0, 48_000), midi(3, 0, 38_400, 3840, &[(69, 0, 960)])],
    })));
    run(&mut e, 480); // apply commands
    let progress = AtomicU32::new(0);
    let cancel = AtomicBool::new(false);
    // export bar 1..2 only (3840 ticks = 2 s) + 0.5 s tail: the clip at tick 38400 must NOT be included
    let pcm = e.export_render(0, 3840, 0.5, &progress, &cancel);
    assert_eq!(pcm.len(), 2 * (96_000 + 24_000));
    assert!(peak(&pcm[2 * 100..2 * 20_000]) > 0.05, "note missing from export");
    assert!(peak(&pcm[2 * 48_000..2 * 96_000]) < 1e-3, "export contains audio after the first note ended");
    assert_eq!(progress.load(std::sync::atomic::Ordering::Relaxed), 1000);
    // engine is idle again and playable
    h.send(Command::Play);
    assert!(peak(&run(&mut e, 4800)) > 0.05);
}

#[test]
fn preview_plays_to_master_when_stopped() {
    let (mut e, mut h) = create_engine(48_000.0);
    h.send(Command::Preview(burst(4800)));
    assert!(peak(&run(&mut e, 960)) > 0.05);
}

/// Real VST3 plugins (see testplugins/). Skipped when the test bundles are not available.
#[test]
fn vst3_instrument_and_effect_in_the_track_chain() {
    let dir = std::env::var("RUSTDAW_TEST_VST3").unwrap_or_else(|_| "/tmp/vst3test".into());
    if !std::path::Path::new(&dir).exists() {
        eprintln!("no test plugins: skipping");
        return;
    }
    let list = plugin_host::scan(&[dir.into()]);
    let mut synth_info = list.iter().find(|p| p.name == "TestSynth").unwrap().clone();
    synth_info.is_instrument = true;
    let gain_info = list.iter().find(|p| p.name.contains("Gain")).unwrap().clone();
    let (mut e, mut h) = create_engine(48_000.0);
    let (synth, _c1) = plugin_host::load(&synth_info, 48_000.0, MAX_BLOCK).unwrap();
    let (gain, c2) = plugin_host::load(&gain_info, 48_000.0, MAX_BLOCK).unwrap();
    let gain_param = c2.unwrap().params[0].id;
    h.send(Command::SetPlugin { track: 4, slot: 0, plugin: Some(Box::new(synth)) });
    h.send(Command::SetPlugin { track: 4, slot: 1, plugin: Some(Box::new(gain)) });
    h.send(Command::SetClips(Box::new(ClipList { automation: Vec::new(), clips: vec![midi(1, 4, 0, 3840, &[(69, 0, 3840)])] })));
    h.send(Command::Play);
    let loud = peak(&run(&mut e, 9600));
    assert!(loud > 0.05, "VST instrument silent through the chain: {loud}");
    h.send(Command::SetPluginParam { track: 4, slot: 1, id: gain_param, value: 0.0 });
    let silent = peak(&run(&mut e, 9600));
    assert!(silent < 1e-3, "VST effect parameter had no effect: {silent}");
    // replacing a plugin hands the old one back for freeing on the GUI thread
    h.send(Command::SetPlugin { track: 4, slot: 1, plugin: None });
    run(&mut e, 480);
    h.collect_garbage();
}

// ---- automation ----

fn flat(secs: usize, level: f32) -> Arc<SampleData> {
    let n = 48_000 * secs;
    Arc::new(SampleData { name: "flat".into(), sample_rate: 48_000.0, left: vec![level; n], right: vec![level; n] })
}

fn auto_clip(id: u64, target: AutoTarget, start: u32, len: u32, pts: &[(u32, f32)]) -> RtAuto {
    RtAuto::new(id, target, start, len, pts.iter().map(|&(t, v)| AutoPoint { tick: t, value: v, shape: Shape::Linear }).collect())
}

fn level(out: &[f32], from_s: f32, to_s: f32) -> (f32, f32) {
    let (a, b) = ((from_s * 48_000.0) as usize, (to_s * 48_000.0) as usize);
    let l = out[2 * a..2 * b].iter().step_by(2).fold(0.0f32, |m, s| m.max(s.abs()));
    let r = out[2 * a + 1..2 * b].iter().step_by(2).fold(0.0f32, |m, s| m.max(s.abs()));
    (l, r)
}

#[test]
fn track_gain_automation_fades_the_track_while_it_plays() {
    let (mut e, mut h) = create_engine(48_000.0);
    // 2 s (3840 ticks at 120 BPM): volume 0 -> 0.6 (= 0.9 in engine units, the default track gain)
    let list = ClipList { clips: vec![audio(1, 0, 0, flat(4, 0.5), 0, 48_000 * 4)], automation: vec![auto_clip(10, AutoTarget::TrackGain(0), 0, 3840, &[(0, 0.0), (3840, 0.6)])] };
    h.send(Command::SetClips(Box::new(list)));
    h.send(Command::Play);
    let out = run(&mut e, 48_000 * 3);
    let early = level(&out, 0.0, 0.1).0;
    let mid = level(&out, 0.95, 1.05).0;
    let late = level(&out, 2.4, 2.6).0;
    assert!(early < mid && mid < late, "volume must rise: {early} {mid} {late}");
    assert!(early < 0.15 * late, "starts (almost) silent: {early} vs {late}");
    // after the clip ended the value is held, not reset
    let held = level(&out, 2.8, 3.0).0;
    assert!((held - late).abs() < 0.02, "value is held after the clip: {late} vs {held}");
}

#[test]
fn pan_automation_moves_the_sound_to_one_side() {
    let (mut e, mut h) = create_engine(48_000.0);
    let list = ClipList { clips: vec![audio(1, 0, 0, flat(2, 0.5), 0, 48_000 * 2)], automation: vec![auto_clip(10, AutoTarget::TrackPan(0), 0, 3840, &[(0, 1.0), (3840, 1.0)])] };
    h.send(Command::SetClips(Box::new(list)));
    h.send(Command::Play);
    let out = run(&mut e, 48_000);
    let (l, r) = level(&out, 0.2, 0.9);
    assert!(l < 1e-3 && r > 0.1, "hard right expected, got L {l} R {r}");
}

#[test]
fn overlapping_automation_clips_do_not_fight_the_later_start_wins() {
    let (mut e, mut h) = create_engine(48_000.0);
    // A: low volume for 4 s; B starts at 2 s (3840 ticks) with a high value for 2 s
    let a = auto_clip(10, AutoTarget::TrackGain(0), 0, 7680, &[(0, 0.2), (7680, 0.2)]);
    let b = auto_clip(11, AutoTarget::TrackGain(0), 3840, 3840, &[(0, 0.6), (3840, 0.6)]);
    let list = ClipList { clips: vec![audio(1, 0, 0, flat(5, 0.5), 0, 48_000 * 5)], automation: vec![a, b] };
    h.send(Command::SetClips(Box::new(list)));
    h.send(Command::Play);
    let out = run(&mut e, 48_000 * 5);
    let first = level(&out, 0.5, 1.5).0;
    let second = level(&out, 2.5, 3.5).0;
    let third = level(&out, 4.2, 4.8).0;
    assert!(second > first * 2.0, "the later clip takes over: {first} -> {second}");
    assert!(third > first * 2.0, "after B ends its last value is held: {third}");
}

#[test]
fn clip_gain_automation_only_changes_that_clip() {
    let (mut e, mut h) = create_engine(48_000.0);
    let list = ClipList {
        clips: vec![audio(1, 0, 0, flat(2, 0.4), 0, 96_000), audio(2, 1, 0, flat(2, 0.4), 0, 96_000)],
        automation: vec![auto_clip(10, AutoTarget::ClipGain(1), 0, 3840, &[(0, 0.0), (3840, 0.0)])],
    };
    h.send(Command::SetClips(Box::new(list)));
    h.send(Command::Play);
    let out = run(&mut e, 48_000);
    // clip 1 is muted by its automation, clip 2 (another track) still plays
    let (l, _) = level(&out, 0.2, 0.9);
    assert!(l > 0.1, "clip 2 must stay audible");
    let (e2, mut h2) = create_engine(48_000.0);
    let mut e2 = e2;
    h2.send(Command::SetClips(Box::new(ClipList { clips: vec![audio(1, 0, 0, flat(2, 0.4), 0, 96_000)], automation: vec![auto_clip(10, AutoTarget::ClipGain(1), 0, 3840, &[(0, 0.0), (3840, 0.0)])] })));
    h2.send(Command::Play);
    let out2 = run(&mut e2, 48_000);
    assert!(level(&out2, 0.2, 0.9).0 < 1e-3, "the automated clip alone is silent");
}
