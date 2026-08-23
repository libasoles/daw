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
mod project_storage;
mod recording;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use audio::{AudioEngine, ScheduledEvent};
use daw_core::{
    ports::Storage, pulse_elapsed_time, Applied, Command, DawCore, Effect, ProjectState, Take,
};
use midi::{load_selected_device, spawn_reconnector, MidiInputHandle, MidiStatus};
use project_storage::FileStorage;
use recording::RecordingHandle;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

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

/// Bumped every time playback starts or is explicitly stopped (issue #18).
/// `play_take_schedule`'s completion timer captures the value current when
/// it starts and only applies `Command::PlaybackFinished` if nothing has
/// bumped it since — otherwise a `stop_playback` call (or a fresh playback
/// started right after) would leave a stale timer free to apply
/// `PlaybackFinished` to whatever plays *next*, well after the schedule it
/// was actually timing has already ended.
struct PlaybackGeneration(AtomicU64);

struct ProjectStorage(Mutex<FileStorage>);
struct CurrentProjectName(Mutex<Option<String>>);

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
    let starting_recording = matches!(command, Command::StartRecording);
    let opening_new_project = matches!(command, Command::NewProject { .. });
    // Only timeline playback (issue #19) may loop; an isolated take or
    // block never does, whatever the project's loop setting is.
    let is_timeline_playback = matches!(command, Command::PlayTimeline);

    let applied = {
        let core = app.state::<Mutex<DawCore>>();
        let mut core = core.lock().expect("DawCore mutex poisoned");
        core.apply(command)
    };
    let refused = applied
        .effects
        .contains(&Effect::ConfirmDiscardUnsavedChanges);

    // A new project has no name yet; forget whatever name the previous
    // project was saved under, so the next save asks again rather than
    // silently overwriting it. Only when the core actually went ahead —
    // a refused (unsaved-changes-gated) `NewProject` leaves the current
    // project, and its name, untouched.
    if opening_new_project && !refused {
        *app.state::<CurrentProjectName>()
            .0
            .lock()
            .expect("project name mutex poisoned") = None;
    }

    sync_audio_engine(app, &applied.state, applied.state.metronome_enabled);

    if starting_recording && applied.state.is_recording {
        recording::begin_session(app, &applied.state);
    }

    for effect in &applied.effects {
        if let Effect::PlaySchedule(take) = effect {
            play_take_schedule(app, take, applied.state.bpm, is_timeline_playback);
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
            state.loop_enabled,
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
///
/// `loopable` marks whether this schedule may be a looping one (issue #19)
/// — only ever `true` for timeline playback, never an isolated take or
/// block. When it is, and the project's loop setting is on, the completion
/// timer re-arms itself for another pass instead of finishing — read fresh
/// each time it would otherwise fire, so toggling loop mid-playback takes
/// effect on the current pass. The audio itself loops seamlessly inside the
/// synth thread regardless of this timer; this only decides when the shell
/// should stop pretending playback is still running.
fn play_take_schedule(app: &AppHandle, take: &Take, bpm: u16, loopable: bool) {
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
    let last_pulse = notes.iter().map(|note| note.end_pulse).max().unwrap_or(0);
    let total_duration = pulse_elapsed_time(last_pulse, bpm).unwrap_or(Duration::ZERO);

    let started = {
        let engine = app.state::<AudioEngineHandle>();
        let mut engine = engine.0.lock().expect("audio engine mutex poisoned");
        match engine.as_mut() {
            Some(engine) => engine.play_schedule(events, loopable).is_ok(),
            None => false,
        }
    };

    // See `PlaybackGeneration`'s doc comment: this schedule's own generation,
    // captured now so the completion timer below can tell whether it's still
    // the one in charge by the time it wakes up.
    let generation = app
        .state::<PlaybackGeneration>()
        .0
        .fetch_add(1, Ordering::SeqCst)
        + 1;

    // Always resolves eventually — immediately if there was nothing to play,
    // otherwise after the schedule's own length (repeatedly, while looping)
    // — so `is_playing` can never get stuck true with no audio actually
    // sounding.
    let app = app.clone();
    thread::spawn(move || {
        if !started {
            dispatch(&app, Command::PlaybackFinished);
            return;
        }
        loop {
            thread::sleep(total_duration);
            if app.state::<PlaybackGeneration>().0.load(Ordering::SeqCst) != generation {
                // Superseded by an explicit stop or a fresh playback — not
                // this timer's schedule to finish.
                return;
            }
            let loop_enabled = {
                let core = app.state::<Mutex<DawCore>>();
                let core = core.lock().expect("DawCore mutex poisoned");
                core.state().loop_enabled
            };
            if !(loopable && loop_enabled) {
                dispatch(&app, Command::PlaybackFinished);
                return;
            }
        }
    });
}

/// Stops whatever is currently playing (issue #18's "`Space` stops
/// playback"): silences the audio engine's active schedule immediately and
/// applies `Command::PlaybackFinished` right away, rather than waiting for
/// `play_take_schedule`'s timer — which this also invalidates, via
/// `PlaybackGeneration`, so it can't later reapply `PlaybackFinished` to
/// whatever plays next. A no-op if nothing is playing.
#[tauri::command]
fn stop_playback(app: AppHandle) -> Applied {
    let is_playing = {
        let core = app.state::<Mutex<DawCore>>();
        let core = core.lock().expect("DawCore mutex poisoned");
        core.state().is_playing
    };
    if !is_playing {
        let core = app.state::<Mutex<DawCore>>();
        let core = core.lock().expect("DawCore mutex poisoned");
        return Applied {
            state: core.state().clone(),
            effects: Vec::new(),
        };
    }

    app.state::<PlaybackGeneration>()
        .0
        .fetch_add(1, Ordering::SeqCst);
    {
        let engine = app.state::<AudioEngineHandle>();
        let mut engine = engine.0.lock().expect("audio engine mutex poisoned");
        if let Some(engine) = engine.as_mut() {
            let _ = engine.stop_schedule();
        }
    }
    dispatch(&app, Command::PlaybackFinished)
}

/// What the webview should tell the user once `export_midi` returns.
#[derive(serde::Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
enum ExportOutcome {
    /// The file was written to the path the user chose.
    Exported,
    /// The core reported the timeline was empty; no dialog was ever shown.
    NothingToExport,
    /// The core had bytes ready, but the user closed the save dialog
    /// without choosing a location.
    Cancelled,
}

/// Exports the timeline as a `.mid` file (issue #24): asks `daw-core` for
/// the encoded bytes, then — the one step the webview cannot do itself —
/// opens a native "Save As" dialog and writes them wherever the user
/// chooses. Unlike `save_project`, this deliberately does go through a
/// native file picker: a `.mid` export is a piece taken *elsewhere*, not
/// part of the app's own managed project storage.
#[tauri::command]
fn export_midi(app: AppHandle) -> Result<ExportOutcome, String> {
    let applied = dispatch(&app, Command::ExportMidi);
    for effect in &applied.effects {
        match effect {
            Effect::NothingToExport => return Ok(ExportOutcome::NothingToExport),
            Effect::ExportedMidi { bytes } => {
                let Some(chosen) = app
                    .dialog()
                    .file()
                    .add_filter("MIDI", &["mid"])
                    .set_file_name("timeline.mid")
                    .blocking_save_file()
                else {
                    return Ok(ExportOutcome::Cancelled);
                };
                let path = chosen
                    .into_path()
                    .map_err(|error| format!("invalid save location: {error}"))?;
                std::fs::write(&path, bytes)
                    .map_err(|error| format!("could not write {}: {error}", path.display()))?;
                return Ok(ExportOutcome::Exported);
            }
            _ => {}
        }
    }
    unreachable!("Command::ExportMidi always reports NothingToExport or ExportedMidi")
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

/// Writes the current document under its existing name, or the supplied name
/// on its first save. This stays entirely inside the app-data directory; no
/// native file picker is involved.
#[tauri::command]
fn save_project(app: AppHandle, requested_name: Option<String>) -> Result<ProjectState, String> {
    let name = {
        let current = app.state::<CurrentProjectName>();
        let current = current.0.lock().expect("project name mutex poisoned");
        requested_name.or_else(|| current.clone())
    }
    .ok_or_else(|| "a project name is required for the first save".to_string())?;

    let request = {
        let core = app.state::<Mutex<DawCore>>();
        let mut core = core.lock().expect("DawCore mutex poisoned");
        core.apply(Command::SaveProject(name.clone()))
    };
    let Effect::SaveProject { name, document } = request
        .effects
        .into_iter()
        .find(|effect| matches!(effect, Effect::SaveProject { .. }))
        .ok_or_else(|| "save command did not request storage".to_string())?
    else {
        unreachable!("the match above only accepts save effects")
    };
    let encoded_document = serde_json::to_string(&document)
        .map_err(|error| format!("could not encode project: {error}"))?;

    {
        let storage = app.state::<ProjectStorage>();
        let mut storage = storage.0.lock().expect("project storage mutex poisoned");
        storage.save(&name, encoded_document)?;
        // A manual save is now the newest record of this work, so the
        // crash-recovery snapshot (issue #15) — meant only to cover the gap
        // between saves — no longer has anything to add.
        storage.delete_snapshot()?;
    }

    let state = {
        let core = app.state::<Mutex<DawCore>>();
        let mut core = core.lock().expect("DawCore mutex poisoned");
        core.apply(Command::ProjectSaved(document)).state
    };
    *app.state::<CurrentProjectName>()
        .0
        .lock()
        .expect("project name mutex poisoned") = Some(name);
    Ok(state)
}

#[tauri::command]
fn list_projects(app: AppHandle) -> Result<Vec<String>, String> {
    app.state::<ProjectStorage>()
        .0
        .lock()
        .expect("project storage mutex poisoned")
        .list()
}

/// Reads and opens a saved project. Gated by unsaved changes exactly like
/// `Command::OpenProject` itself: with `force: false` and a dirty current
/// project, the core refuses (reporting `Effect::ConfirmDiscardUnsavedChanges`
/// in the returned `Applied`) and this leaves `CurrentProjectName` untouched,
/// so a refused open never makes the shell think a different project is
/// active.
#[tauri::command]
fn open_project(app: AppHandle, name: String, force: bool) -> Result<Applied, String> {
    let document = app
        .state::<ProjectStorage>()
        .0
        .lock()
        .expect("project storage mutex poisoned")
        .load(&name)?
        .ok_or_else(|| format!("project '{name}' was not found"))?;
    let project = serde_json::from_str(&document)
        .map_err(|error| format!("could not read project '{name}': {error}"))?;
    let applied = dispatch(
        &app,
        Command::OpenProject {
            document: project,
            force,
        },
    );
    if !applied
        .effects
        .contains(&Effect::ConfirmDiscardUnsavedChanges)
    {
        *app.state::<CurrentProjectName>()
            .0
            .lock()
            .expect("project name mutex poisoned") = Some(name);
    }
    Ok(applied)
}

/// A crash-recovery snapshot (issue #15) as read from or written to storage:
/// the durable project document plus whatever name it was saved under, if
/// any — `ProjectState` itself carries no name, that lives only in the
/// shell's `CurrentProjectName`.
#[derive(serde::Serialize, serde::Deserialize)]
struct RecoverySnapshot {
    project_name: Option<String>,
    document: ProjectState,
}

/// Reports the crash-recovery snapshot found at launch, if any, so the
/// webview can ask the user whether to recover it. Reading never consumes
/// the snapshot — only `resolve_recovery` does that, once the user has
/// actually decided.
#[tauri::command]
fn recovery_snapshot(app: AppHandle) -> Result<Option<RecoverySnapshot>, String> {
    let raw = app
        .state::<ProjectStorage>()
        .0
        .lock()
        .expect("project storage mutex poisoned")
        .load_snapshot()?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    serde_json::from_str(&raw)
        .map(Some)
        .map_err(|error| format!("could not read recovery snapshot: {error}"))
}

/// Acts on the user's recovery decision. Accepting restores the snapshot's
/// session — including its project name, and, per the spec, still reporting
/// unsaved changes since a snapshot was never a manual save — via
/// `Command::RecoverProject`. Either way the snapshot is deleted: declining
/// discards it outright, and accepting has now folded it into the running
/// session, so leaving the file behind would only offer to "recover" the
/// same interruption again next launch.
#[tauri::command]
fn resolve_recovery(app: AppHandle, accept: bool) -> Result<Applied, String> {
    let raw = app
        .state::<ProjectStorage>()
        .0
        .lock()
        .expect("project storage mutex poisoned")
        .load_snapshot()?;

    let applied = if accept {
        if let Some(raw) = raw {
            let snapshot: RecoverySnapshot = serde_json::from_str(&raw)
                .map_err(|error| format!("could not read recovery snapshot: {error}"))?;
            let applied = dispatch(&app, Command::RecoverProject(snapshot.document));
            *app.state::<CurrentProjectName>()
                .0
                .lock()
                .expect("project name mutex poisoned") = snapshot.project_name;
            applied
        } else {
            project_state_applied(&app)
        }
    } else {
        project_state_applied(&app)
    };

    app.state::<ProjectStorage>()
        .0
        .lock()
        .expect("project storage mutex poisoned")
        .delete_snapshot()?;

    Ok(applied)
}

fn project_state_applied(app: &AppHandle) -> Applied {
    let core = app.state::<Mutex<DawCore>>();
    let core = core.lock().expect("DawCore mutex poisoned");
    Applied {
        state: core.state().clone(),
        effects: Vec::new(),
    }
}

/// How often a crash-recovery snapshot is written while there are unsaved
/// changes. "Roughly every ten seconds", per the spec — this is a recovery
/// aid, not a save, so it doesn't need to be exact.
const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(10);

/// Runs for the life of the app, writing a recovery snapshot on every tick
/// while the project is dirty. Never runs while the project is clean: a
/// snapshot only exists to cover the gap since the last manual save, and
/// `save_project` deletes it the moment that gap closes.
fn spawn_snapshot_writer(app: AppHandle) {
    thread::spawn(move || loop {
        thread::sleep(SNAPSHOT_INTERVAL);

        let document = {
            let core = app.state::<Mutex<DawCore>>();
            let core = core.lock().expect("DawCore mutex poisoned");
            if !core.state().is_dirty {
                continue;
            }
            core.project_document()
        };
        let project_name = app
            .state::<CurrentProjectName>()
            .0
            .lock()
            .expect("project name mutex poisoned")
            .clone();

        let snapshot = RecoverySnapshot {
            project_name,
            document,
        };
        let serialized = match serde_json::to_string(&snapshot) {
            Ok(serialized) => serialized,
            Err(error) => {
                eprintln!("could not encode recovery snapshot: {error}");
                continue;
            }
        };

        let storage = app.state::<ProjectStorage>();
        let mut storage = storage.0.lock().expect("project storage mutex poisoned");
        if let Err(error) = storage.save_snapshot(serialized) {
            eprintln!("could not write recovery snapshot: {error}");
        }
    });
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

/// Whether the audio engine is available, checked once at boot so the
/// webview can report a missing output device up front instead of only
/// after the user clicks "Play test note".
#[derive(serde::Serialize)]
struct AudioStatus {
    available: bool,
    message: String,
}

#[tauri::command]
fn audio_status(app: AppHandle) -> AudioStatus {
    let engine = app.state::<AudioEngineHandle>();
    let engine = engine.0.lock().expect("audio engine mutex poisoned");
    if engine.is_some() {
        AudioStatus { available: true, message: "audio ready".to_string() }
    } else {
        AudioStatus {
            available: false,
            message: "no audio output device is available".to_string(),
        }
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // The single source of truth for the whole application, behind a
            // mutex because Tauri command handlers run on the webview's IPC
            // threads and DawCore is neither Sync nor meant to be shared
            // without one.
            app.manage(Mutex::new(DawCore::new()));
            let app_data_dir = app.path().app_data_dir()?;
            app.manage(ProjectStorage(Mutex::new(FileStorage::new(app_data_dir))));
            app.manage(CurrentProjectName(Mutex::new(None)));

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
                        state.loop_enabled,
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
            app.manage(PlaybackGeneration(AtomicU64::new(0)));
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
            spawn_snapshot_writer(app.handle().clone());

            Ok(())
        })
        .on_window_event(|window, event| {
            // A clean quit — including a close the user confirmed past the
            // unsaved-changes warning (#14) — deletes the crash-recovery
            // snapshot (#15): there is nothing left to recover from once the
            // app has shut down in an orderly way. A crash never reaches
            // this handler at all, which is exactly what leaves the
            // snapshot behind for the next launch to offer.
            if matches!(event, tauri::WindowEvent::Destroyed) {
                let storage = window.app_handle().state::<ProjectStorage>();
                let mut storage = storage.0.lock().expect("project storage mutex poisoned");
                if let Err(error) = storage.delete_snapshot() {
                    eprintln!("could not delete recovery snapshot: {error}");
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            apply_command,
            project_state,
            save_project,
            list_projects,
            open_project,
            recovery_snapshot,
            resolve_recovery,
            play_test_note,
            audio_status,
            stop_recording,
            stop_playback,
            export_midi,
            list_midi_devices,
            select_midi_device
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
