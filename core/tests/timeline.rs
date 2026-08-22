//! Issue #17: dragging a block onto the timeline creates a placement, an
//! independent copy of the block that lands flush after the existing
//! placements on its track, snapped to the pulse.

use daw_core::{Command, DawCore, RecordedNote, Take};

fn record_and_freeze(core: &mut DawCore, pitch: u8, end_pulse: u64) -> u64 {
    core.apply(Command::StopRecording(Some(Take::from_raw_notes(vec![
        RecordedNote {
            pitch,
            velocity: 100,
            start_pulse: 0,
            end_pulse,
        },
    ]))));
    core.apply(Command::AddTakeToLibrary).state.blocks[core.state().blocks.len() - 1].id
}

#[test]
fn dropping_a_block_creates_an_independent_copy_starting_at_pulse_zero() {
    let mut core = DawCore::new();
    let block_id = record_and_freeze(&mut core, 60, 4);

    let applied = core.apply(Command::AddPlacement { block_id, track: 0 });

    assert_eq!(applied.state.placements.len(), 1);
    let placement = &applied.state.placements[0];
    assert_eq!(placement.track, 0);
    assert_eq!(placement.start_pulse, 0);
    assert_eq!(placement.notes, applied.state.blocks[0].notes);
    assert_eq!(placement.name, applied.state.blocks[0].name);
    assert_eq!(placement.color, applied.state.blocks[0].color);

    // It's a copy: mutating the source block afterwards must not reach it.
    core.apply(Command::RenameBlock {
        id: block_id,
        name: "Renamed".into(),
    });
    assert_eq!(core.state().placements[0].name, "Take 1");
}

#[test]
fn a_second_placement_lands_flush_after_the_first_on_the_same_track() {
    let mut core = DawCore::new();
    let first = record_and_freeze(&mut core, 60, 4);
    let second = record_and_freeze(&mut core, 64, 6);

    core.apply(Command::AddPlacement {
        block_id: first,
        track: 0,
    });
    let applied = core.apply(Command::AddPlacement {
        block_id: second,
        track: 0,
    });

    assert_eq!(applied.state.placements.len(), 2);
    assert_eq!(applied.state.placements[0].start_pulse, 0);
    assert_eq!(applied.state.placements[1].start_pulse, 4);
}

#[test]
fn the_same_block_can_be_placed_many_times() {
    let mut core = DawCore::new();
    let block_id = record_and_freeze(&mut core, 60, 4);

    core.apply(Command::AddPlacement { block_id, track: 0 });
    core.apply(Command::AddPlacement { block_id, track: 0 });
    let applied = core.apply(Command::AddPlacement { block_id, track: 0 });

    assert_eq!(applied.state.placements.len(), 3);
    assert_eq!(
        applied
            .state
            .placements
            .iter()
            .map(|p| p.start_pulse)
            .collect::<Vec<_>>(),
        vec![0, 4, 8]
    );
}

#[test]
fn tracks_are_independent_so_a_second_track_starts_flush_at_zero_too() {
    let mut core = DawCore::new();
    let first = record_and_freeze(&mut core, 60, 4);
    let second = record_and_freeze(&mut core, 64, 6);

    core.apply(Command::AddPlacement {
        block_id: first,
        track: 0,
    });
    let applied = core.apply(Command::AddPlacement {
        block_id: second,
        track: 1,
    });

    let on_track_1 = applied
        .state
        .placements
        .iter()
        .find(|p| p.track == 1)
        .unwrap();
    assert_eq!(on_track_1.start_pulse, 0);
}

#[test]
fn placements_never_overlap_because_each_one_starts_where_the_last_ended() {
    let mut core = DawCore::new();
    let short = record_and_freeze(&mut core, 60, 3);
    let long = record_and_freeze(&mut core, 64, 10);

    core.apply(Command::AddPlacement {
        block_id: short,
        track: 0,
    });
    core.apply(Command::AddPlacement {
        block_id: long,
        track: 0,
    });
    let applied = core.apply(Command::AddPlacement {
        block_id: short,
        track: 0,
    });

    let placements = &applied.state.placements;
    for pair in placements.windows(2) {
        let [earlier, later] = pair else {
            unreachable!()
        };
        assert!(
            earlier.start_pulse + earlier.notes.iter().map(|n| n.end_pulse).max().unwrap()
                <= later.start_pulse,
            "{earlier:?} must end at or before {later:?} starts"
        );
    }
    assert_eq!(
        placements.iter().map(|p| p.start_pulse).collect::<Vec<_>>(),
        vec![0, 3, 13]
    );
}

#[test]
fn placing_a_nonexistent_block_does_nothing() {
    let mut core = DawCore::new();

    let applied = core.apply(Command::AddPlacement {
        block_id: 999,
        track: 0,
    });

    assert!(applied.state.placements.is_empty());
}

#[test]
fn placing_a_block_is_undoable() {
    let mut core = DawCore::new();
    let block_id = record_and_freeze(&mut core, 60, 4);

    core.apply(Command::AddPlacement { block_id, track: 0 });
    assert_eq!(core.state().placements.len(), 1);

    let undone = core.apply(Command::Undo);
    assert!(undone.state.placements.is_empty());

    let redone = core.apply(Command::Redo);
    assert_eq!(redone.state.placements.len(), 1);
}
