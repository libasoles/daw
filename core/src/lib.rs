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
//! This module is a scaffold. Issue #3 replaces it with the real command
//! vocabulary, project model and undo log; what it establishes now is the shape
//! of the seam and the test conventions in `core/tests/`.

#![forbid(unsafe_code)]

/// Everything about a piece of music that gets persisted.
///
/// This is exactly the structure serialised to `project.json` and
/// `recovery.json`: what the tests assert on is what gets saved, so there is no
/// second representation to drift.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// The in-memory state machine every user gesture is funnelled through.
#[derive(Debug, Default)]
pub struct DawCore {
    state: ProjectState,
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
}
