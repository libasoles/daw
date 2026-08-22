//! Issue #20: dropping a block between two existing placements inserts it
//! and pushes the remainder later; dragging an existing placement to a new
//! position reorders the arrangement with the same push behaviour.

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

/// Places three blocks (lengths 4, 6, 3 pulses) flush on track 0 and
/// returns their block ids in placement order.
fn arrange_three(core: &mut DawCore) -> [u64; 3] {
    let a = record_and_freeze(core, 60, 4);
    let b = record_and_freeze(core, 62, 6);
    let c = record_and_freeze(core, 64, 3);
    core.apply(Command::AddPlacement {
        block_id: a,
        track: 0,
    });
    core.apply(Command::AddPlacement {
        block_id: b,
        track: 0,
    });
    core.apply(Command::AddPlacement {
        block_id: c,
        track: 0,
    });
    [a, b, c]
}

fn ordered_track(core: &DawCore, track: u32) -> Vec<daw_core::Placement> {
    let mut placements: Vec<_> = core
        .state()
        .placements
        .iter()
        .filter(|p| p.track == track)
        .cloned()
        .collect();
    placements.sort_by_key(|p| p.start_pulse);
    placements
}

fn assert_contiguous_and_non_overlapping(placements: &[daw_core::Placement]) {
    let mut cursor = 0u64;
    for placement in placements {
        assert_eq!(
            placement.start_pulse, cursor,
            "placements must sit flush with no gap or overlap: {placements:?}"
        );
        let length = placement
            .notes
            .iter()
            .map(|n| n.end_pulse)
            .max()
            .unwrap_or(0);
        cursor += length;
    }
}

#[test]
fn inserting_at_the_start_pushes_every_existing_placement_later() {
    let mut core = DawCore::new();
    arrange_three(&mut core);
    let d = record_and_freeze(&mut core, 67, 2);

    core.apply(Command::InsertPlacementAt {
        block_id: d,
        track: 0,
        index: 0,
    });

    let ordered = ordered_track(&core, 0);
    let names: Vec<_> = ordered.iter().map(|p| p.name.clone()).collect();
    assert_eq!(names, vec!["Take 4", "Take 1", "Take 2", "Take 3"]);
    assert_eq!(ordered[0].start_pulse, 0);
    assert_eq!(ordered[1].start_pulse, 2); // pushed later by d's length (2)
    assert_eq!(ordered[2].start_pulse, 6);
    assert_eq!(ordered[3].start_pulse, 12);
    assert_contiguous_and_non_overlapping(&ordered);
}

#[test]
fn inserting_in_the_middle_pushes_only_the_remainder() {
    let mut core = DawCore::new();
    arrange_three(&mut core); // A(4) B(6) C(3), flush at 0, 4, 10
    let d = record_and_freeze(&mut core, 67, 2);

    // Insert between A and B (index 1).
    core.apply(Command::InsertPlacementAt {
        block_id: d,
        track: 0,
        index: 1,
    });

    let ordered = ordered_track(&core, 0);
    let names: Vec<_> = ordered.iter().map(|p| p.name.clone()).collect();
    assert_eq!(names, vec!["Take 1", "Take 4", "Take 2", "Take 3"]);
    assert_eq!(ordered[0].start_pulse, 0); // A untouched
    assert_eq!(ordered[1].start_pulse, 4); // D lands where B used to start
    assert_eq!(ordered[2].start_pulse, 6); // B pushed later by D's length
    assert_eq!(ordered[3].start_pulse, 12); // C pushed later too
    assert_contiguous_and_non_overlapping(&ordered);
}

#[test]
fn inserting_at_the_end_behaves_like_a_flush_append() {
    let mut core = DawCore::new();
    arrange_three(&mut core); // ends at pulse 13 (4 + 6 + 3)
    let d = record_and_freeze(&mut core, 67, 2);

    core.apply(Command::InsertPlacementAt {
        block_id: d,
        track: 0,
        index: 3, // the track's current length: "at the end"
    });

    let ordered = ordered_track(&core, 0);
    assert_eq!(ordered.len(), 4);
    assert_eq!(ordered[3].name, "Take 4");
    assert_eq!(ordered[3].start_pulse, 13);
    assert_contiguous_and_non_overlapping(&ordered);
}

#[test]
fn an_index_past_the_end_is_clamped_to_the_end() {
    let mut core = DawCore::new();
    arrange_three(&mut core);
    let d = record_and_freeze(&mut core, 67, 2);

    core.apply(Command::InsertPlacementAt {
        block_id: d,
        track: 0,
        index: 999,
    });

    let ordered = ordered_track(&core, 0);
    assert_eq!(ordered[3].name, "Take 4");
}

#[test]
fn reordering_a_placement_later_moves_it_and_ripples_the_rest_earlier() {
    let mut core = DawCore::new();
    arrange_three(&mut core); // A(4) B(6) C(3) at 0, 4, 10
    let a_id = ordered_track(&core, 0)[0].id;

    // Move A (currently first) to the end.
    core.apply(Command::ReorderPlacement {
        id: a_id,
        new_index: 2,
    });

    let ordered = ordered_track(&core, 0);
    let names: Vec<_> = ordered.iter().map(|p| p.name.clone()).collect();
    assert_eq!(names, vec!["Take 2", "Take 3", "Take 1"]);
    assert_eq!(ordered[0].start_pulse, 0); // B now first, ripples earlier
    assert_eq!(ordered[1].start_pulse, 6); // C follows B
    assert_eq!(ordered[2].start_pulse, 9); // A now last
    assert_contiguous_and_non_overlapping(&ordered);
}

#[test]
fn reordering_a_placement_earlier_moves_it_and_ripples_the_rest_later() {
    let mut core = DawCore::new();
    arrange_three(&mut core); // A(4) B(6) C(3) at 0, 4, 10
    let c_id = ordered_track(&core, 0)[2].id;

    // Move C (currently last) to the start.
    core.apply(Command::ReorderPlacement {
        id: c_id,
        new_index: 0,
    });

    let ordered = ordered_track(&core, 0);
    let names: Vec<_> = ordered.iter().map(|p| p.name.clone()).collect();
    assert_eq!(names, vec!["Take 3", "Take 1", "Take 2"]);
    assert_eq!(ordered[0].start_pulse, 0); // C now first
    assert_eq!(ordered[1].start_pulse, 3); // A ripples later by C's length
    assert_eq!(ordered[2].start_pulse, 7); // B ripples later too
    assert_contiguous_and_non_overlapping(&ordered);
}

#[test]
fn inserting_is_undoable_and_restores_the_exact_previous_positions() {
    let mut core = DawCore::new();
    arrange_three(&mut core);
    let before = ordered_track(&core, 0);
    let d = record_and_freeze(&mut core, 67, 2);

    core.apply(Command::InsertPlacementAt {
        block_id: d,
        track: 0,
        index: 1,
    });
    assert_ne!(ordered_track(&core, 0), before);

    core.apply(Command::Undo);
    assert_eq!(ordered_track(&core, 0), before);

    core.apply(Command::Redo);
    assert_eq!(ordered_track(&core, 0).len(), 4);
}

#[test]
fn reordering_is_undoable_and_restores_the_exact_previous_positions() {
    let mut core = DawCore::new();
    arrange_three(&mut core);
    let before = ordered_track(&core, 0);
    let a_id = before[0].id;

    core.apply(Command::ReorderPlacement {
        id: a_id,
        new_index: 2,
    });
    assert_ne!(ordered_track(&core, 0), before);

    let undone = core.apply(Command::Undo);
    assert_eq!(ordered_track(&core, 0), before);
    assert_eq!(undone.state.placements.len(), 3);

    core.apply(Command::Redo);
    let names: Vec<_> = ordered_track(&core, 0)
        .iter()
        .map(|p| p.name.clone())
        .collect();
    assert_eq!(names, vec!["Take 2", "Take 3", "Take 1"]);
}

#[test]
fn inserting_a_nonexistent_block_does_nothing() {
    let mut core = DawCore::new();
    arrange_three(&mut core);
    let before = ordered_track(&core, 0);

    core.apply(Command::InsertPlacementAt {
        block_id: 999,
        track: 0,
        index: 1,
    });

    assert_eq!(ordered_track(&core, 0), before);
}

#[test]
fn reordering_a_nonexistent_placement_does_nothing() {
    let mut core = DawCore::new();
    arrange_three(&mut core);
    let before = ordered_track(&core, 0);

    core.apply(Command::ReorderPlacement {
        id: 999,
        new_index: 0,
    });

    assert_eq!(ordered_track(&core, 0), before);
}
