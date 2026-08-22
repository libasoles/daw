//! Buffers live MIDI into a take while a recording is armed (issue #8).
//!
//! Deliberately shell-only: `DawCore` performs no I/O and cannot listen for
//! MIDI, so this owns the wall-clock anchor (the take's pulse zero) and
//! hands `daw_core::build_take` the raw stream once recording stops. The
//! anchor is `None` until any count-in has finished, so notes played during
//! the count-in are never captured — see `begin_session`.

use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use daw_core::ports::MidiEventKind;
use daw_core::{
    build_take, count_in_length_in_pulses, pulse_elapsed_time, CapturedEvent, ProjectState, Take,
};
use tauri::{AppHandle, Manager};

use crate::AudioEngineHandle;

/// Tauri-managed recording state: `None` when nothing is being captured.
pub struct RecordingHandle(pub Mutex<Option<RecordingSession>>);

pub struct RecordingSession {
    /// Pulse zero of the take being captured. `None` while a count-in is
    /// still playing; events observed before it is set belong to the
    /// count-in, not the take, and are discarded by `capture_live_note`.
    anchor: Option<Instant>,
    events: Vec<CapturedEvent>,
}

/// Opens a fresh capture buffer right after `StartRecording` succeeds. If a
/// count-in is enabled, it is made audible by temporarily forcing the
/// metronome click on (regardless of the standing metronome toggle) for
/// exactly one bar, and the buffer's anchor opens only once that finishes;
/// otherwise the anchor opens immediately.
pub fn begin_session(app: &AppHandle, state: &ProjectState) {
    let handle = app.state::<RecordingHandle>();
    *handle.0.lock().expect("recording mutex poisoned") = Some(RecordingSession {
        anchor: None,
        events: Vec::new(),
    });

    let count_in = state
        .count_in_enabled
        .then(|| pulse_elapsed_time(count_in_length_in_pulses(state.time_signature), state.bpm));

    match count_in.flatten() {
        Some(duration) if duration > Duration::ZERO => {
            sync_audio_engine(app, state, true);
            let app = app.clone();
            let state = state.clone();
            thread::spawn(move || {
                thread::sleep(duration);
                sync_audio_engine(&app, &state, state.metronome_enabled);
                open_anchor(&app);
            });
        }
        _ => open_anchor(app),
    }
}

/// Pushes the audio engine's control state, overriding whether the click
/// sounds — used to force the count-in click on and then restore the
/// project's actual metronome setting once it's done.
fn sync_audio_engine(app: &AppHandle, state: &ProjectState, metronome_enabled: bool) {
    let engine = app.state::<AudioEngineHandle>();
    let mut engine = engine.0.lock().expect("audio engine mutex poisoned");
    if let Some(engine) = engine.as_mut() {
        let _ = engine.configure(
            state.instrument,
            state.reverb,
            state.bpm,
            state.time_signature,
            metronome_enabled,
            state.loop_enabled,
        );
    }
}

fn open_anchor(app: &AppHandle) {
    let handle = app.state::<RecordingHandle>();
    let mut guard = handle.0.lock().expect("recording mutex poisoned");
    if let Some(session) = guard.as_mut() {
        session.anchor = Some(Instant::now());
    }
}

/// Called from the native MIDI callback (`midi::route_live_event`) for every
/// note on/off observed while a session is open. A no-op outside recording,
/// or before the count-in has finished.
pub fn capture_live_note(app: &AppHandle, kind: MidiEventKind, received_at: Instant) {
    let handle = app.state::<RecordingHandle>();
    let mut guard = handle.0.lock().expect("recording mutex poisoned");
    let Some(session) = guard.as_mut() else {
        return;
    };
    let Some(anchor) = session.anchor else {
        return;
    };
    if received_at < anchor {
        return;
    }

    let (is_on, pitch, velocity) = match kind {
        MidiEventKind::NoteOn { pitch, velocity } => (true, pitch, velocity),
        MidiEventKind::NoteOff { pitch } => (false, pitch, 0),
    };
    session.events.push(CapturedEvent {
        elapsed: received_at.duration_since(anchor),
        pitch,
        velocity,
        is_on,
    });
}

/// Closes the session (if any) and turns its raw capture into a [`Take`].
/// Notes still held at this moment are captured as ending here, per
/// [`build_take`]. Called once, when the user stops recording.
pub fn finish_session(app: &AppHandle, bpm: u16) -> Take {
    let handle = app.state::<RecordingHandle>();
    let session = handle.0.lock().expect("recording mutex poisoned").take();
    match session {
        Some(session) => {
            let stopped_at = session
                .anchor
                .map(|anchor| anchor.elapsed())
                .unwrap_or(Duration::ZERO);
            build_take(&session.events, stopped_at, bpm)
        }
        // Stopped before the count-in even finished: nothing was captured.
        None => Take::default(),
    }
}
