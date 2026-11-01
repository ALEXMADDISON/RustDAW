//! Real-time engine: tracks (instrument + 4 effect slots each), MIDI/audio clips,
//! lock-free messaging, offline export.
//!
//! Threading contract:
//!   * GUI thread owns [`EngineHandle`] and talks to the audio thread ONLY via ring buffers
//!     (plus lock-free atomic meters).
//!   * Audio thread owns [`Engine`]; `Engine::process` never allocates, blocks, or does I/O.
pub mod automation;
pub mod clips;
pub mod engine;
pub mod messages;
pub mod model;
pub mod tools;

pub use automation::{eval_points, owned_value, AutoPoint, AutoTarget, RtAuto, Shape};
pub use clips::{AudioParams, ClipList, RtClip, RtClipKind, MAX_TRACKS};
pub use engine::{create_engine, share_engine, Engine, EngineHandle, Meters, FX_SLOTS, MAX_BLOCK};
pub use messages::{Command, Garbage, InstrumentKind};
pub use model::{Note, PPQ};
