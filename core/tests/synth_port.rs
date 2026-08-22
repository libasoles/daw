//! Exercises the shape of the `Synth` port (issue #4) with a fake, per the
//! spec's testing decisions: "`Synth` is a spy that records which notes it
//! was asked to sound and at which pulse, which is how 'what would be heard'
//! is asserted without audio hardware." There is no sequencer yet to drive
//! this from `DawCore` (that arrives with the timeline, H3), so this test
//! proves the trait itself — its shape, and that a fake satisfying it can be
//! used the way later tickets will use it.

use daw_core::ports::{InstrumentId, Synth};

const PIANO: InstrumentId = 0;

/// One call a test made against the spy, in the order it happened.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SynthCall {
    NoteOn {
        instrument: InstrumentId,
        pitch: u8,
        velocity: u8,
        pulse: u64,
    },
    NoteOff {
        instrument: InstrumentId,
        pitch: u8,
        pulse: u64,
    },
}

/// A fake `Synth` that records every call instead of making sound, so tests
/// of "what would be heard" never need a sound card.
#[derive(Debug, Default)]
struct SpySynth {
    calls: Vec<SynthCall>,
    frames_rendered: usize,
    reverb: u8,
}

impl Synth for SpySynth {
    fn set_reverb(&mut self, reverb: u8) {
        self.reverb = reverb;
    }

    fn note_on(&mut self, instrument: InstrumentId, pitch: u8, velocity: u8, pulse: u64) {
        self.calls.push(SynthCall::NoteOn {
            instrument,
            pitch,
            velocity,
            pulse,
        });
    }

    fn note_off(&mut self, instrument: InstrumentId, pitch: u8, pulse: u64) {
        self.calls.push(SynthCall::NoteOff {
            instrument,
            pitch,
            pulse,
        });
    }

    fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        assert_eq!(
            left.len(),
            right.len(),
            "channels must render equal lengths"
        );
        self.frames_rendered += left.len();
        // A real implementation would fill these with audio; the spy leaves
        // them silent, which is enough to prove `render` was reached.
        left.fill(0.0);
        right.fill(0.0);
    }
}

#[test]
fn spy_records_note_on_with_pitch_velocity_and_pulse() {
    let mut synth = SpySynth::default();

    synth.note_on(PIANO, 60, 100, 480);

    assert_eq!(
        synth.calls,
        vec![SynthCall::NoteOn {
            instrument: PIANO,
            pitch: 60,
            velocity: 100,
            pulse: 480,
        }]
    );
}

#[test]
fn spy_records_the_global_reverb_send() {
    let mut synth = SpySynth::default();

    synth.set_reverb(75);

    assert_eq!(synth.reverb, 75);
}

#[test]
fn spy_records_note_off_with_pitch_and_pulse() {
    let mut synth = SpySynth::default();

    synth.note_on(PIANO, 60, 100, 480);
    synth.note_off(PIANO, 60, 960);

    assert_eq!(
        synth.calls,
        vec![
            SynthCall::NoteOn {
                instrument: PIANO,
                pitch: 60,
                velocity: 100,
                pulse: 480,
            },
            SynthCall::NoteOff {
                instrument: PIANO,
                pitch: 60,
                pulse: 960,
            },
        ]
    );
}

#[test]
fn spy_distinguishes_calls_by_instrument() {
    let mut synth = SpySynth::default();
    const ACCORDION: InstrumentId = 1;

    synth.note_on(PIANO, 60, 100, 0);
    synth.note_on(ACCORDION, 60, 100, 0);

    assert_eq!(
        synth.calls,
        vec![
            SynthCall::NoteOn {
                instrument: PIANO,
                pitch: 60,
                velocity: 100,
                pulse: 0,
            },
            SynthCall::NoteOn {
                instrument: ACCORDION,
                pitch: 60,
                velocity: 100,
                pulse: 0,
            },
        ]
    );
}

#[test]
fn render_fills_the_caller_provided_buffers_without_resizing_them() {
    let mut synth = SpySynth::default();
    let mut left = vec![1.0_f32; 128];
    let mut right = vec![1.0_f32; 128];

    synth.render(&mut left, &mut right);

    assert_eq!(synth.frames_rendered, 128);
    assert_eq!(left.len(), 128);
    assert_eq!(right.len(), 128);
    assert!(left.iter().all(|&sample| sample == 0.0));
    assert!(right.iter().all(|&sample| sample == 0.0));
}

/// A generic helper only compiles if `Synth` really is object-safe /
/// implementable behind `&mut dyn Synth`, which is how the shell will hold
/// whichever concrete synth (spy or `rustysynth`) is in play.
fn trigger_middle_c(synth: &mut dyn Synth, instrument: InstrumentId) {
    synth.note_on(instrument, 60, 100, 0);
    synth.note_off(instrument, 60, 100);
}

#[test]
fn synth_is_usable_as_a_trait_object() {
    let mut synth = SpySynth::default();

    trigger_middle_c(&mut synth, PIANO);

    assert_eq!(synth.calls.len(), 2);
}
