//! Issue #23: deleting a library block used on the timeline asks for
//! confirmation first, then — once confirmed — removes the block and every
//! placement copied from it, closing the gaps they leave, as one undo step.

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

#[test]
fn deleting_an_unused_block_removes_it_without_asking() {
    let mut core = DawCore::new();
    let id = record_and_freeze(&mut core, 60, 4);

    let applied = core.apply(Command::DeleteBlock { id, force: false });

    assert_eq!(applied.effects, Vec::<Effect>::new());
    assert!(applied.state.blocks.is_empty());
}

#[test]
fn deleting_a_block_in_use_asks_for_confirmation_and_changes_nothing() {
    let mut core = DawCore::new();
    let id = record_and_freeze(&mut core, 60, 4);
    core.apply(Command::AddPlacement {
        block_id: id,
        track: 0,
    });
    core.apply(Command::AddPlacement {
        block_id: id,
        track: 0,
    });

    let applied = core.apply(Command::DeleteBlock { id, force: false });

    assert_eq!(
        applied.effects,
        vec![Effect::ConfirmDeleteBlockInUse { uses: 2 }]
    );
    // Cancelling (never resending with force) changes nothing at all.
    assert_eq!(applied.state.blocks.len(), 1);
    assert_eq!(applied.state.placements.len(), 2);
}

#[test]
fn confirming_removes_the_block_every_placement_and_closes_the_gap() {
    let mut core = DawCore::new();
    let a = record_and_freeze(&mut core, 60, 4); // block to delete
    let b = record_and_freeze(&mut core, 62, 6);
    core.apply(Command::AddPlacement {
        block_id: a,
        track: 0,
    }); // 0..4
    core.apply(Command::AddPlacement {
        block_id: b,
        track: 0,
    }); // 4..10
    core.apply(Command::AddPlacement {
        block_id: a,
        track: 0,
    }); // 10..14 (second use of A)

    core.apply(Command::DeleteBlock {
        id: a,
        force: false,
    }); // refused, just confirms
    let applied = core.apply(Command::DeleteBlock { id: a, force: true });

    assert_eq!(applied.effects, Vec::<Effect>::new());
    assert!(applied.state.blocks.iter().all(|block| block.id != a));

    let ordered = ordered_track(&core, 0);
    assert_eq!(ordered.len(), 1);
    assert_eq!(ordered[0].block_id, b);
    assert_eq!(ordered[0].start_pulse, 0); // B ripples to the front, closing both gaps
}

#[test]
fn deliberate_gaps_elsewhere_survive_deleting_a_block_in_use() {
    let mut core = DawCore::new();
    let a = record_and_freeze(&mut core, 60, 4); // to delete
    let b = record_and_freeze(&mut core, 62, 6);
    let c = record_and_freeze(&mut core, 64, 3);
    core.apply(Command::AddPlacement {
        block_id: a,
        track: 0,
    }); // 0..4
    core.apply(Command::AddPlacement {
        block_id: b,
        track: 0,
    }); // 4..10
    let c_id = core
        .apply(Command::AddPlacement {
            block_id: c,
            track: 0,
        })
        .state
        .placements
        .iter()
        .find(|p| p.block_id == c)
        .unwrap()
        .id;
    core.apply(Command::SetPlacementGap {
        id: c_id,
        gap_before: 5,
    }); // A(4) B(6) [gap 5] C(3) -> starts 0, 4, 15

    core.apply(Command::DeleteBlock { id: a, force: true });

    let ordered = ordered_track(&core, 0);
    let block_ids: Vec<_> = ordered.iter().map(|p| p.block_id).collect();
    assert_eq!(block_ids, vec![b, c]);
    assert_eq!(ordered[0].start_pulse, 0); // B, ripples to the front
    assert_eq!(ordered[1].gap_before, 5); // C's own deliberate gap untouched...
    assert_eq!(ordered[1].start_pulse, 11); // ...so it still holds: 6 (B's end) + 5
}

#[test]
fn deleting_a_block_in_use_across_two_tracks_closes_both_gaps_independently() {
    let mut core = DawCore::new();
    let a = record_and_freeze(&mut core, 60, 4); // to delete
    let b = record_and_freeze(&mut core, 62, 6);
    core.apply(Command::AddPlacement {
        block_id: a,
        track: 0,
    }); // track 0: A(4), B(6)
    core.apply(Command::AddPlacement {
        block_id: b,
        track: 0,
    });
    core.apply(Command::AddPlacement {
        block_id: b,
        track: 1,
    }); // track 1: B(6), A(4)
    core.apply(Command::AddPlacement {
        block_id: a,
        track: 1,
    });

    core.apply(Command::DeleteBlock { id: a, force: true });

    let track0 = ordered_track(&core, 0);
    assert_eq!(track0.len(), 1);
    assert_eq!(track0[0].block_id, b);
    assert_eq!(track0[0].start_pulse, 0);

    let track1 = ordered_track(&core, 1);
    assert_eq!(track1.len(), 1);
    assert_eq!(track1[0].block_id, b);
    assert_eq!(track1[0].start_pulse, 0);
}

#[test]
fn deleting_a_block_in_use_is_a_single_undo_step() {
    let mut core = DawCore::new();
    let a = record_and_freeze(&mut core, 60, 4);
    let b = record_and_freeze(&mut core, 62, 6);
    core.apply(Command::AddPlacement {
        block_id: a,
        track: 0,
    });
    core.apply(Command::AddPlacement {
        block_id: b,
        track: 0,
    });
    core.apply(Command::AddPlacement {
        block_id: a,
        track: 0,
    });
    let mut before_blocks = core.state().blocks.clone();
    before_blocks.sort_by_key(|block| block.id);
    let before_placements = ordered_track(&core, 0);

    core.apply(Command::DeleteBlock { id: a, force: true });
    assert_eq!(core.state().blocks.len(), 1);
    assert_eq!(core.state().placements.len(), 1);

    let undone = core.apply(Command::Undo);
    let mut restored_blocks = undone.state.blocks.clone();
    restored_blocks.sort_by_key(|block| block.id);
    // Library order isn't a tracked invariant (undoing a plain `DeleteBlock`
    // has never restored a block to its original list index either — see
    // `RemoveBlock`/`InsertBlock`), only that the exact same block is back.
    assert_eq!(restored_blocks, before_blocks);
    assert_eq!(ordered_track(&core, 0), before_placements);

    // A single Undo was enough: nothing further to undo about this deletion.
    let redone = core.apply(Command::Redo);
    assert_eq!(redone.state.blocks.len(), 1);
    assert_eq!(redone.state.placements.len(), 1);
}

#[test]
fn deleting_a_nonexistent_block_does_nothing() {
    let mut core = DawCore::new();
    let before = core.state().clone();

    let applied = core.apply(Command::DeleteBlock {
        id: 999,
        force: true,
    });

    assert_eq!(applied.effects, Vec::<Effect>::new());
    assert_eq!(applied.state.blocks, before.blocks);
    assert_eq!(applied.state.placements, before.placements);
}
