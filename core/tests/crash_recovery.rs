//! Issue #15: a crash recovery snapshot restores an interrupted session
//! without ever being confused for a manual save.

use daw_core::{Command, DawCore, Effect, RecordedNote, Take};

#[test]
fn recovering_a_snapshot_restores_state_marks_it_still_unsaved_and_clears_undo_history() {
    let mut interrupted = DawCore::new();
    interrupted.apply(Command::SetBpm(150));
    interrupted.apply(Command::StopRecording(Some(Take::from_raw_notes(vec![
        RecordedNote {
            pitch: 60,
            velocity: 100,
            start_pulse: 0,
            end_pulse: 4,
        },
    ]))));
    interrupted.apply(Command::AddTakeToLibrary);
    assert!(interrupted.state().is_dirty);
    let snapshot = interrupted.project_document();

    let mut recovered = DawCore::new();
    let applied = recovered.apply(Command::RecoverProject(snapshot));

    assert_eq!(applied.effects, Vec::<Effect>::new());
    assert_eq!(applied.state.bpm, 150);
    assert_eq!(applied.state.blocks.len(), 1);
    assert!(
        applied.state.is_dirty,
        "a recovered session was never manually saved, so it must still show unsaved changes"
    );

    let undo = recovered.apply(Command::Undo);
    assert_eq!(undo.effects, vec![Effect::NothingToUndo]);
}
