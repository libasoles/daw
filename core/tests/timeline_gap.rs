//! Issue #22: a deliberate silence held before a placement, set with
//! `Command::SetPlacementGap`, is stored on the placement itself so it
//! survives insertions, reorders and deletions elsewhere on the track, and
//! plays back as the expected span of silence.

use daw_core::{Command, DawCore, Effect, RecordedNote, Take};

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

#[test]
fn setting_a_gap_pushes_the_placement_and_everything_after_it_later() {
    let mut core = DawCore::new();
    let [_a_id, b_id, _c_id] = arrange_three(&mut core); // A(4) B(6) C(3) at 0, 4, 10

    core.apply(Command::SetPlacementGap {
        id: b_id,
        gap_before: 3,
    });

    let ordered = ordered_track(&core, 0);
    assert_eq!(ordered[0].start_pulse, 0); // A untouched
    assert_eq!(ordered[0].gap_before, 0);
    assert_eq!(ordered[1].start_pulse, 7); // B: 4 (A's end) + 3 (gap)
    assert_eq!(ordered[1].gap_before, 3);
    assert_eq!(ordered[2].start_pulse, 13); // C follows B flush (7 + 6)
    assert_eq!(ordered[2].gap_before, 0);
}

#[test]
fn a_gap_on_the_first_placement_delays_the_whole_track() {
    let mut core = DawCore::new();
    let [a_id, _b_id, _c_id] = arrange_three(&mut core);

    core.apply(Command::SetPlacementGap {
        id: a_id,
        gap_before: 5,
    });

    let ordered = ordered_track(&core, 0);
    assert_eq!(ordered[0].start_pulse, 5);
    assert_eq!(ordered[1].start_pulse, 9);
    assert_eq!(ordered[2].start_pulse, 15);
}

#[test]
fn a_gap_survives_an_insertion_before_it() {
    let mut core = DawCore::new();
    let [_a_id, b_id, _c_id] = arrange_three(&mut core); // A(4) B(6) C(3) at 0, 4, 10
    core.apply(Command::SetPlacementGap {
        id: b_id,
        gap_before: 3,
    }); // A(4) [gap 3] B(6) C(3) -> starts 0, 7, 13

    let d = record_and_freeze(&mut core, 67, 2);
    core.apply(Command::InsertPlacementAt {
        block_id: d,
        track: 0,
        index: 0, // insert D before A
    });

    let ordered = ordered_track(&core, 0);
    let names: Vec<_> = ordered.iter().map(|p| p.name.clone()).collect();
    assert_eq!(names, vec!["Take 4", "Take 1", "Take 2", "Take 3"]);
    assert_eq!(ordered[0].start_pulse, 0); // D
    assert_eq!(ordered[1].start_pulse, 2); // A, pushed later by D's length
    assert_eq!(ordered[2].gap_before, 3); // B's gap is untouched...
    assert_eq!(ordered[2].start_pulse, 9); // ...so it still holds: 6 (A's end) + 3
    assert_eq!(ordered[3].start_pulse, 15); // C follows B flush
}

#[test]
fn a_gap_survives_a_deletion_before_it() {
    let mut core = DawCore::new();
    let [a_id, b_id, _c_id] = arrange_three(&mut core); // A(4) B(6) C(3) at 0, 4, 10
    core.apply(Command::SetPlacementGap {
        id: b_id,
        gap_before: 3,
    }); // A(4) [gap 3] B(6) C(3) -> starts 0, 7, 13

    core.apply(Command::DeletePlacement(a_id));

    let ordered = ordered_track(&core, 0);
    let names: Vec<_> = ordered.iter().map(|p| p.name.clone()).collect();
    assert_eq!(names, vec!["Take 2", "Take 3"]);
    assert_eq!(ordered[0].gap_before, 3); // B's own gap is untouched...
    assert_eq!(ordered[0].start_pulse, 3); // ...so it still opens the track: 0 + 3
    assert_eq!(ordered[1].start_pulse, 9); // C follows B flush
}

#[test]
fn a_gap_survives_a_reorder_elsewhere() {
    let mut core = DawCore::new();
    let [a_id, b_id, c_id] = arrange_three(&mut core); // A(4) B(6) C(3) at 0, 4, 10
    core.apply(Command::SetPlacementGap {
        id: c_id,
        gap_before: 2,
    }); // A(4) B(6) [gap 2] C(3) -> starts 0, 4, 12

    // Move A (currently first) to the end; C's gap should follow it wherever
    // it lands, since the gap is C's own stored property.
    core.apply(Command::ReorderPlacement {
        id: a_id,
        new_index: 2,
    });

    let ordered = ordered_track(&core, 0);
    let names: Vec<_> = ordered.iter().map(|p| p.name.clone()).collect();
    assert_eq!(names, vec!["Take 2", "Take 3", "Take 1"]);
    assert_eq!(ordered[0].start_pulse, 0); // B now first
    assert_eq!(ordered[1].gap_before, 2); // C's gap is untouched...
    assert_eq!(ordered[1].start_pulse, 8); // ...so it still holds: 6 (B's end) + 2
    assert_eq!(ordered[2].start_pulse, 11); // A now last, follows C flush
    assert_eq!(ordered[2].gap_before, 0); // A never had a gap of its own
    assert_eq!(b_id, ordered[0].id);
}

#[test]
fn setting_a_gap_is_undoable_and_restores_the_exact_previous_positions() {
    let mut core = DawCore::new();
    let [_a_id, b_id, _c_id] = arrange_three(&mut core);
    let before = ordered_track(&core, 0);

    core.apply(Command::SetPlacementGap {
        id: b_id,
        gap_before: 3,
    });
    assert_ne!(ordered_track(&core, 0), before);

    let undone = core.apply(Command::Undo);
    assert_eq!(ordered_track(&core, 0), before);
    assert_eq!(undone.state.placements.len(), 3);

    core.apply(Command::Redo);
    assert_eq!(ordered_track(&core, 0)[1].gap_before, 3);
}

#[test]
fn changing_an_existing_gap_is_undoable() {
    let mut core = DawCore::new();
    let [_a_id, b_id, _c_id] = arrange_three(&mut core);
    core.apply(Command::SetPlacementGap {
        id: b_id,
        gap_before: 3,
    });
    let with_first_gap = ordered_track(&core, 0);

    core.apply(Command::SetPlacementGap {
        id: b_id,
        gap_before: 8,
    });
    assert_ne!(ordered_track(&core, 0), with_first_gap);
    assert_eq!(ordered_track(&core, 0)[1].gap_before, 8);

    core.apply(Command::Undo);
    assert_eq!(ordered_track(&core, 0), with_first_gap);
}

#[test]
fn setting_a_gap_on_a_nonexistent_placement_does_nothing() {
    let mut core = DawCore::new();
    arrange_three(&mut core);
    let before = ordered_track(&core, 0);

    core.apply(Command::SetPlacementGap {
        id: 999,
        gap_before: 5,
    });

    assert_eq!(ordered_track(&core, 0), before);
}

#[test]
fn playing_the_timeline_renders_a_gap_as_silence_of_the_expected_length() {
    let mut core = DawCore::new();
    let [_a_id, b_id, _c_id] = arrange_three(&mut core); // A(4) B(6) C(3) at 0, 4, 10

    core.apply(Command::SetPlacementGap {
        id: b_id,
        gap_before: 3,
    }); // A(4) [silence 3] B(6) C(3) -> starts 0, 7, 13

    let applied = core.apply(Command::PlayTimeline);
    let Effect::PlaySchedule(take) = &applied.effects[0] else {
        panic!("expected a PlaySchedule effect, got {:?}", applied.effects);
    };
    let mut notes = take.notes();
    notes.sort_by_key(|note| (note.start_pulse, note.pitch));

    // A's last note ends at pulse 4; B's first note doesn't start until
    // pulse 7 — a silent gap of exactly 3 pulses, matching `gap_before`.
    assert_eq!(notes[0].pitch, 60); // A
    assert_eq!(notes[0].end_pulse, 4);
    assert_eq!(notes[1].pitch, 62); // B
    assert_eq!(notes[1].start_pulse, 7);
    assert_eq!(notes[1].start_pulse - notes[0].end_pulse, 3);
}
