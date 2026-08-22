//! Issue #21: deleting a selected placement removes it and closes the gap
//! behind it, so the remainder of the track stays flush.

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
/// returns their placement ids in placement order.
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
    let after_c = core.apply(Command::AddPlacement {
        block_id: c,
        track: 0,
    });
    let ordered = ordered_track_from(&after_c.state, 0);
    [ordered[0].id, ordered[1].id, ordered[2].id]
}

fn ordered_track(core: &DawCore, track: u32) -> Vec<daw_core::Placement> {
    ordered_track_from(core.state(), track)
}

fn ordered_track_from(state: &daw_core::ProjectState, track: u32) -> Vec<daw_core::Placement> {
    let mut placements: Vec<_> = state
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
fn deleting_the_first_placement_closes_the_gap_at_the_start() {
    let mut core = DawCore::new();
    let [a_id, _b_id, _c_id] = arrange_three(&mut core); // A(4) B(6) C(3) at 0, 4, 10

    core.apply(Command::DeletePlacement(a_id));

    let ordered = ordered_track(&core, 0);
    let names: Vec<_> = ordered.iter().map(|p| p.name.clone()).collect();
    assert_eq!(names, vec!["Take 2", "Take 3"]);
    assert_eq!(ordered[0].start_pulse, 0); // B ripples to the start
    assert_eq!(ordered[1].start_pulse, 6); // C follows immediately after
    assert_contiguous_and_non_overlapping(&ordered);
}

#[test]
fn deleting_the_middle_placement_closes_the_gap_in_the_middle() {
    let mut core = DawCore::new();
    let [_a_id, b_id, _c_id] = arrange_three(&mut core); // A(4) B(6) C(3) at 0, 4, 10

    core.apply(Command::DeletePlacement(b_id));

    let ordered = ordered_track(&core, 0);
    let names: Vec<_> = ordered.iter().map(|p| p.name.clone()).collect();
    assert_eq!(names, vec!["Take 1", "Take 3"]);
    assert_eq!(ordered[0].start_pulse, 0); // A untouched
    assert_eq!(ordered[1].start_pulse, 4); // C ripples earlier to close B's gap
    assert_contiguous_and_non_overlapping(&ordered);
}

#[test]
fn deleting_the_last_placement_leaves_the_remainder_untouched() {
    let mut core = DawCore::new();
    let [_a_id, _b_id, c_id] = arrange_three(&mut core); // A(4) B(6) C(3) at 0, 4, 10

    core.apply(Command::DeletePlacement(c_id));

    let ordered = ordered_track(&core, 0);
    let names: Vec<_> = ordered.iter().map(|p| p.name.clone()).collect();
    assert_eq!(names, vec!["Take 1", "Take 2"]);
    assert_eq!(ordered[0].start_pulse, 0);
    assert_eq!(ordered[1].start_pulse, 4);
    assert_contiguous_and_non_overlapping(&ordered);
}

#[test]
fn deleting_is_undoable_and_restores_the_exact_previous_positions() {
    let mut core = DawCore::new();
    let [_a_id, b_id, _c_id] = arrange_three(&mut core);
    let before = ordered_track(&core, 0);

    core.apply(Command::DeletePlacement(b_id));
    assert_ne!(ordered_track(&core, 0), before);
    assert_eq!(ordered_track(&core, 0).len(), 2);

    let undone = core.apply(Command::Undo);
    assert_eq!(ordered_track(&core, 0), before);
    assert_eq!(undone.state.placements.len(), 3);

    core.apply(Command::Redo);
    let names: Vec<_> = ordered_track(&core, 0)
        .iter()
        .map(|p| p.name.clone())
        .collect();
    assert_eq!(names, vec!["Take 1", "Take 3"]);
}

#[test]
fn deleting_a_nonexistent_placement_does_nothing() {
    let mut core = DawCore::new();
    arrange_three(&mut core);
    let before = ordered_track(&core, 0);

    core.apply(Command::DeletePlacement(999));

    assert_eq!(ordered_track(&core, 0), before);
}

#[test]
fn deleting_every_placement_leaves_the_track_empty() {
    let mut core = DawCore::new();
    let [a_id, b_id, c_id] = arrange_three(&mut core);

    core.apply(Command::DeletePlacement(a_id));
    core.apply(Command::DeletePlacement(b_id));
    let applied = core.apply(Command::DeletePlacement(c_id));

    assert!(ordered_track(&core, 0).is_empty());
    assert!(applied.state.placements.is_empty());
}
