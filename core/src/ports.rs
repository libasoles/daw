//! Ports: traits the core depends on and the shell implements. Real in
//! production, faked in tests (ADR-0001, `CONTEXT.md`).
//!
//! The concrete adapters live in the shell; the small scripted MIDI adapter
//! below exists so integration tests can exercise the same contract without
//! a keyboard or an operating system MIDI service.

use std::collections::BTreeMap;
use std::fmt;
use std::thread;
use std::time::{Duration, Instant};

/// Identifies an instrument to sound, without the trait knowing what
/// instruments exist. The spec (issue #1, story 77) wants a third instrument
/// addable later "without restructuring the audio engine" — an enum baked
/// into this trait would fail that the moment a second instrument arrived.
/// An opaque id makes adding one a data change (a new id, a new SoundFont
/// preset mapping) rather than a trait change. The shell maps the initial
/// piano and accordion ids to bundled SoundFont presets; future ids extend
/// that mapping.
pub type InstrumentId = u32;

/// Project-document storage, kept behind a port so the musical core never
/// knows filesystem paths or native dialogs (issue #13).
///
/// The crash-recovery snapshot (issue #15) is addressed by its own
/// `*_snapshot` methods, entirely separate from `save`/`load`/`list`'s named
/// project documents — the spec requires the snapshot to be "a separate
/// document" that "never touches a saved project file", so an implementation
/// must back it with distinct storage, never a name a real project could
/// collide with.
pub trait Storage: Send {
    fn save(&mut self, name: &str, document: String) -> Result<(), String>;
    fn load(&self, name: &str) -> Result<Option<String>, String>;
    fn list(&self) -> Result<Vec<String>, String>;

    fn save_snapshot(&mut self, document: String) -> Result<(), String>;
    fn load_snapshot(&self) -> Result<Option<String>, String>;
    fn delete_snapshot(&mut self) -> Result<(), String>;
}

/// In-memory storage adapter for deterministic project round-trip tests.
/// The snapshot lives in its own field, never in `documents`, so a test
/// mistake that tried to reach it by project name would fail loudly rather
/// than silently reading the wrong thing.
#[derive(Default)]
pub struct MemoryStorage {
    documents: BTreeMap<String, String>,
    snapshot: Option<String>,
}

impl Storage for MemoryStorage {
    fn save(&mut self, name: &str, document: String) -> Result<(), String> {
        self.documents.insert(name.into(), document);
        Ok(())
    }

    fn load(&self, name: &str) -> Result<Option<String>, String> {
        Ok(self.documents.get(name).cloned())
    }

    fn list(&self) -> Result<Vec<String>, String> {
        Ok(self.documents.keys().cloned().collect())
    }

    fn save_snapshot(&mut self, document: String) -> Result<(), String> {
        self.snapshot = Some(document);
        Ok(())
    }

    fn load_snapshot(&self) -> Result<Option<String>, String> {
        Ok(self.snapshot.clone())
    }

    fn delete_snapshot(&mut self) -> Result<(), String> {
        self.snapshot = None;
        Ok(())
    }
}

/// A MIDI input exposed to a musician. `id` is an opaque, shell-provided
/// identifier suitable for saving as an application preference; `name` is
/// only for display.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MidiDevice {
    pub id: String,
    pub name: String,
}

/// A note event observed at a MIDI input. `timestamp` is monotonic elapsed
/// time since that input subscription began, rather than wall-clock time, so
/// it is safe to compare in tests and useful for latency instrumentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiEvent {
    pub timestamp: Duration,
    pub kind: MidiEventKind,
}

/// The MIDI messages that live playing needs. Other MIDI messages remain
/// outside the core until a musical feature requires them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MidiEventKind {
    NoteOn { pitch: u8, velocity: u8 },
    NoteOff { pitch: u8 },
}

/// Failure to select a MIDI input. Kept as text so platform adapters can
/// preserve useful native error detail without importing platform error types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiInputError(pub String);

impl fmt::Display for MidiInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MidiInputError {}

/// Callback invoked by a subscribed MIDI input.
pub type MidiEventHandler = Box<dyn Fn(MidiEvent) + Send + 'static>;

/// The native MIDI boundary. It deliberately deals in device enumeration and
/// timestamped note messages, never raw bytes or a browser MIDI API.
pub trait MidiInput: Send {
    fn devices(&self) -> Result<Vec<MidiDevice>, MidiInputError>;
    fn subscribe(
        &mut self,
        device_id: &str,
        handler: MidiEventHandler,
    ) -> Result<(), MidiInputError>;
}

/// A dependency-free MIDI fake that replays a fixed script at its recorded
/// offsets. It is public because tests outside this crate must use the port's
/// public surface (ADR-0001).
pub struct ScriptedMidiInput {
    device: MidiDevice,
    events: Vec<MidiEvent>,
}

impl ScriptedMidiInput {
    pub fn new(device: MidiDevice, events: Vec<MidiEvent>) -> Self {
        Self { device, events }
    }
}

impl MidiInput for ScriptedMidiInput {
    fn devices(&self) -> Result<Vec<MidiDevice>, MidiInputError> {
        Ok(vec![self.device.clone()])
    }

    fn subscribe(
        &mut self,
        device_id: &str,
        handler: MidiEventHandler,
    ) -> Result<(), MidiInputError> {
        if device_id != self.device.id {
            return Err(MidiInputError(format!(
                "MIDI device not found: {device_id}"
            )));
        }

        let events = self.events.clone();
        thread::spawn(move || {
            let started = Instant::now();
            for event in events {
                if let Some(wait) = event.timestamp.checked_sub(started.elapsed()) {
                    thread::sleep(wait);
                }
                handler(event);
            }
        });
        Ok(())
    }
}

/// The adapter boundary that keeps the sound engine replaceable. Swapping
/// `rustysynth` for a different synthesiser is a new implementation of this
/// trait, not a restructuring of `daw-core`.
///
/// `daw-core` depends only on this trait, never on `rustysynth` or `cpal`
/// directly (ADR-0001): the real, SoundFont-backed implementation lives in
/// `src-tauri`, the only crate allowed to know about audio hardware.
///
/// `pulse` is threaded through `note_on`/`note_off` (rather than being
/// implicit "now") because the spec's testing decisions call for a fake that
/// "records which notes it was asked to sound and at which pulse" — that is
/// how a later sequencer's output is asserted on without audio hardware. The
/// real-time `rustysynth` adapter ignores it today (there is no timeline
/// yet); once one exists, it is what turns a schedule into note events
/// without changing this trait's shape.
///
/// `note_on`/`note_off`/`render` take `&mut self` and are meant to be called
/// off the hard real-time audio thread — see `src-tauri`'s `audio` module for
/// where the real-time boundary actually is. This trait itself carries no
/// threading requirement beyond `Send`, since ownership (not `Sync`) is what
/// crossing to a dedicated synth thread requires.
pub trait Synth: Send {
    /// Sets the global reverb send as a percentage from 0 (dry) to 100.
    /// Like note requests, this is called off the hard real-time callback.
    fn set_reverb(&mut self, reverb: u8);

    /// Starts sounding `pitch` (a MIDI note number) on `instrument` at
    /// `velocity`, timestamped at `pulse` for tests/scheduling purposes.
    fn note_on(&mut self, instrument: InstrumentId, pitch: u8, velocity: u8, pulse: u64);

    /// Stops sounding `pitch` on `instrument`, timestamped at `pulse`.
    fn note_off(&mut self, instrument: InstrumentId, pitch: u8, pulse: u64);

    /// Renders the next block of audio into `left`/`right`, one sample per
    /// output frame per channel. Buffers are pre-allocated by the caller;
    /// implementations must not allocate here so this can be called from a
    /// context with real-time constraints.
    fn render(&mut self, left: &mut [f32], right: &mut [f32]);
}
