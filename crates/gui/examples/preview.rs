//! Runs the GUI without an audio device (for screenshots / UI development).
//! `cargo run -p gui --example preview -- kick.wav snare.wav`
fn main() {
    let (engine, mut handle) = core_engine::create_engine(48_000.0);
    let _shared = core_engine::share_engine(engine, &mut handle);
    let files = std::env::args().skip(1).map(std::path::PathBuf::from).collect();
    gui::run_with(handle, 48_000.0, None, files).unwrap();
}
