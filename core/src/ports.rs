//! Ports: traits the core depends on and the shell implements. Real in
//! production, faked in tests (ADR-0001, `CONTEXT.md`).
//!
//! Issue #4 introduces the first of the four ports named in the spec
//! (issue #1): [`Synth`]. The others (`MidiInput`, `AudioOutput`, `Storage`)
//! arrive with the tickets that need them (#6, #4/#6, #13) rather than as
//! placeholders here.

/// Identifies an instrument to sound, without the trait knowing what
/// instruments exist. The spec (issue #1, story 77) wants a third instrument
/// addable later "without restructuring the audio engine" — an enum baked
/// into this trait would fail that the moment a second instrument arrived.
/// An opaque id makes adding one a data change (a new id, a new SoundFont
/// preset mapping) rather than a trait change. The shell maps the initial
/// piano and accordion ids to bundled SoundFont presets; future ids extend
/// that mapping.
pub type InstrumentId = u32;

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
