//! Encodes the timeline as a Standard MIDI File (issue #24): a pure,
//! byte-exact writer for SMF format 0 — no filesystem or network access, so
//! it stays inside `daw-core` per ADR-0001. The shell only ever receives
//! finished bytes, to write wherever the user chooses.

use crate::{ProjectState, RecordedNote, PULSES_PER_BEAT};

/// Ticks per quarter note in the exported file. Chosen so a pulse (an
/// eighth note, since [`PULSES_PER_BEAT`] is 2) maps to a whole number of
/// ticks with no rounding: 480 / 2 = 240.
const TICKS_PER_QUARTER_NOTE: u16 = 480;

/// Every note the timeline plays, flattened into one absolute-pulse stream
/// exactly as `Command::PlayTimeline` schedules it for playback — trims,
/// quantisation and deliberate gaps are already resolved into each
/// placement's stored notes and `start_pulse` by the time this runs, so
/// there is nothing left for the exporter to reapply; matching playback
/// exactly is a matter of reusing this same flattening, not recomputing it.
pub(crate) fn flattened_notes(state: &ProjectState) -> Vec<RecordedNote> {
    state
        .placements
        .iter()
        .flat_map(|placement| {
            placement.notes.iter().map(move |note| RecordedNote {
                pitch: note.pitch,
                velocity: note.velocity,
                start_pulse: placement.start_pulse + note.start_pulse,
                end_pulse: placement.start_pulse + note.end_pulse,
            })
        })
        .collect()
}

/// Encodes `state`'s timeline as a complete Standard MIDI File: a header
/// chunk (format 0, one track) followed by one track chunk carrying the
/// tempo, the time signature, and a Note On/Note Off pair for every
/// flattened note, in the exact pitch/velocity/position/duration playback
/// itself uses. Callers should skip this entirely for an empty timeline
/// rather than write a file with no notes in it — see
/// `Command::ExportMidi`'s handler.
pub(crate) fn encode(state: &ProjectState) -> Vec<u8> {
    let ticks_per_pulse = u64::from(TICKS_PER_QUARTER_NOTE) / PULSES_PER_BEAT;

    // (absolute tick, ordering priority, event bytes). Priority breaks ties
    // at the same tick: meta events first, then a Note Off before any Note
    // On, so a placement landing exactly where another ends never reads as
    // an extra zero-length note or a stuck one.
    let mut events: Vec<(u64, u8, Vec<u8>)> = Vec::new();

    let bpm = u32::from(state.bpm.max(1));
    let microseconds_per_quarter_note = 60_000_000u32 / bpm;
    events.push((
        0,
        0,
        vec![
            0xFF,
            0x51,
            0x03,
            (microseconds_per_quarter_note >> 16) as u8,
            (microseconds_per_quarter_note >> 8) as u8,
            microseconds_per_quarter_note as u8,
        ],
    ));

    let (beats_per_bar, beat_unit) = state.time_signature;
    let denominator_power_of_two = beat_unit.max(1).trailing_zeros() as u8;
    events.push((
        0,
        0,
        vec![
            0xFF,
            0x58,
            0x04,
            beats_per_bar,
            denominator_power_of_two,
            24, // MIDI clocks per metronome click — the standard value.
            8,  // Number of 32nd notes per quarter note — always 8.
        ],
    ));

    for note in flattened_notes(state) {
        let start_tick = note.start_pulse * ticks_per_pulse;
        let end_tick = note.end_pulse * ticks_per_pulse;
        let pitch = note.pitch.min(127);
        events.push((start_tick, 1, vec![0x90, pitch, note.velocity.min(127)]));
        events.push((end_tick, 0, vec![0x80, pitch, 0]));
    }

    events.sort_by_key(|(tick, priority, _)| (*tick, *priority));

    let mut track_data = Vec::new();
    let mut previous_tick = 0u64;
    for (tick, _, bytes) in &events {
        write_variable_length_quantity((tick - previous_tick) as u32, &mut track_data);
        track_data.extend_from_slice(bytes);
        previous_tick = *tick;
    }
    track_data.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]); // End of track.

    let mut file = Vec::new();
    file.extend_from_slice(b"MThd");
    file.extend_from_slice(&6u32.to_be_bytes());
    file.extend_from_slice(&0u16.to_be_bytes()); // Format 0: a single track.
    file.extend_from_slice(&1u16.to_be_bytes());
    file.extend_from_slice(&TICKS_PER_QUARTER_NOTE.to_be_bytes());
    file.extend_from_slice(b"MTrk");
    file.extend_from_slice(&(track_data.len() as u32).to_be_bytes());
    file.extend_from_slice(&track_data);
    file
}

/// MIDI's variable-length quantity: `value` split into 7-bit groups,
/// transmitted most-significant group first, with the continuation bit
/// (0x80) set on every byte except the last.
fn write_variable_length_quantity(value: u32, out: &mut Vec<u8>) {
    let mut groups = vec![(value & 0x7F) as u8];
    let mut remaining = value >> 7;
    while remaining > 0 {
        groups.push((remaining & 0x7F) as u8);
        remaining >>= 7;
    }
    for (index, group) in groups.iter().enumerate().rev() {
        out.push(group | if index == 0 { 0 } else { 0x80 });
    }
}
