# Rust DAW

Open-source DAW in Rust (egui + cpal). Logic-style dark UI.

* 100-track playlist: audio clips, MIDI clips, per-track mute/solo/arm/volume, recording, cycle loop
* Piano roll for MIDI clips (click to add, drag to move, edge to resize, right-click to delete)
* Per-track instrument (built-in synth, sampler, or VST3 with its own editor window on Windows) and 4 VST3 insert-effect slots
* Library with watch folders (auto-updating), drag & drop onto lanes / track headers
* Export to WAV (16/24-bit, 32-bit float) and MP3 (128-320 kbps), rendered faster than real time

## Build & run
    cargo run --release -p daw

Windows needs the "Desktop development with C++" build tools (MSVC) for Rust and for the bundled MP3 encoder.
Linux: `libasound2-dev pkg-config` (and `libxkbcommon-x11-0` etc. for the window).

## Tests
    cargo test --workspace
VST3 integration tests use the plugins in `testplugins/` (see its README); they are skipped if the bundles are not found.

## Layout
`dsp` (oscillator/envelope/sample) - `native-plugins` (synth, sampler) - `plugin-host` (VST3 host) -
`core-engine` (real-time engine, export) - `project-io` (audio import/export, library watcher, config) -
`gui` (egui front-end) - `apps/daw` (audio devices + window)

Licence note: MP3 export uses LAME (LGPL) through the `mp3lame-encoder` crate. If you distribute binaries,
check the LGPL obligations (or build without MP3 export).
