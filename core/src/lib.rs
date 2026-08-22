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

use std::collections::VecDeque;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

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

const BLOCK_COLORS: [&str; 6] = [
    "#f87171", "#fbbf24", "#34d399", "#60a5fa", "#a78bfa", "#f472b6",
];

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
    /// wholesale and clears the undo/redo history, the same capability a
    /// later "switch projects" ticket will reuse.
    NewProject,
    /// Requests a fresh project. Dirty work yields a confirmation effect
    /// rather than discarding anything until the shell resolves it.
    RequestNewProject,
    /// Replaces the current project with a document the shell read through
    /// its storage port. Opening never enters undo history.
    OpenProject(ProjectState),
    /// Requests opening a document the shell has already read through its
    /// storage adapter. Dirty work yields a confirmation effect first.
    RequestOpenProject(ProjectState),
    /// Requests application quit through the same dirty-project policy.
    RequestQuit,
    /// Resolves the most recently requested project change.
    ResolveProjectChange(ProjectChangeResolution),
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
    /// Arms recording. Not itself undoable — arming isn't a musical edit, the
    /// take that results from it is (see [`Command::StopRecording`]).
    ///
    /// If the recording area already holds a take and `force` is `false`,
    /// this reports [`Effect::ConfirmOverwriteRecording`] and leaves
    /// `is_recording` untouched, per the spec's "recording over a take that
    /// has not been added to the library asks for confirmation first" — the
    /// library (#11) doesn't exist yet, so every existing take counts.
    /// Resending with `force: true` (after the shell has confirmed with the
    /// user) proceeds unconditionally. A missing take never needs `force`.
    StartRecording {
        force: bool,
    },
    /// Finishes recording, replacing the recording area's take with `take`.
    /// The shell builds `take` from the raw MIDI it captured — the core
    /// performs no I/O and cannot listen for MIDI itself. Always sent as
    /// `Some` by the shell; `None` only appears as this command's own
    /// undo inverse, when there was no take to revert to. Undoable: reverts
    /// to whatever take (if any) was there before.
    StopRecording(Option<Take>),
    AddTakeToLibrary,
    InsertBlock(Block),
    RemoveBlock(Block),
    PlayBlock(usize),
    RenameBlock {
        id: u64,
        name: String,
    },
    RecolourBlock {
        id: u64,
        color: String,
    },
    DeleteBlock(u64),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProjectChangeResolution {
    Proceed,
    Cancel,
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
    /// `StartRecording { force: false }` was applied while the recording
    /// area already held a take. The shell should ask the user to confirm,
    /// then resend `StartRecording { force: true }` if they agree.
    ConfirmOverwriteRecording,
    /// The shell should show the single unsaved-project decision.
    ConfirmDiscardProjectChanges,
    /// The shell may now close the application.
    QuitApplication,
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

#[derive(Debug, Clone, PartialEq)]
enum PendingProjectChange {
    New,
    Open(ProjectState),
    Quit,
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
    pending_project_change: Option<PendingProjectChange>,
}

impl Default for DawCore {
    fn default() -> Self {
        let mut core = Self {
            state: ProjectState::default(),
            saved_state: None,
            undo_log: VecDeque::new(),
            redo_log: Vec::new(),
            pending_project_change: None,
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
            Command::NewProject => self.apply_project_change(PendingProjectChange::New),
            Command::RequestNewProject => self.request_project_change(PendingProjectChange::New),
            Command::OpenProject(state) => {
                self.apply_project_change(PendingProjectChange::Open(state))
            }
            Command::RequestOpenProject(state) => {
                self.request_project_change(PendingProjectChange::Open(state))
            }
            Command::RequestQuit => self.request_project_change(PendingProjectChange::Quit),
            Command::ResolveProjectChange(resolution) => {
                let pending = self.pending_project_change.take();
                match (resolution, pending) {
                    (ProjectChangeResolution::Proceed, Some(change)) => {
                        self.apply_project_change(change)
                    }
                    _ => Vec::new(),
                }
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
            Command::StartRecording { force } => {
                if self.state.take.is_some() && !force {
                    vec![Effect::ConfirmOverwriteRecording]
                } else {
                    self.state.is_recording = true;
                    Vec::new()
                }
            }
            Command::StopRecording(take) => {
                self.state.is_recording = false;
                let inverse = Command::StopRecording(self.state.take.clone());
                self.state.take = take.clone();
                self.log(Command::StopRecording(take), inverse);
                Vec::new()
            }
            Command::AddTakeToLibrary => {
                if let Some(take) = self.state.take.as_ref() {
                    let block = Block {
                        id: self.state.next_block_id,
                        name: format!("Take {}", self.state.blocks.len() + 1),
                        color: BLOCK_COLORS[self.state.blocks.len() % BLOCK_COLORS.len()].into(),
                        instrument: self.state.instrument,
                        notes: take.notes(),
                    };
                    self.state.blocks.push(block.clone());
                    self.state.next_block_id += 1;
                    self.log(
                        Command::InsertBlock(block.clone()),
                        Command::RemoveBlock(block),
                    );
                }
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
            Command::DeleteBlock(id) => {
                if let Some(index) = self.state.blocks.iter().position(|block| block.id == id) {
                    let block = self.state.blocks.remove(index);
                    self.log(
                        Command::RemoveBlock(block.clone()),
                        Command::InsertBlock(block),
                    );
                }
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
            Command::PlaybackFinished => {
                self.state.is_playing = false;
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
            Command::DeleteBlock(_) => {}
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
            | Command::StartRecording { .. }
            | Command::PlayTake
            | Command::PlayBlock(_)
            | Command::PlaybackFinished
            | Command::NewProject
            | Command::RequestNewProject
            | Command::OpenProject(_)
            | Command::RequestOpenProject(_)
            | Command::RequestQuit
            | Command::ResolveProjectChange(_)
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

    fn request_project_change(&mut self, change: PendingProjectChange) -> Vec<Effect> {
        if self.state.is_dirty {
            self.pending_project_change = Some(change);
            vec![Effect::ConfirmDiscardProjectChanges]
        } else {
            self.apply_project_change(change)
        }
    }

    fn apply_project_change(&mut self, change: PendingProjectChange) -> Vec<Effect> {
        self.pending_project_change = None;
        match change {
            PendingProjectChange::New => {
                self.state = ProjectState::default();
                self.saved_state = None;
                self.clear_history();
                Vec::new()
            }
            PendingProjectChange::Open(mut state) => {
                state.is_recording = false;
                state.is_playing = false;
                state.is_dirty = false;
                self.state = state.clone();
                self.saved_state = Some(state);
                self.clear_history();
                Vec::new()
            }
            PendingProjectChange::Quit => vec![Effect::QuitApplication],
        }
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
