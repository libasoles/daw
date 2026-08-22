//! The editing seam for issues #9 and #10: commands produce a derived view
//! while the captured notes remain intact in the take.

use daw_core::{Command, DawCore, Effect, Quantisation, RecordedNote, Take, Trim};

fn recorded_take() -> Take {
    Take::from_raw_notes(vec![
        RecordedNote {
            pitch: 60,
            velocity: 100,
            start_pulse: 0,
            end_pulse: 4,
        },
        RecordedNote {
            pitch: 64,
            velocity: 90,
            start_pulse: 3,
            end_pulse: 8,
        },
    ])
}

#[test]
fn trimming_is_a_reversible_view_that_keeps_notes_crossing_its_edges() {
    let take = recorded_take();
    let raw_notes = take.raw_notes.clone();
    let mut core = DawCore::new();
    core.apply(Command::StopRecording(Some(take)));

    let trimmed = core.apply(Command::SetTakeTrim(Trim {
        start_pulse: 2,
        end_pulse: 6,
    }));
    let take = trimmed.state.take.unwrap();

    assert_eq!(take.raw_notes, raw_notes);
    assert_eq!(
        take.notes(),
        vec![
            RecordedNote {
                pitch: 60,
                velocity: 100,
                start_pulse: 2,
                end_pulse: 4,
            },
            RecordedNote {
                pitch: 64,
                velocity: 90,
                start_pulse: 3,
                end_pulse: 6,
            },
        ]
    );

    let restored = core.apply(Command::SetTakeTrim(Trim {
        start_pulse: 0,
        end_pulse: 8,
    }));
    assert_eq!(restored.state.take.unwrap().notes(), raw_notes);
}

#[test]
fn quantisation_rederives_from_raw_notes_and_composes_with_trim() {
    let mut core = DawCore::new();
    core.apply(Command::StopRecording(Some(Take::from_raw_notes(vec![
        RecordedNote {
            pitch: 67,
            velocity: 100,
            start_pulse: 1,
            end_pulse: 7,
        },
    ]))));

    core.apply(Command::SetTakeTrim(Trim {
        start_pulse: 2,
        end_pulse: 6,
    }));
    let quantised = core.apply(Command::SetTakeQuantisation(Quantisation::Whole));
    let take = quantised.state.take.unwrap();

    assert_eq!(take.raw_notes[0].start_pulse, 1);
    assert_eq!(take.notes()[0].start_pulse, 2);
    assert_eq!(take.notes()[0].end_pulse, 6);

    let playback = core.apply(Command::PlayTake);
    assert_eq!(playback.effects, vec![Effect::PlaySchedule(take.clone())]);

    let restored = core.apply(Command::SetTakeQuantisation(Quantisation::Off));
    let restored_take = restored.state.take.unwrap();
    assert_eq!(restored_take.notes()[0].start_pulse, 2);
    assert_eq!(restored_take.notes()[0].end_pulse, 6);

    let undone = core.apply(Command::Undo);
    assert_eq!(undone.state.take.unwrap().quantisation, Quantisation::Whole);
}

#[test]
fn quantising_an_untrimmed_take_and_setting_it_back_to_off_restores_the_original_timing_exactly() {
    let take = recorded_take();
    let raw_notes = take.raw_notes.clone();
    let mut core = DawCore::new();
    core.apply(Command::StopRecording(Some(take)));

    core.apply(Command::SetTakeQuantisation(Quantisation::Whole));
    let restored = core.apply(Command::SetTakeQuantisation(Quantisation::Off));

    assert_eq!(restored.state.take.unwrap().notes(), raw_notes);
}

#[test]
fn eighth_quantisation_uses_a_finer_grid_than_quarter_quantisation() {
    let mut core = DawCore::new();
    core.apply(Command::StopRecording(Some(Take::from_raw_notes(vec![
        RecordedNote {
            pitch: 72,
            velocity: 100,
            start_pulse: 1,
            end_pulse: 3,
        },
    ]))));

    let quarter = core.apply(Command::SetTakeQuantisation(Quantisation::Quarter));
    assert_eq!(quarter.state.take.unwrap().notes()[0].start_pulse, 2);

    let eighth = core.apply(Command::SetTakeQuantisation(Quantisation::Eighth));
    assert_eq!(eighth.state.take.unwrap().notes()[0].start_pulse, 1);
}
