<img width="3440" height="1408" alt="image" src="https://github.com/user-attachments/assets/4196bf19-a20e-407e-a01a-efebe1fb7c97" />


# Rust DAW


Open-source DAW in Rust (egui + cpal).

* 100-track playlist: audio clips, MIDI clips, per-track mute/solo/arm/volume, recording, cycle loop
* Piano roll for MIDI clips (click to add, drag to move, edge to resize, right-click to delete)
* Per-track instrument (built-in synth, sampler, or VST3 with its own editor window on Windows) and 4 VST3 insert-effect slots
* Library with watch folders (auto-updating), drag & drop onto lanes / track headers
* Export to WAV (16/24-bit, 32-bit float) and MP3 (128-320 kbps), rendered faster than real time

## Build & run
    cargo run --release -p daw

Windows needs the "Desktop development with C++" build tools (MSVC) for Rust and for the bundled MP3 encoder.
Linux: `libasound2-dev pkg-config` (and `libxkbcommon-x11-0` etc. for the window).

