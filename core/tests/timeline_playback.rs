//! Issue #18: playing the timeline turns every track's placements into one
//! immutable note stream, handed to the shell as a single `PlaySchedule`
//! effect exactly like `PlayTake`/`PlayBlock` already do.

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

#[test]
fn playing_the_timeline_produces_the_exact_note_stream_for_the_arrangement() {
    let mut core = DawCore::new();
    let first = record_and_freeze(&mut core, 60, 4);
    let second = record_and_freeze(&mut core, 67, 6);
    core.apply(Command::AddPlacement {
        block_id: first,
        track: 0,
    });
    core.apply(Command::AddPlacement {
        block_id: second,
        track: 0,
    });
    // A second track's placement plays too -- the whole arrangement, not
    // just track 0.
    core.apply(Command::AddPlacement {
        block_id: first,
        track: 1,
    });

    let applied = core.apply(Command::PlayTimeline);

    assert!(applied.state.is_playing);
    let Effect::PlaySchedule(take) = &applied.effects[0] else {
        panic!("expected a PlaySchedule effect, got {:?}", applied.effects);
    };
    let mut notes = take.notes();
    notes.sort_by_key(|note| (note.start_pulse, note.pitch));
    assert_eq!(
        notes,
        vec![
            RecordedNote {
                pitch: 60,
                velocity: 100,
                start_pulse: 0,
                end_pulse: 4,
            },
            RecordedNote {
                pitch: 60,
                velocity: 100,
                start_pulse: 0,
                end_pulse: 4,
            },
            RecordedNote {
                pitch: 67,
                velocity: 100,
                start_pulse: 4,
                end_pulse: 10,
            },
        ]
    );
}

#[test]
fn playing_an_empty_timeline_does_nothing() {
    let mut core = DawCore::new();

    let applied = core.apply(Command::PlayTimeline);

    assert_eq!(applied.effects, Vec::<Effect>::new());
    assert!(!applied.state.is_playing);
}

#[test]
fn timeline_playback_is_mutually_exclusive_with_take_and_block_playback() {
    let mut core = DawCore::new();
    let block_id = record_and_freeze(&mut core, 60, 4);
    core.apply(Command::AddPlacement { block_id, track: 0 });

    // Take playback blocks timeline playback...
    core.apply(Command::StopRecording(Some(Take::from_raw_notes(vec![
        RecordedNote {
            pitch: 72,
            velocity: 100,
            start_pulse: 0,
            end_pulse: 2,
        },
    ]))));
    core.apply(Command::PlayTake);
    let refused = core.apply(Command::PlayTimeline);
    assert_eq!(refused.effects, Vec::<Effect>::new());
    core.apply(Command::PlaybackFinished);

    // ...and timeline playback blocks take/block playback, in turn.
    core.apply(Command::PlayTimeline);
    let take_refused = core.apply(Command::PlayTake);
    assert_eq!(take_refused.effects, Vec::<Effect>::new());
    let block_refused = core.apply(Command::PlayBlock(0));
    assert_eq!(block_refused.effects, Vec::<Effect>::new());
}

#[test]
fn playback_finished_after_the_timeline_marks_playback_inactive() {
    let mut core = DawCore::new();
    let block_id = record_and_freeze(&mut core, 60, 4);
    core.apply(Command::AddPlacement { block_id, track: 0 });

    core.apply(Command::PlayTimeline);
    assert!(core.state().is_playing);

    let finished = core.apply(Command::PlaybackFinished);
    assert!(!finished.state.is_playing);
}
