//! Integration test against real VST3 binaries (built from `testplugins/`).
//! Set RUSTDAW_TEST_VST3 to a folder containing `gain.vst3` and `testsynth.vst3`; skipped otherwise.
use plugin_host::*;
use std::path::PathBuf;

fn dir() -> Option<PathBuf> {
    let d = std::env::var("RUSTDAW_TEST_VST3").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/tmp/vst3test"));
    d.exists().then_some(d)
}

const T: Transport = Transport { playing: true, tempo: 120.0, quarters: 0.0 };

#[test]
fn scans_loads_and_processes_real_plugins() {
    let Some(d) = dir() else {
        eprintln!("RUSTDAW_TEST_VST3 not set: skipping");
        return;
    };
    let list = scan(&[d]);
    let names: Vec<_> = list.iter().map(|p| p.name.clone()).collect();
    assert!(names.iter().any(|n| n.contains("Gain")), "scan found {names:?}");
    assert!(names.iter().any(|n| n == "TestSynth"), "scan found {names:?}");

    // --- effect: parameters + audio ---
    let gain = list.iter().find(|p| p.name.contains("Gain")).unwrap();
    let (mut fx, ctrl) = load(gain, 48_000.0, 512).expect("load gain");
    let ctrl = ctrl.expect("controller");
    assert!(!ctrl.params.is_empty());
    let id = ctrl.params[0].id;
    let (mut l, mut r) = (vec![0.5f32; 256], vec![-0.25f32; 256]);
    fx.process_effect(&mut l, &mut r, &T);
    assert!((l[10] - 0.5).abs() < 1e-6 && (r[10] + 0.25).abs() < 1e-6, "unity gain expected, got {}", l[10]);
    ctrl.set(id, 0.5);
    assert!((ctrl.get(id) - 0.5).abs() < 1e-9);
    assert!(!ctrl.display(id, 0.5).is_empty());
    fx.set_param(id, 0.5);
    let (mut l, mut r) = (vec![0.5f32; 256], vec![-0.25f32; 256]);
    fx.process_effect(&mut l, &mut r, &T);
    assert!((l[10] - 0.25).abs() < 1e-6 && (r[10] + 0.125).abs() < 1e-6, "half gain expected, got {}", l[10]);

    // --- instrument: MIDI events with sample-accurate offsets ---
    let mut synth_info = list.iter().find(|p| p.name == "TestSynth").unwrap().clone();
    synth_info.is_instrument = true;
    let (mut synth, _c) = load(&synth_info, 48_000.0, 512).expect("load synth");
    let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
    synth.process_instrument(&[MidiEvent { frame: 100, kind: EventKind::NoteOn { key: 69, velocity: 100 } }], &mut l, &mut r, &T);
    assert!(l[..100].iter().all(|s| s.abs() < 1e-6), "output before the note-on offset");
    assert!(l[100..].iter().any(|s| s.abs() > 0.05), "note-on produced no sound");
    let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
    synth.process_instrument(&[MidiEvent { frame: 50, kind: EventKind::NoteOff { key: 69 } }], &mut l, &mut r, &T);
    assert!(l[60..].iter().all(|s| s.abs() < 1e-6), "note-off did not silence the synth");
}

#[test]
fn bad_path_is_an_error_not_a_crash() {
    let info = PluginInfo { name: "x".into(), vendor: String::new(), path: PathBuf::from("/nonexistent/x.vst3"), class_id: [0; 16], is_instrument: false };
    assert!(load(&info, 48_000.0, 512).is_err());
}
