//! Pulse timing stays independent of project structure: tempo maps pulses to
//! wall-clock time, while the time signature only governs audible accents and
//! the one-bar count-in.

use std::time::Duration;

use daw_core::{count_in_length_in_pulses, is_bar_accent, pulse_elapsed_time};

#[test]
fn changing_tempo_rescales_playback_time_without_rewriting_pulse_offsets() {
    let stored_note_pulse = 12;

    assert_eq!(
        pulse_elapsed_time(stored_note_pulse, 120),
        Some(Duration::from_secs(3))
    );
    assert_eq!(
        pulse_elapsed_time(stored_note_pulse, 60),
        Some(Duration::from_secs(6))
    );
    assert_eq!(stored_note_pulse, 12);
}

#[test]
fn changing_time_signature_only_changes_bar_accents_and_count_in_length() {
    let pulse = 8;

    assert!(!is_bar_accent(pulse, (3, 4)));
    assert!(is_bar_accent(pulse, (4, 4)));
    assert_eq!(count_in_length_in_pulses((3, 4)), 6);
    assert_eq!(count_in_length_in_pulses((4, 4)), 8);
}
