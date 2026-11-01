<img width="3440" height="1408" alt="image" src="https://github.com/user-attachments/assets/5bd00cf5-28e4-4d53-9c6e-fd4ede82e8b8" />



# Rust DAW

An open-source digital audio workstation (DAW) built with **Rust**, **egui**, and **cpal**. Features a dark, Logic Pro-inspired interface, a multi-track arrangement timeline, MIDI editing, VST3 plugin support, and high-performance audio rendering.

## Features

- **100-track arrangement** — Audio and MIDI clips, with per-track volume, mute, solo, and record-arm controls.
- **Recording & playback** — Audio recording, real-time playback, and cycle-loop support.
- **MIDI piano roll** — Create notes with a click, drag to move, resize by dragging note edges, and right-click to delete.
- **Instrument support** — Built-in synthesizer, sampler, and VST3 instruments. VST3 plugins can open their own editor windows on Windows.
- **VST3 effects** — Up to four insert-effect slots per track.
- **Audio library** — Watch folders for automatic library updates. Drag and drop samples and audio files onto tracks or track headers.
- **Audio export** — WAV (16-bit, 24-bit, and 32-bit float) and MP3 (128–320 kbps).
- **Fast rendering** — Export audio faster than real time.

## Getting Started

### Build and Run

```bash
cargo run --release -p daw
```

### Requirements

**Windows**
- Rust toolchain
- Visual Studio Build Tools with the **Desktop development with C++** workload (MSVC)
- Required for Rust native dependencies and the bundled MP3 encoder.

**Linux**

Install the required system dependencies:

```bash
sudo apt install libasound2-dev pkg-config libxkbcommon-x11-0
```

Additional system libraries may be required depending on your distribution and desktop environment.

## Tests

Run the complete workspace test suite:

```bash
cargo test --workspace
```

VST3 integration tests use the example plugins in `testplugins/`. See the README in that directory for setup instructions.

These tests are automatically skipped when the required plugin bundles are not found.

## Project Structure

| Crate | Description |
|---|---|
| `dsp` | Oscillators, envelopes, and sample processing |
| `native-plugins` | Built-in synthesizer and sampler |
| `plugin-host` | VST3 plugin hosting |
| `core-engine` | Real-time audio engine and offline rendering |
| `project-io` | Audio import/export, library watcher, and configuration |
| `gui` | egui-based user interface |
| `apps/daw` | Application entry point, audio devices, and window management |
