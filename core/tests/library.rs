//! Issue #11 crosses the public command seam: adding freezes the take's view
//! into an immutable block owned by the project library.

use daw_core::{Command, DawCore, Effect, RecordedNote, Take, Trim};

#[test]
fn adding_a_take_to_the_library_freezes_its_current_view_and_is_undoable() {
    let mut core = DawCore::new();
    core.apply(Command::StopRecording(Some(Take::from_raw_notes(vec![
        RecordedNote {
            pitch: 60,
            velocity: 100,
            start_pulse: 0,
            end_pulse: 8,
        },
    ]))));
    core.apply(Command::SetTakeTrim(Trim {
        start_pulse: 2,
        end_pulse: 6,
    }));

    let added = core.apply(Command::AddTakeToLibrary);

    assert_eq!(added.state.blocks.len(), 1);
    assert_eq!(added.state.blocks[0].name, "Take 1");
    assert_eq!(added.state.blocks[0].color, "#f6cf4a");
    assert_eq!(added.state.blocks[0].notes[0].start_pulse, 2);
    assert_eq!(added.state.blocks[0].notes[0].end_pulse, 6);
    assert!(added.state.take.is_none());

    let undone = core.apply(Command::Undo);
    assert!(undone.state.blocks.is_empty());
    assert_eq!(
        undone.state.take.unwrap().trim,
        Trim {
            start_pulse: 2,
            end_pulse: 6,
        }
    );
    let redone = core.apply(Command::Redo);
    assert_eq!(redone.state.blocks[0].name, "Take 1");
    assert!(redone.state.take.is_none());
}

#[test]
fn renaming_recolouring_and_deleting_a_block_are_undoable() {
    let mut core = DawCore::new();
    core.apply(Command::StopRecording(Some(Take::from_raw_notes(vec![
        RecordedNote {
            pitch: 64,
            velocity: 90,
            start_pulse: 0,
            end_pulse: 4,
        },
    ]))));
    let block = core.apply(Command::AddTakeToLibrary).state.blocks[0].clone();
    core.apply(Command::RenameBlock {
        id: block.id,
        name: "Verse".into(),
    });
    core.apply(Command::RecolourBlock {
        id: block.id,
        color: "#60a5fa".into(),
    });
    let deleted = core.apply(Command::DeleteBlock {
        id: block.id,
        force: false,
    });
    assert!(deleted.state.blocks.is_empty());

    let restored = core.apply(Command::Undo);
    assert_eq!(restored.state.blocks[0].name, "Verse");
    assert_eq!(restored.state.blocks[0].color, "#60a5fa");
    assert_eq!(restored.state.blocks[0].notes, block.notes);
}

#[test]
fn adding_a_quantised_take_to_the_library_resolves_the_quantisation_into_the_block() {
    let mut core = DawCore::new();
    core.apply(Command::StopRecording(Some(Take::from_raw_notes(vec![
        RecordedNote {
            pitch: 72,
            velocity: 100,
            start_pulse: 1,
            end_pulse: 3,
        },
    ]))));
    core.apply(Command::SetTakeQuantisation(
        daw_core::Quantisation::Quarter,
    ));

    let added = core.apply(Command::AddTakeToLibrary);

    assert_eq!(added.state.blocks[0].notes[0].start_pulse, 2);
    // The take's default trim spans exactly its raw length (0..3), so the
    // quantised end (4) is clamped back down to the take's own end (3).
    assert_eq!(added.state.blocks[0].notes[0].end_pulse, 3);
}

fn record_and_freeze(core: &mut DawCore, pitch: u8) {
    core.apply(Command::StopRecording(Some(Take::from_raw_notes(vec![
        RecordedNote {
            pitch,
            velocity: 100,
            start_pulse: 0,
            end_pulse: 4,
        },
    ]))));
    core.apply(Command::AddTakeToLibrary);
}

#[test]
fn a_block_played_in_isolation_reports_its_own_schedule_and_is_mutually_exclusive_with_take_playback(
) {
    let mut core = DawCore::new();
    record_and_freeze(&mut core, 60);
    let block = core.state().blocks[0].clone();

    let applied = core.apply(Command::PlayBlock(0));
    assert_eq!(
        applied.effects,
        vec![Effect::PlaySchedule(Take::from_raw_notes(block.notes))]
    );
    assert!(applied.state.is_playing);

    // Take playback is refused while a block is playing.
    let take_attempt = core.apply(Command::PlayTake);
    assert_eq!(take_attempt.effects, Vec::<Effect>::new());
    assert!(take_attempt.state.is_playing);

    core.apply(Command::PlaybackFinished);
    assert!(!core.state().is_playing);

    // Once playback is idle again, PlayBlock is refused while a take is playing.
    core.apply(Command::StopRecording(Some(Take::from_raw_notes(vec![
        RecordedNote {
            pitch: 62,
            velocity: 100,
            start_pulse: 0,
            end_pulse: 4,
        },
    ]))));
    core.apply(Command::PlayTake);
    let block_attempt = core.apply(Command::PlayBlock(0));
    assert_eq!(block_attempt.effects, Vec::<Effect>::new());
}

#[test]
fn playing_an_out_of_range_block_index_does_nothing() {
    let mut core = DawCore::new();

    let applied = core.apply(Command::PlayBlock(0));

    assert_eq!(applied.effects, Vec::<Effect>::new());
    assert!(!applied.state.is_playing);
}

#[test]
fn deleting_a_block_does_not_cause_the_next_block_to_reuse_its_name_or_colour() {
    let mut core = DawCore::new();
    record_and_freeze(&mut core, 60);
    record_and_freeze(&mut core, 62);
    record_and_freeze(&mut core, 64);
    assert_eq!(core.state().blocks[2].name, "Take 3");

    let second_id = core.state().blocks[1].id;
    core.apply(Command::DeleteBlock {
        id: second_id,
        force: false,
    });
    assert_eq!(core.state().blocks.len(), 2);

    record_and_freeze(&mut core, 67);

    let names: Vec<_> = core.state().blocks.iter().map(|b| b.name.clone()).collect();
    assert_eq!(names, vec!["Take 1", "Take 3", "Take 4"]);
    let colors: Vec<_> = core
        .state()
        .blocks
        .iter()
        .map(|b| b.color.clone())
        .collect();
    assert_ne!(
        colors[1], colors[2],
        "surviving block and the freshly added one must keep distinct colours"
    );
}
