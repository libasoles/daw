//! Issue #24: exporting the timeline writes a standard MIDI file whose
//! notes, tempo and time signature match playback exactly — trims,
//! quantisation and deliberate gaps included, since they're already
//! resolved into each placement's stored notes and position by the time
//! export runs.

use daw_core::{Command, DawCore, Effect, RecordedNote, Take, Trim};

const TICKS_PER_PULSE: u64 = 240; // 480 ticks/quarter note / 2 pulses/quarter note.

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

fn export(core: &mut DawCore) -> Vec<u8> {
    let applied = core.apply(Command::ExportMidi);
    match &applied.effects[..] {
        [Effect::ExportedMidi { bytes }] => bytes.clone(),
        other => panic!("expected exactly one ExportedMidi effect, got {other:?}"),
    }
}

fn read_vlq(bytes: &[u8], pos: &mut usize) -> u32 {
    let mut value = 0u32;
    loop {
        let byte = bytes[*pos];
        *pos += 1;
        value = (value << 7) | u32::from(byte & 0x7F);
        if byte & 0x80 == 0 {
            break;
        }
    }
    value
}

#[derive(Debug, PartialEq, Eq)]
struct DecodedEvent {
    tick: u64,
    bytes: Vec<u8>,
}

/// A minimal, independent decoder — written from the SMF spec, not by
/// reusing any of `daw-core`'s own encoding logic — so a self-consistent
/// bug in the encoder can't also hide from the test that checks it.
fn decode(midi: &[u8]) -> (u16, u16, u16, Vec<DecodedEvent>) {
    assert_eq!(&midi[0..4], b"MThd", "file must open with an MThd chunk");
    let header_length = u32::from_be_bytes(midi[4..8].try_into().unwrap());
    assert_eq!(header_length, 6);
    let format = u16::from_be_bytes(midi[8..10].try_into().unwrap());
    let track_count = u16::from_be_bytes(midi[10..12].try_into().unwrap());
    let division = u16::from_be_bytes(midi[12..14].try_into().unwrap());

    let mut pos = 14;
    assert_eq!(&midi[pos..pos + 4], b"MTrk", "must have an MTrk chunk next");
    pos += 4;
    let track_length = u32::from_be_bytes(midi[pos..pos + 4].try_into().unwrap()) as usize;
    pos += 4;
    let track_end = pos + track_length;
    assert_eq!(
        track_end,
        midi.len(),
        "MTrk length must match the file's actual size"
    );

    let mut tick = 0u64;
    let mut events = Vec::new();
    while pos < track_end {
        let delta = read_vlq(midi, &mut pos);
        tick += u64::from(delta);
        let status = midi[pos];
        if status == 0xFF {
            let meta_type = midi[pos + 1];
            let mut length_pos = pos + 2;
            let length = read_vlq(midi, &mut length_pos) as usize;
            let event_end = length_pos + length;
            events.push(DecodedEvent {
                tick,
                bytes: midi[pos..event_end].to_vec(),
            });
            pos = event_end;
            if meta_type == 0x2F {
                break; // End of track.
            }
        } else {
            events.push(DecodedEvent {
                tick,
                bytes: midi[pos..pos + 3].to_vec(),
            });
            pos += 3;
        }
    }

    (format, track_count, division, events)
}

#[test]
fn exporting_an_empty_timeline_reports_nothing_to_export() {
    let mut core = DawCore::new();

    let applied = core.apply(Command::ExportMidi);

    assert_eq!(applied.effects, vec![Effect::NothingToExport]);
}

#[test]
fn the_file_header_declares_format_zero_one_track_and_the_expected_division() {
    let mut core = DawCore::new();
    let block = record_and_freeze(&mut core, 60, 4);
    core.apply(Command::AddPlacement {
        block_id: block,
        track: 0,
    });

    let bytes = export(&mut core);

    let (format, track_count, division, _) = decode(&bytes);
    assert_eq!(format, 0);
    assert_eq!(track_count, 1);
    assert_eq!(division, 480);
}

#[test]
fn exported_notes_match_the_flattened_timeline_in_pitch_velocity_position_and_duration() {
    let mut core = DawCore::new();
    let a = record_and_freeze(&mut core, 60, 4);
    let b = record_and_freeze(&mut core, 67, 6);
    core.apply(Command::AddPlacement {
        block_id: a,
        track: 0,
    }); // 0..4
    core.apply(Command::AddPlacement {
        block_id: b,
        track: 0,
    }); // 4..10

    let bytes = export(&mut core);
    let (_, _, _, events) = decode(&bytes);

    let note_events: Vec<&DecodedEvent> = events
        .iter()
        .filter(|e| matches!(e.bytes[0], 0x80..=0x9F))
        .collect();
    // A: Note On @0, Note Off @4*240=960. B: Note On @4*240=960, Note Off @10*240=2400.
    assert_eq!(
        note_events
            .iter()
            .map(|e| (e.tick, e.bytes.clone()))
            .collect::<Vec<_>>(),
        vec![
            (0, vec![0x90, 60, 100]),
            (960, vec![0x80, 60, 0]),
            (960, vec![0x90, 67, 100]),
            (2400, vec![0x80, 67, 0]),
        ]
    );
}

#[test]
fn tempo_and_time_signature_are_written_into_the_file() {
    let mut core = DawCore::new();
    let block = record_and_freeze(&mut core, 60, 4);
    core.apply(Command::AddPlacement {
        block_id: block,
        track: 0,
    });
    core.apply(Command::SetBpm(150));
    core.apply(Command::SetTimeSignature {
        beats_per_bar: 3,
        beat_unit: 8,
    });

    let bytes = export(&mut core);
    let (_, _, _, events) = decode(&bytes);

    let tempo = events
        .iter()
        .find(|e| e.bytes[0..2] == [0xFF, 0x51])
        .expect("a tempo meta event");
    assert_eq!(tempo.tick, 0);
    let microseconds_per_quarter = (u32::from(tempo.bytes[3]) << 16)
        | (u32::from(tempo.bytes[4]) << 8)
        | u32::from(tempo.bytes[5]);
    assert_eq!(microseconds_per_quarter, 60_000_000 / 150);

    let time_sig = events
        .iter()
        .find(|e| e.bytes[0..2] == [0xFF, 0x58])
        .expect("a time signature meta event");
    assert_eq!(time_sig.tick, 0);
    assert_eq!(time_sig.bytes[3], 3); // numerator
    assert_eq!(time_sig.bytes[4], 3); // denominator as a power of two: 8 = 2^3
}

#[test]
fn a_deliberate_gap_shifts_the_exported_note_later_by_exactly_the_gap() {
    let mut core = DawCore::new();
    let a = record_and_freeze(&mut core, 60, 4);
    let b = record_and_freeze(&mut core, 67, 6);
    core.apply(Command::AddPlacement {
        block_id: a,
        track: 0,
    }); // 0..4
    let after_b = core.apply(Command::AddPlacement {
        block_id: b,
        track: 0,
    }); // 4..10
    let b_placement_id = after_b
        .state
        .placements
        .iter()
        .find(|p| p.block_id == b)
        .unwrap()
        .id;
    core.apply(Command::SetPlacementGap {
        id: b_placement_id,
        gap_before: 5,
    }); // B now starts at 4 + 5 = 9

    let bytes = export(&mut core);
    let (_, _, _, events) = decode(&bytes);

    let note_events: Vec<&DecodedEvent> = events
        .iter()
        .filter(|e| matches!(e.bytes[0], 0x80..=0x9F))
        .collect();
    assert_eq!(
        note_events
            .iter()
            .map(|e| (e.tick, e.bytes[1]))
            .collect::<Vec<_>>(),
        vec![
            (0, 60),        // A note on
            (4 * 240, 60),  // A note off
            (9 * 240, 67),  // B note on, delayed by the 5-pulse gap
            (15 * 240, 67), // B note off
        ]
    );
}

#[test]
fn a_trimmed_and_quantised_take_exports_at_its_resolved_notes() {
    let mut core = DawCore::new();
    core.apply(Command::StopRecording(Some(Take::from_raw_notes(vec![
        RecordedNote {
            pitch: 64,
            velocity: 90,
            start_pulse: 1,
            end_pulse: 5,
        },
    ]))));
    core.apply(Command::SetTakeTrim(Trim {
        start_pulse: 1,
        end_pulse: 5,
    }));
    let resolved_notes = core.state().take.as_ref().unwrap().notes();
    let added = core.apply(Command::AddTakeToLibrary);
    let block_id = added.state.blocks[0].id;
    core.apply(Command::AddPlacement { block_id, track: 0 });

    let bytes = export(&mut core);
    let (_, _, _, events) = decode(&bytes);

    let note_events: Vec<&DecodedEvent> = events
        .iter()
        .filter(|e| matches!(e.bytes[0], 0x80..=0x9F))
        .collect();
    assert_eq!(note_events.len(), 2 * resolved_notes.len());
    let note = &resolved_notes[0];
    assert_eq!(note_events[0].bytes, vec![0x90, note.pitch, note.velocity]);
    assert_eq!(note_events[0].tick, note.start_pulse * TICKS_PER_PULSE);
    assert_eq!(note_events[1].bytes, vec![0x80, note.pitch, 0]);
    assert_eq!(note_events[1].tick, note.end_pulse * TICKS_PER_PULSE);
}
