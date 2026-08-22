//! Issue #14: creating a new project or opening another one warns first when
//! there are unsaved changes, per the spec's "this warning is the
//! application's only modal dialog" — and either way clears undo history, so
//! it's never possible to undo one's way into another piece.

use daw_core::{Command, DawCore, Effect, RecordedNote, Take};

fn make_dirty(core: &mut DawCore) {
    core.apply(Command::SetBpm(180));
    assert!(core.state().is_dirty);
}

#[test]
fn new_project_with_unsaved_changes_asks_for_confirmation_and_leaves_the_project_untouched() {
    let mut core = DawCore::new();
    make_dirty(&mut core);

    let refused = core.apply(Command::NewProject { force: false });

    assert_eq!(refused.effects, vec![Effect::ConfirmDiscardUnsavedChanges]);
    assert_eq!(
        refused.state.bpm, 180,
        "cancelling leaves the project untouched"
    );
}

#[test]
fn new_project_forced_after_confirmation_resets_state_and_clears_undo_history() {
    let mut core = DawCore::new();
    make_dirty(&mut core);

    let applied = core.apply(Command::NewProject { force: true });

    assert_eq!(applied.effects, Vec::<Effect>::new());
    assert_eq!(applied.state.bpm, 120);
    assert!(applied.state.blocks.is_empty());
    assert!(applied.state.take.is_none());

    let undo = core.apply(Command::Undo);
    assert_eq!(undo.effects, vec![Effect::NothingToUndo]);
}

#[test]
fn a_project_with_no_unsaved_changes_opens_a_new_one_without_confirmation() {
    let mut core = DawCore::new();
    core.apply(Command::ProjectSaved);
    assert!(!core.state().is_dirty);

    let applied = core.apply(Command::NewProject { force: false });

    assert_eq!(applied.effects, Vec::<Effect>::new());
}

#[test]
fn opening_a_project_with_unsaved_changes_asks_for_confirmation_and_leaves_the_project_untouched() {
    let mut core = DawCore::new();
    make_dirty(&mut core);
    let other_project = DawCore::new().project_document();

    let refused = core.apply(Command::OpenProject {
        document: other_project,
        force: false,
    });

    assert_eq!(refused.effects, vec![Effect::ConfirmDiscardUnsavedChanges]);
    assert_eq!(
        refused.state.bpm, 180,
        "cancelling leaves the project untouched"
    );
}

#[test]
fn opening_a_project_forced_after_confirmation_restores_it_and_clears_undo_history() {
    let mut core = DawCore::new();
    make_dirty(&mut core);

    let mut other = DawCore::new();
    other.apply(Command::SetBpm(90));
    other.apply(Command::StopRecording(Some(Take::from_raw_notes(vec![
        RecordedNote {
            pitch: 60,
            velocity: 100,
            start_pulse: 0,
            end_pulse: 4,
        },
    ]))));
    let other_project = other.project_document();

    let applied = core.apply(Command::OpenProject {
        document: other_project,
        force: true,
    });

    assert_eq!(applied.effects, Vec::<Effect>::new());
    assert_eq!(applied.state.bpm, 90);
    assert!(applied.state.take.is_some());
    assert!(!applied.state.is_dirty);

    let undo = core.apply(Command::Undo);
    assert_eq!(undo.effects, vec![Effect::NothingToUndo]);
}
