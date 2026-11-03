<img width="3440" height="1408" alt="image" src="https://github.com/user-attachments/assets/4196bf19-a20e-407e-a01a-efebe1fb7c97" />


# RustDAW 🎛️
### A Digital Audio Workstation Written in Rust — Because Apparently Making Music Wasn't Painful Enough.

**RustDAW** is an open-source Digital Audio Workstation built with Rust, egui, and cpal. It features a dark, Logic Pro & Fl Studio -inspired interface and a modular architecture designed for audio production, MIDI sequencing, plugin hosting, and audio rendering.

It's a place where musical creativity meets imagination. 

**Make music. Break things. Blame the compiler.**

---

## ✨ Features

### 🎹 MIDI & Sequencing
- Piano Roll editor for creating and editing MIDI notes.
- Click to create notes, drag to move them, and resize them using their edges.
- Right-click to delete notes.
- MIDI clips and audio clips in the playlist.
- Up to 100 tracks.
- Cycle loop playback.

### 🎚️ Mixer & Track Management
- Individual track volume controls.
- Mute, Solo, and Record Arm functionality.
- Multiple audio and MIDI tracks.
- Four VST3 insert-effect slots per track.
- Built-in track instrument selection.

### 🎛️ Instruments & Plugins
- Built-in sampler.
- VST3 instrument hosting.
- VST3 effect hosting.
- Native plugin editor windows on Windows.

### 📁 Sample Library
- Watch folders for automatic library updates.
- Browse your audio samples.
- Drag and drop samples directly onto playlist lanes or track headers.

### 🎧 Audio Engine & Export
- Real-time audio processing.
- WAV export:
  - 16-bit PCM
  - 24-bit PCM
  - 32-bit floating point
- MP3 export:
  - 128–320 kbps
- Offline rendering faster than real time.

Your masterpiece can finally leave your computer and disappoint people on other devices.

---

## 🛠️ Technology Stack

RustDAW is built using a modular Rust workspace.

| Technology | Purpose |
|---|---|
| Rust | Core programming language |
| egui | Graphical user interface |
| eframe | Application framework |
| cpal | Cross-platform audio I/O |
| rtrb | Lock-free real-time audio communication |
| Symphonia | Audio decoding |
| vst3 | VST3 plugin integration |
| serde / serde_json | Serialization |
| mp3lame-encoder | MP3 encoding |

### Project Architecture

```text
rustdaw/
│
├── apps/
│   └── daw/                 # Main application
│
├── crates/
│   ├── dsp/                 # Digital signal processing
│   ├── core-engine/         # Audio engine and rendering
│   ├── native-plugins/      # Built-in synth and sampler
│   ├── plugin-host/         # VST3 plugin hosting
│   ├── project-io/          # Audio, MIDI and project I/O
│   └── gui/                 # Graphical interface
│
├── testplugins/
│   └── testsynth/           # VST3 test instrument
│
├── Cargo.toml
└── README.md
```

---

# 🚀 Building from Source

Want to compile RustDAW yourself?

Excellent. Either you're a developer, a curious musician, or someone who enjoys watching compilation logs more than sleeping.

This guide covers Windows

## 1. Requirements

Before building, install the following tools.

### Required tools

- Rust toolchain (stable).
- Cargo (included with Rust).
- Git.
- A C/C++ compiler and linker.
- Platform-specific audio and GUI development libraries.

### Install Rust

Download Rust using the official installer:

https://rustup.rs/

Verify the installation:

```bash
rustc --version
cargo --version
```

Both commands should return version information.

If they don't, congratulations: you've found your first build problem.

---

## 2. Clone the Repository

Open a terminal and run:

```bash
git clone https://github.com/YOUR_USERNAME/rustdaw.git
```

Enter the project directory:

```bash
cd rustdaw
```

Make sure you're in the directory containing the main `Cargo.toml` file.

The workspace root is important. Running Cargo from the wrong directory is an excellent way to waste ten minutes.

---

# 🪟 Building on Windows

## 3. Install Microsoft C++ Build Tools

RustDAW requires the Microsoft C++ toolchain for native dependencies.

Download Visual Studio Build Tools:

https://visualstudio.microsoft.com/visual-cpp-build-tools/

During installation, select:

- Desktop development with C++
- MSVC C++ build tools
- Windows SDK
- C++ build tools compatible with your installed Rust toolchain

Install everything and restart your terminal.

## 4. Verify the Rust Target

Run:

```powershell
rustup show
```

For a standard 64-bit Windows build, the target should be:

```text
x86_64-pc-windows-msvc
```

If necessary, install it:

```powershell
rustup target add x86_64-pc-windows-msvc
```

## 5. Build RustDAW

Navigate to the project root and execute:

```powershell
cargo build --release -p daw
```

Cargo will download dependencies, compile the workspace components, and build the application.

The first compilation may take a while.

Don't panic if your CPU starts sounding like a jet engine. That's just Rust doing its thing.

## 6. Run RustDAW

After a successful build:

```powershell
cargo run --release -p daw
```

Alternatively, launch the executable directly:

```powershell
.\target\release\daw.exe
```

Your DAW should now be running.

If it doesn't start, check your audio device configuration and installed runtime dependencies.

---


Once RustDAW is running:

1. Configure your audio device.
2. Create and organize tracks.
3. Add MIDI or audio clips to the playlist.
4. Open the Piano Roll to create MIDI sequences.
5. Select a built-in instrument or a supported VST3 plugin.
6. Add insert effects to your tracks.
7. Use the mixer controls to balance your project.
8. Add samples through the library browser.
9. Export your finished composition to WAV or MP3.

Make something beautiful.

Or make something so aggressively experimental that even your audio interface files a complaint.

---

# ⚠️ Known Considerations

- VST3 editor windows are implemented for Windows.
- Audio device availability depends on the host operating system and installed drivers.
- Native audio and GUI dependencies must be installed before compilation.
- VST3 integration tests require the corresponding test plugin bundles.
- macOS build support has not been established by the supplied project configuration.
- MP3 export uses LAME through the `mp3lame-encoder` crate.

### Licensing Notice

The Rust workspace declares:

```text
MIT OR Apache-2.0
```

However, MP3 export uses LAME, which is LGPL-licensed.

If you distribute compiled binaries, review the applicable LGPL obligations and the dependencies' licensing terms. Alternatively, consider building without MP3 export.

Legal paperwork: the final boss of open-source development.

---

# 🤝 Contributing

Contributions, bug reports, suggestions, and improvements are welcome.

If you want to contribute:

1. Fork the repository.
2. Create a feature branch.
3. Implement your changes.
4. Run the workspace tests.
5. Submit a pull request.

Please include useful descriptions and reproduction steps when reporting bugs.

Remember: a bug report saying "it doesn't work" is about as useful as a MIDI controller with no MIDI.

---

# 📜 License

The Rust workspace is licensed under either:

- MIT License
- Apache License 2.0

See the relevant license files and third-party dependency licenses before redistribution.

---

# 💀 Final Words

RustDAW is an open-source attempt to make music production accessible through Rust, modular software architecture, and a little bit of stubbornness.

No subscription fees. No unnecessary cloud accounts. No artificial limitations imposed by a corporation that thinks a compressor plugin should cost as much as rent.

Just code, audio, plugins, and the eternal struggle against latency.

**Happy producing. May your buffers never underrun and your projects never corrupt.**
