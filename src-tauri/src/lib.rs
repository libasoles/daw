//! The Tauri shell.
//!
//! Deliberately thin: it opens the window and, later, owns the ports that
//! `daw-core` cannot own itself (MIDI input, synthesis, audio output,
//! storage). No musical logic lives here — it belongs in `daw-core`, where it
//! can be tested without a window.

use std::sync::Mutex;

use daw_core::DawCore;
use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // The single source of truth for the whole application. Issue #3
            // adds the commands the webview uses to drive it; until then it is
            // simply constructed, which is enough to keep the shell honest
            // about which crate owns the state.
            app.manage(Mutex::new(DawCore::new()));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
