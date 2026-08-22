//! Issue #11 crosses the public command seam: adding freezes the take's view
//! into an immutable block owned by the project library.

use daw_core::{Command, DawCore, RecordedNote, Take, Trim};

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
    assert_eq!(added.state.blocks[0].notes[0].start_pulse, 2);
    assert_eq!(added.state.blocks[0].notes[0].end_pulse, 6);

    core.apply(Command::SetTakeTrim(Trim {
        start_pulse: 0,
        end_pulse: 8,
    }));
    assert_eq!(core.state().blocks[0].notes[0].start_pulse, 2);
    core.apply(Command::Undo);
    assert!(core.apply(Command::Undo).state.blocks.is_empty());
    assert_eq!(core.apply(Command::Redo).state.blocks[0].name, "Take 1");
}
