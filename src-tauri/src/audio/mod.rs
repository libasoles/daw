//! The audio engine: turns the [`daw_core::ports::Synth`] port into sound
//! actually reaching the speakers.
//!
//! ## Real-time safety
//!
//! `cpal` invokes its render callback on a dedicated **real-time audio
//! thread**, where blocking and heap allocation are forbidden (issue #1,
//! "Threading and real-time safety"). `DawCore` must never run there, and
//! neither must anything in [`synth::RustySynth`] that could block or
//! allocate — `rustysynth`'s rendering is not documented as allocation-free,
//! so it cannot run on that thread either. The engine is split in two:
//!
//! - The **synth thread** (`engine::spawn_synth_thread`) is a plain,
//!   non-real-time OS thread. It owns the `Synth` implementation, drains a
//!   queue of note on/off requests, renders audio in fixed-size chunks, and
//!   pushes the rendered samples into a second queue.
//! - The **`cpal` callback**, which does run on the real-time thread, only
//!   pops already-rendered samples from that second queue and copies them
//!   into the output buffer. No lock, no allocation, no call into `Synth` or
//!   `DawCore` — it only reads and renders, per the spec.
//!
//! Both queues are [`rtrb::RingBuffer`]s: a single-producer/single-consumer
//! ring buffer that allocates its fixed capacity once, on construction, and
//! never again. `push`/`pop` are documented as wait-free and non-blocking —
//! returning `Err` rather than blocking when the buffer is empty or full —
//! which is exactly the property the real-time thread needs and a general
//! channel (e.g. `std::sync::mpsc`, which allocates a node per send) does
//! not guarantee. `rtrb` is single-producer/single-consumer, which matches
//! this design exactly: one command producer (the IPC thread), one command
//! consumer (the synth thread); one sample producer (the synth thread), one
//! sample consumer (the `cpal` callback).
//!
//! There is no timeline yet (that arrives in H3), so the only "schedule" this
//! ticket plumbs through is a single note on/off request, triggered by a
//! debug command for manual verification (see `play_test_note` in
//! `src-tauri/src/lib.rs`). [`EngineCommand`] is the extension point: later
//! tickets (#6 live MIDI-through, #8 recording/playback) add variants there
//! (e.g. a schedule snapshot to render from) without restructuring the
//! thread split or the queues.
//!
//! ## Why the `cpal::Stream` never touches Tauri-managed state
//!
//! `cpal::Stream` is not `Send` on every backend (it wraps platform handles
//! that are only valid on the thread that created them), so it cannot be
//! stored in `tauri::State`, which requires `Send + Sync`. Instead,
//! [`AudioEngine::start`] spawns a dedicated thread that builds the stream,
//! starts it playing, and then parks itself forever purely to keep the
//! stream alive — the actual audio work happens on the synth thread and
//! inside the callback, not on this parking thread. Only the lock-free
//! command producer (safe to share behind a `Mutex`, since pushing is
//! wait-free and cheap) is handed back to the caller.

mod engine;
mod synth;

// Only `AudioEngine` is consumed outside this module today (from
// `src-tauri/src/lib.rs`). `EngineCommand`, `AudioEngineError`, `RustySynth`
// and `RustySynthError` stay module-private but are still real, used types —
// `engine.rs` and `synth.rs` construct and match on them internally, and
// `AudioEngine::start`'s `Result` surfaces `AudioEngineError` through this
// module's one public type without needing the error type itself exported.
pub use engine::AudioEngine;
