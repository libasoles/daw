//! `Command` and `Applied` cross the Tauri IPC boundary as JSON (see
//! `src-tauri/src/lib.rs`), and the frontend hand-mirrors their shape in
//! TypeScript. These tests pin that shape down so a change here is a visible,
//! deliberate decision rather than a silent break of the frontend.

use daw_core::{Applied, Command, Effect, ProjectState};

#[test]
fn commands_serialise_as_an_adjacently_tagged_object() {
    assert_eq!(
        serde_json::to_value(Command::NewProject).unwrap(),
        serde_json::json!({ "type": "newProject" })
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
            bpm: 140,
            time_signature: (4, 4),
        },
        effects: vec![Effect::NothingToUndo],
    };

    assert_eq!(
        serde_json::to_value(applied).unwrap(),
        serde_json::json!({
            "state": { "bpm": 140, "time_signature": [4, 4] },
            "effects": [{ "type": "nothingToUndo" }]
        })
    );
}
