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
fn applying_set_instrument_changes_the_project_state() {
    let mut core = DawCore::new();

    let applied = core.apply(Command::SetInstrument(1));

    assert_eq!(applied.state.instrument, 1);
    assert_eq!(core.state().instrument, 1);
}

#[test]
fn applying_set_reverb_changes_the_project_state() {
    let mut core = DawCore::new();

    let applied = core.apply(Command::SetReverb(75));

    assert_eq!(applied.state.reverb, 75);
    assert_eq!(core.state().reverb, 75);
}

#[test]
fn applying_pulse_controls_changes_the_project_state() {
    let mut core = DawCore::new();

    core.apply(Command::SetMetronomeEnabled(true));
    core.apply(Command::SetCountInEnabled(false));
    let applied = core.apply(Command::SetLoopEnabled(true));

    assert!(applied.state.metronome_enabled);
    assert!(!applied.state.count_in_enabled);
    assert!(applied.state.loop_enabled);
}

#[test]
fn new_project_resets_state_to_default() {
    let mut core = DawCore::new();
    core.apply(Command::SetBpm(200));

    let applied = core.apply(Command::NewProject { force: true });

    assert_eq!(applied.state.bpm, 120);
    assert_eq!(applied.state.time_signature, (3, 4));
    assert_eq!(applied.state.instrument, 0);
    assert_eq!(applied.state.reverb, 0);
    assert!(!applied.state.metronome_enabled);
    assert!(applied.state.count_in_enabled);
}
