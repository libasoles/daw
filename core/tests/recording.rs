//! Issue #8: recording a take and playing it back in isolation. The MIDI
//! side is driven through the same `ScriptedMidiInput` port other tickets
//! use (see `midi_input_port.rs`), so these tests never touch a keyboard.

use std::sync::mpsc;
use std::time::Duration;

use daw_core::ports::{MidiDevice, MidiEvent, MidiEventKind, MidiInput, ScriptedMidiInput};
use daw_core::{build_take, CapturedEvent, Command, DawCore, Effect, RecordedNote, Take};

/// Replays `events` through a `ScriptedMidiInput` and collects them as
/// `CapturedEvent`s relative to `started`, the same shape the real shell
/// builds from `route_live_event`'s `Instant` timestamps.
fn capture_via_scripted_input(events: Vec<MidiEvent>) -> Vec<CapturedEvent> {
    let device = MidiDevice {
        id: "scripted-keyboard".into(),
        name: "Scripted keyboard".into(),
    };
    let mut input = ScriptedMidiInput::new(device.clone(), events.clone());

    let (sent, received) = mpsc::channel();
    input
        .subscribe(&device.id, Box::new(move |event| sent.send(event).unwrap()))
        .unwrap();

    (0..events.len())
        .map(|_| {
            let event = received.recv_timeout(Duration::from_secs(5)).unwrap();
            let (is_on, pitch, velocity) = match event.kind {
                MidiEventKind::NoteOn { pitch, velocity } => (true, pitch, velocity),
                MidiEventKind::NoteOff { pitch } => (false, pitch, 0),
            };
            CapturedEvent {
                elapsed: event.timestamp,
                pitch,
                velocity,
                is_on,
            }
        })
        .collect()
}

#[test]
fn a_scripted_note_on_and_off_becomes_one_recorded_note() {
    let events = vec![
        MidiEvent {
            timestamp: Duration::ZERO,
            kind: MidiEventKind::NoteOn {
                pitch: 60,
                velocity: 91,
            },
        },
        MidiEvent {
            timestamp: Duration::from_secs(1),
            kind: MidiEventKind::NoteOff { pitch: 60 },
        },
    ];
    let captured = capture_via_scripted_input(events);

    let take = build_take(&captured, Duration::from_secs(2), 120);

    assert_eq!(
        take,
        Take::from_raw_notes(vec![RecordedNote {
            pitch: 60,
            velocity: 91,
            start_pulse: 0,
            end_pulse: 4,
        }])
    );
}

#[test]
fn a_note_still_held_at_stop_is_captured_as_ending_there() {
    let held = CapturedEvent {
        elapsed: Duration::from_secs(1),
        pitch: 64,
        velocity: 80,
        is_on: true,
    };

    let take = build_take(&[held], Duration::from_secs(3), 120);

    assert_eq!(
        take,
        Take::from_raw_notes(vec![RecordedNote {
            pitch: 64,
            velocity: 80,
            start_pulse: 4,
            end_pulse: 12,
        }])
    );
}

#[test]
fn tempo_governs_how_captured_wall_clock_time_maps_to_pulses() {
    let events = [
        CapturedEvent {
            elapsed: Duration::from_secs(1),
            pitch: 60,
            velocity: 100,
            is_on: true,
        },
        CapturedEvent {
            elapsed: Duration::from_secs(2),
            pitch: 60,
            velocity: 100,
            is_on: false,
        },
    ];

    let at_120_bpm = build_take(&events, Duration::from_secs(3), 120);
    let at_60_bpm = build_take(&events, Duration::from_secs(3), 60);

    assert_eq!(at_120_bpm.notes()[0].start_pulse, 4);
    assert_eq!(at_120_bpm.notes()[0].end_pulse, 8);
    assert_eq!(at_60_bpm.notes()[0].start_pulse, 2);
    assert_eq!(at_60_bpm.notes()[0].end_pulse, 4);
}

#[test]
fn stop_recording_replaces_the_take_and_is_undoable() {
    let mut core = DawCore::new();
    core.apply(Command::StartRecording);

    let take = Take::from_raw_notes(vec![RecordedNote {
        pitch: 60,
        velocity: 100,
        start_pulse: 0,
        end_pulse: 4,
    }]);
    let applied = core.apply(Command::StopRecording(Some(take.clone())));

    assert_eq!(applied.state.take, Some(take));
    assert!(!applied.state.is_recording);

    let undone = core.apply(Command::Undo);
    assert_eq!(undone.state.take, None);

    let redone = core.apply(Command::Redo);
    assert_eq!(
        redone.state.take,
        Some(Take::from_raw_notes(vec![RecordedNote {
            pitch: 60,
            velocity: 100,
            start_pulse: 0,
            end_pulse: 4,
        }]))
    );
}

#[test]
fn starting_recording_over_an_existing_take_replaces_it_without_confirmation() {
    let mut core = DawCore::new();
    let take = Take::from_raw_notes(vec![RecordedNote {
        pitch: 60,
        velocity: 100,
        start_pulse: 0,
        end_pulse: 4,
    }]);
    core.apply(Command::StartRecording);
    core.apply(Command::StopRecording(Some(take.clone())));

    let applied = core.apply(Command::StartRecording);
    assert_eq!(applied.effects, Vec::<Effect>::new());
    assert!(applied.state.is_recording);
    // The take stays in place until `StopRecording` overwrites it — arming
    // is not itself the moment the previous take disappears.
    assert_eq!(applied.state.take, Some(take));
}

#[test]
fn starting_recording_with_no_existing_take_arms_it() {
    let mut core = DawCore::new();

    let applied = core.apply(Command::StartRecording);

    assert_eq!(applied.effects, Vec::<Effect>::new());
    assert!(applied.state.is_recording);
}

#[test]
fn playing_a_take_reports_a_schedule_and_marks_playback_active() {
    let mut core = DawCore::new();
    let take = Take::from_raw_notes(vec![RecordedNote {
        pitch: 60,
        velocity: 100,
        start_pulse: 0,
        end_pulse: 4,
    }]);
    core.apply(Command::StartRecording);
    core.apply(Command::StopRecording(Some(take.clone())));

    let applied = core.apply(Command::PlayTake);

    assert_eq!(applied.effects, vec![Effect::PlaySchedule(take)]);
    assert!(applied.state.is_playing);

    let finished = core.apply(Command::PlaybackFinished);
    assert!(!finished.state.is_playing);
}

#[test]
fn playing_an_empty_recording_area_does_nothing() {
    let mut core = DawCore::new();

    let applied = core.apply(Command::PlayTake);

    assert_eq!(applied.effects, Vec::<Effect>::new());
    assert!(!applied.state.is_playing);
}
