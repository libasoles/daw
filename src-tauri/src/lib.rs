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

use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use audio::AudioEngine;
use daw_core::ports::InstrumentId;
use daw_core::{Applied, Command, DawCore, ProjectState};
use tauri::{AppHandle, Manager, State};

/// The instrument and note the debug "play a note" trigger sounds. There is
/// no instrument dropdown yet (issue #5) and no MIDI input yet (issue #6),
/// so this ticket's only way to prove sound reaches the speakers is a fixed
/// middle C on the bundled SoundFont's default program.
const DEBUG_NOTE_INSTRUMENT: InstrumentId = 0;
const DEBUG_NOTE_PITCH: u8 = 60;
const DEBUG_NOTE_VELOCITY: u8 = 100;
const DEBUG_NOTE_DURATION: Duration = Duration::from_millis(500);

/// The audio engine, if one could be started. `None` when no output device
/// was available at launch (or the bundled SoundFont failed to load) — the
/// spec calls for reporting that rather than crashing, so the app keeps
/// running with sound simply unavailable.
struct AudioEngineHandle(Mutex<Option<AudioEngine>>);

/// The webview's one way in: turn a `Command` into the new `ProjectState` and
/// any `Effect`s, via `DawCore::apply`. A single generic handler (rather than
/// one `#[tauri::command]` per `Command` variant) mirrors the core's own
/// seam — `Command -> DawCore -> (ProjectState, Vec<Effect>)` — so adding a
/// `Command` variant in a later ticket never means adding IPC surface here.
#[tauri::command]
fn apply_command(core: State<Mutex<DawCore>>, command: Command) -> Applied {
    let mut core = core.lock().expect("DawCore mutex poisoned");
    core.apply(command)
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
            .note_on(DEBUG_NOTE_INSTRUMENT, DEBUG_NOTE_PITCH, DEBUG_NOTE_VELOCITY)
            .map_err(|err| err.to_string())?;
    }

    thread::spawn(move || {
        thread::sleep(DEBUG_NOTE_DURATION);
        let engine = app.state::<AudioEngineHandle>();
        let mut engine = engine.0.lock().expect("audio engine mutex poisoned");
        if let Some(engine) = engine.as_mut() {
            // Best-effort: if the queue is full there is nothing more useful
            // to do than let the note ring out.
            let _ = engine.note_off(DEBUG_NOTE_INSTRUMENT, DEBUG_NOTE_PITCH);
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
                Ok(engine) => Some(engine),
                Err(err) => {
                    eprintln!("audio engine unavailable: {err}");
                    None
                }
            };
            app.manage(AudioEngineHandle(Mutex::new(engine)));

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            apply_command,
            project_state,
            play_test_note
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
