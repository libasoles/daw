//! MIDI input is tested through the public port, with no keyboard or native
//! service. The scripted adapter is intentionally the same API the shell uses.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use daw_core::ports::{MidiDevice, MidiEvent, MidiEventKind, MidiInput, ScriptedMidiInput};
use daw_core::{DawCore, Effect};

#[test]
fn scripted_input_replays_timestamped_note_events_in_order() {
    let device = MidiDevice {
        id: "scripted-keyboard".into(),
        name: "Scripted keyboard".into(),
    };
    let events = vec![
        MidiEvent {
            timestamp: Duration::ZERO,
            kind: MidiEventKind::NoteOn {
                pitch: 60,
                velocity: 91,
            },
        },
        MidiEvent {
            timestamp: Duration::from_millis(10),
            kind: MidiEventKind::NoteOff { pitch: 60 },
        },
    ];
    let mut input = ScriptedMidiInput::new(device.clone(), events.clone());
    assert_eq!(input.devices().unwrap(), vec![device.clone()]);

    let (sent, received) = mpsc::channel();
    let started = Instant::now();
    input
        .subscribe(&device.id, Box::new(move |event| sent.send(event).unwrap()))
        .unwrap();

    assert_eq!(
        received.recv_timeout(Duration::from_secs(1)).unwrap(),
        events[0]
    );
    assert_eq!(
        received.recv_timeout(Duration::from_secs(1)).unwrap(),
        events[1]
    );
    assert!(started.elapsed() >= Duration::from_millis(10));
}

#[test]
fn missing_midi_device_is_reported_without_changing_project_state() {
    let core = DawCore::new();

    let applied = core.no_midi_device_available();

    assert_eq!(applied.state, *core.state());
    assert_eq!(applied.effects, vec![Effect::NoMidiDeviceAvailable]);
}
