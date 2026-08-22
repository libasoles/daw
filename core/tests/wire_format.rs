//! `Command` and `Applied` cross the Tauri IPC boundary as JSON (see
//! `src-tauri/src/lib.rs`), and the frontend hand-mirrors their shape in
//! TypeScript. These tests pin that shape down so a change here is a visible,
//! deliberate decision rather than a silent break of the frontend.

use daw_core::{Applied, Command, Effect, ProjectState, Quantisation, RecordedNote, Take, Trim};

#[test]
fn commands_serialise_as_an_adjacently_tagged_object() {
    assert_eq!(
        serde_json::to_value(Command::NewProject { force: false }).unwrap(),
        serde_json::json!({ "type": "newProject", "payload": { "force": false } })
    );
    assert_eq!(
        serde_json::to_value(Command::SetBpm(140)).unwrap(),
        serde_json::json!({ "type": "setBpm", "payload": 140 })
    );
    assert_eq!(
        serde_json::to_value(Command::SetTimeSignature {
            beats_per_bar: 4,
            beat_unit: 4
        })
        .unwrap(),
        serde_json::json!({
            "type": "setTimeSignature",
            "payload": { "beats_per_bar": 4, "beat_unit": 4 }
        })
    );
    assert_eq!(
        serde_json::to_value(Command::SetInstrument(1)).unwrap(),
        serde_json::json!({ "type": "setInstrument", "payload": 1 })
    );
    assert_eq!(
        serde_json::to_value(Command::SetReverb(75)).unwrap(),
        serde_json::json!({ "type": "setReverb", "payload": 75 })
    );
    assert_eq!(
        serde_json::to_value(Command::SetMetronomeEnabled(true)).unwrap(),
        serde_json::json!({ "type": "setMetronomeEnabled", "payload": true })
    );
    assert_eq!(
        serde_json::to_value(Command::SetCountInEnabled(false)).unwrap(),
        serde_json::json!({ "type": "setCountInEnabled", "payload": false })
    );
    assert_eq!(
        serde_json::to_value(Command::StartRecording { force: false }).unwrap(),
        serde_json::json!({ "type": "startRecording", "payload": { "force": false } })
    );
    assert_eq!(
        serde_json::to_value(Command::SetTakeTrim(Trim {
            start_pulse: 2,
            end_pulse: 6
        }))
        .unwrap(),
        serde_json::json!({ "type": "setTakeTrim", "payload": { "start_pulse": 2, "end_pulse": 6 } })
    );
    assert_eq!(
        serde_json::to_value(Command::SetTakeQuantisation(Quantisation::Quarter)).unwrap(),
        serde_json::json!({ "type": "setTakeQuantisation", "payload": "quarter" })
    );
    assert_eq!(
        serde_json::to_value(Command::StopRecording(Some(Take::from_raw_notes(vec![
            RecordedNote {
                pitch: 60,
                velocity: 100,
                start_pulse: 0,
                end_pulse: 4
            }
        ]))))
        .unwrap(),
        serde_json::json!({
            "type": "stopRecording",
            "payload": {
                "raw_notes": [{ "pitch": 60, "velocity": 100, "start_pulse": 0, "end_pulse": 4 }],
                "notes": [{ "pitch": 60, "velocity": 100, "start_pulse": 0, "end_pulse": 4 }],
                "trim": { "start_pulse": 0, "end_pulse": 4 },
                "quantisation": "off"
            }
        })
    );
    assert_eq!(
        serde_json::to_value(Command::PlayTake).unwrap(),
        serde_json::json!({ "type": "playTake" })
    );
    assert_eq!(
        serde_json::to_value(Command::PlaybackFinished).unwrap(),
        serde_json::json!({ "type": "playbackFinished" })
    );
    assert_eq!(
        serde_json::to_value(Command::Undo).unwrap(),
        serde_json::json!({ "type": "undo" })
    );
    assert_eq!(
        serde_json::to_value(Command::Redo).unwrap(),
        serde_json::json!({ "type": "redo" })
    );
}

#[test]
fn a_command_sent_from_the_frontend_as_json_deserialises_correctly() {
    let command: Command = serde_json::from_value(serde_json::json!({
        "type": "setBpm",
        "payload": 90
    }))
    .unwrap();

    assert_eq!(command, Command::SetBpm(90));
}

#[test]
fn applied_serialises_state_and_effects_for_the_frontend_to_render() {
    let applied = Applied {
        state: ProjectState {
            is_dirty: false,
            bpm: 140,
            time_signature: (4, 4),
            instrument: 1,
            reverb: 75,
            metronome_enabled: true,
            count_in_enabled: false,
            take: None,
            blocks: Vec::new(),
            next_block_id: 1,
            is_recording: false,
            is_playing: false,
        },
        effects: vec![Effect::NoMidiDeviceAvailable],
    };

    assert_eq!(
        serde_json::to_value(applied).unwrap(),
        serde_json::json!({
            "state": {
                "is_dirty": false,
                "bpm": 140,
                "time_signature": [4, 4],
                "instrument": 1,
                "reverb": 75,
                "metronome_enabled": true,
                "count_in_enabled": false,
                "take": null,
                "blocks": [],
                "next_block_id": 1,
                "is_recording": false,
                "is_playing": false
            },
            "effects": [{ "type": "noMidiDeviceAvailable" }]
        })
    );
}

#[test]
fn effects_that_carry_a_take_serialise_the_take_inline() {
    let take = Take::from_raw_notes(vec![RecordedNote {
        pitch: 60,
        velocity: 100,
        start_pulse: 0,
        end_pulse: 4,
    }]);

    assert_eq!(
        serde_json::to_value(Effect::PlaySchedule(take)).unwrap(),
        serde_json::json!({
            "type": "playSchedule",
            "raw_notes": [{ "pitch": 60, "velocity": 100, "start_pulse": 0, "end_pulse": 4 }],
            "notes": [{ "pitch": 60, "velocity": 100, "start_pulse": 0, "end_pulse": 4 }],
            "trim": { "start_pulse": 0, "end_pulse": 4 },
            "quantisation": "off"
        })
    );
    assert_eq!(
        serde_json::to_value(Effect::ConfirmOverwriteRecording).unwrap(),
        serde_json::json!({ "type": "confirmOverwriteRecording" })
    );
    assert_eq!(
        serde_json::to_value(Effect::ConfirmDiscardUnsavedChanges).unwrap(),
        serde_json::json!({ "type": "confirmDiscardUnsavedChanges" })
    );
}
