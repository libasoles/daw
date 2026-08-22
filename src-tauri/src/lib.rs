//! The Tauri shell.
//!
//! Deliberately thin: it opens the window, hosts the single `DawCore`
//! instance behind a mutex, and exposes it to the webview as IPC commands. No
//! musical logic lives here — every one of these handlers just forwards a
//! `daw_core::Command` to `DawCore::apply` and hands the result back. It knows
//! nothing about *what* a command does, only how to carry one across the IPC
//! boundary; that keeps the surface stable as `daw-core` grows new `Command`
//! variants in later tickets.

mod audio;
mod midi;
mod recording;

use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use audio::{AudioEngine, ScheduledEvent};
use daw_core::{pulse_elapsed_time, Applied, Command, DawCore, Effect, ProjectState, Take};
use midi::{load_selected_device, spawn_reconnector, MidiInputHandle, MidiStatus};
use recording::RecordingHandle;
use tauri::{AppHandle, Manager, State};

/// The fixed note the debug trigger sounds. There is no MIDI input yet
/// (issue #6), so this remains the only way to prove sound reaches the
/// speakers; it uses the current global instrument and reverb configuration.
const DEBUG_NOTE_PITCH: u8 = 60;
const DEBUG_NOTE_VELOCITY: u8 = 100;
const DEBUG_NOTE_DURATION: Duration = Duration::from_millis(500);

/// The audio engine, if one could be started. `None` when no output device
/// was available at launch (or the bundled SoundFont failed to load) — the
/// spec calls for reporting that rather than crashing, so the app keeps
/// running with sound simply unavailable.
struct AudioEngineHandle(Mutex<Option<AudioEngine>>);

/// Lists native MIDI inputs and the live connection state for the picker.
#[tauri::command]
fn list_midi_devices(app: AppHandle) -> MidiStatus {
    let handle = app.state::<MidiInputHandle>();
    let mut controller = handle.0.lock().expect("MIDI input mutex poisoned");
    controller.refresh(&app);
    controller.status()
}

/// Selects a MIDI input. The id is stored in app data, not project state, and
/// the reconnect poller will use it automatically at future launches.
#[tauri::command]
fn select_midi_device(app: AppHandle, device_id: String) -> Result<MidiStatus, String> {
    let handle = app.state::<MidiInputHandle>();
    let mut controller = handle.0.lock().expect("MIDI input mutex poisoned");
    controller.select(&app, device_id)?;
    Ok(controller.status())
}

/// The webview's one way in: turn a `Command` into the new `ProjectState` and
/// any `Effect`s, via `DawCore::apply`. A single generic handler (rather than
/// one `#[tauri::command]` per `Command` variant) mirrors the core's own
/// seam — `Command -> DawCore -> (ProjectState, Vec<Effect>)` — so adding a
/// `Command` variant in a later ticket never means adding IPC surface here.
///
/// Two commands (`StopRecording`, in `stop_recording` below) can't be built
/// by the webview at all — turning raw MIDI into a `Take` needs the native
/// capture state only the shell holds — so they go through their own
/// dedicated command and call [`dispatch`] directly instead.
#[tauri::command]
fn apply_command(app: AppHandle, command: Command) -> Applied {
    dispatch(&app, command)
}

/// Shared by every entry point that turns a `Command` into an `Applied`:
/// applies it to the single `DawCore`, pushes the resulting sound controls
/// to the audio engine, and carries out any effect the core asked for that
/// the shell — not the webview — is responsible for.
fn dispatch(app: &AppHandle, command: Command) -> Applied {
    let starting_recording = matches!(command, Command::StartRecording { .. });

    let applied = {
        let core = app.state::<Mutex<DawCore>>();
        let mut core = core.lock().expect("DawCore mutex poisoned");
        core.apply(command)
    };

    sync_audio_engine(app, &applied.state, applied.state.metronome_enabled);

    if starting_recording && applied.state.is_recording {
        recording::begin_session(app, &applied.state);
    }

    for effect in &applied.effects {
        if let Effect::PlaySchedule(take) = effect {
            play_take_schedule(app, take, applied.state.bpm);
        }
    }

    applied
}

fn sync_audio_engine(app: &AppHandle, state: &ProjectState, metronome_enabled: bool) {
    let engine = app.state::<AudioEngineHandle>();
    let mut engine = engine.0.lock().expect("audio engine mutex poisoned");
    if let Some(engine) = engine.as_mut() {
        if let Err(err) = engine.configure(
            state.instrument,
            state.reverb,
            state.bpm,
            state.time_signature,
            metronome_enabled,
        ) {
            eprintln!("could not update audio controls: {err}");
        }
    }
}

/// Turns a take into a schedule the audio engine can play frame-accurately
/// (see `audio::engine`'s synth-thread scheduling), and arranges for
/// `Command::PlaybackFinished` to be applied once it's done — computed from
/// the take's own length rather than any signal back from the audio thread,
/// the same way `play_test_note` already turns a fixed hold into a timed
/// note-off on a plain thread.
fn play_take_schedule(app: &AppHandle, take: &Take, bpm: u16) {
    let notes = take.notes();
    let events: Vec<ScheduledEvent> = notes
        .iter()
        .flat_map(|note| {
            [
                ScheduledEvent {
                    at_pulse: note.start_pulse,
                    pitch: note.pitch,
                    velocity: note.velocity,
                    is_on: true,
                },
                ScheduledEvent {
                    at_pulse: note.end_pulse,
                    pitch: note.pitch,
                    velocity: 0,
                    is_on: false,
                },
            ]
        })
        .collect();
    let last_pulse = notes
        .iter()
        .map(|note| note.end_pulse)
        .max()
        .unwrap_or(0);
    let total_duration = pulse_elapsed_time(last_pulse, bpm).unwrap_or(Duration::ZERO);

    let started = {
        let engine = app.state::<AudioEngineHandle>();
        let mut engine = engine.0.lock().expect("audio engine mutex poisoned");
        match engine.as_mut() {
            Some(engine) => engine.play_schedule(events).is_ok(),
            None => false,
        }
    };

    // Always resolves eventually — immediately if there was nothing to play,
    // otherwise after the schedule's own length — so `is_playing` can never
    // get stuck true with no audio actually sounding.
    let app = app.clone();
    thread::spawn(move || {
        if started {
            thread::sleep(total_duration);
        }
        dispatch(&app, Command::PlaybackFinished);
    });
}

/// Finishes recording: turns the shell's buffered MIDI capture into a
/// `Take` and applies it as `Command::StopRecording`, the one step the
/// webview cannot request through the generic `apply_command` (it never
/// sees the raw MIDI the native connection captured).
#[tauri::command]
fn stop_recording(app: AppHandle) -> Applied {
    let bpm = {
        let core = app.state::<Mutex<DawCore>>();
        let core = core.lock().expect("DawCore mutex poisoned");
        core.state().bpm
    };
    let take = recording::finish_session(&app, bpm);
    dispatch(&app, Command::StopRecording(Some(take)))
}

/// The project state as it stands right now, for the webview to render on
/// load without having to invent a no-op command just to read it.
#[tauri::command]
fn project_state(core: State<Mutex<DawCore>>) -> ProjectState {
    let core = core.lock().expect("DawCore mutex poisoned");
    core.state().clone()
}

/// Sounds a single fixed note for manual/test verification, since neither
/// MIDI input (#6) nor the instrument dropdown (#5) exist yet to trigger one
/// another way. Turns note-on into note-off after a fixed hold, on a plain
/// thread — never the real-time audio thread, which only renders.
///
/// Returns an error (surfaced to the webview as a rejected promise) rather
/// than panicking when no audio output device is available, per the spec's
/// acceptance criterion.
#[tauri::command]
fn play_test_note(app: AppHandle) -> Result<(), String> {
    {
        let engine = app.state::<AudioEngineHandle>();
        let mut engine = engine.0.lock().expect("audio engine mutex poisoned");
        let engine = engine
            .as_mut()
            .ok_or_else(|| "no audio output device is available".to_string())?;
        engine
            .note_on(DEBUG_NOTE_PITCH, DEBUG_NOTE_VELOCITY)
            .map_err(|err| err.to_string())?;
    }

    thread::spawn(move || {
        thread::sleep(DEBUG_NOTE_DURATION);
        let engine = app.state::<AudioEngineHandle>();
        let mut engine = engine.0.lock().expect("audio engine mutex poisoned");
        if let Some(engine) = engine.as_mut() {
            // Best-effort: if the queue is full there is nothing more useful
            // to do than let the note ring out.
            let _ = engine.note_off(DEBUG_NOTE_PITCH);
        }
    });

    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // The single source of truth for the whole application, behind a
            // mutex because Tauri command handlers run on the webview's IPC
            // threads and DawCore is neither Sync nor meant to be shared
            // without one.
            app.manage(Mutex::new(DawCore::new()));

            // A missing output device (or a corrupt bundled SoundFont) must
            // not crash the app — it is reported here and `play_test_note`
            // reports it again at call time, per the spec.
            let engine = match AudioEngine::start() {
                Ok(mut engine) => {
                    // Initialise the synth thread from the same default
                    // project state the UI receives, before any notes play.
                    let state = ProjectState::default();
                    if let Err(err) = engine.configure(
                        state.instrument,
                        state.reverb,
                        state.bpm,
                        state.time_signature,
                        state.metronome_enabled,
                    ) {
                        eprintln!("could not initialise audio controls: {err}");
                    }
                    Some(engine)
                }
                Err(err) => {
                    eprintln!("audio engine unavailable: {err}");
                    None
                }
            };
            app.manage(AudioEngineHandle(Mutex::new(engine)));
            app.manage(RecordingHandle(Mutex::new(None)));

            // The one-line preference lives under the OS app-data directory,
            // deliberately apart from a future project.json. It identifies a
            // local peripheral, so syncing it with musical content would be
            // misleading and fragile across machines.
            let selected_device = load_selected_device(app.handle());
            app.manage(MidiInputHandle(Mutex::new(midi::MidiController::new(
                selected_device,
            ))));
            spawn_reconnector(app.handle().clone());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            apply_command,
            project_state,
            play_test_note,
            stop_recording,
            list_midi_devices,
            select_midi_device
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
