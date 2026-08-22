//! Undo is a command log, not a state snapshot: applying a command pushes its
//! inverse onto a bounded history; `Undo`/`Redo` are themselves never logged.

use daw_core::{Command, DawCore, Effect, HISTORY_CAPACITY};

#[test]
fn undo_reverts_the_most_recent_command_exactly() {
    let mut core = DawCore::new();
    core.apply(Command::SetBpm(140));

    let applied = core.apply(Command::Undo);

    assert_eq!(applied.state.bpm, 120);
    assert_eq!(applied.effects, Vec::new());
}

#[test]
fn redo_reapplies_an_undone_command() {
    let mut core = DawCore::new();
    core.apply(Command::SetBpm(140));
    core.apply(Command::Undo);

    let applied = core.apply(Command::Redo);

    assert_eq!(applied.state.bpm, 140);
}

#[test]
fn undo_reverts_an_instrument_change() {
    let mut core = DawCore::new();
    core.apply(Command::SetInstrument(1));

    let applied = core.apply(Command::Undo);

    assert_eq!(applied.state.instrument, 0);
}

#[test]
fn reverb_changes_are_not_logged_or_undone() {
    let mut core = DawCore::new();
    core.apply(Command::SetReverb(75));
    core.apply(Command::SetBpm(140));

    let applied = core.apply(Command::Undo);

    assert_eq!(applied.state.bpm, 120);
    assert_eq!(applied.state.reverb, 75);

    let applied = core.apply(Command::Undo);
    assert_eq!(applied.state.reverb, 75);
    assert_eq!(applied.effects, vec![Effect::NothingToUndo]);
}

#[test]
fn pulse_control_changes_are_not_logged_or_undone() {
    let mut core = DawCore::new();
    core.apply(Command::SetMetronomeEnabled(true));
    core.apply(Command::SetCountInEnabled(false));
    core.apply(Command::SetBpm(140));

    let applied = core.apply(Command::Undo);

    assert_eq!(applied.state.bpm, 120);
    assert!(applied.state.metronome_enabled);
    assert!(!applied.state.count_in_enabled);
    assert_eq!(
        core.apply(Command::Undo).effects,
        vec![Effect::NothingToUndo]
    );
}

#[test]
fn a_sequence_of_commands_undone_and_redone_ends_in_the_expected_state() {
    let mut core = DawCore::new();
    core.apply(Command::SetBpm(140)); // 120 -> 140
    core.apply(Command::SetTimeSignature {
        beats_per_bar: 4,
        beat_unit: 4,
    }); // (3,4) -> (4,4)
    core.apply(Command::SetBpm(90)); // 140 -> 90

    core.apply(Command::Undo); // -> bpm 140, sig (4,4)
    core.apply(Command::Undo); // -> bpm 140, sig (3,4)
    let applied = core.apply(Command::Redo); // -> bpm 140, sig (4,4)

    assert_eq!(applied.state.bpm, 140);
    assert_eq!(applied.state.time_signature, (4, 4));

    let applied = core.apply(Command::Redo); // -> bpm 90, sig (4,4)
    assert_eq!(applied.state.bpm, 90);
    assert_eq!(applied.state.time_signature, (4, 4));
}

#[test]
fn applying_a_command_after_undo_discards_the_redo_history() {
    let mut core = DawCore::new();
    core.apply(Command::SetBpm(140));
    core.apply(Command::Undo);

    core.apply(Command::SetBpm(200));
    let applied = core.apply(Command::Redo);

    // Nothing to redo: the branch taken after undo replaced the future.
    assert_eq!(applied.state.bpm, 200);
    assert_eq!(applied.effects, vec![Effect::NothingToRedo]);
}

#[test]
fn history_is_bounded_to_its_capacity() {
    let mut core = DawCore::new();

    // Push well past the cap.
    for bpm in 1..=(HISTORY_CAPACITY as u16 + 10) {
        core.apply(Command::SetBpm(bpm));
    }

    // Undo back as far as the cap allows...
    for _ in 0..HISTORY_CAPACITY {
        core.apply(Command::Undo);
    }
    // ...and the next undo has nothing left, without panicking.
    let applied = core.apply(Command::Undo);
    assert_eq!(applied.effects, vec![Effect::NothingToUndo]);

    // The oldest entries were evicted, so undoing stopped short of bpm 1.
    let expected_bpm = 10; // the (10+1)th command survived the eviction of the first 10
    assert_eq!(applied.state.bpm, expected_bpm);
}

#[test]
fn undoing_with_empty_history_is_a_no_op_that_reports_nothing_to_undo() {
    let mut core = DawCore::new();

    let applied = core.apply(Command::Undo);

    assert_eq!(applied.state.bpm, 120);
    assert_eq!(applied.effects, vec![Effect::NothingToUndo]);
}

#[test]
fn redoing_with_empty_history_is_a_no_op_that_reports_nothing_to_redo() {
    let mut core = DawCore::new();

    let applied = core.apply(Command::Redo);

    assert_eq!(applied.state.bpm, 120);
    assert_eq!(applied.effects, vec![Effect::NothingToRedo]);
}

#[test]
fn clearing_history_leaves_nothing_to_undo() {
    let mut core = DawCore::new();
    core.apply(Command::SetBpm(140));

    core.clear_history();
    let applied = core.apply(Command::Undo);

    // The bpm change itself is untouched; only the log was cleared.
    assert_eq!(applied.state.bpm, 140);
    assert_eq!(applied.effects, vec![Effect::NothingToUndo]);
}
