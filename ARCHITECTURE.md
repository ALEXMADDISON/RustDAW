# Rust DAW — Phase 1 Architecture

## 1. Workspace

```
rustdaw/
├─ Cargo.toml                 workspace + shared dependency versions
├─ crates/
│  ├─ dsp/                    oscillators (PolyBLEP), ADSR; pure, alloc-free
│  ├─ native-plugins/         SimpleSynth now; 3xOSC / Sampler / EQ / Reverb / Delay next
│  ├─ core-engine/            model, command/feedback rings, sequencer, mixer, Engine
│  ├─ project-io/             serde project (JSON now; BSON/zstd later), MIDI via `midly`, audio via `symphonia`
│  ├─ plugin-host/            VST3 / CLAP boundary (Phase 3)
│  └─ gui/                    dock-layout model (+ egui front-end in Phase 2)
└─ apps/daw/                  binary: cpal stream + GUI thread
```
Dependency direction: `dsp ← native-plugins ← core-engine ← {project-io, gui, apps/daw}`.
`dsp` never depends on anything audio-I/O related, so it stays testable and reusable.

## 2. Threading & lock-free design

```
 GUI thread                                         Audio thread (cpal callback)
 ┌───────────────┐   Command ring (rtrb SPSC)    ┌────────────────────────────┐
 │ EngineHandle  │ ────────────────────────────▶ │ Engine::process()          │
 │  send()       │   Box<Pattern> ownership moves│  drain_commands()          │
 │               │                               │  schedule note events      │
 │               │ ◀──────────────────────────── │  sort events (in place)    │
 │  fb_rx        │   Feedback ring (pos, peaks)  │  render segments per event │
 │               │ ◀──────────────────────────── │  mixer: bus N → … → master │
 │ collect_      │   Garbage ring (old Box<..>)  │  tanh soft-clip → out      │
 │  garbage()    │                               └────────────────────────────┘
 └───────────────┘
```
Rules enforced by construction:
* Everything the callback touches is pre-allocated in `create_engine` (buses, voices, event array, pattern slots).
* New patterns arrive as `Box<Pattern>`; the replaced one is sent back on the garbage ring so the free happens on the GUI thread. If that ring is ever full we leak rather than free on the audio thread.
* Instruments and FX are enums (`Instrument`, `Fx`) → static dispatch, no `dyn` in the hot path.
* Events are sample-accurate: notes become `(frame offset, kind)`, sorted in place, and the block is rendered in segments between events.
* Mixer routing in Phase 1: a bus may only target a lower index (0 = master), so one reverse pass is a valid topological order. Phase 2 replaces this with a real graph (sidechain, sends).

### Core models
`Note{channel,key,velocity,start,len}` (960 PPQ) → `Pattern` (sorted notes, looping length) ← `StepGrid` (Channel Rack, swing, per-step velocity/pitch) → `Playlist{Clip{track,start,len,kind}}` with `ClipKind::{Pattern, Audio, Automation}`; clips are track-agnostic like FL. Mixer: 16 inserts + master, 8 FX slots, gain/pan/mute/route, peak metering.

## 3. GUI layout (egui, Phase 2)
* Rendering: `eframe` + `egui_dock`. `gui::Dock::fl_default()` is the serializable layout tree
  (toolbar on top, browser left, Channel Rack + Playlist/Piano Roll tabs in the middle, Mixer bottom).
  Map it to `egui_dock::DockState` and implement `TabViewer::ui` per `Panel`.
* Piano Roll / Playlist / step grid are custom-painted with `egui::Painter` (rects + lines), with the
  zoom/scroll transform in one struct; if profiling shows egui too slow for dense grids, move those
  panels to a `wgpu` paint callback (instanced quads) while keeping egui for chrome.
* The GUI never touches engine state directly. Each frame: drain `fb_rx` for playhead/meters,
  call `collect_garbage()`, push `Command`s for edits.
* FL look: dark theme via `egui::Visuals`, high-contrast orange/green accents, compact spacing.

## 4. Roadmap
| Phase | Scope |
|---|---|
| 1 (this) | Workspace, engine, ring buffers, synth, step sequencer, headless test |
| 2 | Piano Roll (chop/flam/arp/strum/quantize/slide), real routing graph, parametric EQ + FFT analyzer, egui shell |
| 3 | Playlist/song mode + automation clips, sampler with time-stretch, delay/reverb, project I/O (MIDI, WAV/FLAC) |
| 4 | CLAP then VST3 hosting (consider `nih-plug` for authoring, `clack` for CLAP hosting), ASIO/JACK/PipeWire backends |

Honest scope note: full FL Studio feature parity is a multi-year, multi-contributor effort. This
phased structure gets a working core early and keeps each piece replaceable.

## Patterns, audio clip editing, undo and projects (phase 6)

* **Patterns** (`gui/state.rs`, `panels/patterns.rs`): a `Pattern` is a named note list; playlist
  `ClipBody::Pattern { pat, len }` clips point at it, so editing the pattern in the piano roll updates
  every clip. When a clip is longer than its pattern the notes repeat (`tile_notes`).
* **Audio clips** keep the untouched source (`orig`) and a rendered copy (`sample`). Pitch and tempo
  (`AudioFx`: semitones, source BPM, Stretch/Resample mode, algorithm) are rendered off the audio
  thread by `dsp::process_clip` (WSOLA time-stretch + cubic resampling) in a worker; newer requests
  for a clip replace older ones. Trim is just `offset` / `frames` into the rendered sample, so it never
  changes speed. Gain, pan, fades and reverse are *not* rendered: they travel to the engine as
  `AudioParams` and are applied while the clip plays.
* **Sample Editor** (`app.rs::sample_editor`): edits are applied live; Cancel restores a backup of the
  clip, Apply / closing keeps it. Preview loops the clip through the normal transport.
* **Undo / redo**: snapshot based (`State::checkpoint`, `undo`, `redo`) over clips and patterns, 100 steps.
* **Projects** (`gui/project.rs`): `<name>.rdaw` JSON + `<name>_audio/` WAV sources. Track names, mixer settings and built-in instruments are saved;
  VST3 plug-ins are saved with their full state (`IComponent::getState` + controller state, via an in-memory
  `IBStream`) in `<n>_plugins/*.bin`; on load the state is handed to the plug-in before it starts
  processing (`plugin_host::load_with_state`). Changed parameters are saved too as a fallback.

## Automation (phase 9)

* **Model** (`core-engine/src/automation.rs`, shared by GUI and audio thread): an `AutoTarget` (track volume /
  pan / pitch, master volume, one audio clip's volume, any VST3 parameter) and a list of `AutoPoint`s
  (tick, normalized value, curve shape: linear, step, S-curve, ease in / out). The GUI keeps automation
  as `ClipBody::Automation` clips in the playlist, so move / copy / split / undo / project save all work on them.
  In the playlist every track row has, under its own lane, one automation layer per automated parameter
  (a track without automation shows none; the "+" in its header adds one), so automation never overlaps the track's pattern / audio clips or
  another parameter's automation. The clip's `track` field is unchanged; the layer follows from its target.
* **Playback**: `SetClips` carries the automation list. Each audio block the engine evaluates every
  automated parameter at the block's start position and applies it (`Engine::apply_automation`); track
  gain / pan are ramped across the block, VST parameters go through `PluginProcessor::set_param`.
  Parameters without automation clips are never touched.
* **Rules** (one clip owns a parameter at a time): inside a clip the parameter follows its curve; when clips
  overlap, the one that starts latest wins (ties: created later); between clips the last value is held; before
  the first clip its first value is used. The playlist darkens the part of a clip that is overridden.
* **GUI**: right-click a mixer slider / pan / plugin parameter, or use "+ Automation" / a track header's
  right-click menu. `State::sync_automation_display` makes sliders and plug-in knobs follow the automation.
  The editor is `panels/automation.rs`.
