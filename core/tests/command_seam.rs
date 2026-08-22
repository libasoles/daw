//! The basic seam: a `Command` goes in, the new `ProjectState` and any
//! `Effect`s come out. See `undo_redo.rs` for the history behaviour.

use daw_core::{Command, DawCore};

#[test]
fn applying_set_bpm_changes_the_project_state() {
    let mut core = DawCore::new();

    let applied = core.apply(Command::SetBpm(140));

    assert_eq!(applied.state.bpm, 140);
    assert_eq!(applied.effects, Vec::new());
    // The returned state matches what `core.state()` reports afterwards.
    assert_eq!(core.state().bpm, 140);
}

#[test]
fn applying_set_time_signature_changes_the_project_state() {
    let mut core = DawCore::new();

    let applied = core.apply(Command::SetTimeSignature {
        beats_per_bar: 4,
        beat_unit: 4,
    });

    assert_eq!(applied.state.time_signature, (4, 4));
    assert_eq!(core.state().time_signature, (4, 4));
}

#[test]
fn new_project_resets_state_to_default() {
    let mut core = DawCore::new();
    core.apply(Command::SetBpm(200));

    let applied = core.apply(Command::NewProject);

    assert_eq!(applied.state.bpm, 120);
    assert_eq!(applied.state.time_signature, (3, 4));
}
