//! The musical core of the daw.
//!
//! Everything testable in this project lives behind one seam:
//!
//! ```text
//! Command -> DawCore -> (ProjectState, Vec<Effect>)
//! ```
//!
//! `DawCore` is a pure, in-memory state machine. It performs no I/O: anything
//! the outside world must do — schedule audio, write a file, raise a prompt —
//! comes back out as an [`Effect`] for the shell to carry out. It knows nothing
//! about Tauri, webviews, MIDI hardware or audio devices; those live behind
//! ports in the binary crate.
//!
//! Issue #3 establishes the seam for real: the `Command` vocabulary, the
//! `ProjectState` shape and a command-log undo/redo. Later tickets add
//! variants to `Command` and `Effect` (recording, the library, the timeline,
//! persistence) without restructuring either.

#![forbid(unsafe_code)]

use std::collections::{BTreeSet, VecDeque};
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

mod midi;
pub mod ports;

use ports::InstrumentId;

/// Everything about a piece of music that gets persisted.
///
/// This is exactly the structure serialised to `project.json` and
/// `recovery.json`: what the tests assert on is what gets saved, so there is no
/// second representation to drift. `Serialize`/`Deserialize` are derived now
/// (rather than deferred to the persistence ticket, #13) because the Tauri
/// command that returns this across the IPC boundary needs it today, and
/// deriving it costs nothing — the shape doesn't change either way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectState {
    /// Whether this project differs from the most recent manual save. This is
    /// runtime state for the shell, never part of a project document.
    #[serde(default, skip_deserializing)]
    pub is_dirty: bool,
    /// The global pulse, in beats per minute.
    pub bpm: u16,
    /// Beats per bar and the beat unit, e.g. `(3, 4)`.
    ///
    /// The time signature has no structural power: it governs the metronome's
    /// accent, the length of the count-in and a purely visual grid accent.
    pub time_signature: (u8, u8),
    /// The instrument used globally for live playing and playback.
    pub instrument: InstrumentId,
    /// Global reverb send, expressed as a percentage from 0 (dry) to 100.
    ///
    /// This is deliberately project state but not undoable: adjusting a
    /// continuous sound control must not displace musical actions from undo.
    pub reverb: u8,
    /// Whether the audible pulse is sounding.
    ///
    /// Like reverb, this is a performance preference stored with the project
    /// rather than a musical edit in its undo history.
    pub metronome_enabled: bool,
    /// Whether recording begins with one count-in bar.
    ///
    /// This has no structural effect on notes; it only decides whether a
    /// future recording action waits for [`count_in_length_in_pulses`].
    pub count_in_enabled: bool,
    /// The recording area's one take, if anything has been recorded yet.
    /// `None` until the first `StopRecording`.
    pub take: Option<Take>,
    /// Frozen blocks held by the library.
    pub blocks: Vec<Block>,
    pub next_block_id: u64,
    /// Blocks placed on the timeline (issue #17).
    pub placements: Vec<Placement>,
    pub next_placement_id: u64,
    /// Whether timeline playback (issue #18) restarts from the beginning
    /// when it reaches the end (issue #19), off by default. A performance
    /// preference like `metronome_enabled`, not a musical edit — it lives
    /// in project state (and is saved with it) but never enters undo
    /// history.
    pub loop_enabled: bool,
    /// Whether the shell is currently capturing live MIDI into a take.
    /// A performance/session flag, not a musical edit — see
    /// [`Command::StartRecording`].
    pub is_recording: bool,
    /// Whether the shell is currently sounding a schedule (today, only the
    /// take's own isolated playback — see [`Command::PlayTake`]). The record
    /// button is disabled while this is true, per the spec.
    pub is_playing: bool,
}

/// A single note exactly as played, captured during recording. Pulse
/// offsets, never wall-clock time or bars (CONTEXT.md's "Pulse").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedNote {
    pub pitch: u8,
    pub velocity: u8,
    pub start_pulse: u64,
    pub end_pulse: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub id: u64,
    pub name: String,
    pub color: String,
    pub instrument: InstrumentId,
    pub notes: Vec<RecordedNote>,
}

const BLOCK_COLORS: [&str; 10] = [
    "#f6cf4a", "#da4e91", "#ec756f", "#f39b4b", "#6561df", "#6c9ce8", "#4794b8", "#68cdbc",
    "#77d987", "#bc4bd8",
];

/// A block placed on the timeline (CONTEXT.md's "Placement"): an
/// **independent copy** of the block's notes, name, colour and instrument,
/// not a reference. Blocks are immutable, so a copy behaves identically to a
/// reference during playback: renaming or recolouring a block (#12) never
/// retroactively changes a placement already made from it.
///
/// `track` and `start_pulse` are stored explicitly — rather than, say, the
/// placement's index in an ordered list standing in for its position — so
/// that adding a second track later (per the spec) is a data change, not a
/// rewrite of how placements are addressed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    pub id: u64,
    /// The block this placement was copied from, kept only so deleting that
    /// block (#23) can find every placement it produced — never consulted
    /// to re-read the block's *current* name, colour or notes, which would
    /// defeat the whole point of an independent copy. `#[serde(default)]`
    /// so a document saved before this field existed still opens; such a
    /// placement simply never matches a real block and so never
    /// participates in a future delete-in-use cascade.
    #[serde(default)]
    pub block_id: u64,
    pub track: u32,
    pub start_pulse: u64,
    pub name: String,
    pub color: String,
    pub instrument: InstrumentId,
    pub notes: Vec<RecordedNote>,
    /// A deliberate silence, in pulses, held before this placement (issue
    /// #22). Stored on the placement that follows it, not inferred from
    /// coordinates, so it survives ripple edits — inserting, reordering or
    /// deleting placements elsewhere never touches this field.
    /// `#[serde(default)]` so a document saved before this field existed
    /// still opens, as a gap of zero.
    #[serde(default)]
    pub gap_before: u64,
}

impl Placement {
    /// How many pulses this placement spans, from its own start. Used to
    /// find where the *next* placement on a track lands flush.
    fn length(&self) -> u64 {
        self.notes
            .iter()
            .map(|note| note.end_pulse)
            .max()
            .unwrap_or(0)
    }

    fn end_pulse(&self) -> u64 {
        self.start_pulse + self.length()
    }
}

/// Recomputes every placement's `start_pulse` in `order` sequentially from
/// pulse 0, honouring each placement's own `gap_before` (issue #22) — with
/// every `gap_before` at zero they sit flush with no gaps and no overlap
/// (CONTEXT.md's "Ripple"), the shared reflow step behind inserting-with-
/// push, reordering (issue #20) and deleting (#21). `order` is every
/// placement on one track, already arranged in the order they should end up
/// in; each placement starts exactly `gap_before` pulses after the last one
/// ended, so a deliberate gap set on one placement holds regardless of what
/// moves elsewhere on the track.
fn reflow(order: &mut [Placement]) {
    let mut cursor = 0u64;
    for placement in order.iter_mut() {
        cursor += placement.gap_before;
        placement.start_pulse = cursor;
        cursor += placement.length();
    }
}

/// Removes `block_id`'s block and every placement copied from it (issue
/// #23), reflowing only the tracks that actually lost a placement so
/// deliberate gaps elsewhere are left exactly as they were. Shared between
/// `DeleteBlock`'s own handler (which needs to know what it touched, to log
/// as an undo inverse) and `RemoveBlockAndPlacements`'s replay on redo,
/// which discards the return value.
///
/// Returns the removed block together with a *complete* pre-removal
/// snapshot of every placement on every track that lost one — not just the
/// removed placements themselves. A track's surviving placements ripple
/// earlier to close the gap, so undoing this has to put the whole track
/// back, exactly as `Command::SetTrackPlacements` already does for insert,
/// reorder and delete; restoring only the removed items would leave the
/// survivors stranded at their post-ripple positions. Returns `None` if no
/// block has `block_id`.
fn remove_block_and_its_placements(
    state: &mut ProjectState,
    block_id: u64,
) -> Option<(Block, Vec<Placement>)> {
    let index = state.blocks.iter().position(|block| block.id == block_id)?;
    let block = state.blocks.remove(index);

    let affected_tracks: BTreeSet<u32> = state
        .placements
        .iter()
        .filter(|placement| placement.block_id == block_id)
        .map(|placement| placement.track)
        .collect();

    let mut before_snapshot = Vec::new();
    for track in affected_tracks {
        let mut ordered: Vec<Placement> = state
            .placements
            .iter()
            .filter(|placement| placement.track == track)
            .cloned()
            .collect();
        before_snapshot.extend(ordered.iter().cloned());

        ordered.retain(|placement| placement.block_id != block_id);
        ordered.sort_by_key(|placement| placement.start_pulse);
        reflow(&mut ordered);

        state
            .placements
            .retain(|placement| placement.track != track);
        state.placements.extend(ordered);
    }

    Some((block, before_snapshot))
}

/// A recording in the recording area (CONTEXT.md's "Take"): the raw notes
/// exactly as played. There is at most one at a time, and it is never
/// rewritten by any later edit — trimming and quantising (issues #9, #10)
/// are views applied on read, not mutations of this data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trim {
    pub start_pulse: u64,
    pub end_pulse: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Quantisation {
    #[default]
    Off,
    Whole,
    Half,
    Quarter,
    Eighth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Take {
    /// The MIDI capture, retained exactly as it was recorded.
    pub raw_notes: Vec<RecordedNote>,
    pub trim: Trim,
    pub quantisation: Quantisation,
}

impl Take {
    pub fn from_raw_notes(raw_notes: Vec<RecordedNote>) -> Self {
        let end_pulse = raw_notes
            .iter()
            .map(|note| note.end_pulse)
            .max()
            .unwrap_or(0);
        Self {
            raw_notes,
            trim: Trim {
                start_pulse: 0,
                end_pulse,
            },
            quantisation: Quantisation::Off,
        }
    }

    /// Applies the take's editable views every time it is read. Captured MIDI
    /// remains the sole stored source of notes.
    pub fn notes(&self) -> Vec<RecordedNote> {
        self.raw_notes
            .iter()
            .filter_map(|note| {
                // Quantise the capture first, then apply the trim. This keeps
                // either edit order equivalent and means a trimmed edge is
                // always an exact audible boundary.
                let start_pulse =
                    quantise_pulse(note.start_pulse, self.quantisation).max(self.trim.start_pulse);
                let end_pulse =
                    quantise_pulse(note.end_pulse, self.quantisation).min(self.trim.end_pulse);
                (start_pulse < end_pulse).then(|| RecordedNote {
                    start_pulse,
                    end_pulse,
                    ..note.clone()
                })
            })
            .filter(|note| note.start_pulse < note.end_pulse)
            .collect()
    }

    fn set_trim(&mut self, trim: Trim) {
        let raw_end = self
            .raw_notes
            .iter()
            .map(|note| note.end_pulse)
            .max()
            .unwrap_or(0);
        let start_pulse = trim.start_pulse.min(raw_end);
        self.trim = Trim {
            start_pulse,
            end_pulse: trim.end_pulse.min(raw_end).max(start_pulse),
        };
    }

    fn set_quantisation(&mut self, quantisation: Quantisation) {
        self.quantisation = quantisation;
    }
}

#[derive(Serialize, Deserialize)]
struct TakeWire {
    raw_notes: Vec<RecordedNote>,
    /// Sent to the shell as a read-only rendered view; ignored when loading
    /// so it can never become a second source of truth.
    #[serde(default)]
    notes: Vec<RecordedNote>,
    trim: Trim,
    quantisation: Quantisation,
}

impl Serialize for Take {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        TakeWire {
            raw_notes: self.raw_notes.clone(),
            notes: self.notes(),
            trim: self.trim,
            quantisation: self.quantisation,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Take {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = TakeWire::deserialize(deserializer)?;
        let _ = wire.notes;
        Ok(Self {
            raw_notes: wire.raw_notes,
            trim: wire.trim,
            quantisation: wire.quantisation,
        })
    }
}

impl Default for Take {
    fn default() -> Self {
        Self::from_raw_notes(Vec::new())
    }
}

fn quantise_pulse(pulse: u64, quantisation: Quantisation) -> u64 {
    let grid = match quantisation {
        Quantisation::Off => return pulse,
        Quantisation::Whole => 8,
        Quantisation::Half => 4,
        Quantisation::Quarter => 2,
        Quantisation::Eighth => 1,
    };
    (pulse + grid / 2) / grid * grid
}

/// Stored pulses are eighth-note subdivisions. This preserves exact integer
/// offsets for every grid the editor offers while tempo remains beat-based.
pub const PULSES_PER_BEAT: u64 = 2;

impl Default for ProjectState {
    fn default() -> Self {
        Self {
            is_dirty: false,
            bpm: 120,
            time_signature: (3, 4),
            instrument: 0,
            reverb: 0,
            metronome_enabled: false,
            count_in_enabled: true,
            take: None,
            blocks: Vec::new(),
            next_block_id: 1,
            placements: Vec::new(),
            next_placement_id: 1,
            loop_enabled: false,
            is_recording: false,
            is_playing: false,
        }
    }
}

/// The complete vocabulary of things a user can do, as described in the spec
/// (issue #1). Only the variants needed to make undo/redo, project state and
/// effects real and testable now are implemented; recording, MIDI, the
/// library, the timeline and persistence arrive in later tickets as new
/// variants, not as a restructuring of this enum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "camelCase")]
pub enum Command {
    /// Opens a new, empty project. Not itself undoable: it resets the state
    /// wholesale and clears the undo/redo history.
    ///
    /// If the current project has unsaved changes and `force` is `false`,
    /// this reports [`Effect::ConfirmDiscardUnsavedChanges`] and leaves the
    /// project untouched, per the spec's warning before discarding work.
    /// Resending with `force: true` (after the shell has confirmed with the
    /// user, or saved on their behalf) proceeds unconditionally.
    NewProject {
        force: bool,
    },
    /// Replaces the current project with `document`, read by the shell
    /// through its storage port. Opening never enters undo history.
    ///
    /// Gated by unsaved changes exactly like [`Command::NewProject`]: with
    /// `force: false` and a dirty project, this reports
    /// [`Effect::ConfirmDiscardUnsavedChanges`] and leaves the current
    /// project untouched.
    OpenProject {
        document: ProjectState,
        force: bool,
    },
    /// Restores a session from a crash-recovery snapshot (issue #15).
    /// Unlike [`Command::OpenProject`], this never establishes a new saved
    /// baseline: the snapshot was written while there were unsaved changes
    /// and was never itself a manual save, so the restored session must
    /// still report `is_dirty`, exactly as it was when the snapshot was
    /// taken, prompting the user to save for real. Not undoable, and — like
    /// `OpenProject` — clears undo/redo history. Never gated: recovery only
    /// ever happens once, immediately at launch, before there is any current
    /// project whose unsaved changes could be at risk.
    RecoverProject(ProjectState),
    /// Requests a manual save under an application-owned project name. The
    /// core emits [`Effect::SaveProject`] for the shell's storage adapter.
    SaveProject(String),
    /// Records that the shell successfully wrote this exact document. This
    /// is not undoable: it changes the save baseline, not musical content.
    ProjectSaved(ProjectState),
    /// Sets the global tempo in beats per minute.
    SetBpm(u16),
    /// Sets the project's time signature (beats per bar, beat unit).
    SetTimeSignature {
        beats_per_bar: u8,
        beat_unit: u8,
    },
    /// Selects the global instrument using the opaque id understood by the
    /// synth port.
    SetInstrument(InstrumentId),
    /// Sets the global reverb send as a percentage from 0 (dry) to 100.
    /// This command is intentionally excluded from undo/redo history.
    SetReverb(u8),
    /// Switches the audible pulse on or off. This is intentionally excluded
    /// from undo/redo history, like [`Command::SetReverb`].
    SetMetronomeEnabled(bool),
    /// Switches the one-bar count-in on or off. This is intentionally excluded
    /// from undo/redo history, like [`Command::SetReverb`].
    SetCountInEnabled(bool),
    /// Switches timeline looping (issue #19) on or off. Takes effect on the
    /// current playback pass, if any — the shell reads this fresh each time
    /// playback would otherwise end. This is intentionally excluded from
    /// undo/redo history, like [`Command::SetReverb`].
    SetLoopEnabled(bool),
    /// Arms recording. Not itself undoable — arming isn't a musical edit, the
    /// take that results from it is (see [`Command::StopRecording`]).
    ///
    /// Silently replaces any take already in the recording area — recording
    /// over one is not a confirmed action, the record button itself is the
    /// commitment.
    StartRecording,
    /// Finishes recording, replacing the recording area's take with `take`.
    /// The shell builds `take` from the raw MIDI it captured — the core
    /// performs no I/O and cannot listen for MIDI itself. Always sent as
    /// `Some` by the shell; `None` only appears as this command's own
    /// undo inverse, when there was no take to revert to. Undoable: reverts
    /// to whatever take (if any) was there before.
    StopRecording(Option<Take>),
    /// Freezes the current take in the library and clears the recording area
    /// so the next recording starts fresh. Undo restores both the removed
    /// block and the take as one edit.
    AddTakeToLibrary,
    /// The replayable forward half of [`Command::AddTakeToLibrary`]. This is
    /// logged internally rather than sent by the shell.
    AddBlockAndClearTake(Block),
    /// The replayable inverse of [`Command::AddBlockAndClearTake`].
    RemoveBlockAndRestoreTake {
        block: Block,
        take: Take,
    },
    InsertBlock(Block),
    RemoveBlock(Block),
    PlayBlock(usize),
    /// Drops `block_id` onto `track` (issue #17): lands as an independent
    /// copy, flush after the existing placements on that track, snapped to
    /// the pulse. A no-op — nothing added, no effect, nothing to undo — if
    /// the block no longer exists. Never overlaps an existing placement by
    /// construction (it always lands at or after the rightmost one on the
    /// track); a real "drop somewhere specific" that could conflict arrives
    /// with #20's insert-with-push.
    AddPlacement {
        block_id: u64,
        track: u32,
    },
    InsertPlacement(Placement),
    RemovePlacement(Placement),
    /// Drops `block_id` onto `track` at `index` within its ordered
    /// placements — 0 is the very start, the track's current placement
    /// count is the very end (matching `AddPlacement`'s flush-append) —
    /// pushing everything from `index` onward later by the new placement's
    /// length (issue #20). A no-op if the block no longer exists.
    InsertPlacementAt {
        block_id: u64,
        track: u32,
        index: usize,
    },
    /// Moves an existing placement to `new_index` within its track's
    /// ordered placements, rippling the rest of the track to stay flush
    /// (issue #20). A no-op if no placement has `id`.
    ReorderPlacement {
        id: u64,
        new_index: usize,
    },
    /// Removes the placement with `id` from the timeline and closes the gap
    /// behind it: the rest of its track ripples earlier so everything stays
    /// flush (issue #21). A no-op if no placement has `id`. Undoable via the
    /// same [`Command::SetTrackPlacements`] snapshot pair as insert/reorder.
    DeletePlacement(u64),
    /// Sets the deliberate silence held before the placement with `id`
    /// (issue #22) — a rest written into the piece, snapped to the pulse
    /// grid by the caller, stored on the placement itself so it survives
    /// insertions, reorders and deletions elsewhere on the track. A no-op
    /// if no placement has `id`. Undoable via the same
    /// [`Command::SetTrackPlacements`] snapshot pair as insert/reorder/
    /// delete.
    SetPlacementGap {
        id: u64,
        gap_before: u64,
    },
    /// Replaces every placement on `track` wholesale. The undo/redo
    /// primitive behind `InsertPlacementAt`/`ReorderPlacement`: rippling a
    /// track moves more than one placement at once, so a single before/
    /// after pair for "this one placement's position" can't capture it —
    /// this logs (and replays) the whole track's placements as one snapshot
    /// instead, exactly as they were.
    SetTrackPlacements {
        track: u32,
        placements: Vec<Placement>,
    },
    /// Plays the whole arrangement from the beginning (issue #18): every
    /// track's placements, flattened into one immutable note stream in a
    /// single [`Effect::PlaySchedule`], exactly like `PlayTake`/`PlayBlock`
    /// already hand the shell a schedule to turn into real audio. Not
    /// undoable. Mutually exclusive with take/block playback through the
    /// same `is_playing` flag — refused (no effect) while anything is
    /// already playing, and a no-op if the timeline holds nothing yet.
    PlayTimeline,
    /// Exports the timeline as a Standard MIDI File (issue #24): the exact
    /// same flattened note stream `PlayTimeline` plays, plus tempo and time
    /// signature, encoded as bytes for the shell to write wherever the user
    /// chooses — daw-core performs no file I/O of its own. Not undoable;
    /// export is not a musical edit. Reports [`Effect::NothingToExport`]
    /// rather than [`Effect::ExportedMidi`] if the timeline holds no
    /// placements, so the shell never writes an empty file.
    ExportMidi,
    RenameBlock {
        id: u64,
        name: String,
    },
    RecolourBlock {
        id: u64,
        color: String,
    },
    /// Deletes the block with `id` from the library (issue #23) — the
    /// application's one destructive path across the two areas. A block
    /// with no placements is deleted outright. A block used in the
    /// timeline reports [`Effect::ConfirmDeleteBlockInUse`] and leaves
    /// everything untouched unless `force` is `true`, in which case the
    /// block and every placement copied from it are removed together,
    /// closing the gap each removal leaves on its track — deliberate gaps
    /// elsewhere are untouched, exactly as insert/reorder/delete already
    /// preserve them. The whole thing is a single undo step.
    DeleteBlock {
        id: u64,
        force: bool,
    },
    /// Removes the block with `id` and every placement copied from it,
    /// closing the resulting gaps (issue #23). The redo half of
    /// `DeleteBlock`'s undo pair — deterministic replay of the same
    /// deletion, not itself logged.
    RemoveBlockAndPlacements(u64),
    /// Restores a block and a complete pre-removal snapshot of every
    /// placement on every track `DeleteBlock` touched — not just the
    /// removed placements, but the survivors too, since they rippled to
    /// close the gap and need putting back at their exact previous
    /// positions as well. The undo half of `DeleteBlock`'s pair.
    RestoreBlockAndPlacements {
        block: Block,
        placements: Vec<Placement>,
    },
    SetTakeTrim(Trim),
    SetTakeQuantisation(Quantisation),
    /// Starts isolated playback of the current take, if there is one. Not
    /// undoable — playback isn't a musical edit. Reports
    /// [`Effect::PlaySchedule`] with the take to play; a no-op with no
    /// effect if the recording area is empty.
    PlayTake,
    /// Reported by the shell once a schedule requested by `PlayTake` has
    /// finished sounding. Not undoable.
    PlaybackFinished,
    /// Reverts the most recently applied command. Not itself logged.
    Undo,
    /// Reapplies the most recently undone command. Not itself logged.
    Redo,
}

/// What the core asks the shell to do rather than doing itself, or reports
/// back about a command it could not usefully act on. The core performs no
/// I/O of its own — this is the only channel out.
///
/// At this stage there is no real audio/file/device I/O to trigger yet, so
/// the vocabulary covers the feedback case the spec calls out explicitly:
/// `Undo`/`Redo` against an empty history are no-ops rather than errors, and
/// report that back so the shell can, say, disable the undo button.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Effect {
    /// `Undo` was applied with nothing in the undo log.
    NothingToUndo,
    /// `Redo` was applied with nothing in the redo log.
    NothingToRedo,
    /// The selected MIDI input is unavailable. This is a shell-originated
    /// status, reported through the same effect vocabulary as other feedback.
    NoMidiDeviceAvailable,
    /// `NewProject { force: false }` or `OpenProject { force: false, .. }`
    /// was applied against a project with unsaved changes. The shell should
    /// offer to save, discard or cancel, then resend the same command with
    /// `force: true` if the user chooses to save or discard.
    ConfirmDiscardUnsavedChanges,
    /// `DeleteBlock { force: false, .. }` was applied against a block used
    /// by `uses` placements on the timeline. The shell should ask the user
    /// to confirm, stating how many placements will be removed, then
    /// resend with `force: true` if they agree.
    ConfirmDeleteBlockInUse { uses: usize },
    /// The shell should persist this durable project document under `name`.
    /// It must report success by applying [`Command::ProjectSaved`].
    SaveProject {
        name: String,
        document: ProjectState,
    },
    /// `PlayTake` was applied with a take present: the shell should turn
    /// this into real audio and, once it has finished sounding, apply
    /// `PlaybackFinished`.
    PlaySchedule(Take),
    /// `ExportMidi` was applied against an empty timeline. The shell should
    /// tell the user there is nothing to export rather than opening a save
    /// dialog or writing a file.
    NothingToExport,
    /// `ExportMidi` succeeded: a complete Standard MIDI File, ready to write
    /// exactly as given. The shell should open a native save dialog and
    /// write these bytes wherever the user chooses. A named field, not a
    /// bare newtype, because `#[serde(tag = "type")]`'s internal tagging
    /// can't represent a variant whose payload serialises as a JSON array
    /// rather than an object.
    ExportedMidi { bytes: Vec<u8> },
}

/// The number of applied commands retained in the undo log. The spec (#1,
/// #3) requires at least 50; oldest entries are evicted once the log is full.
pub const HISTORY_CAPACITY: usize = 50;

/// A previously-applied command paired with the command that undoes it.
/// Undo is a command log, not a state snapshot: reverting means applying
/// `inverse`, and redoing means reapplying `forward`.
#[derive(Debug, Clone, PartialEq)]
struct LoggedCommand {
    forward: Command,
    inverse: Command,
}

/// The result of applying a [`Command`]: the resulting project state and any
/// effects for the shell to carry out.
///
/// `state` is an owned clone of the project state rather than a `&ProjectState`
/// borrow of `DawCore`. That is the more idiomatic shape at this seam: a
/// `#[tauri::command]` handler holds a `MutexGuard<DawCore>` only for the
/// duration of the call and needs to hand an owned, `Serialize` value back
/// across IPC once the guard drops. A borrow would tie the return value's
/// lifetime to the guard for no benefit, since `ProjectState` is small and
/// cheap to clone.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Applied {
    pub state: ProjectState,
    pub effects: Vec<Effect>,
}

/// The in-memory state machine every user gesture is funnelled through.
#[derive(Debug)]
pub struct DawCore {
    state: ProjectState,
    saved_state: Option<ProjectState>,
    undo_log: VecDeque<LoggedCommand>,
    redo_log: Vec<LoggedCommand>,
}

impl Default for DawCore {
    fn default() -> Self {
        let mut core = Self {
            state: ProjectState::default(),
            saved_state: None,
            undo_log: VecDeque::new(),
            redo_log: Vec::new(),
        };
        core.sync_dirty_state();
        core
    }
}

impl DawCore {
    /// Opens a new, empty project.
    pub fn new() -> Self {
        Self::default()
    }

    /// The current project, as the interface should render it.
    pub fn state(&self) -> &ProjectState {
        &self.state
    }

    /// Returns the durable document for the current project. Session-only
    /// recording and playback flags are deliberately left out.
    pub fn project_document(&self) -> ProjectState {
        Self::document_state(&self.state)
    }

    /// The single entry point into the core: a command goes in, the new
    /// project state and any effects come out. Performs no I/O.
    pub fn apply(&mut self, command: Command) -> Applied {
        let effects = match command {
            Command::NewProject { force } => {
                if self.state.is_dirty && !force {
                    vec![Effect::ConfirmDiscardUnsavedChanges]
                } else {
                    self.state = ProjectState::default();
                    self.saved_state = None;
                    self.clear_history();
                    Vec::new()
                }
            }
            Command::OpenProject {
                document: mut state,
                force,
            } => {
                if self.state.is_dirty && !force {
                    vec![Effect::ConfirmDiscardUnsavedChanges]
                } else {
                    state.is_recording = false;
                    state.is_playing = false;
                    state.is_dirty = false;
                    self.state = state.clone();
                    self.saved_state = Some(state);
                    self.clear_history();
                    Vec::new()
                }
            }
            Command::RecoverProject(mut state) => {
                state.is_recording = false;
                state.is_playing = false;
                // Deliberately leave `saved_state` untouched (always `None`
                // here, since recovery only ever runs once, right at
                // launch) so `sync_dirty_state` reports the restored
                // session as still unsaved.
                self.state = state;
                self.clear_history();
                Vec::new()
            }
            Command::SaveProject(name) => vec![Effect::SaveProject {
                name,
                document: self.project_document(),
            }],
            Command::ProjectSaved(document) => {
                self.saved_state = Some(Self::document_state(&document));
                Vec::new()
            }
            Command::Undo => self.undo(),
            Command::Redo => self.redo(),
            Command::SetBpm(bpm) => {
                let inverse = Command::SetBpm(self.state.bpm);
                self.state.bpm = bpm;
                self.log(Command::SetBpm(bpm), inverse);
                Vec::new()
            }
            Command::SetTimeSignature {
                beats_per_bar,
                beat_unit,
            } => {
                let inverse = Command::SetTimeSignature {
                    beats_per_bar: self.state.time_signature.0,
                    beat_unit: self.state.time_signature.1,
                };
                self.state.time_signature = (beats_per_bar, beat_unit);
                self.log(
                    Command::SetTimeSignature {
                        beats_per_bar,
                        beat_unit,
                    },
                    inverse,
                );
                Vec::new()
            }
            Command::SetInstrument(instrument) => {
                let inverse = Command::SetInstrument(self.state.instrument);
                self.state.instrument = instrument;
                self.log(Command::SetInstrument(instrument), inverse);
                Vec::new()
            }
            Command::SetReverb(reverb) => {
                self.state.reverb = reverb.min(100);
                // Reverb is a continuous performance control, not a musical
                // edit. In particular it must neither enter nor clear either
                // history so Cmd+Z continues to target musical actions.
                Vec::new()
            }
            Command::SetMetronomeEnabled(enabled) => {
                self.state.metronome_enabled = enabled;
                // The metronome is an immediate performance control, not a
                // musical edit, so it must leave undo/redo aimed at music.
                Vec::new()
            }
            Command::SetCountInEnabled(enabled) => {
                self.state.count_in_enabled = enabled;
                // Count-in changes how a future recording starts; it does
                // not change any recorded material or the undo history.
                Vec::new()
            }
            Command::SetLoopEnabled(enabled) => {
                self.state.loop_enabled = enabled;
                // A performance preference, not a musical edit — leaves
                // undo/redo aimed at music, like reverb/metronome/count-in.
                Vec::new()
            }
            Command::StartRecording => {
                self.state.is_recording = true;
                Vec::new()
            }
            Command::StopRecording(take) => {
                self.state.is_recording = false;
                let inverse = Command::StopRecording(self.state.take.clone());
                self.state.take = take.clone();
                self.log(Command::StopRecording(take), inverse);
                Vec::new()
            }
            Command::AddTakeToLibrary => {
                if let Some(take) = self.state.take.clone() {
                    let block = Block {
                        id: self.state.next_block_id,
                        name: format!("Take {}", self.state.next_block_id),
                        color: BLOCK_COLORS
                            [(self.state.next_block_id as usize - 1) % BLOCK_COLORS.len()]
                        .into(),
                        instrument: self.state.instrument,
                        notes: take.notes(),
                    };
                    self.state.blocks.push(block.clone());
                    self.state.take = None;
                    self.state.next_block_id += 1;
                    self.log(
                        Command::AddBlockAndClearTake(block.clone()),
                        Command::RemoveBlockAndRestoreTake { block, take },
                    );
                }
                Vec::new()
            }
            Command::AddBlockAndClearTake(block) => {
                self.state.blocks.push(block);
                self.state.take = None;
                Vec::new()
            }
            Command::RemoveBlockAndRestoreTake { block, take } => {
                self.state.blocks.retain(|candidate| candidate != &block);
                self.state.take = Some(take);
                Vec::new()
            }
            Command::InsertBlock(block) => {
                self.state.blocks.push(block);
                Vec::new()
            }
            Command::RemoveBlock(block) => {
                self.state.blocks.retain(|candidate| candidate != &block);
                Vec::new()
            }
            Command::RenameBlock { id, name } => {
                if let Some(block) = self.state.blocks.iter_mut().find(|block| block.id == id) {
                    let inverse = Command::RenameBlock {
                        id,
                        name: block.name.clone(),
                    };
                    block.name = name.clone();
                    self.log(Command::RenameBlock { id, name }, inverse);
                }
                Vec::new()
            }
            Command::RecolourBlock { id, color } => {
                if let Some(block) = self.state.blocks.iter_mut().find(|block| block.id == id) {
                    let inverse = Command::RecolourBlock {
                        id,
                        color: block.color.clone(),
                    };
                    block.color = color.clone();
                    self.log(Command::RecolourBlock { id, color }, inverse);
                }
                Vec::new()
            }
            Command::DeleteBlock { id, force } => {
                let uses = self
                    .state
                    .placements
                    .iter()
                    .filter(|placement| placement.block_id == id)
                    .count();
                if uses > 0 && !force {
                    vec![Effect::ConfirmDeleteBlockInUse { uses }]
                } else {
                    if let Some((block, placements)) =
                        remove_block_and_its_placements(&mut self.state, id)
                    {
                        self.log(
                            Command::RemoveBlockAndPlacements(id),
                            Command::RestoreBlockAndPlacements { block, placements },
                        );
                    }
                    Vec::new()
                }
            }
            Command::RemoveBlockAndPlacements(block_id) => {
                remove_block_and_its_placements(&mut self.state, block_id);
                Vec::new()
            }
            Command::RestoreBlockAndPlacements { block, placements } => {
                self.state.blocks.push(block);
                let restored_tracks: BTreeSet<u32> =
                    placements.iter().map(|placement| placement.track).collect();
                self.state
                    .placements
                    .retain(|placement| !restored_tracks.contains(&placement.track));
                self.state.placements.extend(placements);
                Vec::new()
            }
            Command::SetTakeTrim(trim) => {
                if let Some(take) = self.state.take.as_mut() {
                    let inverse = Command::SetTakeTrim(take.trim);
                    take.set_trim(trim);
                    self.log(Command::SetTakeTrim(trim), inverse);
                }
                Vec::new()
            }
            Command::SetTakeQuantisation(quantisation) => {
                if let Some(take) = self.state.take.as_mut() {
                    let inverse = Command::SetTakeQuantisation(take.quantisation);
                    take.set_quantisation(quantisation);
                    self.log(Command::SetTakeQuantisation(quantisation), inverse);
                }
                Vec::new()
            }
            Command::PlayTake => match self.state.take.clone().filter(|_| !self.state.is_playing) {
                Some(take) => {
                    self.state.is_playing = true;
                    vec![Effect::PlaySchedule(take)]
                }
                None => Vec::new(),
            },
            Command::PlayBlock(index) => match self
                .state
                .blocks
                .get(index)
                .filter(|_| !self.state.is_playing)
            {
                Some(block) => {
                    self.state.is_playing = true;
                    vec![Effect::PlaySchedule(Take::from_raw_notes(
                        block.notes.clone(),
                    ))]
                }
                None => Vec::new(),
            },
            Command::PlayTimeline => {
                if self.state.is_playing || self.state.placements.is_empty() {
                    Vec::new()
                } else {
                    self.state.is_playing = true;
                    let notes = midi::flattened_notes(&self.state);
                    vec![Effect::PlaySchedule(Take::from_raw_notes(notes))]
                }
            }
            Command::PlaybackFinished => {
                self.state.is_playing = false;
                Vec::new()
            }
            Command::ExportMidi => {
                if self.state.placements.is_empty() {
                    vec![Effect::NothingToExport]
                } else {
                    vec![Effect::ExportedMidi {
                        bytes: midi::encode(&self.state),
                    }]
                }
            }
            Command::AddPlacement { block_id, track } => {
                if let Some(block) = self.state.blocks.iter().find(|block| block.id == block_id) {
                    // Always lands flush after the rightmost placement on
                    // this track, which by construction can never overlap
                    // an existing one — there is no other way to reach this
                    // command yet (that's #20's "insert with push, and
                    // reorder"), so there is nothing here to reject.
                    let start_pulse = self
                        .state
                        .placements
                        .iter()
                        .filter(|placement| placement.track == track)
                        .map(Placement::end_pulse)
                        .max()
                        .unwrap_or(0);
                    let placement = Placement {
                        id: self.state.next_placement_id,
                        block_id,
                        track,
                        start_pulse,
                        name: block.name.clone(),
                        color: block.color.clone(),
                        instrument: block.instrument,
                        notes: block.notes.clone(),
                        gap_before: 0,
                    };
                    self.state.placements.push(placement.clone());
                    self.state.next_placement_id += 1;
                    self.log(
                        Command::InsertPlacement(placement.clone()),
                        Command::RemovePlacement(placement),
                    );
                }
                Vec::new()
            }
            Command::InsertPlacement(placement) => {
                self.state.placements.push(placement);
                Vec::new()
            }
            Command::RemovePlacement(placement) => {
                self.state
                    .placements
                    .retain(|candidate| candidate != &placement);
                Vec::new()
            }
            Command::InsertPlacementAt {
                block_id,
                track,
                index,
            } => {
                if let Some(block) = self.state.blocks.iter().find(|block| block.id == block_id) {
                    let before: Vec<Placement> = self
                        .state
                        .placements
                        .iter()
                        .filter(|placement| placement.track == track)
                        .cloned()
                        .collect();
                    let mut ordered = before.clone();
                    ordered.sort_by_key(|placement| placement.start_pulse);

                    let new_placement = Placement {
                        id: self.state.next_placement_id,
                        block_id,
                        track,
                        start_pulse: 0,
                        name: block.name.clone(),
                        color: block.color.clone(),
                        instrument: block.instrument,
                        notes: block.notes.clone(),
                        gap_before: 0,
                    };
                    self.state.next_placement_id += 1;
                    ordered.insert(index.min(ordered.len()), new_placement);
                    reflow(&mut ordered);

                    self.state
                        .placements
                        .retain(|placement| placement.track != track);
                    self.state.placements.extend(ordered.clone());
                    self.log(
                        Command::SetTrackPlacements {
                            track,
                            placements: ordered,
                        },
                        Command::SetTrackPlacements {
                            track,
                            placements: before,
                        },
                    );
                }
                Vec::new()
            }
            Command::ReorderPlacement { id, new_index } => {
                if let Some(track) = self
                    .state
                    .placements
                    .iter()
                    .find(|placement| placement.id == id)
                    .map(|placement| placement.track)
                {
                    let before: Vec<Placement> = self
                        .state
                        .placements
                        .iter()
                        .filter(|placement| placement.track == track)
                        .cloned()
                        .collect();
                    let mut ordered = before.clone();
                    ordered.sort_by_key(|placement| placement.start_pulse);

                    let current_index = ordered
                        .iter()
                        .position(|placement| placement.id == id)
                        .expect("id was just found on this track");
                    let moved = ordered.remove(current_index);
                    ordered.insert(new_index.min(ordered.len()), moved);
                    reflow(&mut ordered);

                    self.state
                        .placements
                        .retain(|placement| placement.track != track);
                    self.state.placements.extend(ordered.clone());
                    self.log(
                        Command::SetTrackPlacements {
                            track,
                            placements: ordered,
                        },
                        Command::SetTrackPlacements {
                            track,
                            placements: before,
                        },
                    );
                }
                Vec::new()
            }
            Command::DeletePlacement(id) => {
                if let Some(track) = self
                    .state
                    .placements
                    .iter()
                    .find(|placement| placement.id == id)
                    .map(|placement| placement.track)
                {
                    let before: Vec<Placement> = self
                        .state
                        .placements
                        .iter()
                        .filter(|placement| placement.track == track)
                        .cloned()
                        .collect();
                    let mut ordered = before.clone();
                    ordered.sort_by_key(|placement| placement.start_pulse);
                    ordered.retain(|placement| placement.id != id);
                    reflow(&mut ordered);

                    self.state
                        .placements
                        .retain(|placement| placement.track != track);
                    self.state.placements.extend(ordered.clone());
                    self.log(
                        Command::SetTrackPlacements {
                            track,
                            placements: ordered,
                        },
                        Command::SetTrackPlacements {
                            track,
                            placements: before,
                        },
                    );
                }
                Vec::new()
            }
            Command::SetPlacementGap { id, gap_before } => {
                if let Some(track) = self
                    .state
                    .placements
                    .iter()
                    .find(|placement| placement.id == id)
                    .map(|placement| placement.track)
                {
                    let before: Vec<Placement> = self
                        .state
                        .placements
                        .iter()
                        .filter(|placement| placement.track == track)
                        .cloned()
                        .collect();
                    let mut ordered = before.clone();
                    ordered.sort_by_key(|placement| placement.start_pulse);
                    if let Some(placement) = ordered.iter_mut().find(|placement| placement.id == id)
                    {
                        placement.gap_before = gap_before;
                    }
                    reflow(&mut ordered);

                    self.state
                        .placements
                        .retain(|placement| placement.track != track);
                    self.state.placements.extend(ordered.clone());
                    self.log(
                        Command::SetTrackPlacements {
                            track,
                            placements: ordered,
                        },
                        Command::SetTrackPlacements {
                            track,
                            placements: before,
                        },
                    );
                }
                Vec::new()
            }
            Command::SetTrackPlacements { track, placements } => {
                self.state
                    .placements
                    .retain(|placement| placement.track != track);
                self.state.placements.extend(placements);
                Vec::new()
            }
        };

        self.sync_dirty_state();

        Applied {
            state: self.state.clone(),
            effects,
        }
    }

    /// Clears the undo/redo history without touching the current state.
    /// Built now so a later "switch projects" ticket can call it, per the
    /// spec's "undo history cleared when I switch projects".
    pub fn clear_history(&mut self) {
        self.undo_log.clear();
        self.redo_log.clear();
    }

    /// Reports that the shell cannot find the selected MIDI device. This does
    /// not alter musical project state or undo history: device selection is an
    /// application preference, not part of a project.
    pub fn no_midi_device_available(&self) -> Applied {
        Applied {
            state: self.state.clone(),
            effects: vec![Effect::NoMidiDeviceAvailable],
        }
    }

    /// Records an applied command and its inverse, evicting the oldest entry
    /// once the log is at capacity, and discards the redo log: applying a
    /// fresh command replaces whatever future `Redo` would have reached.
    fn log(&mut self, forward: Command, inverse: Command) {
        self.redo_log.clear();
        if self.undo_log.len() == HISTORY_CAPACITY {
            self.undo_log.pop_front();
        }
        self.undo_log.push_back(LoggedCommand { forward, inverse });
    }

    fn undo(&mut self) -> Vec<Effect> {
        match self.undo_log.pop_back() {
            Some(entry) => {
                Self::write(&mut self.state, &entry.inverse);
                self.redo_log.push(entry);
                Vec::new()
            }
            None => vec![Effect::NothingToUndo],
        }
    }

    fn redo(&mut self) -> Vec<Effect> {
        match self.redo_log.pop() {
            Some(entry) => {
                Self::write(&mut self.state, &entry.forward);
                // Re-inserting can never exceed capacity: it only replaces an
                // entry that was popped by `undo` moments before.
                self.undo_log.push_back(entry);
                Vec::new()
            }
            None => vec![Effect::NothingToRedo],
        }
    }

    /// Applies a command's effect on `state` directly, without touching the
    /// history. Used to replay a logged `forward`/`inverse` command during
    /// undo/redo. `NewProject`, `Undo` and `Redo` never appear in the log (see
    /// [`DawCore::apply`]), so they are unreachable here in practice; treating
    /// them as a no-op rather than panicking keeps this function total.
    fn write(state: &mut ProjectState, command: &Command) {
        match command {
            Command::SetBpm(bpm) => state.bpm = *bpm,
            Command::SetTimeSignature {
                beats_per_bar,
                beat_unit,
            } => state.time_signature = (*beats_per_bar, *beat_unit),
            Command::SetInstrument(instrument) => state.instrument = *instrument,
            Command::StopRecording(take) => state.take = take.clone(),
            Command::AddTakeToLibrary => {}
            Command::AddBlockAndClearTake(block) => {
                state.blocks.push(block.clone());
                state.take = None;
            }
            Command::RemoveBlockAndRestoreTake { block, take } => {
                state.blocks.retain(|candidate| candidate != block);
                state.take = Some(take.clone());
            }
            Command::InsertBlock(block) => state.blocks.push(block.clone()),
            Command::RemoveBlock(block) => {
                state.blocks.retain(|candidate| candidate != block);
            }
            Command::RenameBlock { id, name } => {
                if let Some(block) = state.blocks.iter_mut().find(|block| block.id == *id) {
                    block.name = name.clone();
                }
            }
            Command::RecolourBlock { id, color } => {
                if let Some(block) = state.blocks.iter_mut().find(|block| block.id == *id) {
                    block.color = color.clone();
                }
            }
            Command::RemoveBlockAndPlacements(block_id) => {
                remove_block_and_its_placements(state, *block_id);
            }
            Command::RestoreBlockAndPlacements { block, placements } => {
                state.blocks.push(block.clone());
                let restored_tracks: BTreeSet<u32> =
                    placements.iter().map(|placement| placement.track).collect();
                state
                    .placements
                    .retain(|placement| !restored_tracks.contains(&placement.track));
                state.placements.extend(placements.iter().cloned());
            }
            Command::InsertPlacement(placement) => state.placements.push(placement.clone()),
            Command::RemovePlacement(placement) => {
                state.placements.retain(|candidate| candidate != placement);
            }
            Command::SetTrackPlacements { track, placements } => {
                state
                    .placements
                    .retain(|placement| placement.track != *track);
                state.placements.extend(placements.iter().cloned());
            }
            Command::SetTakeTrim(trim) => {
                if let Some(take) = state.take.as_mut() {
                    take.set_trim(*trim);
                }
            }
            Command::SetTakeQuantisation(quantisation) => {
                if let Some(take) = state.take.as_mut() {
                    take.set_quantisation(*quantisation);
                }
            }
            // These controls are never logged, so undo/redo never reaches
            // them. The remaining variants cannot occur in the log either.
            Command::SetReverb(_)
            | Command::SetMetronomeEnabled(_)
            | Command::SetCountInEnabled(_)
            | Command::SetLoopEnabled(_)
            | Command::StartRecording
            | Command::PlayTake
            | Command::PlayBlock(_)
            | Command::AddPlacement { .. }
            | Command::InsertPlacementAt { .. }
            | Command::ReorderPlacement { .. }
            | Command::DeletePlacement(_)
            | Command::SetPlacementGap { .. }
            | Command::DeleteBlock { .. }
            | Command::ExportMidi
            | Command::PlayTimeline
            | Command::PlaybackFinished
            | Command::NewProject { .. }
            | Command::OpenProject { .. }
            | Command::RecoverProject(_)
            | Command::SaveProject(_)
            | Command::ProjectSaved(_)
            | Command::Undo
            | Command::Redo => {}
        }
    }

    /// Strips session-only fields before comparing or persisting a project.
    /// Recording, playback and dirty state describe the running application,
    /// not musical work that should reappear after reopening a document.
    fn document_state(state: &ProjectState) -> ProjectState {
        let mut document = state.clone();
        document.is_dirty = false;
        document.is_recording = false;
        document.is_playing = false;
        document
    }

    fn sync_dirty_state(&mut self) {
        let document = Self::document_state(&self.state);
        self.state.is_dirty = self
            .saved_state
            .as_ref()
            .is_none_or(|saved| document != *saved);
    }
}

/// Maps a pulse offset to wall-clock time at `bpm`.
///
/// Notes remain stored at their pulse offsets, so callers can use this only
/// when rendering playback. A zero BPM has no meaningful wall-clock mapping.
pub fn pulse_elapsed_time(pulse: u64, bpm: u16) -> Option<Duration> {
    (bpm != 0).then(|| {
        Duration::from_secs_f64(pulse as f64 * 60.0 / (f64::from(bpm) * PULSES_PER_BEAT as f64))
    })
}

/// Whether `pulse` is the first pulse of its bar and should receive the
/// metronome accent. The beat unit deliberately has no role: pulses are the
/// application's unit of time, while the time signature has no structural
/// power.
pub fn is_bar_accent(pulse: u64, time_signature: (u8, u8)) -> bool {
    let beats_per_bar = u64::from(time_signature.0) * PULSES_PER_BEAT;
    beats_per_bar != 0 && pulse.is_multiple_of(beats_per_bar)
}

/// The number of pulses in the one-bar count-in governed by `time_signature`.
/// The beat unit deliberately has no role for the same reason as in
/// [`is_bar_accent`].
pub fn count_in_length_in_pulses(time_signature: (u8, u8)) -> u64 {
    u64::from(time_signature.0) * PULSES_PER_BEAT
}

/// The inverse of [`pulse_elapsed_time`]: how many whole pulses have elapsed
/// by `elapsed` at `bpm`. Used to timestamp live-captured MIDI (wall-clock
/// time, from the shell) into the pulse offsets a [`Take`] stores.
pub fn pulse_at_elapsed_time(elapsed: Duration, bpm: u16) -> u64 {
    (elapsed.as_secs_f64() * f64::from(bpm) * PULSES_PER_BEAT as f64 / 60.0) as u64
}

/// One note-on or note-off observed live, timestamped as elapsed time since
/// recording's pulse zero (i.e. after any count-in). This is the shell's
/// input to [`build_take`] — it owns the native MIDI connection and the
/// clock; the core only turns the raw stream into pulse-stamped notes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedEvent {
    pub elapsed: Duration,
    pub pitch: u8,
    pub velocity: u8,
    pub is_on: bool,
}

/// Turns a chronological stream of captured note on/off events into a
/// [`Take`], pairing each note-on with its note-off. A pitch still held when
/// `stopped_at` is reached is captured as ending exactly there, per the
/// spec's "notes still held at the stop point are captured as ending there"
/// — this is the one place that rule is enforced, so every caller (real
/// MIDI, or a test driving a `ScriptedMidiInput`) gets it for free. A second
/// note-on for an already-held pitch closes the first at that pulse before
/// opening the next, so a fast retrigger never produces a note with no end.
pub fn build_take(events: &[CapturedEvent], stopped_at: Duration, bpm: u16) -> Take {
    let mut notes = Vec::new();
    let mut held: std::collections::HashMap<u8, (u64, u8)> = std::collections::HashMap::new();

    for event in events {
        let pulse = pulse_at_elapsed_time(event.elapsed, bpm);
        if event.is_on {
            if let Some((start_pulse, velocity)) = held.remove(&event.pitch) {
                notes.push(RecordedNote {
                    pitch: event.pitch,
                    velocity,
                    start_pulse,
                    end_pulse: pulse,
                });
            }
            held.insert(event.pitch, (pulse, event.velocity));
        } else if let Some((start_pulse, velocity)) = held.remove(&event.pitch) {
            notes.push(RecordedNote {
                pitch: event.pitch,
                velocity,
                start_pulse,
                end_pulse: pulse,
            });
        }
    }

    let stop_pulse = pulse_at_elapsed_time(stopped_at, bpm);
    let mut still_held: Vec<_> = held.into_iter().collect();
    still_held.sort_by_key(|(pitch, _)| *pitch);
    for (pitch, (start_pulse, velocity)) in still_held {
        notes.push(RecordedNote {
            pitch,
            velocity,
            start_pulse,
            end_pulse: stop_pulse.max(start_pulse),
        });
    }

    notes.sort_by_key(|note| note.start_pulse);
    Take::from_raw_notes(notes)
}
