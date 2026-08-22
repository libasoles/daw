//! The Tauri shell.
//!
//! Deliberately thin: it opens the window, hosts the single `DawCore`
//! instance behind a mutex, and exposes it to the webview as IPC commands. No
//! musical logic lives here — every one of these handlers just forwards a
//! `daw_core::Command` to `DawCore::apply` and hands the result back. It knows
//! nothing about *what* a command does, only how to carry one across the IPC
//! boundary; that keeps the surface stable as `daw-core` grows new `Command`
//! variants in later tickets.

use std::sync::Mutex;

use daw_core::{Applied, Command, DawCore, ProjectState};
use tauri::{Manager, State};

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

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // The single source of truth for the whole application, behind a
            // mutex because Tauri command handlers run on the webview's IPC
            // threads and DawCore is neither Sync nor meant to be shared
            // without one.
            app.manage(Mutex::new(DawCore::new()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![apply_command, project_state])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
