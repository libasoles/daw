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

use serde::{Deserialize, Serialize};

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
    /// The global pulse, in beats per minute.
    pub bpm: u16,
    /// Beats per bar and the beat unit, e.g. `(3, 4)`.
    ///
    /// The time signature has no structural power: it governs the metronome's
    /// accent, the length of the count-in and a purely visual grid accent.
    pub time_signature: (u8, u8),
}

impl Default for ProjectState {
    fn default() -> Self {
        Self {
            bpm: 120,
            time_signature: (3, 4),
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
    /// Sets the global tempo in beats per minute.
    SetBpm(u16),
    /// Sets the project's time signature (beats per bar, beat unit).
    SetTimeSignature { beats_per_bar: u8, beat_unit: u8 },
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
#[derive(Debug, Default)]
pub struct DawCore {
    state: ProjectState,
    undo_log: VecDeque<LoggedCommand>,
    redo_log: Vec<LoggedCommand>,
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

    /// The single entry point into the core: a command goes in, the new
    /// project state and any effects come out. Performs no I/O.
    pub fn apply(&mut self, command: Command) -> Applied {
        let effects = match command {
            Command::NewProject => {
                self.state = ProjectState::default();
                self.clear_history();
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
        };

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
            Command::NewProject | Command::Undo | Command::Redo => {}
        }
    }
}
